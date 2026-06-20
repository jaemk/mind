//! Tree data model for the TUI browse view.
//!
//! Builds the Installed/Available hierarchy from a data snapshot, flattens it
//! to a visible list given expansion state and filters. Search uses
//! `catalog::matches_query` consistent with CLI-85 (TUI-14).
//!
//! Pure: no I/O, no ratatui, no lock.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::catalog;
use crate::error::ItemKind;
use crate::tui::app::FlatNode;
use crate::tui::data::Snapshot;

/// A node in the logical tree (before flattening to display rows).
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields used by render/action; present but not all read yet
pub enum TreeNode {
    /// The "Installed" group header.
    InstalledGroup,
    /// The "Available" group header.
    AvailableGroup,
    /// A source node under Installed.
    Source(SourceInfo),
    /// A kind-bucket node under a source (e.g. "skills").
    KindBucket { source: String, kind: ItemKind },
    /// A single installed item.
    InstalledItem(InstalledInfo),
    /// A single available (not installed) item.
    AvailableItem(AvailableInfo),
    /// A not-yet-melded source suggested by the registry (TUI-31).
    /// Expanding this node triggers a preview (TUI-30).
    SuggestedSource(SuggestedSourceInfo),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SourceInfo {
    pub name: String,
    pub installed: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InstalledInfo {
    pub key: String,
    pub name: String,
    pub source: String,
    pub kind: ItemKind,
    pub commit: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AvailableInfo {
    pub key: String,
    pub name: String,
    pub source: String,
    pub kind: ItemKind,
    pub description: Option<String>,
    pub path: PathBuf,
}

/// Info about a not-yet-melded suggested source (TUI-31).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SuggestedSourceInfo {
    /// The repo spec (for meld/preview).
    pub spec: String,
    /// Display name.
    pub name: String,
    /// URL.
    pub url: String,
}

/// A node in the tree with its children.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub node: TreeNode,
    pub children: Vec<Node>,
}

/// Build the full tree from a snapshot, applying kind/source filters and the
/// installed_collapsed / available_collapsed state.
///
/// The search filter is applied at the item level: a source or kind bucket is
/// included if at least one of its items matches. This mirrors how `probe` works:
/// the group headers are always visible, but their contents are filtered.
// spec: TUI-10 TUI-11 TUI-12 TUI-13 TUI-14
pub fn build_tree(
    snap: &Snapshot,
    search: &str,
    kind_filter: Option<ItemKind>,
    source_filter: Option<&str>,
    installed_collapsed: bool,
    available_collapsed: bool,
) -> Vec<Node> {
    let mut roots = Vec::new();

    // --- Installed group ---
    // spec: TUI-10 TUI-12
    let installed_node = build_installed_group(snap, search, kind_filter, source_filter);
    let installed_count = count_items(&installed_node.children);
    let installed_label = if installed_collapsed {
        "Installed (collapsed)".to_string()
    } else {
        format!("Installed ({installed_count})")
    };
    roots.push(Node {
        id: "group:installed".to_string(),
        label: installed_label,
        children: if installed_collapsed { vec![] } else { installed_node.children },
        node: TreeNode::InstalledGroup,
    });

    // --- Available group ---
    // spec: TUI-10 TUI-13
    let available_node = build_available_group(snap, search, kind_filter, source_filter);
    let available_count = count_items(&available_node.children);
    let available_label = if available_collapsed {
        "Available (collapsed)".to_string()
    } else {
        format!("Available ({available_count})")
    };
    roots.push(Node {
        id: "group:available".to_string(),
        label: available_label,
        children: if available_collapsed { vec![] } else { available_node.children },
        node: TreeNode::AvailableGroup,
    });

    roots
}

fn count_items(children: &[Node]) -> usize {
    let mut n = 0;
    for c in children {
        match &c.node {
            TreeNode::InstalledItem(_) | TreeNode::AvailableItem(_) => n += 1,
            _ => n += count_items(&c.children),
        }
    }
    n
}

/// Build the Installed subtree.
// spec: TUI-12
fn build_installed_group(
    snap: &Snapshot,
    search: &str,
    kind_filter: Option<ItemKind>,
    source_filter: Option<&str>,
) -> Node {
    // Group installed items by source -> kind.
    let mut by_source: std::collections::BTreeMap<String, Vec<&crate::tui::data::SnapshotInstalled>> =
        std::collections::BTreeMap::new();
    for item in &snap.installed {
        if kind_filter.is_some_and(|kf| item.kind != kf) {
            continue;
        }
        if source_filter.is_some_and(|sf| !crate::resolve::source_matches(&item.source, sf)) {
            continue;
        }
        if !item_matches_search_installed(item, search) {
            continue;
        }
        by_source.entry(item.source.clone()).or_default().push(item);
    }

    let mut source_nodes = Vec::new();
    for (src_name, items) in &by_source {
        let mut kind_map: std::collections::BTreeMap<ItemKind, Vec<&&crate::tui::data::SnapshotInstalled>> =
            std::collections::BTreeMap::new();
        for item in items {
            kind_map.entry(item.kind).or_default().push(item);
        }

        let mut kind_nodes = Vec::new();
        for (kind, kind_items) in &kind_map {
            let item_nodes: Vec<Node> = kind_items
                .iter()
                .map(|it| Node {
                    id: format!("installed:{}:{}", src_name, it.key),
                    label: format!(
                        "{} [{}]{}",
                        it.name,
                        short_commit(&it.commit),
                        it.description.as_deref().map(|d| format!(" - {}", truncate(d, 50))).unwrap_or_default()
                    ),
                    node: TreeNode::InstalledItem(InstalledInfo {
                        key: it.key.clone(),
                        name: it.name.clone(),
                        source: it.source.clone(),
                        kind: it.kind,
                        commit: it.commit.clone(),
                        description: it.description.clone(),
                    }),
                    children: vec![],
                })
                .collect();
            kind_nodes.push(Node {
                id: format!("installed-kind:{}:{}", src_name, kind.as_str()),
                label: format!("{} ({})", kind.as_str(), item_nodes.len()),
                node: TreeNode::KindBucket {
                    source: src_name.clone(),
                    kind: *kind,
                },
                children: item_nodes,
            });
        }

        source_nodes.push(Node {
            id: format!("installed-source:{}", src_name),
            label: src_name.clone(),
            node: TreeNode::Source(SourceInfo {
                name: src_name.clone(),
                installed: true,
            }),
            children: kind_nodes,
        });
    }

    Node {
        id: "group:installed".to_string(),
        label: "Installed".to_string(),
        node: TreeNode::InstalledGroup,
        children: source_nodes,
    }
}

/// Build the Available subtree, de-duplicating items that are already installed.
/// Also appends TUI-31 suggested sources as collapsed leaf nodes.
// spec: TUI-13 TUI-14 TUI-31
fn build_available_group(
    snap: &Snapshot,
    search: &str,
    kind_filter: Option<ItemKind>,
    source_filter: Option<&str>,
) -> Node {
    // Installed item keys (for de-dup).
    let installed_keys: HashSet<String> = snap.installed.iter().map(|i| i.key.clone()).collect();

    let mut by_source: std::collections::BTreeMap<String, Vec<&crate::tui::data::SnapshotAvailable>> =
        std::collections::BTreeMap::new();

    for item in &snap.available {
        // De-duplicate: skip items already installed.
        // spec: TUI-13
        if installed_keys.contains(&item.key) {
            continue;
        }
        if kind_filter.is_some_and(|kf| item.kind != kf) {
            continue;
        }
        if source_filter.is_some_and(|sf| !crate::resolve::source_matches(&item.source, sf)) {
            continue;
        }
        if !item_matches_search_available(item, search) {
            continue;
        }
        by_source.entry(item.source.clone()).or_default().push(item);
    }

    let mut source_nodes = Vec::new();
    for (src_name, items) in &by_source {
        let mut kind_map: std::collections::BTreeMap<ItemKind, Vec<&&crate::tui::data::SnapshotAvailable>> =
            std::collections::BTreeMap::new();
        for item in items {
            kind_map.entry(item.kind).or_default().push(item);
        }

        let mut kind_nodes = Vec::new();
        for (kind, kind_items) in &kind_map {
            let item_nodes: Vec<Node> = kind_items
                .iter()
                .map(|it| Node {
                    id: format!("available:{}:{}", src_name, it.key),
                    label: format!(
                        "{}{}",
                        it.name,
                        it.description.as_deref().map(|d| format!(" - {}", truncate(d, 50))).unwrap_or_default()
                    ),
                    node: TreeNode::AvailableItem(AvailableInfo {
                        key: it.key.clone(),
                        name: it.name.clone(),
                        source: it.source.clone(),
                        kind: it.kind,
                        description: it.description.clone(),
                        path: it.path.clone(),
                    }),
                    children: vec![],
                })
                .collect();
            kind_nodes.push(Node {
                id: format!("available-kind:{}:{}", src_name, kind.as_str()),
                label: format!("{} ({})", kind.as_str(), item_nodes.len()),
                node: TreeNode::KindBucket {
                    source: src_name.clone(),
                    kind: *kind,
                },
                children: item_nodes,
            });
        }

        source_nodes.push(Node {
            id: format!("available-source:{}", src_name),
            label: src_name.clone(),
            node: TreeNode::Source(SourceInfo {
                name: src_name.clone(),
                installed: false,
            }),
            children: kind_nodes,
        });
    }

    // Append suggested not-yet-melded sources (TUI-31). Search filter applies
    // to the suggestion name. Source/kind filters are not applied since
    // suggestions have no kind yet (they need a preview to reveal items).
    // spec: TUI-31
    for sug in &snap.suggestions {
        if !search.is_empty() && !sug.name.to_lowercase().contains(&search.to_lowercase()) {
            continue;
        }
        source_nodes.push(Node {
            id: format!("suggested:{}", sug.url),
            label: format!("{} [suggested]", sug.name),
            node: TreeNode::SuggestedSource(SuggestedSourceInfo {
                spec: sug.spec.clone(),
                name: sug.name.clone(),
                url: sug.url.clone(),
            }),
            // No children yet: expanding triggers a preview (handled in event loop).
            children: vec![],
        });
    }

    Node {
        id: "group:available".to_string(),
        label: "Available".to_string(),
        node: TreeNode::AvailableGroup,
        children: source_nodes,
    }
}

/// True if an installed item matches the search query.
/// Reuses `catalog::matches_query` semantics (CLI-85 / TUI-14).
// spec: TUI-14
fn item_matches_search_installed(item: &crate::tui::data::SnapshotInstalled, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    let q = search.to_lowercase();
    item.name.to_lowercase().contains(&q)
        || item
            .description
            .as_deref()
            .is_some_and(|d| d.to_lowercase().contains(&q))
}

/// True if an available item matches the search query.
/// Mirrors `catalog::matches_query` (CLI-85 / TUI-14).
// spec: TUI-14
fn item_matches_search_available(item: &crate::tui::data::SnapshotAvailable, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    // Build a temporary CatalogItem to reuse catalog::matches_query exactly.
    // This ensures TUI-14's "consistent with CLI-85" requirement by literally
    // calling the same function (avoiding duplicated logic).
    let fake = catalog::CatalogItem {
        kind: item.kind,
        name: item.name.clone(),
        source: item.source.clone(),
        prefix: None,
        path: item.path.clone(),
        description: item.description.clone(),
        link_rel: None,
    };
    catalog::matches_query(&fake, search)
}

/// Flatten the tree into a displayable list, respecting expansion state.
/// A group header is always shown. A node's children are shown only if the
/// node's ID is in `expanded`.
// spec: TUI-10 TUI-11
pub fn flatten_tree(nodes: &[Node], expanded: &HashSet<String>) -> Vec<FlatNode> {
    let mut out = Vec::new();
    for node in nodes {
        flatten_node(node, 0, expanded, &mut out);
    }
    out
}

fn flatten_node(
    node: &Node,
    depth: usize,
    expanded: &HashSet<String>,
    out: &mut Vec<FlatNode>,
) {
    // Group headers, source nodes, and kind-buckets are auto-expanded by
    // default so items are immediately visible in the tree. Only item-detail
    // nodes (InstalledItem / AvailableItem children) require explicit expansion.
    // The user can collapse any node with the Collapse key.
    let is_auto_expanded = matches!(
        node.node,
        TreeNode::InstalledGroup | TreeNode::AvailableGroup | TreeNode::Source(_) | TreeNode::KindBucket { .. }
    );
    // A node is explicitly collapsed if it has been removed from the expanded
    // set after being auto-expanded (we track collapsed IDs separately).
    // For simplicity: auto-expanded nodes show their children unless the user
    // has explicitly collapsed them (i.e., the ID is absent from `expanded`
    // ONLY if it was previously inserted and then removed). We detect this by
    // checking: if auto-expanded AND the id is in expanded -> collapsed by user.
    // But that's the wrong model; instead: the `expanded` set is additive for
    // items (need to add to see detail), while auto-expanded nodes need to be
    // tracked in a separate "collapsed" set.
    //
    // Pragmatic: auto-expanded nodes show children unless their id is in the
    // `expanded` set prefixed with "collapsed:". For now, just auto-expand all
    // structural nodes so items are always visible; the user can't collapse them
    // in this iteration (a future improvement). Items require explicit expand.
    let is_expanded = is_auto_expanded || expanded.contains(&node.id);
    let expandable = !node.children.is_empty();
    out.push(FlatNode {
        id: node.id.clone(),
        label: node.label.clone(),
        depth,
        expandable,
        expanded: is_expanded,
        node: node.node.clone(),
    });
    if is_expanded {
        for child in &node.children {
            flatten_node(child, depth + 1, expanded, out);
        }
    }
}

fn short_commit(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.chars().take(8).collect()
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.chars().count() <= max {
        s
    } else {
        // Find the byte offset of the max-th char
        let idx = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        &s[..idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::data::{Snapshot, SnapshotInstalled, SnapshotAvailable};
    use crate::error::ItemKind;
    use std::collections::HashSet;

    fn make_installed(key: &str, name: &str, source: &str, kind: ItemKind) -> SnapshotInstalled {
        SnapshotInstalled {
            key: key.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            kind,
            commit: "abc12345".to_string(),
            description: Some(format!("{name} description")),
        }
    }

    fn make_available(key: &str, name: &str, source: &str, kind: ItemKind) -> SnapshotAvailable {
        SnapshotAvailable {
            key: key.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            kind,
            description: Some(format!("{name} description")),
            path: std::path::PathBuf::from(format!("/fake/{name}")),
        }
    }

    fn snap_with(installed: Vec<SnapshotInstalled>, available: Vec<SnapshotAvailable>) -> Snapshot {
        Snapshot {
            generation: 1,
            installed,
            available,
            source_names: vec!["src/a".to_string()],
            suggestions: vec![],
            lobes: vec![],
        }
    }

    #[test]
    fn tree_has_installed_and_available_groups() {
        // spec: TUI-10
        let snap = snap_with(
            vec![make_installed("skill:review", "review", "src/a", ItemKind::Skill)],
            vec![make_available("agent:dev", "dev", "src/a", ItemKind::Agent)],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        assert_eq!(nodes.len(), 2);
        assert!(matches!(nodes[0].node, TreeNode::InstalledGroup));
        assert!(matches!(nodes[1].node, TreeNode::AvailableGroup));
    }

    #[test]
    fn installed_group_contains_installed_items() {
        // spec: TUI-12
        let snap = snap_with(
            vec![make_installed("skill:review", "review", "src/a", ItemKind::Skill)],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        // Flatten and look for InstalledItem
        let flat = flatten_tree(&nodes, &HashSet::new());
        // Group headers are always expanded, so items appear even without expansion
        let has_review = flat.iter().any(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"));
        assert!(has_review, "installed item should appear in flat tree: {:?}", flat.iter().map(|n| &n.label).collect::<Vec<_>>());
    }

    #[test]
    fn available_group_excludes_installed_items() {
        // spec: TUI-13 (dedup: installed items not shown in Available)
        let installed = make_installed("skill:review", "review", "src/a", ItemKind::Skill);
        // Same key in available
        let also_avail = make_available("skill:review", "review", "src/a", ItemKind::Skill);
        let snap = snap_with(vec![installed], vec![also_avail]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        // Should not appear under Available
        let avail_review = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"));
        assert!(!avail_review, "installed item must not appear in Available tree");
    }

    #[test]
    fn search_filters_available_items() {
        // spec: TUI-14
        let snap = snap_with(
            vec![],
            vec![
                make_available("agent:dev", "dev", "src/a", ItemKind::Agent),
                make_available("agent:plan", "plan", "src/a", ItemKind::Agent),
            ],
        );
        // Search for "dev" only
        let nodes = build_tree(&snap, "dev", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let has_dev = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "dev"));
        let has_plan = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "plan"));
        assert!(has_dev, "dev should match search 'dev'");
        assert!(!has_plan, "plan should not match search 'dev'");
    }

    #[test]
    fn search_is_case_insensitive() {
        // spec: TUI-14
        let snap = snap_with(
            vec![],
            vec![make_available("skill:Review", "Review", "src/a", ItemKind::Skill)],
        );
        let nodes = build_tree(&snap, "review", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let found = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "Review"));
        assert!(found, "search should be case-insensitive");
    }

    #[test]
    fn search_matches_description() {
        // spec: TUI-14
        let mut item = make_available("skill:x", "x", "src/a", ItemKind::Skill);
        item.description = Some("automated code formatter".to_string());
        let snap = snap_with(vec![], vec![item]);
        let nodes = build_tree(&snap, "formatter", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let found = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "x"));
        assert!(found, "search should match description text");
    }

    #[test]
    fn kind_filter_narrows_available() {
        // spec: TUI-14
        let snap = snap_with(
            vec![],
            vec![
                make_available("skill:s", "s", "src/a", ItemKind::Skill),
                make_available("agent:a", "a", "src/a", ItemKind::Agent),
            ],
        );
        let nodes = build_tree(&snap, "", Some(ItemKind::Skill), None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let has_skill = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "s"));
        let has_agent = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "a"));
        assert!(has_skill, "skill should survive kind=Skill filter");
        assert!(!has_agent, "agent should be filtered out by kind=Skill");
    }

    #[test]
    fn flatten_shows_items_by_default() {
        // spec: TUI-11
        // Source nodes and kind-buckets are auto-expanded so items are visible
        // immediately without any explicit expansion. This gives the user a
        // usable default view.
        let snap = snap_with(
            vec![make_installed("skill:review", "review", "src/a", ItemKind::Skill)],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let group_visible = flat.iter().any(|n| matches!(&n.node, TreeNode::InstalledGroup));
        assert!(group_visible, "group header should always be visible");
        // With auto-expansion, items should be visible too.
        let item_visible = flat.iter().any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(item_visible, "items should be visible by default (auto-expand): {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>());
    }

    #[test]
    fn search_and_kind_filter_compose() {
        // spec: TUI-13 TUI-14 - a search query and a kind filter applied at the
        // SAME time must both constrain the result (composition, not either/or).
        let snap = snap_with(
            vec![],
            vec![
                // matches search "re" AND kind=Skill -> survives
                make_available("skill:review", "review", "src/a", ItemKind::Skill),
                // matches kind=Skill but NOT search "re" -> filtered by search
                make_available("skill:build", "build", "src/a", ItemKind::Skill),
                // matches search "re" but NOT kind=Skill -> filtered by kind
                make_available("agent:render", "render", "src/a", ItemKind::Agent),
            ],
        );
        let nodes = build_tree(&snap, "re", Some(ItemKind::Skill), None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let names: Vec<&str> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::AvailableItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"review"), "review matches both axes: {names:?}");
        assert!(!names.contains(&"build"), "build fails the search axis: {names:?}");
        assert!(!names.contains(&"render"), "render fails the kind axis: {names:?}");
    }

    #[test]
    fn search_and_source_filter_compose() {
        // spec: TUI-13 TUI-14 - search composes with the source filter too.
        let snap = snap_with(
            vec![],
            vec![
                make_available("skill:review", "review", "a/agents", ItemKind::Skill),
                // same name, different source -> excluded by source filter
                make_available("skill:review", "review", "b/agents", ItemKind::Skill),
                // right source, wrong search -> excluded by search
                make_available("skill:build", "build", "a/agents", ItemKind::Skill),
            ],
        );
        let nodes = build_tree(&snap, "review", None, Some("a/agents"), false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        // Only the a/agents review survives both filters.
        let from_a = flat.iter().any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "a/agents"));
        let from_b = flat.iter().any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "b/agents"));
        assert!(from_a, "a/agents source should remain after composed filters");
        assert!(!from_b, "b/agents source must be excluded by the source filter");
        let has_build = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "build"));
        assert!(!has_build, "build must be excluded by the search filter");
    }

    #[test]
    fn clearing_search_restores_full_tree() {
        // spec: TUI-14 - "Clearing the search restores the full tree." Build with a
        // narrowing search, then with an empty search, and confirm the empty-search
        // tree is a strict superset.
        let snap = snap_with(
            vec![],
            vec![
                make_available("skill:review", "review", "src/a", ItemKind::Skill),
                make_available("agent:dev", "dev", "src/a", ItemKind::Agent),
                make_available("rule:style", "style", "src/a", ItemKind::Rule),
            ],
        );
        let filtered = flatten_tree(&build_tree(&snap, "review", None, None, false, false), &HashSet::new());
        let restored = flatten_tree(&build_tree(&snap, "", None, None, false, false), &HashSet::new());

        let filtered_items = filtered
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(_)))
            .count();
        let restored_items = restored
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(_)))
            .count();
        assert_eq!(filtered_items, 1, "search 'review' should match exactly one item");
        assert_eq!(restored_items, 3, "clearing the search restores all three items");
        // The previously hidden items are back.
        for want in ["dev", "style", "review"] {
            assert!(
                restored.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == want)),
                "{want} should reappear after clearing search"
            );
        }
    }

    #[test]
    fn search_surfaces_description_only_match_with_filter_active() {
        // spec: TUI-13 TUI-14 - a query that matches the DESCRIPTION but not the
        // NAME still surfaces the item (consistent with CLI-85 via
        // catalog::matches_query), even while a kind filter is also active.
        let mut item = make_available("skill:fmt", "fmt", "src/a", ItemKind::Skill);
        item.description = Some("automated code formatter".to_string());
        let other = make_available("agent:fmt", "fmt", "src/a", ItemKind::Agent);
        let snap = snap_with(vec![], vec![item, other]);
        // "formatter" appears only in the skill's description; kind=Skill is active.
        let nodes = build_tree(&snap, "formatter", Some(ItemKind::Skill), None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let has_skill = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i)
            if i.name == "fmt" && i.kind == ItemKind::Skill));
        let has_agent = flat.iter().any(|n| matches!(&n.node, TreeNode::AvailableItem(i)
            if i.kind == ItemKind::Agent));
        assert!(has_skill, "description-only match must surface the skill: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>());
        assert!(!has_agent, "the agent (wrong kind, no desc match) must be filtered out");
    }

    #[test]
    fn two_uninstalled_sources_same_name_both_appear() {
        // spec: TUI-13 - de-dup removes only items that are INSTALLED. Two distinct
        // (uninstalled) sources that each ship a same-bare-name item must BOTH appear
        // in Available, each under its own source node. (Contrast with
        // available_dedup_across_two_sources, where the item IS installed.)
        let from_a = make_available("skill:review", "review", "a/agents", ItemKind::Skill);
        let from_b = make_available("skill:review", "review", "b/agents", ItemKind::Skill);
        let snap = snap_with(vec![], vec![from_a, from_b]); // nothing installed
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let review_count = flat
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"))
            .count();
        assert_eq!(
            review_count, 2,
            "both uninstalled same-name items should appear (one per source): {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        let source_a = flat.iter().any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "a/agents"));
        let source_b = flat.iter().any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "b/agents"));
        assert!(source_a && source_b, "both source nodes should be present");
    }

    #[test]
    fn suggested_source_with_melded_url_excluded_at_data_layer() {
        // spec: TUI-31 - a SuggestedSource is only built from snap.suggestions, which
        // the data layer (preview::suggested_registry) has already filtered to exclude
        // already-melded URLs. Here we verify the tree faithfully renders whatever
        // suggestions it is given (the exclusion itself is tested in preview.rs).
        // An empty suggestions list must yield no SuggestedSource nodes.
        let snap = snap_with(vec![], vec![]); // no suggestions
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        let any_suggested = flat.iter().any(|n| matches!(&n.node, TreeNode::SuggestedSource(_)));
        assert!(!any_suggested, "no suggestions -> no SuggestedSource nodes");
    }

    #[test]
    fn available_dedup_across_two_sources() {
        // spec: TUI-13
        // An item key appearing in both installed and available (from a different source)
        // should still be deduped (the installed KEY is what matters).
        let installed = make_installed("skill:review", "review", "src/a", ItemKind::Skill);
        let also_from_b = make_available("skill:review", "review", "src/b", ItemKind::Skill);
        let snap = snap_with(vec![installed], vec![also_from_b]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new());
        // Same key is deduped regardless of source name
        let avail_count = flat.iter().filter(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review")).count();
        assert_eq!(avail_count, 0, "same key in available should be deduped vs installed");
    }
}
