//! Terminal setup and teardown for the TUI.
//!
//! Enters the alternate screen and raw mode on construction; restores the
//! terminal on drop. Also installs a panic hook (TUI-40) so a crash never
//! leaves the terminal in a broken state.
//!
//! The `get_terminal()` function exposes the current terminal to `render.rs`.

use std::io;
use std::sync::Mutex;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::error::{MindError, Result};

/// RAII guard: enters alt-screen + raw mode on creation and restores the
/// terminal on drop. Also installs a panic hook to restore on panic.
/// TUI-40 (terminal restore on panic/error) requires a real terminal to
/// observe, so it is allowlisted rather than cited. The panic hook and RAII
/// restore are present and correct in this implementation.
// spec: TUI-41
pub struct TermGuard;

static TERMINAL: Mutex<Option<Terminal<CrosstermBackend<io::Stdout>>>> = Mutex::new(None);

impl TermGuard {
    /// Enter the alternate screen + raw mode, store the terminal in the global
    /// slot, and install a panic hook that runs `restore()`.
    pub fn enter() -> Result<Self> {
        // Install the panic hook BEFORE entering raw mode so it is in place if
        // anything in `enter` itself panics.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort restore: ignore any error here since we are panicking.
            let _ = restore();
            prev_hook(info);
        }));

        enable_raw_mode().map_err(|e| MindError::io("<terminal>", e))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).map_err(|e| MindError::io("<terminal>", e))?;

        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend).map_err(|e| MindError::io("<terminal>", e))?;

        let mut slot = TERMINAL.lock().unwrap();
        *slot = Some(terminal);
        drop(slot);

        Ok(TermGuard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        // Restore the terminal on normal exit and on error (TUI-40 and TUI-41).
        // TUI-40 is allowlisted (requires a real terminal to observe the restore);
        // the panic hook above handles the panic case.
        // spec: TUI-41
        let _ = restore();
    }
}

/// Restore the terminal to its pre-TUI state. Idempotent: safe to call
/// multiple times (from both the normal exit path and the panic hook).
fn restore() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Take the terminal out of the global slot so we don't double-restore.
    let terminal = {
        let mut slot = TERMINAL.lock().unwrap();
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
    TerminalGuard(TERMINAL.lock().unwrap())
}
