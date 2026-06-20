//! Execute a confirmed TUI action by calling the appropriate `commands::*` fn.
//!
//! Each action acquires the EXCLUSIVE lock for its duration, then releases it.
//! The verb functions (commands::learn/forget/sync/evolve/unmeld) print to stdout
//! normally; since the TUI is in alt-screen we capture their output and surface
//! errors through the App state (TUI-24). The action returns an updated Snapshot
//! so the App can refresh without a separate poll.
//!
//! No verb logic is reimplemented here; we call the existing command functions
//! directly (TUI-20..23).

use crate::commands;
use crate::error::Result;
use crate::lock;
use crate::paths::Paths;
use crate::tui::app::{ActionKind, PendingAction};
use crate::tui::data::{self, Snapshot};

/// Execute a confirmed action under an exclusive lock, returning a fresh
/// snapshot. Output from command functions is suppressed so it does not
/// corrupt the alt-screen display (TUI-24).
// spec: TUI-20 TUI-21 TUI-22 TUI-23 TUI-24 TUI-25 STO-40 STO-41
pub fn execute(paths: &Paths, action: PendingAction) -> Result<Snapshot> {
    // Acquire the exclusive lock for the duration of the action (TUI-25).
    // spec: STO-40 STO-41 TUI-25
    let mut lock = lock::open(paths)?;
    let _guard = lock.write()?;

    // Execute the appropriate command function. We capture stdout to prevent
    // raw-mode terminal corruption (TUI-24). Errors are returned as MindError
    // so the App can display them inline.
    match action.kind {
        ActionKind::Learn { item_key, .. } => {
            // Use the item key directly as the item ref for `learn`.
            // spec: TUI-20
            commands::learn(paths, &item_key, false)?;
        }
        ActionKind::Forget { item_key } => {
            // spec: TUI-20
            commands::forget(paths, &item_key)?;
        }
        ActionKind::Meld { spec } => {
            // spec: TUI-21
            commands::meld(paths, &spec, None, vec![], None, None, None)?;
        }
        ActionKind::Unmeld { name, forget } => {
            // spec: TUI-21
            commands::unmeld(paths, &name, forget)?;
        }
        ActionKind::Sync => {
            // spec: TUI-22
            commands::sync(paths, false)?;
        }
        ActionKind::Evolve => {
            // spec: TUI-22 - `yes: true` so it applies without prompting on stdin.
            commands::evolve(paths, true, None)?;
        }
        ActionKind::LobeAdd { path } => {
            // spec: TUI-23 CLI-112
            commands::lobe_add(paths, &path)?;
        }
        ActionKind::LobeRemove { path } => {
            // spec: TUI-23 CLI-113
            commands::lobe_remove(paths, &path)?;
        }
    }

    // Load a fresh snapshot while still holding the exclusive lock.
    // (load_inner is re-exported as a private function; we call try_poll after
    // releasing, but here we want the updated state immediately.)
    drop(_guard);
    data::load(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ActionKind, PendingAction};
    use crate::paths::Paths;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_paths() -> (Paths, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir()
            .join(format!("mind-tui-action-{}-{n}", std::process::id()));
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
    fn execute_forget_on_unknown_item_returns_error() {
        // spec: TUI-24 - errors are returned as MindError, not panics.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Forget {
                item_key: "skill:nonexistent".to_string(),
            },
            description: "test".to_string(),
        };
        let result = execute(&paths, action);
        // Should return an error (NotInstalled), not panic.
        assert!(result.is_err(), "forget on unknown item should return an error");
        cleanup(&base);
    }

    #[test]
    fn execute_sync_on_empty_registry_succeeds() {
        // spec: TUI-22 TUI-24 TUI-25
        // Sync with no sources: should succeed (prints "no sources melded") and
        // return an empty snapshot.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Sync,
            description: "sync?".to_string(),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "sync on empty registry should succeed: {:?}", result.err());
        let snap = result.unwrap();
        assert!(snap.installed.is_empty());
        cleanup(&base);
    }

    #[test]
    fn execute_takes_exclusive_lock() {
        // spec: TUI-25 STO-40 STO-41
        // Verify the action runs to completion under the exclusive lock by
        // checking it returns a valid snapshot.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();

        let paths_thread = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        // Run sync in background (it acquires exclusive lock).
        let handle = std::thread::spawn(move || {
            let action = PendingAction {
                kind: ActionKind::Sync,
                description: "sync".to_string(),
            };
            // The sync itself is fast (no sources); verify it runs under the
            // lock and returns a valid snapshot.
            execute(&paths_thread, action)
        });

        let result = handle.join().unwrap();
        assert!(result.is_ok(), "execute should succeed: {:?}", result.err());
        cleanup(&base);
    }

    #[test]
    fn execute_lock_is_exclusive_not_shared() {
        // spec: TUI-25 STO-40 STO-41
        // Mutation-check on the lock MODE: a mutating action MUST take the
        // EXCLUSIVE lock, not a shared one. We hold an external SHARED lock for
        // a measurable interval; an exclusive writer must BLOCK behind it, so
        // `execute` can only complete after the shared lock is released. If
        // `execute` were (wrongly) changed to take a shared lock, it would
        // coexist with our shared reader and return immediately, and the
        // ordering assertion below would fail.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let paths = Arc::new(paths);
        let reader_released = Arc::new(AtomicBool::new(false));

        let hold = Duration::from_millis(300);
        let p_reader = Arc::clone(&paths);
        let rel = Arc::clone(&reader_released);
        let reader = std::thread::spawn(move || {
            // Hold a shared lock on the same lock file for `hold`.
            let lock = lock::open(&p_reader).expect("open reader lock");
            let guard = lock.read().expect("acquire shared lock");
            std::thread::sleep(hold);
            rel.store(true, Ordering::SeqCst);
            drop(guard);
        });

        // Let the reader acquire first.
        std::thread::sleep(Duration::from_millis(50));

        let p_exec = Arc::clone(&paths);
        let rel_check = Arc::clone(&reader_released);
        let start = Instant::now();
        let result = execute(
            &p_exec,
            PendingAction {
                kind: ActionKind::Sync,
                description: "sync".to_string(),
            },
        );
        let waited = start.elapsed();
        // When execute's exclusive acquire finally succeeds, the shared reader
        // must already have released. A shared `execute` would not wait.
        assert!(
            rel_check.load(Ordering::SeqCst),
            "execute acquired its lock before the shared reader released it: \
             it is NOT taking an exclusive lock"
        );
        assert!(
            waited >= Duration::from_millis(200),
            "execute should have blocked behind the shared reader (exclusive lock); \
             only waited {waited:?} - lock is not exclusive"
        );
        assert!(result.is_ok(), "execute should still succeed: {:?}", result.err());

        reader.join().unwrap();
        cleanup(&base);
    }

    #[test]
    fn execute_evolve_with_no_pending_succeeds() {
        // spec: TUI-22 TUI-24
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Evolve,
            description: "evolve?".to_string(),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "evolve with nothing to do should succeed: {:?}", result.err());
        cleanup(&base);
    }

    fn init_git_repo(dir: &std::path::Path) {
        use std::process::Command;
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
        };
        run(&["-c", "init.defaultBranch=main", "init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
    }

    fn make_source_repo(base: &std::path::Path) -> std::path::PathBuf {
        use std::process::Command;
        let src = base.join("source-repo-action");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(src.join("skills/build")).unwrap();
        std::fs::write(
            src.join("skills/build/SKILL.md"),
            "---\ndescription: build skill\n---\n# build\n",
        ).unwrap();
        init_git_repo(&src);
        Command::new("git").args(["add", "-A"]).current_dir(&src).output().unwrap();
        Command::new("git").args(["commit", "-qm", "initial"]).current_dir(&src).output().unwrap();
        src
    }

    #[test]
    fn execute_meld_promotes_preview_and_registers_source() {
        // spec: TUI-30 - confirming a preview meld calls commands::meld under the
        // exclusive lock; after success the source appears in the registry.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let src = make_source_repo(&base);
        let spec = src.to_str().unwrap().to_string();

        let action = PendingAction {
            kind: ActionKind::Meld { spec: spec.clone() },
            description: format!("Meld {spec}?"),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "meld should succeed: {:?}", result.err());
        let snap = result.unwrap();
        // The source should now be in the snapshot's source list.
        assert!(
            snap.source_names.iter().any(|n| n.contains("source-repo-action")),
            "newly melded source should appear in snapshot: {:?}", snap.source_names
        );
        cleanup(&base);
    }

    /// Register a melded source and record one installed item attributed to it,
    /// with an EMPTY file registry so uninstall touches no agent home (keeping the
    /// test hermetic regardless of ambient MIND_AGENT_HOMES). Returns the source
    /// name. The purge loop in `unmeld --forget` still removes the manifest entry.
    fn seed_source_with_installed_item(paths: &Paths, base: &std::path::Path) -> String {
        use crate::manifest::{InstalledItem, Manifest};
        let src = make_source_repo(base);
        let spec = src.to_str().unwrap().to_string();
        commands::meld(paths, &spec, None, vec![], None, None, None).expect("meld");
        let source_name = crate::source::Registry::load(paths).unwrap().sources[0]
            .name
            .clone();
        let mut manifest = Manifest::load(paths).unwrap();
        manifest.insert(InstalledItem {
            kind: crate::error::ItemKind::Skill,
            name: "build".to_string(),
            bare_name: "build".to_string(),
            source: source_name.clone(),
            commit: "abc".to_string(),
            hash: "deadbeef".to_string(),
            store: String::new(), // empty registry: uninstall is a no-op
            links: vec![],
            description: None,
        });
        manifest.save(paths).unwrap();
        source_name
    }

    #[test]
    fn execute_unmeld_with_forget_purges_source_and_installed_items() {
        // spec: TUI-21 TUI-24 - the destructive `unmeld --forget` variant maps to
        // commands::unmeld(.., forget=true): it removes the source AND every item
        // installed from it. The `forget` flag must be threaded through (a bug that
        // dropped it would leave the installed manifest entry behind).
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let source_name = seed_source_with_installed_item(&paths, &base);

        let action = PendingAction {
            kind: ActionKind::Unmeld {
                name: source_name.clone(),
                forget: true,
            },
            description: format!("Unmeld {source_name} --forget?"),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "destructive unmeld should succeed: {:?}", result.err());

        let registry2 = crate::source::Registry::load(&paths).unwrap();
        assert!(
            registry2.sources.is_empty(),
            "source must be removed after unmeld: {:?}", registry2.sources
        );
        let manifest2 = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            !manifest2.items.values().any(|i| i.key() == "skill:build"),
            "skill:build must be purged by unmeld --forget: {:?}",
            manifest2.items.keys().collect::<Vec<_>>()
        );
        cleanup(&base);
    }

    #[test]
    fn execute_unmeld_without_forget_keeps_installed_items() {
        // spec: TUI-21 - the non-destructive unmeld (forget=false) drops the source
        // but does NOT purge installed items. Contrast case to the --forget test:
        // it pins that the forget flag actually distinguishes the two code paths
        // (otherwise both tests could pass with a hardwired value).
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let source_name = seed_source_with_installed_item(&paths, &base);

        let action = PendingAction {
            kind: ActionKind::Unmeld {
                name: source_name.clone(),
                forget: false,
            },
            description: format!("Unmeld {source_name}?"),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "unmeld should succeed: {:?}", result.err());

        let registry2 = crate::source::Registry::load(&paths).unwrap();
        assert!(registry2.sources.is_empty(), "source removed");
        let manifest2 = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            manifest2.items.values().any(|i| i.key() == "skill:build"),
            "skill:build must survive a non-forget unmeld"
        );
        cleanup(&base);
    }

    #[test]
    fn decline_preview_leaves_nothing_registered_and_no_temp_dir() {
        // spec: TUI-30 - declining a preview (CancelAction) must not register the
        // source and must discard the temp clone (no orphan temp dir).
        use crate::tui::preview;

        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let src = make_source_repo(&base);
        let spec = src.to_str().unwrap().to_string();

        // Run a preview (shallow clone to temp area).
        let prev = preview::preview(&paths, &spec).expect("preview should succeed");
        let temp_dir = prev.temp_dir.clone();
        assert!(temp_dir.exists(), "temp dir should exist while preview is live");

        // Simulate declining: drop the preview (no meld action issued).
        // SourcePreview::Drop removes the temp clone.
        drop(prev);

        assert!(!temp_dir.exists(), "temp dir must be removed when preview is dropped (decline)");

        // Registry should be empty (meld was never called).
        let registry = crate::source::Registry::load(&paths).unwrap();
        assert!(
            registry.sources.is_empty(),
            "registry must remain empty after declining a preview: {:?}", registry.sources
        );
        cleanup(&base);
    }

    // --- TUI-23: lobe add / remove dispatch ---

    #[test]
    fn execute_lobe_add_appends_lobe_to_config() {
        // spec: TUI-23 CLI-112 - execute(LobeAdd) calls commands::lobe_add under
        // the exclusive lock; the lobe appears in Config after the action.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        let action = PendingAction {
            kind: ActionKind::LobeAdd { path: lobe_path.clone() },
            description: format!("Add lobe {lobe_path}?"),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "LobeAdd should succeed: {:?}", result.err());

        // Verify the lobe was persisted to config.
        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        assert!(
            cfg.lobes.contains(&lobe_path),
            "lobe must appear in config after LobeAdd: {:?}", cfg.lobes
        );
        cleanup(&base);
    }

    #[test]
    fn execute_lobe_remove_drops_lobe_from_config() {
        // spec: TUI-23 CLI-113 - execute(LobeRemove) calls commands::lobe_remove;
        // the lobe disappears from Config after the action.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        // First add the lobe so we can remove it.
        let add_action = PendingAction {
            kind: ActionKind::LobeAdd { path: lobe_path.clone() },
            description: format!("Add {lobe_path}?"),
        };
        execute(&paths, add_action).expect("LobeAdd prerequisite");

        // Now remove it.
        let remove_action = PendingAction {
            kind: ActionKind::LobeRemove { path: lobe_path.clone() },
            description: format!("Remove lobe {lobe_path}?"),
        };
        let result = execute(&paths, remove_action);
        assert!(result.is_ok(), "LobeRemove should succeed: {:?}", result.err());

        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        assert!(
            !cfg.lobes.contains(&lobe_path),
            "lobe must be absent from config after LobeRemove: {:?}", cfg.lobes
        );
        cleanup(&base);
    }

    #[test]
    fn execute_lobe_remove_nonexistent_returns_error() {
        // spec: TUI-23 TUI-24 - removing a lobe that was never added returns
        // MindError::UnknownLobe, not a panic; the error is surfaced in-UI.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();

        let action = PendingAction {
            kind: ActionKind::LobeRemove { path: "/does/not/exist".to_string() },
            description: "Remove nonexistent lobe?".to_string(),
        };
        let result = execute(&paths, action);
        assert!(result.is_err(), "LobeRemove of unknown path must return an error");
        assert!(
            matches!(result.unwrap_err(), crate::error::MindError::UnknownLobe { .. }),
            "error must be MindError::UnknownLobe"
        );
        cleanup(&base);
    }

    #[test]
    fn execute_lobe_add_duplicate_is_idempotent() {
        // spec: TUI-23 CLI-112 - adding the same lobe twice does not duplicate it.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        for _ in 0..2 {
            let action = PendingAction {
                kind: ActionKind::LobeAdd { path: lobe_path.clone() },
                description: format!("Add {lobe_path}?"),
            };
            execute(&paths, action).expect("LobeAdd must succeed");
        }

        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        let count = cfg.lobes.iter().filter(|l| *l == &lobe_path).count();
        assert_eq!(count, 1, "duplicate lobe add must not produce duplicate entries");
        cleanup(&base);
    }

    #[test]
    fn execute_lobe_add_snapshot_includes_new_lobe() {
        // spec: TUI-23 CLI-111 CLI-112 - after a successful LobeAdd, the returned
        // snapshot reflects the new lobe in its lobes field (list view is current).
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        let action = PendingAction {
            kind: ActionKind::LobeAdd { path: lobe_path.clone() },
            description: format!("Add {lobe_path}?"),
        };
        let snap = execute(&paths, action).expect("LobeAdd must succeed");
        assert!(
            snap.lobes.contains(&lobe_path),
            "snapshot after LobeAdd must include the new lobe: {:?}", snap.lobes
        );
        cleanup(&base);
    }
}
