//! App state model for the interactive TUI.
//!
//! Pure state; no I/O, no ratatui. All state transitions are through methods so
//! the logic is unit-testable without a terminal.

use crate::error::ItemKind;
use crate::tui::data::Snapshot;
use crate::tui::event::Intent;
use crate::tui::preview::SourcePreview;
use crate::tui::tree::{TreeNode, flatten_tree, is_auto_expanded};

/// A pending mutating action waiting for confirmation.
// spec: TUI-24
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub kind: ActionKind,
    /// Human-readable description shown in the confirm dialog.
    pub description: String,
    /// The dependency tree to show in the confirm modal (DEP-40). Only set for a
    /// Learn action whose closure adds dependencies beyond the explicit
    /// selection; `None` for every other action and for a Learn that pulls in
    /// nothing extra (the confirm stays as before in that case).
    // spec: DEP-40
    pub dep_tree: Option<String>,
}

impl PendingAction {
    /// Construct a pending action with no dependency tree (the common case for
    /// every non-Learn action and for a Learn that adds no dependencies).
    pub fn new(kind: ActionKind, description: String) -> Self {
        PendingAction {
            kind,
            description,
            dep_tree: None,
        }
    }
}

/// Build the source-qualified learn ref the TUI/CLI use for an Available item:
/// `{source}#{item_key}` when a source is recorded, else the bare `item_key`.
/// This mirrors the qualification `action::execute` applies for `ActionKind::Learn`
/// so the previewed tree (DEP-40) matches what confirming installs (DEP-41).
// spec: DEP-40
pub fn learn_ref(item_key: &str, source: &str) -> String {
    if source.is_empty() {
        item_key.to_string()
    } else {
        format!("{source}#{item_key}")
    }
}

/// Detail lines for an item dialog (TUI-26): kind and source always, the commit
/// when installed, and the description (if any) after a blank separator line.
fn item_detail(
    kind: ItemKind,
    source: &str,
    commit: Option<&str>,
    description: Option<&str>,
) -> Vec<String> {
    let mut d = vec![
        format!("kind:   {}", kind.as_str()),
        format!("source: {source}"),
    ];
    if let Some(c) = commit {
        d.push(format!("commit: {}", c.chars().take(8).collect::<String>()));
    }
    if let Some(s) = description {
        d.push(String::new());
        d.push(s.to_string());
    }
    d
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
    /// Upgrade pending items (TUI-22).
    Upgrade,
    /// Add an agent home (lobe) via `config lobes add` (TUI-23, CLI-112).
    // spec: TUI-23
    LobeAdd { path: String },
    /// Remove an agent home (lobe) via `config lobes remove` (TUI-23, CLI-113).
    // spec: TUI-23
    LobeRemove { path: String },
}

/// A details-and-actions dialog, opened with Enter on a source or item (TUI-26).
/// It describes the focused node and offers the actions valid for it; choosing
/// one runs it through the normal confirm-and-execute path (TUI-24).
#[derive(Debug, Clone)]
pub struct Dialog {
    /// The node being acted on (its name).
    pub title: String,
    /// Detail lines describing the node.
    pub detail: Vec<String>,
    /// Offered actions, in order.
    pub actions: Vec<DialogAction>,
    /// Index of the highlighted action.
    pub selected: usize,
}

/// One selectable action in a details dialog (TUI-26).
#[derive(Debug, Clone)]
pub struct DialogAction {
    /// Menu label shown to the user.
    pub label: String,
    /// The action run when chosen.
    pub kind: ActionKind,
    /// The confirm-modal description for the chosen action.
    pub description: String,
    /// For an Install action, the learn-ref whose dependency closure the event
    /// loop previews (DEP-40); None for every other action.
    pub learn_ref: Option<String>,
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
    /// Expanded node IDs (for non-auto-expanded nodes: InstalledItem,
    /// AvailableItem, SuggestedSource).
    pub expanded: std::collections::HashSet<String>,
    /// Collapsed auto-expanded node IDs (for Source and KindBucket nodes).
    /// Auto-expanded nodes are shown by default; inserting an ID here hides
    /// their children until explicitly re-expanded.
    // spec: TUI-11
    pub collapsed: std::collections::HashSet<String>,
    /// The last snapshot we applied.
    pub last_snapshot: Option<Snapshot>,

    // --- groups ---
    pub installed_collapsed: bool,
    pub available_collapsed: bool,

