//! Data load and poll layer for the TUI.
//!
//! The only module in the TUI that acquires the global lock for reads.
//! Uses a non-blocking shared acquire for the poll tick (TUI-15, TUI-25)
//! so the UI never freezes behind a writer.
//!
//! Change detection: the source-set and manifest are compared across polls.
//! The catalog is only re-scanned when the source set changes (TUI-15).

use std::path::PathBuf;

use cached::Cached as _;

use crate::catalog;
use crate::config::Config;
use crate::error::{ItemKind, Result};
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
    /// Identity, NOT display: drives TUI action dispatch (`ActionKind::Forget
    /// { item_key }`, `upgrade_keys`, tree.rs node ids) and the manifest/store
    /// path lookup. The name is source-derived (a directory name, or a
    /// `[[items]] name`) and can carry ANSI/control/bidi code points, so it is
    /// NEVER rendered directly -- see [`Self::display_key`] for the sanitized
    /// counterpart (H1/TUI-75).
    pub key: String,
    /// The sanitized form of `key` (`ItemKey::display`, DSC-95): the ONLY
    /// reading of this item's key that may reach a confirm-modal description,
    /// dependents list, or any other terminal/`--json` print site (TUI-75).
    // spec: TUI-75
    pub display_key: String,
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
    /// The item's live catalog path as of the last load/poll, or `None` when
    /// it currently has no matching catalog entry (its source was unmelded,
    /// or the item was removed upstream) -- the same condition under which
    /// `stale` is false above. Carried on the snapshot itself (not a
    /// thread-local side index) so the "never staler than `last_snapshot`"
    /// property `is_actually_stale` relies on holds by construction: it is
    /// set in the same `load_inner` pass that computes every other field on
    /// this struct, from the exact same snapshot generation.
    pub path: Option<PathBuf>,
    /// The recorded manifest hash for this item as of the last load/poll,
    /// paired with `path` above for the TUI-74 authoritative recompute
    /// (`hash_path_ignoring(path, ignore) != recorded_hash`).
    pub recorded_hash: String,
    /// The item's effective ignore patterns (IGN-1) as of the last load/poll,
    /// carried alongside `path`/`recorded_hash` so the TUI-74 authoritative
    /// recompute can build the SAME ignore set the recorded hash was computed
    /// with (`CatalogItem::ignore_set`), not an empty one (H1): comparing
    /// `hash_path` (zero rules, not even the IGN-2 built-ins) against a
    /// manifest hash computed with the item's ignores applied reports
    /// permanent, unfixable drift for any item with declared ignores, and for
    /// every `path = "."` item (which always has a `.git` dir). Kept as
    /// `Vec<String>` (the raw, uncompiled patterns) rather than a compiled
    /// `IgnoreSet` so this struct's `PartialEq`/`Eq` derives stay intact.
    // spec: TUI-74 IGN-10
    pub ignore: Vec<String>,
}

/// One available (catalog) item in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAvailable {
    /// Identity, NOT display: see the identical note on
    /// [`SnapshotInstalled::key`].
    pub key: String,
    /// The sanitized form of `key` (TUI-75); see
    /// [`SnapshotInstalled::display_key`].
    // spec: TUI-75
    pub display_key: String,
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
    /// Identity, NOT display: see the identical note on
    /// [`SnapshotInstalled::key`]. Unmanaged names come straight off the
    /// filesystem (a lobe directory/file entry) with NO validation gate
    /// equivalent to DSC-96's catalog-scan rejection (H1/TUI-75) -- the one
    /// name class where a hostile value is guaranteed to reach this struct.
    pub key: String,
    /// The sanitized form of `key` (TUI-75); see
    /// [`SnapshotInstalled::display_key`].
    // spec: TUI-75
    pub display_key: String,
    pub name: String,
    pub kind: ItemKind,
    pub paths: Vec<PathBuf>,
}

