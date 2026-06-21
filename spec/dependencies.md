# Within-source dependency resolution

Status: done. Installing a selected subset of a source's items pulls in the
other items in that source those items reference, so a partial `learn` installs a
working set rather than a dangling one.

## Overview

An item can reference a sibling in the same source: most commonly a skill names a
specific agent profile it expects to exist. The Claude harness resolves that name
at runtime, so the referenced sibling must also be installed or the reference
dangles. References are already declared with `{{ns:name}}` tokens (see
namespacing.md), so they are the dependency edges.

When `learn` installs every item of a source, the references are satisfied for
free and nothing extra is needed. The work here is for a *partial* selection:
`learn skill:review` should also bring in the agent that `review` references, and
that agent's references in turn, to the full closure. Resolution is computed by
analyzing references, stored as a graph, and used two ways: to show the user a
dependency tree before installing (from `learn` and from the interactive TUI),
and to install the closure in a dependency-first order.

The rest of this document states these rules normatively. A dependency target is
identified by stable identity `(source, kind, bare_name)` (see namespacing.md).

## What a dependency is

- `DEP-1` An item's dependencies are its intra-source references: each sibling
  named by a `{{ns:name}}` token (NS-10) appearing in the item's text files (the
  whole skill directory, or the agent/rule file, matching the scan breadth of
  NS-20). A skill that references an agent profile is the common case.
- `DEP-2` Dependencies are intra-source only. A `{{ns:}}` token never crosses
  sources, so resolution stays within the one source and never pulls an item from
  another source.
- `DEP-3` A `{{ns:name}}` token resolves to the sibling(s) whose bare name is
  `name`; each such sibling is a dependency (normally exactly one). Resolving by
  bare name mirrors token expansion (NS-11), so installing the dependency makes
  the runtime reference resolve.

## When resolution applies

- `DEP-10` Resolution applies when `learn` selects a proper subset of a source's
  items. When the selection already covers all of that source's items (e.g. `'*'`
  or `'owner/repo#*'`, CLI-31), resolution is a no-op for that source: every
  referent is installed regardless.
- `DEP-11` Resolution is transitive: the dependencies of a dependency are included
  as well, to the full closure. The closure is computed per source over that
  source's selected items.
- `DEP-12` An item with no `{{ns:}}` tokens has no dependencies. A selection whose
  items reference nothing installs exactly the selected set, unchanged from a
  source with no references.

## The dependency graph

- `DEP-20` Resolution produces a dependency graph: nodes are items keyed by stable
  identity `(source, kind, bare_name)`, and an edge points from an item to each
  item it depends on (DEP-1). The explicitly selected items are the graph roots.
- `DEP-21` The graph is stored so it yields both forms without re-analysis: (a) a
  tree rooted at each selected item with its transitive dependencies as
  descendants, for display; and (b) a topological order, dependencies before
  dependents, for the install.
- `DEP-22` References may form a cycle. Resolution visits each item once and
  terminates. In the display tree, a reference back to an item already on the
  current path is shown as a marked back-edge rather than expanded again; in the
  install order, the members of a cycle are placed in a stable order (their
  discovery order).
- `DEP-23` A dependency that is already installed (present in the manifest) is
  shown in the tree marked as already installed and is not re-installed; only
  not-yet-installed items in the closure are installed.

## `learn`

- `DEP-30` When `learn` pulls in dependencies (DEP-10), it installs the whole
  closure as one operation, dependencies before dependents (DEP-21). The collision
  check (CLI-33) runs over the full closure, not just the explicit selection.
- `DEP-31` When the closure adds items beyond the explicit selection, `learn`
  prints the dependency tree (DEP-21), distinguishing explicitly selected items,
  pulled-in dependencies, and already-installed nodes, and prompts before
  installing; `--yes` (or a `[y/N]` yes) confirms. When the closure adds nothing,
  `learn` installs directly with no prompt (CLI-30 behavior is unchanged).
- `DEP-32` `--dry-run` (CLI-32) renders the same dependency tree and lists the full
  closure that would be installed, and installs nothing.

## Interactive TUI (`probe`)

- `DEP-40` Choosing to install an Available item (TUI-20) shows its dependency tree
  (DEP-21) in the confirm step before applying, with the same selected /
  dependency / already-installed distinction as DEP-31.
- `DEP-41` Confirming installs the whole closure in dependency-first order (DEP-30);
  declining installs nothing.

## Non-goals

- `DEP-50` `forget` does not automatically remove an item's dependencies: a
  dependency may be shared by another installed item, so uninstall stays a
  per-item operation (CLI-40). Reverse-dependency cleanup is out of scope here.
