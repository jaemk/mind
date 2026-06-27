//! Within-source dependency resolution (DEP-1..50) and installed-item
//! dependency graph (DEP-60/61/62/63).
//!
//! An item's dependencies are the siblings it names with `{{ns:name}}` tokens
//! (DEP-1, via [`crate::namespace::referenced_names`]). For a *partial* selection
//! of a source's items, [`resolve`] computes the transitive closure of those
//! references so a partial `learn` installs a working set rather than a dangling
//! one. The result is stored once (DEP-21) and exposed two ways: a display tree
//! ([`Resolution::render_tree`]) and a dependency-first install order
//! ([`Resolution::install_order`]).
//!
//! [`InstalledGraph`] provides the graph over **installed** items only (no
//! DEP-10 gating): it powers `forget`'s dependent-warning (DEP-60), `recall
//! --tree` (DEP-61), `probe --json`'s adjacency field (DEP-62), and `recall
//! --tree --json` structured output (DEP-63). Build it with [`installed_graph`]
//! and query it via [`InstalledGraph::dependents`], [`InstalledGraph::render_forest`],
//! [`InstalledGraph::render_subtree`], [`InstalledGraph::forest_nodes`], and
//! [`InstalledGraph::subtree_node`].
//!
//! This module is pure: it reads each item's text through an injected closure so
//! it can be unit-tested with synthetic content (no filesystem).
//!

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::catalog::CatalogItem;
use crate::namespace;

// ---------------------------------------------------------------------------
// Shared edge primitive
// ---------------------------------------------------------------------------

/// Build a `(source, bare_name) -> [indices]` lookup over `items`.
fn make_by_name(items: &[CatalogItem]) -> HashMap<(&str, &str), Vec<usize>> {
    let mut by_name: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        by_name
            .entry((item.source.as_str(), item.name.as_str()))
            .or_default()
            .push(i);
    }
    by_name
}

/// Compute the direct dependency node indices of `items[node]` within its own
/// source. The result is the union of:
///
/// - indices referenced by `{{ns:name}}` tokens in `text` (DEP-1),
/// - indices from the item's `requires` frontmatter entries (DEP-4), with
///   source-qualified entries skipped and kind narrowed when specified (DEP-5).
///
/// Self-edges are always skipped. The returned slice is in stable discovery
/// order, deduped.
///
/// `by_name` must have been built by [`make_by_name`] over the same `items`.
fn item_edges(
    node: usize,
    items: &[CatalogItem],
    by_name: &HashMap<(&str, &str), Vec<usize>>,
    text: &str,
) -> Vec<usize> {
    let item = &items[node];
    let mut out: Vec<usize> = Vec::new();

    // DEP-1: edges from {{ns:name}} tokens in the item's text.
    for name in namespace::referenced_names(text) {
        if let Some(matches) = by_name.get(&(item.source.as_str(), name.as_str())) {
            for &m in matches {
                // DEP-2: intra-source guaranteed by the (source, name) key.
                // Skip self-reference so it never forms a trivial loop.
                if m != node && !out.contains(&m) {
                    out.push(m);
                }
            }
        }
    }

    // DEP-4: union with explicit `requires` frontmatter entries. Source-
    // qualified entries (`owner/repo#name`) are skipped here; they are
    // rejected at install/review (DEP-5/6) but do not contribute edges in
    // the pure resolver.
    for entry in &item.requires {
        let Ok(r) = crate::resolve::parse_item_ref(entry) else {
            continue;
        };
        if r.source.is_some() {
            // Source-qualified: reject per DEP-5 (no cross-source edges).
            continue;
        }
        // Resolve against siblings in the same source, narrowing by kind
        // when a kind prefix was supplied (DEP-5).
        if let Some(matches) = by_name.get(&(item.source.as_str(), r.name.as_str())) {
            for &m in matches {
                let candidate = &items[m];
                // Narrow by kind if the ref specifies one (DEP-5).
                if r.kind.is_some_and(|k| candidate.kind != k) {
                    continue;
                }
                if m != node && !out.contains(&m) {
                    out.push(m);
                }
            }
        }
    }

    out
}

/// Return the `kind:effective_name` keys of `item`'s direct dependencies
/// (within its own source), in stable discovery order, deduped.
///
/// The dependency set is the union of `{{ns:name}}` token references (DEP-1)
/// and `requires` frontmatter entries (DEP-4), with source-qualified entries
/// skipped and kind narrowed when the entry is `kind:name` (DEP-5).
///
/// `items` is the full catalog slice (for index-based sibling lookup).
/// `read` is injected so callers and tests can supply synthetic content.
///
/// This is the primitive the DEP-62 `probe --json` adjacency field uses and
/// TUI-50 uses to list an item's children.
// Consuming shards (commands.rs probe/recall/forget) will call this; allow
// dead_code until those shards land.
#[allow(dead_code)]
pub fn direct_dependency_keys(
    item: &CatalogItem,
    items: &[CatalogItem],
    read: &impl Fn(&CatalogItem) -> String,
) -> Vec<String> {
    let by_name = make_by_name(items);
    // Find this item's index.
    let node = items
        .iter()
        .position(|it| std::ptr::eq(it, item))
        .unwrap_or(usize::MAX);
    if node == usize::MAX {
        return Vec::new();
    }
    let text = read(item);
    item_edges(node, items, &by_name, &text)
        .into_iter()
        .map(|i| items[i].key())
        .collect()
}

// ---------------------------------------------------------------------------
// resolve() -- unchanged in behavior; routes through item_edges
// ---------------------------------------------------------------------------

/// The computed dependency graph for one selection, stored so that both the
/// display tree and the install order come from it without re-analyzing
/// references (DEP-21).
pub struct Resolution {
    /// The explicitly selected roots (indices into `items`), in input order.
    roots: Vec<usize>,
    /// Adjacency: for each node index, its dependency node indices (edges
    /// item -> dep), in stable discovery order. Only populated for nodes whose
    /// source was expanded (DEP-10); a non-expanded node has no edges here.
    deps: HashMap<usize, Vec<usize>>,
    /// Indices that are dependencies pulled in beyond the explicit selection.
    pulled: HashSet<usize>,
    /// Indices already installed (present in the manifest); shown but not
    /// installed (DEP-23).
    installed: HashSet<usize>,
    /// Dependency-first install order, excluding already-installed items.
    order: Vec<usize>,
}

/// Resolve the dependency closure of `selected` over `items`.
///
/// - `items`: the full catalog (each is a unique `(source, kind, bare_name)`).
/// - `selected`: indices into `items` the user explicitly chose (the roots).
/// - `installed`: manifest keys already installed (`CatalogItem::key()` form,
///   `kind:effective_name`).
/// - `read`: returns the concatenated text of an item's files; references are
///   read from this (tests pass synthetic strings).
pub fn resolve(
    items: &[CatalogItem],
    selected: &[usize],
    installed: &HashSet<String>,
    read: impl Fn(&CatalogItem) -> String,
) -> Resolution {
    let installed_idx: HashSet<usize> = (0..items.len())
        .filter(|&i| installed.contains(&items[i].key()))
        .collect();

    // DEP-10: per source, decide whether to expand. A source is expanded only
    // when the selection is a *proper* subset of that source's items. When the
    // selection already covers every item of a source, expansion is a no-op.
    let mut source_items: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        source_items.entry(&item.source).or_default().push(i);
    }
    let selected_set: HashSet<usize> = selected.iter().copied().collect();
    let mut expand_source: HashMap<&str, bool> = HashMap::new();
    for (src, idxs) in &source_items {
        let all_selected = idxs.iter().all(|i| selected_set.contains(i));
        expand_source.insert(src, !all_selected);
    }

    // Index sibling lookup by (source, bare_name) -> node indices (DEP-2, DEP-3).
    let by_name = make_by_name(items);

    // Resolve and cache each expanded node's dependency edges, in discovery
    // order. Memoized so the closure walk visits each node's refs once (DEP-22).
    let mut deps: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut edges_of = |node: usize| -> Vec<usize> {
        if let Some(d) = deps.get(&node) {
            return d.clone();
        }
        let item = &items[node];
        let edges = if *expand_source.get(item.source.as_str()).unwrap_or(&true) {
            let text = read(item);
            item_edges(node, items, &by_name, &text)
        } else {
            Vec::new()
        };
        deps.insert(node, edges.clone());
        edges
    };

    // DFS from each root in input order. `visited` makes every node's edges
    // expand once so cycles terminate (DEP-22); `order` records dependency-first
    // discovery order (post-order: a dep is emitted before its dependent).
    let mut visited: HashSet<usize> = HashSet::new();
    let mut order: Vec<usize> = Vec::new();
    let mut closure: HashSet<usize> = HashSet::new();

    // Iterative post-order DFS preserving stable child order.
    enum Frame {
        Enter(usize),
        Exit(usize),
    }
    for &root in selected {
        if visited.contains(&root) {
            continue;
        }
        let mut stack = vec![Frame::Enter(root)];
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Enter(node) => {
                    if !visited.insert(node) {
                        continue;
                    }
                    closure.insert(node);
                    stack.push(Frame::Exit(node));
                    // Push children in reverse so they are entered in order.
                    let children = edges_of(node);
                    for &child in children.iter().rev() {
                        if !visited.contains(&child) {
                            stack.push(Frame::Enter(child));
                        }
                    }
                }
                Frame::Exit(node) => {
                    // DEP-23: already-installed items are in the closure (shown in
                    // the tree) but excluded from the install order.
                    if !installed_idx.contains(&node) {
                        order.push(node);
                    }
                }
            }
        }
    }

    let pulled: HashSet<usize> = closure
        .iter()
        .copied()
        .filter(|i| !selected_set.contains(i))
        .collect();

    Resolution {
        roots: selected.to_vec(),
        deps,
        pulled,
        installed: installed_idx,
        order,
    }
}

impl Resolution {
    /// Indices into `items` to install, dependency-first (each dependency
    /// precedes any item that depends on it), excluding items already installed
    /// (DEP-21, DEP-23). Cycle members appear in a stable discovery order
    /// (DEP-22).
    pub fn install_order(&self) -> &[usize] {
        &self.order
    }

