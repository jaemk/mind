//! Data load and poll layer for the TUI.
//!
//! The only module in the TUI that acquires the global lock for reads.
//! Uses a non-blocking shared acquire for the poll tick (TUI-15, TUI-25)
//! so the UI never freezes behind a writer.
//!
//! Change detection: the source-set and manifest are compared across polls.
//! The catalog is only re-scanned when the source set changes (TUI-15).

use std::path::PathBuf;

use crate::catalog;
use crate::config::Config;
use crate::error::{ItemKind, Result};
use crate::hash::hash_path;
use crate::lock;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::sanitize::strip_ansi;
use crate::source::Registry;

/// A snapshot of the TUI's data, built from registry + manifest + catalog.
///
/// Change detection is by VALUE (`PartialEq`), not by a counter: the poll tick
/// compares the freshly loaded snapshot against the last applied one and skips
/// the rebuild when they are equal (TUI-15). An earlier `generation` counter was
/// minted fresh on every load, so it always compared unequal and the gate never
/// fired -- every tick rebuilt the tree even when nothing had changed.
// spec: TUI-15
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub installed: Vec<SnapshotInstalled>,
    pub available: Vec<SnapshotAvailable>,
    /// Unmanaged lobe items: skills/agents/rules present in a configured agent
    /// home that `mind` did not install (UNM-6).
    // spec: UNM-6
    pub unmanaged: Vec<SnapshotUnmanaged>,
    /// Names of all melded sources (for change detection in future: TUI-15).
    #[allow(dead_code)]
    pub source_names: Vec<String>,
    /// Not-yet-melded sources from the suggested registry (TUI-31).
    pub suggestions: Vec<crate::tui::preview::RegistrySuggestion>,
    /// Configured agent homes (lobes) from config.toml (TUI-23).
    // spec: TUI-23
    pub lobes: Vec<String>,
    /// Effective namespace prefix for each melded source (NS-1). The key is the
    /// source name; the value is the effective prefix (`Some(p)`) or `None` when
    /// the source has no prefix. Derived in priority order: consumer alias, then
    /// `[source].prefix` from mind.toml, then none.
    // spec: TUI-53
    pub source_namespaces: std::collections::HashMap<String, Option<String>>,
}

/// One installed item in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInstalled {
    pub key: String,
    pub name: String,
    pub source: String,
    pub kind: ItemKind,
    pub commit: String,
    pub description: Option<String>,
    /// Direct dependency keys (`kind:name`) for TUI-50 dependency subtree.
    // spec: TUI-50
    pub deps: Vec<String>,
    /// Whether `upgrade` would act on this item: its source content hash has
    /// drifted from the recorded manifest hash, or its effective name has
    /// changed (a rename, e.g. a prefix change). Mirrors the CLI-75 outdated
    /// check (commands.rs, three call sites) at the TUI snapshot boundary so
    /// the browse tree and item dialog can show the same drift a user would
    /// see from `recall`, without re-deriving the comparison at draw time.
    // spec: TUI-63
    pub stale: bool,
}

/// One available (catalog) item in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAvailable {
    pub key: String,
    pub name: String,
    pub source: String,
    pub kind: ItemKind,
    pub description: Option<String>,
    pub path: PathBuf,
    /// Direct dependency keys (`kind:name`) for TUI-50 dependency subtree.
    // spec: TUI-50
    pub deps: Vec<String>,
}

/// One unmanaged lobe item in the snapshot (UNM-6). Its `key` is the
/// `kind:name` form so the `forget` action resolves it like a managed ref.
// spec: UNM-6
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotUnmanaged {
    pub key: String,
    pub name: String,
    pub kind: ItemKind,
    pub paths: Vec<PathBuf>,
}