/// Memo of item content hashes for the TUI's staleness check, keyed on the
/// pair (item path, cheap stat fingerprint).
///
/// The poll tick runs about once a second and computes TUI-63 staleness for
/// every installed item, which means a full content hash of every item's source
/// tree. For a markdown skill that is trivial; for a `tool` directory carrying a
/// vendored binary it is tens of megabytes re-read per second. The cheap stat
/// fingerprint (`hash::stat_fingerprint_ignoring`: `mtime`, `size`, and -- under
/// `cfg(unix)` -- `ctime` and inode number) stands in for the content: it is
/// part of the KEY, not a token stored beside the value and compared by hand,
/// so a tree that changed simply misses and re-reads. There is no validity
/// check to get wrong, and no way to serve a value whose fingerprint does not
/// match the one asked for.
///
/// The ignore set is deliberately NOT part of the key. A changed set changes
/// which entries the fingerprint walk sees, so it already moves the
/// fingerprint; and two sets that walk the identical files agree on the content
/// hash too, so they are interchangeable here (IGN-10).
///
/// Sized and pruned by [`fit_hash_memo`] on every full load rather than held at
/// a fixed capacity. Capacity is a runtime value for a reason: `load_inner`
/// hashes every installed item once per tick in a stable order, which is a
/// cyclic sequential scan, the worst case for LRU. At N items and capacity C
/// the hit rate is ~100% while N <= C and collapses to ~0% the moment N > C,
/// because each tick evicts exactly the entries the next tick reaches for
/// first. That is a cliff, not a slope, and past it the memo costs a full
/// re-read of every item tree every second: precisely what it exists to
/// prevent. Deriving C from the catalog keeps N below it by construction.
///
/// Display-side only (TUI-72): `upgrade`/`introspect`/`recall` all call
/// `CatalogItem::content_hash` directly, never this memo, so a missed
/// fingerprint change can never change what a verb ACTS on -- at most it makes
/// the TUI-63 confirm modal list fewer stale items than the no-sync apply
/// (TUI-73) then acts on, since that apply always re-hashes for real. A miss is
/// NOT bounded to "one tick": with no TTL it is served until the entry is
/// pruned or evicted. `stat_fingerprint`'s `ctime`/inode fields close the
/// realistic miss case -- a same-size, mtime-preserving replace (`cp -p`,
/// `rsync -a`, `tar -p`, `touch -r`) -- but the fingerprint is still not a
/// content hash and must never be compared against a recorded manifest hash.
static HASH_MEMO: std::sync::LazyLock<
    cached::sync_sync::RwLock<cached::LruCache<(PathBuf, String), String>>,
> = std::sync::LazyLock::new(|| cached::sync_sync::RwLock::new(new_hash_memo(HASH_MEMO_FLOOR)));

/// The capacity the memo starts at and never drops below, covering the load
/// before the first [`fit_hash_memo`] call and every install smaller than it.
const HASH_MEMO_FLOOR: usize = 1024;

/// Entries per installed item. One holds the item's current fingerprint; the
/// spare absorbs an in-flight edit without evicting a neighbour, so a user
/// editing items in a loop does not push the working set over capacity.
const HASH_MEMO_PER_ITEM: usize = 2;

fn new_hash_memo(capacity: usize) -> cached::LruCache<(PathBuf, String), String> {
    cached::LruCache::builder()
        .max_size(capacity.max(1))
        .build()
        .expect("a non-zero max_size is the only failure mode")
}

