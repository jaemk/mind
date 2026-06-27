# Interactive TUI

`mind probe` opens an interactive terminal UI: a browsable, searchable view of
every source and item, and the interactive front end for the rest of the CLI
(meld, learn, sync, upgrade, config). For the `probe` verb and its flags, see
[Commands](commands.md).

## Opening it

- `mind probe` with no opt-out launches the TUI. It requires a TTY on stdout.
- It falls back to the non-interactive catalog listing when `--no-tui` (short
  `-n`) is given, when `--json` is given, or when stdout is not a terminal (piped
  or redirected).
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
mind did not install. See [Unmanaged items](unmanaged.md).

Under each group the hierarchy is source -> kind (skills, agents, rules) ->
item -> detail. Expanding an item shows its description and frontmatter, and for
a skill its file tree.

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
manifest, and store. Every mutating action confirms before applying; destructive
actions (`forget`, `unmeld --forget`) require an explicit confirmation. Results
and errors are shown inline.

- **Install / learn**: install the selected Available item. On a higher node it
  installs in bulk: a source installs every available item from it, a kind bucket
  every item of that kind, and the Available group everything. Already-installed
  items are skipped.
- **Forget**: uninstall the selected Installed item.
- **Meld / unmeld**: meld the selected or entered source; unmeld a melded source.
  The TUI's unmeld uninstalls the source's installed items by default and
  confirms first.
- **Sync / upgrade**: sync all or the selected source; upgrade pending or the
  selected items, showing the same deltas and confirming before applying.
- **Config lobes**: view and manage agent homes (list / add / remove).

An install preview arms the same dependency closure as the direct install
action. See [Dependencies](dependencies.md).

## Details dialog

Pressing `Enter` on a source or item opens a centered details dialog describing
the node and listing the actions valid for it as a selectable list, each run
through the normal confirm-and-execute path.

- For an item: its kind, source, the commit when installed, and the description.
  It offers Install when the item is not installed, else Forget.
- For a source: its name and installed/available item counts, and Install all
  available items, Uninstall all installed items, and Unmeld. An action is
  omitted when it would do nothing.

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
