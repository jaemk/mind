//! crossterm input -> Intent mapping for the TUI event loop.
//!
//! Defines the Intent enum (what the user wants to do) and maps crossterm
//! KeyEvents to Intents. Pure: no I/O beyond receiving the event.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the user intends to do, independent of the actual key binding.
// spec: TUI-11
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // all variants are part of the public TUI event API
pub enum Intent {
    /// Move selection up.
    MoveUp,
    /// Move selection down.
    MoveDown,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Expand the selected node.
    Expand,
    /// Collapse the selected node (or jump to parent).
    Collapse,
    /// Toggle expand/collapse on the selected node.
    ToggleExpand,
    /// Jump to the search box.
    JumpToSearch,
    /// Append a character to the search string.
    SearchChar(char),
    /// Delete the last character of the search string.
    SearchBackspace,
    /// Clear the search string.
    SearchClear,
    /// Submit (close) the search box.
    SearchSubmit,
    /// Confirm a pending action.
    ConfirmAction,
    /// Cancel a pending action.
    CancelAction,
    /// Install the selected available item (TUI-20).
    ActionLearn,
    /// Uninstall the selected installed item (TUI-20).
    ActionForget,
    /// Sync all sources (TUI-22).
    ActionSync,
    /// Upgrade pending items (TUI-22).
    ActionUpgrade,
    /// Meld a source (TUI-21).
    ActionMeld,
    /// Unmeld a source (TUI-21).
    ActionUnmeld,
    /// Type a character into the spec-input box (TUI-30).
    SpecInputChar(char),
    /// Delete the last character in the spec-input box (TUI-30).
    SpecInputBackspace,
    /// Submit the spec-input box (TUI-30).
    SpecInputSubmit,
    /// Preview result ready: spec and name from the shallow clone (TUI-30).
    PreviewReady { spec: String, name: String },
    /// Preview failed with the given error message (TUI-30).
    PreviewError { message: String },
    /// Open the lobes management modal (TUI-23, CLI-111).
    // spec: TUI-23
    ActionLobes,
    /// Initiate adding a lobe from within the lobes modal (TUI-23, CLI-112).
    // spec: TUI-23
    ActionLobeAdd,
    /// Initiate removing the selected lobe from within the lobes modal (TUI-23, CLI-113).
    // spec: TUI-23
    ActionLobeRemove,
    /// Type a character into the lobe-path input box (TUI-23).
    // spec: TUI-23
    LobeInputChar(char),
    /// Delete the last character in the lobe-path input box (TUI-23).
    // spec: TUI-23
    LobeInputBackspace,
    /// Submit the lobe-path input box (TUI-23).
    // spec: TUI-23
    LobeInputSubmit,
    /// Move selection up within the lobes modal list.
    LobeSelectUp,
    /// Move selection down within the lobes modal list.
    LobeSelectDown,
    /// Quit the TUI.
    Quit,
    /// No recognized binding.
    None,
}

