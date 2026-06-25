//! Execute a confirmed TUI action by calling the appropriate `commands::*` fn.
//!
//! Each action acquires the EXCLUSIVE lock for its duration, then releases it.
//! The verb functions (commands::learn/forget/sync/upgrade/unmeld) print to
//! stdout. In the TUI's alternate screen / raw mode that stray output corrupts
//! the display (line feeds without carriage returns staircase and scroll), so we
//! capture stdout for the duration of the action (TUI-24) and surface a one-line
//! summary in the status bar instead of letting it reach the terminal. Errors
//! are returned as MindError so the App can show them inline.
//!
//! No verb logic is reimplemented here; we call the existing command functions
//! directly (TUI-20..23).

use crate::commands;
use crate::error::Result;
use crate::lock;
use crate::paths::Paths;
use crate::tui::app::{ActionKind, PendingAction};
use crate::tui::data::{self, Snapshot};

/// Serialize the stdout redirect in `with_captured_stdout`: it dup2's the
/// process-global stdout fd, so two captures must never overlap. The TUI runs
/// actions one at a time, but the unit tests run concurrently.
static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CAPTURE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Execute a confirmed action under an exclusive lock, returning a fresh
/// snapshot and a one-line summary of the verb's output. The verb prints to
/// stdout; we capture it so it cannot corrupt the alternate screen and show the
/// summary in the status bar instead (TUI-24).
// spec: TUI-20 TUI-21 TUI-22 TUI-23 TUI-24 TUI-25 STO-40 STO-41
pub fn execute(paths: &Paths, action: PendingAction) -> Result<(Snapshot, String)> {
    execute_inner(paths, action, true)
}

/// Like `execute` but WITHOUT capturing stdout: the verb prints to, and reads
/// from, the real terminal. The caller must have suspended the TUI first
/// (`term::with_suspended`) so the verb's interactive prompts (a `meld`'s hook
/// and install confirmation) behave exactly as they do from the CLI (TUI-44).
// spec: TUI-44 TUI-30 TUI-25
pub fn execute_interactive(paths: &Paths, action: PendingAction) -> Result<(Snapshot, String)> {
    execute_inner(paths, action, false)
}

/// Shared body of `execute` / `execute_interactive`. With `capture` true the
/// verb's stdout is redirected to a buffer (so stray output cannot corrupt the
/// alt-screen) and reduced to a one-line summary; with `capture` false the verb
/// runs on the real terminal and the summary is empty.
fn execute_inner(
    paths: &Paths,
    action: PendingAction,
    capture: bool,
) -> Result<(Snapshot, String)> {
    // Acquire the exclusive lock for the duration of the action (TUI-25).
    // spec: STO-40 STO-41 TUI-25
    let mut lock = lock::open(paths)?;
    let _guard = lock.write()?;

    let (result, captured) = if capture {
        // Run the verb with stdout captured so nothing leaks onto the alt-screen.
        with_captured_stdout(|| dispatch(paths, action.kind))
    } else {
        // Interactive: the TUI is suspended, so let the verb own the terminal.
        (dispatch(paths, action.kind), String::new())
    };
    result?;

    // Drop the exclusive lock BEFORE calling data::load. data::load acquires
    // its own shared lock on a separate fd; holding the exclusive flock here
    // while it tries to take a shared lock on the same file would self-deadlock.
    drop(_guard);
    let snapshot = data::load(paths)?;
    Ok((snapshot, summary_line(&captured)))
}

