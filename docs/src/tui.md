# Browse in the terminal UI

`mind probe` opens an interactive terminal UI: a browsable, searchable view of
every source and item, from which you can meld, install, remove, sync, upgrade,
and manage agent homes. For the `probe` verb's flags, see
[Commands](commands.md#probe).

```
mind probe                 # open it
mind probe review          # open it with the search set to "review"
mind probe --kind skill    # open it filtered to skills
```

Quick keys: arrows move, `Enter` opens a details dialog with the actions for
the selected source or item, `m` melds a new source, `q` quits.

## Opening it

- `mind probe` with no opt-out launches the TUI. It requires a TTY on stdout.
- It falls back to the non-interactive catalog listing when `--no-tui` is
  given, when `--json` is given, or when stdout is not a terminal (piped or
  redirected).
- The `query`, `--kind`, and `--source` arguments apply in both modes. In the
  listing they filter it; in the TUI they seed the initial search and filter
  state.

## The browse tree

Two top-level groups, each an independently collapsible tree:

- **Installed**: the manifest. Installed items grouped source -> kind -> item,
  each showing effective name, source, and short commit, matching what `recall`
  reports.
- **Available**: catalog items of melded sources that are not installed,
  not-yet-melded sources suggested by the registry, and ad-hoc sources you enter,
  de-duplicated.

A third **Unmanaged** group appears below them when the agent home holds items
mind did not install. See [Adopt items you placed by hand](unmanaged.md).

Under each group the hierarchy is source -> kind (skills, agents, rules,
commands, workflows, tools, resources) -> item -> detail. Expanding an item shows its description
and frontmatter, and for a skill its file tree.

### Navigation keys

- `Up` / `Down`: move the highlight.
- `Left` / `Right`: collapse / expand a structural node (source and kind
  buckets).
- `Space`: also collapses / expands a structural node.
- `Enter`: open the details dialog on a source or item; on a group header or kind
  bucket it toggles instead.

As the selection moves, the view scrolls to keep the highlighted row away from
the top and bottom edges while there are more rows to scroll. Near the start or
end of the list the highlight may sit at the edge.

## Search

A search box filters the visible tree by case-insensitive substring over item
name and description, across both groups, and composes with the active kind and
source filters. Clearing the search restores the full tree.

## Live refresh

The TUI polls the on-disk registry (`sources.json`) and manifest
(`manifest.json`) about once a second, so changes made by another `mind` process
or a direct edit appear without a manual reload. A refresh preserves the current
selection, expansion, and search state, and is skipped while a mutating action is
running.

## Actions

Each action invokes the same verb the CLI exposes, against the same registry,
manifest, and store, with one exception: upgrade does not fetch first (see
below). Every mutating action confirms before applying; destructive actions
(`forget`, `unmeld --forget`) require an explicit confirmation. Results and
errors are shown inline. When an action stops on a hook failure or interruption,
the error is followed by a pointer to `mind hooks history --limit 1 --show`,
since the hook's output went to a terminal the TUI has since redrawn.

- **Install / learn**: install the selected Available item. On a higher node it
  installs in bulk: a source installs every available item from it, a kind bucket
  every item of that kind, and the Available group everything. Already-installed
  items are skipped.
- **Forget**: uninstall the selected Installed item.
- **Meld / unmeld**: meld the selected or entered source; unmeld a melded source.
  The TUI's unmeld uninstalls the source's installed items by default and
  confirms first. The `m` input takes a repo spec followed by any `mind meld`
  flags, parsed exactly as the CLI parses them, for example
  `owner/repo --pin branch=dev --profile full`. A flag the CLI would reject is
  shown as an error and the input stays open with your text. The meld then runs
  the CLI's full flow on the normal terminal: registering, the profile picker,
  and the install offer.
- **Sync**: sync all or the selected source.
- **Upgrade**: upgrade pending items (or the selected item), showing the same
  deltas the CLI would and confirming before applying. Unlike the CLI's plain
  `upgrade`, this does NOT sync first: it acts on whatever the last sync or poll
  already found to be out of date. Run **Sync** first if you want to check
  upstream before upgrading. When nothing is currently known to be out of date,
  the confirm says so relative to the last sync, not as a claim that you are
  fully current.
- **Config lobes**: view and manage agent homes (list / add / remove).

An install preview arms the same dependency closure as the direct install
action. See [Find and install items](installing.md#items-that-depend-on-other-items).

### The confirm

While a confirm is open it takes every key: `y` confirms, `n` or `Esc` cancels,
and anything else is ignored. An install's confirm also takes `f`, which runs it
with `--force`, replacing every link target in the way that `mind` did not
create. If an install you confirmed with `y` stops on such a target, a second
confirm names the path and offers the same retry with `--force`. For a meld,
add `--overwrite` to the `m` input instead.

## Details dialog

Pressing `Enter` on a source or item opens a centered details dialog describing
the node and listing the actions valid for it as a selectable list, each run
through the normal confirm-and-execute path.

- For an item: its kind, source, the commit when installed, the
  [install profiles](choosing-items.md#install-profiles) it was installed for, and the
  description. It offers Install when the item is not installed, else Forget.
  Installing a single item this way counts as asking for it by name, so
  disabling its profiles later keeps it.
- For a source: its name and installed/available item counts, and Install all
  available items, Uninstall all installed items, and Unmeld. While the source
  has no installed items, a **Set namespace** action is also offered: it opens
  an input for a namespace prefix, so items later install as `<prefix>:<name>`.
  It disappears once any item from the source is installed (a source's prefix
  locks once items are installed). An action is omitted when it would do
  nothing.
- For a source that declares profiles: the declared profiles, with the enabled
  ones marked, and an **Enable profile** or **Disable profile** action for each.
  The confirm lists the items enabling installs, or the items disabling forgets
  and any it keeps because another installed item depends on them. These run
  `mind learn <source> --profile <name>` and `mind forget <source> --profile
  <name>`. When profiles are enabled, **Install enabled profiles** comes first:
  it installs the enabled profiles' members that are not installed, running
  `mind learn <source> --profile <name>` for each.
- The newest [hook result](install-hooks.md) recorded for what the dialog
  describes, when there is one: a source's own hooks on a source, an installed
  item's hooks on that item, e.g. `last hook: failed install 'setup' (exit 2,
  840ms) 2026-10-28T10:00:00Z`.

In the dialog, `j` / `k` move the highlight, `Enter` or `y` runs the highlighted
action, and `Esc`, `q`, or `n` dismisses without acting. On a group header, kind
bucket, or suggested source there is no dialog: `Enter` keeps its toggle/preview.

## Registry preview

The Available group lists suggested, not-yet-melded sources: the union of the
`[discover].sources` entries declared by all melded sources, de-duplicated by URL
and excluding sources already melded. Expanding a registry entry shallow-clones
it to a temporary preview area and shows its catalog tree under Available without
registering it. Confirming promotes the preview to a real meld; declining
discards the temp clone.

## Terminal handling

- Actions whose verbs may prompt interactively run with the TUI suspended: mind
  leaves raw mode and the alternate screen, runs the verb on the normal terminal
  so its prompts read stdin and write stdout exactly as the CLI does, then
  restores the alternate screen and redraws. After the verb you press `Enter` to
  return. The verbs that suspend are `meld` and `unmeld`.
- While the TUI holds the terminal, every `git` child runs non-interactively
  (`GIT_TERMINAL_PROMPT=0` and an ssh `BatchMode=yes` wrapper), so an
  auth-required remote fails fast with an error surfaced inline instead of hanging
  the UI on a hidden prompt. The suspended interactive meld restores interactive
  git for its duration, so a passphrase or host-key prompt works on the normal
  terminal.

## Exit

- `q` quits from the main view.
- Ctrl-C is a force-exit from every mode. One Ctrl-C arms and shows a hint; a
  second consecutive Ctrl-C exits, so a single accidental Ctrl-C while typing does
  not quit. Any other key disarms.
