//! Terminal setup and teardown for the TUI.
//!
//! Enters the alternate screen and raw mode on construction; restores the
//! terminal on drop. Also installs a panic hook (TUI-40) so a crash never
//! leaves the terminal in a broken state.
//!
//! The `get_terminal()` function exposes the current terminal to `render.rs`.

use std::io;
use std::panic::PanicHookInfo;
use std::sync::{Arc, Mutex};

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::{MindError, Result};

/// A panic hook with the bounds `std::panic::set_hook` requires.
type PanicHook = Arc<dyn Fn(&PanicHookInfo) + Sync + Send>;

/// RAII guard: enters alt-screen + raw mode on creation and restores the
/// terminal on drop. Also installs a panic hook to restore on panic.
/// The end-to-end restore for TUI-40 requires a real terminal to observe, but
/// the poison-recovery path that makes restore reachable after a panic in
/// `render::draw` is verified by the unit test below (cited TUI-40).
// spec: TUI-41
pub struct TermGuard {
    /// The panic hook that was in effect before `enter()` installed ours.
    /// Restored on drop so the global hook is left as we found it (M6).
    /// Shared with the installed hook (which still calls it on a real panic);
    /// `None` only after `Drop` has taken it.
    prev_hook: Option<PanicHook>,
}

static TERMINAL: Mutex<Option<Terminal<CrosstermBackend<io::Stdout>>>> = Mutex::new(None);

impl TermGuard {
    /// Enter the alternate screen + raw mode, store the terminal in the global
    /// slot, and install a panic hook that runs `restore()`.
    pub fn enter() -> Result<Self> {
        // Install the panic hook BEFORE entering raw mode so it is in place if
        // anything in `enter` itself panics. Keep the previous hook so it can be
        // restored on drop (M6) and still be called on an actual panic.
        let prev_hook: Arc<dyn Fn(&PanicHookInfo) + Sync + Send> =
            Arc::from(std::panic::take_hook());
        let hook_for_panic = Arc::clone(&prev_hook);
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort restore: ignore any error here since we are panicking.
            let _ = restore();
            hook_for_panic(info);
        }));

        enable_raw_mode().map_err(|e| MindError::io("<terminal>", e))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(|e| MindError::io("<terminal>", e))?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend).map_err(|e| MindError::io("<terminal>", e))?;

        let mut slot = lock_terminal();
        *slot = Some(terminal);
        drop(slot);

        Ok(TermGuard { prev_hook: Some(prev_hook) })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        // Restore the terminal on normal exit and on error (TUI-40 and TUI-41);
        // the panic hook above handles the panic case.
        // spec: TUI-41
        let _ = restore();

        // Reinstall the panic hook that was in effect before `enter()`, so the
        // global hook is left exactly as we found it (M6). Done AFTER restore so
        // a panic mid-drop still benefits from our restoring hook. Wrapping the
        // shared prior hook in a fresh box keeps `set_hook`'s `Box<dyn Fn>`
        // signature satisfied.
        if let Some(prev) = self.prev_hook.take() {
            std::panic::set_hook(Box::new(move |info| prev(info)));
        }
    }
}

/// Lock the global terminal slot, recovering from a poisoned mutex.
///
/// `render::draw` holds this lock across `terminal.draw(|frame| ...)`. A panic
/// inside that closure poisons the mutex. Recovering (rather than unwrapping)
/// lets the panic hook and the RAII `Drop` still reach the terminal to restore
/// it instead of panicking a second time and leaving raw mode + alt-screen on
/// the terminal (TUI-40).
fn lock_terminal()
-> std::sync::MutexGuard<'static, Option<Terminal<CrosstermBackend<io::Stdout>>>> {
    TERMINAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Restore the terminal to its pre-TUI state. Idempotent: safe to call
/// multiple times (from both the normal exit path and the panic hook).
fn restore() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Take the terminal out of the global slot so we don't double-restore.
    let terminal = {
        let mut slot = lock_terminal();
        slot.take()
    };
    if let Some(mut t) = terminal {
        let _ = disable_raw_mode();
        let _ = execute!(t.backend_mut(), LeaveAlternateScreen);
        let _ = t.show_cursor();
    }
    Ok(())
}