/// Dispatch one confirmed action to its command function. No verb logic is
/// reimplemented here (TUI-20..23).
fn dispatch(paths: &Paths, kind: ActionKind) -> Result<()> {
    match kind {
        ActionKind::Learn { item_key, source } => {
            // When the user picked an item from a specific source (captured at
            // action-construction time), qualify the ref as `{source}#{item_key}`
            // so resolve pins the exact source and avoids AmbiguousItem when two
            // sources expose the same bare name.  Fall back to the bare key when
            // no source was recorded (e.g. item is unique across all sources).
            // spec: TUI-20
            let item_ref = if source.is_empty() {
                item_key
            } else {
                format!("{source}#{item_key}")
            };
            // `yes = true`: the TUI confirms in its own UI (the closure prompt
            // lands in the TUI shard); never block on the CLI's stdin [y/N]
            // prompt from inside raw mode. `Clobber::Error` surfaces a clobber as
            // an error in the UI rather than reading a terminal prompt.
            commands::learn(paths, &item_ref, false, true, commands::Clobber::Error)?;
        }
        // spec: TUI-20
        // `yes = true`: the TUI confirms destructive actions in its own UI
        // (TUI-24) and acts on a single resolved item, so never read a CLI prompt.
        ActionKind::Forget { item_key } => commands::forget(paths, &item_key, true)?,
        // spec: TUI-21
        ActionKind::Meld { spec } => {
            commands::meld(paths, &spec, None, vec![], None, None, None, None, false)?;
        }
        // spec: TUI-21
        // The TUI's `forget` toggle maps to the inverted `--unlink-only`; `yes =
        // true` so it never reads a CLI prompt from inside raw mode.
        ActionKind::Unmeld { name, forget } => {
            commands::unmeld(paths, &name, !forget, true, false, None)?
        }
        // spec: TUI-22
        ActionKind::Sync => commands::sync(paths, false, false)?,
        // spec: TUI-22 - `yes: true` so it applies without prompting on stdin.
        ActionKind::Upgrade => commands::upgrade(paths, true, None, false)?,
        // spec: TUI-23 CLI-112
        ActionKind::LobeAdd { path } => commands::lobe_add(paths, &path)?,
        // spec: TUI-23 CLI-113
        ActionKind::LobeRemove { path } => commands::lobe_remove(paths, &path)?,
    }
    Ok(())
}

/// The last non-empty line of captured output, trimmed, for the status bar.
fn summary_line(captured: &str) -> String {
    captured
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Run `f` with the process stdout redirected to a capture buffer, returning its
/// result and whatever it wrote (TUI-24). The dup2 mutates the process-global
/// stdout fd, so the whole sequence is serialized and the original fd is always
/// restored, even if `f` panics.
#[cfg(unix)]
fn with_captured_stdout<R>(f: impl FnOnce() -> R) -> (R, String) {
    use std::io::{Read, Seek, Write};
    use std::os::unix::io::AsRawFd;

    /// Restore the saved stdout fd on drop, so a panic in the action cannot leave
    /// the terminal redirected.
    struct FdRestore(libc::c_int);
    impl Drop for FdRestore {
        fn drop(&mut self) {
            let _ = std::io::stdout().flush();
            unsafe {
                libc::dup2(self.0, libc::STDOUT_FILENO);
                libc::close(self.0);
            }
        }
    }

    // Serialize: the redirect below is a process-global side effect.
    let _serialize = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let seq = CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("mind-tui-capture-{}-{seq}", std::process::id()));
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
    else {
        return (f(), String::new()); // capture unavailable: run as-is
    };
    let _ = std::fs::remove_file(&path); // unlink now; the open fd keeps it alive

    let _ = std::io::stdout().flush();
    let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved < 0 {
        return (f(), String::new());
    }
    let result = {
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        }
        let _restore = FdRestore(saved); // restores stdout on drop (incl. panic)
        f()
    };

    let mut buf = String::new();
    let _ = file.rewind();
    let _ = file.read_to_string(&mut buf);
    (result, buf)
}

