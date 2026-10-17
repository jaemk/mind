# Find and install items

Once a source is melded, its items are available to browse and install. This
page covers finding an item, installing it, and what `learn` does with the
items it depends on.

```
mind probe review            # search names and descriptions for "review"
mind learn skill:review      # install it
mind recall                  # confirm it is installed
```

## Browse what is available

`mind probe` opens an interactive browser on a terminal (see
[Browse in the terminal UI](tui.md)). Give it a query, or pipe it, for a plain
listing:

```
mind probe --no-tui               # everything every source offers
mind probe pdf                    # names or descriptions containing "pdf"
mind probe --kind agent           # only agents
mind probe --source '*anthropic*' # only items from matching sources
mind probe --json                 # one JSON document, for scripts
```

`--kind` takes `skill`, `agent`, `rule`, `command`, `workflow`, `tool`, or
`resource`.
`--source` takes an exact source name, a trailing part of it (`repo` or
`owner/repo`), or a glob matched against the full `host/owner/repo` identity.
Matching several sources is normal. `mind recall` accepts the same two filters
for the installed listing.

## Install by name

```
mind learn review                  # an item named review
mind learn skill:review            # the skill named review, when an agent shares the name
mind learn acme/skills#review      # review from one specific source
```

A bare name that matches items of two kinds (`skill:review` and
`agent:review`) is an error rather than a guess; add the kind.

## Install many at once

`learn` takes a glob. Quote it so the shell does not expand it first:

```
mind learn 'skill:*'               # every skill from every source
mind learn 'acme/skills#*'         # every item of one source
mind learn acme/skills --all       # the same, without the glob
mind learn 'review*'               # every item whose name starts with review
```

A glob that matches nothing is an error. To install a named subset of a source
in one step at meld time, see
[Choose what a source installs](choosing-items.md).

## Items that depend on other items

An item can reference siblings in its own source, for example a skill that
hands off to an agent. When you install part of a source, `learn` follows those
references and installs what they need, then shows the tree before changing
anything:

```text
$ mind learn skill:review
skill:review  [selected]
  agent:dev   [dependency]
    agent:test  [dependency]

Install these 3 items? [y/N]
```

The same goes for the [tools](tooling.md) and [resources](resources.md) an
item points at with `{{tools:name}}` or `{{resources:name}}`: learning the item
installs them too, after the same prompt.

Items already installed are marked `[installed]` and left alone. Preview the
set without installing anything with `--dry-run`:

```
mind learn --dry-run skill:review
```

`forget` does not cascade: removing `review` leaves `dev` and `test`
installed, since another item may still use them. See
[Dependencies between items](dependencies.md) for how authors declare these
references.

## When a path is already taken

`mind` never overwrites a file or link it did not create. If
`~/.claude/skills/review` already exists and is not one of its links, `learn`
asks on a terminal and refuses otherwise. Move the existing entry aside, or
pass `--force` to replace it. To bring a hand-placed item under management
instead, see [Adopt items you placed by hand](unmanaged.md).

## Installing into one project only

By default an item links into every configured lobe. Inside a project that has
its own lobe (see [Use more than one agent](lobes.md#project-lobes)), `--local`
links it into that project's lobe only:

```
cd ~/projects/myapp
mind learn skill:review --local
```

## Other ways in

- A single skill from a pasted GitHub or GitLab URL:
  [Install one item from a link](item-links.md).
- A Claude plugin or marketplace repo melds as it is; its skills, agents,
  commands, and workflows show up as items. See
  [Claude plugins and marketplaces](marketplace.md).
- A curated list of repos registered in one meld:
  [Follow a curated collection](curate.md).
