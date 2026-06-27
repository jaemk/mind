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
- `DEP-4` An item may declare explicit dependencies with an optional `requires`
  key in its frontmatter (the `SKILL.md` for a skill, the `.md` for an agent or
  rule): a whitespace-separated list of intra-source refs, e.g.
  `requires: skill:x agent:y`. It is a top-level scalar, read by the same
  scalar-only frontmatter reader as `description` (DSC-21); the list form is a
  single string split on whitespace, not a YAML sequence. An item's dependency set
  is the union of its `requires` entries and its `{{ns:}}`-derived siblings (DEP-1),
  deduped by stable identity, so the existing closure, tree, order, and prompt
  (DEP-10..41) apply unchanged over the combined edges. Absent `requires` means no
  explicit dependencies. Unlike a `{{ns:}}` token, `requires` is pure metadata: it
  is not rewritten into the item body, it only adds a dependency edge.
- `DEP-5` A `requires` entry is an intra-source ref (DEP-2), `kind:name` or a bare
  `name`, resolved against siblings the same way an item ref resolves (a bare name
  matches across kinds with the standard ambiguity rules, a `kind:` prefix narrows,
  CLI-1/CLI-2). It names the sibling by bare name, mirroring `{{ns:}}` resolution
  (DEP-3), so a prefix in effect applies at install. A source-qualified ref
  (`owner/repo#name`) is rejected: `requires` never crosses sources.
- `DEP-6` A `requires` entry that resolves to no sibling (a typo, an unknown item,
  an ambiguous bare name, or a source-qualified ref) is an error (`BadReference`)
  at install, the same validation a `{{ns:}}` token receives. `review` (CLI-131)
  reports an unresolved `requires` entry as a hard error.

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

## forget warns about dependents

- `DEP-60` `forget` of a single installed item computes its installed *dependents*:
  the items whose dependency set (DEP-4) includes the item being removed, over the
  graph of installed items (the manifest plus each item's catalog edges). When any
  exist, `forget` lists them and warns that removing this item breaks them (their
  `{{ns:}}` or `requires` reference to it will not resolve), then prompts `[y/N]`
  before removing anything. `--force` (or a global `--yes`) proceeds without the
  prompt; a non-TTY run without `--force`/`--yes` refuses (`ConfirmationRequired`)
  and removes nothing. The item is still removed when confirmed: `forget` does not
  cascade to dependents (it warns, it does not remove them) and does not remove the
  item's own dependencies (DEP-50). The check is for a single-item `forget`; a glob
  `forget` keeps its existing multi-item confirmation (CLI-42).

## recall --tree

- `DEP-61` `recall --tree` renders installed items as a dependency forest: each
  root is an installed item that no other installed item depends on, with its
  transitive dependencies nested beneath it (the DEP-21 tree form), and a cycle is
  broken with a marked back-edge rather than expanded again (DEP-22). An optional
  `recall <item> --tree` scopes the forest to one item's subtree. Items reachable
  only as a dependency appear nested under their dependents, not as their own root.

## Non-interactive probe shows the tree

- `DEP-62` Non-interactive `probe` (`--no-tui` / `-n`, a non-TTY stdout, or
  `--json`) always renders the dependency relationships, cycle-safe (DEP-22). The
  human listing nests each item's transitive dependencies beneath it (DEP-21)
  rather than a flat list (CLI-80). `probe --json` (CLI-84) adds a `dependencies`
  field to each row, the list of that item's direct dependency keys (effective
  `kind:name`), so a consumer can reconstruct the graph and detect cycles itself;
  the JSON stays flat adjacency rather than a nested tree.

## Non-goals

- `DEP-50` `forget` does not automatically remove an item's dependencies: a
  dependency may be shared by another installed item, so uninstall stays a
  per-item operation (CLI-40). It warns about dependents (DEP-60) but never cascades
  a removal.