/// Memo of item content hashes for the TUI's staleness check, keyed by item
/// path, holding `(stat_fingerprint, content_hash)`.
///
/// The poll tick runs about once a second and computes TUI-63 staleness for
/// every installed item, which means a full content hash of every item's source
/// tree. For a markdown skill that is trivial; for a `tool` directory carrying a
/// vendored binary it is tens of megabytes re-read per second. The memo replaces
/// that with the same directory walk reading only a cheap stat fingerprint
/// (`hash::stat_fingerprint`: `mtime`, `size`, and -- under `cfg(unix)` -- `ctime`
/// and inode number), and re-reads content only when that fingerprint changes.
///
/// Display-side only (TUI-72): `upgrade`/`introspect`/`recall` all call
/// `hash_path` directly, never this memo, so a missed fingerprint change can
/// never change what a verb ACTS on -- at most it makes the TUI-63 confirm
/// modal list fewer stale items than the no-sync apply (TUI-73) then acts on,
/// since that apply always re-hashes for real. A miss is NOT bounded to "one
/// tick": with no TTL, it is served for the life of the process, until the
/// item's path leaves the current catalog (its source is unmelded, or the item
/// is removed upstream), at which point the next full load's
/// [`prune_hash_memo`] evicts it. `stat_fingerprint`'s `ctime`/inode fields
/// close the realistic miss case -- a same-size, mtime-preserving replace
/// (`cp -p`, `rsync -a`, `tar -p`, `touch -r`) -- but the fingerprint is still
/// not a content hash and must never be compared against a recorded manifest
/// hash.
static HASH_MEMO: std::sync::Mutex<std::collections::BTreeMap<PathBuf, (String, String)>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Bound `HASH_MEMO` to the paths currently present in the catalog (TUI-72,
/// L14): without this, an unmelded source or a removed item leaves its entry
/// in the memo for the life of the process, since the memo is otherwise
/// insert-only and nothing else ever evicts a key. Called once per full load
/// with the freshly scanned catalog's item paths, so the memo's size tracks
/// "items observed in the current catalog", not "every path ever seen this
/// session".
fn prune_hash_memo(live_paths: &std::collections::HashSet<PathBuf>) {
    if let Ok(mut memo) = HASH_MEMO.lock() {
        prune_memo_map(&mut memo, live_paths);
    }
}

/// The actual eviction logic behind [`prune_hash_memo`], split out so it can
/// be unit-tested against a local map rather than the process-global
/// `HASH_MEMO` (which every test in this module shares, so a test that pruned
/// the real static could race a concurrently running test's own `load` call).
fn prune_memo_map(
    memo: &mut std::collections::BTreeMap<PathBuf, (String, String)>,
    live_paths: &std::collections::HashSet<PathBuf>,
) {
    memo.retain(|path, _| live_paths.contains(path));
}

/// The content hash of `path`, reusing the memo when the cheap stat fingerprint
/// is unchanged. `None` on any hash failure, which callers treat as drift
/// (matching `hash_path(..).ok()`, the CLI-75 rule). When the fingerprint itself
/// cannot be computed, this falls back to hashing every time and caches nothing,
/// so an unsupported-mtime platform behaves exactly as before the memo existed.
fn memoized_hash(path: &std::path::Path) -> Option<String> {
    let fingerprint = crate::hash::stat_fingerprint(path).ok();
    if let Some(fp) = &fingerprint
        && let Ok(memo) = HASH_MEMO.lock()
        && let Some((cached_fp, cached_hash)) = memo.get(path)
        && cached_fp == fp
    {
        return Some(cached_hash.clone());
    }
    let hash = hash_path(path).ok()?;
    if let Some(fp) = fingerprint
        && let Ok(mut memo) = HASH_MEMO.lock()
    {
        memo.insert(path.to_path_buf(), (fp, hash.clone()));
    }
    Some(hash)
}

/// Load the initial snapshot under a blocking shared lock (called once at
/// startup). Returns an error if the lock cannot be acquired.
// spec: TUI-25
pub fn load(paths: &Paths) -> Result<Snapshot> {
    let lock = lock::open(paths)?;
    let _guard = lock.read()?;
    load_inner(paths)
}

/// Try to load a refreshed snapshot under a NON-BLOCKING shared lock.
/// Returns `None` if the lock is held exclusively (e.g. a mutation is in
/// progress). The TUI poll tick calls this and silently skips if blocked.
// spec: TUI-15 TUI-25
pub fn try_poll(paths: &Paths) -> Option<Snapshot> {
    let lock = lock::open(paths).ok()?;
    let _guard = lock.try_read()?;
    load_inner(paths).ok()
}

/// Sanitize a set of dependency keys (`kind:name`) through the same TUI-60
/// model boundary as the sibling `name`/`source`/`description` fields on
/// `SnapshotInstalled`/`SnapshotAvailable` -- L15: dep keys were the only
/// source-derived strings on those structs that skipped `strip_ansi`.
/// `catalog::scan` already rejects an unsafe item NAME outright (a control
/// character or bidi/zero-width code point never survives into a `CatalogItem`
/// in the first place), so today this is defense-in-depth rather than a live
/// path: it closes the gap the moment a label reaches a non-ratatui sink, or a
/// future dep-key source that is not name-gated the same way.
fn sanitize_dep_keys(keys: Vec<String>) -> Vec<String> {
    keys.iter().map(|k| strip_ansi(k)).collect()
}

/// Read all of a catalog item's text files into one buffer, for dependency
/// detection (mirrors `commands::read_item_text`, kept local so data.rs stays
/// independent of commands.rs and avoids a cross-module dep).
fn read_item_text(item: &catalog::CatalogItem) -> String {
    let mut buf = String::new();
    for file in crate::review::item_files(item) {
        if let Ok(content) = std::fs::read_to_string(&file) {
            buf.push_str(&content);
            buf.push('\n');
        }
    }
    buf
}

/// Load registry, manifest, and catalog without acquiring the lock (the
/// caller must already hold an appropriate guard).
fn load_inner(paths: &Paths) -> Result<Snapshot> {
    let registry = Registry::load(paths)?;
    let manifest = Manifest::load(paths)?;
    let catalog_items = catalog::scan(paths, &registry)?;

    // spec: TUI-72 - bound HASH_MEMO to the current catalog's paths before it
    // is consulted below, so an unmelded source's or a removed item's stale
    // entry does not linger for the rest of the process.
    let live_paths: std::collections::HashSet<PathBuf> =
        catalog_items.iter().map(|c| c.path.clone()).collect();
    prune_hash_memo(&live_paths);

    // spec: TUI-60 - source names are source-controlled and must be sanitized.
    let source_names: Vec<String> = registry
        .sources
        .iter()
        .map(|s| strip_ansi(&s.name))
        .collect();

    // Build installed list.
    // spec: TUI-50 - compute direct dep keys for each installed item so the
    // TUI can render the dependency subtree without extra I/O at display time.
    // spec: TUI-60 - all source-derived strings are sanitized through strip_ansi
    // at the model boundary to prevent terminal injection from catalog-controlled
    // content (consistent with the CLI's DSC-69 / MKT-9 call sites).
    // spec: TUI-63 - out-of-date detection mirrors the CLI's CLI-75 check:
    // hash drift (the source content changed) or a rename (effective name
    // no longer matches the recorded manifest name), computed against the
    // matching catalog item.
    let installed: Vec<SnapshotInstalled> = manifest
        .items
        .values()
        .map(|it| {
            // Find the matching catalog item to get direct deps + drift.
            let matched = catalog_items
                .iter()
                .find(|ci| ci.source == it.source && ci.kind == it.kind && ci.name == it.bare_name);
            // spec: TUI-60 - dep keys are catalog-derived (a `kind:name` built
            // from source-controlled item names) like every sibling field on
            // this struct, so they get the same strip_ansi treatment at the
            // model boundary (`sanitize_dep_keys`).
            let deps: Vec<String> = matched
                .map(|ci| {
                    sanitize_dep_keys(crate::deps::direct_dependency_keys(
                        ci,
                        &catalog_items,
                        &read_item_text,
                    ))
                })
                .unwrap_or_default();
            let stale = matched.is_some_and(|ci| {
                // Memoized on the cheap stat fingerprint: the poll tick would
                // otherwise re-read every installed item's content every second.
                let cur = memoized_hash(&ci.path);
                let hash_drift = cur.as_deref().is_none_or(|h| h != it.hash);
                let rename_drift = ci.effective_name() != it.name;
                hash_drift || rename_drift
            });
            SnapshotInstalled {
                key: it.key(),
                name: strip_ansi(&it.name),
                source: strip_ansi(&it.source),
                kind: it.kind,
                commit: it.commit.clone(),
                description: it.description.as_deref().map(strip_ansi),
                deps,
                stale,
            }
        })
        .collect();

    // Build available list (all catalog items; de-dup vs installed happens in tree.rs).
    // spec: TUI-50 - compute direct dep keys for each available item.
    // spec: TUI-60 - strip_ansi on all source-derived display strings, deps
    // included (they are catalog-derived `kind:name` keys, same as `installed`
    // above; `sanitize_dep_keys`).
    let available: Vec<SnapshotAvailable> = catalog_items
        .iter()
        .map(|it| {
            let deps = sanitize_dep_keys(crate::deps::direct_dependency_keys(
                it,
                &catalog_items,
                &read_item_text,
            ));
            SnapshotAvailable {
                key: it.key(),
                name: strip_ansi(&it.effective_name()),
                source: strip_ansi(&it.source),
                kind: it.kind,
                description: it.description.as_deref().map(strip_ansi),
                path: it.path.clone(),
                deps,
            }
        })
        .collect();

    // Unmanaged lobe items (UNM-6): kind-dir entries in a configured agent home
    // that mind did not install. A scan failure is non-fatal: the rest of the
    // TUI stays usable, the unmanaged group is simply empty.
    // spec: UNM-6
    // spec: TUI-60 - strip_ansi on name (unmanaged item names come from lobe filenames).
    let unmanaged: Vec<SnapshotUnmanaged> = crate::unmanaged::scan(paths, &manifest)
        .unwrap_or_default()
        .into_iter()
        .map(|u| SnapshotUnmanaged {
            key: u.key(),
            name: strip_ansi(&u.name),
            kind: u.kind,
            paths: u.paths,
        })
        .collect();

    // Build the suggested registry (TUI-31). Failures are silently ignored
    // so a bad mind.toml in a melded source does not break the whole TUI.
    let suggestions = crate::tui::preview::suggested_registry(paths).unwrap_or_default();

    // Load configured lobes for TUI-23. Falls back to empty (default lobe used).
    // spec: TUI-23
    let lobes = Config::load(paths)
        .map(|c| c.lobes.iter().map(|e| e.path().to_string()).collect())
        .unwrap_or_default();

    // Build source namespace map (TUI-53, NS-1): effective prefix per source.
    // All catalog items from the same source share the same prefix (set in
    // catalog::scan), so the first item's prefix is the effective prefix.
    // For sources with no catalog items, fall back to the raw alias.
    // spec: TUI-53
    let source_namespaces: std::collections::HashMap<String, Option<String>> = {
        let mut m = std::collections::HashMap::new();
        for item in &catalog_items {
            m.entry(item.source.clone())
                .or_insert_with(|| item.prefix.clone());
        }
        for source in &registry.sources {
            m.entry(source.name.clone())
                .or_insert_with(|| source.alias.clone().filter(|p| !p.is_empty()));
        }
        m
    };

    Ok(Snapshot {
        installed,
        available,
        unmanaged,
        source_names,
        suggestions,
        lobes,
        source_namespaces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_paths() -> (Paths, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-tui-data-{}-{n}", std::process::id()));
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        (paths, base)
    }

    fn cleanup(base: &std::path::Path) {
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn load_returns_empty_snapshot_on_fresh_home() {
        // spec: TUI-12 TUI-13 TUI-15
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let snap = load(&paths).expect("load should succeed on fresh home");
        assert!(snap.installed.is_empty(), "fresh home: no installed items");
        assert!(snap.available.is_empty(), "fresh home: no available items");
        assert!(snap.unmanaged.is_empty(), "fresh home: no unmanaged items");
        assert!(snap.source_names.is_empty(), "fresh home: no sources");
        cleanup(&base);
    }

    #[test]
    fn try_poll_succeeds_when_no_exclusive_lock_held() {
        // spec: TUI-15 TUI-25
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let snap = try_poll(&paths);
        assert!(
            snap.is_some(),
            "try_poll should succeed when no exclusive lock is held"
        );
        cleanup(&base);
    }

    #[test]
    fn try_poll_returns_none_when_exclusive_lock_held() {
        // spec: TUI-25 (non-blocking poll skips while mutation holds exclusive lock)
        use fd_lock::RwLock;
        use std::fs::OpenOptions;

        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();

        // Hold an exclusive lock on the lock file directly.
        let lock_path = paths.lock_file();
        std::fs::write(&lock_path, b"").unwrap();
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        let mut raw_lock = RwLock::new(f);
        let _excl = raw_lock.write().expect("acquire exclusive lock");

        // try_poll must return None (non-blocking, skips under exclusive lock).
        let snap = try_poll(&paths);
        assert!(
            snap.is_none(),
            "try_poll must return None when exclusive lock is held"
        );
        drop(_excl);
        cleanup(&base);
    }

    // spec: TUI-72
    #[test]
    fn memoized_hash_matches_hash_path_and_refreshes_on_change() {
        // The memo must be transparent: the value it returns is always the value
        // `hash_path` would return, both on the cold path and on a memo hit, and
        // it must pick up a content change rather than serving a stale hash.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let item = base.join("item");
        crate::paths::mkdir_p(&item).unwrap();
        std::fs::write(item.join("SKILL.md"), b"v1").unwrap();

        // Cold: computes and caches.
        let first = memoized_hash(&item).expect("hash");
        assert_eq!(
            first,
            hash_path(&item).unwrap(),
            "cold value must be correct"
        );
        // Warm: served from the memo, same value.
        let second = memoized_hash(&item).expect("hash");
        assert_eq!(second, first, "a memo hit must return the same hash");

        // Content change (also a size change): the fingerprint moves, so the
        // memo must recompute rather than serve the cached value.
        std::fs::write(item.join("SKILL.md"), b"v2 is longer than v1").unwrap();
        let after = memoized_hash(&item).expect("hash");
        assert_ne!(after, first, "a content change must invalidate the memo");
        assert_eq!(
            after,
            hash_path(&item).unwrap(),
            "the refreshed value must equal a direct hash_path"
        );

        cleanup(&base);
    }

    // spec: TUI-72
    #[test]
    fn memoized_hash_detects_same_size_content_change() {
        // M3(d): the test above only edits content in a way that also changes
        // SIZE. A same-size rewrite (mtime advances, size does not) exercises
        // the other half of the fingerprint and must still invalidate the memo.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let item = base.join("item-same-size");
        crate::paths::mkdir_p(&item).unwrap();
        std::fs::write(item.join("SKILL.md"), b"aaaaa").unwrap();

        let first = memoized_hash(&item).expect("hash");
        std::fs::write(item.join("SKILL.md"), b"bbbbb").unwrap();
        let after = memoized_hash(&item).expect("hash");
        assert_ne!(
            first, after,
            "a same-size content rewrite must invalidate the memo"
        );
        assert_eq!(
            after,
            hash_path(&item).unwrap(),
            "the refreshed memo value must equal a direct hash_path"
        );

        cleanup(&base);
    }

    // spec: TUI-72
    #[test]
    fn prune_memo_map_evicts_paths_no_longer_in_the_catalog() {
        // L14: HASH_MEMO is otherwise insert-only, so a path whose item was
        // unmelded or removed upstream would sit in the memo for the life of
        // the process. `prune_memo_map` (the logic behind `prune_hash_memo`)
        // bounds it to the live path set. Exercised against a LOCAL map, not
        // the process-global `HASH_MEMO`: every test in this module shares
        // that static, so pruning it directly here could race a concurrently
        // running test's own `load` call, which prunes it too.
        let p1 = PathBuf::from("/fake/still-in-catalog");
        let p2 = PathBuf::from("/fake/dropped-from-catalog");
        let mut memo = std::collections::BTreeMap::new();
        memo.insert(p1.clone(), ("fp1".to_string(), "hash1".to_string()));
        memo.insert(p2.clone(), ("fp2".to_string(), "hash2".to_string()));

        // Prune to a live set that no longer includes p2 (as if its source
        // was unmelded or the item was removed upstream).
        let mut live = std::collections::HashSet::new();
        live.insert(p1.clone());
        prune_memo_map(&mut memo, &live);

        assert!(
            memo.contains_key(&p1),
            "a path still in the catalog must survive pruning"
        );
        assert!(
            !memo.contains_key(&p2),
            "a path no longer in the catalog must be evicted, not retained forever"
        );
    }

    #[test]
    fn two_loads_with_no_change_produce_equal_snapshots() {
        // spec: TUI-15 - change detection is by VALUE, so two loads over
        // unchanged state must compare equal; otherwise the poll tick's
        // `apply_snapshot_if_changed` gate can never fire and every tick
        // rebuilds the tree. (The previous `generation` counter was minted
        // fresh per load, which is exactly the bug this pins against.)
        //
        // M-test5: a fresh, empty MIND_HOME makes every snapshot vector empty,
        // so the comparison would pass trivially even if `installed`/
        // `available` ordering were nondeterministic (a future switch away
        // from the current BTreeMap/sorted-scan ordering would silently
        // reinstate the "every tick rebuilds" bug with no failing test). Seed
        // a melded source (with an installed AND an uninstalled item, so both
        // `installed` and `available` are non-empty) before comparing.
        use std::process::Command;

        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = base.join("determinism-source");
        std::fs::create_dir_all(src.join("skills/review")).unwrap();
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("skills/extra")).unwrap();
        std::fs::write(
            src.join("skills/extra/SKILL.md"),
            "---\ndescription: extra skill\n---\n# extra\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&src)
                .output()
                .expect("git");
        };
        git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "initial"]);

        crate::commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            crate::commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        crate::commands::learn(
            &paths,
            "skill:review",
            false,
            crate::commands::InstallFlow {
                yes: true,
                clobber: crate::commands::Clobber::Force,
                dangerously_skip: true,
                dangerously_skip_build: true,
            },
        )
        .expect("learn");

        let snap1 = load(&paths).unwrap();
        let snap2 = load(&paths).unwrap();
        assert!(
            !snap1.installed.is_empty(),
            "fixture must have an installed item for this test to be meaningful"
        );
        assert!(
            !snap1.available.is_empty(),
            "fixture must have an available item for this test to be meaningful"
        );
        assert_eq!(
            snap1, snap2,
            "two loads over unchanged state (melded source + installed item) must be \
             equal so the poll gate can skip the rebuild"
        );
        cleanup(&base);
    }

    /// ANSI escapes and bidi-override code points in source-derived strings
    /// must be stripped before they enter the TUI snapshot model (TUI-60).
    ///
    /// Builds a manifest.json with ANSI color escapes in name/source and a
    /// bidi-override (U+202E) in description, loads the snapshot, and asserts
    /// every model field is clean. The bidi character is injected via format!
    /// to avoid triggering the text_direction_codepoint_in_literal lint.
    #[test]
    fn snapshot_installed_strips_ansi_from_source_derived_strings() {
        // spec: TUI-60
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();

        // U+202E RIGHT-TO-LEFT OVERRIDE injected at runtime to avoid lint.
        let bidi = '\u{202E}';
        let manifest_json = format!(
            concat!(
                "{{\n",
                "  \"items\": {{\n",
                "    \"skill:\\u001b[31mevil\\u001b[0m\": {{\n",
                "      \"kind\": \"skill\",\n",
                "      \"name\": \"\\u001b[31mevil\\u001b[0m\",\n",
                "      \"bare_name\": \"evil\",\n",
                "      \"source\": \"\\u001b[32msrc\\u001b[0m\",\n",
                "      \"commit\": \"abc1234\",\n",
                "      \"hash\": \"deadbeef\",\n",
                "      \"store\": \"store/skill/evil\",\n",
                "      \"links\": [],\n",
                "      \"description\": \"\\u001b[1mbold\\u001b[0m with {}bidi\"\n",
                "    }}\n",
                "  }}\n",
                "}}"
            ),
            bidi
        );
        std::fs::write(paths.manifest_file(), manifest_json).unwrap();

        let snap = load(&paths).expect("load should succeed");

        assert_eq!(snap.installed.len(), 1, "one installed item");
        let item = &snap.installed[0];

        assert_eq!(
            item.name, "evil",
            "ANSI escapes must be stripped from name; got: {:?}",
            item.name
        );
        assert_eq!(
            item.source, "src",
            "ANSI escapes must be stripped from source; got: {:?}",
            item.source
        );
        let desc = item.description.as_deref().unwrap_or("");
        assert!(
            !desc.contains('\x1b'),
            "ANSI escapes must be stripped from description; got: {:?}",
            desc
        );
        assert!(
            !desc.contains('\u{202E}'),
            "bidi-override must be stripped from description; got: {:?}",
            desc
        );
        assert_eq!(item.kind, ItemKind::Skill, "kind field must be preserved");

        cleanup(&base);
    }

    // spec: TUI-60
    #[test]
    fn sanitize_dep_keys_strips_ansi_and_bidi() {
        // L15: dep keys were the only source-derived strings on
        // SnapshotInstalled/SnapshotAvailable that skipped strip_ansi, unlike
        // the sibling name/source/description fields on the same structs.
        // `catalog::scan` already rejects an unsafe item NAME outright, so a
        // full end-to-end reproduction through a real catalog scan can no
        // longer smuggle an escape into a dep key; test the mapping function
        // itself (what `load_inner` actually calls) directly instead.
        let bidi = '\u{202E}';
        let raw = vec![
            format!("skill:re\x1b[31mview"),
            format!("agent:d{bidi}ev"),
            "rule:clean".to_string(),
        ];
        let sanitized = sanitize_dep_keys(raw);
        assert_eq!(
            sanitized,
            vec![
                "skill:review".to_string(),
                "agent:dev".to_string(),
                "rule:clean".to_string(),
            ]
        );
        assert!(
            sanitized
                .iter()
                .all(|s| !s.contains('\x1b') && !s.contains(bidi)),
            "no dep key may retain a raw ANSI escape or bidi override: {sanitized:?}"
        );
    }

    /// M2/TUI-63: `SnapshotInstalled.stale` mirrors the CLI's CLI-75 outdated
    /// check. A local-path source is read live from its working tree (no
    /// separate clone step), so editing the item file in place changes its
    /// content hash while the recorded commit stays put -- exactly the CLI-75
    /// scenario (`recall_marks_item_outdated_after_in_place_content_edit`).
    #[test]
    fn snapshot_installed_marks_stale_after_in_place_content_edit() {
        // spec: TUI-63
        use std::process::Command;

        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = base.join("stale-source");
        std::fs::create_dir_all(src.join("skills/review")).unwrap();
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\noriginal content\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&src)
                .output()
                .expect("git");
        };
        git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "initial"]);

        crate::commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            crate::commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        crate::commands::learn(
            &paths,
            "skill:review",
            false,
            crate::commands::InstallFlow {
                yes: true,
                clobber: crate::commands::Clobber::Force,
                dangerously_skip: true,
                dangerously_skip_build: true,
            },
        )
        .expect("learn");

        let snap = load(&paths).expect("load should succeed");
        assert_eq!(snap.installed.len(), 1, "one installed item");
        assert!(
            !snap.installed[0].stale,
            "a freshly installed item must not be marked stale"
        );

        // Edit the item source file in place without committing (mirrors the
        // CLI-75 test): content hash changes, commit does not.
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\nmodified content\n",
        )
        .unwrap();

        let snap2 = load(&paths).expect("load should succeed after edit");
        assert_eq!(snap2.installed.len(), 1, "still one installed item");
        assert!(
            snap2.installed[0].stale,
            "an in-place content edit must mark the item stale (TUI-63)"
        );

        cleanup(&base);
    }
}