/// Drop entries whose path left the catalog, and grow the memo if the current
/// catalog no longer fits. Called once per full load with the freshly scanned
/// item paths.
///
/// This is what keeps the LRU bound from becoming a cliff (see [`HASH_MEMO`]):
/// capacity tracks the workload instead of a constant chosen in advance, so the
/// N > C regime is not reachable by installing more items. The prune half also
/// keeps a dead path (its source unmelded, or the item removed upstream) from
/// occupying capacity that live items need, which a bare LRU would let it do
/// until it aged out on its own.
fn fit_hash_memo(live_paths: &std::collections::HashSet<PathBuf>) {
    let needed = (live_paths.len() * HASH_MEMO_PER_ITEM).max(HASH_MEMO_FLOOR);
    let mut memo = HASH_MEMO.write();
    memo.retain(|(path, _), _| live_paths.contains(path));
    if memo.capacity() < needed {
        // `capacity` has no setter, so growing means rebuilding. Carry the
        // surviving entries over in LRU order so a grow does not throw away
        // the work the memo exists to preserve.
        let mut grown = new_hash_memo(needed);
        for (key, value) in memo.iter_order() {
            grown.cache_set(key, value);
        }
        *memo = grown;
    }
}

/// Test-only: seed `HASH_MEMO` with the entry `memoized_hash` would look up
/// for `path` under `ignore`, but carrying an arbitrary (possibly wrong)
/// `fake_hash`. This simulates "the memo believes this path is clean"
/// independent of whatever `path`'s real content hash is right now.
///
/// Needed because `stat_fingerprint_ignoring` folds in `ctime`/inode under
/// `cfg(unix)` (a prior commit closed the mtime-preserving blind spot), so a
/// real on-disk edit always moves the fingerprint and the memo always
/// recomputes -- there is no longer a realistic sequence of filesystem calls
/// that reproduces "memo says clean, real content differs". Poisoning the
/// memo directly is the only way to construct that condition in a test, to
/// prove the display-only `stale` flag `load_inner` computes via
/// `memoized_hash` really can lag reality (TUI-72).
///
/// Takes the SAME `ignore` the matching `memoized_hash` call will pass, and
/// builds the key the same way it does. The two must not diverge: the
/// fingerprint is half the key now, so a helper that computed it from a
/// different ignore set would seed an entry nothing ever looks up, and the
/// test would fail reporting "served the real hash" rather than naming the
/// mismatch.
///
/// A caller must assert directly against [`memoized_hash`], never through a
/// full `load`/`try_poll` with other work interleaved between the poison and
/// the read. `HASH_MEMO` is one `static` shared by the whole test binary, so a
/// concurrently running test that inserts enough entries can evict this one
/// under LRU pressure; the shorter the gap between poisoning and reading, the
/// smaller that window.
#[cfg(test)]
pub(crate) fn poison_memo_for_test(
    path: &std::path::Path,
    ignore: &crate::ignore::IgnoreSet,
    fake_hash: &str,
) {
    use cached::Cached as _;
    if let Ok(fp) = crate::hash::stat_fingerprint_ignoring(path, ignore) {
        HASH_MEMO
            .write()
            .cache_set((path.to_path_buf(), fp), fake_hash.to_string());
    }
}

