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
use crate::lock;
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::sanitize::strip_ansi;
use crate::source::Registry;

/// A snapshot of the TUI's data, built from registry + manifest + catalog.
/// The `generation` counter increments on each structural change so the App
/// can detect when a rebuild is needed.
// spec: TUI-15
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub generation: u64,
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
#[derive(Debug, Clone)]
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
}

/// One available (catalog) item in the snapshot.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct SnapshotUnmanaged {
    pub key: String,
    pub name: String,
    pub kind: ItemKind,
    pub paths: Vec<PathBuf>,
}

/// Global generation counter, incremented when data changes are detected.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    let installed: Vec<SnapshotInstalled> = manifest
        .items
        .values()
        .map(|it| {
            // Find the matching catalog item to get direct deps.
            let deps = catalog_items
                .iter()
                .find(|ci| ci.source == it.source && ci.kind == it.kind && ci.name == it.bare_name)
                .map(|ci| crate::deps::direct_dependency_keys(ci, &catalog_items, &read_item_text))
                .unwrap_or_default();
            SnapshotInstalled {
                key: it.key(),
                name: strip_ansi(&it.name),
                source: strip_ansi(&it.source),
                kind: it.kind,
                commit: it.commit.clone(),
                description: it.description.as_deref().map(strip_ansi),
                deps,
            }
        })
        .collect();

    // Build available list (all catalog items; de-dup vs installed happens in tree.rs).
    // spec: TUI-50 - compute direct dep keys for each available item.
    // spec: TUI-60 - strip_ansi on all source-derived display strings.
    let available: Vec<SnapshotAvailable> = catalog_items
        .iter()
        .map(|it| {
            let deps = crate::deps::direct_dependency_keys(it, &catalog_items, &read_item_text);
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
        generation: next_generation(),
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

    #[test]
    fn generation_increments_on_each_load() {
        // spec: TUI-15
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let snap1 = load(&paths).unwrap();
        let snap2 = load(&paths).unwrap();
        assert!(
            snap2.generation > snap1.generation,
            "generation should increment on each load"
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
}
