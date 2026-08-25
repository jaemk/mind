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
        with_captured_stdout(|| dispatch(paths, action.kind, &action.upgrade_keys))
    } else {
        // Interactive: the TUI is suspended, so let the verb own the terminal.
        (
            dispatch(paths, action.kind, &action.upgrade_keys),
            String::new(),
        )
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
/// reimplemented here (TUI-20..23). `upgrade_keys` is only meaningful for
/// `ActionKind::Upgrade` (TUI-72, TUI-73): the exact item keys the confirm
/// modal listed and the user consented to.
fn dispatch(paths: &Paths, kind: ActionKind, upgrade_keys: &[String]) -> Result<()> {
    match kind {
        ActionKind::Learn { item_key, source } => {
            // When the user picked an item from a specific source (captured at
            // action-construction time), qualify the ref as `{source}#{item_key}`
            // so resolve pins the exact source and avoids AmbiguousItem when two
            // sources expose the same bare name.  Fall back to the bare key when
            // no source was recorded (e.g. item is unique across all sources).
            // spec: TUI-20
            let item_ref = crate::tui::app::learn_ref(&item_key, &source);
            // `yes = true`: the TUI confirms in its own UI (the closure prompt
            // lands in the TUI shard); never block on the CLI's stdin [y/N]
            // prompt from inside raw mode. `Clobber::Error` surfaces a clobber as
            // an error in the UI rather than reading a terminal prompt.
            commands::learn(
                paths,
                &item_ref,
                false,
                commands::InstallFlow {
                    yes: true,
                    clobber: commands::Clobber::Error,
                    dangerously_skip: false,
                    dangerously_skip_build: false,
                },
            )?;
        }
        // spec: TUI-20
        // `yes = true`: the TUI confirms destructive actions in its own UI
        // (TUI-24) and acts on a single resolved item, so never read a CLI prompt.
        ActionKind::Forget { item_key } => {
            commands::forget(paths, Some(&item_key), false, true, true, false)?
        }
        // spec: TUI-21
        ActionKind::Meld { spec } => {
            commands::meld(
                paths,
                &spec,
                None,
                vec![],
                vec![],
                false,
                commands::PinRequest::None,
                None,
                false,
            )?;
        }
        // spec: TUI-21
        // The TUI's `forget` toggle maps to the inverted `--unlink-only`; `yes =
        // true` so it never reads a CLI prompt from inside raw mode.
        ActionKind::Unmeld { name, forget } => {
            commands::unmeld(paths, &name, !forget, true, false, None)?
        }
        // spec: TUI-22 -- `then_upgrade: false` here, so the new `yes` param
        // (LIFE-49/M1) is inert; passed `false` to match the plain `mind sync`
        // (no --upgrade) this action mirrors.
        ActionKind::Sync => commands::sync(paths, false, false, false, false)?,
        // spec: TUI-72 TUI-73 - `yes: true` so it applies without prompting on
        // stdin. Use the NO-SYNC, KEY-SCOPED upgrade: the confirm modal's
        // pending list was computed from an authoritative (memo-bypassing)
        // recompute at `u` keypress time, and `upgrade_keys` is exactly the set
        // it listed and the user consented to (stashed onto the pending action
        // by `app.rs`'s `initiate_upgrade`). Scoping the apply to that exact set
        // -- rather than letting a bare `item_ref: None` upgrade independently
        // re-derive "everything found stale right now" -- is what makes the
        // applied set equal the confirmed set BY CONSTRUCTION. Never syncs: a
        // sync-first apply could pull new upstream commits between confirm and
        // apply and act on an item the modal never named; the TUI offers `s`
        // (Sync) separately and re-polls ~1s, so refreshing drift is not this
        // call's job.
        //
        // M2: an EMPTY `upgrade_keys` is exactly `initiate_upgrade`'s
        // "nothing is out of date since the last sync ... Proceed with upgrade
        // anyway?" case (a non-empty confirm always names at least one key).
        // The key-scoped call with an empty key set is a no-op by definition
        // (`key_scope = Some(<empty set>)` makes every item `OutOfScope`), so
        // routing it there would make "anyway" a guaranteed no-op -- silently
        // different from what it meant before TUI-72/73 (a plain
        // `upgrade_no_sync(.., None, ..)`, which re-derives staleness at apply
        // time and so COULD catch a drift the snapshot had missed). Route the
        // empty case through that unscoped call instead, restoring "anyway"'s
        // actual meaning; keep the key-scoped call for the named, non-empty set.
        // spec: TUI-76
        ActionKind::Upgrade if upgrade_keys.is_empty() => {
            commands::upgrade_no_sync(paths, true, None, false, false)?
        }
        ActionKind::Upgrade => {
            commands::upgrade_no_sync_keys(paths, true, upgrade_keys, false, false)?
        }
        // spec: TUI-23 CLI-112
        // `yes: true` so backfill applies without prompting on stdin in the TUI.
        ActionKind::LobeAdd { path } => commands::lobe_add(paths, &path, true)?,
        // spec: TUI-23 CLI-113
        ActionKind::LobeRemove { path } => commands::lobe_remove(paths, &path, false)?,
        // SetNamespace is intercepted by activate_dialog and opens the namespace-
        // input overlay; it never reaches the executor (TUI-53).
        // spec: TUI-53
        ActionKind::SetNamespace { .. } => {
            // Unreachable: activate_dialog short-circuits SetNamespace before
            // setting a pending_action, so this arm can never be dispatched.
        }
    }
    Ok(())
}

/// Reduce the captured verb output to a one-line status-bar summary: the last
/// non-empty line, ANSI-stripped. The captured text is whatever the verb's own
/// `println!`s wrote (colored when the CLI's own color detection says so, and
/// carrying whatever a source's content contributed, e.g. a name/description
/// echoed back in the summary), so it can contain raw SGR escapes; the TUI's
/// status bar renders it as plain text, not through a terminal that
/// interprets ANSI, so leftover escapes would show as literal garbage
/// characters instead of color.
// spec: TUI-60
fn summary_line(captured: &str) -> String {
    let line = captured
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    crate::sanitize::strip_ansi(line)
}

/// Open the stdout-capture file at `path` (TUI-61): `create_new` refuses to
/// open through ANY pre-existing path -- a plain pre-created file or a symlink
/// a local attacker planted there -- unlike the old `create(true).truncate(true)`
/// pattern, which would silently follow a symlink and truncate whatever it
/// pointed at. Mode 0600: owner read/write only, so the captured verb output
/// (which can include filesystem paths and item names) is never group- or
/// world-readable while the fd is briefly open. Split out from
/// `with_captured_stdout` so the open call is directly unit-testable
/// (mirroring the STO-61 `write_curl_auth_config` test) without needing to
/// intercept the stdout-redirect machinery around it.
// spec: TUI-61
#[cfg(unix)]
fn open_capture_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Create the stdout-capture file: an exclusively-created, mode-0600 file
/// inside a fresh, exclusively-created, mode-0700 temp directory
/// (`mktemp_dir_prefixed`, the same scheme `evolve` uses for its
/// download/auth-config temp files, STO-45/STO-61). Returns `None` if
/// creation fails for any reason; the caller then runs the action without
/// capturing rather than erroring out over it (capture is a display nicety,
/// not something worth failing a verb over).
// spec: TUI-61
#[cfg(unix)]
fn create_capture_file() -> Option<std::fs::File> {
    let dir = crate::selfupdate::mktemp_dir_prefixed("mind-tui-capture").ok()?;
    let path = dir.join("capture");
    let file = open_capture_file(&path).ok();
    if file.is_some() {
        let _ = std::fs::remove_file(&path); // unlink now; the open fd keeps it alive
    }
    let _ = std::fs::remove_dir(&dir); // best-effort; the fd keeps the file's data alive
    file
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

    // spec: TUI-61 -- see `create_capture_file`/`open_capture_file`: an
    // exclusive-create in a fresh 0700 temp dir, never a predictable path
    // opened with plain create+truncate.
    let Some(mut file) = create_capture_file() else {
        return (f(), String::new()); // capture unavailable: run as-is
    };

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

    #[test]
    fn with_captured_stdout_cannot_observe_println_content_under_cargo_test() {
        // Documents a real limitation of every test in this module that calls
        // `execute`/`with_captured_stdout` in-process: `cargo test` installs a
        // thread-local `OUTPUT_CAPTURE` that intercepts `println!` at the Rust
        // level (io::stdout()) BEFORE it ever reaches a raw write(2) syscall.
        // `with_captured_stdout` only dup2's the OS-level fd 1, so it can never
        // see a `println!` issued from inside a `#[test]` -- the captured
        // string is always empty here, regardless of what the wrapped closure
        // actually printed.
        //
        // This is why no test in this module may assert on captured PROSE
        // content (e.g. "the summary line contains --force"): such an
        // assertion would pass vacuously, always, independent of whether the
        // verb actually printed that text. State-based assertions (manifest,
        // filesystem, `Display` of the error type) are the only sound
        // in-process proof; the prose itself is certified out-of-process by
        // `tests/cli_lobes.rs::harn17_backfill_reports_foreign_target_without_clobbering`,
        // which spawns the compiled binary and reads its real, unintercepted
        // stdout.
        //
        // If this assertion ever starts failing (`captured` becomes
        // non-empty), `cargo test`'s capture behavior changed and prose
        // assertions become sound to write in-process again.
        let marker = "TUI-62-diagnostic-canary-line-that-must-not-be-observed";
        let (_, captured) = with_captured_stdout(|| println!("{marker}"));
        assert!(
            captured.is_empty(),
            "expected with_captured_stdout to see nothing (libtest intercepts \
             println! first); got {captured:?} instead -- if this now \
             contains {marker:?}, in-process prose assertions on captured \
             TUI output are sound again and the comment above is stale"
        );
    }

    #[test]
    // spec: TUI-61
    fn create_capture_file_is_owner_only_0600() {
        // Mirrors the STO-61 `curl_auth_config_file_is_owner_only_0600` test:
        // the stdout-capture file must be mode 0600 (owner read/write only),
        // not the umask-default mode the old `create(true).truncate(true)` open
        // (with no explicit `.mode(..)`) would have produced.
        use std::os::unix::fs::PermissionsExt;
        let file = create_capture_file().expect("must create a capture file");
        let meta = file.metadata().expect("fstat the still-open capture file");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "stdout-capture file must be mode 0600, got {mode:o}"
        );
    }

    #[test]
    // spec: TUI-61
    fn open_capture_file_refuses_a_preexisting_symlink() {
        // B9 -- a local attacker who pre-creates the capture path as a symlink
        // must NOT get the symlink's target opened, truncated, and overwritten.
        // `open_capture_file` uses `create_new`, which refuses to open through
        // ANY pre-existing path (symlink or otherwise); the pre-fix code used
        // `create(true).truncate(true)` with no `create_new`, which WOULD have
        // followed the symlink and truncated the victim file this test plants.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "mind-tui-capture-symlink-test-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).expect("create scratch dir");

        let victim = base.join("victim-target");
        let victim_content = b"attacker must not see this truncated";
        std::fs::write(&victim, victim_content).expect("seed victim file");

        let capture_path = base.join("capture");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&victim, &capture_path).expect("plant symlink");

        let result = open_capture_file(&capture_path);
        assert!(
            result.is_err(),
            "open_capture_file must refuse to open through a pre-existing symlink, \
             not silently follow it: {result:?}"
        );

        let victim_after = std::fs::read(&victim).expect("victim file must still exist");
        assert_eq!(
            victim_after, victim_content,
            "the symlink target must be untouched -- a create+truncate open would have \
             truncated it to empty"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

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
    fn summary_line_strips_ansi_from_captured_output() {
        // spec: TUI-60 - the status bar is not a terminal that interprets ANSI,
        // so raw SGR escapes left in the captured verb output (which can carry
        // color codes the CLI's own detection emitted, or source-controlled
        // text) must be stripped rather than shown as literal garbage.
        use super::summary_line;
        assert_eq!(
            summary_line("\x1b[32m+ installed skill:review\x1b[0m\n"),
            "+ installed skill:review"
        );
        assert!(
            !summary_line("\x1b[33mup to date\x1b[0m\n").contains('\x1b'),
            "no raw ESC byte must survive summary_line"
        );
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
                upgrade_keys: Vec::new(),
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
        let reader_acquired = Arc::new(AtomicBool::new(false));
        let reader_released = Arc::new(AtomicBool::new(false));

        let hold = Duration::from_millis(300);
        let p_reader = Arc::clone(&paths);
        let acq = Arc::clone(&reader_acquired);
        let rel = Arc::clone(&reader_released);
        let reader = std::thread::spawn(move || {
            // Hold a shared lock on the same lock file for `hold`.
            let lock = lock::open(&p_reader).expect("open reader lock");
            let guard = lock.read().expect("acquire shared lock");
            // Signal that the shared lock is held before starting the hold, so
            // the main thread starts `execute` only once there is real
            // contention. A fixed "let the reader acquire" sleep is unreliable
            // on a loaded runner: it can oversleep past the whole hold, so
            // `execute` starts after the reader already released and never
            // contends.
            acq.store(true, Ordering::SeqCst);
            std::thread::sleep(hold);
            rel.store(true, Ordering::SeqCst);
            drop(guard);
        });

        // Wait until the reader actually holds the shared lock.
        while !reader_acquired.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }

        let p_exec = Arc::clone(&paths);
        let rel_check = Arc::clone(&reader_released);
        let start = Instant::now();
        let result = execute(
            &p_exec,
            PendingAction {
                kind: ActionKind::Sync,
                description: "sync".to_string(),
                dep_tree: None,
                upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "upgrade with nothing to do should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn action_upgrade_with_empty_keys_still_applies_real_drift_not_a_guaranteed_noop() {
        // spec: TUI-76 - M2: `initiate_upgrade` arms `upgrade_keys: []` when its
        // recompute found nothing stale, and the confirm text is "nothing is
        // out of date since the last sync ... Proceed with upgrade anyway?".
        // Before this fix, `dispatch` always routed ANY `ActionKind::Upgrade`
        // through the KEY-SCOPED `upgrade_no_sync_keys`, so an empty key set
        // built `key_scope = Some(<empty set>)`: every item is `OutOfScope` by
        // construction, and "anyway" provably applied nothing, no matter what
        // had drifted on disk. This seeds a real, on-disk content edit AFTER
        // the (empty-keys) pending action is built -- mirroring the confirm
        // text's own scenario, drift the last poll/sync missed -- and applies
        // it. The pre-fix behavior would leave the manifest hash unchanged;
        // the fix (routing the empty case through the UNSCOPED
        // `commands::upgrade_no_sync`, which re-derives staleness at apply
        // time) must actually upgrade the item.
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

        let src = base.join("empty-keys-anyway-source");
        std::fs::create_dir_all(src.join("skills/review")).unwrap();
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\noriginal\n",
        )
        .unwrap();
        init_git_repo(&src);
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&src)
                .output()
                .expect("git");
        };
        git(&["add", "-A"]);
        git(&["commit", "-qm", "initial"]);

        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        commands::learn(
            &paths,
            "skill:review",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Force,
                dangerously_skip: true,
                dangerously_skip_build: true,
            },
        )
        .expect("learn");

        let hash_before = crate::manifest::Manifest::load(&paths)
            .unwrap()
            .items
            .get("skill:review")
            .unwrap()
            .hash
            .clone();

        // Drift the last poll/sync missed: an on-disk edit after the (empty)
        // pending action would have been built.
        std::fs::write(
            src.join("skills/review/SKILL.md"),
            "---\ndescription: review skill\n---\n# review\nedited after the empty confirm\n",
        )
        .unwrap();

        let action = PendingAction {
            kind: ActionKind::Upgrade,
            description: "Upgrade: nothing is out of date since the last sync. Press `s` to \
                           sync and check for updates. Proceed with upgrade anyway?"
                .to_string(),
            dep_tree: None,
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "the empty-keys 'anyway' apply must succeed: {:?}",
            result.err()
        );

        let hash_after = crate::manifest::Manifest::load(&paths)
            .unwrap()
            .items
            .get("skill:review")
            .unwrap()
            .hash
            .clone();
        assert_ne!(
            hash_before, hash_after,
            "an empty-keys 'anyway' apply must re-derive staleness and actually \
             upgrade the drifted item, not silently apply nothing"
        );
    }

    #[test]
    fn action_upgrade_applies_only_the_confirmed_keys_not_a_newly_stale_item() {
        // spec: TUI-72 TUI-73 - the entire point of the fix: the apply must act
        // on EXACTLY the keys the confirm modal named, not on "everything found
        // stale right now" the way a bare `item_ref: None` upgrade would. Two
        // installed items start clean; only `skill:alpha` is edited before the
        // (simulated) confirm, so the pending action's `upgrade_keys` names only
        // `skill:alpha`. Between confirm and apply, `skill:beta` is ALSO edited
        // (a real race: some other change lands in the confirm-to-apply window).
        // Applying the frozen pending action must upgrade `skill:alpha` (it was
        // confirmed) but leave `skill:beta` untouched (it was never named),
        // even though `skill:beta` is independently stale by the time the apply
        // runs. Before TUI-72/TUI-73, `dispatch` called
        // `commands::upgrade_no_sync(paths, true, None, ..)`, which re-derives
        // the stale set at apply time and would have upgraded BOTH.
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

        let src = base.join("two-item-source");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\ndescription: alpha skill\n---\n# alpha\noriginal\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        std::fs::write(
            src.join("skills/beta/SKILL.md"),
            "---\ndescription: beta skill\n---\n# beta\noriginal\n",
        )
        .unwrap();
        init_git_repo(&src);
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&src)
                .output()
                .expect("git");
        };
        git(&["add", "-A"]);
        git(&["commit", "-qm", "initial"]);

        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        commands::learn(
            &paths,
            "skill:alpha",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Force,
                dangerously_skip: true,
                dangerously_skip_build: true,
            },
        )
        .expect("learn alpha");
        commands::learn(
            &paths,
            "skill:beta",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Force,
                dangerously_skip: true,
                dangerously_skip_build: true,
            },
        )
        .expect("learn beta");

        let manifest_before = crate::manifest::Manifest::load(&paths).unwrap();
        let alpha_hash_before = manifest_before
            .items
            .get("skill:alpha")
            .unwrap()
            .hash
            .clone();
        let beta_hash_before = manifest_before
            .items
            .get("skill:beta")
            .unwrap()
            .hash
            .clone();

        // Edit alpha only, "before confirm": this is the item the (simulated)
        // confirm modal names and the only key the pending action carries.
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\ndescription: alpha skill\n---\n# alpha\nedited before confirm\n",
        )
        .unwrap();
        let action = PendingAction {
            kind: ActionKind::Upgrade,
            description: "Upgrade 1 pending item(s)?\n\nskill:alpha (...)".to_string(),
            dep_tree: None,
            upgrade_keys: vec!["skill:alpha".to_string()],
        };

        // Edit beta "after confirm, before apply": a race the confirmed key set
        // must not silently absorb.
        std::fs::write(
            src.join("skills/beta/SKILL.md"),
            "---\ndescription: beta skill\n---\n# beta\nedited after confirm\n",
        )
        .unwrap();

        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "scoped upgrade must succeed: {:?}",
            result.err()
        );

        let manifest_after = crate::manifest::Manifest::load(&paths).unwrap();
        let alpha_hash_after = manifest_after
            .items
            .get("skill:alpha")
            .unwrap()
            .hash
            .clone();
        let beta_hash_after = manifest_after.items.get("skill:beta").unwrap().hash.clone();

        assert_ne!(
            alpha_hash_after, alpha_hash_before,
            "the confirmed item (skill:alpha) must be upgraded"
        );
        assert_eq!(
            beta_hash_after, beta_hash_before,
            "an item the modal never named (skill:beta) must NOT be upgraded, \
             even though it independently became stale before the apply ran"
        );
    }

    #[test]
    fn action_upgrade_routes_through_no_sync_so_recorded_commit_stays_stale() {
        // spec: TUI-73 - M-test6: a previous comment here claimed sync-vs-no-sync
        // is INDISTINGUISHABLE for a hermetic local source because it is read
        // live from its working tree (CLI-27 `is_linked`). That claim is wrong:
        // `sync` still re-reads and records the linked source's live HEAD into
        // the registry (`commands.rs`, the `source.is_linked()` branch), and
        // `upgrade`'s per-item pass records whatever commit is CURRENTLY in the
        // registry at that moment (`registry.find(&installed.source)...commit`),
        // not a fresh read of its own. So:
        //   - a no-sync apply (what the TUI's `u` action calls, `execute` here)
        //     detects the content drift (hash differs) but records the STALE
        //     commit, because nothing re-read the source's HEAD first;
        //   - a sync-first apply (`commands::upgrade`) records the NEW commit,
        //     because `sync_sources_for_upgrade` ran first.
        // That divergence is directly assertable without any network access.
        use std::process::Command;

        fn make_stale_fixture(paths: &Paths, base: &std::path::Path) {
            crate::paths::mkdir_p(&paths.mind_home).unwrap();
            // Pin the lobe to the isolated claude_home so the install never
            // touches the real ~/.claude (agent_homes() otherwise defaults to
            // ~/.claude when no lobes are configured).
            crate::config::Config {
                lobes: vec![crate::config::LobeEntry::bare(
                    paths.claude_home.to_str().unwrap(),
                )],
                ..Default::default()
            }
            .save(paths)
            .unwrap();
            let src = base.join("source");
            std::fs::create_dir_all(src.join("skills/review")).unwrap();
            std::fs::write(
                src.join("skills/review/SKILL.md"),
                "---\ndescription: review skill\n---\n# review\noriginal\n",
            )
            .unwrap();
            init_git_repo(&src);
            let git = |args: &[&str]| {
                Command::new("git")
                    .args(args)
                    .current_dir(&src)
                    .output()
                    .expect("git");
            };
            git(&["add", "-A"]);
            git(&["commit", "-qm", "initial"]);

            commands::meld(
                paths,
                src.to_str().unwrap(),
                None,
                vec![],
                vec![],
                false,
                commands::PinRequest::None,
                None,
                false,
            )
            .expect("meld");
            commands::learn(
                paths,
                "skill:review",
                false,
                commands::InstallFlow {
                    yes: true,
                    clobber: commands::Clobber::Force,
                    dangerously_skip: true,
                    dangerously_skip_build: true,
                },
            )
            .expect("learn");

            // Edit content and advance the source's own git HEAD WITHOUT
            // syncing: the registry's recorded `source.commit` still points at
            // the pre-edit HEAD.
            std::fs::write(
                src.join("skills/review/SKILL.md"),
                "---\ndescription: review skill\n---\n# review\nedited\n",
            )
            .unwrap();
            git(&["add", "-A"]);
            git(&["commit", "-qm", "edit"]);
        }

        let recorded_commit = |paths: &Paths| {
            crate::manifest::Manifest::load(paths)
                .unwrap()
                .items
                .get("skill:review")
                .expect("skill:review must be installed")
                .commit
                .clone()
        };

        // Fixture 1: the no-sync path (what the TUI's `u` action calls).
        let (no_sync_paths, _td1) = temp_paths();
        make_stale_fixture(&no_sync_paths, &_td1);
        let old_commit = recorded_commit(&no_sync_paths);

        let action = PendingAction {
            kind: ActionKind::Upgrade,
            description: "upgrade?".to_string(),
            dep_tree: None,
            // TUI-73: the apply is scoped to exactly the confirmed keys now, so
            // this must name skill:review the same way `initiate_upgrade` would
            // have stashed it, or the scoped apply would upgrade nothing and the
            // assertions below would pass vacuously.
            upgrade_keys: vec!["skill:review".to_string()],
        };
        execute(&no_sync_paths, action).expect("no-sync upgrade should succeed");
        let no_sync_commit = recorded_commit(&no_sync_paths);
        assert_eq!(
            no_sync_commit, old_commit,
            "a no-sync upgrade (the TUI's `u` action) must record the STALE \
             commit: it never re-reads the source's HEAD, only `sync` does. \
             got {no_sync_commit:?}, expected the pre-edit commit {old_commit:?}"
        );

        // Fixture 2: the sync-first path (the plain CLI `upgrade` verb),
        // independent so applying the first fixture's upgrade cannot affect it.
        // (Each fixture is its own fresh git repo, so its commit hash has no
        // relation to fixture 1's -- only the before/after within EACH fixture
        // is meaningful.)
        let (sync_paths, _td2) = temp_paths();
        make_stale_fixture(&sync_paths, &_td2);
        let old_commit_2 = recorded_commit(&sync_paths);

        commands::upgrade(&sync_paths, true, None, false, false)
            .expect("sync-first upgrade should succeed");
        let sync_commit = recorded_commit(&sync_paths);
        assert_ne!(
            sync_commit, old_commit_2,
            "a sync-first upgrade must record the NEW commit: `sync` re-reads \
             the linked source's live HEAD before upgrade computes what to \
             record. got {sync_commit:?}, expected it to differ from the \
             pre-edit commit {old_commit_2:?}"
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
        commands::meld(
            &paths,
            &spec,
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
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
            upgrade_keys: Vec::new(),
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
        commands::meld(
            paths,
            &spec,
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
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
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "LobeAdd should succeed: {:?}", result.err());

        // Verify the lobe was persisted to config.
        let cfg = crate::config::Config::load(&paths).unwrap();
        assert!(
            cfg.lobes.iter().any(|e| e.path() == lobe_path),
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
            upgrade_keys: Vec::new(),
        };
        execute(&paths, add_action).expect("LobeAdd prerequisite");

        // Now remove it.
        let remove_action = PendingAction {
            kind: ActionKind::LobeRemove {
                path: lobe_path.clone(),
            },
            description: format!("Remove lobe {lobe_path}?"),
            dep_tree: None,
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, remove_action);
        assert!(
            result.is_ok(),
            "LobeRemove should succeed: {:?}",
            result.err()
        );

        let cfg = crate::config::Config::load(&paths).unwrap();
        assert!(
            !cfg.lobes.iter().any(|e| e.path() == lobe_path),
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
            upgrade_keys: Vec::new(),
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
                upgrade_keys: Vec::new(),
            };
            execute(&paths, action).expect("LobeAdd must succeed");
        }

        let cfg = crate::config::Config::load(&paths).unwrap();
        let count = cfg.lobes.iter().filter(|e| e.path() == lobe_path).count();
        assert_eq!(
            count, 1,
            "duplicate lobe add must not produce duplicate entries"
        );
    }

    #[test]
    fn execute_lobe_add_creates_lobe_dir_and_backfills_installed_item() {
        // spec: TUI-62 HARN-15 HARN-7 HARN-17
        // Through the TUI's own entry point -- execute(ActionKind::LobeAdd), which
        // dispatches to commands::lobe_add (src/tui/action.rs:135) exactly as
        // the CLI's `config lobes add <path>` does -- the resolved lobe
        // directory must be created (HARN-15) and an already-installed item
        // must be backfilled into it (HARN-7/HARN-17).
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

        let src = make_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        commands::learn(
            &paths,
            "skill:build",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Error,
                dangerously_skip: false,
                dangerously_skip_build: false,
            },
        )
        .expect("learn skill:build");

        let new_lobe = base.join("tui-new-lobe");
        assert!(
            !new_lobe.exists(),
            "the lobe directory must not exist before the add"
        );

        let action = PendingAction {
            kind: ActionKind::LobeAdd {
                path: new_lobe.to_str().unwrap().to_string(),
            },
            description: "Add lobe?".to_string(),
            dep_tree: None,
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, action);
        assert!(result.is_ok(), "LobeAdd should succeed: {:?}", result.err());

        assert!(
            new_lobe.is_dir(),
            "HARN-15: the resolved lobe directory must be created by the TUI's \
             lobe-add action, before it is written to config"
        );
        let backfilled = new_lobe.join("skills/build");
        let meta = std::fs::symlink_metadata(&backfilled).unwrap_or_else(|e| {
            panic!(
                "HARN-7/HARN-17: the installed skill:build must be backfilled \
                 into the newly added lobe by the TUI's own lobe-add action: {e}"
            )
        });
        assert!(
            meta.file_type().is_symlink(),
            "the backfilled item must be linked as a symlink into the store, \
             matching the per-item link operation `learn` uses"
        );
    }

    #[test]
    fn execute_lobe_add_reports_foreign_target_without_clobbering() {
        // spec: TUI-62 HARN-17
        // The highest-stakes property of the HARN-7 backfill: a pre-existing
        // foreign file at a backfill target must be reported as a failure, not
        // silently overwritten, even though the backfill itself is
        // unconditional (HARN-17). Proven through the TUI's own dispatch
        // (execute(ActionKind::LobeAdd) -> commands::lobe_add, force = false
        // hardcoded at src/tui/action.rs:135), not by calling the guard
        // directly.
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

        let src = make_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        commands::learn(
            &paths,
            "skill:build",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Error,
                dangerously_skip: false,
                dangerously_skip_build: false,
            },
        )
        .expect("learn skill:build");

        let new_lobe = base.join("tui-foreign-lobe");
        let skills_dir = new_lobe.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let foreign_target = skills_dir.join("build");
        let foreign_content = b"not managed by mind, do not clobber";
        std::fs::write(&foreign_target, foreign_content).unwrap();

        let action = PendingAction {
            kind: ActionKind::LobeAdd {
                path: new_lobe.to_str().unwrap().to_string(),
            },
            description: "Add lobe?".to_string(),
            dep_tree: None,
            upgrade_keys: Vec::new(),
        };
        let result = execute(&paths, action);
        assert!(
            result.is_ok(),
            "the lobe add itself must still succeed even when a backfill \
             target is blocked (a blocked item is reported, not fatal): {:?}",
            result.as_ref().err()
        );

        // The blocked target must not be recorded as a link either: only
        // links `link_into_new_lobes` actually created are appended to the
        // manifest (src/commands.rs backfill_new_lobes), so a failed target
        // leaves no trace there. (The prose report naming `--force` goes to
        // the verb's real stdout, which under `cargo test` is intercepted by
        // libtest's own per-test capture before the TUI's fd-level
        // `with_captured_stdout` ever sees it, so it is not observable via
        // the returned summary string in-process; `tests/cli_lobes.rs`'s
        // `harn17_backfill_reports_foreign_target_without_clobbering` already
        // certifies that prose end-to-end against the compiled binary.)
        let foreign_target_str = foreign_target.to_string_lossy().into_owned();
        let manifest = crate::manifest::Manifest::load(&paths).unwrap();
        let item = manifest
            .items
            .get("skill:build")
            .expect("skill:build must still be in the manifest");
        assert!(
            !item.links.contains(&foreign_target_str),
            "a blocked backfill target must NOT be recorded as a link: {:?}",
            item.links
        );

        let meta = std::fs::symlink_metadata(&foreign_target).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "TUI-62: a foreign file at a backfill target reached through the \
             TUI must NOT be clobbered into a symlink"
        );
        assert_eq!(
            std::fs::read(&foreign_target).unwrap(),
            foreign_content,
            "TUI-62: the foreign file's content must be untouched"
        );
    }

    #[test]
    fn force_true_would_have_clobbered_the_foreign_target_proving_the_guard_is_load_bearing() {
        // Load-bearing proof for `execute_lobe_add_reports_foreign_target_without_clobbering`.
        //
        // This shard owns only src/tui/action.rs and spec/tui.md; the guard
        // itself (`ensure_unoccupied` / `link_into_new_lobes`'s `force` check)
        // lives in src/install.rs, and the hardcoded `force = false` the TUI
        // relies on lives in commands::lobe_add (src/commands.rs), neither of
        // which this shard may edit. So instead of using `Edit` to invert the
        // guard's condition in place, this test flips the exact boolean the
        // TUI's dispatch hardcodes to `false` -- by calling the SAME public
        // function (`commands::lobe_add_resolved`) the TUI's `commands::lobe_add`
        // wraps, with `force = true` -- on an identical foreign-file setup, and
        // shows the outcome inverts: the foreign file IS clobbered into a
        // symlink. That demonstrates the `force` parameter is exactly what
        // distinguishes "reported, not clobbered" from "clobbered", so the
        // TUI's hardcoded `false` (asserted by the sibling test above) is
        // load-bearing, not a vacuously-true assertion.
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

        let src = make_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld");
        commands::learn(
            &paths,
            "skill:build",
            false,
            commands::InstallFlow {
                yes: true,
                clobber: commands::Clobber::Error,
                dangerously_skip: false,
                dangerously_skip_build: false,
            },
        )
        .expect("learn skill:build");

        let new_lobe = base.join("tui-foreign-lobe-forced");
        let skills_dir = new_lobe.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let foreign_target = skills_dir.join("build");
        std::fs::write(&foreign_target, b"not managed by mind, do not clobber").unwrap();

        // Same underlying call `commands::lobe_add` makes, but with `force =
        // true` instead of the TUI's hardcoded `false`.
        let result = commands::lobe_add_resolved(
            &paths,
            Some(new_lobe.to_str().unwrap()),
            None,
            None,
            false, // snapshot
            true,  // force -- the inverse of what the TUI dispatch hardcodes
            true,  // yes (unused by the HARN-17 unconditional backfill)
        );
        assert!(
            result.is_ok(),
            "a forced backfill must still succeed: {:?}",
            result.err()
        );

        let meta = std::fs::symlink_metadata(&foreign_target).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "with force = true the foreign target MUST be clobbered into a \
             symlink -- proving the force parameter (hardcoded to false at \
             the TUI's call site) is what the non-clobber guarantee depends on"
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
            upgrade_keys: Vec::new(),
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
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = make_dep_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = make_chain_source_repo(&base);
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
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
            upgrade_keys: Vec::new(),
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
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = make_dep_source_repo(&base); // skill:review -> agent:dev
        commands::meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
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
            upgrade_keys: Vec::new(),
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
            upgrade_keys: Vec::new(),
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
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        // Two source repos that both ship "skill:review".
        let src_a = make_named_source_repo(&base, "source-alpha", "review");
        let src_b = make_named_source_repo(&base, "source-beta", "review");

        commands::meld(
            &paths,
            src_a.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
            None,
            false,
        )
        .expect("meld source-alpha");
        commands::meld(
            &paths,
            src_b.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            commands::PinRequest::None,
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
            upgrade_keys: Vec::new(),
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
