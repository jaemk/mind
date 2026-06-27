# Within-source dependency resolution

Items in a source can reference siblings via `{{ns:name}}` tokens (see
[Namespacing](namespacing.md)). Those tokens are
dependency edges. When you install a partial selection, `mind` follows those
edges to the full transitive closure so you get a working set, not a dangling
one.

## What counts as a dependency

A dependency is an intra-source reference: each sibling named by a `{{ns:name}}`
token in the item's text files (the whole skill directory, or the agent/rule
file). Dependencies never cross sources; `{{ns:}}` is always resolved within the
one source (DEP-1, DEP-2).

## Partial `learn` pulls in the closure

When `learn` selects every item in a source (e.g. `learn owner/repo#*`),
resolution is a no-op: every referent installs anyway (DEP-10). The work is for
a partial selection.

Example: a source ships `skill:review`, `agent:dev`, and `agent:test`. `review`
references `dev` via `{{ns:dev}}`, and `dev` references `test` via `{{ns:test}}`.

```
mind learn skill:review
```

`mind` resolves the closure -- `review` -> `dev` -> `test` -- and installs all
three, dependencies first (DEP-11, DEP-21, DEP-30).

## What you see before install

When the closure adds items beyond your explicit selection, `mind` prints the
dependency tree and prompts before changing anything (DEP-31):

```
skill:review  [selected]
  agent:dev   [dependency]
    agent:test  [dependency]

Install these 3 items? [y/N]
```

Nodes already in the manifest are marked `[installed]` and are not re-installed
(DEP-23). Cycles are shown as marked back-edges rather than expanded again
(DEP-22).

`--yes` (or answering `y`) confirms without prompting. See
[Commands - learn](commands.md) for the full flag reference.

## `--dry-run` preview

`--dry-run` renders the same dependency tree and lists the full closure that
would be installed, then exits without installing anything (DEP-32):

```
mind learn --dry-run skill:review
```

## Interactive TUI (`probe`)

Choosing to install an available item in `mind probe` shows the same dependency
tree in the confirm step before applying, with the same selected / dependency /
already-installed distinction. Confirming installs the whole closure in
dependency-first order; declining installs nothing (DEP-40, DEP-41).

## `forget` does not cascade

`forget` removes exactly the one named item. It does not automatically remove
its dependencies, because a dependency may be shared by another installed item.
Uninstall is always a per-item operation (DEP-50).