/// The content hash of `path`, served from [`hash_at_fingerprint`]'s memo when
/// the tree's cheap stat fingerprint is one already seen. `None` on any hash
/// failure, which callers treat as drift (matching `content_hash().ok()`, the
/// CLI-75 rule). When the fingerprint itself cannot be computed, this falls back
/// to hashing every time and caches nothing, so an unsupported-mtime platform
/// behaves exactly as before the memo existed.
///
/// spec: IGN-10 -- both the fingerprint and the hash take the ITEM's ignore
/// set, so the memo measures the same tree the install wrote and the recorded
/// manifest hash was computed from. A set that changes (the source edited its
/// `ignore` list) also changes the fingerprint, since the newly excluded
/// entries' stats were part of it, so the memo misses and re-hashes.
fn memoized_hash(path: &std::path::Path, ignore: &crate::ignore::IgnoreSet) -> Option<String> {
    // No fingerprint (an unsupported-mtime platform): hash every time and cache
    // nothing, exactly as before the memo existed.
    let Ok(fingerprint) = crate::hash::stat_fingerprint_ignoring(path, ignore) else {
        return crate::hash::hash_path_ignoring(path, ignore).ok();
    };

    let key = (path.to_path_buf(), fingerprint);
    if let Some(hit) = HASH_MEMO.write().cache_get(&key) {
        return Some(hit.clone());
    }
    // A failure is not stored, so it is retried on the next tick rather than
    // remembered for the life of the entry.
    let hash = crate::hash::hash_path_ignoring(path, ignore).ok()?;
    HASH_MEMO.write().cache_set(key, hash.clone());
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

    // spec: TUI-72 -- size the memo to this catalog and drop entries whose path
    // has left it, before it is consulted below.
    let live_paths: std::collections::HashSet<PathBuf> =
        catalog_items.iter().map(|c| c.path.clone()).collect();
    fit_hash_memo(&live_paths);

    // L10: one sibling index for both passes below. Each walks every item, so
    // rebuilding it per row (what `deps::direct_dependency_keys` does) made a
    // poll tick quadratic in the catalog size.
    let dep_index = crate::deps::DepIndex::new(&catalog_items);

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
    let mut installed: Vec<SnapshotInstalled> = Vec::with_capacity(manifest.items.len());
    for it in manifest.items.values() {
        // Find the matching catalog item to get direct deps + drift.
        // Keep the position, not just the reference: `DepIndex` keys on it.
        let matched_node = catalog_items
            .iter()
            .position(|ci| ci.source == it.source && ci.kind == it.kind && ci.name == it.bare_name);
        let matched = matched_node.map(|n| &catalog_items[n]);
        // spec: TUI-60 - dep keys are catalog-derived (a `kind:name` built
        // from source-controlled item names) like every sibling field on
        // this struct, so they get the same strip_ansi treatment at the
        // model boundary (`sanitize_dep_keys`).
        let deps: Vec<String> = matched_node
            .map(|n| sanitize_dep_keys(dep_index.direct_keys(n, &read_item_text)))
            .unwrap_or_default();
        // spec: TUI-74 IGN-10 - compiled once per item per load pass (L7),
        // not inside the per-tick fast path the memo exists to keep cheap.
        // The raw (uncompiled) patterns are taken straight from `ci.ignore`
        // below for the snapshot field, so this is the only compile.
        let ignore_set = matched.and_then(|ci| ci.ignore_set().ok());
        let stale = matched.is_some_and(|ci| {
            // Memoized on the cheap stat fingerprint: the poll tick would
            // otherwise re-read every installed item's content every second.
            let cur = ignore_set
                .as_ref()
                .and_then(|ig| memoized_hash(&ci.path, ig));
            let hash_drift = cur.as_deref().is_none_or(|h| h != it.hash);
            let rename_drift = ci.effective_name() != it.name;
            hash_drift || rename_drift
        });
        installed.push(SnapshotInstalled {
            // `key` is identity, not display: it drives TUI action dispatch
            // (learn/forget refs), tree.rs node ids, and `manifest`/store
            // lookups, all keyed on the raw string. It is NEVER rendered
            // directly -- `display_key` below is the sanitized reading a
            // confirm-modal description or any other print site must use
            // instead (H1/TUI-75).
            key: it.key().into(),
            // spec: TUI-75
            display_key: it.display_key(),
            name: strip_ansi(&it.name),
            source: strip_ansi(&it.source),
            kind: it.kind,
            commit: it.commit.clone(),
            description: it.description.as_deref().map(strip_ansi),
            deps,
            stale,
            // spec: TUI-72 TUI-73 TUI-74 - carried on the snapshot itself
            // (M10), not a side thread-local index, so `is_actually_stale`'s
            // "never staler than `last_snapshot`" property holds by
            // construction: both are set in this same `load_inner` pass.
            path: matched.map(|ci| ci.path.clone()),
            recorded_hash: it.hash.clone(),
            // spec: TUI-74 IGN-10 - the item's effective (resolved) ignore
            // patterns, so `is_actually_stale` (app.rs) can rebuild the SAME
            // ignore set the recorded hash was computed with instead of
            // comparing against a `hash_path` result computed with none (H1).
            ignore: matched
                .map(|ci| ci.ignore.clone().unwrap_or_default())
                .unwrap_or_default(),
        });
    }

    // Build available list (all catalog items; de-dup vs installed happens in tree.rs).
    // spec: TUI-50 - compute direct dep keys for each available item.
    // spec: TUI-60 - strip_ansi on all source-derived display strings, deps
    // included (they are catalog-derived `kind:name` keys, same as `installed`
    // above; `sanitize_dep_keys`).
    let available: Vec<SnapshotAvailable> = catalog_items
        .iter()
        .enumerate()
        .map(|(node, it)| {
            let deps = sanitize_dep_keys(dep_index.direct_keys(node, &read_item_text));
            SnapshotAvailable {
                // See the identical `key` note on `SnapshotInstalled` above.
                key: it.key().into(),
                // spec: TUI-75
                display_key: it.display_key(),
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
            // See the identical `key` note on `SnapshotInstalled` above.
            key: u.key().into(),
            // spec: TUI-75
            display_key: u.display_key(),
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
    use crate::hash::hash_path;
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
        let first = memoized_hash(&item, &crate::ignore::IgnoreSet::default()).expect("hash");
        assert_eq!(
            first,
            hash_path(&item).unwrap(),
            "cold value must be correct"
        );
        // Warm: served from the memo, same value.
        let second = memoized_hash(&item, &crate::ignore::IgnoreSet::default()).expect("hash");
        assert_eq!(second, first, "a memo hit must return the same hash");

        // Content change (also a size change): the fingerprint moves, so the
        // memo must recompute rather than serve the cached value.
        std::fs::write(item.join("SKILL.md"), b"v2 is longer than v1").unwrap();
        let after = memoized_hash(&item, &crate::ignore::IgnoreSet::default()).expect("hash");
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

        let first = memoized_hash(&item, &crate::ignore::IgnoreSet::default()).expect("hash");
        std::fs::write(item.join("SKILL.md"), b"bbbbb").unwrap();
        let after = memoized_hash(&item, &crate::ignore::IgnoreSet::default()).expect("hash");
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
    fn fitting_the_memo_grows_capacity_past_a_large_catalog() {
        // The property that matters is that capacity TRACKS the catalog, not
        // that it equals any particular number. `load_inner` hashes every
        // installed item once per tick in a stable order, so the memo sees a
        // cyclic sequential scan: at N items over capacity C the LRU evicts
        // exactly what the next tick wants first and the hit rate goes to ~0,
        // re-reading every tree every second. A fixed capacity makes that
        // regime reachable by installing more items; deriving it from the live
        // set does not.
        let many: std::collections::HashSet<PathBuf> = (0..5000)
            .map(|i| PathBuf::from(format!("/fake/item-{i}")))
            .collect();
        fit_hash_memo(&many);

        let capacity = HASH_MEMO.read().capacity();
        assert!(
            capacity >= many.len(),
            "capacity must cover the catalog ({} items), got {capacity}: below \
             it the memo does not degrade, it stops working",
            many.len()
        );

        // And it never drops below the floor for a small catalog.
        fit_hash_memo(&std::collections::HashSet::new());
        assert!(
            HASH_MEMO.read().capacity() >= HASH_MEMO_FLOOR,
            "capacity must never fall below the floor"
        );
    }

    // spec: TUI-72
    #[test]
    fn fitting_the_memo_drops_entries_whose_path_left_the_catalog() {
        // A dead path (its source unmelded, or the item removed upstream)
        // otherwise sits in the memo occupying capacity that live items need,
        // until it happens to age out on its own.
        let live = PathBuf::from("/fake/still-in-catalog");
        let dead = PathBuf::from("/fake/dropped-from-catalog");
        HASH_MEMO
            .write()
            .cache_set((live.clone(), "fp".into()), "hash-live".into());
        HASH_MEMO
            .write()
            .cache_set((dead.clone(), "fp".into()), "hash-dead".into());

        let mut live_paths = std::collections::HashSet::new();
        live_paths.insert(live.clone());
        fit_hash_memo(&live_paths);

        assert!(
            HASH_MEMO
                .write()
                .cache_get(&(live, "fp".to_string()))
                .is_some(),
            "a path still in the catalog must survive the prune"
        );
        assert!(
            HASH_MEMO
                .write()
                .cache_get(&(dead, "fp".to_string()))
                .is_none(),
            "a path no longer in the catalog must be dropped, not retained"
        );
    }

    // spec: TUI-72
    #[test]
    fn a_failed_hash_is_not_cached_and_is_retried() {
        // `None` is skipped by `cached`'s default for an `Option` return
        // (`cache_none = false`). Flipping that default would make a transient
        // failure stick for the life of the entry, and nothing else in the
        // suite would notice. Driving `hash_at_fingerprint` directly with a
        // literal fingerprint is the only way to hold the key fixed across the
        // failure and the success: through `memoized_hash` any filesystem
        // change that fixes the hash also moves the fingerprint, so the retry
        // would land on a different key and prove nothing.
        let (_paths, base) = temp_paths();
        let missing = base.join("not-created-yet");
        let ignore = crate::ignore::IgnoreSet::default();

        assert!(
            memoized_hash(&missing, &ignore).is_none(),
            "hashing a path that does not exist must fail"
        );
        assert!(
            !HASH_MEMO
                .read()
                .key_order()
                .iter()
                .any(|(path, _)| path == &missing),
            "a failed hash must leave NO entry behind: a stored failure would \
             be served for the life of the entry instead of being retried"
        );

        crate::paths::mkdir_p(&missing).unwrap();
        std::fs::write(missing.join("SKILL.md"), b"now it exists").unwrap();

        assert!(
            memoized_hash(&missing, &ignore).is_some(),
            "the next poll must recompute rather than serve a remembered failure"
        );

        cleanup(&base);
    }

    // spec: TUI-72
    #[test]
    fn poisoned_memo_serves_the_seeded_hash_instead_of_the_real_one() {
        // This asserts `poison_memo_for_test`'s effect DIRECTLY against
        // `memoized_hash`, with no intervening `load`/`try_poll` call between
        // the poison and the read. `HASH_MEMO` is one `static` shared by every
        // concurrently running test in this binary, so a sibling inserting
        // enough entries could evict this one under LRU pressure. Keeping the
        // gap between poisoning and reading to exactly these two calls (no
        // catalog scan, no meld/learn I/O) minimizes that window; a full
        // `load()` round trip (the pattern this replaces, formerly in
        // `app.rs`) held it open far longer.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let item = base.join("poison-target");
        crate::paths::mkdir_p(&item).unwrap();
        std::fs::write(item.join("SKILL.md"), b"v1").unwrap();

        // A real edit, so the poisoned value and the real value are provably
        // different (not just "the memo returned SOMETHING").
        std::fs::write(item.join("SKILL.md"), b"v2 is longer than v1").unwrap();
        let real_hash = hash_path(&item).expect("real hash");

        let fake_hash = "0000000000000000000000000000000000000000000000000000000000000000";
        poison_memo_for_test(&item, &crate::ignore::IgnoreSet::default(), fake_hash);
        let served = memoized_hash(&item, &crate::ignore::IgnoreSet::default())
            .expect("memoized_hash after poisoning");

        assert_eq!(
            served, fake_hash,
            "a poisoned memo must serve the seeded fake hash, not recompute"
        );
        assert_ne!(
            served, real_hash,
            "the seeded fake hash must differ from the item's real current hash \
             -- otherwise this test cannot distinguish 'served from memo' from \
             'recomputed and happened to match'"
        );

        cleanup(&base);
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
            // Not `format!`: there is nothing to interpolate here, and clippy's
            // `useless_format` (a hard error under `-D warnings`) rejects it.
            // The sibling below keeps `format!` because it interpolates `bidi`.
            "skill:re\x1b[31mview".to_string(),
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