/// Wrapper giving `DerefMut` access to the global terminal.
pub struct TerminalGuard(std::sync::MutexGuard<'static, Option<Terminal<CrosstermBackend<io::Stdout>>>>);

impl std::ops::Deref for TerminalGuard {
    type Target = Terminal<CrosstermBackend<io::Stdout>>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("terminal not initialized")
    }
}

impl std::ops::DerefMut for TerminalGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("terminal not initialized")
    }
}

/// Access the terminal for drawing. Called by render.rs.
pub fn get_terminal() -> TerminalGuard {
    TerminalGuard(lock_terminal())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serializes the tests below: both mutate the process-global panic hook (and
    /// one permanently poisons `TERMINAL`), so they must not run concurrently.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Poison the global `TERMINAL` mutex by panicking while holding its guard,
    /// then assert `restore()` recovers from the poison instead of panicking a
    /// second time. This is the path the panic hook hits after `render::draw`
    /// poisons the lock; without recovery the terminal would be stranded in raw
    /// mode + alt-screen.
    ///
    /// Note: poisoning is permanent for the process, but every accessor
    /// (`lock_terminal`, and through it `restore`/`get_terminal`) recovers, so a
    /// poisoned global does not break sibling tests in this binary.
    // spec: TUI-40
    #[test]
    fn restore_recovers_from_poisoned_terminal_mutex() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Poison the mutex: panic inside a thread while the guard is held.
        let handle = std::thread::spawn(|| {
            let _guard = TERMINAL.lock().unwrap();
            panic!("intentional poison");
        });
        assert!(handle.join().is_err(), "poisoning thread should have panicked");
        assert!(TERMINAL.is_poisoned(), "mutex should now be poisoned");

        // The fix: restore() must not panic and must return Ok despite poison.
        let result = std::panic::catch_unwind(restore);
        assert!(result.is_ok(), "restore() panicked on a poisoned mutex");
        assert!(
            matches!(result, Ok(Ok(()))),
            "restore() should return Ok even when the mutex is poisoned"
        );

        // lock_terminal() itself must also yield a usable guard under poison.
        let mut slot = lock_terminal();
        assert!(slot.is_none(), "restore() should have left the slot empty");
        *slot = None;
    }

    /// `TermGuard`'s drop reinstalls the panic hook that was in effect before the
    /// guard was created (M6). We exercise the guard's hook bookkeeping directly
    /// rather than through `enter()`, which needs a real TTY (`enable_raw_mode`).
    // spec: TUI-41
    #[test]
    fn drop_restores_previous_panic_hook() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        static SENTINEL_HITS: AtomicUsize = AtomicUsize::new(0);

        // Snapshot whatever hook the test harness has installed so we can put it
        // back at the end and not perturb other tests.
        let harness_hook = std::panic::take_hook();

        // Install a sentinel hook that records when it fires. This stands in for
        // "the hook that existed before enter()".
        std::panic::set_hook(Box::new(|_info| {
            SENTINEL_HITS.fetch_add(1, Ordering::SeqCst);
        }));

        // Simulate what enter() does to the hook chain: take the prior (sentinel)
        // hook, stash it in the guard, and install our restoring wrapper.
        let prev_hook: Arc<dyn Fn(&PanicHookInfo) + Sync + Send> =
            Arc::from(std::panic::take_hook());
        let hook_for_panic = Arc::clone(&prev_hook);
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore();
            hook_for_panic(info);
        }));
        let guard = TermGuard { prev_hook: Some(prev_hook) };

        // While our wrapper is installed, a panic still reaches the sentinel
        // (the wrapper calls the prior hook on a real panic).
        let before = SENTINEL_HITS.load(Ordering::SeqCst);
        let _ = std::panic::catch_unwind(|| panic!("through wrapper"));
        assert_eq!(
            SENTINEL_HITS.load(Ordering::SeqCst),
            before + 1,
            "installed hook must still call the previous hook on a panic"
        );

        // Dropping the guard must reinstall the sentinel hook directly.
        drop(guard);

        let before = SENTINEL_HITS.load(Ordering::SeqCst);
        let _ = std::panic::catch_unwind(|| panic!("after drop"));
        assert_eq!(
            SENTINEL_HITS.load(Ordering::SeqCst),
            before + 1,
            "after drop the previous (sentinel) hook should be back in effect"
        );

        // Restore the harness hook so this test leaves the global as it found it.
        std::panic::set_hook(harness_hook);
    }
}
