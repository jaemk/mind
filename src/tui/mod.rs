//! Interactive TUI for `mind probe`.
//!
//! Launched by `mind probe` when stdout is a TTY and no opt-out flag is given.
//! Falls back to the plain catalog listing for `--no-tui`, `--json`, or
//! non-TTY stdout (TUI-2). The TUI manages its own per-operation locks (TUI-25)
//! and never holds an outer lock while idle.
//!
//! Module decomposition:
//! - `app`     - pure App state model; no I/O
//! - `tree`    - build & flatten the Installed/Available tree
//! - `data`    - load/poll under shared lock
//! - `render`  - ratatui draw functions
//! - `event`   - crossterm input -> Intent mapping
//! - `action`  - execute a confirmed Intent under exclusive lock
//! - `preview` - shallow-clone preview + suggested-registry union
//! - `term`    - enter/leave alt-screen + raw mode; RAII restore + panic hook

pub mod app;
pub mod data;
pub mod event;
pub mod preview;
pub mod term;
pub mod tree;

// render and action are pub(crate) helpers
mod action;
mod render;

use crate::error::{ItemKind, Result};
use crate::paths::Paths;

/// Entry point: run the interactive TUI with optional seed state. Returns when
/// the user quits; the terminal is restored before returning.
///
/// `seed_query`, `seed_kind`, `seed_source` come from the CLI args and seed
/// the initial search/filter state (TUI-2). TUI-1 (interactive launch needs a
/// real TTY) is allowlisted; this function is only reachable when a TTY is
/// present (see `probe_launches_tui` in main.rs).
// spec: TUI-2
pub fn run(
    paths: &Paths,
    seed_query: Option<&str>,
    seed_kind: Option<ItemKind>,
    seed_source: Option<&str>,
) -> Result<()> {
    // Install terminal restore + panic hook before entering raw mode so a
    // crash or early return always leaves the terminal usable (TUI-40).
    let _guard = term::TermGuard::enter()?;

    // Build initial App state seeded with any CLI args.
    // spec: TUI-2
    let mut app = app::App::new(
        seed_query.unwrap_or("").to_string(),
        seed_kind,
        seed_source.map(|s| s.to_string()),
    );

    // Load initial data under a shared lock.
    let snapshot = data::load(paths)?;
    app.apply_snapshot(snapshot);

    // Main event loop.
    event_loop(paths, &mut app)?;

    Ok(())
}

fn event_loop(paths: &Paths, app: &mut app::App) -> Result<()> {
    // We run a synchronous loop using crossterm's poll+read API.
    // Poll with a ~1s timeout; on each tick, refresh state if appropriate.
    use crossterm::event::{self, Event as CEvent};
    use std::time::{Duration, Instant};

    let tick_rate = Duration::from_millis(1000);
    let mut last_tick = Instant::now();

    loop {
        // Poll for an event with at most the remainder of the tick interval.
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);

        if event::poll(timeout).unwrap_or(false)
            && let Ok(evt) = event::read()
        {
            match evt {
                CEvent::Key(k) => {
                    handle_key(paths, app, k);
                }
                CEvent::Resize(w, h) => {
                    app.set_size(w, h);
                }
                _ => {}
            }
        }

        // Tick: attempt a non-blocking refresh.
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
            // TUI-15: poll under a brief non-blocking shared lock; skip if
            // the app is holding a mutation lock.
            if !app.is_mutating()
                && let Some(snapshot) = data::try_poll(paths)
            {
                app.apply_snapshot_if_changed(snapshot);
            }
        }

        // Draw after every event/tick.
        render::draw(app)?;

        if app.should_quit() {
            break;
        }
    }

    Ok(())
}

