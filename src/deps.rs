//! Within-source dependency resolution (DEP-1..50).
//!
//! An item's dependencies are the siblings it names with `{{ns:name}}` tokens
//! (DEP-1, via [`crate::namespace::referenced_names`]). For a *partial* selection
//! of a source's items, [`resolve`] computes the transitive closure of those
//! references so a partial `learn` installs a working set rather than a dangling
//! one. The result is stored once (DEP-21) and exposed two ways: a display tree
//! ([`Resolution::render_tree`]) and a dependency-first install order
//! ([`Resolution::install_order`]).
//!
//! This module is pure: it reads each item's text through an injected closure so
//! it can be unit-tested with synthetic content (no filesystem).
//!
//! The public API here is consumed by the `learn` CLI path and the interactive
//! TUI (DEP-30..41), which land in sibling changes; until those wire it up the
//! resolver is exercised only by this module's tests.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use crate::catalog::CatalogItem;
use crate::namespace;

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
    // Normally one match; possibly several across kinds.
    let mut by_name: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        by_name
            .entry((item.source.as_str(), item.name.as_str()))
            .or_default()
            .push(i);
    }

    // Resolve and cache each expanded node's dependency edges, in discovery
    // order. Memoized so the closure walk visits each node's refs once (DEP-22).
    let mut deps: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut edges_of = |node: usize| -> Vec<usize> {
        if let Some(d) = deps.get(&node) {
            return d.clone();
        }
        let item = &items[node];
        let mut out: Vec<usize> = Vec::new();
        if *expand_source.get(item.source.as_str()).unwrap_or(&true) {
            for name in namespace::referenced_names(&read(item)) {
                if let Some(matches) = by_name.get(&(item.source.as_str(), name.as_str())) {
                    for &m in matches {
                        // DEP-2: intra-source guaranteed by the (source, name) key.
                        // Skip a self-reference so it never forms a trivial loop.
                        if m != node && !out.contains(&m) {
                            out.push(m);
                        }
                    }
                }
            }
        }
        deps.insert(node, out.clone());
        out
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
}