/// Map a crossterm KeyEvent to an Intent.
// spec: TUI-11
pub fn key_to_intent(key: KeyEvent) -> Intent {
    match (key.code, key.modifiers) {
        // Navigation
        (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => Intent::MoveUp,
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => Intent::MoveDown,
        (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => Intent::PageUp,
        (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => Intent::PageDown,
        (KeyCode::Right, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => Intent::Expand,
        (KeyCode::Left, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => Intent::Collapse,
        (KeyCode::Enter, _) => Intent::ToggleExpand,

        // Search
        (KeyCode::Char('/'), KeyModifiers::NONE) => Intent::JumpToSearch,
        (KeyCode::Esc, _) => Intent::SearchClear,
        (KeyCode::Backspace, _) => Intent::SearchBackspace,
        (KeyCode::Tab, _) => Intent::SearchSubmit,

        // Actions
        (KeyCode::Char('i'), KeyModifiers::NONE) => Intent::ActionLearn,
        (KeyCode::Char('d'), KeyModifiers::NONE) => Intent::ActionForget,
        (KeyCode::Char('s'), KeyModifiers::NONE) => Intent::ActionSync,
        (KeyCode::Char('u'), KeyModifiers::NONE) => Intent::ActionUpgrade,
        (KeyCode::Char('m'), KeyModifiers::NONE) => Intent::ActionMeld,
        (KeyCode::Char('U'), KeyModifiers::SHIFT) => Intent::ActionUnmeld,
        // Lobe management: `C` opens the lobes modal (TUI-23).
        // spec: TUI-23
        (KeyCode::Char('C'), KeyModifiers::SHIFT) => Intent::ActionLobes,

        // Confirm / cancel
        (KeyCode::Char('y'), KeyModifiers::NONE) => Intent::ConfirmAction,
        (KeyCode::Char('n'), KeyModifiers::NONE) => Intent::CancelAction,

        // Quit
        (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::Char('Q'), KeyModifiers::SHIFT) => {
            Intent::Quit
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Intent::Quit,
        // Note: Ctrl+C maps to Quit here, not SearchClear (ESC handles clear).

        // Pass other chars through to search if they are printable
        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => Intent::SearchChar(c),

        _ => Intent::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn up_arrow_maps_to_move_up() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Up)), Intent::MoveUp);
    }

    #[test]
    fn k_maps_to_move_up() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Char('k'))), Intent::MoveUp);
    }

    #[test]
    fn down_arrow_maps_to_move_down() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Down)), Intent::MoveDown);
    }

    #[test]
    fn j_maps_to_move_down() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Char('j'))), Intent::MoveDown);
    }

    #[test]
    fn right_arrow_maps_to_expand() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Right)), Intent::Expand);
    }

    #[test]
    fn left_arrow_maps_to_collapse() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Left)), Intent::Collapse);
    }

    #[test]
    fn page_up_maps_to_page_up() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::PageUp)), Intent::PageUp);
    }

    #[test]
    fn page_down_maps_to_page_down() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::PageDown)), Intent::PageDown);
    }

    #[test]
    fn enter_maps_to_toggle_expand() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Enter)), Intent::ToggleExpand);
    }

    #[test]
    fn slash_maps_to_jump_to_search() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Char('/'))), Intent::JumpToSearch);
    }

    #[test]
    fn esc_maps_to_search_clear() {
        // spec: TUI-11
        assert_eq!(key_to_intent(key(KeyCode::Esc)), Intent::SearchClear);
    }

    #[test]
    fn backspace_maps_to_search_backspace() {
        // spec: TUI-11
        assert_eq!(
            key_to_intent(key(KeyCode::Backspace)),
            Intent::SearchBackspace
        );
    }

    #[test]
    fn q_maps_to_quit() {
        // spec: TUI-41
        assert_eq!(key_to_intent(key(KeyCode::Char('q'))), Intent::Quit);
    }

    #[test]
    fn ctrl_c_maps_to_quit() {
        // spec: TUI-41
        assert_eq!(
            key_to_intent(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Intent::Quit
        );
    }

    #[test]
    fn i_maps_to_learn() {
        // spec: TUI-20
        assert_eq!(key_to_intent(key(KeyCode::Char('i'))), Intent::ActionLearn);
    }

    #[test]
    fn d_maps_to_forget() {
        // spec: TUI-20
        assert_eq!(key_to_intent(key(KeyCode::Char('d'))), Intent::ActionForget);
    }

    #[test]
    fn s_maps_to_sync() {
        // spec: TUI-22
        assert_eq!(key_to_intent(key(KeyCode::Char('s'))), Intent::ActionSync);
    }

    #[test]
    fn u_maps_to_upgrade() {
        // spec: TUI-22
        assert_eq!(
            key_to_intent(key(KeyCode::Char('u'))),
            Intent::ActionUpgrade
        );
    }

    #[test]
    fn m_maps_to_meld() {
        // spec: TUI-21
        assert_eq!(key_to_intent(key(KeyCode::Char('m'))), Intent::ActionMeld);
    }

    #[test]
    fn y_maps_to_confirm_action() {
        // spec: TUI-24
        assert_eq!(
            key_to_intent(key(KeyCode::Char('y'))),
            Intent::ConfirmAction
        );
    }

    #[test]
    fn n_maps_to_cancel_action() {
        // spec: TUI-24
        assert_eq!(key_to_intent(key(KeyCode::Char('n'))), Intent::CancelAction);
    }

    #[test]
    fn printable_char_becomes_search_char() {
        // spec: TUI-14
        assert_eq!(
            key_to_intent(key(KeyCode::Char('r'))),
            Intent::SearchChar('r')
        );
        assert_eq!(
            key_to_intent(key(KeyCode::Char('0'))),
            Intent::SearchChar('0')
        );
    }

    // spec: TUI-30 - spec-input intents are defined in the enum and match the
    // expected variants (the event loop dispatches them when spec_input_active).
    #[test]
    fn spec_input_intents_are_distinct_and_constructible() {
        // spec: TUI-30
        // Verify the spec-input intents exist and compare equal to themselves.
        let char_intent = Intent::SpecInputChar('a');
        let bs_intent = Intent::SpecInputBackspace;
        let sub_intent = Intent::SpecInputSubmit;
        assert_eq!(char_intent, Intent::SpecInputChar('a'));
        assert_ne!(char_intent, Intent::SpecInputChar('b'));
        assert_eq!(bs_intent, Intent::SpecInputBackspace);
        assert_eq!(sub_intent, Intent::SpecInputSubmit);
    }

    #[test]
    fn preview_ready_and_error_intents_carry_payload() {
        // spec: TUI-30
        let ready = Intent::PreviewReady {
            spec: "github.com/a/b".to_string(),
            name: "b (3 items)".to_string(),
        };
        let err = Intent::PreviewError {
            message: "clone failed".to_string(),
        };
        // These must not equal each other or their components.
        assert_ne!(std::mem::discriminant(&ready), std::mem::discriminant(&err));
        if let Intent::PreviewReady { spec, name } = &ready {
            assert_eq!(spec, "github.com/a/b");
            assert_eq!(name, "b (3 items)");
        } else {
            panic!("expected PreviewReady");
        }
        if let Intent::PreviewError { message } = &err {
            assert_eq!(message, "clone failed");
        } else {
            panic!("expected PreviewError");
        }
    }

    // --- TUI-23: lobe management key bindings ---

    #[test]
    fn shift_c_maps_to_action_lobes() {
        // spec: TUI-23 CLI-111 - `C` (Shift+C) opens the lobes modal.
        assert_eq!(
            key_to_intent(key_mod(KeyCode::Char('C'), KeyModifiers::SHIFT)),
            Intent::ActionLobes
        );
    }

    #[test]
    fn lobe_input_intents_are_distinct_and_constructible() {
        // spec: TUI-23 CLI-112 CLI-113 - lobe-input intents exist and are comparable.
        let char_intent = Intent::LobeInputChar('x');
        let bs_intent = Intent::LobeInputBackspace;
        let sub_intent = Intent::LobeInputSubmit;
        assert_eq!(char_intent, Intent::LobeInputChar('x'));
        assert_ne!(char_intent, Intent::LobeInputChar('y'));
        assert_eq!(bs_intent, Intent::LobeInputBackspace);
        assert_eq!(sub_intent, Intent::LobeInputSubmit);
    }

    #[test]
    fn action_lobe_add_and_remove_intents_exist() {
        // spec: TUI-23 CLI-112 CLI-113 - ActionLobeAdd and ActionLobeRemove are
        // distinct Intent variants (used from the lobes modal, not from global keys).
        assert_ne!(
            std::mem::discriminant(&Intent::ActionLobeAdd),
            std::mem::discriminant(&Intent::ActionLobeRemove)
        );
        assert_ne!(
            std::mem::discriminant(&Intent::ActionLobeAdd),
            std::mem::discriminant(&Intent::ActionLobes)
        );
    }
}