    /// True iff the closure pulls in any item beyond the explicit selection,
    /// i.e. dependencies were added (DEP-31). Used to decide whether to prompt.
    pub fn adds_dependencies(&self) -> bool {
        !self.pulled.is_empty()
    }

    /// Render an ASCII dependency tree (DEP-21): one line per node, roots are the
    /// selected items, each transitive dependency a descendant indented by depth.
    /// Each node is tagged `[selected]`, `[dep]`, or `[installed]`; a reference
    /// back to an item already on the current path is shown as a `(cycle)`
    /// back-edge rather than expanded again (DEP-22, DEP-23).
    pub fn render_tree(&self, items: &[CatalogItem]) -> String {
        let mut out = String::new();
        for &root in &self.roots {
            let mut path: Vec<usize> = Vec::new();
            self.render_node(items, root, 0, &mut path, &mut out);
        }
        out
    }

    fn render_node(
        &self,
        items: &[CatalogItem],
        node: usize,
        depth: usize,
        path: &mut Vec<usize>,
        out: &mut String,
    ) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str("- ");
        out.push_str(&items[node].key());

        if path.contains(&node) {
            // Back-edge: do not expand again (DEP-22).
            out.push_str(" (cycle)\n");
            return;
        }

        let role = if self.installed.contains(&node) {
            "installed"
        } else if self.pulled.contains(&node) {
            "dep"
        } else {
            "selected"
        };
        out.push_str(" [");
        out.push_str(role);
        out.push_str("]\n");

        path.push(node);
        if let Some(children) = self.deps.get(&node) {
            for &child in children {
                self.render_node(items, child, depth + 1, path, out);
            }
        }
        path.pop();
    }
}

// ---------------------------------------------------------------------------
// DepNode -- structured tree node for JSON output (DEP-63)
// ---------------------------------------------------------------------------

/// A single node in the structured dependency tree returned by
/// [`InstalledGraph::forest_nodes`] and [`InstalledGraph::subtree_node`].
///
/// Serializes as:
/// - Normal node: `{"key": "kind:name", "dependencies": [...]}`
///   (`cycle` field absent, `dependencies` present, possibly empty)
/// - Cycle back-edge: `{"key": "kind:name", "cycle": true}`
///   (`dependencies` field absent, `cycle` present and `true`)
///
/// This is the machine-readable counterpart of the human `- key (cycle)` /
/// `- key` lines produced by `render_forest` / `render_subtree` (DEP-63).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DepNode {
    /// The `kind:effective_name` key for this node, identical to the key used
    /// in the human rendering and the manifest.
    pub key: String,

    /// Present and `true` only for a cycle back-edge (a reference back to an
    /// item already on the current ancestor path, DEP-22). Omitted for normal
    /// nodes. When `true`, `dependencies` is `None`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cycle: bool,

    /// The child nodes (transitive installed dependencies) of this node.
    /// `None` only for a cycle leaf (to keep the field absent in JSON).
    /// Present (possibly empty) for every normal node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<DepNode>>,
}

impl DepNode {
    pub fn normal(key: String, dependencies: Vec<DepNode>) -> Self {
        Self {
            key,
            cycle: false,
            dependencies: Some(dependencies),
        }
    }

    fn cycle_leaf(key: String) -> Self {
        Self {
            key,
            cycle: true,
            dependencies: None,
        }
    }
}

// ---------------------------------------------------------------------------
// InstalledGraph -- dependency graph over the installed set (DEP-60/61/62/63)
// ---------------------------------------------------------------------------

/// A dependency graph whose nodes are installed items and whose edges are each
/// installed item's direct dependencies restricted to other installed items.
///
/// Build with [`installed_graph`]; query with [`InstalledGraph::dependents`],
/// [`InstalledGraph::render_forest`], and [`InstalledGraph::render_subtree`].
///
/// # Forest format (`render_forest` / `render_subtree`)
///
/// Lines follow the same two-space-indent, `- <key>` bullet form as
/// [`Resolution::render_tree`]:
///
/// ```text
/// - skill:a
///   - skill:b
///     - skill:c
///   - skill:b (cycle)
/// ```
///
/// Primary roots are installed items with no incoming edge from another
/// installed item. Each root's transitive installed dependencies are nested
/// beneath it (DEP-21). A reference back to an item already on the current
/// path is rendered as `- <key> (cycle)` and not expanded further (DEP-22).
/// An item reachable only as a dependency does not appear as a primary root.
///
/// Connected components that are pure cycles (all members have in-degree >= 1)
/// are still fully rendered: the lowest-index member of each unvisited cycle
/// component is promoted as a secondary root after all primary roots, so every
/// installed item appears in the forest. Cycle back-edges are marked `(cycle)`.
///
/// No role tag (`[selected]` / `[dep]`) is added: every node is installed.
// Consuming shards wire up the callers; allow dead_code until they land.
#[allow(dead_code)]
pub struct InstalledGraph {
    /// The installed catalog items that are nodes in this graph, in stable
    /// index order (index == position in this vec).
    nodes: Vec<CatalogItem>,
    /// Adjacency list: `edges[i]` = indices of the direct installed dependencies
    /// of `nodes[i]`, in discovery order.
    edges: Vec<Vec<usize>>,
    /// Key -> node index, for lookup.
    key_to_idx: HashMap<String, usize>,
}

/// Build an [`InstalledGraph`] from a catalog and the set of installed keys.
///
/// - `items`: the full catalog (scanned from all melded sources).
/// - `installed_keys`: the `kind:effective_name` keys present in the manifest.
/// - `read`: item text reader, same contract as [`resolve`].
///
/// Edges are each installed item's direct dependency set restricted to other
/// installed items (non-installed dependencies are silently omitted from the
/// graph edges, though they may be warned about separately at `forget` time).
#[allow(dead_code)]
pub fn installed_graph(
    items: &[CatalogItem],
    installed_keys: &HashSet<String>,
    read: impl Fn(&CatalogItem) -> String,
) -> InstalledGraph {
    // Filter catalog to installed items only.
    let nodes: Vec<CatalogItem> = items
        .iter()
        .filter(|it| installed_keys.contains(&it.key()))
        .cloned()
        .collect();

    // Build key -> index for the installed node set.
    let key_to_idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, it)| (it.key(), i))
        .collect();

    // Build the full catalog by_name lookup (edges_of uses the full catalog for
    // resolution; then we restrict targets to installed nodes).
    let by_name = make_by_name(items);

    // For each installed node, compute its direct dependency edges, then keep
    // only those that are also installed.
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    // Build a parallel index map: full-catalog index -> installed-node index.
    let full_key_to_installed: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(installed_i, it)| (it.key(), installed_i))
        .collect();

    // We need to call item_edges using the full catalog indices, then translate.
    // Build a map from installed node key -> full catalog index.
    let installed_to_full: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| installed_keys.contains(&it.key()))
        .map(|(full_i, it)| (it.key(), full_i))
        .collect();

    for (installed_i, node_item) in nodes.iter().enumerate() {
        let full_i = match installed_to_full.get(&node_item.key()) {
            Some(&idx) => idx,
            None => continue,
        };
        let text = read(node_item);
        let dep_full_indices = item_edges(full_i, items, &by_name, &text);

        for dep_full_idx in dep_full_indices {
            let dep_key = items[dep_full_idx].key();
            if let Some(&dep_installed_i) = full_key_to_installed.get(&dep_key)
                && dep_installed_i != installed_i
                && !edges[installed_i].contains(&dep_installed_i)
            {
                edges[installed_i].push(dep_installed_i);
            }
        }
    }

    InstalledGraph {
        nodes,
        edges,
        key_to_idx,
    }
}

// Consuming shards wire up the callers; allow dead_code until they land.
#[allow(dead_code)]
impl InstalledGraph {
    /// Return the `kind:effective_name` keys of installed items that directly
    /// depend on `target_key` (i.e. items whose dependency set includes the
    /// target). This is the "who would break if I remove this?" query for
    /// `forget` (DEP-60).
    ///
    /// Returns an empty vec when nothing installed depends on `target_key`, or
    /// when `target_key` is not a node in this graph.
    /// Result is in stable index order, deduped.
    pub fn dependents(&self, target_key: &str) -> Vec<String> {
        let Some(&target_idx) = self.key_to_idx.get(target_key) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, dep_list) in self.edges.iter().enumerate() {
            if dep_list.contains(&target_idx) {
                let key = self.nodes[i].key();
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
        out
    }

    /// Render installed items as a dependency forest (DEP-61).
    ///
    /// Primary roots are installed items with no incoming edge from another
    /// installed item. Each root's transitive installed dependencies are nested
    /// beneath it using two-space indentation per depth level, with `- <key>`
    /// bullets. A reference back to a node already on the current path is
    /// rendered as `- <key> (cycle)` and not expanded again (DEP-22).
    ///
    /// An item reachable only as a dependency of another installed item is NOT
    /// shown as its own primary root; it appears only nested under its
    /// dependents.
    ///
    /// Connected components that are pure cycles (all members have positive
    /// in-degree within the installed set, so no natural root exists) are
    /// still fully rendered. After all primary roots are rendered, any node
    /// not yet visited is promoted to a secondary root in stable index order
    /// and its subtree rendered with cycle back-edges as usual. This
    /// guarantees every installed item appears in the forest output.
    ///
    /// See the [`InstalledGraph`] doc comment for the exact line format.
    pub fn render_forest(&self) -> String {
        // Compute in-degree (number of installed items that depend on each node).
        let mut in_degree = vec![0usize; self.nodes.len()];
        for dep_list in &self.edges {
            for &dep_idx in dep_list {
                in_degree[dep_idx] += 1;
            }
        }

        let mut out = String::new();
        // Track which node indices have been emitted (as a root or as a
        // descendant of one). A node is "emitted" once render_installed_node
        // starts on it at depth 0; descendants are reached recursively but we
        // only need to avoid re-promoting them as secondary roots.
        let mut emitted: HashSet<usize> = HashSet::new();

        // Pass 1: natural roots (in-degree == 0).
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                // Mark all nodes reachable from this root as emitted via a
                // pre-pass DFS so that cycle-component members reachable from
                // a real root are not re-promoted.
                self.mark_reachable(i, &mut emitted);
                let mut path = Vec::new();
                self.render_installed_node(i, 0, &mut path, &mut out);
            }
        }

