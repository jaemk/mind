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
    /// The synthetic "unmanaged" group header (UNM-6): lobe items mind did not
    /// install, distinct from any source.
    UnmanagedGroup,
    /// A single unmanaged lobe item under the unmanaged group (UNM-6).
    UnmanagedItem(UnmanagedInfo),
    /// A dependency child node shown under an expanded item node (TUI-50).
    /// This is a VIEW of the graph, not the item's canonical line.
    // spec: TUI-50
    DepChild(DepChildInfo),
}

/// Info for a dependency child node under an expanded item node (TUI-50).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DepChildInfo {
    /// The `kind:name` key of the dependency.
    pub key: String,
    /// Human-readable name of the dependency.
    pub name: String,
    /// True if this dependency would revisit an ancestor on the current path
    /// (DEP-22 cycle safety): shown with a marker and not expanded again.
    // spec: DEP-22
    pub is_cycle: bool,
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
    /// Direct dependency keys for TUI-50 dependency subtree expansion.
    // spec: TUI-50
    pub deps: Vec<String>,
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
    /// Direct dependency keys for TUI-50 dependency subtree expansion.
    // spec: TUI-50
    pub deps: Vec<String>,
}

/// Info about a single unmanaged lobe item (UNM-6). `key` is the `kind:name`
/// form so the forget action resolves it via the same ref path as a managed
/// item (UNM-4).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UnmanagedInfo {
    pub key: String,
    pub name: String,
    pub kind: ItemKind,
    pub paths: Vec<PathBuf>,
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

/// Build the dependency subtree children for an item node (TUI-50).
///
/// `item_key` is the `kind:name` key of the item whose children we build.
/// `dep_keys` is the item's list of direct dependency keys.
/// `all_deps` maps every known item key to its direct dep keys (from the snapshot).
/// `ancestors` is the set of keys on the current path from the tree root to this
/// item, used for DEP-22 cycle detection: a dep that appears in `ancestors` is
/// shown as a back-edge marker (`(cycle)`) and not expanded again.
///
/// Returns a Vec of child Nodes representing the dependency subtree view.
// spec: TUI-50 DEP-22
fn build_dep_children(
    item_key: &str,
    dep_keys: &[String],
    all_deps: &std::collections::HashMap<String, Vec<String>>,
    ancestors: &mut Vec<String>,
) -> Vec<Node> {
    ancestors.push(item_key.to_string());
    let mut children = Vec::new();
    for dep_key in dep_keys {
        let is_cycle = ancestors.contains(dep_key);
        let name = dep_key
            .split_once(':')
            .map(|(_, n)| n)
            .unwrap_or(dep_key.as_str())
            .to_string();
        let label = if is_cycle {
            format!("{} (cycle)", dep_key)
        } else {
            dep_key.clone()
        };
        // The node id encodes the path so sibling items with a shared dep each
        // get their own independently expandable node in the view.
        let node_id = format!("dep:{}:{}", item_key, dep_key);
        // Build grandchildren only for non-cycle deps that have their own deps.
        let grandchildren = if is_cycle {
            Vec::new()
        } else {
            let grandchild_deps: Vec<String> = all_deps.get(dep_key).cloned().unwrap_or_default();
            if grandchild_deps.is_empty() {
                Vec::new()
            } else {
                build_dep_children(dep_key, &grandchild_deps, all_deps, ancestors)
            }
        };
        children.push(Node {
            id: node_id,
            label,
            node: TreeNode::DepChild(DepChildInfo {
                key: dep_key.clone(),
                name,
                is_cycle,
            }),
            children: grandchildren,
        });
    }
    ancestors.pop();
    children
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
    // Build a combined dep-key map from all installed and available items so
    // dep children can recursively expand regardless of which group they live in.
    // spec: TUI-50
    let mut all_deps: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for it in &snap.installed {
        all_deps
            .entry(it.key.clone())
            .or_default()
            .extend(it.deps.iter().cloned());
    }
    for it in &snap.available {
        all_deps
            .entry(it.key.clone())
            .or_default()
            .extend(it.deps.iter().cloned());
    }

    let mut roots = Vec::new();

    // --- Installed group ---
    // spec: TUI-10 TUI-12
    let installed_node = build_installed_group(snap, search, kind_filter, source_filter, &all_deps);
    let installed_count = count_items(&installed_node.children);
    let installed_label = if installed_collapsed {
        "Installed (collapsed)".to_string()
    } else {
        format!("Installed ({installed_count})")
    };
    roots.push(Node {
        id: "group:installed".to_string(),
        label: installed_label,
        children: if installed_collapsed {
            vec![]
        } else {
            installed_node.children
        },
        node: TreeNode::InstalledGroup,
    });

    // --- Available group ---
    // spec: TUI-10 TUI-13
    let available_node = build_available_group(snap, search, kind_filter, source_filter, &all_deps);
    let available_count = count_items(&available_node.children);
    let available_label = if available_collapsed {
        "Available (collapsed)".to_string()
    } else {
        format!("Available ({available_count})")
    };
    roots.push(Node {
        id: "group:available".to_string(),
        label: available_label,
        children: if available_collapsed {
            vec![]
        } else {
            available_node.children
        },
        node: TreeNode::AvailableGroup,
    });

    // --- Unmanaged group (UNM-6) ---
    // Built only when there are unmanaged items to show after filtering, so the
    // group is absent (not an empty header) on the common all-managed home. It
    // is auto-expanded and collapses via the `collapsed` set like a Source node.
    // spec: UNM-6
    let unmanaged_children = build_unmanaged_children(snap, search, kind_filter, source_filter);
    if !unmanaged_children.is_empty() {
        roots.push(Node {
            id: "group:unmanaged".to_string(),
            label: format!("Unmanaged ({})", unmanaged_children.len()),
            node: TreeNode::UnmanagedGroup,
            children: unmanaged_children,
        });
    }

    roots
}