    // --- modal state ---
    pub pending_action: Option<PendingAction>,
    pub modal_visible: bool,
    /// The open details-and-actions dialog (Enter on a source/item, TUI-26).
    // spec: TUI-26
    pub dialog: Option<Dialog>,
    /// Cached first-visible-row offset for the tree list, kept so the highlight
    /// stays within the middle two-thirds of the viewport (TUI-16). Updated by
    /// the render pass, which knows the actual viewport height.
    // spec: TUI-16
    pub scroll: std::cell::Cell<usize>,

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
    /// Set by `initiate_learn`; the event loop consumes this to call
    /// `commands::learn_preview` (I/O) and stash the resulting dependency tree
    /// onto the pending Learn action so the confirm modal can show it (DEP-40).
    // spec: DEP-40
    pub pending_learn_ref: Option<String>,

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
    /// True after one Ctrl-C: a second consecutive Ctrl-C force-exits from any
    /// mode (TUI-43). Any other key disarms it.
    pub ctrl_c_armed: bool,
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
            collapsed: std::collections::HashSet::new(),
            last_snapshot: None,
            installed_collapsed: false,
            available_collapsed: false,
            pending_action: None,
            modal_visible: false,
            dialog: None,
            scroll: std::cell::Cell::new(0),
            status: None,
            error: None,
            spec_input_active: false,
            spec_input_text: String::new(),
            active_preview: None,
            pending_preview_spec: None,
            pending_learn_ref: None,
            lobes_modal_visible: false,
            lobes: Vec::new(),
            lobes_selected: 0,
            lobe_input_active: false,
            lobe_input_text: String::new(),
            size: (80, 24),
            mutating: false,
            quit: false,
            search_focused: false,
            ctrl_c_armed: false,
        }
    }

    /// Apply a data snapshot, rebuilding the tree. Preserves selection and
    /// expansion state across refreshes (TUI-15). Also refreshes the lobes
    /// list for the lobes modal (TUI-23).
    // spec: TUI-15
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        // Any successful snapshot-applying refresh ends a mutation: clearing the
        // flag here is what re-arms the once-a-second poll after a successful
        // learn/forget/sync/upgrade/meld/lobe action (TUI-15). The mid-action
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
        let selected_id = self.visible.get(self.selected).map(|n| n.id.clone());

        let nodes = crate::tui::tree::build_tree(
            snap,
            &self.search,
            self.kind_filter,
            self.source_filter.as_deref(),
            self.installed_collapsed,
            self.available_collapsed,
        );
        self.visible = flatten_tree(&nodes, &self.expanded, &self.collapsed);

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
                    } else if is_auto_expanded(&node.node) {
                        // Source/KindBucket: expand by removing from collapsed set.
                        // InstalledGroup/AvailableGroup: toggle their boolean.
                        // spec: TUI-11
                        match node.node {
                            TreeNode::InstalledGroup => {
                                self.installed_collapsed = false;
                            }
                            TreeNode::AvailableGroup => {
                                self.available_collapsed = false;
                            }
                            _ => {
                                self.collapsed.remove(&node.id);
                            }
                        }
                        self.rebuild_tree();
                    } else if node.expandable {
                        self.expanded.insert(node.id.clone());
                        self.rebuild_tree();
                    }
                }
            }
            Intent::Collapse => {
                if let Some(node) = self.visible.get(self.selected).cloned() {
                    if is_auto_expanded(&node.node) {
                        // Auto-expanded nodes (Source/KindBucket/groups): collapse
                        // by inserting into collapsed set (or toggling boolean).
                        // spec: TUI-11
                        match node.node {
                            TreeNode::InstalledGroup => {
                                self.installed_collapsed = true;
                            }
                            TreeNode::AvailableGroup => {
                                self.available_collapsed = true;
                            }
                            _ => {
                                self.collapsed.insert(node.id.clone());
                            }
                        }
                        self.rebuild_tree();
                    } else if self.expanded.remove(&node.id) {
                        self.rebuild_tree();
                    } else if node.depth > 0 {
                        // Jump to parent (for non-auto nodes already collapsed).
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
                    } else if is_auto_expanded(&node.node) {
                        // Auto-expanded nodes toggle their respective set/boolean.
                        // spec: TUI-11
                        match node.node {
                            TreeNode::InstalledGroup => {
                                self.installed_collapsed = !self.installed_collapsed;
                            }
                            TreeNode::AvailableGroup => {
                                self.available_collapsed = !self.available_collapsed;
                            }
                            _ => {
                                if self.collapsed.contains(&node.id) {
                                    self.collapsed.remove(&node.id);
                                } else {
                                    self.collapsed.insert(node.id.clone());
                                }
                            }
                        }
                        self.rebuild_tree();
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
            Intent::OpenDialog => {
                self.open_dialog();
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
            Intent::ActionUpgrade => {
                self.initiate_upgrade();
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
                    // Drop any queued learn-preview request: declining installs
                    // nothing (DEP-41), so the closure tree is never fetched.
                    // spec: DEP-41
                    self.pending_learn_ref = None;
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
                self.pending_action = Some(PendingAction::new(
                    ActionKind::Meld { spec: spec.clone() },
                    desc,
                ));
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
                    self.pending_action =
                        Some(PendingAction::new(ActionKind::LobeRemove { path }, desc));
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

    /// Initiate an install action for the selected node. On a single available
    /// item it installs that item; on a Source it installs every available item
    /// from that source (`<source>#*`); on a kind bucket every item of that kind
    /// (`<source>#<kind>:*`); on the Available group everything available (`*`).
    /// The glob goes through the same learn flow (closure preview + confirm), and
    /// `learn` skips anything already installed, so this is the interactive form
    /// of `learn '<source>#*'` (TUI-20).
    // spec: TUI-20
    fn initiate_learn(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        // Map the selected node to a learn selection: (item_key, source, prompt).
        let (item_key, source, desc) = match &node.node {
            TreeNode::AvailableItem(item) => (
                item.key.clone(),
                item.source.clone(),
                format!("Install {} from {}?", item.key, item.source),
            ),
            TreeNode::Source(src) => (
                "*".to_string(),
                src.name.clone(),
                format!("Install all available items from {}?", src.name),
            ),
            TreeNode::KindBucket { source, kind } => (
                format!("{kind}:*"),
                source.clone(),
                format!("Install all {kind} items from {source}?"),
            ),
            TreeNode::AvailableGroup => (
                "*".to_string(),
                String::new(),
                "Install all available items?".to_string(),
            ),
            // Nothing to install for installed items/groups or a suggested source.
            _ => return,
        };
        // Queue the dependency-tree preview (DEP-40): the event loop in mod.rs
        // consumes `pending_learn_ref`, calls `commands::learn_preview` (I/O, kept
        // out of this pure model), and stashes the tree onto this pending action
        // via `set_learn_dep_tree` before the modal is drawn.
        // spec: DEP-40
        self.pending_learn_ref = Some(learn_ref(&item_key, &source));
        self.pending_action = Some(PendingAction::new(
            ActionKind::Learn { item_key, source },
            desc,
        ));
        self.modal_visible = true;
    }

    /// Stash the dependency tree (computed by the I/O layer via
    /// `commands::learn_preview`) onto the pending Learn action so the confirm
    /// modal can render it (DEP-40). A no-op if there is no pending action.
    /// `tree` is the rendered closure tree; pass `None` when the closure adds no
    /// dependencies (the confirm then stays as before).
    // spec: DEP-40
    pub fn set_learn_dep_tree(&mut self, tree: Option<String>) {
        if let Some(pending) = self.pending_action.as_mut() {
            pending.dep_tree = tree;
        }
    }

    /// Initiate an uninstall action for the currently selected installed item.
    fn initiate_forget(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        match &node.node {
            TreeNode::InstalledItem(item) => {
                let desc = format!("Forget (uninstall) {}?", item.key);
                self.pending_action = Some(PendingAction::new(
                    ActionKind::Forget {
                        item_key: item.key.clone(),
                    },
                    desc,
                ));
                self.modal_visible = true;
            }
            // Forget on an unmanaged item removes the user's own lobe entry
            // (UNM-4/5). The key is the `kind:name` ref that commands::forget
            // resolves to the unmanaged item; the warning that it is not
            // mind-managed is printed by the executor.
            // spec: UNM-6
            TreeNode::UnmanagedItem(item) => {
                let desc = format!(
                    "Forget {} (NOT managed by mind: deletes your own file)?",
                    item.key
                );
                self.pending_action = Some(PendingAction::new(
                    ActionKind::Forget {
                        item_key: item.key.clone(),
                    },
                    desc,
                ));
                self.modal_visible = true;
            }
            _ => {}
        }
    }

    fn initiate_sync(&mut self) {
        self.pending_action = Some(PendingAction::new(
            ActionKind::Sync,
            "Sync all sources?".to_string(),
        ));
        self.modal_visible = true;
    }

    fn initiate_upgrade(&mut self) {
        self.pending_action = Some(PendingAction::new(
            ActionKind::Upgrade,
            "Upgrade all pending items?".to_string(),
        ));
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
        self.pending_action = Some(PendingAction::new(ActionKind::LobeAdd { path }, desc));
        self.modal_visible = true;
    }

    fn initiate_unmeld(&mut self) {
        let Some(node) = self.visible.get(self.selected) else {
            return;
        };
        if let TreeNode::Source(ref src) = node.node {
            // Destructive unmeld: unlinks the source AND uninstalls its items.
            // Routed through the confirm modal (TUI-24) before executing.
            // spec: TUI-21 TUI-24
            let desc = format!("Unmeld {} and uninstall its items?", src.name);
            self.pending_action = Some(PendingAction::new(
                ActionKind::Unmeld {
                    name: src.name.clone(),
                    forget: true,
                },
                desc,
            ));
            self.modal_visible = true;
        }
    }

    /// Open the details-and-actions dialog for the focused node (TUI-26). On a
    /// source or item it builds a dialog; on a group header, kind bucket, or
    /// suggested source there is no dialog, so Enter falls back to the existing
    /// toggle/preview behavior (TUI-11/TUI-31).
    // spec: TUI-26
    fn open_dialog(&mut self) {
        let Some(node) = self.visible.get(self.selected).cloned() else {
            return;
        };
        self.error = None;
        self.status = None;
        let dialog = match &node.node {
            TreeNode::AvailableItem(it) => Some(Dialog {
                title: it.name.clone(),
                detail: item_detail(it.kind, &it.source, None, it.description.as_deref()),
                actions: vec![DialogAction {
                    label: "Install".to_string(),
                    kind: ActionKind::Learn {
                        item_key: it.key.clone(),
                        source: it.source.clone(),
                    },
                    description: format!("Install {} from {}?", it.key, it.source),
                    learn_ref: Some(learn_ref(&it.key, &it.source)),
                }],
                selected: 0,
            }),
            TreeNode::InstalledItem(it) => Some(Dialog {
                title: it.name.clone(),
                detail: item_detail(
                    it.kind,
                    &it.source,
                    Some(&it.commit),
                    it.description.as_deref(),
                ),
                actions: vec![DialogAction {
                    label: "Forget".to_string(),
                    kind: ActionKind::Forget {
                        item_key: it.key.clone(),
                    },
                    description: format!("Forget (uninstall) {}?", it.key),
                    learn_ref: None,
                }],
                selected: 0,
            }),
            TreeNode::UnmanagedItem(it) => Some(Dialog {
                title: it.name.clone(),
                detail: vec![
                    format!("kind:   {}", it.kind.as_str()),
                    "not managed by mind".to_string(),
                ],
                actions: vec![DialogAction {
                    label: "Forget".to_string(),
                    kind: ActionKind::Forget {
                        item_key: it.key.clone(),
                    },
                    description: format!(
                        "Forget {} (NOT managed by mind: deletes your own file)?",
                        it.key
                    ),
                    learn_ref: None,
                }],
                selected: 0,
            }),
            TreeNode::Source(src) => Some(self.source_dialog(&src.name)),
            // Group headers, kind buckets, and suggested sources have no details
            // dialog: keep the existing toggle/preview on Enter.
            _ => None,
        };
        match dialog {
            Some(d) => self.dialog = Some(d),
            None => self.apply_intent(Intent::ToggleExpand),
        }
    }

    /// Build the source dialog: detail plus the actions valid for a source
    /// (install all available, uninstall all installed, unmeld), gated on whether
    /// there is anything to install/uninstall (TUI-26).
    fn source_dialog(&self, name: &str) -> Dialog {
        let (installed, available) = self.source_counts(name);
        let mut actions = Vec::new();
        if available > 0 {
            actions.push(DialogAction {
                label: format!("Install all available ({available})"),
                kind: ActionKind::Learn {
                    item_key: "*".to_string(),
                    source: name.to_string(),
                },
                description: format!("Install all available items from {name}?"),
                learn_ref: Some(learn_ref("*", name)),
            });
        }
        if installed > 0 {
            actions.push(DialogAction {
                label: format!("Uninstall all installed ({installed})"),
                kind: ActionKind::Forget {
                    item_key: format!("{name}#*"),
                },
                description: format!("Uninstall all {installed} item(s) from {name}?"),
                learn_ref: None,
            });
        }
        actions.push(DialogAction {
            label: "Unmeld".to_string(),
            kind: ActionKind::Unmeld {
                name: name.to_string(),
                forget: true,
            },
            description: format!("Unmeld {name} and uninstall its items?"),
            learn_ref: None,
        });
        Dialog {
            title: name.to_string(),
            detail: vec![
                format!("source:    {name}"),
                format!("installed: {installed}"),
                format!("available: {available}"),
            ],
            actions,
            selected: 0,
        }
    }

    /// Count a source's installed items and its not-yet-installed available items
    /// from the last snapshot (TUI-26).
    fn source_counts(&self, name: &str) -> (usize, usize) {
        let Some(snap) = &self.last_snapshot else {
            return (0, 0);
        };
        let installed = snap.installed.iter().filter(|i| i.source == name).count();
        let installed_keys: std::collections::HashSet<&String> =
            snap.installed.iter().map(|i| &i.key).collect();
        let available = snap
            .available
            .iter()
            .filter(|a| a.source == name && !installed_keys.contains(&a.key))
            .count();
        (installed, available)
    }

    /// Move the dialog's action highlight up (TUI-26). No-op when no dialog is open.
    pub fn dialog_up(&mut self) {
        if let Some(d) = self.dialog.as_mut()
            && d.selected > 0
        {
            d.selected -= 1;
        }
    }

    /// Move the dialog's action highlight down (TUI-26).
    pub fn dialog_down(&mut self) {
        if let Some(d) = self.dialog.as_mut()
            && d.selected + 1 < d.actions.len()
        {
            d.selected += 1;
        }
    }

    /// Dismiss the dialog without acting (TUI-26).
    pub fn close_dialog(&mut self) {
        self.dialog = None;
    }

    /// Run the highlighted dialog action: stash it as the pending action behind
    /// the confirm modal (TUI-24) and close the dialog. For an Install it also
    /// arms the dependency-closure preview (DEP-40) by setting `pending_learn_ref`,
    /// which the event loop consumes exactly as it does for the direct `i` action.
    // spec: TUI-26
    pub fn activate_dialog(&mut self) {
        let Some(d) = self.dialog.take() else {
            return;
        };
        let sel = d.selected;
        let Some(action) = d.actions.into_iter().nth(sel) else {
            return;
        };
        if let Some(lr) = &action.learn_ref {
            self.pending_learn_ref = Some(lr.clone());
        }
        self.pending_action = Some(PendingAction::new(action.kind, action.description));
        self.modal_visible = true;
    }

    /// User pressed Enter/Return to confirm the pending action.
    pub fn confirm_selected(&mut self) {
        if self.modal_visible {
            // The event loop will handle ConfirmAction
        } else if let Some(node) = self.visible.get(self.selected).cloned() {
            // Toggle expand on Enter, routing by node type (spec: TUI-11).
            if let TreeNode::SuggestedSource(ref sug) = node.node {
                // Enter on a SuggestedSource triggers a preview (TUI-31).
                // spec: TUI-31
                self.pending_preview_spec = Some(sug.spec.clone());
                self.set_status(format!("Previewing {}...", sug.name));
            } else if is_auto_expanded(&node.node) {
                // Auto-expanded nodes toggle their respective set/boolean.
                match node.node {
                    TreeNode::InstalledGroup => {
                        self.installed_collapsed = !self.installed_collapsed;
                    }
                    TreeNode::AvailableGroup => {
                        self.available_collapsed = !self.available_collapsed;
                    }
                    _ => {
                        if self.collapsed.contains(&node.id) {
                            self.collapsed.remove(&node.id);
                        } else {
                            self.collapsed.insert(node.id.clone());
                        }
                    }
                }
                self.rebuild_tree();
            } else if node.expandable {
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
    use crate::error::ItemKind;
    use crate::tui::data::{Snapshot, SnapshotAvailable, SnapshotInstalled};

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
            unmanaged: vec![],
            source_names: vec!["local/agents".to_string()],
            suggestions: vec![],
            lobes: vec![],
        }
    }

    #[test]
    fn new_app_seeds_from_cli_args() {
        // spec: TUI-2
        let app = App::new(
            "review".to_string(),
            Some(ItemKind::Skill),
            Some("src".to_string()),
        );
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
        assert!(
            !app.visible.is_empty(),
            "tree should be non-empty after snapshot"
        );
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
            assert_eq!(
                app.selected,
                initial + 1,
                "MoveDown should advance selection"
            );
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
        let matched: Vec<_> = app
            .visible
            .iter()
            .filter(|n| n.label.contains("zzznomatch"))
            .collect();
        assert!(
            matched.is_empty(),
            "non-matching items should be hidden: {:?}",
            app.visible.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        let _ = all_count;
    }

    #[test]
    fn expand_toggles_correct_set_for_node_type() {
        // spec: TUI-11 - Expand routes to the correct state set depending on the
        // node type: auto-expanded nodes (Source, KindBucket) use `collapsed`;
        // non-auto nodes (InstalledItem children) use `expanded`.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());

        // Find a Source or KindBucket node (auto-expanded).
        if let Some(idx) = app.visible.iter().position(|n| {
            matches!(
                &n.node,
                crate::tui::tree::TreeNode::Source(_)
                    | crate::tui::tree::TreeNode::KindBucket { .. }
            ) && n.expandable
        }) {
            app.selected = idx;
            let id = app.visible[idx].id.clone();
            // First collapse it so Expand has something to do.
            app.collapsed.insert(id.clone());
            app.rebuild_tree();
            if let Some(new_idx) = app.visible.iter().position(|n| n.id == id) {
                app.selected = new_idx;
                app.apply_intent(Intent::Expand);
                assert!(
                    !app.collapsed.contains(&id),
                    "Expand on an auto-expanded node must remove it from the collapsed set"
                );
                assert!(
                    !app.expanded.contains(&id),
                    "Expand on an auto-expanded node must NOT touch the expanded set"
                );
            }
        }
    }

    #[test]
    fn collapse_adds_to_collapsed_set_for_auto_expanded_nodes() {
        // spec: TUI-11 - Collapse on an auto-expanded node (Source, KindBucket)
        // inserts the id into the `collapsed` set, not the `expanded` set. The
        // children are then hidden on the next flatten.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());

        // Find a Source or KindBucket node.
        if let Some(idx) = app.visible.iter().position(|n| {
            matches!(
                &n.node,
                crate::tui::tree::TreeNode::Source(_)
                    | crate::tui::tree::TreeNode::KindBucket { .. }
            ) && n.expandable
        }) {
            app.selected = idx;
            let id = app.visible[idx].id.clone();
            app.apply_intent(Intent::Collapse);
            assert!(
                app.collapsed.contains(&id),
                "Collapse on an auto-expanded node must add it to the collapsed set"
            );
            assert!(
                !app.expanded.contains(&id),
                "Collapse on an auto-expanded node must NOT touch the expanded set"
            );
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
                assert_eq!(
                    app.selected, idx,
                    "selection should be preserved by ID after refresh"
                );
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

        assert_eq!(
            app.search, search_before,
            "search query must survive a refresh"
        );
        assert_eq!(
            app.expanded, expanded_before,
            "expansion set must survive a refresh"
        );
        // And the search filter is still in effect after the refresh.
        let has_unfiltered = app.visible.iter().any(|n| {
            matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(i) if i.name == "review")
        });
        assert!(
            !has_unfiltered,
            "the preserved 'dev' search must still hide non-matching items"
        );
    }

    #[test]
    fn pending_action_set_and_taken() {
        // spec: TUI-24
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction::new(
            ActionKind::Sync,
            "Sync all?".to_string(),
        ));
        app.modal_visible = true;
        let taken = app.take_pending_action();
        assert!(
            taken.is_some(),
            "take_pending_action should return the pending action"
        );
        assert!(
            app.pending_action.is_none(),
            "pending_action should be cleared after take"
        );
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
        app.pending_action = Some(PendingAction::new(
            ActionKind::Sync,
            "Sync all?".to_string(),
        ));
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
        app.pending_action = Some(PendingAction::new(ActionKind::Sync, "Sync?".to_string()));
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

        assert_eq!(
            app.selected, selected_before,
            "selection must be unchanged on equal generation"
        );
        assert_eq!(
            app.expanded, expanded_before,
            "expansion must be unchanged on equal generation"
        );
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
        assert!(
            !still_installed,
            "a higher generation must rebuild and reflect the new data"
        );
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
        assert!(
            app.modal_visible,
            "forget must open a confirm modal (destructive gating)"
        );
        let pending = app
            .pending_action
            .as_ref()
            .expect("forget must set a pending action");
        assert!(
            matches!(&pending.kind, ActionKind::Forget { item_key } if item_key == "skill:review"),
            "pending action should be Forget for the selected item"
        );
        // Declining (n) must clear it WITHOUT mutating.
        app.apply_intent(Intent::CancelAction);
        assert!(
            app.pending_action.is_none(),
            "decline clears the pending forget"
        );
        assert!(!app.modal_visible);
    }

    #[test]
    fn forget_on_unmanaged_item_sets_confirm_gated_forget() {
        // spec: UNM-6 - the forget action is available from the unmanaged group:
        // selecting an unmanaged item and forgetting it opens a confirm modal with
        // a Forget action keyed by `kind:name` (the ref commands::forget resolves
        // to the unmanaged item per UNM-4), so nothing mutates without confirmation.
        let mut app = App::new(String::new(), None, None);
        let mut snap = make_snapshot();
        snap.unmanaged = vec![crate::tui::data::SnapshotUnmanaged {
            key: "skill:hand-written".to_string(),
            name: "hand-written".to_string(),
            kind: ItemKind::Skill,
            paths: vec![std::path::PathBuf::from("/lobe/skills/hand-written")],
        }];
        app.apply_snapshot(snap);
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::UnmanagedItem(_)))
            .expect("unmanaged item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionForget);
        assert!(app.modal_visible, "forget must open a confirm modal");
        let pending = app
            .pending_action
            .as_ref()
            .expect("forget must set a pending action");
        assert!(
            matches!(&pending.kind, ActionKind::Forget { item_key } if item_key == "skill:hand-written"),
            "pending action should be Forget keyed by the unmanaged item's kind:name"
        );
        assert!(
            pending.description.contains("NOT managed by mind"),
            "the confirm prompt must warn the item is not mind-managed: {:?}",
            pending.description
        );
    }

    #[test]
    fn forget_on_unmanaged_collision_keys_unmanaged_node_with_warning() {
        // spec: UNM-6 - when an unmanaged item shares its kind:name with a managed
        // installed item, selecting the UNMANAGED node and forgetting must build a
        // Forget keyed by that kind:name AND carry the not-mind-managed warning
        // (the unmanaged branch), proving the action is driven by the selected
        // node, not by a name lookup that could hit the managed item first.
        let mut app = App::new(String::new(), None, None);
        let mut snap = make_snapshot(); // installs managed skill:review
        snap.unmanaged = vec![crate::tui::data::SnapshotUnmanaged {
            key: "skill:review".to_string(),
            name: "review".to_string(),
            kind: ItemKind::Skill,
            paths: vec![std::path::PathBuf::from("/lobe/skills/review")],
        }];
        app.apply_snapshot(snap);
        // Select the UNMANAGED review (not the managed one of the same key).
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::UnmanagedItem(_)))
            .expect("unmanaged item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionForget);
        let pending = app
            .pending_action
            .as_ref()
            .expect("forget must set a pending action");
        assert!(
            matches!(&pending.kind, ActionKind::Forget { item_key } if item_key == "skill:review"),
            "Forget must be keyed by the unmanaged item's kind:name"
        );
        assert!(
            pending.description.contains("NOT managed by mind"),
            "the unmanaged branch must warn it is not mind-managed even when a \
             managed item shares the key: {:?}",
            pending.description
        );
    }

    #[test]
    fn forget_on_managed_item_omits_unmanaged_warning() {
        // spec: UNM-6 - control for the collision test: forgetting a MANAGED item
        // uses the uninstall prompt and must NOT carry the unmanaged warning, so
        // the two branches are distinguishable by their prompt text.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)))
            .expect("installed item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionForget);
        let pending = app
            .pending_action
            .as_ref()
            .expect("forget must set a pending action");
        assert!(
            !pending.description.contains("NOT managed by mind"),
            "the managed forget prompt must not claim the item is unmanaged: {:?}",
            pending.description
        );
    }

    #[test]
    fn destructive_unmeld_forget_is_gated_by_confirmation() {
        // spec: TUI-24 - a destructive Unmeld{forget:true} is only handed to the
        // executor by take_pending_action (the confirm step). Until confirmed it
        // sits as a pending action and does not run.
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction::new(
            ActionKind::Unmeld {
                name: "local/x".to_string(),
                forget: true,
            },
            "Unmeld local/x --forget?".to_string(),
        ));
        app.modal_visible = true;
        // No confirmation yet: the action is still pending (not executed).
        assert!(
            app.pending_action.is_some(),
            "destructive action waits for confirm"
        );
        // Confirm: take_pending_action yields it (the executor then runs it).
        let taken = app
            .take_pending_action()
            .expect("confirm yields the action");
        assert!(
            matches!(taken.kind, ActionKind::Unmeld { forget: true, .. }),
            "the destructive forget flag must survive through confirmation"
        );
        assert!(
            app.mutating,
            "mutating flag is set once the action is taken"
        );
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
    fn learn_on_a_source_node_installs_all_from_that_source() {
        // spec: TUI-20 - pressing install (`i`) on a Source node queues a glob
        // learn of the whole source (`<source>#*`), so the user need not select
        // each item. The glob flows through the same confirm/closure path and
        // `learn` skips anything already installed.
        let mut app = App::new(String::new(), None, None);
        app.visible = vec![FlatNode {
            id: "s".into(),
            label: "agents".into(),
            depth: 1,
            expandable: true,
            expanded: false,
            node: TreeNode::Source(crate::tui::tree::SourceInfo {
                name: "local/agents".into(),
                installed: false,
            }),
        }];
        app.selected = 0;
        app.apply_intent(Intent::ActionLearn);
        assert_eq!(
            app.pending_learn_ref.as_deref(),
            Some("local/agents#*"),
            "a Source install must queue the whole-source glob"
        );
        match &app.pending_action.as_ref().expect("a pending Learn").kind {
            ActionKind::Learn { item_key, source } => {
                assert_eq!(item_key, "*");
                assert_eq!(source, "local/agents");
            }
            other => panic!("expected Learn, got {other:?}"),
        }
        assert!(app.modal_visible, "the confirm modal opens");
    }

    #[test]
    fn learn_on_the_available_group_installs_everything() {
        // spec: TUI-20 - installing on the Available group queues a learn of every
        // available item (`*`), with no single source.
        let mut app = App::new(String::new(), None, None);
        app.visible = vec![FlatNode {
            id: "avail".into(),
            label: "Available".into(),
            depth: 0,
            expandable: true,
            expanded: true,
            node: TreeNode::AvailableGroup,
        }];
        app.selected = 0;
        app.apply_intent(Intent::ActionLearn);
        assert_eq!(app.pending_learn_ref.as_deref(), Some("*"));
        match &app.pending_action.as_ref().expect("a pending Learn").kind {
            ActionKind::Learn { item_key, source } => {
                assert_eq!(item_key, "*");
                assert!(source.is_empty(), "a group install has no single source");
            }
            other => panic!("expected Learn, got {other:?}"),
        }
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
        assert!(
            app.spec_input_active,
            "ActionMeld should activate spec input"
        );
        assert!(
            app.spec_input_text.is_empty(),
            "spec input text should be empty initially"
        );
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
        assert!(
            !app.spec_input_active,
            "spec input should be inactive after cancel"
        );
        assert!(
            app.spec_input_text.is_empty(),
            "spec input text should be cleared on cancel"
        );
        assert!(
            app.pending_action.is_none(),
            "no action should be pending after cancel"
        );
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
        assert!(
            !app.spec_input_active,
            "spec input should be inactive after preview ready"
        );
        assert!(
            app.modal_visible,
            "confirm modal should be visible after preview ready"
        );
        let action = app
            .pending_action
            .as_ref()
            .expect("pending action should be set");
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
        assert!(
            !app.spec_input_active,
            "spec input should be inactive after preview error"
        );
        assert!(
            app.active_preview.is_none(),
            "no preview should remain after error"
        );
        assert_eq!(
            app.error.as_deref(),
            Some("git clone failed"),
            "error should be surfaced inline"
        );
        assert!(
            app.pending_action.is_none(),
            "no action should be pending after error"
        );
    }

    #[test]
    fn cancel_action_drops_active_preview() {
        // spec: TUI-30 - declining (CancelAction while modal is visible) discards the preview
        // (active_preview = None -> SourcePreview::drop -> temp dir removed).
        let mut app = App::new(String::new(), None, None);
        // Simulate a live preview (SourcePreview::Drop removes temp dir, but since
        // we can't create a real temp clone in a unit test we just check state).
        app.pending_action = Some(PendingAction::new(
            ActionKind::Meld {
                spec: "some/repo".to_string(),
            },
            "Meld some/repo?".to_string(),
        ));
        app.modal_visible = true;
        // Set a dummy (no-op) active_preview indication - can't construct SourcePreview
        // directly in a unit test, so we just verify the field gets cleared.
        app.apply_intent(Intent::CancelAction);
        assert!(
            app.active_preview.is_none(),
            "active_preview must be cleared on decline"
        );
        assert!(
            app.pending_action.is_none(),
            "pending action must be cleared on decline"
        );
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
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "some-repo"));
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
            let flat = crate::tui::tree::flatten_tree(
                &nodes,
                &std::collections::HashSet::new(),
                &std::collections::HashSet::new(),
            );
            let has_sug = flat
                .iter()
                .any(|n| matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "some-repo"));
            assert!(
                has_sug,
                "SuggestedSource node should appear in the Available tree: {:?}",
                flat.iter().map(|n| &n.label).collect::<Vec<_>>()
            );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());

        let has_sug = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::SuggestedSource(s) if s.name == "suggested"));
        assert!(
            has_sug,
            "SuggestedSource should appear in Available tree: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
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
        assert!(
            app.lobes_modal_visible,
            "ActionLobes must open the lobes modal"
        );
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
        assert!(
            app.lobe_input_active,
            "ActionLobeAdd must activate lobe-path input"
        );
        assert!(app.lobe_input_text.is_empty(), "input text starts empty");
    }

    #[test]
    fn action_lobe_add_noop_when_modal_closed() {
        // spec: TUI-23 - ActionLobeAdd is a no-op when the lobes modal is not open.
        let mut app = App::new(String::new(), None, None);
        app.lobes_modal_visible = false;
        app.apply_intent(Intent::ActionLobeAdd);
        assert!(
            !app.lobe_input_active,
            "ActionLobeAdd must not activate input when modal is closed"
        );
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
        assert!(
            !app.lobe_input_active,
            "input should be inactive after submit"
        );
        assert!(
            app.lobe_input_text.is_empty(),
            "input text cleared after submit"
        );
        assert!(
            !app.lobes_modal_visible,
            "lobes modal closed to show confirm modal"
        );
        assert!(
            app.modal_visible,
            "confirm modal must be visible (TUI-24 gating)"
        );
        let action = app
            .pending_action
            .as_ref()
            .expect("pending action must be set");
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
        assert!(
            app.lobes_modal_visible,
            "empty submit returns to lobes modal"
        );
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
        assert!(
            !app.lobes_modal_visible,
            "lobes modal closes when confirm modal opens"
        );
        assert!(
            app.modal_visible,
            "confirm modal must be visible (TUI-24 gating)"
        );
        let action = app
            .pending_action
            .as_ref()
            .expect("pending action must be set");
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
        assert!(
            app.pending_action.is_none(),
            "no action set when lobes list is empty"
        );
        assert!(
            app.lobes_modal_visible,
            "modal stays open when nothing to remove"
        );
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
        assert!(
            !app.lobe_input_active,
            "input must be inactive after cancel"
        );
        assert!(
            app.lobe_input_text.is_empty(),
            "input text cleared on cancel"
        );
        assert!(
            app.lobes_modal_visible,
            "cancel during input returns to lobes modal"
        );
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
        assert!(
            app.pending_action.is_some(),
            "LobeRemove waits in pending_action"
        );
        // Confirming yields the action for execution.
        let taken = app
            .take_pending_action()
            .expect("confirm yields the action");
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

    // --- DEP-40: dependency tree surfaced in the Learn confirm ---

    #[test]
    fn learn_ref_qualifies_with_source() {
        // spec: DEP-40 - the previewed ref must match what action::execute installs
        // (`{source}#{item_key}`), so the tree shown is the tree that gets applied.
        // A bare key is used only when no source is recorded.
        assert_eq!(
            learn_ref("skill:review", "local/agents"),
            "local/agents#skill:review"
        );
        assert_eq!(learn_ref("skill:review", ""), "skill:review");
    }

    #[test]
    fn initiate_learn_queues_a_learn_preview_request() {
        // spec: DEP-40 - choosing to install an Available item stages the Learn
        // confirm AND queues a learn-preview request (the source-qualified ref) for
        // the I/O layer to resolve the dependency tree. Without this the confirm
        // could never carry a tree.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::AvailableItem(_)))
            .expect("an available item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionLearn);

        assert!(app.modal_visible, "learn must open the confirm modal");
        assert!(
            matches!(
                app.pending_action.as_ref().map(|p| &p.kind),
                Some(ActionKind::Learn { .. })
            ),
            "a Learn action must be pending"
        );
        // The fixture's available item is `agent:dev` from `local/agents`.
        assert_eq!(
            app.pending_learn_ref.as_deref(),
            Some("local/agents#agent:dev"),
            "initiate_learn must queue the source-qualified learn ref for preview"
        );
        // The tree is not computed yet (that is the I/O layer's job).
        assert!(
            app.pending_action.as_ref().unwrap().dep_tree.is_none(),
            "the tree is filled in later by the I/O layer, not by the pure model"
        );
    }

    #[test]
    fn set_learn_dep_tree_attaches_tree_to_confirm() {
        // spec: DEP-40 - the I/O layer stashes the resolved dependency tree onto
        // the pending Learn action via set_learn_dep_tree; the confirm state then
        // carries it for the modal to render. A regression that drops the tree from
        // the confirm fails here.
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction::new(
            ActionKind::Learn {
                item_key: "skill:review".to_string(),
                source: "local/agents".to_string(),
            },
            "Install skill:review from local/agents?".to_string(),
        ));
        let tree = "review (selected)\n  dev (dependency)".to_string();
        app.set_learn_dep_tree(Some(tree.clone()));
        assert_eq!(
            app.pending_action.as_ref().unwrap().dep_tree.as_deref(),
            Some(tree.as_str()),
            "set_learn_dep_tree must attach the tree to the pending Learn action"
        );
    }

    #[test]
    fn decline_clears_queued_learn_preview() {
        // spec: DEP-41 - declining a Learn (CancelAction) installs nothing: the
        // queued learn-preview request and pending action are dropped.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::AvailableItem(_)))
            .expect("an available item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionLearn);
        assert!(app.pending_learn_ref.is_some(), "learn queued a preview");

        app.apply_intent(Intent::CancelAction);
        assert!(
            app.pending_action.is_none(),
            "decline clears the pending Learn"
        );
        assert!(
            app.pending_learn_ref.is_none(),
            "decline drops the queued learn-preview request (nothing installed)"
        );
    }

    #[test]
    fn pending_learn_ref_survives_a_non_confirm_intent_without_panic() {
        // spec: DEP-40 - the event loop consumes `pending_learn_ref` (.take()) only
        // AFTER routing the intent. If a different, non-confirm intent (e.g. a cursor
        // move) is applied while a learn-preview is still queued, the pure model must
        // not panic and must leave the queued ref intact for the I/O layer to drain
        // on the next pass. This pins that movement does not disturb the queued ref.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::AvailableItem(_)))
            .expect("an available item should be visible");
        app.selected = idx;
        app.apply_intent(Intent::ActionLearn);
        let queued = app
            .pending_learn_ref
            .clone()
            .expect("learn must queue a preview ref");

        // A movement intent fires before the event loop drains the queued ref.
        app.apply_intent(Intent::MoveUp);
        app.apply_intent(Intent::MoveDown);

        // State stays sane: the Learn is still pending and the queued ref is intact.
        assert!(
            matches!(
                app.pending_action.as_ref().map(|p| &p.kind),
                Some(ActionKind::Learn { .. })
            ),
            "the pending Learn must survive an interleaved movement intent"
        );
        assert_eq!(
            app.pending_learn_ref.as_deref(),
            Some(queued.as_str()),
            "the queued learn-preview ref must NOT be disturbed by a non-confirm intent"
        );
        assert!(app.modal_visible, "the confirm modal stays open");
    }

    #[test]
    fn set_learn_dep_tree_none_keeps_confirm_plain() {
        // spec: DEP-40 - the no-deps case: the I/O layer calls
        // set_learn_dep_tree(None) when the closure adds nothing. Even if a stale tree
        // was previously attached, passing None must clear it so the confirm stays a
        // plain prompt with no stray closure. A regression that ignored a None (only
        // ever set Some) would leave the stale tree and fail here.
        let mut app = App::new(String::new(), None, None);
        app.pending_action = Some(PendingAction::new(
            ActionKind::Learn {
                item_key: "skill:solo".to_string(),
                source: "local/agents".to_string(),
            },
            "Install skill:solo from local/agents?".to_string(),
        ));
        // First a tree is attached...
        app.set_learn_dep_tree(Some("- skill:solo [selected]".to_string()));
        assert!(
            app.pending_action.as_ref().unwrap().dep_tree.is_some(),
            "precondition: a tree was attached"
        );
        // ...then the no-deps verdict clears it.
        app.set_learn_dep_tree(None);
        assert!(
            app.pending_action.as_ref().unwrap().dep_tree.is_none(),
            "set_learn_dep_tree(None) must clear the tree so the confirm stays plain"
        );
    }

    // --- TUI-11: collapsed set used for Source/KindBucket; groups use boolean ---

    /// Helper: build an app with the standard snapshot and return the index of
    /// the Source node under Installed (id = "installed-source:local/agents").
    fn app_with_source_node() -> (App, usize) {
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::Source(_)))
            .expect("a Source node should be visible");
        (app, idx)
    }

    #[test]
    fn toggle_expand_on_source_removes_items_from_visible() {
        // spec: TUI-11 - ToggleExpand on a Source node collapses it, hiding its
        // descendant items from visible. A second ToggleExpand restores them.
        let (mut app, idx) = app_with_source_node();
        app.selected = idx;
        let source_id = app.visible[idx].id.clone();

        // Initially Source is expanded (auto-expanded): items should be visible.
        let items_before = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(items_before, "items should be visible before toggle");

        // First ToggleExpand: collapses the source.
        app.apply_intent(Intent::ToggleExpand);
        assert!(
            app.collapsed.contains(&source_id),
            "ToggleExpand on Source must insert id into collapsed set"
        );
        let items_after_collapse = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(
            !items_after_collapse,
            "items must be absent from visible after collapsing the Source"
        );

        // Second ToggleExpand: restores children.
        // (selection may have moved; re-locate the source node)
        if let Some(new_idx) = app.visible.iter().position(|n| n.id == source_id) {
            app.selected = new_idx;
        }
        app.apply_intent(Intent::ToggleExpand);
        assert!(
            !app.collapsed.contains(&source_id),
            "second ToggleExpand must remove Source from collapsed set"
        );
        let items_restored = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(
            items_restored,
            "items must reappear in visible after second ToggleExpand"
        );
    }

    #[test]
    fn collapse_then_expand_on_source_hides_and_restores_items() {
        // spec: TUI-11 - explicit Collapse/Expand intents on a Source node must
        // respectively hide and restore its descendant items.
        let (mut app, idx) = app_with_source_node();
        app.selected = idx;
        let source_id = app.visible[idx].id.clone();

        // Collapse.
        app.apply_intent(Intent::Collapse);
        assert!(
            app.collapsed.contains(&source_id),
            "Collapse must add Source to collapsed set"
        );
        let items_hidden = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(!items_hidden, "items must be hidden after Collapse");

        // Expand.
        if let Some(new_idx) = app.visible.iter().position(|n| n.id == source_id) {
            app.selected = new_idx;
        }
        app.apply_intent(Intent::Expand);
        assert!(
            !app.collapsed.contains(&source_id),
            "Expand must remove Source from collapsed set"
        );
        let items_restored = app
            .visible
            .iter()
            .any(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledItem(_)));
        assert!(items_restored, "items must reappear after Expand");
    }

    #[test]
    fn group_node_toggle_does_not_insert_into_collapsed() {
        // spec: TUI-11 - toggling an InstalledGroup or AvailableGroup must NOT
        // insert the group id into `self.collapsed` (that is reserved for
        // Source/KindBucket). Groups use the `installed_collapsed` /
        // `available_collapsed` booleans exclusively. Inserting a group id into
        // `collapsed` would be a double-toggle and break the model.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());

        // Toggle the InstalledGroup.
        let ig_idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::InstalledGroup))
            .expect("InstalledGroup must be visible");
        let ig_id = app.visible[ig_idx].id.clone();
        app.selected = ig_idx;
        app.apply_intent(Intent::ToggleExpand);
        assert!(
            app.installed_collapsed,
            "ToggleExpand on InstalledGroup must set installed_collapsed=true"
        );
        assert!(
            !app.collapsed.contains(&ig_id),
            "InstalledGroup id must NOT be inserted into the collapsed set"
        );

        // Toggle the AvailableGroup.
        let ag_idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, crate::tui::tree::TreeNode::AvailableGroup))
            .expect("AvailableGroup must be visible");
        let ag_id = app.visible[ag_idx].id.clone();
        app.selected = ag_idx;
        app.apply_intent(Intent::ToggleExpand);
        assert!(
            app.available_collapsed,
            "ToggleExpand on AvailableGroup must set available_collapsed=true"
        );
        assert!(
            !app.collapsed.contains(&ag_id),
            "AvailableGroup id must NOT be inserted into the collapsed set"
        );
    }

    // --- TUI-21 TUI-24: initiate_unmeld produces forget:true ---

    #[test]
    fn initiate_unmeld_produces_forget_true_and_opens_modal() {
        // spec: TUI-21 TUI-24 - pressing unmeld on a Source node queues a
        // destructive Unmeld{forget:true} action and shows the confirm modal.
        // The action must NOT execute until confirmed (gated by take_pending_action).
        let mut app = App::new(String::new(), None, None);
        // Wire a Source node directly into visible (simpler than seeding a full
        // snapshot since initiate_unmeld only inspects the selected node).
        app.visible = vec![FlatNode {
            id: "installed-source:local/agents".into(),
            label: "local/agents".into(),
            depth: 1,
            expandable: false,
            expanded: true,
            node: crate::tui::tree::TreeNode::Source(crate::tui::tree::SourceInfo {
                name: "local/agents".into(),
                installed: true,
            }),
        }];
        app.selected = 0;
        app.apply_intent(Intent::ActionUnmeld);

        assert!(
            app.modal_visible,
            "unmeld must open the confirm modal (TUI-24 gating)"
        );
        let pending = app
            .pending_action
            .as_ref()
            .expect("unmeld must set a pending action");
        assert!(
            matches!(
                &pending.kind,
                ActionKind::Unmeld { name, forget: true } if name == "local/agents"
            ),
            "pending action must be Unmeld{{forget:true}} for the selected source, got: {:?}",
            pending.kind
        );
        assert!(
            pending.description.contains("local/agents"),
            "confirm message must mention the source name"
        );
        // The action has not executed yet (gated by confirmation).
        assert!(
            !app.mutating,
            "mutating must not be set until take_pending_action is called"
        );
    }

    #[test]
    fn initiate_unmeld_forget_true_survives_confirmation() {
        // spec: TUI-21 TUI-24 - the forget:true flag must survive through the
        // confirm modal (take_pending_action). A bug that reset forget to false
        // or dropped it would leave installed items behind after unmeld.
        let mut app = App::new(String::new(), None, None);
        app.visible = vec![FlatNode {
            id: "installed-source:test-source".into(),
            label: "test-source".into(),
            depth: 1,
            expandable: false,
            expanded: true,
            node: crate::tui::tree::TreeNode::Source(crate::tui::tree::SourceInfo {
                name: "test-source".into(),
                installed: true,
            }),
        }];
        app.selected = 0;
        app.apply_intent(Intent::ActionUnmeld);

        let taken = app
            .take_pending_action()
            .expect("take_pending_action must yield the action");
        assert!(
            matches!(taken.kind, ActionKind::Unmeld { forget: true, .. }),
            "forget:true must survive take_pending_action: {:?}",
            taken.kind
        );
        assert!(app.mutating, "mutating flag set after take");
    }

    // --- TUI-26: the Enter details-and-actions dialog ---

    fn select_node(app: &mut App, pred: impl Fn(&TreeNode) -> bool) {
        let idx = app
            .visible
            .iter()
            .position(|n| pred(&n.node))
            .expect("a matching node must be visible");
        app.selected = idx;
    }

    #[test]
    fn enter_on_available_item_opens_install_dialog() {
        // spec: TUI-26 - Enter on an available item opens a dialog whose action is
        // Install for that item (not an expand-toggle).
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::AvailableItem(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a dialog must open");
        assert_eq!(d.actions.len(), 1);
        assert_eq!(d.actions[0].label, "Install");
        assert!(matches!(
            &d.actions[0].kind,
            ActionKind::Learn { item_key, .. } if item_key == "agent:dev"
        ));
        // An Install action carries the learn-ref so the closure preview fires.
        assert!(d.actions[0].learn_ref.is_some());
    }

    #[test]
    fn enter_on_installed_item_opens_forget_dialog() {
        // spec: TUI-26 - Enter on an installed item offers Forget.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::InstalledItem(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a dialog must open");
        assert_eq!(d.actions[0].label, "Forget");
        assert!(matches!(
            &d.actions[0].kind,
            ActionKind::Forget { item_key } if item_key == "skill:review"
        ));
    }

    #[test]
    fn enter_on_source_offers_bulk_actions_and_unmeld() {
        // spec: TUI-26 - a source dialog offers install-all, uninstall-all, and
        // unmeld. make_snapshot has one available and one installed item under
        // `local/agents`, so all three are present.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::Source(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a source dialog must open");
        let kinds: Vec<&ActionKind> = d.actions.iter().map(|a| &a.kind).collect();
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, ActionKind::Learn { item_key, .. } if item_key == "*")),
            "install-all action present: {kinds:?}"
        );
        assert!(
            kinds.iter().any(
                |k| matches!(k, ActionKind::Forget { item_key } if item_key == "local/agents#*")
            ),
            "uninstall-all action present: {kinds:?}"
        );
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, ActionKind::Unmeld { forget: true, .. })),
            "unmeld action present: {kinds:?}"
        );
    }

    #[test]
    fn enter_on_group_header_has_no_dialog_and_toggles() {
        // spec: TUI-26 - a group header has no details dialog; Enter falls back to
        // the toggle behavior (TUI-11), so no dialog opens and the group collapses.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::InstalledGroup));
        assert!(!app.installed_collapsed);
        app.apply_intent(Intent::OpenDialog);
        assert!(app.dialog.is_none(), "no dialog for a group header");
        assert!(app.installed_collapsed, "Enter toggled the group instead");
    }

    #[test]
    fn activate_dialog_sets_confirm_gated_pending_action() {
        // spec: TUI-26 - choosing a dialog action stashes it as the pending action
        // behind the confirm modal and closes the dialog; nothing runs yet.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::InstalledItem(_)));
        app.apply_intent(Intent::OpenDialog);
        app.activate_dialog();
        assert!(app.dialog.is_none(), "dialog closes on activate");
        assert!(app.modal_visible, "confirm modal opens");
        let pending = app.pending_action.as_ref().expect("a pending action");
        assert!(matches!(
            &pending.kind,
            ActionKind::Forget { item_key } if item_key == "skill:review"
        ));
    }

    #[test]
    fn activate_install_dialog_arms_learn_preview() {
        // spec: TUI-26 DEP-40 - activating an Install action arms the dependency
        // closure preview (pending_learn_ref) the event loop consumes.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::AvailableItem(_)));
        app.apply_intent(Intent::OpenDialog);
        app.activate_dialog();
        assert_eq!(
            app.pending_learn_ref.as_deref(),
            Some("local/agents#agent:dev"),
            "install must arm the closure preview"
        );
    }

    /// A snapshot with a source that has ONLY an installed item (nothing
    /// available): the source dialog must omit install-all.
    fn snapshot_installed_only() -> Snapshot {
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
            available: vec![],
            unmanaged: vec![],
            source_names: vec!["local/agents".to_string()],
            suggestions: vec![],
            lobes: vec![],
        }
    }

    /// A snapshot with a source that has ONLY an available item (nothing
    /// installed): the source dialog must omit uninstall-all.
    fn snapshot_available_only() -> Snapshot {
        Snapshot {
            generation: 1,
            installed: vec![],
            available: vec![SnapshotAvailable {
                key: "agent:dev".to_string(),
                name: "dev".to_string(),
                source: "local/agents".to_string(),
                kind: ItemKind::Agent,
                description: Some("Dev agent".to_string()),
                path: std::path::PathBuf::from("/fake/path"),
            }],
            unmanaged: vec![],
            source_names: vec!["local/agents".to_string()],
            suggestions: vec![],
            lobes: vec![],
        }
    }

    #[test]
    fn enter_on_unmanaged_item_opens_forget_dialog_with_warning() {
        // spec: TUI-26 UNM-5 - Enter on an unmanaged item opens a dialog whose only
        // action is Forget, keyed by the item's kind:name and carrying the
        // not-mind-managed warning in its confirm description (so the dialog path,
        // not just the direct `d` key, surfaces the warning).
        let mut app = App::new(String::new(), None, None);
        let mut snap = make_snapshot();
        snap.unmanaged = vec![crate::tui::data::SnapshotUnmanaged {
            key: "skill:hand-written".to_string(),
            name: "hand-written".to_string(),
            kind: ItemKind::Skill,
            paths: vec![std::path::PathBuf::from("/lobe/skills/hand-written")],
        }];
        app.apply_snapshot(snap);
        select_node(&mut app, |n| matches!(n, TreeNode::UnmanagedItem(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a dialog must open");
        assert_eq!(d.actions.len(), 1, "an unmanaged item offers only Forget");
        assert_eq!(d.actions[0].label, "Forget");
        assert!(matches!(
            &d.actions[0].kind,
            ActionKind::Forget { item_key } if item_key == "skill:hand-written"
        ));
        assert!(
            d.actions[0].description.contains("NOT managed by mind"),
            "the dialog's Forget must carry the not-mind-managed warning: {:?}",
            d.actions[0].description
        );
        // The detail block also marks the item as not mind-managed.
        assert!(
            d.detail.iter().any(|l| l.contains("not managed by mind")),
            "detail must note the item is not mind-managed: {:?}",
            d.detail
        );
        // An unmanaged Forget is not an install, so no closure preview is armed.
        assert!(
            d.actions[0].learn_ref.is_none(),
            "a Forget action carries no learn-ref"
        );
    }

    #[test]
    fn source_dialog_omits_install_all_when_nothing_available() {
        // spec: TUI-26 - "an action is omitted when it would do nothing": a source
        // with only installed items (nothing available) must NOT offer install-all,
        // but must still offer uninstall-all and unmeld.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(snapshot_installed_only());
        select_node(&mut app, |n| matches!(n, TreeNode::Source(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a source dialog must open");
        let kinds: Vec<&ActionKind> = d.actions.iter().map(|a| &a.kind).collect();
        assert!(
            !kinds
                .iter()
                .any(|k| matches!(k, ActionKind::Learn { item_key, .. } if item_key == "*")),
            "install-all must be omitted with nothing available: {kinds:?}"
        );
        assert!(
            kinds.iter().any(
                |k| matches!(k, ActionKind::Forget { item_key } if item_key == "local/agents#*")
            ),
            "uninstall-all must still be present: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| matches!(k, ActionKind::Unmeld { .. })),
            "unmeld is always present: {kinds:?}"
        );
    }

    #[test]
    fn source_dialog_omits_uninstall_all_when_nothing_installed() {
        // spec: TUI-26 - the mirror case: a source with only available items
        // (nothing installed) must NOT offer uninstall-all, but must still offer
        // install-all and unmeld.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(snapshot_available_only());
        select_node(&mut app, |n| matches!(n, TreeNode::Source(_)));
        app.apply_intent(Intent::OpenDialog);
        let d = app.dialog.as_ref().expect("a source dialog must open");
        let kinds: Vec<&ActionKind> = d.actions.iter().map(|a| &a.kind).collect();
        assert!(
            !kinds.iter().any(|k| matches!(k, ActionKind::Forget { .. })),
            "uninstall-all must be omitted with nothing installed: {kinds:?}"
        );
        assert!(
            kinds
                .iter()
                .any(|k| matches!(k, ActionKind::Learn { item_key, .. } if item_key == "*")),
            "install-all must still be present: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| matches!(k, ActionKind::Unmeld { .. })),
            "unmeld is always present: {kinds:?}"
        );
    }

    #[test]
    fn open_dialog_does_not_mutate_pending_action_or_modal() {
        // spec: TUI-26 - opening the dialog is non-committal: it must NOT set a
        // pending action, show the confirm modal, or arm a learn preview. Only
        // activate_dialog does that. A regression that armed the action on open
        // would mutate without the user choosing anything.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::AvailableItem(_)));
        app.apply_intent(Intent::OpenDialog);
        assert!(app.dialog.is_some(), "the dialog opened");
        assert!(
            app.pending_action.is_none(),
            "opening a dialog must not set a pending action"
        );
        assert!(
            !app.modal_visible,
            "opening a dialog must not show the confirm modal"
        );
        assert!(
            app.pending_learn_ref.is_none(),
            "opening a dialog must not arm the closure preview (only activate does)"
        );
    }

    #[test]
    fn close_dialog_clears_dialog_without_arming_an_action() {
        // spec: TUI-26 - Esc/q/n dismisses the dialog without acting: the dialog is
        // cleared and no pending action / confirm modal is left behind.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::AvailableItem(_)));
        app.apply_intent(Intent::OpenDialog);
        assert!(app.dialog.is_some(), "dialog open before close");
        app.close_dialog();
        assert!(app.dialog.is_none(), "close_dialog clears the dialog");
        assert!(
            app.pending_action.is_none(),
            "dismissing must not set a pending action"
        );
        assert!(
            !app.modal_visible,
            "dismissing must not show the confirm modal"
        );
        assert!(
            app.pending_learn_ref.is_none(),
            "dismissing must not arm a learn preview"
        );
    }

    #[test]
    fn enter_on_kind_bucket_has_no_dialog_and_toggles_bucket() {
        // spec: TUI-26 - the fallback case for a kind bucket (not a group header):
        // Enter opens no dialog and instead toggles that bucket's collapsed state,
        // mirroring TUI-11. Distinguishes the kind-bucket branch from the group
        // branch already covered by enter_on_group_header_*.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        let idx = app
            .visible
            .iter()
            .position(|n| matches!(&n.node, TreeNode::KindBucket { .. }))
            .expect("a kind bucket must be visible");
        app.selected = idx;
        let id = app.visible[idx].id.clone();
        assert!(
            !app.collapsed.contains(&id),
            "kind bucket starts expanded (not in collapsed set)"
        );
        app.apply_intent(Intent::OpenDialog);
        assert!(app.dialog.is_none(), "no dialog for a kind bucket");
        assert!(
            app.collapsed.contains(&id),
            "Enter on a kind bucket toggled it into the collapsed set"
        );
    }

    #[test]
    fn dialog_navigation_clamps_to_action_list() {
        // spec: TUI-26 - up/down move the highlighted action and clamp at the ends.
        let mut app = App::new(String::new(), None, None);
        app.apply_snapshot(make_snapshot());
        select_node(&mut app, |n| matches!(n, TreeNode::Source(_)));
        app.apply_intent(Intent::OpenDialog);
        let n = app.dialog.as_ref().unwrap().actions.len();
        assert!(n >= 2, "source dialog has multiple actions");
        app.dialog_up(); // already at top: no-op
        assert_eq!(app.dialog.as_ref().unwrap().selected, 0);
        for _ in 0..n + 2 {
            app.dialog_down();
        }
        assert_eq!(
            app.dialog.as_ref().unwrap().selected,
            n - 1,
            "down clamps at the last action"
        );
    }
}