/// Handle a single key event, routing to lobe-input, spec-input, or normal
/// mode as needed.
// spec: TUI-30 TUI-23
fn handle_key(paths: &Paths, app: &mut app::App, k: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;

    // --- Lobe-path input mode (TUI-23): user is typing a new lobe path. ---
    if app.lobe_input_active {
        match k.code {
            KeyCode::Enter => {
                // Submit the lobe-path input; this wires a LobeAdd pending action.
                // spec: TUI-23 CLI-112
                app.submit_lobe_add();
            }
            KeyCode::Esc => {
                app.apply_intent(crate::tui::event::Intent::CancelAction);
            }
            KeyCode::Backspace => {
                app.apply_intent(crate::tui::event::Intent::LobeInputBackspace);
            }
            KeyCode::Char(c) => {
                app.apply_intent(crate::tui::event::Intent::LobeInputChar(c));
            }
            _ => {}
        }
        return;
    }

    // --- Lobes modal (TUI-23): navigate lobes list, initiate add/remove. ---
    if app.lobes_modal_visible {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.apply_intent(crate::tui::event::Intent::CancelAction);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.apply_intent(crate::tui::event::Intent::LobeSelectUp);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.apply_intent(crate::tui::event::Intent::LobeSelectDown);
            }
            // 'a' adds a lobe (opens path-input box).
            KeyCode::Char('a') => {
                app.apply_intent(crate::tui::event::Intent::ActionLobeAdd);
            }
            // 'D' removes the selected lobe (shows confirm modal).
            KeyCode::Char('D') => {
                app.apply_intent(crate::tui::event::Intent::ActionLobeRemove);
            }
            _ => {}
        }
        return;
    }

    if app.spec_input_active {
        // When the spec-input box is open, all keys go to it (TUI-30).
        match k.code {
            KeyCode::Enter => {
                let spec = app.spec_input_text.trim().to_string();
                if spec.is_empty() {
                    // Empty input: cancel.
                    app.spec_input_active = false;
                    app.status = None;
                } else {
                    // Submit: run the preview (I/O here, not in App).
                    run_preview(paths, app, spec);
                }
            }
            KeyCode::Esc => {
                app.apply_intent(crate::tui::event::Intent::CancelAction);
            }
            KeyCode::Backspace => {
                app.apply_intent(crate::tui::event::Intent::SpecInputBackspace);
            }
            KeyCode::Char(c) => {
                app.apply_intent(crate::tui::event::Intent::SpecInputChar(c));
            }
            _ => {}
        }
        return;
    }

    // --- Search-focused mode (TUI-14): the search box owns the keyboard. ---
    // Once `/` focuses search, every printable key extends the query rather than
    // routing through `key_to_intent` (which would treat i/d/s/e/m/q as actions
    // or quit). This is what makes the search box actually usable.
    // spec: TUI-14
    if app.search_focused {
        match k.code {
            KeyCode::Enter | KeyCode::Tab => {
                // Submit: close/unfocus search but keep the filter in effect.
                app.apply_intent(crate::tui::event::Intent::SearchSubmit);
            }
            KeyCode::Esc => {
                // Esc clears the query and unfocuses search.
                app.apply_intent(crate::tui::event::Intent::SearchClear);
            }
            KeyCode::Backspace => {
                app.apply_intent(crate::tui::event::Intent::SearchBackspace);
            }
            KeyCode::Char(c) => {
                app.apply_intent(crate::tui::event::Intent::SearchChar(c));
            }
            _ => {}
        }
        return;
    }

    // Normal mode.
    // Esc while a confirm modal is up must cancel the action, not wipe the
    // search filter (key_to_intent maps Esc -> SearchClear unconditionally).
    // spec: TUI-24
    if app.modal_visible && k.code == KeyCode::Esc {
        app.apply_intent(crate::tui::event::Intent::CancelAction);
        return;
    }
    let intent = crate::tui::event::key_to_intent(k);
    match intent {
        crate::tui::event::Intent::Quit => {
            app.quit = true;
        }
        crate::tui::event::Intent::ConfirmAction => {
            if let Some(pending) = app.take_pending_action() {
                // Execute the action under the exclusive lock (TUI-25).
                // For Meld actions: the preview temp clone is discarded after the
                // action regardless of outcome (TUI-30):
                //   - On success, meld re-clones into the registry path; temp is redundant.
                //   - On failure, temp is cleaned up by dropping active_preview (no orphan).
                // spec: TUI-30 TUI-24 TUI-25
                let result = action::execute(paths, pending);
                // Drop active_preview unconditionally: on success meld owns its clone,
                // on failure clean up the temp dir (SourcePreview::drop removes it).
                app.active_preview = None;
                match result {
                    Ok(snapshot) => {
                        app.apply_snapshot(snapshot);
                        app.set_status("Done.".to_string());
                    }
                    Err(e) => {
                        app.set_error(format!("{e}"));
                    }
                }
            } else {
                app.confirm_selected();
            }
        }
        other => {
            app.apply_intent(other);
            // After intent: check if a preview was requested (TUI-31 suggestion expand).
            // spec: TUI-31
            if let Some(spec) = app.pending_preview_spec.take() {
                run_preview(paths, app, spec);
            }
        }
    }
}

