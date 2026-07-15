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
    use crossterm::event::{KeyCode, KeyModifiers};

    // --- Force exit (TUI-43): Ctrl-C is intercepted before any mode routing, so
    // it works even while typing in the search/spec/lobe input boxes (where a
    // bare `Char('c')` would otherwise be entered as text). One Ctrl-C arms; a
    // second consecutive Ctrl-C force-exits. Any other key disarms.
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        if app.ctrl_c_armed {
            app.quit = true;
        } else {
            app.ctrl_c_armed = true;
            app.set_status("Press Ctrl-C again to force exit.".to_string());
        }
        return;
    }
    app.ctrl_c_armed = false;

    // --- Namespace-input mode (TUI-53): user is typing a namespace prefix. ---
    // Opened via "Set namespace" in the source details dialog when no items are
    // installed. Intercept all keys so none route through normal intent dispatch.
    // spec: TUI-53
    if app.namespace_input_active {
        match k.code {
            KeyCode::Enter => {
                let source = app.namespace_input_source.take().unwrap_or_default();
                let text = app.namespace_input_text.trim().to_string();
                app.namespace_input_active = false;
                app.namespace_input_text.clear();
                // Empty input means "no prefix" (clear consumer alias, NS-30).
                let new_alias = if text.is_empty() { None } else { Some(text) };
                run_set_namespace(paths, app, source, new_alias);
            }
            KeyCode::Esc => {
                app.apply_intent(crate::tui::event::Intent::CancelAction);
            }
            KeyCode::Backspace => {
                app.apply_intent(crate::tui::event::Intent::NamespaceInputBackspace);
            }
            KeyCode::Char(c) => {
                app.apply_intent(crate::tui::event::Intent::NamespaceInputChar(c));
            }
            _ => {}
        }
        return;
    }

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
                app.spec_input_active = false;
                app.spec_input_text.clear();
                if spec.is_empty() {
                    // Empty input: cancel.
                    app.status = None;
                } else {
                    // Submit: the user named the source, so meld it directly with
                    // the same interactive flow as the CLI (suspend the TUI). This
                    // also lets an auth-required (SSH) remote prompt for a key on
                    // the normal terminal instead of hanging the UI (TUI-30, TUI-45).
                    run_interactive_meld(paths, app, spec);
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

    // --- Details dialog (TUI-26): navigate the action list, run or dismiss. ---
    // The dialog owns the keyboard while open, ahead of the confirm modal (which
    // it opens on activation). Esc/q/n dismiss; Up/Down move; Enter/y run.
    // spec: TUI-26
    if app.dialog.is_some() {
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => app.close_dialog(),
            KeyCode::Up | KeyCode::Char('k') => app.dialog_up(),
            KeyCode::Down | KeyCode::Char('j') => app.dialog_down(),
            KeyCode::Enter | KeyCode::Char('y') => {
                app.activate_dialog();
                // An Install action arms a dependency-closure preview (DEP-40),
                // computed here (I/O) before the confirm modal is drawn, exactly
                // as the direct `i` action does in normal mode.
                if let Some(item_ref) = app.pending_learn_ref.take() {
                    run_learn_preview(paths, app, &item_ref);
                }
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
                // spec: TUI-30 TUI-24 TUI-25 TUI-44
                let result = if action_needs_suspension(&pending.kind) {
                    // Meld and unmeld may prompt interactively (hook HOOK-20,
                    // install confirm CLI-23, uninstall hook HOOK-54). Suspend the
                    // TUI and run on the normal terminal so its flow is identical to
                    // the CLI, then let the user read the output before the browser
                    // redraws over it (TUI-44).
                    term::with_suspended(|| {
                        let r = action::execute_interactive(paths, pending);
                        pause_for_return();
                        r
                    })
                } else {
                    action::execute(paths, pending)
                };
                // Drop active_preview unconditionally: on success meld owns its clone,
                // on failure clean up the temp dir (SourcePreview::drop removes it).
                app.active_preview = None;
                match result {
                    Ok((snapshot, msg)) => {
                        app.apply_snapshot(snapshot);
                        // Show the verb's own summary line (captured, never printed
                        // to the alt-screen); fall back to a generic note.
                        app.set_status(if msg.is_empty() {
                            "Done.".to_string()
                        } else {
                            msg
                        });
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
            // After intent: if a Learn was initiated, compute its dependency tree
            // (I/O: reads files + manifest) and stash it onto the pending action so
            // the confirm modal shows what the closure will pull in (DEP-40).
            // spec: DEP-40
            if let Some(item_ref) = app.pending_learn_ref.take() {
                run_learn_preview(paths, app, &item_ref);
            }
        }
    }
}

/// Return true if this action kind requires the TUI to suspend (leave raw mode
/// and the alt-screen) before executing, because the verb may prompt
/// interactively on stdin/stdout.
///
/// Currently: Meld (install-hook disclosure HOOK-20, install confirm CLI-23,
/// SSH passphrase TUI-45) and Unmeld (uninstall-hook prompt HOOK-54). All other
/// actions run with stdout captured while the TUI holds the terminal (TUI-24).
// spec: TUI-44
pub(crate) fn action_needs_suspension(kind: &crate::tui::app::ActionKind) -> bool {
    use crate::tui::app::ActionKind;
    matches!(kind, ActionKind::Meld { .. } | ActionKind::Unmeld { .. })
}

/// Persist a namespace alias change for a source (TUI-53, NS-30).
///
/// Calls `commands::set_source_namespace` under the I/O layer (which holds the
/// lock for the write). On success refreshes the snapshot so the dialog's
/// namespace field reflects the change immediately. On failure surfaces the
/// error inline (TUI-24): a `NamespaceLocked` error means items are installed.
// spec: TUI-53 NS-30
fn run_set_namespace(paths: &Paths, app: &mut app::App, source: String, new_alias: Option<String>) {
    match crate::commands::set_source_namespace(paths, &source, new_alias) {
        Ok(()) => {
            app.set_status("Namespace updated.".to_string());
            // Reload so the tree + dialog show the updated effective prefix.
            if let Some(snapshot) = data::try_poll(paths) {
                app.apply_snapshot(snapshot);
            }
        }
        Err(e) => {
            app.set_error(format!("{e}"));
        }
    }
}

/// After an interactive verb runs with the TUI suspended (TUI-44), hold the
/// normal terminal so the user can read the verb's output before the browser
/// redraws over it. A bare Enter (or EOF) returns. Best-effort: any I/O error
/// just returns immediately.
fn pause_for_return() {
    use std::io::Write;
    print!("\n[press Enter to return to the browser] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

/// Meld a hand-entered repo spec with the full interactive CLI flow: suspend the
/// TUI (TUI-44) so the clone, the install-hook prompt, and the install
/// confirmation all run on the normal terminal -- and so an auth-required (SSH)
/// remote can prompt for a passphrase or host-key instead of hanging the UI
/// (TUI-45). On success the snapshot is reloaded; an error is surfaced inline.
// spec: TUI-30 TUI-44 TUI-45
fn run_interactive_meld(paths: &Paths, app: &mut app::App, spec: String) {
    let action = app::PendingAction::new(app::ActionKind::Meld { spec }, String::new());
    let result = term::with_suspended(|| {
        let r = action::execute_interactive(paths, action);
        pause_for_return();
        r
    });
    // Any preview clone is now redundant (a real meld owns its clone) or unwanted.
    app.active_preview = None;
    match result {
        Ok((snapshot, msg)) => {
            app.apply_snapshot(snapshot);
            app.set_status(if msg.is_empty() {
                "Done.".to_string()
            } else {
                msg
            });
        }
        Err(e) => {
            app.set_error(format!("{e}"));
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

/// Compute the dependency tree for a Learn selection and stash it onto the
/// pending action so the confirm modal can render it (DEP-40). The actual file
/// and manifest reads live here in the I/O layer, not in the pure App model.
///
/// The tree is only attached when the closure adds dependencies beyond the
/// explicit selection; when it adds nothing, the confirm stays as before (just
/// the description). A `learn_preview` error is surfaced inline (TUI-24) and the
/// pending action is cleared so we never confirm an install we could not plan.
// spec: DEP-40
fn run_learn_preview(paths: &Paths, app: &mut app::App, item_ref: &str) {
    match crate::commands::learn_preview(paths, item_ref) {
        Ok(plan) => {
            if plan.adds_dependencies {
                app.set_learn_dep_tree(Some(plan.tree));
            } else {
                app.set_learn_dep_tree(None);
            }
        }
        Err(e) => {
            app.pending_action = None;
            app.modal_visible = false;
            app.set_error(format!("{e}"));
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
                deps: vec![],
            }],
            available: vec![SnapshotAvailable {
                key: "agent:dev".to_string(),
                name: "dev".to_string(),
                source: "local/agents".to_string(),
                kind: ItemKind::Agent,
                description: Some("Dev agent".to_string()),
                path: std::path::PathBuf::from("/fake/path"),
                deps: vec![],
            }],
            unmanaged: vec![],
            source_names: vec!["local/agents".to_string()],
            suggestions: vec![],
            lobes: vec![],
            source_namespaces: std::collections::HashMap::new(),
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
        app.pending_action = Some(PendingAction::new(
            ActionKind::Sync,
            "Sync all?".to_string(),
        ));
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

    #[test]
    fn double_ctrl_c_force_exits_from_input_mode() {
        // spec: TUI-43 - Ctrl-C must force-exit even from a text-input mode, where
        // a bare key is typed into the box. First Ctrl-C arms (no quit, not typed);
        // a second consecutive Ctrl-C exits.
        let paths = temp_paths();
        let mut app = seeded_app();
        app.spec_input_active = true; // typing a repo spec: keys go into the box
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        handle_key(&paths, &mut app, ctrl_c);
        assert!(!app.should_quit(), "one Ctrl-C must not exit");
        assert!(app.ctrl_c_armed, "one Ctrl-C arms the force-exit");
        assert_eq!(
            app.spec_input_text, "",
            "Ctrl-C must not be typed into the input box"
        );

        handle_key(&paths, &mut app, ctrl_c);
        assert!(
            app.should_quit(),
            "a second consecutive Ctrl-C must force exit"
        );
    }

    #[test]
    fn a_key_between_ctrl_c_disarms_force_exit() {
        // spec: TUI-43 - the two Ctrl-C must be consecutive; any other key resets
        // the arm so a single stray Ctrl-C never quits.
        let paths = temp_paths();
        let mut app = seeded_app();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        handle_key(&paths, &mut app, ctrl_c);
        assert!(app.ctrl_c_armed);
        handle_key(&paths, &mut app, key(KeyCode::Char('j'))); // navigate: disarms
        assert!(!app.ctrl_c_armed, "another key must disarm the force-exit");

        handle_key(&paths, &mut app, ctrl_c);
        assert!(
            !app.should_quit(),
            "a lone Ctrl-C after a reset must not exit"
        );
    }

    /// A self-cleaning temp base dir (removes itself on drop, even on panic).
    struct OwnedTemp(std::path::PathBuf);
    impl Drop for OwnedTemp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn run_learn_preview_lands_tree_in_pending_action() {
        // spec: DEP-40 - the I/O seam: given a real melded source whose skill
        // references an agent via {{ns:}}, run_learn_preview (called from the event
        // loop) must compute the dependency tree and stash it onto the pending
        // Learn action so the confirm modal can show it. This pins that the tree
        // actually reaches App state (not just that learn_preview exists).
        use std::process::Command;

        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("mind-tui-mod-dep-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _owned = OwnedTemp(base.clone());
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        // Pin the lobe to the isolated claude_home (hermeticity).
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        // Build a source: skill `review` references agent `dev` via {{ns:dev}}.
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
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld");

        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        // Stage the Learn pending action as initiate_learn would.
        let mut app = app::App::new(String::new(), None, None);
        app.pending_action = Some(app::PendingAction::new(
            app::ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: source_name.clone(),
            },
            "Install skill:review?".to_string(),
        ));
        let item_ref = app::learn_ref("skill:review", &source_name);

        run_learn_preview(&paths, &mut app, &item_ref);

        let tree = app
            .pending_action
            .as_ref()
            .expect("pending action survives a successful preview")
            .dep_tree
            .clone()
            .expect("the dependency tree must be stashed onto the Learn confirm");
        assert!(
            tree.contains("review"),
            "tree must mention the selected skill: {tree}"
        );
        assert!(
            tree.contains("dev"),
            "tree must mention the pulled-in dependency agent: {tree}"
        );
    }

    #[test]
    fn run_learn_preview_no_deps_leaves_tree_none() {
        // spec: DEP-40 - when the selected item references NOTHING, its closure
        // adds no dependencies, so run_learn_preview must leave the pending Learn
        // confirm WITHOUT a tree (dep_tree stays None). A regression that always
        // attached a tree (or rendered a single-node "tree") would fail here, and
        // the confirm modal would then show a stray closure for a plain install.
        use std::process::Command;

        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("mind-tui-mod-nodep-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _owned = OwnedTemp(base.clone());
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        // Source with a single self-contained skill: no `{{ns:}}` tokens at all.
        let src = base.join("plain-source");
        std::fs::create_dir_all(src.join("skills/solo")).unwrap();
        std::fs::write(
            src.join("skills/solo/SKILL.md"),
            "---\ndescription: solo skill\n---\n# solo\nNo references here.\n",
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
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld");

        let source_name = crate::source::Registry::load(&paths).unwrap().sources[0]
            .name
            .clone();

        let mut app = app::App::new(String::new(), None, None);
        app.pending_action = Some(app::PendingAction::new(
            app::ActionKind::Learn {
                item_key: "skill:solo".to_string(),
                source: source_name.clone(),
            },
            "Install skill:solo?".to_string(),
        ));
        // Seed a non-None tree to prove run_learn_preview actively clears it on the
        // no-deps path (set_learn_dep_tree(None)), rather than just leaving it alone.
        app.set_learn_dep_tree(Some("stale tree".to_string()));

        let item_ref = app::learn_ref("skill:solo", &source_name);
        run_learn_preview(&paths, &mut app, &item_ref);

        let pending = app
            .pending_action
            .as_ref()
            .expect("a no-deps preview must keep the pending Learn action");
        assert!(
            pending.dep_tree.is_none(),
            "a selection that references nothing must carry NO dependency tree, got: {:?}",
            pending.dep_tree
        );
        assert!(
            app.error.is_none(),
            "a successful no-deps preview sets no error"
        );
    }

    #[test]
    fn run_learn_preview_error_clears_pending_and_surfaces_error() {
        // spec: DEP-40 - an unresolvable learn ref (no such item) must NOT silently
        // attach a tree. run_learn_preview surfaces the error inline (TUI-24) and
        // clears the pending action + hides the modal, so the user never confirms an
        // install that could not even be planned. A regression that swallowed the
        // error and left the pending action would fail both assertions.
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("mind-tui-mod-err-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _owned = OwnedTemp(base.clone());
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
        // No source melded: any learn ref is unresolvable.

        let mut app = app::App::new(String::new(), None, None);
        app.pending_action = Some(app::PendingAction::new(
            app::ActionKind::Learn {
                item_key: "skill:ghost".to_string(),
                source: "no/such-source".to_string(),
            },
            "Install skill:ghost?".to_string(),
        ));
        app.modal_visible = true;

        run_learn_preview(&paths, &mut app, "no/such-source#skill:ghost");

        assert!(
            app.pending_action.is_none(),
            "a preview error must clear the pending action (no stale confirm to apply)"
        );
        assert!(
            !app.modal_visible,
            "a preview error must hide the confirm modal"
        );
        assert!(
            app.error.is_some(),
            "a preview error must be surfaced inline, got error: {:?}",
            app.error
        );
    }

    // --- TUI-44: action_needs_suspension routing ---

    #[test]
    fn meld_needs_suspension() {
        // spec: TUI-44 - Meld may prompt (install-hook HOOK-20, install confirm
        // CLI-23, SSH passphrase TUI-45) and must run on the suspended terminal.
        assert!(
            action_needs_suspension(&ActionKind::Meld {
                spec: "git@github.com:foo/bar".to_string()
            }),
            "Meld must be classified as requiring suspension"
        );
    }

    #[test]
    fn unmeld_needs_suspension() {
        // spec: TUI-44 - Unmeld can trigger an uninstall-hook prompt (HOOK-54);
        // it must run on the suspended terminal, not with stdout captured.
        assert!(
            action_needs_suspension(&ActionKind::Unmeld {
                name: "foo/bar".to_string(),
                forget: true,
            }),
            "Unmeld with forget=true must be classified as requiring suspension"
        );
        assert!(
            action_needs_suspension(&ActionKind::Unmeld {
                name: "foo/bar".to_string(),
                forget: false,
            }),
            "Unmeld with forget=false must also be classified as requiring suspension"
        );
    }

    /// Meld a throwaway local git source under `base` and return its registry name.
    /// `commands::meld` only registers/clones (install is a separate step), so the
    /// manifest is empty afterward unless the caller seeds it.
    fn meld_plain_source(paths: &Paths, base: &std::path::Path) -> String {
        use std::process::Command;
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(paths)
        .unwrap();
        let src = base.join("ns-source");
        std::fs::create_dir_all(src.join("skills/solo")).unwrap();
        std::fs::write(
            src.join("skills/solo/SKILL.md"),
            "---\ndescription: solo skill\n---\n# solo\nNo references here.\n",
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
            paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("meld");
        crate::source::Registry::load(paths).unwrap().sources[0]
            .name
            .clone()
    }

    #[test]
    fn run_set_namespace_persists_alias_when_no_items_installed() {
        // spec: TUI-53 NS-30 - the live TUI persist seam writes the new alias to
        // sources.json (via commands::set_source_namespace), reports success, and
        // surfaces no error when no items from the source are installed.
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("mind-tui-ns-ok-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _owned = OwnedTemp(base.clone());
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let source_name = meld_plain_source(&paths, &base);

        let mut app = app::App::new(String::new(), None, None);
        run_set_namespace(
            &paths,
            &mut app,
            source_name.clone(),
            Some("jk".to_string()),
        );

        // The alias is persisted to sources.json.
        let reg = crate::source::Registry::load(&paths).unwrap();
        let src = reg.find(&source_name).expect("source present");
        assert_eq!(
            src.alias.as_deref(),
            Some("jk"),
            "run_set_namespace must persist the new alias"
        );
        assert!(
            app.error.is_none(),
            "success must set no error: {:?}",
            app.error
        );
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("Namespace")),
            "success must set a status: {:?}",
            app.status
        );
    }

    #[test]
    fn run_set_namespace_surfaces_lock_error_when_items_installed() {
        // spec: TUI-53 NS-30 - when items are installed the seam surfaces the
        // NamespaceLocked error inline (TUI-24) and does NOT change sources.json.
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("mind-tui-ns-lock-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _owned = OwnedTemp(base.clone());
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let source_name = meld_plain_source(&paths, &base);

        // Seed an installed item attributed to the source so the lock engages.
        let mut manifest = crate::manifest::Manifest::load(&paths).unwrap();
        manifest.insert(crate::manifest::InstalledItem {
            kind: ItemKind::Skill,
            name: "solo".to_string(),
            bare_name: "solo".to_string(),
            source: source_name.clone(),
            commit: "deadbeef".to_string(),
            hash: "abc123".to_string(),
            store: "store/solo".to_string(),
            links: vec![],
            description: None,
        });
        manifest.save(&paths).unwrap();

        let mut app = app::App::new(String::new(), None, None);
        run_set_namespace(
            &paths,
            &mut app,
            source_name.clone(),
            Some("jk".to_string()),
        );

        // The error is surfaced inline.
        let err = app
            .error
            .as_deref()
            .expect("a locked change must set an error");
        assert!(
            err.contains("skill:solo") && err.contains("forget"),
            "lock error must name the item and direct to forget: {err}"
        );
        // The alias must NOT have been written.
        let reg = crate::source::Registry::load(&paths).unwrap();
        let src = reg.find(&source_name).expect("source present");
        assert_eq!(
            src.alias, None,
            "a refused change must leave the alias unchanged"
        );
    }

    #[test]
    fn non_suspending_actions_do_not_need_suspension() {
        // spec: TUI-44 - learn/forget/sync/upgrade run captured (no suspension).
        // A regression that added them to the suspension list would break this.
        for kind in [
            ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: "src".to_string(),
            },
            ActionKind::Forget {
                item_key: "skill:review".to_string(),
            },
            ActionKind::Sync,
            ActionKind::Upgrade,
            ActionKind::LobeAdd {
                path: "/some/path".to_string(),
            },
            ActionKind::LobeRemove {
                path: "/some/path".to_string(),
            },
        ] {
            assert!(
                !action_needs_suspension(&kind),
                "{kind:?} must NOT require suspension (runs captured)"
            );
        }
    }
}