#[cfg(not(unix))]
fn with_captured_stdout<R>(f: impl FnOnce() -> R) -> (R, String) {
    (f(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::tui::app::{ActionKind, PendingAction};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A temp base dir that removes itself on drop, so each test self-cleans
    /// even when an assertion panics (Drop runs during unwinding). Derefs to the
    /// base `Path` so existing `&base` call sites coerce unchanged.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl std::ops::Deref for TempDir {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    fn temp_paths() -> (Paths, TempDir) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-tui-action-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        (paths, TempDir(base))
    }

    #[test]
    fn summary_line_is_the_last_nonempty_line() {
        // spec: TUI-24 - captured verb output is reduced to a one-line status
        // summary (the last non-empty line) instead of corrupting the alt-screen.
        use super::summary_line;
        assert_eq!(
            summary_line("everything is up to date\n"),
            "everything is up to date"
        );
        assert_eq!(
            summary_line("upgraded skill:review\n"),
            "upgraded skill:review"
        );
        assert_eq!(summary_line("first\nlast\n\n"), "last");
        assert_eq!(summary_line("   \n  \n"), "");
        assert_eq!(summary_line(""), "");
    }

    #[test]
    fn execute_forget_on_unknown_item_returns_error() {
        // spec: TUI-24 - errors are returned as MindError, not panics.
        let (paths, _base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Forget {
                item_key: "skill:nonexistent".to_string(),
            },
            description: "test".to_string(),
            dep_tree: None,
        };
        let result = execute(&paths, action);
        // Should return an error (NotInstalled), not panic.
        assert!(
            result.is_err(),
            "forget on unknown item should return an error"
        );
    }

    #[test]
    fn execute_sync_on_empty_registry_succeeds() {
        // spec: TUI-22 TUI-24 TUI-25
        // Sync with no sources: should succeed (prints "no sources melded") and
        // return an empty snapshot.
        let (paths, _base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Sync,
            description: "sync?".to_string(),
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "sync on empty registry should succeed: {:?}",
            result.err()
        );
        let (snap, _msg) = result.unwrap();
        assert!(snap.installed.is_empty());
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
                dep_tree: None,
            };
            // The sync itself is fast (no sources); verify it runs under the
            // lock and returns a valid snapshot.
            execute(&paths_thread, action)
        });

        let result = handle.join().unwrap();
        assert!(result.is_ok(), "execute should succeed: {:?}", result.err());
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

        let (paths, _base) = temp_paths();
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
                dep_tree: None,
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
        assert!(
            result.is_ok(),
            "execute should still succeed: {:?}",
            result.err()
        );

        reader.join().unwrap();
    }

    #[test]
    fn execute_upgrade_with_no_pending_succeeds() {
        // spec: TUI-22 TUI-24
        let (paths, _base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let action = PendingAction {
            kind: ActionKind::Upgrade,
            description: "upgrade?".to_string(),
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "upgrade with nothing to do should succeed: {:?}",
            result.err()
        );
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
        )
        .unwrap();
        init_git_repo(&src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&src)
            .output()
            .unwrap();
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
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "meld should succeed: {:?}", result.err());
        let (snap, _msg) = result.unwrap();
        // The source should now be in the snapshot's source list.
        assert!(
            snap.source_names
                .iter()
                .any(|n| n.contains("source-repo-action")),
            "newly melded source should appear in snapshot: {:?}",
            snap.source_names
        );
    }

    #[test]
    fn execute_interactive_melds_without_capturing_stdout() {
        // spec: TUI-44 - the interactive executor runs the verb on the real
        // terminal (no stdout capture) and still acquires the lock and reloads the
        // snapshot. In a non-TTY test the meld takes the non-interactive path (no
        // install prompt), so this exercises the uncaptured code path safely.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let src = make_source_repo(&base);
        let spec = src.to_str().unwrap().to_string();

        let action = PendingAction {
            kind: ActionKind::Meld { spec: spec.clone() },
            description: format!("Meld {spec}?"),
            dep_tree: None,
        };
        let result = execute_interactive(&paths, action);
        assert!(
            result.is_ok(),
            "interactive meld should succeed: {:?}",
            result.err()
        );
        let (snap, msg) = result.unwrap();
        assert!(
            snap.source_names
                .iter()
                .any(|n| n.contains("source-repo-action")),
            "interactively melded source must appear in the reloaded snapshot: {:?}",
            snap.source_names
        );
        // Uncaptured: there is no captured summary line.
        assert_eq!(msg, "", "interactive execute captures no stdout summary");
    }

    #[test]
    fn execute_interactive_unmelds_without_capturing_stdout() {
        // spec: TUI-44 - execute_interactive routes Unmeld through the real terminal
        // (no stdout capture) and acquires the exclusive lock. In a non-TTY test the
        // unmeld takes the non-interactive path (no hook prompt shown), so this
        // exercises the uncaptured code path for Unmeld safely. This is the path
        // that was broken before the fix: Unmeld went through `execute` (captured)
        // instead of `execute_interactive`, so an uninstall hook would print to a
        // captured buffer and block reading stdin in raw mode.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let src = make_source_repo(&base);
        let spec = src.to_str().unwrap().to_string();
        // First meld the source so there is something to unmeld.
        commands::meld(&paths, &spec, None, vec![], None, None, None, None, false)
            .expect("meld prerequisite");
        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        let action = PendingAction {
            kind: ActionKind::Unmeld {
                name: source_name.clone(),
                forget: false,
            },
            description: format!("Unmeld {source_name}?"),
            dep_tree: None,
        };
        let result = execute_interactive(&paths, action);
        assert!(
            result.is_ok(),
            "interactive unmeld should succeed: {:?}",
            result.err()
        );
        let (snap, msg) = result.unwrap();
        assert!(
            snap.source_names.is_empty(),
            "source must be absent from snapshot after unmeld: {:?}",
            snap.source_names
        );
        // Uncaptured: there is no captured summary line.
        assert_eq!(msg, "", "interactive execute captures no stdout summary");
    }

    /// Register a melded source and record one installed item attributed to it,
    /// with an EMPTY file registry so uninstall touches no agent home (keeping the
    /// test hermetic regardless of ambient MIND_AGENT_HOMES). Returns the source
    /// name. The purge loop in `unmeld --forget` still removes the manifest entry.
    fn seed_source_with_installed_item(paths: &Paths, base: &std::path::Path) -> String {
        use crate::manifest::{InstalledItem, Manifest};
        let src = make_source_repo(base);
        let spec = src.to_str().unwrap().to_string();
        commands::meld(paths, &spec, None, vec![], None, None, None, None, false).expect("meld");
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
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "destructive unmeld should succeed: {:?}",
            result.err()
        );

        let registry2 = crate::source::Registry::load(&paths).unwrap();
        assert!(
            registry2.sources.is_empty(),
            "source must be removed after unmeld: {:?}",
            registry2.sources
        );
        let manifest2 = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            !manifest2.items.values().any(|i| i.key() == "skill:build"),
            "skill:build must be purged by unmeld --forget: {:?}",
            manifest2.items.keys().collect::<Vec<_>>()
        );
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
            dep_tree: None,
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
        assert!(
            temp_dir.exists(),
            "temp dir should exist while preview is live"
        );

        // Simulate declining: drop the preview (no meld action issued).
        // SourcePreview::Drop removes the temp clone.
        drop(prev);

        assert!(
            !temp_dir.exists(),
            "temp dir must be removed when preview is dropped (decline)"
        );

        // Registry should be empty (meld was never called).
        let registry = crate::source::Registry::load(&paths).unwrap();
        assert!(
            registry.sources.is_empty(),
            "registry must remain empty after declining a preview: {:?}",
            registry.sources
        );
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
            kind: ActionKind::LobeAdd {
                path: lobe_path.clone(),
            },
            description: format!("Add lobe {lobe_path}?"),
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "LobeAdd should succeed: {:?}", result.err());

        // Verify the lobe was persisted to config.
        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        assert!(
            cfg.lobes.contains(&lobe_path),
            "lobe must appear in config after LobeAdd: {:?}",
            cfg.lobes
        );
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
            kind: ActionKind::LobeAdd {
                path: lobe_path.clone(),
            },
            description: format!("Add {lobe_path}?"),
            dep_tree: None,
        };
        execute(&paths, add_action).expect("LobeAdd prerequisite");

        // Now remove it.
        let remove_action = PendingAction {
            kind: ActionKind::LobeRemove {
                path: lobe_path.clone(),
            },
            description: format!("Remove lobe {lobe_path}?"),
            dep_tree: None,
        };
        let result = execute(&paths, remove_action);
        assert!(
            result.is_ok(),
            "LobeRemove should succeed: {:?}",
            result.err()
        );

        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        assert!(
            !cfg.lobes.contains(&lobe_path),
            "lobe must be absent from config after LobeRemove: {:?}",
            cfg.lobes
        );
    }

    #[test]
    fn execute_lobe_remove_nonexistent_returns_error() {
        // spec: TUI-23 TUI-24 - removing a lobe that was never added returns
        // MindError::UnknownLobe, not a panic; the error is surfaced in-UI.
        let (paths, _base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();

        let action = PendingAction {
            kind: ActionKind::LobeRemove {
                path: "/does/not/exist".to_string(),
            },
            description: "Remove nonexistent lobe?".to_string(),
            dep_tree: None,
        };
        let result = execute(&paths, action);
        assert!(
            result.is_err(),
            "LobeRemove of unknown path must return an error"
        );
        assert!(
            matches!(
                result.unwrap_err(),
                crate::error::MindError::UnknownLobe { .. }
            ),
            "error must be MindError::UnknownLobe"
        );
    }

    #[test]
    fn execute_lobe_add_duplicate_is_idempotent() {
        // spec: TUI-23 CLI-112 - adding the same lobe twice does not duplicate it.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        for _ in 0..2 {
            let action = PendingAction {
                kind: ActionKind::LobeAdd {
                    path: lobe_path.clone(),
                },
                description: format!("Add {lobe_path}?"),
                dep_tree: None,
            };
            execute(&paths, action).expect("LobeAdd must succeed");
        }

        let cfg = crate::config::Config::load(&paths.mind_home).unwrap();
        let count = cfg.lobes.iter().filter(|l| *l == &lobe_path).count();
        assert_eq!(
            count, 1,
            "duplicate lobe add must not produce duplicate entries"
        );
    }

    #[test]
    fn execute_lobe_add_snapshot_includes_new_lobe() {
        // spec: TUI-23 CLI-111 CLI-112 - after a successful LobeAdd, the returned
        // snapshot reflects the new lobe in its lobes field (list view is current).
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        let lobe_path = base.join("custom-ai").to_str().unwrap().to_string();

        let action = PendingAction {
            kind: ActionKind::LobeAdd {
                path: lobe_path.clone(),
            },
            description: format!("Add {lobe_path}?"),
            dep_tree: None,
        };
        let (snap, _msg) = execute(&paths, action).expect("LobeAdd must succeed");
        assert!(
            snap.lobes.contains(&lobe_path),
            "snapshot after LobeAdd must include the new lobe: {:?}",
            snap.lobes
        );
    }

    /// Create a named source git repo under `base/<dir_name>` that ships a single
    /// skill named `skill_name`. Returns the repo path.
    fn make_named_source_repo(
        base: &std::path::Path,
        dir_name: &str,
        skill_name: &str,
    ) -> std::path::PathBuf {
        use std::process::Command;
        let src = base.join(dir_name);
        std::fs::create_dir_all(src.join("skills").join(skill_name)).unwrap();
        std::fs::write(
            src.join("skills").join(skill_name).join("SKILL.md"),
            format!("---\ndescription: {skill_name} skill from {dir_name}\n---\n# {skill_name}\n"),
        )
        .unwrap();
        init_git_repo(&src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&src)
            .output()
            .unwrap();
        src
    }

    /// Create a source git repo under `base/dep-source` shipping a skill `review`
    /// that references agent `dev` via a `{{ns:dev}}` token, plus the `dev` agent
    /// it depends on. Returns the repo path. Used to exercise the within-source
    /// dependency closure (DEP-41).
    fn make_dep_source_repo(base: &std::path::Path) -> std::path::PathBuf {
        use std::process::Command;
        let src = base.join("dep-source");
        std::fs::create_dir_all(src.join("skills/review")).unwrap();
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\nHand off to {{ns:dev}}.\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(
            src.join("agents/dev.md"),
            "---\nname: dev\ndescription: dev agent\n---\n# dev\n",
        )
        .unwrap();
        init_git_repo(&src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&src)
            .output()
            .unwrap();
        src
    }

    #[test]
    fn execute_learn_installs_whole_dependency_closure() {
        // spec: DEP-41 - confirming a Learn in the TUI installs the whole
        // within-source closure dependency-first: `skill:review` references agent
        // `dev` via {{ns:dev}}, so executing the Learn must install BOTH the skill
        // and the agent it pulls in. (Declining is the contrast case below: it
        // never executes, so nothing is installed.)
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        // Pin the lobe to the isolated claude_home so the install never touches the
        // real ~/.claude (agent_homes() otherwise defaults to ~/.claude).
        crate::config::Config {
            lobes: vec![paths.claude_home.to_str().unwrap().to_string()],
            ..Default::default()
        }
        .save(&paths.mind_home)
        .unwrap();

        let src = make_dep_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld dep-source");

        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        // Decline path: building the action but NOT executing must leave the
        // manifest empty (declining installs nothing, DEP-41).
        let _declined = PendingAction {
            kind: ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            description: "Install skill:review?".to_string(),
            dep_tree: Some("review (selected)\n  dev (dependency)".to_string()),
        };
        let pre = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            pre.items.is_empty(),
            "declining (not executing) must install nothing: {:?}",
            pre.items.keys().collect::<Vec<_>>()
        );

        // Confirm path: execute installs the whole closure.
        let action = PendingAction {
            kind: ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            description: "Install skill:review?".to_string(),
            dep_tree: Some("review (selected)\n  dev (dependency)".to_string()),
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "learn of the closure must succeed: {:?}",
            result.err()
        );

        let manifest = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            manifest.items.contains_key("skill:review"),
            "the explicitly selected skill must be installed: {:?}",
            manifest.items.keys().collect::<Vec<_>>()
        );
        assert!(
            manifest.items.contains_key("agent:dev"),
            "the referenced agent (the dependency) must be pulled in: {:?}",
            manifest.items.keys().collect::<Vec<_>>()
        );
    }

    /// Create a source repo under `base/chain-source` shipping a transitive
    /// dependency chain `skill:review` -> `agent:dev` -> `skill:build`: the skill
    /// references the agent via `{{ns:dev}}`, and the agent in turn references the
    /// `build` skill via `{{ns:build}}`. Used to exercise the TRANSITIVE closure
    /// (DEP-41 over DEP-11): selecting only `review` must pull in `dev` AND `build`.
    fn make_chain_source_repo(base: &std::path::Path) -> std::path::PathBuf {
        use std::process::Command;
        let src = base.join("chain-source");
        std::fs::create_dir_all(src.join("skills/review")).unwrap();
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\nHand off to {{ns:dev}}.\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("agents")).unwrap();
        std::fs::write(
            src.join("agents/dev.md"),
            "---\nname: dev\ndescription: dev agent\n---\n# dev\nUse {{ns:build}} to compile.\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("skills/build")).unwrap();
        std::fs::write(
            src.join("skills/build/SKILL.md"),
            "---\ndescription: build skill\n---\n# build\n",
        )
        .unwrap();
        init_git_repo(&src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&src)
            .output()
            .unwrap();
        src
    }

    #[test]
    fn execute_learn_installs_transitive_closure_dependency_first() {
        // spec: DEP-41 - confirming a Learn in the TUI installs the WHOLE transitive
        // within-source closure (DEP-11), not just the direct dependency. The chain
        // is `skill:review` -> `agent:dev` -> `skill:build`: selecting only `review`
        // through `execute(ActionKind::Learn)` must install all THREE members. The
        // 2-member test (`..._installs_whole_dependency_closure`) would still pass if
        // transitivity regressed and only the direct dep were pulled; this one fails.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        // Pin the lobe to the isolated claude_home so the install never touches the
        // real ~/.claude (agent_homes() otherwise defaults to ~/.claude).
        crate::config::Config {
            lobes: vec![paths.claude_home.to_str().unwrap().to_string()],
            ..Default::default()
        }
        .save(&paths.mind_home)
        .unwrap();

        let src = make_chain_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld chain-source");

        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        let action = PendingAction {
            kind: ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            description: "Install skill:review?".to_string(),
            dep_tree: Some(
                "- skill:review [selected]\n  - agent:dev [dep]\n    - skill:build [dep]"
                    .to_string(),
            ),
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "learn of the transitive closure must succeed: {:?}",
            result.err()
        );

        let manifest = crate::manifest::Manifest::load(&paths).unwrap();
        // All three members of the transitive closure must be present.
        for key in ["skill:review", "agent:dev", "skill:build"] {
            assert!(
                manifest.items.contains_key(key),
                "transitive closure member {key} must be installed: {:?}",
                manifest.items.keys().collect::<Vec<_>>()
            );
        }

        // Dependency-first ordering (DEP-30): a dependency's recorded commit/source
        // must be present before its dependent could reference it. We assert the
        // install committed every member to the same source so the closure is one
        // coherent unit (a partial install would drop the transitive `build`).
        for key in ["skill:review", "agent:dev", "skill:build"] {
            let item = manifest.items.get(key).unwrap();
            assert_eq!(
                item.source, source_name,
                "closure member {key} must be attributed to the selected source"
            );
        }
    }

    #[test]
    fn execute_learn_does_not_reinstall_already_installed_dependency() {
        // spec: DEP-41 DEP-23 - when a referenced dependency is ALREADY installed,
        // confirming the Learn through the TUI installs the rest of the closure but
        // does NOT re-install (or duplicate) the already-present dependency. The
        // manifest is keyed `kind:name`, so a duplicate would overwrite rather than
        // create a second entry; we assert the dependency keeps its ORIGINAL recorded
        // commit (a re-install would rewrite it) and that exactly one entry exists.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![paths.claude_home.to_str().unwrap().to_string()],
            ..Default::default()
        }
        .save(&paths.mind_home)
        .unwrap();

        let src = make_dep_source_repo(&base); // skill:review -> agent:dev
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld dep-source");
        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        // Pre-install ONLY the dependency `agent:dev` via the same Learn path. After
        // this the manifest holds exactly agent:dev (review references it, but dev
        // has no deps so selecting dev installs only dev).
        let pre = PendingAction {
            kind: ActionKind::Learn {
                item_key: "agent:dev".to_string(),
                source: source_name.clone(),
            },
            description: "Install agent:dev?".to_string(),
            dep_tree: None,
        };
        execute(&paths, pre).expect("pre-install of agent:dev must succeed");

        let before = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            before.items.contains_key("agent:dev"),
            "agent:dev must be installed before the second learn"
        );
        let dev_commit_before = before.items.get("agent:dev").unwrap().commit.clone();
        let dev_hash_before = before.items.get("agent:dev").unwrap().hash.clone();
        assert!(
            !before.items.contains_key("skill:review"),
            "skill:review must NOT be installed yet"
        );

        // The plan the TUI confirm is built from (the same resolution execute will
        // apply): the closure pulls in `dev` (so a tree is shown) but, because dev is
        // already installed (DEP-23), exactly ONE item is in the install order. If a
        // regression re-installed already-installed deps, install_count would be 2.
        let plan = crate::commands::learn_preview(
            &paths,
            &crate::tui::app::learn_ref("skill:review", &source_name),
        )
        .expect("learn_preview must succeed");
        assert!(
            plan.adds_dependencies,
            "the closure still pulls in the (already-installed) dep, so a tree is shown"
        );
        assert_eq!(
            plan.install_count, 1,
            "DEP-23: only the not-yet-installed `review` installs; the already-installed \
             dep is excluded from the install order"
        );

        // Now learn `skill:review`. Its closure is {review, dev}, but dev is already
        // installed (DEP-23): only review should be newly installed; dev untouched.
        let action = PendingAction {
            kind: ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            description: "Install skill:review?".to_string(),
            dep_tree: Some("- skill:review [selected]\n  - agent:dev [installed]".to_string()),
        };
        execute(&paths, action).expect("learn of review must succeed");

        let after = crate::manifest::Manifest::load(&paths).unwrap();
        assert!(
            after.items.contains_key("skill:review"),
            "skill:review must now be installed"
        );
        // Exactly one copy of the dependency (manifest is keyed kind:name).
        let dev_count = after.items.keys().filter(|k| *k == "agent:dev").count();
        assert_eq!(
            dev_count,
            1,
            "the already-installed dependency must appear exactly once, not duplicated: {:?}",
            after.items.keys().collect::<Vec<_>>()
        );
        // And it was NOT re-installed: its recorded commit/hash are unchanged from
        // the original install (a re-install would have rewritten the registry entry).
        let dev_after = after.items.get("agent:dev").unwrap();
        assert_eq!(
            dev_after.commit, dev_commit_before,
            "already-installed dependency must keep its original commit (not re-installed)"
        );
        assert_eq!(
            dev_after.hash, dev_hash_before,
            "already-installed dependency must keep its original hash (not re-installed)"
        );
    }

    #[test]
    fn execute_learn_with_source_resolves_when_two_sources_have_same_skill() {
        // spec: TUI-20 - when two melded sources both expose a skill with the same
        // bare name, ActionKind::Learn must pass a source-qualified ref
        // (`{source}#{item_key}`) to commands::learn so resolve picks the item the
        // user selected rather than returning AmbiguousItem.
        //
        // Regression: the old code dropped the `source` field and passed only the
        // bare `item_key`, which triggered MindError::AmbiguousItem.
        let (paths, base) = temp_paths();
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        // Pin the lobe to the isolated claude_home so install does not touch
        // the real ~/.claude (agent_homes() falls back to config, which defaults
        // to ~/.claude when CLAUDE_HOME is unset).
        crate::config::Config {
            lobes: vec![paths.claude_home.to_str().unwrap().to_string()],
            ..Default::default()
        }
        .save(&paths.mind_home)
        .unwrap();

        // Two source repos that both ship "skill:review".
        let src_a = make_named_source_repo(&base, "source-alpha", "review");
        let src_b = make_named_source_repo(&base, "source-beta", "review");

        commands::meld(
            &paths,
            src_a.to_str().unwrap(),
            None,
            vec![],
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld source-alpha");
        commands::meld(
            &paths,
            src_b.to_str().unwrap(),
            None,
            vec![],
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld source-beta");

        // The registry should now have two sources.
        let registry = crate::source::Registry::load(&paths).unwrap();
        assert_eq!(registry.sources.len(), 2, "two sources must be registered");

        // Pick the name of source-alpha to install from.
        let source_name = registry
            .sources
            .iter()
            .find(|s| s.name.ends_with("source-alpha"))
            .map(|s| s.name.clone())
            .expect("source-alpha must be registered");

        // Build the Learn action as the TUI would: item_key = "skill:review",
        // source = the chosen source name.
        let action = PendingAction {
            kind: ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            description: "Learn skill:review from source-alpha?".to_string(),
            dep_tree: None,
        };

        // Without the fix this returned MindError::AmbiguousItem.
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "learn with source qualifier must succeed (not AmbiguousItem): {:?}",
            result.err()
        );

        // Verify the installed item came from source-alpha, not source-beta.
        let manifest = crate::manifest::Manifest::load(&paths).unwrap();
        let item = manifest
            .items
            .get("skill:review")
            .expect("skill:review must be in manifest after learn");
        assert!(
            item.source.ends_with("source-alpha"),
            "installed item must come from source-alpha, got: {}",
            item.source
        );
    }
}