fn count_items(children: &[Node]) -> usize {
    let mut n = 0;
    for c in children {
        match &c.node {
            TreeNode::InstalledItem(_)
            | TreeNode::AvailableItem(_)
            | TreeNode::UnmanagedItem(_) => n += 1,
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
    all_deps: &std::collections::HashMap<String, Vec<String>>,
) -> Node {
    // Group installed items by source -> kind.
    let mut by_source: std::collections::BTreeMap<
        String,
        Vec<&crate::tui::data::SnapshotInstalled>,
    > = std::collections::BTreeMap::new();
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
        let mut kind_map: std::collections::BTreeMap<
            ItemKind,
            Vec<&&crate::tui::data::SnapshotInstalled>,
        > = std::collections::BTreeMap::new();
        for item in items {
            kind_map.entry(item.kind).or_default().push(item);
        }

        let mut kind_nodes = Vec::new();
        for (kind, kind_items) in &kind_map {
            let item_nodes: Vec<Node> = kind_items
                .iter()
                .map(|it| {
                    // Build dep children for TUI-50 (cycle-safe via ancestors path).
                    // spec: TUI-50
                    let dep_children = if it.deps.is_empty() {
                        Vec::new()
                    } else {
                        let mut ancestors = Vec::new();
                        build_dep_children(&it.key, &it.deps, all_deps, &mut ancestors)
                    };
                    Node {
                        id: format!("installed:{}:{}", src_name, it.key),
                        label: format!(
                            "{} [{}]{}",
                            it.name,
                            short_commit(&it.commit),
                            it.description
                                .as_deref()
                                .map(|d| format!(" - {}", truncate(d, 50)))
                                .unwrap_or_default()
                        ),
                        node: TreeNode::InstalledItem(InstalledInfo {
                            key: it.key.clone(),
                            name: it.name.clone(),
                            source: it.source.clone(),
                            kind: it.kind,
                            commit: it.commit.clone(),
                            description: it.description.clone(),
                            deps: it.deps.clone(),
                        }),
                        children: dep_children,
                    }
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
    all_deps: &std::collections::HashMap<String, Vec<String>>,
) -> Node {
    // Installed item keys (for de-dup).
    let installed_keys: HashSet<String> = snap.installed.iter().map(|i| i.key.clone()).collect();

    let mut by_source: std::collections::BTreeMap<
        String,
        Vec<&crate::tui::data::SnapshotAvailable>,
    > = std::collections::BTreeMap::new();

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
        let mut kind_map: std::collections::BTreeMap<
            ItemKind,
            Vec<&&crate::tui::data::SnapshotAvailable>,
        > = std::collections::BTreeMap::new();
        for item in items {
            kind_map.entry(item.kind).or_default().push(item);
        }

        let mut kind_nodes = Vec::new();
        for (kind, kind_items) in &kind_map {
            let item_nodes: Vec<Node> = kind_items
                .iter()
                .map(|it| {
                    // Build dep children for TUI-50.
                    // spec: TUI-50
                    let dep_children = if it.deps.is_empty() {
                        Vec::new()
                    } else {
                        let mut ancestors = Vec::new();
                        build_dep_children(&it.key, &it.deps, all_deps, &mut ancestors)
                    };
                    Node {
                        id: format!("available:{}:{}", src_name, it.key),
                        label: format!(
                            "{}{}",
                            it.name,
                            it.description
                                .as_deref()
                                .map(|d| format!(" - {}", truncate(d, 50)))
                                .unwrap_or_default()
                        ),
                        node: TreeNode::AvailableItem(AvailableInfo {
                            key: it.key.clone(),
                            name: it.name.clone(),
                            source: it.source.clone(),
                            kind: it.kind,
                            description: it.description.clone(),
                            path: it.path.clone(),
                            deps: it.deps.clone(),
                        }),
                        children: dep_children,
                    }
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

/// Build the leaf nodes under the unmanaged group (UNM-6): one row per
/// unmanaged lobe item, sorted by `(kind, name)` as the snapshot already is.
/// `--kind` filters; a `--source` filter excludes every unmanaged item (they
/// have no source, per UNM-3); search matches the item name (CLI-85: unmanaged
/// items carry no description, so name is all there is to match).
// spec: UNM-6
fn build_unmanaged_children(
    snap: &Snapshot,
    search: &str,
    kind_filter: Option<ItemKind>,
    source_filter: Option<&str>,
) -> Vec<Node> {
    // A source filter excludes unmanaged items entirely (UNM-3).
    if source_filter.is_some() {
        return Vec::new();
    }
    let needle = search.to_lowercase();
    snap.unmanaged
        .iter()
        .filter(|it| kind_filter.is_none_or(|kf| it.kind == kf))
        .filter(|it| needle.is_empty() || it.name.to_lowercase().contains(&needle))
        .map(|it| Node {
            id: format!("unmanaged:{}", it.key),
            label: format!("{} [{}] (unmanaged)", it.name, it.kind.as_str()),
            node: TreeNode::UnmanagedItem(UnmanagedInfo {
                key: it.key.clone(),
                name: it.name.clone(),
                kind: it.kind,
                paths: it.paths.clone(),
            }),
            children: vec![],
        })
        .collect()
}

/// True if an installed item matches the search query.
/// Delegates to `catalog::matches_query` (CLI-85 / TUI-14) so both installed
/// and available search share one source of truth.
// spec: TUI-14 CLI-85
fn item_matches_search_installed(item: &crate::tui::data::SnapshotInstalled, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    // Build a temporary CatalogItem to reuse catalog::matches_query exactly,
    // mirroring item_matches_search_available. The installed name is already the
    // effective (possibly prefixed) name, so prefix is None to avoid
    // double-prefixing via effective_name().
    let fake = catalog::CatalogItem {
        kind: item.kind,
        name: item.name.clone(),
        source: item.source.clone(),
        prefix: None,
        path: std::path::PathBuf::new(),
        description: item.description.clone(),
        link_rel: None,
        bin: None,
        build: None,
        install: None,
        uninstall: None,
        requires: Vec::new(),
        hooks: Vec::new(),
    };
    catalog::matches_query(&fake, search)
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
        bin: None,
        build: None,
        install: None,
        uninstall: None,
        requires: Vec::new(),
        hooks: Vec::new(),
    };
    catalog::matches_query(&fake, search)
}

/// Returns true for structural nodes that are auto-expanded by default
/// (InstalledGroup, AvailableGroup, Source, KindBucket). These nodes use the
/// `collapsed` set to opt-out of expansion, rather than requiring an explicit
/// `expanded` entry to opt-in.
// spec: TUI-11
pub fn is_auto_expanded(node: &TreeNode) -> bool {
    matches!(
        node,
        TreeNode::InstalledGroup
            | TreeNode::AvailableGroup
            | TreeNode::UnmanagedGroup
            | TreeNode::Source(_)
            | TreeNode::KindBucket { .. }
    )
}

/// Flatten the tree into a displayable list, respecting expansion state.
/// A group header is always shown. A node's children are shown only if the
/// node is expanded. Auto-expanded nodes (Source, KindBucket, group headers)
/// use the `collapsed` set to track user-initiated collapses; non-auto nodes
/// (InstalledItem, AvailableItem, SuggestedSource children) require explicit
/// membership in `expanded` to show children.
// spec: TUI-10 TUI-11
pub fn flatten_tree(
    nodes: &[Node],
    expanded: &HashSet<String>,
    collapsed: &HashSet<String>,
) -> Vec<FlatNode> {
    let mut out = Vec::new();
    for node in nodes {
        flatten_node(node, 0, expanded, collapsed, &mut out);
    }
    out
}

fn flatten_node(
    node: &Node,
    depth: usize,
    expanded: &HashSet<String>,
    collapsed: &HashSet<String>,
    out: &mut Vec<FlatNode>,
) {
    // Auto-expanded nodes (group headers, source nodes, kind-buckets) are
    // visible by default and collapsed only when their ID is in `collapsed`.
    // Item-detail nodes (InstalledItem / AvailableItem) require explicit
    // membership in `expanded` to reveal children.
    let is_exp = if is_auto_expanded(&node.node) {
        !collapsed.contains(&node.id)
    } else {
        expanded.contains(&node.id)
    };
    let expandable = !node.children.is_empty();
    out.push(FlatNode {
        id: node.id.clone(),
        label: node.label.clone(),
        depth,
        expandable,
        expanded: is_exp,
        node: node.node.clone(),
    });
    if is_exp {
        for child in &node.children {
            flatten_node(child, depth + 1, expanded, collapsed, out);
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
    use crate::error::ItemKind;
    use crate::tui::data::{Snapshot, SnapshotAvailable, SnapshotInstalled};
    use std::collections::HashSet;

    fn make_installed(key: &str, name: &str, source: &str, kind: ItemKind) -> SnapshotInstalled {
        SnapshotInstalled {
            key: key.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            kind,
            commit: "abc12345".to_string(),
            description: Some(format!("{name} description")),
            deps: vec![],
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
            deps: vec![],
        }
    }

    fn snap_with(installed: Vec<SnapshotInstalled>, available: Vec<SnapshotAvailable>) -> Snapshot {
        Snapshot {
            generation: 1,
            installed,
            available,
            unmanaged: vec![],
            source_names: vec!["src/a".to_string()],
            suggestions: vec![],
            lobes: vec![],
            source_namespaces: std::collections::HashMap::new(),
        }
    }

    fn make_unmanaged(name: &str, kind: ItemKind) -> crate::tui::data::SnapshotUnmanaged {
        crate::tui::data::SnapshotUnmanaged {
            key: format!("{}:{}", kind.as_str(), name),
            name: name.to_string(),
            kind,
            paths: vec![std::path::PathBuf::from(format!("/lobe/{name}"))],
        }
    }

    #[test]
    fn tree_has_installed_and_available_groups() {
        // spec: TUI-10
        let snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
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
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        // Flatten and look for InstalledItem
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        // Group headers are always expanded, so items appear even without expansion
        let has_review = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"));
        assert!(
            has_review,
            "installed item should appear in flat tree: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn available_group_excludes_installed_items() {
        // spec: TUI-13 (dedup: installed items not shown in Available)
        let installed = make_installed("skill:review", "review", "src/a", ItemKind::Skill);
        // Same key in available
        let also_avail = make_available("skill:review", "review", "src/a", ItemKind::Skill);
        let snap = snap_with(vec![installed], vec![also_avail]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        // Should not appear under Available
        let avail_review = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"));
        assert!(
            !avail_review,
            "installed item must not appear in Available tree"
        );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let has_dev = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "dev"));
        let has_plan = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "plan"));
        assert!(has_dev, "dev should match search 'dev'");
        assert!(!has_plan, "plan should not match search 'dev'");
    }

    #[test]
    fn search_is_case_insensitive() {
        // spec: TUI-14
        let snap = snap_with(
            vec![],
            vec![make_available(
                "skill:Review",
                "Review",
                "src/a",
                ItemKind::Skill,
            )],
        );
        let nodes = build_tree(&snap, "review", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let found = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "Review"));
        assert!(found, "search should be case-insensitive");
    }

    #[test]
    fn search_matches_description() {
        // spec: TUI-14
        let mut item = make_available("skill:x", "x", "src/a", ItemKind::Skill);
        item.description = Some("automated code formatter".to_string());
        let snap = snap_with(vec![], vec![item]);
        let nodes = build_tree(&snap, "formatter", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let found = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "x"));
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let has_skill = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "s"));
        let has_agent = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "a"));
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
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let group_visible = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledGroup));
        assert!(group_visible, "group header should always be visible");
        // With auto-expansion, items should be visible too.
        let item_visible = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(
            item_visible,
            "items should be visible by default (auto-expand): {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let names: Vec<&str> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::AvailableItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            names.contains(&"review"),
            "review matches both axes: {names:?}"
        );
        assert!(
            !names.contains(&"build"),
            "build fails the search axis: {names:?}"
        );
        assert!(
            !names.contains(&"render"),
            "render fails the kind axis: {names:?}"
        );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        // Only the a/agents review survives both filters.
        let from_a = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "a/agents"));
        let from_b = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "b/agents"));
        assert!(
            from_a,
            "a/agents source should remain after composed filters"
        );
        assert!(
            !from_b,
            "b/agents source must be excluded by the source filter"
        );
        let has_build = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "build"));
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
        let filtered = flatten_tree(
            &build_tree(&snap, "review", None, None, false, false),
            &HashSet::new(),
            &HashSet::new(),
        );
        let restored = flatten_tree(
            &build_tree(&snap, "", None, None, false, false),
            &HashSet::new(),
            &HashSet::new(),
        );

        let filtered_items = filtered
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(_)))
            .count();
        let restored_items = restored
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(_)))
            .count();
        assert_eq!(
            filtered_items, 1,
            "search 'review' should match exactly one item"
        );
        assert_eq!(
            restored_items, 3,
            "clearing the search restores all three items"
        );
        // The previously hidden items are back.
        for want in ["dev", "style", "review"] {
            assert!(
                restored
                    .iter()
                    .any(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == want)),
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
        let nodes = build_tree(
            &snap,
            "formatter",
            Some(ItemKind::Skill),
            None,
            false,
            false,
        );
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let has_skill = flat.iter().any(|n| {
            matches!(&n.node, TreeNode::AvailableItem(i)
            if i.name == "fmt" && i.kind == ItemKind::Skill)
        });
        let has_agent = flat.iter().any(|n| {
            matches!(&n.node, TreeNode::AvailableItem(i)
            if i.kind == ItemKind::Agent)
        });
        assert!(
            has_skill,
            "description-only match must surface the skill: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        assert!(
            !has_agent,
            "the agent (wrong kind, no desc match) must be filtered out"
        );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let review_count = flat
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"))
            .count();
        assert_eq!(
            review_count,
            2,
            "both uninstalled same-name items should appear (one per source): {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        let source_a = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "a/agents"));
        let source_b = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::Source(s) if s.name == "b/agents"));
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let any_suggested = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::SuggestedSource(_)));
        assert!(!any_suggested, "no suggestions -> no SuggestedSource nodes");
    }

    #[test]
    fn installed_search_matches_description_not_just_name() {
        // spec: CLI-85
        // An installed item whose NAME does not contain the query but whose
        // DESCRIPTION does must still be matched. This proves item_matches_search_installed
        // delegates to catalog::matches_query (same logic as available search),
        // rather than reimplementing an inline name-only check.
        let mut item = make_installed("skill:fmt", "fmt", "src/a", ItemKind::Skill);
        item.description = Some("automated code formatter".to_string());
        // A second installed item with neither name nor description matching.
        let other = make_installed("skill:lint", "lint", "src/a", ItemKind::Skill);

        let snap = snap_with(vec![item, other], vec![]);
        // "formatter" appears only in the first item's description.
        let nodes = build_tree(&snap, "formatter", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());

        let has_fmt = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "fmt"));
        let has_lint = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "lint"));

        assert!(
            has_fmt,
            "installed item should be matched by description: {:?}",
            flat.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        assert!(
            !has_lint,
            "installed item with no name/desc match must be filtered out"
        );
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
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        // Same key is deduped regardless of source name
        let avail_count = flat
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"))
            .count();
        assert_eq!(
            avail_count, 0,
            "same key in available should be deduped vs installed"
        );
    }

    // --- TUI-11: collapsed set governs Source/KindBucket visibility ---

    #[test]
    fn source_id_in_collapsed_hides_its_children() {
        // spec: TUI-11 - when a Source node's id is in the `collapsed` set, its
        // children (KindBucket and item nodes) must be absent from the flat output.
        let snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);

        // Find the Source node id.
        let flat_default = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let source_id = flat_default
            .iter()
            .find(|n| matches!(&n.node, TreeNode::Source(_)))
            .map(|n| n.id.clone())
            .expect("a Source node must be present");

        // Children are present when not collapsed.
        let has_item = flat_default
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(
            has_item,
            "item should be visible when source is not collapsed"
        );

        // Put the source id in collapsed: children must disappear.
        let mut collapsed = HashSet::new();
        collapsed.insert(source_id.clone());
        let flat_collapsed = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let has_item_after = flat_collapsed
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        let has_bucket_after = flat_collapsed
            .iter()
            .any(|n| matches!(&n.node, TreeNode::KindBucket { .. }));
        assert!(
            !has_item_after,
            "item must be absent when source is in the collapsed set"
        );
        assert!(
            !has_bucket_after,
            "KindBucket must also be absent when source is in the collapsed set"
        );

        // Source node itself must still be visible.
        let source_visible = flat_collapsed.iter().any(|n| n.id == source_id);
        assert!(source_visible, "the Source node itself must remain visible");
    }

    #[test]
    fn kind_bucket_id_in_collapsed_hides_its_items() {
        // spec: TUI-11 - when a KindBucket node's id is in the `collapsed` set,
        // its item children must be absent while the bucket node itself stays visible.
        let snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);

        let flat_default = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let bucket_id = flat_default
            .iter()
            .find(|n| matches!(&n.node, TreeNode::KindBucket { .. }))
            .map(|n| n.id.clone())
            .expect("a KindBucket node must be present");

        // Items visible without collapsed.
        let has_item = flat_default
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(
            has_item,
            "item should be visible when bucket is not collapsed"
        );

        // Collapse the bucket: items disappear, bucket stays.
        let mut collapsed = HashSet::new();
        collapsed.insert(bucket_id.clone());
        let flat_collapsed = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let has_item_after = flat_collapsed
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(
            !has_item_after,
            "item must be absent when KindBucket is in the collapsed set"
        );
        let bucket_visible = flat_collapsed.iter().any(|n| n.id == bucket_id);
        assert!(
            bucket_visible,
            "the KindBucket node itself must stay visible"
        );
    }

    #[test]
    fn removing_source_from_collapsed_restores_children() {
        // spec: TUI-11 - removing a Source id from the collapsed set (expand
        // action) must restore its children in the next flatten call.
        let snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat_default = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let source_id = flat_default
            .iter()
            .find(|n| matches!(&n.node, TreeNode::Source(_)))
            .map(|n| n.id.clone())
            .expect("a Source node must be present");

        // Collapse then expand.
        let mut collapsed = HashSet::new();
        collapsed.insert(source_id.clone());
        let flat_collapsed = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let hidden = flat_collapsed
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(!hidden, "items hidden after collapse");

        collapsed.remove(&source_id);
        let flat_restored = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let restored = flat_restored
            .iter()
            .any(|n| matches!(&n.node, TreeNode::InstalledItem(_)));
        assert!(
            restored,
            "items restored after removing source from collapsed set"
        );
    }

    #[test]
    fn normal_item_node_still_requires_expanded_membership() {
        // spec: TUI-11 - non-auto-expanded nodes (InstalledItem, AvailableItem)
        // use the `expanded` set, not `collapsed`. A node with children is only
        // shown expanded when its id is in `expanded`; the `collapsed` set has
        // no effect on it.
        //
        // Build a synthetic Node tree with a parent InstalledItem that has a
        // child InstalledItem (not possible from the default snapshot but valid
        // for exercising flatten_node's branching).
        let parent = Node {
            id: "item-parent".to_string(),
            label: "parent".to_string(),
            node: TreeNode::InstalledItem(InstalledInfo {
                key: "skill:parent".to_string(),
                name: "parent".to_string(),
                source: "src/a".to_string(),
                kind: ItemKind::Skill,
                commit: "abc".to_string(),
                description: None,
                deps: vec![],
            }),
            children: vec![Node {
                id: "item-child".to_string(),
                label: "child".to_string(),
                node: TreeNode::InstalledItem(InstalledInfo {
                    key: "skill:child".to_string(),
                    name: "child".to_string(),
                    source: "src/a".to_string(),
                    kind: ItemKind::Skill,
                    commit: "abc".to_string(),
                    description: None,
                    deps: vec![],
                }),
                children: vec![],
            }],
        };

        let nodes = vec![parent];

        // Neither collapsed nor expanded: child must NOT be visible.
        let flat_empty = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let child_visible = flat_empty.iter().any(|n| n.id == "item-child");
        assert!(
            !child_visible,
            "InstalledItem child must be hidden without expanded membership"
        );

        // Collapsed contains the parent: still hidden (collapsed has no effect on non-auto nodes).
        let mut collapsed = HashSet::new();
        collapsed.insert("item-parent".to_string());
        let flat_only_collapsed = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let child_visible2 = flat_only_collapsed.iter().any(|n| n.id == "item-child");
        assert!(
            !child_visible2,
            "collapsed set must not expand non-auto nodes (they need `expanded` membership)"
        );

        // Expanded contains the parent: child appears.
        let mut expanded = HashSet::new();
        expanded.insert("item-parent".to_string());
        let flat_expanded = flatten_tree(&nodes, &expanded, &HashSet::new());
        let child_visible3 = flat_expanded.iter().any(|n| n.id == "item-child");
        assert!(
            child_visible3,
            "InstalledItem child must appear when parent is in `expanded`"
        );
    }

    // --- UNM-6: the unmanaged group node ---

    fn snap_with_unmanaged(unmanaged: Vec<crate::tui::data::SnapshotUnmanaged>) -> Snapshot {
        let mut snap = snap_with(vec![], vec![]);
        snap.unmanaged = unmanaged;
        snap
    }

    #[test]
    fn unmanaged_group_appears_with_items() {
        // spec: UNM-6 - unmanaged items appear under a dedicated group node,
        // distinct from any source, browsable (auto-expanded) like a source.
        let snap = snap_with_unmanaged(vec![
            make_unmanaged("hand-written", ItemKind::Skill),
            make_unmanaged("foreign", ItemKind::Agent),
        ]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        // A third root group beyond Installed/Available.
        let group = nodes
            .iter()
            .find(|n| matches!(n.node, TreeNode::UnmanagedGroup))
            .expect("an unmanaged group node must be present");
        assert_eq!(group.label, "Unmanaged (2)");
        // Items are visible by default (the group auto-expands).
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let names: Vec<&str> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::UnmanagedItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"hand-written") && names.contains(&"foreign"));
    }

    #[test]
    fn no_unmanaged_group_when_none() {
        // spec: UNM-6 - with no unmanaged items the group is absent entirely (not
        // an empty header), keeping the common all-managed home uncluttered.
        let snap = snap_with_unmanaged(vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        assert!(
            !nodes
                .iter()
                .any(|n| matches!(n.node, TreeNode::UnmanagedGroup)),
            "no unmanaged items -> no unmanaged group"
        );
    }

    #[test]
    fn unmanaged_search_filters_by_name() {
        // spec: UNM-6 - the unmanaged group is searchable like a source's items.
        let snap = snap_with_unmanaged(vec![
            make_unmanaged("review", ItemKind::Skill),
            make_unmanaged("deploy", ItemKind::Skill),
        ]);
        let nodes = build_tree(&snap, "rev", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let names: Vec<&str> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::UnmanagedItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["review"], "search 'rev' keeps only review");
    }

    #[test]
    fn unmanaged_kind_filter_applies() {
        // spec: UNM-6 - `--kind` filters unmanaged items as it does managed ones.
        let snap = snap_with_unmanaged(vec![
            make_unmanaged("s", ItemKind::Skill),
            make_unmanaged("a", ItemKind::Agent),
        ]);
        let nodes = build_tree(&snap, "", Some(ItemKind::Skill), None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let kinds: Vec<ItemKind> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::UnmanagedItem(i) => Some(i.kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec![ItemKind::Skill], "kind=Skill drops the agent");
    }

    #[test]
    fn unmanaged_source_filter_excludes_them() {
        // spec: UNM-6 - a `--source` filter excludes unmanaged items (they have no
        // source, per UNM-3), so the group is absent under any source filter.
        let snap = snap_with_unmanaged(vec![make_unmanaged("x", ItemKind::Skill)]);
        let nodes = build_tree(&snap, "", None, Some("some/source"), false, false);
        assert!(
            !nodes
                .iter()
                .any(|n| matches!(n.node, TreeNode::UnmanagedGroup)),
            "a source filter must exclude the unmanaged group"
        );
    }

    #[test]
    fn unmanaged_group_label_count_reflects_filtered_visible_items() {
        // spec: UNM-6 - the "Unmanaged (N)" label must report the number of items
        // VISIBLE after filtering, not the raw snapshot size. With three items and
        // a kind filter that keeps one, the label must read "Unmanaged (1)" and
        // exactly one item node must be present (count and contents agree).
        let snap = snap_with_unmanaged(vec![
            make_unmanaged("a", ItemKind::Skill),
            make_unmanaged("b", ItemKind::Agent),
            make_unmanaged("c", ItemKind::Agent),
        ]);
        let nodes = build_tree(&snap, "", Some(ItemKind::Skill), None, false, false);
        let group = nodes
            .iter()
            .find(|n| matches!(n.node, TreeNode::UnmanagedGroup))
            .expect("an unmanaged group node must be present");
        assert_eq!(
            group.label, "Unmanaged (1)",
            "label count must reflect items surviving the kind filter"
        );
        let item_count = group
            .children
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::UnmanagedItem(_)))
            .count();
        assert_eq!(
            item_count, 1,
            "the visible item count must equal the label count"
        );
    }

    #[test]
    fn unmanaged_search_composes_with_kind_filter() {
        // spec: UNM-6 - a search query and a kind filter applied at the same time
        // must BOTH constrain the unmanaged items (composition, not either/or),
        // mirroring how the managed axes compose.
        let snap = snap_with_unmanaged(vec![
            // matches search "re" AND kind=Skill -> survives
            make_unmanaged("review", ItemKind::Skill),
            // matches kind=Skill but NOT search "re" -> dropped by search
            make_unmanaged("build", ItemKind::Skill),
            // matches search "re" but NOT kind=Skill -> dropped by kind
            make_unmanaged("render", ItemKind::Agent),
        ]);
        let nodes = build_tree(&snap, "re", Some(ItemKind::Skill), None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let names: Vec<&str> = flat
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::UnmanagedItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["review"],
            "only the item matching BOTH search and kind survives: {names:?}"
        );
    }

    #[test]
    fn unmanaged_items_preserve_snapshot_kind_name_order() {
        // spec: UNM-6 - the unmanaged children are emitted in the snapshot's order
        // (the scanner sorts by (kind, name), UNM-1), with no reordering by
        // build_unmanaged_children. Feed an already-sorted snapshot and assert the
        // emitted node order matches it exactly.
        let snap = snap_with_unmanaged(vec![
            make_unmanaged("alpha", ItemKind::Skill),
            make_unmanaged("zeta", ItemKind::Skill),
            make_unmanaged("beta", ItemKind::Agent),
        ]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let group = nodes
            .iter()
            .find(|n| matches!(n.node, TreeNode::UnmanagedGroup))
            .expect("an unmanaged group node must be present");
        let order: Vec<&str> = group
            .children
            .iter()
            .filter_map(|n| match &n.node {
                TreeNode::UnmanagedItem(i) => Some(i.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            order,
            vec!["alpha", "zeta", "beta"],
            "node order must follow the snapshot order verbatim"
        );
    }

    #[test]
    fn unmanaged_group_sorts_after_installed_and_available() {
        // spec: UNM-6 - the synthetic unmanaged group is appended after the
        // Installed and Available group roots, so it is the last top-level group.
        let mut snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![make_available("agent:dev", "dev", "src/a", ItemKind::Agent)],
        );
        snap.unmanaged = vec![make_unmanaged("x", ItemKind::Skill)];
        let nodes = build_tree(&snap, "", None, None, false, false);
        assert!(matches!(nodes[0].node, TreeNode::InstalledGroup));
        assert!(matches!(nodes[1].node, TreeNode::AvailableGroup));
        assert!(
            matches!(nodes[2].node, TreeNode::UnmanagedGroup),
            "unmanaged group must be the last root group"
        );
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn unmanaged_item_distinct_node_id_from_managed_same_name() {
        // spec: UNM-6 - an unmanaged item whose kind:name collides with a managed
        // installed item must still produce a DISTINCT tree node (its own id under
        // "unmanaged:") so the two are independently selectable. The unmanaged
        // item's key remains its kind:name, matching the managed key by value but
        // carried on a separate node (UNM-4 ambiguity is resolved at forget time).
        let mut snap = snap_with(
            vec![make_installed(
                "skill:review",
                "review",
                "src/a",
                ItemKind::Skill,
            )],
            vec![],
        );
        snap.unmanaged = vec![make_unmanaged("review", ItemKind::Skill)];
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());

        let installed_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"))
            .map(|n| n.id.clone())
            .expect("managed review must be present");
        let unmanaged_node = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::UnmanagedItem(i) if i.name == "review"))
            .expect("unmanaged review must be present");
        assert_ne!(
            installed_id, unmanaged_node.id,
            "the colliding-name items must have distinct node ids"
        );
        assert_eq!(
            unmanaged_node.id, "unmanaged:skill:review",
            "the unmanaged node id is namespaced under 'unmanaged:'"
        );
        // Both keys equal kind:name by value, but the nodes are independent.
        if let TreeNode::UnmanagedItem(i) = &unmanaged_node.node {
            assert_eq!(i.key, "skill:review");
        } else {
            unreachable!();
        }
    }

    #[test]
    fn unmanaged_group_collapse_hides_items() {
        // spec: UNM-6 - the group is collapsible: with its id in `collapsed`, its
        // items disappear while the group header itself stays visible.
        let snap = snap_with_unmanaged(vec![make_unmanaged("x", ItemKind::Skill)]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let mut collapsed = HashSet::new();
        collapsed.insert("group:unmanaged".to_string());
        let flat = flatten_tree(&nodes, &HashSet::new(), &collapsed);
        let item_visible = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::UnmanagedItem(_)));
        let group_visible = flat
            .iter()
            .any(|n| matches!(&n.node, TreeNode::UnmanagedGroup));
        assert!(!item_visible, "collapsed group hides its items");
        assert!(
            group_visible,
            "the group header stays visible when collapsed"
        );
    }

    // --- TUI-50: item node expands to its dependency subtree ---

    /// Make an installed item with known direct dep keys.
    fn make_installed_with_deps(
        key: &str,
        name: &str,
        source: &str,
        kind: ItemKind,
        deps: Vec<&str>,
    ) -> SnapshotInstalled {
        SnapshotInstalled {
            key: key.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            kind,
            commit: "abc12345".to_string(),
            description: None,
            deps: deps.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn item_node_with_deps_has_dep_children_in_tree() {
        // spec: TUI-50 - an item that has dependencies gets DepChild nodes as
        // children in the built tree. The children are the dep subtree VIEW, not
        // canonical item lines.
        let review = make_installed_with_deps(
            "skill:review",
            "review",
            "src/a",
            ItemKind::Skill,
            vec!["agent:dev"],
        );
        let dev = make_installed("agent:dev", "dev", "src/a", ItemKind::Agent);
        let snap = snap_with(vec![review, dev], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        // Find the InstalledItem node for "review".
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let review_node = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"))
            .expect("review item must be in the flat tree");

        // The review node must be marked expandable (it has dep children).
        assert!(
            review_node.expandable,
            "an item with dependencies must be marked expandable: {:?}",
            review_node
        );

        // When expanded, its dep child must appear.
        let mut expanded = HashSet::new();
        expanded.insert(review_node.id.clone());
        let flat_exp = flatten_tree(&nodes, &expanded, &HashSet::new());
        let dep_child = flat_exp
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:dev"));
        assert!(
            dep_child.is_some(),
            "expanding review must show agent:dev as a DepChild: {:?}",
            flat_exp.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dep_child_is_itself_expandable_for_transitive_deps() {
        // spec: TUI-50 - a dependency child is itself expandable so the user can
        // walk the graph transitively. When the dep child node is in the `expanded`
        // set, its own deps appear as grandchild DepChild nodes.
        // Chain: skill:review -> agent:dev -> skill:build
        let review = make_installed_with_deps(
            "skill:review",
            "review",
            "src/a",
            ItemKind::Skill,
            vec!["agent:dev"],
        );
        let dev = make_installed_with_deps(
            "agent:dev",
            "dev",
            "src/a",
            ItemKind::Agent,
            vec!["skill:build"],
        );
        let build = make_installed("skill:build", "build", "src/a", ItemKind::Skill);
        let snap = snap_with(vec![review, dev, build], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        // Expand review to see agent:dev dep child.
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let review_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"))
            .map(|n| n.id.clone())
            .expect("review must be in the tree");

        let mut expanded = HashSet::new();
        expanded.insert(review_id.clone());
        let flat_one = flatten_tree(&nodes, &expanded, &HashSet::new());

        let dev_child_id = flat_one
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:dev"))
            .map(|n| n.id.clone())
            .expect("agent:dev dep child must appear when review is expanded");

        // The dep child for agent:dev must be expandable (it has its own dep).
        let dev_child_node = flat_one
            .iter()
            .find(|n| n.id == dev_child_id)
            .expect("dep child must be in flat tree");
        assert!(
            dev_child_node.expandable,
            "agent:dev dep child must be expandable (it has skill:build as a dep)"
        );

        // Expand the dep child for agent:dev to see skill:build as grandchild.
        expanded.insert(dev_child_id.clone());
        let flat_two = flatten_tree(&nodes, &expanded, &HashSet::new());
        let build_grandchild = flat_two
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "skill:build"));
        assert!(
            build_grandchild.is_some(),
            "expanding agent:dev dep child must show skill:build as grandchild: {:?}",
            flat_two.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dep_child_cycle_is_marked_and_not_expanded() {
        // spec: TUI-50 DEP-22 - a dependency that would revisit an ancestor on
        // the current path is shown as a marked back-edge (`is_cycle = true`) and
        // does NOT expand infinitely (no grandchildren for the cycle node).
        // Cycle: skill:a -> skill:b -> skill:a (self-referential through b).
        let item_a =
            make_installed_with_deps("skill:a", "a", "src/a", ItemKind::Skill, vec!["skill:b"]);
        let item_b =
            make_installed_with_deps("skill:b", "b", "src/a", ItemKind::Skill, vec!["skill:a"]);
        let snap = snap_with(vec![item_a, item_b], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        // Expand item_a.
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let a_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "a"))
            .map(|n| n.id.clone())
            .expect("skill:a must be in the tree");

        let mut expanded = HashSet::new();
        expanded.insert(a_id.clone());
        let flat_one = flatten_tree(&nodes, &expanded, &HashSet::new());

        // skill:b appears as a dep child of skill:a.
        let b_child_id = flat_one
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "skill:b" && !d.is_cycle))
            .map(|n| n.id.clone())
            .expect("skill:b must appear as a non-cycle dep child under skill:a");

        // Expand skill:b dep child: skill:a would be its dep, but skill:a is an
        // ancestor -> it must appear as a CYCLE back-edge, not a normal child.
        expanded.insert(b_child_id.clone());
        let flat_two = flatten_tree(&nodes, &expanded, &HashSet::new());

        let cycle_node = flat_two
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "skill:a"));
        assert!(
            cycle_node.is_some(),
            "skill:a must appear under skill:b's dep children: {:?}",
            flat_two.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        let cycle_node = cycle_node.unwrap();
        assert!(
            matches!(&cycle_node.node, TreeNode::DepChild(d) if d.is_cycle),
            "skill:a dep under skill:b must be marked as a cycle: {:?}",
            cycle_node.node
        );
        assert!(
            cycle_node.label.contains("cycle"),
            "cycle dep label must contain '(cycle)': {:?}",
            cycle_node.label
        );
        // The cycle node must NOT be expandable (no grandchildren).
        assert!(
            !cycle_node.expandable,
            "cycle dep child must not be expandable (prevents infinite expansion): {:?}",
            cycle_node
        );
    }

    #[test]
    fn dep_child_is_distinct_from_canonical_item_line() {
        // spec: TUI-50 - a DepChild node is a VIEW of the graph, distinct from the
        // canonical InstalledItem/AvailableItem line of the same item. Both must
        // appear in the flat tree when the parent is expanded: the canonical line
        // under its own source->kind bucket, and the dep child under its parent.
        let review = make_installed_with_deps(
            "skill:review",
            "review",
            "src/a",
            ItemKind::Skill,
            vec!["agent:dev"],
        );
        let dev = make_installed("agent:dev", "dev", "src/a", ItemKind::Agent);
        let snap = snap_with(vec![review, dev], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        // Canonical agent:dev InstalledItem must be present.
        let canonical_dev = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "dev"))
            .expect("canonical dev InstalledItem must be present in the flat tree");

        // Expand review so its dep child becomes visible.
        let review_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"))
            .map(|n| n.id.clone())
            .expect("review item must be in the tree");
        let mut expanded = HashSet::new();
        expanded.insert(review_id);
        let flat_exp = flatten_tree(&nodes, &expanded, &HashSet::new());

        // The canonical line is still an InstalledItem (not DepChild).
        assert!(
            matches!(&canonical_dev.node, TreeNode::InstalledItem(_)),
            "canonical dev node must be InstalledItem, not DepChild"
        );

        // The dep child under review is a DepChild with the same key.
        let dep_child_dev = flat_exp
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:dev"))
            .expect("dep child for agent:dev must appear under review when expanded");
        assert!(
            matches!(&dep_child_dev.node, TreeNode::DepChild(_)),
            "the dep child must be a DepChild node, not InstalledItem"
        );
        // Their node IDs must differ (they are distinct tree nodes).
        let canonical_dev_id = flat_exp
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "dev"))
            .map(|n| n.id.clone())
            .expect("canonical dev InstalledItem must still be present after expand");
        assert_ne!(
            canonical_dev_id, dep_child_dev.id,
            "canonical InstalledItem and DepChild must have distinct node IDs"
        );
    }

    #[test]
    fn item_without_deps_has_no_dep_children_and_is_not_expandable_for_deps() {
        // spec: TUI-50 - an item with no dependencies has no DepChild nodes and
        // must not be marked expandable in the flat tree from deps (though the
        // expanded set has no effect if children is empty).
        let solo = make_installed("skill:solo", "solo", "src/a", ItemKind::Skill);
        let snap = snap_with(vec![solo], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);
        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());

        let solo_node = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "solo"))
            .expect("solo item must be in the flat tree");
        assert!(
            !solo_node.expandable,
            "an item with no deps must not be expandable: {:?}",
            solo_node
        );
        // No DepChild nodes in the whole flat tree.
        assert!(
            !flat
                .iter()
                .any(|n| matches!(&n.node, TreeNode::DepChild(_))),
            "no DepChild nodes should appear when no item has deps"
        );
    }

    /// Make an available item with known direct dep keys.
    fn make_available_with_deps(
        key: &str,
        name: &str,
        source: &str,
        kind: ItemKind,
        deps: Vec<&str>,
    ) -> SnapshotAvailable {
        SnapshotAvailable {
            key: key.to_string(),
            name: name.to_string(),
            source: source.to_string(),
            kind,
            description: None,
            path: std::path::PathBuf::from(format!("/fake/{name}")),
            deps: deps.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn available_item_with_deps_has_dep_children_in_tree() {
        // spec: TUI-50 - the dep-subtree view is built for AVAILABLE items too,
        // not just installed ones: an available item with deps is expandable and
        // shows its dependency as a DepChild when expanded.
        let review = make_available_with_deps(
            "skill:review",
            "review",
            "src/a",
            ItemKind::Skill,
            vec!["agent:dev"],
        );
        let dev = make_available("agent:dev", "dev", "src/a", ItemKind::Agent);
        let snap = snap_with(vec![], vec![review, dev]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let review_node = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "review"))
            .expect("available review item must be in the flat tree");
        assert!(
            review_node.expandable,
            "an available item with deps must be marked expandable: {:?}",
            review_node
        );

        let mut expanded = HashSet::new();
        expanded.insert(review_node.id.clone());
        let flat_exp = flatten_tree(&nodes, &expanded, &HashSet::new());
        let dep_child = flat_exp
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:dev"));
        assert!(
            dep_child.is_some(),
            "expanding an available item must show its dep as a DepChild: {:?}",
            flat_exp.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn available_item_dep_child_id_distinct_from_installed_parent_dep_child() {
        // spec: TUI-50 - the dep child node id encodes the parent path, so the
        // same dependency reached from an installed parent and from an available
        // parent gets two independently-expandable nodes (no id collision that
        // would let one parent's expansion leak into the other).
        let installed_parent = make_installed_with_deps(
            "skill:a",
            "a",
            "src/a",
            ItemKind::Skill,
            vec!["agent:shared"],
        );
        let available_parent = make_available_with_deps(
            "skill:b",
            "b",
            "src/a",
            ItemKind::Skill,
            vec!["agent:shared"],
        );
        // The shared dep itself is installed; both parents reference it.
        let shared = make_installed("agent:shared", "shared", "src/a", ItemKind::Agent);
        let snap = snap_with(vec![installed_parent, shared], vec![available_parent]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let installed_a_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "a"))
            .map(|n| n.id.clone())
            .expect("installed parent a must be present");
        let available_b_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::AvailableItem(i) if i.name == "b"))
            .map(|n| n.id.clone())
            .expect("available parent b must be present");

        // Expand BOTH parents.
        let mut expanded = HashSet::new();
        expanded.insert(installed_a_id);
        expanded.insert(available_b_id);
        let flat_exp = flatten_tree(&nodes, &expanded, &HashSet::new());

        let dep_child_ids: Vec<String> = flat_exp
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:shared"))
            .map(|n| n.id.clone())
            .collect();
        assert_eq!(
            dep_child_ids.len(),
            2,
            "the shared dep must appear once under each parent: {:?}",
            flat_exp.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        assert_ne!(
            dep_child_ids[0], dep_child_ids[1],
            "dep child ids must differ per parent so expansion does not leak between them"
        );
    }

    #[test]
    fn three_node_cycle_terminates_and_marks_only_the_back_edge() {
        // spec: TUI-50 DEP-22 - a 3-node cycle a -> b -> c -> a terminates with a
        // single (cycle) back-edge at the node that revisits an ancestor, and the
        // forward edges (a->b, b->c) are NOT marked as cycles. No infinite flatten.
        let a = make_installed_with_deps("skill:a", "a", "src/a", ItemKind::Skill, vec!["skill:b"]);
        let b = make_installed_with_deps("skill:b", "b", "src/a", ItemKind::Skill, vec!["skill:c"]);
        let c = make_installed_with_deps("skill:c", "c", "src/a", ItemKind::Skill, vec!["skill:a"]);
        let snap = snap_with(vec![a, b, c], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        // Walk a -> b -> c by expanding each non-cycle dep child in turn.
        let flat0 = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let a_id = flat0
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "a"))
            .map(|n| n.id.clone())
            .expect("skill:a must be in the tree");

        let mut expanded = HashSet::new();
        expanded.insert(a_id);
        let flat1 = flatten_tree(&nodes, &expanded, &HashSet::new());
        let b_child_id = flat1
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "skill:b" && !d.is_cycle))
            .map(|n| n.id.clone())
            .expect("skill:b non-cycle dep child under a");
        expanded.insert(b_child_id);

        let flat2 = flatten_tree(&nodes, &expanded, &HashSet::new());
        let c_child_id = flat2
            .iter()
            .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "skill:c" && !d.is_cycle))
            .map(|n| n.id.clone())
            .expect("skill:c non-cycle dep child under b");
        expanded.insert(c_child_id);

        let flat3 = flatten_tree(&nodes, &expanded, &HashSet::new());
        // The back-edge: skill:a under c must be the only cycle-marked DepChild.
        let cycle_nodes: Vec<&FlatNode> = flat3
            .iter()
            .filter(|n| matches!(&n.node, TreeNode::DepChild(d) if d.is_cycle))
            .collect();
        assert_eq!(
            cycle_nodes.len(),
            1,
            "exactly one back-edge must be cycle-marked in a 3-node cycle: {:?}",
            flat3.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        assert!(
            matches!(&cycle_nodes[0].node, TreeNode::DepChild(d) if d.key == "skill:a"),
            "the back-edge must be skill:a (the ancestor revisited): {:?}",
            cycle_nodes[0].node
        );
        assert!(
            !cycle_nodes[0].expandable,
            "the cycle back-edge must not be expandable (no infinite flatten)"
        );
        // The forward edges must NOT be cycle-marked.
        for key in ["skill:b", "skill:c"] {
            let forward = flat3
                .iter()
                .find(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == key))
                .expect("forward dep child must exist");
            assert!(
                matches!(&forward.node, TreeNode::DepChild(d) if !d.is_cycle),
                "forward edge {key} must not be marked as a cycle"
            );
        }
    }

    #[test]
    fn item_dep_expansion_does_not_clobber_structural_collapse() {
        // spec: TUI-50 TUI-11 - the dep-subtree expansion (tracked in `expanded`)
        // and structural collapse of buckets (tracked in `collapsed`) are
        // independent sets and must not interfere: an item's dep child can be
        // shown while a sibling source/kind bucket is collapsed, each honoring its
        // own state.
        let review = make_installed_with_deps(
            "skill:review",
            "review",
            "src/a",
            ItemKind::Skill,
            vec!["agent:dev"],
        );
        let dev = make_installed("agent:dev", "dev", "src/a", ItemKind::Agent);
        // A second, unrelated installed item in its own kind bucket to collapse.
        let other = make_installed("rule:style", "style", "src/a", ItemKind::Rule);
        let snap = snap_with(vec![review, dev, other], vec![]);
        let nodes = build_tree(&snap, "", None, None, false, false);

        let flat = flatten_tree(&nodes, &HashSet::new(), &HashSet::new());
        let review_id = flat
            .iter()
            .find(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "review"))
            .map(|n| n.id.clone())
            .expect("review must be present");
        // Identify the rules kind bucket id to collapse it structurally.
        let rules_bucket_id = flat
            .iter()
            .find(
                |n| matches!(&n.node, TreeNode::KindBucket { kind, .. } if *kind == ItemKind::Rule),
            )
            .map(|n| n.id.clone())
            .expect("rules bucket must be present");

        let mut expanded = HashSet::new();
        expanded.insert(review_id); // dep-subtree expansion
        let mut collapsed = HashSet::new();
        collapsed.insert(rules_bucket_id); // structural collapse

        let flat_both = flatten_tree(&nodes, &expanded, &collapsed);

        // The dep child appears (expanded honored).
        assert!(
            flat_both
                .iter()
                .any(|n| matches!(&n.node, TreeNode::DepChild(d) if d.key == "agent:dev")),
            "dep child must show even while another bucket is collapsed: {:?}",
            flat_both.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        // The collapsed rules bucket hides its item (collapsed honored).
        assert!(
            !flat_both
                .iter()
                .any(|n| matches!(&n.node, TreeNode::InstalledItem(i) if i.name == "style")),
            "collapsed rules bucket must hide rule:style: {:?}",
            flat_both.iter().map(|n| &n.label).collect::<Vec<_>>()
        );
        // The rules bucket header itself stays visible.
        assert!(
            flat_both.iter().any(
                |n| matches!(&n.node, TreeNode::KindBucket { kind, .. } if *kind == ItemKind::Rule)
            ),
            "the collapsed bucket header must remain visible"
        );
    }
}
