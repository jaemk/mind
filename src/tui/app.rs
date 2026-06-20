//! App state model for the interactive TUI.
//!
//! Pure state; no I/O, no ratatui. All state transitions are through methods so
//! the logic is unit-testable without a terminal.

use crate::error::ItemKind;
use crate::tui::data::Snapshot;
use crate::tui::event::Intent;
use crate::tui::preview::SourcePreview;
use crate::tui::tree::{TreeNode, flatten_tree};

/// A pending mutating action waiting for confirmation.
// spec: TUI-24
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub kind: ActionKind,
    /// Human-readable description shown in the confirm dialog.
    pub description: String,
}

/// What kind of action is being confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Meld is used by the interactive meld flow (TUI-30/31)
pub enum ActionKind {
    /// Install an available item (TUI-20).
    Learn { item_key: String, source: String },
    /// Uninstall an installed item (TUI-20).
    Forget { item_key: String },
    /// Meld a source (TUI-21). Used when user confirms an ad-hoc or preview meld.
    Meld { spec: String },
    /// Unmeld a source (TUI-21).
    Unmeld { name: String, forget: bool },
    /// Sync all sources (TUI-22).
    Sync,
    /// Evolve pending items (TUI-22).
    Evolve,
    /// Add an agent home (lobe) via `config lobes add` (TUI-23, CLI-112).
    // spec: TUI-23
    LobeAdd { path: String },
    /// Remove an agent home (lobe) via `config lobes remove` (TUI-23, CLI-113).
    // spec: TUI-23
    LobeRemove { path: String },
}

/// The complete UI state.
// spec: TUI-10 TUI-11 TUI-14
pub struct App {
    // --- filters / search ---
    pub search: String,
    pub kind_filter: Option<ItemKind>,
    pub source_filter: Option<String>,

    // --- tree state ---
    /// Nodes from the last snapshot, filtered/flattened for display.
    pub visible: Vec<FlatNode>,
    /// Selected row index into `visible`.
    pub selected: usize,
    /// Expanded node IDs.
    pub expanded: std::collections::HashSet<String>,
    /// The last snapshot we applied.
    pub last_snapshot: Option<Snapshot>,

    // --- groups ---
    pub installed_collapsed: bool,
    pub available_collapsed: bool,

    // --- modal state ---
    pub pending_action: Option<PendingAction>,
    pub modal_visible: bool,

    // --- status ---
    pub status: Option<String>,
    pub error: Option<String>,

    // --- spec-input / preview state (TUI-30) ---
    /// True when the user is entering a repo spec via the `m` key flow.
    pub spec_input_active: bool,
    /// The text being typed into the spec input box.
    pub spec_input_text: String,
    /// An active preview (shallow clone awaiting confirm/decline). Dropping this
    /// field discards the temp clone (via SourcePreview::drop, TUI-30).
    pub active_preview: Option<SourcePreview>,
    /// Set by expand of a SuggestedSource; the event loop consumes this to call preview().
    pub pending_preview_spec: Option<String>,

    // --- lobe management state (TUI-23) ---
    /// True when the lobes management modal is open (list + add/remove).
    // spec: TUI-23
    pub lobes_modal_visible: bool,
    /// Configured lobes shown in the lobes modal; mirrors last snapshot.
    pub lobes: Vec<String>,
    /// Selected row in the lobes modal list.
    pub lobes_selected: usize,
    /// True when the user is typing a new lobe path in the add-lobe input box.
    pub lobe_input_active: bool,
    /// Text being typed into the add-lobe input box.
    pub lobe_input_text: String,

    // --- misc ---
    pub size: (u16, u16),
    pub mutating: bool,
    pub quit: bool,
    pub search_focused: bool,
}

/// One row in the flat (display) tree.
#[derive(Debug, Clone)]
pub struct FlatNode {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
    pub node: TreeNode,
}

impl App {
    /// Create a new App, seeded from CLI arguments (TUI-2).
    // spec: TUI-2
    pub fn new(
        seed_query: String,
        seed_kind: Option<ItemKind>,
        seed_source: Option<String>,
    ) -> Self {
        App {
            search: seed_query,
            kind_filter: seed_kind,
            source_filter: seed_source,
            visible: Vec::new(),
            selected: 0,
            expanded: std::collections::HashSet::new(),
            last_snapshot: None,
            installed_collapsed: false,
            available_collapsed: false,
            pending_action: None,
            modal_visible: false,
            status: None,
            error: None,
            spec_input_active: false,
            spec_input_text: String::new(),
            active_preview: None,
            pending_preview_spec: None,
            lobes_modal_visible: false,
            lobes: Vec::new(),
            lobes_selected: 0,
            lobe_input_active: false,
            lobe_input_text: String::new(),
            size: (80, 24),
            mutating: false,
            quit: false,
            search_focused: false,
        }
    }