/// Run `preview::preview()` for the given spec and update App state with the result.
///
/// On success: stores the SourcePreview in `app.active_preview` and opens the
/// Meld confirm modal (TUI-30).
/// On error: surfaces the error inline (TUI-24); any partial clone is cleaned
/// up by preview() itself before returning Err.
// spec: TUI-30
fn run_preview(paths: &Paths, app: &mut app::App, spec: String) {
    match preview::preview(paths, &spec) {
        Ok(prev) => {
            let name = prev.name.clone();
            let items_count = prev.items.len();
            app.active_preview = Some(prev);
            app.apply_intent(crate::tui::event::Intent::PreviewReady {
                spec,
                name: format!("{name} ({items_count} items)"),
            });
        }
        Err(e) => {
            app.apply_intent(crate::tui::event::Intent::PreviewError {
                message: format!("{e}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    //! These tests drive REAL crossterm KeyEvents through `handle_key`, exercising
    //! `key_to_intent` and the mode-routing the app.rs tests bypass (they call
    //! `apply_intent` directly). That routing is exactly where the search-focus
    //! and confirm-modal-Esc bugs lived.
    use super::*;
    use crate::error::ItemKind;
    use crate::tui::app::{ActionKind, App, PendingAction};
    use crate::tui::data::{Snapshot, SnapshotAvailable, SnapshotInstalled};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A throwaway `Paths` pointing at a temp dir. The key paths under test
    /// (search-focused input, confirm-modal cancel) route to `apply_intent` and
    /// never touch disk, so this is only here to satisfy `handle_key`'s signature.
    fn temp_paths() -> Paths {
        let base = std::env::temp_dir().join(format!("mind-tui-test-{}", std::process::id()));
        Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn seeded_app() -> App {
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(Snapshot {
            generation: 1,
            installed: vec![SnapshotInstalled {
                key: "skill:review".to_string(),
                name: "review".to_string(),
                source: "local/agents".to_string(),
                kind: ItemKind::Skill,
                commit: "abc12345".to_string(),
                description: Some("Review skill".to_string()),
            }],
            available: vec![SnapshotAvailable {
                key: "agent:dev".to_string(),
                name: "dev".to_string(),
                source: "local/agents".to_string(),
                kind: ItemKind::Agent,
                description: Some("Dev agent".to_string()),
                path: std::path::PathBuf::from("/fake/path"),
            }],
            source_names: vec!["local/agents".to_string()],
            suggestions: vec![],
            lobes: vec![],
        });
        app
    }

    #[test]
    fn search_focused_routes_action_letter_to_query() {
        // spec: TUI-14 - with search focused, an action letter like 'd' (which
        // key_to_intent maps to ActionForget) must extend the query instead of
        // triggering a forget. This is the bug: handle_key had no search branch.
        let paths = temp_paths();
        let mut app = seeded_app();
        // Focus search via the real `/` key.
        handle_key(&paths, &mut app, key(KeyCode::Char('/')));
        assert!(app.search_focused, "'/' must focus the search box");

        handle_key(&paths, &mut app, key(KeyCode::Char('d')));
        assert_eq!(
            app.search, "d",
            "'d' must be appended to the query while search-focused"
        );
        assert!(
            app.pending_action.is_none(),
            "'d' must NOT initiate a forget while searching"
        );
        assert!(
            !app.modal_visible,
            "no confirm modal should open from typing in search"
        );
    }

    #[test]
    fn search_focused_q_does_not_quit() {
        // spec: TUI-14 - 'q' (the quit key in normal mode) must type into the
        // query while search is focused, never quit the TUI mid-search.
        let paths = temp_paths();
        let mut app = seeded_app();
        handle_key(&paths, &mut app, key(KeyCode::Char('/')));
        handle_key(&paths, &mut app, key(KeyCode::Char('q')));
        assert!(
            !app.should_quit(),
            "'q' must not quit while search is focused"
        );
        assert_eq!(app.search, "q", "'q' must be typed into the query");
    }

    #[test]
    fn confirm_modal_esc_cancels_and_keeps_search_filter() {
        // spec: TUI-24 - with a confirm modal up, Esc must cancel the pending
        // action. It must NOT fall through to key_to_intent (which maps Esc to
        // SearchClear) and wipe the user's search filter as a side effect.
        let paths = temp_paths();
        let mut app = seeded_app();
        // Establish a search filter (not focused: a settled filter from /-then-Tab).
        app.search = "rev".to_string();
        // Stage a pending action + confirm modal.
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Sync,
            description: "Sync all?".to_string(),
        });
        app.modal_visible = true;

        handle_key(&paths, &mut app, key(KeyCode::Esc));

        assert!(
            app.pending_action.is_none(),
            "Esc must cancel the pending action"
        );
        assert!(!app.modal_visible, "Esc must dismiss the confirm modal");
        assert_eq!(
            app.search, "rev",
            "Esc-to-cancel must leave the search filter intact"
        );
    }
}
