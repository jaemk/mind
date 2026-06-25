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

/// Load registry, manifest, and catalog without acquiring the lock (the
/// caller must already hold an appropriate guard).
fn load_inner(paths: &Paths) -> Result<Snapshot> {
    let registry = Registry::load(paths)?;
    let manifest = Manifest::load(paths)?;
    let catalog_items = catalog::scan(paths, &registry)?;

    let source_names: Vec<String> = registry.sources.iter().map(|s| s.name.clone()).collect();

    // Build installed list.
    let installed: Vec<SnapshotInstalled> = manifest
        .items
        .values()
        .map(|it| SnapshotInstalled {
            key: it.key(),
            name: it.name.clone(),
            source: it.source.clone(),
            kind: it.kind,
            commit: it.commit.clone(),
            description: it.description.clone(),
        })
        .collect();

    // Build available list (all catalog items; de-dup vs installed happens in tree.rs).
    let available: Vec<SnapshotAvailable> = catalog_items
        .iter()
        .map(|it| SnapshotAvailable {
            key: it.key(),
            name: it.effective_name(),
            source: it.source.clone(),
            kind: it.kind,
            description: it.description.clone(),
            path: it.path.clone(),
        })
        .collect();

    // Unmanaged lobe items (UNM-6): kind-dir entries in a configured agent home
    // that mind did not install. A scan failure is non-fatal: the rest of the
    // TUI stays usable, the unmanaged group is simply empty.
    // spec: UNM-6
    let unmanaged: Vec<SnapshotUnmanaged> = crate::unmanaged::scan(paths, &manifest)
        .unwrap_or_default()
        .into_iter()
        .map(|u| SnapshotUnmanaged {
            key: u.key(),
            name: u.name,
            kind: u.kind,
            paths: u.paths,
        })
        .collect();

    // Build the suggested registry (TUI-31). Failures are silently ignored
    // so a bad mind.toml in a melded source does not break the whole TUI.
    let suggestions = crate::tui::preview::suggested_registry(paths).unwrap_or_default();

    // Load configured lobes for TUI-23. Falls back to empty (default lobe used).
    // spec: TUI-23
    let lobes = Config::load(&paths.mind_home)
        .map(|c| c.lobes)
        .unwrap_or_default();

    Ok(Snapshot {
        generation: next_generation(),
        installed,
        available,
        unmanaged,
        source_names,
        suggestions,
        lobes,
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
}