    /// Apply a data snapshot, rebuilding the tree. Preserves selection and
    /// expansion state across refreshes (TUI-15). Also refreshes the lobes
    /// list for the lobes modal (TUI-23).
    // spec: TUI-15
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        // Any successful snapshot-applying refresh ends a mutation: clearing the
        // flag here is what re-arms the once-a-second poll after a successful
        // learn/forget/sync/evolve/meld/lobe action (TUI-15). The mid-action
        // assertion in `take_pending_action` still holds because the flag is set
        // and observed before any snapshot is applied.
        // spec: TUI-15
        self.mutating = false;
        // Refresh lobes from snapshot before storing (TUI-23).
        // spec: TUI-23
        self.lobes = snapshot.lobes.clone();
        // Clamp selection in case lobes list shrank.
        self.lobes_selected = self.lobes_selected.min(self.lobes.len().saturating_sub(1));
        self.last_snapshot = Some(snapshot);
        self.rebuild_tree();
    }

    /// Apply a snapshot only if the data changed (used for poll ticks).
    // spec: TUI-15
    pub fn apply_snapshot_if_changed(&mut self, snapshot: Snapshot) {
        let changed = self
            .last_snapshot
            .as_ref()
            .map(|s| s.generation != snapshot.generation)
            .unwrap_or(true);
        if changed {
            self.apply_snapshot(snapshot);
        }
    }

    /// Rebuild the flat visible list from the current snapshot and filters,
    /// preserving the selected item by ID if possible.
    fn rebuild_tree(&mut self) {
        let Some(snap) = &self.last_snapshot else {
            return;
        };

        // Remember what was selected by ID so we can restore after rebuild.
        let selected_id = self
            .visible
            .get(self.selected)
            .map(|n| n.id.clone());

        let nodes = crate::tui::tree::build_tree(
            snap,
            &self.search,
            self.kind_filter,
            self.source_filter.as_deref(),
            self.installed_collapsed,
            self.available_collapsed,
        );
        self.visible = flatten_tree(&nodes, &self.expanded);

        // Restore selection to same ID, or clamp.
        if let Some(id) = selected_id {
            if let Some(idx) = self.visible.iter().position(|n| n.id == id) {
                self.selected = idx;
            } else {
                self.selected = self.selected.min(self.visible.len().saturating_sub(1));
            }
        } else {
            self.selected = self.selected.min(self.visible.len().saturating_sub(1));
        }
    }

    // --- Intent handling ---

    /// Apply a non-action intent (movement, expand/collapse, search, etc.).
    // spec: TUI-11 TUI-14
    pub fn apply_intent(&mut self, intent: Intent) {
        match intent {
            Intent::MoveUp => {
                self.error = None;
                self.status = None;
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            Intent::MoveDown => {
                self.error = None;
                self.status = None;
                if self.selected + 1 < self.visible.len() {
                    self.selected += 1;
                }
            }
            Intent::PageUp => {
                let page = self.page_size();
                self.selected = self.selected.saturating_sub(page);
            }
            Intent::PageDown => {
                let page = self.page_size();
                self.selected = (self.selected + page).min(self.visible.len().saturating_sub(1));
            }
            Intent::Expand => {
                if let Some(node) = self.visible.get(self.selected).cloned() {
                    if let TreeNode::SuggestedSource(ref sug) = node.node {
                        // Expanding a suggested source triggers a preview (TUI-31).
                        // spec: TUI-31
                        self.pending_preview_spec = Some(sug.spec.clone());
                        self.set_status(format!("Previewing {}...", sug.name));
                    } else if node.expandable {
                        self.expanded.insert(node.id.clone());
                        self.rebuild_tree();
                    }
                }
            }
            Intent::Collapse => {
                if let Some(node) = self.visible.get(self.selected).cloned() {
                    if self.expanded.remove(&node.id) {
                        self.rebuild_tree();
                    } else if node.depth > 0 {
                        // Jump to parent.
                        let parent_depth = node.depth - 1;
                        if let Some(idx) = (0..self.selected)
                            .rev()
                            .find(|&i| self.visible[i].depth == parent_depth)
                        {
                            self.selected = idx;
                        }
                    }
                }
            }
            Intent::ToggleExpand => {
                if let Some(node) = self.visible.get(self.selected).cloned() {
                    if let TreeNode::SuggestedSource(ref sug) = node.node {
                        // Toggle-expand on a suggested source triggers a preview (TUI-31).
                        // spec: TUI-31
                        self.pending_preview_spec = Some(sug.spec.clone());
                        self.set_status(format!("Previewing {}...", sug.name));
                    } else {
                        if self.expanded.contains(&node.id) {
                            self.expanded.remove(&node.id);
                        } else if node.expandable {
                            self.expanded.insert(node.id.clone());
                        }
                        self.rebuild_tree();
                    }
                }
            }
            Intent::JumpToSearch => {
                self.search_focused = true;
            }
            Intent::SearchChar(c) => {
                if self.search_focused {
                    self.search.push(c);
                    self.rebuild_tree();
                }
            }
            Intent::SearchBackspace => {
                if self.search_focused {
                    self.search.pop();
                    self.rebuild_tree();
                }
            }
            Intent::SearchClear => {
                self.search.clear();
                self.search_focused = false;
                self.rebuild_tree();
            }
            Intent::SearchSubmit => {
                self.search_focused = false;
            }
            Intent::ActionLearn => {
                self.initiate_learn();
            }
            Intent::ActionForget => {
                self.initiate_forget();
            }
            Intent::ActionSync => {
                self.initiate_sync();
            }
            Intent::ActionEvolve => {
                self.initiate_evolve();
            }
            Intent::ActionMeld => {
                self.initiate_meld();
            }
            Intent::ActionUnmeld => {
                self.initiate_unmeld();
            }
            Intent::CancelAction => {
                if self.lobe_input_active {
                    // Esc while in lobe-input mode cancels the input.
                    self.lobe_input_active = false;
                    self.lobe_input_text.clear();
                    self.lobes_modal_visible = true; // return to lobes list
                } else if self.spec_input_active {
                    // Esc while in spec-input mode cancels the input.
                    self.spec_input_active = false;
                    self.spec_input_text.clear();
                    self.status = None;
                    self.error = None;
                } else if self.lobes_modal_visible {
                    // Esc/n closes the lobes modal.
                    self.lobes_modal_visible = false;
                } else {
                    // Cancel a pending confirm modal (decline preview meld too).
                    self.pending_action = None;
                    self.modal_visible = false;
                    // Discard any active preview on decline (TUI-30).
                    // spec: TUI-30
                    self.active_preview = None;
                }
            }
            // spec: TUI-30
            Intent::SpecInputChar(c) => {
                if self.spec_input_active {
                    self.spec_input_text.push(c);
                }
            }
            Intent::SpecInputBackspace => {
                if self.spec_input_active {
                    self.spec_input_text.pop();
                }
            }
            // SpecInputSubmit: the event loop will call preview() and wire the result.
            // Handled in mod.rs event_loop, not here, because it requires I/O.
            Intent::SpecInputSubmit => {
                // Handled in mod.rs where I/O is available.
            }
            // Set a preview result (called by the event loop after preview() succeeds).
            // This creates a Meld PendingAction and shows the confirm modal.
            // spec: TUI-30
            Intent::PreviewReady { spec, name } => {
                self.spec_input_active = false;
                // Active preview already set by the event loop.
                let desc = format!("Preview: {} - confirm meld?", name);
                self.pending_action = Some(PendingAction {
                    kind: ActionKind::Meld { spec: spec.clone() },
                    description: desc,
                });
                self.modal_visible = true;
                self.status = None;
                self.error = None;
            }
            // spec: TUI-30
            Intent::PreviewError { message } => {
                self.spec_input_active = false;
                self.spec_input_text.clear();
                self.active_preview = None;
                self.set_error(message);
            }
            // --- Lobe management intents (TUI-23) ---

            // spec: TUI-23 CLI-111
            Intent::ActionLobes => {
                self.lobes_modal_visible = true;
                self.lobe_input_active = false;
                self.lobe_input_text.clear();
                // Clamp selection.
                self.lobes_selected = self.lobes_selected.min(self.lobes.len().saturating_sub(1));
            }
            // spec: TUI-23 CLI-112
            Intent::ActionLobeAdd => {
                if self.lobes_modal_visible {
                    self.lobe_input_active = true;
                    self.lobe_input_text.clear();
                }
            }
            // spec: TUI-23 CLI-113
            Intent::ActionLobeRemove => {
                if self.lobes_modal_visible && !self.lobes.is_empty() {
                    let path = self.lobes[self.lobes_selected].clone();
                    let desc = format!("Remove agent home (lobe) \"{}\"?", path);
                    self.pending_action = Some(PendingAction {
                        kind: ActionKind::LobeRemove { path },
                        description: desc,
                    });
                    self.lobes_modal_visible = false;
                    self.modal_visible = true;
                }
            }
            Intent::LobeInputChar(c) => {
                if self.lobe_input_active {
                    self.lobe_input_text.push(c);
                }
            }
            Intent::LobeInputBackspace => {
                if self.lobe_input_active {
                    self.lobe_input_text.pop();
                }
            }
            // LobeInputSubmit: handled in mod.rs (needs to trigger an action).
            Intent::LobeInputSubmit => {
                // Handled in mod.rs where action dispatch is available.
            }
            Intent::LobeSelectUp => {
                if self.lobes_selected > 0 {
                    self.lobes_selected -= 1;
                }
            }
            Intent::LobeSelectDown => {
                if self.lobes_selected + 1 < self.lobes.len() {
                    self.lobes_selected += 1;
                }
            }
            Intent::Quit => {
                self.quit = true;
            }
            _ => {}
        }
    }

    fn page_size(&self) -> usize {
        let h = self.size.1 as usize;
        h.saturating_sub(6).max(1)
    }

    /// Initiate an install action for the currently selected available item.
    fn initiate_learn(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        if let TreeNode::AvailableItem(ref item) = node.node {
            let desc = format!(
                "Install {} from {}?",
                item.key,
                item.source
            );
            self.pending_action = Some(PendingAction {
                kind: ActionKind::Learn {
                    item_key: item.key.clone(),
                    source: item.source.clone(),
                },
                description: desc,
            });
            self.modal_visible = true;
        }
    }

    /// Initiate an uninstall action for the currently selected installed item.
    fn initiate_forget(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        if let TreeNode::InstalledItem(ref item) = node.node {
            let desc = format!("Forget (uninstall) {}?", item.key);
            self.pending_action = Some(PendingAction {
                kind: ActionKind::Forget {
                    item_key: item.key.clone(),
                },
                description: desc,
            });
            self.modal_visible = true;
        }
    }

    fn initiate_sync(&mut self) {
        self.pending_action = Some(PendingAction {
            kind: ActionKind::Sync,
            description: "Sync all sources?".to_string(),
        });
        self.modal_visible = true;
    }

    fn initiate_evolve(&mut self) {
        self.pending_action = Some(PendingAction {
            kind: ActionKind::Evolve,
            description: "Evolve all pending items?".to_string(),
        });
        self.modal_visible = true;
    }

    fn initiate_meld(&mut self) {
        // Open the spec-input box for the user to type a repo spec (TUI-30).
        self.spec_input_active = true;
        self.spec_input_text.clear();
        self.set_status("Enter repo spec and press Enter to preview. Esc to cancel.".to_string());
    }

    /// Initiate a LobeAdd action from the current lobe_input_text (TUI-23).
    /// Called by mod.rs when the user submits the lobe-path input box.
    // spec: TUI-23 CLI-112
    pub fn submit_lobe_add(&mut self) {
        let path = self.lobe_input_text.trim().to_string();
        if path.is_empty() {
            // Empty input: cancel back to lobes list.
            self.lobe_input_active = false;
            self.lobe_input_text.clear();
            self.lobes_modal_visible = true;
            return;
        }
        self.lobe_input_active = false;
        self.lobe_input_text.clear();
        self.lobes_modal_visible = false;
        let desc = format!("Add agent home (lobe) \"{}\"?", path);
        self.pending_action = Some(PendingAction {
            kind: ActionKind::LobeAdd { path },
            description: desc,
        });
        self.modal_visible = true;
    }

    fn initiate_unmeld(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        if let TreeNode::Source(ref src) = node.node {
            let desc = format!("Unmeld source {}?", src.name);
            self.pending_action = Some(PendingAction {
                kind: ActionKind::Unmeld {
                    name: src.name.clone(),
                    forget: false,
                },
                description: desc,
            });
            self.modal_visible = true;
        }
    }

    /// User pressed Enter/Return to confirm the pending action.
    pub fn confirm_selected(&mut self) {
        if self.modal_visible {
            // The event loop will handle ConfirmAction
        } else if let Some(node) = self.visible.get(self.selected).cloned() {
            // Toggle expand on Enter
            if node.expandable {
                if self.expanded.contains(&node.id) {
                    self.expanded.remove(&node.id);
                } else {
                    self.expanded.insert(node.id.clone());
                }
                self.rebuild_tree();
            }
        }
    }

    /// Take the pending action (consume it for execution).
    pub fn take_pending_action(&mut self) -> Option<PendingAction> {
        let action = self.pending_action.take();
        if action.is_some() {
            self.modal_visible = false;
            self.mutating = true;
        }
        action
    }

    pub fn is_mutating(&self) -> bool {
        self.mutating
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn set_status(&mut self, s: String) {
        self.status = Some(s);
        self.error = None;
    }

    pub fn set_error(&mut self, s: String) {
        self.error = Some(s);
        self.status = None;
        self.mutating = false;
    }

    pub fn set_size(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{Snapshot, SnapshotInstalled, SnapshotAvailable};
    use crate::error::ItemKind;

    fn make_snapshot() -> Snapshot {
        Snapshot {
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
        }
    }

    #[test]
    fn new_app_seeds_from_cli_args() {
        // spec: TUI-2
        let app = App::new("review".to_string(), Some(ItemKind::Skill), Some("src".to_string()));
        assert_eq!(app.search, "review");
        assert_eq!(app.kind_filter, Some(ItemKind::Skill));
        assert_eq!(app.source_filter, Some("src".to_string()));
    }

    #[test]
    fn apply_snapshot_populates_visible_tree() {
        // spec: TUI-12 TUI-13
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        // Should have at least the group headers + some items
        assert!(!app.visible.is_empty(), "tree should be non-empty after snapshot");
    }

    #[test]
    fn move_down_advances_selection() {
        // spec: TUI-11
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        app.selected = 0;
        let initial = app.selected;
        if app.visible.len() > 1 {
            app.apply_intent(Intent::MoveDown);
            assert_eq!(app.selected, initial + 1, "MoveDown should advance selection");
        }
    }

    #[test]
    fn move_up_does_not_go_below_zero() {
        // spec: TUI-11
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        app.selected = 0;
        app.apply_intent(Intent::MoveUp);
        assert_eq!(app.selected, 0, "MoveUp at top should stay at 0");
    }

    #[test]
    fn move_down_clamped_at_end() {
        // spec: TUI-11
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let last = app.visible.len().saturating_sub(1);
        app.selected = last;
        app.apply_intent(Intent::MoveDown);
        assert_eq!(app.selected, last, "MoveDown at bottom should stay at last");
    }

    #[test]
    fn search_filters_tree() {
        // spec: TUI-14
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let all_count = app.visible.len();

        // Expand first to get items visible
        app.search = "zzznomatch".to_string();
        app.rebuild_tree();
        // Either fewer nodes or the same (headers may remain)
        // The key assertion: items not matching query should be hidden
        let matched: Vec<_> = app.visible.iter()
            .filter(|n| n.label.contains("zzznomatch"))
            .collect();
        assert!(matched.is_empty(), "non-matching items should be hidden: {:?}",
            app.visible.iter().map(|n| &n.label).collect::<Vec<_>>());
        let _ = all_count;
    }

    #[test]
    fn expand_toggles_expand_set() {
        // spec: TUI-11
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        // Find an expandable node
        if let Some(idx) = app.visible.iter().position(|n| n.expandable) {
            app.selected = idx;
            let id = app.visible[idx].id.clone();
            app.apply_intent(Intent::Expand);
            assert!(app.expanded.contains(&id), "expand should add to expanded set");
        }
    }

    #[test]
    fn collapse_removes_from_expand_set() {
        // spec: TUI-11
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        if let Some(idx) = app.visible.iter().position(|n| n.expandable) {
            app.selected = idx;
            let id = app.visible[idx].id.clone();
            app.expanded.insert(id.clone());
            app.rebuild_tree();
            // Find same node
            if let Some(new_idx) = app.visible.iter().position(|n| n.id == id) {
                app.selected = new_idx;
                app.apply_intent(Intent::Collapse);
                assert!(!app.expanded.contains(&id), "collapse should remove from expanded set");
            }
        }
    }

    #[test]
    fn snapshot_preserves_selection_across_refresh() {
        // spec: TUI-15
        let mut app = App::new(String::new(), None, None);
        let snap1 = make_snapshot();
        app.apply_snapshot(snap1);
        if !app.visible.is_empty() {
            app.selected = 0;
            let id = app.visible[0].id.clone();
            // Apply same snapshot again (same data, same generation -> no change expected)
            let mut snap2 = make_snapshot();
            snap2.generation = 2; // different generation
            app.apply_snapshot(snap2);
            // Selection should still point to same id if it exists
            if let Some(idx) = app.visible.iter().position(|n| n.id == id) {
                assert_eq!(app.selected, idx, "selection should be preserved by ID after refresh");
            }
        }
    }

    #[test]
    fn refresh_preserves_search_and_expansion_state() {
        // spec: TUI-15 - a refresh (a genuinely changed snapshot) must preserve the
        // user's search query and expansion set, not just the selection. Otherwise
        // the once-a-second poll would wipe what the user is doing.
        let mut app = App::new(String::new(), None, None);
        let mut snap1 = make_snapshot();
        snap1.generation = 1;
        app.apply_snapshot(snap1);

        // Establish search + expansion state.
        app.search = "dev".to_string();
        if let Some(idx) = app.visible.iter().position(|n| n.expandable) {
            let id = app.visible[idx].id.clone();
            app.expanded.insert(id);
        }
        app.rebuild_tree();
        let search_before = app.search.clone();
        let expanded_before = app.expanded.clone();

        // A new, higher-generation snapshot arrives (another process touched disk).
        let mut snap2 = make_snapshot();
        snap2.generation = 2;
        app.apply_snapshot_if_changed(snap2);

        assert_eq!(app.search, search_before, "search query must survive a refresh");
        assert_eq!(app.expanded, expanded_before, "expansion set must survive a refresh");
        // And the search filter is still in effect after the refresh.
        let has_unfiltered = app.visible.iter().any(|n| {
            matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(i) if i.name == "review")
        });
        assert!(!has_unfiltered, "the preserved 'dev' search must still hide non-matching items");
    }

    #[test]
    fn pending_action_set_and_taken() {
        // spec: TUI-24
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Sync,
            description: "Sync all?".to_string(),
        });
        app.modal_visible = true;
        let taken = app.take_pending_action();
        assert!(taken.is_some(), "take_pending_action should return the pending action");
        assert!(app.pending_action.is_none(), "pending_action should be cleared after take");
        assert!(!app.modal_visible, "modal should be hidden after take");
        assert!(app.mutating, "mutating should be true while action runs");
    }

    #[test]
    fn successful_action_clears_mutating_so_poll_rearms() {
        // spec: TUI-15 - the success path (take_pending_action sets mutating,
        // then apply_snapshot applies the refreshed data) MUST clear the
        // mutating flag. Otherwise the once-a-second poll, gated on
        // `!is_mutating()`, stops forever after the first successful action.
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Sync,
            description: "Sync all?".to_string(),
        });
        app.modal_visible = true;

        // Confirm the action: this is what the `y` path does before executing.
        let taken = app.take_pending_action();
        assert!(taken.is_some(), "confirm should yield the pending action");
        assert!(app.is_mutating(), "mutating is set while the action runs");

        // The action succeeded and produced a snapshot; the success path in
        // handle_key applies it. Applying any snapshot must re-arm the poll.
        app.apply_snapshot(make_snapshot());
        assert!(
            !app.is_mutating(),
            "a successful snapshot-applying refresh must clear mutating"
        );
    }

    #[test]
    fn cancel_action_clears_pending() {
        // spec: TUI-24
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Sync,
            description: "Sync?".to_string(),
        });
        app.modal_visible = true;
        app.apply_intent(Intent::CancelAction);
        assert!(app.pending_action.is_none());
        assert!(!app.modal_visible);
    }

    #[test]
    fn apply_snapshot_if_changed_is_noop_when_generation_equal() {
        // spec: TUI-15 - the poll tick gates re-application on the generation
        // counter. When the new snapshot has the SAME generation as the last
        // applied one, apply_snapshot_if_changed must NOT rebuild (it would
        // otherwise reset selection/expansion every second). The implementor
        // flagged the "equal, not just less" edge, so pin it explicitly.
        let mut app = App::new(String::new(), None, None);
        let mut snap1 = make_snapshot();
        snap1.generation = 7;
        app.apply_snapshot(snap1);

        // Move selection and expand a node so we can detect an unwanted rebuild.
        let expandable_idx = app.visible.iter().position(|n| n.expandable);
        if let Some(idx) = expandable_idx {
            app.selected = idx;
            let id = app.visible[idx].id.clone();
            app.expanded.insert(id);
        }
        let selected_before = app.selected;
        let expanded_before = app.expanded.clone();
        let visible_before: Vec<String> = app.visible.iter().map(|n| n.id.clone()).collect();

        // A snapshot with the SAME generation: must be ignored (no rebuild).
        let mut snap_same = make_snapshot();
        snap_same.generation = 7;
        // Make the data clearly different so a wrongful rebuild would be visible.
        snap_same.installed.clear();
        app.apply_snapshot_if_changed(snap_same);

        assert_eq!(app.selected, selected_before, "selection must be unchanged on equal generation");
        assert_eq!(app.expanded, expanded_before, "expansion must be unchanged on equal generation");
        let visible_after: Vec<String> = app.visible.iter().map(|n| n.id.clone()).collect();
        assert_eq!(
            visible_after, visible_before,
            "tree must NOT be rebuilt when generation is equal (would have dropped installed items)"
        );
    }

    #[test]
    fn apply_snapshot_if_changed_rebuilds_when_generation_differs() {
        // spec: TUI-15 - the contrast case: a higher generation IS applied, so a
        // genuine on-disk change (e.g. a source added by another process) shows up.
        let mut app = App::new(String::new(), None, None);
        let mut snap1 = make_snapshot();
        snap1.generation = 1;
        app.apply_snapshot(snap1);
        let had_installed = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(had_installed, "fixture should start with an installed item");

        // New generation, installed list emptied: must be applied.
        let mut snap2 = make_snapshot();
        snap2.generation = 2;
        snap2.installed.clear();
        app.apply_snapshot_if_changed(snap2);
        let still_installed = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(!still_installed, "a higher generation must rebuild and reflect the new data");
    }

    #[test]
    fn forget_initiates_a_confirm_gated_pending_action() {
        // spec: TUI-24 - forget (a destructive uninstall) must not run immediately:
        // it sets a pending action and shows the confirm modal. The action is only
        // released by take_pending_action (the `y` path), so nothing mutates without
        // an explicit confirmation.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        // Select the installed item.
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)))
            .expect("installed item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionForget);
        assert!(app.modal_visible, "forget must open a confirm modal (destructive gating)");
        let pending = app.pending_action.as_ref().expect("forget must set a pending action");
        assert!(
            matches!(&pending.kind, ActionKind::Forget { item_key } if item_key == "skill:review"),
            "pending action should be Forget for the selected item"
        );
        // Declining (n) must clear it WITHOUT mutating.
        app.apply_intent(Intent::CancelAction);
        assert!(app.pending_action.is_none(), "decline clears the pending forget");
        assert!(!app.modal_visible);
    }

    #[test]
    fn destructive_unmeld_forget_is_gated_by_confirmation() {
        // spec: TUI-24 - a destructive Unmeld{forget:true} is only handed to the
        // executor by take_pending_action (the confirm step). Until confirmed it
        // sits as a pending action and does not run.
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Unmeld { name: "local/x".to_string(), forget: true },
            description: "Unmeld local/x --forget?".to_string(),
        });
        app.modal_visible = true;
        // No confirmation yet: the action is still pending (not executed).
        assert!(app.pending_action.is_some(), "destructive action waits for confirm");
        // Confirm: take_pending_action yields it (the executor then runs it).
        let taken = app.take_pending_action().expect("confirm yields the action");
        assert!(
            matches!(taken.kind, ActionKind::Unmeld { forget: true, .. }),
            "the destructive forget flag must survive through confirmation"
        );
        assert!(app.mutating, "mutating flag is set once the action is taken");
    }

    #[test]
    fn quit_intent_sets_quit_flag() {
        // spec: TUI-41
        let mut app = App::new(String::new(), None, None);
        assert!(!app.should_quit());
        app.apply_intent(Intent::Quit);
        assert!(app.should_quit());
    }

    #[test]
    fn set_error_clears_status_and_resets_mutating() {
        // spec: TUI-24
        let mut app = App::new(String::new(), None, None);
        app.mutating = true;
        app.status = Some("Working...".to_string());
        app.set_error("something went wrong".to_string());
        assert_eq!(app.error, Some("something went wrong".to_string()));
        assert!(app.status.is_none());
        assert!(!app.mutating);
    }

    #[test]
    fn kind_filter_is_seeded_from_cli() {
        // spec: TUI-2
        let app = App::new(String::new(), Some(ItemKind::Rule), None);
        assert_eq!(app.kind_filter, Some(ItemKind::Rule));
    }

    #[test]
    fn search_char_appends_when_search_focused() {
        // spec: TUI-14
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        app.search_focused = true;
        app.apply_intent(Intent::SearchChar('r'));
        app.apply_intent(Intent::SearchChar('e'));
        assert_eq!(app.search, "re");
    }

    #[test]
    fn search_backspace_removes_last_char() {
        // spec: TUI-14
        let mut app = App::new("rev".to_string(), None, None);
        app.apply_snapshot(make_snapshot());
        app.search_focused = true;
        app.apply_intent(Intent::SearchBackspace);
        assert_eq!(app.search, "re");
    }

    #[test]
    fn search_clear_resets_search_string() {
        // spec: TUI-14
        let mut app = App::new("something".to_string(), None, None);
        app.apply_snapshot(make_snapshot());
        app.search_focused = true;
        app.apply_intent(Intent::SearchClear);
        assert_eq!(app.search, "");
        assert!(!app.search_focused);
    }

    // --- TUI-30: spec-input / preview flow ---

    #[test]
    fn action_meld_activates_spec_input() {
        // spec: TUI-30 - pressing `m` opens the spec-input box.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        app.apply_intent(Intent::ActionMeld);
        assert!(app.spec_input_active, "ActionMeld should activate spec input");
        assert!(app.spec_input_text.is_empty(), "spec input text should be empty initially");
    }

    #[test]
    fn spec_input_char_appends_when_active() {
        // spec: TUI-30 - characters are routed to the spec-input buffer.
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = true;
        app.apply_intent(Intent::SpecInputChar('g'));
        app.apply_intent(Intent::SpecInputChar('h'));
        assert_eq!(app.spec_input_text, "gh");
    }

    #[test]
    fn spec_input_char_ignored_when_not_active() {
        // spec: TUI-30 - SpecInputChar is a no-op when not in input mode.
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = false;
        app.apply_intent(Intent::SpecInputChar('x'));
        assert_eq!(app.spec_input_text, "");
    }

    #[test]
    fn spec_input_backspace_removes_last_char() {
        // spec: TUI-30
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = true;
        app.spec_input_text = "gh".to_string();
        app.apply_intent(Intent::SpecInputBackspace);
        assert_eq!(app.spec_input_text, "g");
    }

    #[test]
    fn cancel_while_spec_input_active_clears_input() {
        // spec: TUI-30 - Esc/CancelAction during spec-input cancels without setting a pending action.
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = true;
        app.spec_input_text = "some/repo".to_string();
        app.apply_intent(Intent::CancelAction);
        assert!(!app.spec_input_active, "spec input should be inactive after cancel");
        assert!(app.spec_input_text.is_empty(), "spec input text should be cleared on cancel");
        assert!(app.pending_action.is_none(), "no action should be pending after cancel");
    }

    #[test]
    fn preview_ready_sets_meld_pending_action_and_modal() {
        // spec: TUI-30 - PreviewReady intent opens the confirm modal for a Meld action.
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = true;
        app.spec_input_text = "github.com/a/repo".to_string();
        app.apply_intent(Intent::PreviewReady {
            spec: "github.com/a/repo".to_string(),
            name: "repo (2 items)".to_string(),
        });
        assert!(!app.spec_input_active, "spec input should be inactive after preview ready");
        assert!(app.modal_visible, "confirm modal should be visible after preview ready");
        let action = app.pending_action.as_ref().expect("pending action should be set");
        assert!(
            matches!(&action.kind, ActionKind::Meld { spec } if spec == "github.com/a/repo"),
            "pending action should be Meld with the submitted spec"
        );
    }

    #[test]
    fn preview_error_clears_input_and_sets_error_inline() {
        // spec: TUI-30 TUI-24 - preview errors are surfaced inline, not to stderr.
        let mut app = App::new(String::new(), None, None);
        app.spec_input_active = true;
        app.spec_input_text = "bad/spec".to_string();
        app.apply_intent(Intent::PreviewError {
            message: "git clone failed".to_string(),
        });
        assert!(!app.spec_input_active, "spec input should be inactive after preview error");
        assert!(app.active_preview.is_none(), "no preview should remain after error");
        assert_eq!(
            app.error.as_deref(),
            Some("git clone failed"),
            "error should be surfaced inline"
        );
        assert!(app.pending_action.is_none(), "no action should be pending after error");
    }

    #[test]
    fn cancel_action_drops_active_preview() {
        // spec: TUI-30 - declining (CancelAction while modal is visible) discards the preview
        // (active_preview = None -> SourcePreview::drop -> temp dir removed).
        let mut app = App::new(String::new(), None, None);
        // Simulate a live preview (SourcePreview::Drop removes temp dir, but since
        // we can't create a real temp clone in a unit test we just check state).
        app.pending_action = Some(PendingAction {
            kind: ActionKind::Meld { spec: "some/repo".to_string() },
            description: "Meld some/repo?".to_string(),
        });
        app.modal_visible = true;
        // Set a dummy (no-op) active_preview indication - can't construct SourcePreview
        // directly in a unit test, so we just verify the field gets cleared.
        app.apply_intent(Intent::CancelAction);
        assert!(app.active_preview.is_none(), "active_preview must be cleared on decline");
        assert!(app.pending_action.is_none(), "pending action must be cleared on decline");
        assert!(!app.modal_visible);
    }

    // --- TUI-31: suggested sources in the Available tree ---

    #[test]
    fn expand_on_suggested_source_sets_pending_preview_spec() {
        // spec: TUI-31 - expanding a SuggestedSource node queues a preview request.
        use crate::tui::tree::TreeNode;
        let mut app = App::new(String::new(), None, None);

        // Build a snapshot with no items but inject a suggestion manually.
        let mut snap = make_snapshot();
        snap.suggestions = vec![crate::tui::preview::RegistrySuggestion {
            spec: "/tmp/some-repo".to_string(),
            name: "some-repo".to_string(),
            url: "/tmp/some-repo".to_string(),
            alias: None,
        }];
        app.apply_snapshot(snap);

        // Find the SuggestedSource node in visible.
        let idx = app.visible.iter().position(|n| {
            matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "some-repo")
        });
        if let Some(idx) = idx {
            app.selected = idx;
            app.apply_intent(Intent::Expand);
            assert_eq!(
                app.pending_preview_spec.as_deref(),
                Some("/tmp/some-repo"),
                "expanding a SuggestedSource should queue a preview request"
            );
        } else {
            // If the node is not visible (e.g. it's filtered), the test is inconclusive.
            // Build the tree manually to verify the suggestion node is built.
            let nodes = crate::tui::tree::build_tree(
                app.last_snapshot.as_ref().unwrap(),
                "",
                None,
                None,
                false,
                false,
            );
            let flat = crate::tui::tree::flatten_tree(&nodes, &std::collections::HashSet::new());
            let has_sug = flat.iter().any(|n| {
                matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "some-repo")
            });
            assert!(has_sug, "SuggestedSource node should appear in the Available tree: {:?}",
                flat.iter().map(|n| &n.label).collect::<Vec<_>>());
        }
    }

    #[test]
    fn suggestion_node_appears_in_available_tree() {
        // spec: TUI-31 - RegistrySuggestion from snapshot is shown as a SuggestedSource
        // node in the Available group.
        use crate::tui::tree::{TreeNode, build_tree, flatten_tree};
        use std::collections::HashSet;

        let mut snap = make_snapshot();
        snap.suggestions = vec![crate::tui::preview::RegistrySuggestion {
            spec: "github.com/owner/suggested".to_string(),
            name: "suggested".to_string(),
            url: "https://github.com/owner/suggested".to_string(),
            alias: None,
        }];

        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());

        let has_sug = flat.iter().any(|n| {
            matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "suggested")
        });
        assert!(has_sug, "SuggestedSource should appear in Available tree: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>());
    }

    // --- TUI-23: lobe management state transitions ---

    #[test]
    fn action_lobes_opens_lobes_modal() {
        // spec: TUI-23 CLI-111 - pressing the lobes key opens the lobes modal,
        // making the current list of configured lobes visible.
        let mut app = App::new(String::new(), None, None);
        let mut snap = make_snapshot();
        snap.lobes = vec!["/home/user/.custom-ai".to_string()];
        app.apply_snapshot(snap);
        assert!(!app.lobes_modal_visible, "modal should be closed initially");
        app.apply_intent(Intent::ActionLobes);
        assert!(app.lobes_modal_visible, "ActionLobes must open the lobes modal");
        assert_eq!(app.lobes, vec!["/home/user/.custom-ai"]);
    }

    #[test]
    fn apply_snapshot_syncs_lobes_list() {
        // spec: TUI-23 CLI-111 - the lobes list in App tracks the snapshot's lobes
        // field so the modal always shows the current configured homes.
        let mut app = App::new(String::new(), None, None);
        let mut snap = make_snapshot();
        snap.lobes = vec!["~/.claude".to_string(), "~/.other".to_string()];
        app.apply_snapshot(snap);
        assert_eq!(app.lobes, vec!["~/.claude", "~/.other"]);

        // A subsequent snapshot with a different list replaces it.
        let mut snap2 = make_snapshot();
        snap2.generation = 2;
        snap2.lobes = vec!["~/.claude".to_string()];
        app.apply_snapshot(snap2);
        assert_eq!(app.lobes, vec!["~/.claude"]);
    }

    #[test]
    fn action_lobe_add_activates_input_when_modal_open() {
        // spec: TUI-23 CLI-112 - ActionLobeAdd opens the path-input box when the
        // lobes modal is open.
        let mut app = App::new(String::new(), None, None);
        app.lobes_modal_visible = true;
        assert!(!app.lobe_input_active);
        app.apply_intent(Intent::ActionLobeAdd);
        assert!(app.lobe_input_active, "ActionLobeAdd must activate lobe-path input");
        assert!(app.lobe_input_text.is_empty(), "input text starts empty");
    }

    #[test]
    fn action_lobe_add_noop_when_modal_closed() {
        // spec: TUI-23 - ActionLobeAdd is a no-op when the lobes modal is not open.
        let mut app = App::new(String::new(), None, None);
        app.lobes_modal_visible = false;
        app.apply_intent(Intent::ActionLobeAdd);
        assert!(!app.lobe_input_active, "ActionLobeAdd must not activate input when modal is closed");
    }

    #[test]
    fn lobe_input_char_appends_when_active() {
        // spec: TUI-23 CLI-112 - characters are routed to the lobe-input buffer.
        let mut app = App::new(String::new(), None, None);
        app.lobe_input_active = true;
        app.apply_intent(Intent::LobeInputChar('/'));
        app.apply_intent(Intent::LobeInputChar('h'));
        app.apply_intent(Intent::LobeInputChar('o'));
        assert_eq!(app.lobe_input_text, "/ho");
    }

    #[test]
    fn lobe_input_backspace_removes_last_char() {
        // spec: TUI-23 CLI-112
        let mut app = App::new(String::new(), None, None);
        app.lobe_input_active = true;
        app.lobe_input_text = "/home/me".to_string();
        app.apply_intent(Intent::LobeInputBackspace);
        assert_eq!(app.lobe_input_text, "/home/m");
    }

    #[test]
    fn submit_lobe_add_with_path_sets_pending_action_and_modal() {
        // spec: TUI-23 CLI-112 - submitting a non-empty lobe path creates a LobeAdd
        // pending action and shows the confirm modal (goes through TUI-24 gating).
        let mut app = App::new(String::new(), None, None);
        app.lobe_input_active = true;
        app.lobe_input_text = "~/.other-ai".to_string();
        app.submit_lobe_add();
        assert!(!app.lobe_input_active, "input should be inactive after submit");
        assert!(app.lobe_input_text.is_empty(), "input text cleared after submit");
        assert!(!app.lobes_modal_visible, "lobes modal closed to show confirm modal");
        assert!(app.modal_visible, "confirm modal must be visible (TUI-24 gating)");
        let action = app.pending_action.as_ref().expect("pending action must be set");
        assert!(
            matches!(&action.kind, ActionKind::LobeAdd { path } if path == "~/.other-ai"),
            "pending action must be LobeAdd with the submitted path"
        );
    }

    #[test]
    fn submit_lobe_add_empty_path_cancels_back_to_lobes_modal() {
        // spec: TUI-23 CLI-112 - submitting an empty path is a no-op that returns
        // to the lobes list (does not create a pending action).
        let mut app = App::new(String::new(), None, None);
        app.lobe_input_active = true;
        app.lobe_input_text.clear();
        app.submit_lobe_add();
        assert!(!app.lobe_input_active);
        assert!(app.lobes_modal_visible, "empty submit returns to lobes modal");
        assert!(app.pending_action.is_none(), "no action set on empty path");
    }

    #[test]
    fn action_lobe_remove_sets_pending_action_for_selected_lobe() {
        // spec: TUI-23 CLI-113 - ActionLobeRemove on a non-empty lobes list creates
        // a LobeRemove pending action for the currently selected lobe.
        let mut app = App::new(String::new(), None, None);
        app.lobes = vec!["~/.claude".to_string(), "~/.other".to_string()];
        app.lobes_selected = 1;
        app.lobes_modal_visible = true;
        app.apply_intent(Intent::ActionLobeRemove);
        assert!(!app.lobes_modal_visible, "lobes modal closes when confirm modal opens");
        assert!(app.modal_visible, "confirm modal must be visible (TUI-24 gating)");
        let action = app.pending_action.as_ref().expect("pending action must be set");
        assert!(
            matches!(&action.kind, ActionKind::LobeRemove { path } if path == "~/.other"),
            "LobeRemove action must target the selected lobe"
        );
    }

    #[test]
    fn action_lobe_remove_noop_on_empty_list() {
        // spec: TUI-23 CLI-113 - ActionLobeRemove on an empty lobes list does nothing.
        let mut app = App::new(String::new(), None, None);
        app.lobes = vec![];
        app.lobes_modal_visible = true;
        app.apply_intent(Intent::ActionLobeRemove);
        assert!(app.pending_action.is_none(), "no action set when lobes list is empty");
        assert!(app.lobes_modal_visible, "modal stays open when nothing to remove");
    }

    #[test]
    fn cancel_while_lobe_input_returns_to_lobes_modal() {
        // spec: TUI-23 - Esc during lobe-path input cancels input and returns to
        // the lobes modal (not all the way back to the main view).
        let mut app = App::new(String::new(), None, None);
        app.lobe_input_active = true;
        app.lobe_input_text = "/partial".to_string();
        app.lobes_modal_visible = false; // simulating state during input
        app.apply_intent(Intent::CancelAction);
        assert!(!app.lobe_input_active, "input must be inactive after cancel");
        assert!(app.lobe_input_text.is_empty(), "input text cleared on cancel");
        assert!(app.lobes_modal_visible, "cancel during input returns to lobes modal");
    }

    #[test]
    fn cancel_while_lobes_modal_open_closes_modal() {
        // spec: TUI-23 - Esc/n while the lobes modal is open (not in input mode)
        // closes the modal and returns to the main view.
        let mut app = App::new(String::new(), None, None);
        app.lobes_modal_visible = true;
        app.lobe_input_active = false;
        app.apply_intent(Intent::CancelAction);
        assert!(!app.lobes_modal_visible, "cancel closes the lobes modal");
    }

    #[test]
    fn lobe_remove_confirm_gate_does_not_execute_immediately() {
        // spec: TUI-23 TUI-24 - LobeRemove must be gated by the confirm modal;
        // it must not execute without an explicit y confirmation.
        let mut app = App::new(String::new(), None, None);
        app.lobes = vec!["~/.x".to_string()];
        app.lobes_selected = 0;
        app.lobes_modal_visible = true;
        app.apply_intent(Intent::ActionLobeRemove);
        // Action is now pending but NOT yet taken (not executed).
        assert!(app.pending_action.is_some(), "LobeRemove waits in pending_action");
        // Confirming yields the action for execution.
        let taken = app.take_pending_action().expect("confirm yields the action");
        assert!(
            matches!(taken.kind, ActionKind::LobeRemove { path } if path == "~/.x"),
            "the LobeRemove action must survive through confirmation"
        );
    }

    #[test]
    fn lobe_select_up_and_down_navigate_list() {
        // spec: TUI-23 - j/k (LobeSelectDown/Up) navigate the lobes list.
        let mut app = App::new(String::new(), None, None);
        app.lobes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        app.lobes_selected = 1;
        app.apply_intent(Intent::LobeSelectUp);
        assert_eq!(app.lobes_selected, 0);
        app.apply_intent(Intent::LobeSelectUp); // already at top
        assert_eq!(app.lobes_selected, 0);
        app.apply_intent(Intent::LobeSelectDown);
        assert_eq!(app.lobes_selected, 1);
        app.apply_intent(Intent::LobeSelectDown);
        assert_eq!(app.lobes_selected, 2);
        app.apply_intent(Intent::LobeSelectDown); // already at bottom
        assert_eq!(app.lobes_selected, 2);
    }
}