        // Pass 2: promote the lowest-index unvisited node of each all-cycle
        // component as a secondary root, in stable index order.
        for i in 0..self.nodes.len() {
            if !emitted.contains(&i) {
                self.mark_reachable(i, &mut emitted);
                let mut path = Vec::new();
                self.render_installed_node(i, 0, &mut path, &mut out);
            }
        }

        out
    }

    /// DFS to mark all nodes reachable from `start` as emitted. Used by
    /// [`render_forest`] to track which nodes have been covered before
    /// promoting cycle-component stragglers.
    fn mark_reachable(&self, start: usize, emitted: &mut HashSet<usize>) {
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if emitted.insert(node) {
                for &child in &self.edges[node] {
                    if !emitted.contains(&child) {
                        stack.push(child);
                    }
                }
            }
        }
    }

    /// Render the subtree rooted at `root_key` (DEP-61 scoped variant for
    /// `recall <item> --tree`). Returns `None` when `root_key` is not a node
    /// in this graph (not installed).
    ///
    /// The format is identical to a single root's portion of [`render_forest`]:
    /// the root item at depth 0, its transitive installed dependencies nested
    /// beneath it, cycles marked `(cycle)`.
    pub fn render_subtree(&self, root_key: &str) -> Option<String> {
        let &root_idx = self.key_to_idx.get(root_key)?;
        let mut out = String::new();
        let mut path = Vec::new();
        self.render_installed_node(root_idx, 0, &mut path, &mut out);
        Some(out)
    }

    /// Recursive (path-tracked) renderer for one installed node, reused by
    /// both [`render_forest`] and [`render_subtree`].
    fn render_installed_node(
        &self,
        node: usize,
        depth: usize,
        path: &mut Vec<usize>,
        out: &mut String,
    ) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str("- ");
        out.push_str(&self.nodes[node].key());

        if path.contains(&node) {
            // Back-edge: do not expand again (DEP-22).
            out.push_str(" (cycle)\n");
            return;
        }

        out.push('\n');

        path.push(node);
        for &child in &self.edges[node] {
            self.render_installed_node(child, depth + 1, path, out);
        }
        path.pop();
    }

    /// Return the installed dependency forest as a vec of structured root
    /// nodes (DEP-63). Produces the same roots and cycle-component promotion
    /// as [`render_forest`], but as [`DepNode`] data rather than a string.
    ///
    /// A normal node carries `{"key": ..., "dependencies": [...]}`.
    /// A cycle back-edge carries `{"key": ..., "cycle": true}` (no
    /// `dependencies` field) and is not expanded further (DEP-22).
    pub fn forest_nodes(&self) -> Vec<DepNode> {
        // spec: DEP-63
        let mut in_degree = vec![0usize; self.nodes.len()];
        for dep_list in &self.edges {
            for &dep_idx in dep_list {
                in_degree[dep_idx] += 1;
            }
        }

        let mut emitted: HashSet<usize> = HashSet::new();
        let mut roots: Vec<DepNode> = Vec::new();

        // Pass 1: natural roots (in-degree == 0).
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                self.mark_reachable(i, &mut emitted);
                let mut path = Vec::new();
                roots.push(self.build_node(i, &mut path));
            }
        }

        // Pass 2: promote lowest-index unvisited node of each all-cycle
        // component so every installed item appears (mirrors render_forest).
        for i in 0..self.nodes.len() {
            if !emitted.contains(&i) {
                self.mark_reachable(i, &mut emitted);
                let mut path = Vec::new();
                roots.push(self.build_node(i, &mut path));
            }
        }

        roots
    }

    /// Return the subtree rooted at `root_key` as a single structured
    /// [`DepNode`] (DEP-63 scoped variant for `recall <item> --tree --json`).
    /// Returns `None` when `root_key` is not a node in this graph (not installed).
    pub fn subtree_node(&self, root_key: &str) -> Option<DepNode> {
        // spec: DEP-63
        let &root_idx = self.key_to_idx.get(root_key)?;
        let mut path = Vec::new();
        Some(self.build_node(root_idx, &mut path))
    }

    /// Recursive (path-tracked) builder for one structured [`DepNode`].
    /// Mirrors the traversal of [`render_installed_node`] exactly:
    /// same path-based cycle detection (DEP-22), same child expansion.
    fn build_node(&self, node: usize, path: &mut Vec<usize>) -> DepNode {
        let key = self.nodes[node].key();

        if path.contains(&node) {
            // Back-edge: cycle leaf, not expanded (DEP-22).
            return DepNode::cycle_leaf(key);
        }

        path.push(node);
        let children: Vec<DepNode> = self.edges[node]
            .iter()
            .map(|&child| self.build_node(child, path))
            .collect();
        path.pop();

        DepNode::normal(key, children)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Build a synthetic catalog item.
    fn item(kind: ItemKind, name: &str, source: &str) -> CatalogItem {
        CatalogItem {
            kind,
            name: name.to_string(),
            source: source.to_string(),
            prefix: None,
            path: PathBuf::from("/tmp/fake"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            hooks: Vec::new(),
        }
    }

    /// A `read` closure backed by a name->content map (keyed by `kind:name`).
    fn reader(map: HashMap<String, String>) -> impl Fn(&CatalogItem) -> String {
        move |it: &CatalogItem| -> String {
            map.get(&format!("{}:{}", it.kind.as_str(), it.name))
                .cloned()
                .unwrap_or_default()
        }
    }

    fn no_installed() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn skill_pulls_referenced_agent_before_itself() {
        // spec: DEP-1 DEP-3 DEP-20 DEP-21
        // A skill referencing an agent pulls the agent into the install order,
        // and the agent (the dependency) precedes the skill (the dependent).
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "hand off to {{ns:test}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        // Roots are the explicitly selected items (DEP-20).
        assert_eq!(r.roots, vec![0]);
        // Dependency-first: agent (1) before skill (0).
        assert_eq!(r.install_order(), &[1, 0]);
        assert!(r.adds_dependencies());
    }

    #[test]
    fn references_never_cross_sources() {
        // spec: DEP-2
        // A token resolves only within the referencing item's own source. An
        // identically named sibling in a different source is never pulled in.
        let items = vec![
            item(ItemKind::Skill, "review", "a"), // selected
            item(ItemKind::Agent, "test", "b"),   // same name, other source
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "see {{ns:test}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        // Only the selected item installs; the cross-source `test` is not pulled.
        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
    }

    #[test]
    fn transitive_chain_resolves_dependency_first() {
        // spec: DEP-11 DEP-21
        // a -> b -> c: selecting a brings in b and c, with c before b before a.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:c}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        assert_eq!(r.install_order(), &[2, 1, 0]);
        assert!(r.adds_dependencies());
    }

    #[test]
    fn selecting_whole_source_adds_nothing() {
        // spec: DEP-10
        // When the selection covers every item of a source, resolution is a
        // no-op for that source: its closure is exactly the selected items and
        // no dependency is added even though references exist.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Agent, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        // Both items selected -> full coverage of source "s".
        let r = resolve(&items, &[0, 1], &no_installed(), reader(content));

        assert!(!r.adds_dependencies());
        // Order is the selected set, unexpanded but still dependency-stable.
        let mut order = r.install_order().to_vec();
        order.sort_unstable();
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn no_token_selection_is_unchanged() {
        // spec: DEP-12
        // A selection whose items reference nothing installs exactly the
        // selected set, with adds_dependencies() == false.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let content = HashMap::new(); // no tokens at all
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
    }

    #[test]
    fn cycle_terminates_with_back_edge_and_stable_order() {
        // spec: DEP-22
        // a -> b -> a is a cycle. Resolution visits each node once and
        // terminates; install order holds both members in stable discovery
        // order, and the tree marks the back-edge as (cycle) rather than
        // expanding a again.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        // Both members present exactly once; b (the dep) before a.
        let order = r.install_order().to_vec();
        assert_eq!(order.len(), 2);
        assert!(order.contains(&0) && order.contains(&1));
        assert_eq!(order, vec![1, 0]);

        // The tree shows the back-edge to `a` as a cycle, not an infinite expand.
        let tree = r.render_tree(&items);
        assert!(
            tree.contains("(cycle)"),
            "tree should mark a back-edge: {tree}"
        );
    }

    #[test]
    fn already_installed_dep_excluded_from_order_but_shown_in_tree() {
        // spec: DEP-23
        // The referenced agent is already installed: it is excluded from the
        // install order yet still appears in the tree marked [installed].
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let mut installed = HashSet::new();
        installed.insert("agent:test".to_string());
        let r = resolve(&items, &[0], &installed, reader(content));

        // The agent is not re-installed.
        assert_eq!(r.install_order(), &[0]);
        // But it is shown in the tree, marked installed.
        let tree = r.render_tree(&items);
        assert!(
            tree.contains("agent:test [installed]"),
            "installed dep must be shown marked: {tree}"
        );
    }

    #[test]
    fn tree_roots_are_selected_items() {
        // spec: DEP-20 DEP-21
        // The graph roots (top-level tree lines) are exactly the selected items,
        // each marked [selected]; the pulled-in dependency is a descendant
        // marked [dep], and the stored structure yields both tree and order.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let tree = r.render_tree(&items);
        // Root line (depth 0) is the selected skill.
        assert!(
            tree.starts_with("- skill:review [selected]\n"),
            "root must be the selected item: {tree}"
        );
        // The dependency is an indented descendant marked [dep].
        assert!(
            tree.contains("  - agent:test [dep]\n"),
            "dependency must be an indented [dep] descendant: {tree}"
        );
    }

    #[test]
    fn one_token_can_match_several_siblings_across_kinds() {
        // spec: DEP-3
        // A bare name shared by two kinds in the same source resolves to both;
        // each is a dependency.
        let items = vec![
            item(ItemKind::Skill, "root", "s"),
            item(ItemKind::Agent, "shared", "s"),
            item(ItemKind::Rule, "shared", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:root".into(), "{{ns:shared}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let order = r.install_order();
        // Both `shared` siblings precede the root.
        assert!(order.contains(&1) && order.contains(&2));
        assert_eq!(*order.last().unwrap(), 0);
    }

    #[test]
    fn diamond_shares_one_install_of_the_common_dependency() {
        // spec: DEP-11 DEP-21
        // a -> b, a -> c, b -> d, c -> d. The shared dependency `d` is installed
        // exactly once (a node is visited once across the whole closure, not once
        // per path) and ordered before both b and c that depend on it; a is last.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
            item(ItemKind::Skill, "d", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}} {{ns:c}}".into());
        content.insert("skill:b".into(), "{{ns:d}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let order = r.install_order().to_vec();
        // d appears exactly once across the whole install order.
        assert_eq!(order.iter().filter(|&&n| n == 3).count(), 1);
        // Every dependency precedes its dependent.
        let pos = |n: usize| order.iter().position(|&x| x == n).unwrap();
        assert!(pos(3) < pos(1), "d before b: {order:?}");
        assert!(pos(3) < pos(2), "d before c: {order:?}");
        assert!(pos(1) < pos(0), "b before a: {order:?}");
        assert!(pos(2) < pos(0), "c before a: {order:?}");
        // Exact order is the deterministic post-order DFS result.
        assert_eq!(order, vec![3, 1, 2, 0]);
        // All four install; a is the only selected root.
        assert_eq!(order.len(), 4);
        assert!(r.adds_dependencies());
    }

    #[test]
    fn multiple_roots_across_sources_resolve_per_source_independently() {
        // spec: DEP-2 DEP-10
        // Two roots from two different sources are resolved in one call. Each
        // source expands its own proper-subset closure; a token never crosses
        // into the other source even though a same-named sibling exists there.
        // Source "x": x0 (selected) -> x1 (dep), x2 not selected -> proper subset.
        // Source "y": y0 (selected) -> y1 (dep), y2 not selected -> proper subset.
        let items = vec![
            item(ItemKind::Skill, "root", "x"),   // 0
            item(ItemKind::Agent, "helper", "x"), // 1
            item(ItemKind::Rule, "spare", "x"),   // 2 (keeps x a proper subset)
            item(ItemKind::Skill, "root", "y"),   // 3 same bare name, other source
            item(ItemKind::Agent, "helper", "y"), // 4 same bare name, other source
            item(ItemKind::Rule, "spare", "y"),   // 5 (keeps y a proper subset)
        ];
        let mut content = HashMap::new();
        // Both roots reference "helper"; each must bind only to its own source.
        content.insert("skill:root".into(), "{{ns:helper}}".into());
        let r = resolve(&items, &[0, 3], &no_installed(), reader(content));

        let order = r.install_order().to_vec();
        // Closure is exactly {x0,x1,y0,y1}; the spares (2,5) are untouched.
        assert!(!order.contains(&2) && !order.contains(&5));
        let pos = |n: usize| order.iter().position(|&x| x == n);
        // Each source's dependency precedes its own root.
        assert!(
            pos(1).unwrap() < pos(0).unwrap(),
            "x-helper before x-root: {order:?}"
        );
        assert!(
            pos(4).unwrap() < pos(3).unwrap(),
            "y-helper before y-root: {order:?}"
        );
        // Roots are processed in input order: source x's closure, then source y's.
        assert_eq!(order, vec![1, 0, 4, 3]);
        assert!(r.adds_dependencies());
    }

    #[test]
    fn selected_root_that_is_already_installed_is_a_root_but_not_reinstalled() {
        // spec: DEP-23
        // A selected root that is itself already installed is excluded from the
        // install order, yet still rendered as a top-level root marked
        // [installed] (it was explicitly chosen). Its not-yet-installed
        // dependency is still pulled and ordered.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let mut installed = HashSet::new();
        installed.insert("skill:review".to_string()); // the ROOT is installed
        let r = resolve(&items, &[0], &installed, reader(content));

        // Root excluded from install; the dependency `test` still installs.
        assert_eq!(r.install_order(), &[1]);
        // The tree still shows the selected root, marked installed (not selected).
        let tree = r.render_tree(&items);
        assert!(
            tree.starts_with("- skill:review [installed]\n"),
            "installed selected root must still head the tree: {tree}"
        );
        assert!(
            tree.contains("  - agent:test [dep]\n"),
            "dep of an installed root is still shown: {tree}"
        );
    }

    #[test]
    fn adds_dependencies_true_even_when_pulled_dep_is_already_installed() {
        // spec: DEP-23 DEP-31
        // The only item pulled beyond the selection is already installed, so the
        // install order adds nothing new. adds_dependencies() is still true:
        // something beyond the explicit selection was brought into the closure
        // and must be shown in the tree, so `learn` prompts (DEP-31) rather than
        // installing silently. This pins the intended interaction.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let mut installed = HashSet::new();
        installed.insert("agent:test".to_string());
        let r = resolve(&items, &[0], &installed, reader(content));

        // Nothing new installs (the dep is already there)...
        assert_eq!(r.install_order(), &[0]);
        // ...but the closure still pulled in `test`, so we surface/prompt.
        assert!(
            r.adds_dependencies(),
            "a pulled-in (already-installed) dep still counts as adding deps"
        );
    }

    #[test]
    fn render_tree_exact_nested_format_is_locked() {
        // spec: DEP-21
        // Lock the full multi-line tree string for a two-level nesting: two-space
        // indent per depth, "- " bullet, item key, then a [role] tag. The shared
        // dependency in a diamond is rendered once per path (the tree mirrors the
        // graph structure, not the deduplicated install order).
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
            item(ItemKind::Skill, "d", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}} {{ns:c}}".into());
        content.insert("skill:b".into(), "{{ns:d}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let expected = "\
- skill:a [selected]
  - skill:b [dep]
    - skill:d [dep]
  - skill:c [dep]
    - skill:d [dep]
";
        assert_eq!(r.render_tree(&items), expected);
    }

    #[test]
    fn render_tree_exact_cycle_back_edge_format_is_locked() {
        // spec: DEP-21 DEP-22
        // a -> b -> a. Lock the full tree string: the back-edge to a node already
        // on the current path is rendered as "(cycle)" with no [role] tag and not
        // expanded further.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let expected = "\
- skill:a [selected]
  - skill:b [dep]
    - skill:a (cycle)
";
        assert_eq!(r.render_tree(&items), expected);
    }

    #[test]
    fn token_with_no_matching_sibling_is_a_non_edge() {
        // spec: DEP-1
        // A {{ns:typo}} token that names no sibling in the source forms no
        // dependency edge here: resolve silently ignores it (typo detection is
        // `expand`'s job at install time, not the resolver's). The selection
        // installs alone and adds_dependencies() is false.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "see {{ns:nonexistent}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
    }

    #[test]
    fn self_reference_token_forms_no_edge() {
        // spec: DEP-1 DEP-22
        // An item whose text references its own bare name resolves to no edge
        // (the resolver skips m == node), so it never forms a trivial self-cycle
        // and installs exactly once with no pulled dependency.
        let items = vec![item(ItemKind::Skill, "solo", "s")];
        let mut content = HashMap::new();
        content.insert("skill:solo".into(), "I call {{ns:solo}} recursively".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
        // Tree is a single self-referencing root with no (cycle) child.
        assert_eq!(r.render_tree(&items), "- skill:solo [selected]\n");
    }

    // ---- DEP-4: `requires` entries union with {{ns:}} edges ----------------

    #[test]
    fn requires_entry_adds_a_dependency_edge() {
        // spec: DEP-4
        // A `requires:` entry on a skill adds its dependency to the resolution
        // graph exactly like a {{ns:}} token does, placing the dep before the
        // dependent in the install order.
        let mut skill = item(ItemKind::Skill, "review", "s");
        skill.requires = vec!["agent:test".to_string()];
        let items = vec![skill, item(ItemKind::Agent, "test", "s")];

        // No {{ns:}} tokens in the text; the edge comes purely from `requires`.
        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));

        assert_eq!(
            r.install_order(),
            &[1, 0],
            "dep (1) must precede the skill (0)"
        );
        assert!(r.adds_dependencies());
    }

    #[test]
    fn requires_and_token_same_dep_deduped_to_one_edge() {
        // spec: DEP-4
        // An item that both declares `requires: agent:test` AND has a {{ns:test}}
        // token must yield only one dependency edge (the union is deduped).
        let mut skill = item(ItemKind::Skill, "review", "s");
        skill.requires = vec!["agent:test".to_string()];
        let items = vec![skill, item(ItemKind::Agent, "test", "s")];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());

        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let order = r.install_order().to_vec();
        // agent:test (1) appears exactly once.
        assert_eq!(
            order.iter().filter(|&&n| n == 1).count(),
            1,
            "dep must appear once"
        );
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn requires_bare_name_resolves_across_all_kinds() {
        // spec: DEP-4 DEP-5
        // A bare `name` in `requires` (no kind prefix) resolves to every sibling
        // sharing that name, across all kinds, the same way {{ns:name}} does.
        let mut skill = item(ItemKind::Skill, "root", "s");
        skill.requires = vec!["shared".to_string()];
        let items = vec![
            skill,
            item(ItemKind::Agent, "shared", "s"),
            item(ItemKind::Rule, "shared", "s"),
        ];

        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));

        let order = r.install_order().to_vec();
        // Both `shared` kinds must precede the root.
        assert!(
            order.contains(&1) && order.contains(&2),
            "both shared siblings pulled: {order:?}"
        );
        assert_eq!(*order.last().unwrap(), 0, "root is last: {order:?}");
    }

    #[test]
    fn requires_kind_prefix_narrows_to_that_kind() {
        // spec: DEP-5
        // A `kind:name` ref in `requires` narrows to that kind only; sibling of
        // a different kind with the same name is not pulled.
        let mut skill = item(ItemKind::Skill, "root", "s");
        skill.requires = vec!["agent:shared".to_string()];
        let items = vec![
            skill,
            item(ItemKind::Agent, "shared", "s"), // should be pulled
            item(ItemKind::Rule, "shared", "s"),  // same bare name, different kind: not pulled
        ];

        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));

        let order = r.install_order().to_vec();
        assert!(order.contains(&1), "agent:shared must be pulled: {order:?}");
        assert!(
            !order.contains(&2),
            "rule:shared must NOT be pulled: {order:?}"
        );
    }

    #[test]
    fn requires_source_qualified_entry_contributes_no_edge() {
        // spec: DEP-5
        // A source-qualified `owner/repo#name` entry in `requires` must be
        // skipped by the resolver (validation catches it at install/review).
        let mut skill = item(ItemKind::Skill, "root", "s");
        skill.requires = vec!["owner/repo#agent:test".to_string()];
        let items = vec![skill, item(ItemKind::Agent, "test", "s")];

        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));

        assert_eq!(
            r.install_order(),
            &[0],
            "source-qualified entry must not pull the dep"
        );
        assert!(!r.adds_dependencies());
    }

    #[test]
    fn same_bare_name_different_kind_is_a_distinct_dependency() {
        // spec: DEP-3
        // Depending on a sibling by bare name binds to every kind that shares the
        // name. A skill referencing "test" pulls BOTH an agent:test and a
        // rule:test (distinct identities), each before the dependent.
        let items = vec![
            item(ItemKind::Skill, "root", "s"),
            item(ItemKind::Agent, "test", "s"),
            item(ItemKind::Rule, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:root".into(), "{{ns:test}}".into());
        let r = resolve(&items, &[0], &no_installed(), reader(content));

        let order = r.install_order().to_vec();
        assert_eq!(order.len(), 3);
        let pos = |n: usize| order.iter().position(|&x| x == n).unwrap();
        assert!(
            pos(1) < pos(0) && pos(2) < pos(0),
            "both kinds before root: {order:?}"
        );
        // Both distinct-kind deps render under the root.
        let tree = r.render_tree(&items);
        assert!(tree.contains("  - agent:test [dep]\n"), "{tree}");
        assert!(tree.contains("  - rule:test [dep]\n"), "{tree}");
    }

    #[test]
    fn requires_chain_resolves_transitively_dependency_first() {
        // spec: DEP-4 DEP-11
        // A transitive `requires` chain a -> b -> c (each edge declared purely via
        // `requires`, no {{ns:}} tokens) must install c before b before a, exactly
        // like a token chain. Pins that the closure walk follows `requires` edges
        // transitively, not just the first hop.
        let mut a = item(ItemKind::Skill, "a", "s");
        a.requires = vec!["skill:b".to_string()];
        let mut b = item(ItemKind::Skill, "b", "s");
        b.requires = vec!["skill:c".to_string()];
        let c = item(ItemKind::Skill, "c", "s");
        let items = vec![a, b, c];

        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));
        assert_eq!(
            r.install_order(),
            &[2, 1, 0],
            "transitive requires chain must order c before b before a"
        );
        assert!(r.adds_dependencies());
    }

    #[test]
    fn empty_requires_yields_no_edge() {
        // spec: DEP-4
        // An item with an empty `requires` vec (the catalog representation of an
        // absent or whitespace-only `requires:` scalar) contributes no edge and no
        // error: the selection installs alone.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        // requires stays empty (the default); no {{ns:}} tokens either.
        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));
        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
    }

    #[test]
    fn requires_edge_is_a_noop_when_whole_source_selected() {
        // spec: DEP-4 DEP-10
        // When the selection already covers every item of the source, a `requires`
        // edge adds nothing (the same no-op DEP-10 grants a {{ns:}} edge): every
        // referent is installed regardless, so adds_dependencies() is false.
        let mut a = item(ItemKind::Skill, "a", "s");
        a.requires = vec!["agent:b".to_string()];
        let items = vec![a, item(ItemKind::Agent, "b", "s")];

        // Both items selected -> full coverage of source "s".
        let r = resolve(&items, &[0, 1], &no_installed(), reader(HashMap::new()));
        assert!(
            !r.adds_dependencies(),
            "a requires edge must be a no-op under full-source selection"
        );
        let mut order = r.install_order().to_vec();
        order.sort_unstable();
        assert_eq!(order, vec![0, 1]);
    }

    #[test]
    fn requires_self_reference_forms_no_edge() {
        // spec: DEP-4 DEP-22
        // An item whose `requires` names its own identity must form no edge (the
        // resolver skips m == node), so it never becomes a trivial self-cycle and
        // installs exactly once with no pulled dependency.
        let mut solo = item(ItemKind::Skill, "solo", "s");
        solo.requires = vec!["skill:solo".to_string()];
        let items = vec![solo];

        let r = resolve(&items, &[0], &no_installed(), reader(HashMap::new()));
        assert_eq!(r.install_order(), &[0]);
        assert!(!r.adds_dependencies());
        assert_eq!(r.render_tree(&items), "- skill:solo [selected]\n");
    }

    #[test]
    fn requires_already_installed_dep_excluded_but_still_pulled() {
        // spec: DEP-4 DEP-23
        // A `requires`-only edge to a dep that is already installed: the dep is
        // shown but not reinstalled (excluded from the order), yet still pulled
        // into the closure so adds_dependencies() is true and `learn` prompts.
        let mut review = item(ItemKind::Skill, "review", "s");
        review.requires = vec!["agent:test".to_string()];
        let items = vec![review, item(ItemKind::Agent, "test", "s")];
        let mut installed = HashSet::new();
        installed.insert("agent:test".to_string());

        let r = resolve(&items, &[0], &installed, reader(HashMap::new()));
        assert_eq!(
            r.install_order(),
            &[0],
            "an already-installed requires dep must not be reinstalled"
        );
        assert!(
            r.adds_dependencies(),
            "the pulled-in (installed) requires dep still counts as adding deps"
        );
        let tree = r.render_tree(&items);
        assert!(
            tree.contains("agent:test [installed]"),
            "installed requires dep must be shown marked: {tree}"
        );
    }

    // ---- direct_dependency_keys --------------------------------------------

    #[test]
    fn direct_dependency_keys_ns_token_edge() {
        // spec: DEP-1
        // A {{ns:name}} token in the item's text yields the sibling's key.
        let items = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "hand off to {{ns:test}}".into());
        let read = reader(content);
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(keys, vec!["agent:test".to_string()]);
    }

    #[test]
    fn direct_dependency_keys_requires_edge() {
        // spec: DEP-4
        // A `requires` entry on the item yields the sibling's key even without a
        // token in the text.
        let mut skill = item(ItemKind::Skill, "review", "s");
        skill.requires = vec!["agent:test".to_string()];
        let items = vec![skill, item(ItemKind::Agent, "test", "s")];
        let read = reader(HashMap::new());
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(keys, vec!["agent:test".to_string()]);
    }

    #[test]
    fn direct_dependency_keys_union_deduped() {
        // spec: DEP-4
        // When both a token and a requires entry name the same sibling, the key
        // appears exactly once (deduped union).
        let mut skill = item(ItemKind::Skill, "review", "s");
        skill.requires = vec!["agent:test".to_string()];
        let items = vec![skill, item(ItemKind::Agent, "test", "s")];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let read = reader(content);
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(keys, vec!["agent:test".to_string()]);
        assert_eq!(keys.len(), 1, "must be deduped to one key");
    }

    #[test]
    fn direct_dependency_keys_kind_narrowing() {
        // spec: DEP-4 DEP-5
        // A `kind:name` requires entry narrows to that kind; a same-named sibling
        // of a different kind is not included.
        let mut skill = item(ItemKind::Skill, "root", "s");
        skill.requires = vec!["agent:shared".to_string()];
        let items = vec![
            skill,
            item(ItemKind::Agent, "shared", "s"),
            item(ItemKind::Rule, "shared", "s"),
        ];
        let read = reader(HashMap::new());
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(keys, vec!["agent:shared".to_string()]);
        assert!(
            !keys.contains(&"rule:shared".to_string()),
            "rule:shared must not be included with kind-narrowed requires"
        );
    }

    #[test]
    fn direct_dependency_keys_within_source_only() {
        // spec: DEP-2
        // A token referring to a same-named item in a different source yields no key.
        let items = vec![
            item(ItemKind::Skill, "review", "a"),
            item(ItemKind::Agent, "test", "b"), // different source
        ];
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let read = reader(content);
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert!(
            keys.is_empty(),
            "cross-source token must yield no dependency key"
        );
    }

    // ---- InstalledGraph: dependents ----------------------------------------

    #[test]
    fn installed_dependents_direct_dep_reported() {
        // spec: DEP-1
        // a depends on b (via token): dependents("...b") returns ["skill:a"].
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let deps = g.dependents("skill:b");
        assert_eq!(deps, vec!["skill:a".to_string()]);
    }

    #[test]
    fn installed_dependents_non_depended_item_returns_empty() {
        // spec: DEP-1
        // An installed item that nothing depends on has an empty dependents list.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        // Nothing depends on "a"; a is the dependent, not the dependency.
        let deps = g.dependents("skill:a");
        assert!(
            deps.is_empty(),
            "nothing depends on a, so dependents must be empty: {deps:?}"
        );
    }

    #[test]
    fn installed_dependents_non_installed_edge_not_shown() {
        // spec: DEP-1
        // a -> b, but b is NOT installed. The graph restricts edges to installed
        // nodes only, so dependents("skill:b") returns empty (b is not a node).
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        // b is NOT installed

        let g = installed_graph(&items, &installed_keys, reader(content));
        // b is not a node, so asking for its dependents returns empty.
        let deps = g.dependents("skill:b");
        assert!(
            deps.is_empty(),
            "non-installed item is not a graph node: {deps:?}"
        );
    }

    #[test]
    fn installed_dependents_reverse_direction_only() {
        // spec: DEP-1
        // a -> b (a depends on b). b is NOT a dependent of a; only a is a
        // dependent of b.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        // b is not a dependent of a (b has no edge to a).
        assert!(
            !g.dependents("skill:a").contains(&"skill:b".to_string()),
            "b must not appear as a dependent of a"
        );
    }

    #[test]
    fn installed_dependents_requires_edge() {
        // spec: DEP-4
        // a requires b: dependents("skill:b") includes "skill:a".
        let mut a = item(ItemKind::Skill, "a", "s");
        a.requires = vec!["skill:b".to_string()];
        let items = vec![a, item(ItemKind::Skill, "b", "s")];
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(HashMap::new()));
        let deps = g.dependents("skill:b");
        assert_eq!(deps, vec!["skill:a".to_string()]);
    }

    // ---- InstalledGraph: render_forest -------------------------------------

    #[test]
    fn forest_roots_are_items_with_no_incoming_edge() {
        // spec: DEP-21
        // a -> b: a is a root (nothing depends on it); b is not a root (a
        // depends on b), so it appears only nested under a.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let forest = g.render_forest();
        // a is the root; b is nested under it, not a separate root.
        assert!(
            forest.starts_with("- skill:a\n"),
            "a must be the root line: {forest:?}"
        );
        assert!(
            forest.contains("  - skill:b\n"),
            "b must be nested under a: {forest:?}"
        );
        // b must NOT appear as its own root (no line starting with "- skill:b").
        let root_b = forest.lines().any(|l| l == "- skill:b");
        assert!(!root_b, "b must not be its own root line: {forest:?}");
    }

    #[test]
    fn forest_transitive_nesting() {
        // spec: DEP-21
        // a -> b -> c: the forest nests c under b under a.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:c}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let expected = "- skill:a\n  - skill:b\n    - skill:c\n";
        assert_eq!(g.render_forest(), expected);
    }

    #[test]
    fn forest_cycle_marks_back_edge() {
        // spec: DEP-21 DEP-22
        // a -> b -> a: a pure 2-cycle. The lowest-index node (a) is promoted as
        // a secondary root. The forest must contain a (cycle) back-edge and must
        // terminate (finite output). Both render_forest and render_subtree mark
        // the back-edge.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        // render_forest promotes a and marks the cycle.
        let forest = g.render_forest();
        assert!(
            forest.contains("(cycle)"),
            "render_forest must mark a back-edge as (cycle): {forest:?}"
        );
        assert!(!forest.is_empty());
        // render_subtree also marks the cycle.
        let sub = g.render_subtree("skill:a").expect("skill:a must be a node");
        assert!(
            sub.contains("(cycle)"),
            "render_subtree must mark a back-edge as (cycle): {sub:?}"
        );
    }

    #[test]
    fn forest_render_subtree_scopes_to_one_root() {
        // spec: DEP-21
        // render_subtree("skill:a") returns just a's subtree, not the full forest.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:c}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let sub = g.render_subtree("skill:a").expect("skill:a must be a node");
        let expected = "- skill:a\n  - skill:b\n    - skill:c\n";
        assert_eq!(sub, expected);
    }

    #[test]
    fn forest_render_subtree_none_for_non_node() {
        // spec: DEP-21
        // render_subtree returns None for a key not in the installed graph.
        let items = vec![item(ItemKind::Skill, "a", "s")];
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());

        let g = installed_graph(&items, &installed_keys, reader(HashMap::new()));
        assert!(
            g.render_subtree("skill:nonexistent").is_none(),
            "non-installed key must return None"
        );
    }

    #[test]
    fn forest_exact_format_locked() {
        // spec: DEP-21
        // Lock the exact multi-line forest string for a->b->c: two-space indent
        // per depth, "- <key>" bullet, no role tag, trailing newline on each line.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:c}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let expected = "\
- skill:a
  - skill:b
    - skill:c
";
        assert_eq!(g.render_forest(), expected);
    }

    #[test]
    fn forest_cycle_exact_format_locked() {
        // spec: DEP-21 DEP-22
        // Lock the exact forest string for a two-node cycle a->b->a.
        // Both have in-degree 1 from each other; no natural (in-degree-0) root
        // exists. The lowest-index node (a, index 0) is promoted as a secondary
        // root so every installed item appears in the forest.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        // a is promoted as the secondary root (lowest index); b nests under it;
        // the back-edge from b to a is marked (cycle).
        let expected = "\
- skill:a
  - skill:b
    - skill:a (cycle)
";
        assert_eq!(
            g.render_forest(),
            expected,
            "pure 2-cycle must render with the lowest-index node promoted as root"
        );
        // render_subtree still works for either node.
        let sub = g.render_subtree("skill:a").expect("skill:a must be a node");
        assert_eq!(sub, expected, "subtree of a must match the forest root");
    }

    // ---- ADVERSARIAL CERTIFICATION: gaps probed from outside the author ----

    #[test]
    fn direct_dependency_keys_item_not_in_catalog_is_empty() {
        // spec: DEP-1
        // direct_dependency_keys is given an `item` that is not an element of
        // `items` (no pointer-identity match). It must return empty rather than
        // panic on the usize::MAX sentinel index. A consuming shard that passes a
        // stale/cloned item must degrade to "no deps", not crash.
        let in_catalog = vec![
            item(ItemKind::Skill, "review", "s"),
            item(ItemKind::Agent, "test", "s"),
        ];
        // A separate item value with the same fields is NOT the same pointer.
        let outsider = item(ItemKind::Skill, "review", "s");
        let mut content = HashMap::new();
        content.insert("skill:review".into(), "{{ns:test}}".into());
        let read = reader(content);
        let keys = direct_dependency_keys(&outsider, &in_catalog, &read);
        assert!(
            keys.is_empty(),
            "an item not present in `items` must yield no keys, not panic: {keys:?}"
        );
    }

    #[test]
    fn direct_dependency_keys_prefixed_item_keys_use_effective_name() {
        // spec: DEP-1 DEP-3
        // A prefixed dependency: the referent sibling carries prefix "jk", so its
        // effective_name() ("jk-test") differs from its bare name ("test"). The
        // {{ns:test}} token must still resolve by BARE name (DEP-3 mirrors token
        // expansion), and the returned dependency KEY must use the EFFECTIVE
        // (prefixed) name, because that is the manifest/install identity the
        // DEP-62 adjacency consumer needs.
        let mut review = item(ItemKind::Skill, "review", "s");
        review.prefix = Some("jk".to_string());
        let mut test = item(ItemKind::Agent, "test", "s");
        test.prefix = Some("jk".to_string());
        let items = vec![review, test];
        let mut content = HashMap::new();
        // The token names the BARE sibling name, not the prefixed one.
        content.insert("skill:review".into(), "hand off to {{ns:test}}".into());
        let read = reader(content);
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(
            keys,
            vec!["agent:jk-test".to_string()],
            "dep key must use the effective (prefixed) name while the token resolved by bare name"
        );
    }

    #[test]
    fn direct_dependency_keys_multiple_tokens_in_stable_discovery_order() {
        // spec: DEP-1
        // Several distinct {{ns:}} tokens in one item's text yield one key each,
        // in the order the tokens appear in the text (stable discovery order),
        // deduped.
        let items = vec![
            item(ItemKind::Skill, "root", "s"),
            item(ItemKind::Agent, "alpha", "s"),
            item(ItemKind::Agent, "beta", "s"),
            item(ItemKind::Agent, "gamma", "s"),
        ];
        let mut content = HashMap::new();
        // Order in text: gamma, alpha, beta, then a duplicate alpha.
        content.insert(
            "skill:root".into(),
            "first {{ns:gamma}} then {{ns:alpha}} then {{ns:beta}} again {{ns:alpha}}".into(),
        );
        let read = reader(content);
        let keys = direct_dependency_keys(&items[0], &items, &read);
        assert_eq!(
            keys,
            vec![
                "agent:gamma".to_string(),
                "agent:alpha".to_string(),
                "agent:beta".to_string(),
            ],
            "keys must follow token discovery order and be deduped: {keys:?}"
        );
    }

    #[test]
    fn installed_graph_diamond_forest_nests_shared_dep_under_both_dependents() {
        // spec: DEP-1 DEP-21
        // Diamond a->b, a->c, b->d, c->d over installed items. The forest must
        // mirror graph STRUCTURE (like render_tree_exact_nested_format_is_locked):
        // a is the sole root (in-degree 0), and d nests under BOTH b and c. d must
        // never be its own root (it has incoming edges). Lock the exact string.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
            item(ItemKind::Skill, "d", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}} {{ns:c}}".into());
        content.insert("skill:b".into(), "{{ns:d}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());
        installed_keys.insert("skill:d".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let expected = "\
- skill:a
  - skill:b
    - skill:d
  - skill:c
    - skill:d
";
        assert_eq!(
            g.render_forest(),
            expected,
            "diamond forest must nest the shared dep under each dependent with a as sole root"
        );
        // d is reachable but never a top-level root line.
        assert!(
            !g.render_forest().lines().any(|l| l == "- skill:d"),
            "shared dependency d must not appear as its own root"
        );
    }

    #[test]
    fn installed_graph_excludes_catalog_items_with_no_installed_key() {
        // spec: DEP-1 DEP-21
        // The catalog has three items but only two are installed. The third
        // (uninstalled) must be neither a node nor an edge target: it never
        // appears in the forest, and render_subtree on it returns None.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "ghost", "s"),
        ];
        let mut content = HashMap::new();
        // a depends on b AND on the uninstalled ghost.
        content.insert("skill:a".into(), "{{ns:b}} {{ns:ghost}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        // ghost is NOT installed.

        let g = installed_graph(&items, &installed_keys, reader(content));
        let forest = g.render_forest();
        assert!(
            !forest.contains("ghost"),
            "uninstalled catalog item must not appear in the forest: {forest:?}"
        );
        // The edge to ghost is dropped, so the forest is exactly a -> b.
        assert_eq!(forest, "- skill:a\n  - skill:b\n");
        assert!(
            g.render_subtree("skill:ghost").is_none(),
            "uninstalled item is not a node: render_subtree must be None"
        );
        // ghost is not a dependent of anything and has no dependents.
        assert!(g.dependents("skill:ghost").is_empty());
    }

    #[test]
    fn installed_graph_prefixed_items_resolve_edges_by_effective_keys() {
        // spec: DEP-1 DEP-21
        // Prefixed installed items: keys in the manifest are the EFFECTIVE
        // (prefixed) names ("skill:jk-a", "skill:jk-b"), while the {{ns:b}} token
        // resolves by BARE name. The graph must still wire a -> b and render the
        // prefixed keys.
        let mut a = item(ItemKind::Skill, "a", "s");
        a.prefix = Some("jk".to_string());
        let mut b = item(ItemKind::Skill, "b", "s");
        b.prefix = Some("jk".to_string());
        let items = vec![a, b];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:jk-a".to_string());
        installed_keys.insert("skill:jk-b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        assert_eq!(
            g.dependents("skill:jk-b"),
            vec!["skill:jk-a".to_string()],
            "edge must resolve between prefixed nodes by effective key"
        );
        assert_eq!(
            g.render_forest(),
            "- skill:jk-a\n  - skill:jk-b\n",
            "forest must render the prefixed effective keys"
        );
    }

    #[test]
    fn installed_dependents_multiple_dependents_all_returned() {
        // spec: DEP-1
        // Two installed items both depend on a common target. dependents(target)
        // returns BOTH, in stable index order, deduped.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "shared", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:shared}}".into());
        content.insert("skill:b".into(), "{{ns:shared}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:shared".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let deps = g.dependents("skill:shared");
        assert_eq!(
            deps,
            vec!["skill:a".to_string(), "skill:b".to_string()],
            "all dependents of the shared target must be returned in index order"
        );
    }

    #[test]
    fn installed_dependents_target_with_no_dependents_in_larger_graph() {
        // spec: DEP-1 DEP-21
        // In a non-trivial diamond, the top root `a` is depended on by nothing.
        // dependents("skill:a") is empty even though a participates in many edges
        // as a SOURCE. (Reverse-direction-only, exercised in a larger graph.)
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
            item(ItemKind::Skill, "d", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}} {{ns:c}}".into());
        content.insert("skill:b".into(), "{{ns:d}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());
        installed_keys.insert("skill:d".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        assert!(
            g.dependents("skill:a").is_empty(),
            "the top root must have no dependents in a diamond"
        );
        // Sanity: d (the sink) is depended on by both b and c.
        assert_eq!(
            g.dependents("skill:d"),
            vec!["skill:b".to_string(), "skill:c".to_string()]
        );
    }

    #[test]
    fn forest_dependency_only_item_never_appears_as_root_two_dependents() {
        // spec: DEP-21
        // A single dependency-only item shared by TWO independent roots must nest
        // under each dependent but NEVER appear at the top level. roots r1, r2 both
        // depend on `lib`; lib has in-degree 2, so it is not a root. Lock the
        // exact multi-line forest string.
        let items = vec![
            item(ItemKind::Skill, "r1", "s"),
            item(ItemKind::Skill, "r2", "s"),
            item(ItemKind::Skill, "lib", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:r1".into(), "{{ns:lib}}".into());
        content.insert("skill:r2".into(), "{{ns:lib}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:r1".to_string());
        installed_keys.insert("skill:r2".to_string());
        installed_keys.insert("skill:lib".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let expected = "\
- skill:r1
  - skill:lib
- skill:r2
  - skill:lib
";
        assert_eq!(
            g.render_forest(),
            expected,
            "lib must nest under both roots and never appear as its own top-level root"
        );
        // Defensive: no top-level "- skill:lib" line at depth 0.
        assert!(
            !g.render_forest().lines().any(|l| l == "- skill:lib"),
            "dependency-only item must not be a root line"
        );
    }

    #[test]
    fn render_subtree_from_dependency_only_node_renders_its_own_subtree() {
        // spec: DEP-21
        // render_subtree on a node WITH incoming edges (a dependency-only node,
        // in-degree > 0) must still render that node's own subtree, regardless of
        // in-degree. This is the `recall <dep> --tree` path: scoping to a
        // non-root node is valid.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:c}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        // b is a dependency-only node (in-degree 1 from a) but its subtree is its
        // own: b at depth 0 with c nested under it. a (its dependent) is absent.
        let sub = g.render_subtree("skill:b").expect("skill:b must be a node");
        assert_eq!(
            sub, "- skill:b\n  - skill:c\n",
            "subtree of a dependency-only node is rooted at that node, ignoring in-degree"
        );
    }

    #[test]
    fn forest_all_cycle_component_promoted_alongside_independent_root() {
        // spec: DEP-21 DEP-22
        // A graph with an independent root `c` plus an `a<->b` pure cycle.
        // c has in-degree 0 and renders first. The a<->b cycle component has
        // no in-degree-0 member, so `a` (lowest index, 0) is promoted as a
        // secondary root after c. All three installed items appear in the output.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        // c stands alone (no edges): a natural primary root.
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let forest = g.render_forest();
        // c renders first (primary root), then the cycle component with a
        // promoted (index 0 < index 1).
        let expected = "\
- skill:c
- skill:a
  - skill:b
    - skill:a (cycle)
";
        assert_eq!(
            forest, expected,
            "independent root then promoted cycle root: all items must appear"
        );
        // All three installed items appear in the forest.
        assert!(
            forest.contains("skill:a") && forest.contains("skill:b") && forest.contains("skill:c"),
            "no installed item may be hidden from the forest"
        );
    }

    #[test]
    fn forest_independent_items_both_roots() {
        // spec: DEP-21
        // Two installed items with no edges between them: both are roots in the
        // forest (each has in-degree 0) and each appears as a top-level line.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(HashMap::new()));
        let forest = g.render_forest();
        assert!(
            forest.contains("- skill:a\n"),
            "a must be a root: {forest:?}"
        );
        assert!(
            forest.contains("- skill:b\n"),
            "b must be a root: {forest:?}"
        );
    }

    // ---- InstalledGraph: forest_nodes / subtree_node (DEP-63) -------------

    #[test]
    fn forest_nodes_simple_chain_structured() {
        // spec: DEP-63
        // a -> b: forest_nodes() returns one root DepNode for "skill:a" whose
        // `dependencies` list contains one child DepNode for "skill:b" with an
        // empty `dependencies` list.  No cycle field on either node.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();

        // Exactly one root: skill:a (in-degree 0).
        assert_eq!(nodes.len(), 1, "expected 1 root: {nodes:?}");
        let root = &nodes[0];
        assert_eq!(root.key, "skill:a");
        assert!(!root.cycle, "root must not be a cycle leaf");
        let children = root
            .dependencies
            .as_ref()
            .expect("root must have dependencies");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].key, "skill:b");
        assert!(!children[0].cycle);
        let leaf_deps = children[0]
            .dependencies
            .as_ref()
            .expect("leaf must have dependencies");
        assert!(
            leaf_deps.is_empty(),
            "leaf must have empty dependencies: {leaf_deps:?}"
        );
    }

    #[test]
    fn forest_nodes_cycle_yields_cycle_leaf() {
        // spec: DEP-63
        // a -> b -> a: forest_nodes promotes a as the root.  b is a child of a,
        // and the back-edge from b back to a is a cycle leaf: {key: "skill:a",
        // cycle: true} with no `dependencies` field.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();

        // One root (promoted all-cycle component).
        assert_eq!(nodes.len(), 1);
        let root = &nodes[0];
        assert_eq!(root.key, "skill:a");
        assert!(!root.cycle);
        let children = root.dependencies.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        let b_node = &children[0];
        assert_eq!(b_node.key, "skill:b");
        assert!(!b_node.cycle);
        let b_children = b_node.dependencies.as_ref().unwrap();
        assert_eq!(b_children.len(), 1);
        let cycle_leaf = &b_children[0];
        assert_eq!(cycle_leaf.key, "skill:a");
        assert!(cycle_leaf.cycle, "back-edge to a must be a cycle leaf");
        assert!(
            cycle_leaf.dependencies.is_none(),
            "cycle leaf must have no dependencies field"
        );
    }

    #[test]
    fn subtree_node_returns_scoped_node() {
        // spec: DEP-63
        // subtree_node("skill:a") returns the root DepNode for a's subtree;
        // subtree_node on a non-installed key returns None.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));

        let node = g.subtree_node("skill:a").expect("skill:a must be a node");
        assert_eq!(node.key, "skill:a");
        let children = node.dependencies.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].key, "skill:b");

        // Non-installed key returns None.
        assert!(g.subtree_node("skill:ghost").is_none());
    }

    #[test]
    fn subtree_node_leaf_has_empty_dependencies() {
        // spec: DEP-63
        // A leaf node (no outgoing edges) has `dependencies: []`, not absent.
        let items = vec![item(ItemKind::Skill, "leaf", "s")];
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:leaf".to_string());

        let g = installed_graph(&items, &installed_keys, reader(HashMap::new()));
        let node = g.subtree_node("skill:leaf").expect("leaf must be a node");
        assert_eq!(node.key, "skill:leaf");
        assert!(!node.cycle);
        let deps = node
            .dependencies
            .as_ref()
            .expect("leaf must have dependencies field");
        assert!(deps.is_empty(), "leaf must have empty dependencies");
    }

    #[test]
    fn forest_nodes_covers_same_items_as_render_forest() {
        // spec: DEP-63
        // forest_nodes() and render_forest() must cover exactly the same set of
        // keys.  Extract all keys from the DepNode tree and compare with the keys
        // found in the human forest string.  Verified over a graph that has a
        // natural root, a promoted all-cycle component, and shared dependencies.
        let items = vec![
            item(ItemKind::Skill, "a", "s"), // 0: pure cycle with b
            item(ItemKind::Skill, "b", "s"), // 1: pure cycle with a
            item(ItemKind::Skill, "c", "s"), // 2: natural root, depends on d
            item(ItemKind::Skill, "d", "s"), // 3: dependency only
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());
        installed_keys.insert("skill:d".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));

        // Collect all unique keys in the DepNode forest (non-cycle entries are
        // the installed set; cycle leaves duplicate their ancestor's key, which
        // is fine but we de-dup for the coverage check).
        fn collect_keys(nodes: &[DepNode], out: &mut HashSet<String>) {
            for n in nodes {
                out.insert(n.key.clone());
                if let Some(children) = &n.dependencies {
                    collect_keys(children, out);
                }
            }
        }
        let nodes = g.forest_nodes();
        let mut node_keys: HashSet<String> = HashSet::new();
        collect_keys(&nodes, &mut node_keys);

        // The forest string covers exactly the installed keys (every item
        // appears at least once, counting cycle annotations too).
        for key in &installed_keys {
            assert!(
                node_keys.contains(key),
                "forest_nodes must include installed key {key}: {nodes:?}"
            );
        }

        // The render_forest output contains exactly the same installed keys.
        let forest = g.render_forest();
        for key in &installed_keys {
            assert!(
                forest.contains(key.as_str()),
                "render_forest must contain key {key}: {forest:?}"
            );
        }
    }

    #[test]
    fn forest_nodes_json_serialization_shape() {
        // spec: DEP-63
        // Serialize forest_nodes() with serde_json and assert the exact JSON
        // shape: a normal node has "key" and "dependencies" but no "cycle"
        // field; a cycle leaf has "key" and "cycle":true but no "dependencies".
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();
        let json_str = serde_json::to_string(&nodes).expect("must serialize");
        let v: serde_json::Value = serde_json::from_str(&json_str).expect("must parse");

        // Top-level is an array with one root.
        assert!(v.is_array());
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let root = &arr[0];

        // Root has "key" and "dependencies", no "cycle".
        assert_eq!(root["key"], "skill:a");
        assert!(
            root.get("dependencies").is_some(),
            "normal node must have dependencies"
        );
        assert!(
            root.get("cycle").is_none(),
            "normal node must not have cycle field"
        );

        // b is the one child.
        let b_node = &root["dependencies"][0];
        assert_eq!(b_node["key"], "skill:b");
        assert!(b_node.get("cycle").is_none());
        assert!(b_node.get("dependencies").is_some());

        // The cycle leaf (back-edge to a) has "cycle":true and no "dependencies".
        let cycle_leaf = &b_node["dependencies"][0];
        assert_eq!(cycle_leaf["key"], "skill:a");
        assert_eq!(cycle_leaf["cycle"], true);
        assert!(
            cycle_leaf.get("dependencies").is_none(),
            "cycle leaf must not have dependencies field"
        );
    }

    #[test]
    fn forest_nodes_prefixed_items_use_effective_keys() {
        // spec: DEP-63
        // When items carry a prefix, forest_nodes() must use the effective
        // (prefixed) keys, matching what render_forest() emits.
        let mut a = item(ItemKind::Skill, "a", "s");
        a.prefix = Some("jk".to_string());
        let mut b = item(ItemKind::Skill, "b", "s");
        b.prefix = Some("jk".to_string());
        let items = vec![a, b];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:jk-a".to_string());
        installed_keys.insert("skill:jk-b".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].key, "skill:jk-a");
        let children = nodes[0].dependencies.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].key, "skill:jk-b");
    }

    #[test]
    fn forest_nodes_diamond_nests_shared_dep_under_both_dependents() {
        // spec: DEP-63
        // PARITY (load-bearing): the structured forest must mirror the human
        // render_forest STRUCTURE, not just cover the same key set. Diamond
        // a->b, a->c, b->d, c->d: `a` is the sole root (in-degree 0), and `d`
        // must be nested under BOTH `b` and `c` -- emitted twice -- exactly as
        // render_forest renders d twice (see
        // installed_graph_diamond_forest_nests_shared_dep_under_both_dependents).
        // This is the JSON counterpart of render_tree_exact_nested_format_is_locked.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
            item(ItemKind::Skill, "d", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}} {{ns:c}}".into());
        content.insert("skill:b".into(), "{{ns:d}}".into());
        content.insert("skill:c".into(), "{{ns:d}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());
        installed_keys.insert("skill:d".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();

        // `a` is the sole root.
        assert_eq!(nodes.len(), 1, "a must be the only root: {nodes:?}");
        let a = &nodes[0];
        assert_eq!(a.key, "skill:a");
        assert!(!a.cycle);

        // a's children are b then c (stable discovery order from the token text).
        let a_children = a.dependencies.as_ref().expect("a has deps");
        assert_eq!(
            a_children
                .iter()
                .map(|n| n.key.as_str())
                .collect::<Vec<_>>(),
            vec!["skill:b", "skill:c"],
            "a's children must be b then c in discovery order"
        );

        // d nests under BOTH b and c, as a distinct (non-cycle) leaf each time.
        for (i, parent) in ["skill:b", "skill:c"].iter().enumerate() {
            let p = &a_children[i];
            assert_eq!(&p.key, parent);
            assert!(!p.cycle, "{parent} must not be a cycle leaf");
            let p_children = p.dependencies.as_ref().expect("parent has deps");
            assert_eq!(p_children.len(), 1, "{parent} must have exactly one child");
            let d = &p_children[0];
            assert_eq!(d.key, "skill:d", "{parent}'s child must be d");
            assert!(
                !d.cycle,
                "d under {parent} must be a normal node, not a cycle"
            );
            assert!(
                d.dependencies.as_ref().is_some_and(|v| v.is_empty()),
                "d is a leaf: empty dependencies, not absent or a cycle"
            );
        }

        // Serialized shape parity: d appears as a nested object under both b and c.
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&nodes).unwrap()).unwrap();
        assert_eq!(v[0]["key"], "skill:a");
        assert_eq!(v[0]["dependencies"][0]["key"], "skill:b");
        assert_eq!(v[0]["dependencies"][0]["dependencies"][0]["key"], "skill:d");
        assert_eq!(v[0]["dependencies"][1]["key"], "skill:c");
        assert_eq!(v[0]["dependencies"][1]["dependencies"][0]["key"], "skill:d");

        // Cross-check against the human renderer: same nodes, same structure.
        // The human forest renders d twice (once per path); the structured form
        // must too. Counting "skill:d" occurrences pins the duplication parity.
        let forest = g.render_forest();
        assert_eq!(
            forest.matches("- skill:d").count(),
            2,
            "human forest renders d twice (under b and under c): {forest:?}"
        );
    }

    #[test]
    fn forest_nodes_independent_root_plus_cycle_component_all_items_present() {
        // spec: DEP-63
        // Mixed graph: an independent root `c` (in-degree 0) plus an a<->b pure
        // cycle (no in-degree-0 member). The structured forest must, like
        // render_forest (forest_all_cycle_component_promoted_alongside_independent_root),
        // emit `c` first, then promote `a` (lowest index) as a secondary root,
        // so EVERY installed item appears -- the cycle-visibility fix carried
        // into JSON. The back-edge from b to a is a cycle leaf and there is no
        // infinite nesting.
        let items = vec![
            item(ItemKind::Skill, "a", "s"),
            item(ItemKind::Skill, "b", "s"),
            item(ItemKind::Skill, "c", "s"),
        ];
        let mut content = HashMap::new();
        content.insert("skill:a".into(), "{{ns:b}}".into());
        content.insert("skill:b".into(), "{{ns:a}}".into());
        let mut installed_keys = HashSet::new();
        installed_keys.insert("skill:a".to_string());
        installed_keys.insert("skill:b".to_string());
        installed_keys.insert("skill:c".to_string());

        let g = installed_graph(&items, &installed_keys, reader(content));
        let nodes = g.forest_nodes();

        // Two roots: the natural root c, then the promoted cycle root a.
        assert_eq!(
            nodes.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(),
            vec!["skill:c", "skill:a"],
            "natural root c first, then promoted cycle root a: {nodes:?}"
        );

        // c is a leaf.
        assert!(nodes[0].dependencies.as_ref().is_some_and(|v| v.is_empty()));

        // a -> b -> (cycle back to a). No infinite nesting.
        let a = &nodes[1];
        let b = &a.dependencies.as_ref().unwrap()[0];
        assert_eq!(b.key, "skill:b");
        assert!(!b.cycle);
        let back = &b.dependencies.as_ref().unwrap()[0];
        assert_eq!(back.key, "skill:a");
        assert!(back.cycle, "the b->a back-edge must be a cycle leaf");
        assert!(
            back.dependencies.is_none(),
            "cycle leaf has no dependencies"
        );

        // Every installed item appears as a node somewhere in the structured forest.
        fn collect(nodes: &[DepNode], out: &mut HashSet<String>) {
            for n in nodes {
                out.insert(n.key.clone());
                if let Some(c) = &n.dependencies {
                    collect(c, out);
                }
            }
        }
        let mut seen = HashSet::new();
        collect(&nodes, &mut seen);
        for k in &installed_keys {
            assert!(
                seen.contains(k),
                "every installed item must appear: missing {k}"
            );
        }
    }

    #[test]
    fn forest_nodes_empty_graph_is_empty_array() {
        // spec: DEP-63
        // Nothing installed: forest_nodes() is an empty vec, which serializes to
        // the JSON empty array `[]` (not null, not an error). This is the data
        // behind `recall --tree --json` over an empty manifest.
        let items: Vec<CatalogItem> = vec![];
        let installed_keys: HashSet<String> = HashSet::new();
        let g = installed_graph(&items, &installed_keys, reader(HashMap::new()));
        let nodes = g.forest_nodes();
        assert!(nodes.is_empty(), "empty graph yields no roots: {nodes:?}");
        assert_eq!(
            serde_json::to_string(&nodes).unwrap(),
            "[]",
            "empty forest must serialize to a JSON empty array"
        );
    }
}
