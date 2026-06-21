# Interactive TUI

`mind probe` launches an interactive terminal UI: a browsable, searchable view of
every source and item, and the interactive front end for the rest of the CLI
(meld, learn, sync, upgrade, config). The non-interactive catalog listing
(cli.md, CLI-80..85) remains available behind an opt-out. Built on `ratatui` +
`crossterm`.

This document is the spec for that mode. Verbs it drives are defined in cli.md;
the lock it takes per action is defined in storage.md (STO-40, STO-41).

## Entry and modes

- `TUI-1` `mind probe` with no opt-out launches the interactive TUI. `probe` is
  listed first in the command help, as the primary entry point. Launching the TUI
  requires a TTY on stdout.
- `TUI-2` `probe` falls back to the non-interactive catalog listing (CLI-80..85)
  when any of these holds: `--no-tui` is given, `--json` is given, or stdout is not
  a TTY (piped or redirected). The `query`, `--kind`, and `--source` arguments
  apply in both modes: in the listing they filter it (CLI-80, CLI-83); in the TUI
  they seed the initial search and filter state. Bare `mind` (no subcommand) is
  unchanged and does not launch the TUI.

## Browse tree

- `TUI-10` The view has two top-level groups: **Installed** and **Available**.
  Each is an independently collapsible tree.
- `TUI-11` Under each group the hierarchy is source -> kind (skills, agents,
  rules) -> item -> detail. A node expands and collapses; expanding an item shows
  its description and frontmatter, and for a skill its file tree. Navigation is
  keyboard-driven (move, expand, collapse, page, jump to search).
- `TUI-12` **Installed** is the manifest (manifest.json): installed items grouped
  source -> kind -> item, each showing effective name, source, and short commit,
  matching what `recall` reports (CLI-70).
- `TUI-13` **Available** aggregates, de-duplicated: (a) catalog items of melded
  sources that are not installed; (b) not-yet-melded sources suggested by the
  registry (TUI-31); and (c) ad-hoc sources the user enters (TUI-30). A melded
  source's items are known from its catalog; a not-yet-cloned Available source is
  shown as a collapsed node whose items are populated by a preview on expand
  (TUI-30).
- `TUI-14` A search box filters the visible tree by case-insensitive substring
  over item name and description (consistent with CLI-85), across both groups, and
  composes with the active kind and source filters. Clearing the search restores
  the full tree.
- `TUI-15` The TUI polls the on-disk registry (sources.json) and manifest
  (manifest.json) on a short interval (about once a second), under a brief shared
  lock (TUI-25), so changes made by another `mind` process or a direct edit appear
  without a manual reload. A refresh preserves the current selection, expansion,
  and search state, and is skipped while a mutating action holds the lock. Catalog
  contents are re-scanned when the melded source set changes or after a sync, not
  on every tick.

## Actions (CLI parity)

Each action invokes the same verb the CLI exposes, against the same registry,
manifest, and store.

- `TUI-20` Install the selected Available item (`learn`, CLI-30); uninstall the
  selected Installed item (`forget`, CLI-40).
- `TUI-21` Meld the selected/entered source (`meld`, CLI-10); unmeld a melded
  source (`unmeld`, CLI-20), offering the `--forget` purge (CLI-22).
- `TUI-22` Sync all or the selected source (`sync`, CLI-50); upgrade pending or the
  selected item(s) (`upgrade`, CLI-60), showing the same deltas and confirming
  before applying (CLI-61).
- `TUI-23` View and manage agent homes (`config lobes` list / add / remove,
  CLI-111..113).
- `TUI-24` Every mutating action confirms before applying; destructive actions
  (`forget`, `unmeld --forget`) require an explicit confirmation. Results and
  errors are shown inline; a `MindError` is surfaced in the UI, not printed to a
  hidden stderr. The verb's own stdout is captured for the duration of the action
  so it cannot corrupt the alternate screen; a one-line summary of it is shown in
  the status bar. After a successful mutation the affected tree refreshes.
- `TUI-25` The TUI holds no lock while idle. It acquires the global lock
  (storage.md) only for the duration of a single operation: a shared lock while
  loading or refreshing state, an exclusive lock for each mutating action
  (STO-40, STO-41), releasing it immediately after. A running TUI therefore never
  blocks another `mind` invocation for longer than one operation.

## Preview and registry (browsing the not-yet-melded)

- `TUI-30` Entering a repo spec (any form `meld` accepts, CLI-11) shallow-clones
  it to a temporary preview area and shows its catalog tree under Available
  without registering it. Confirming meld promotes the preview to a real source
  (registers it and keeps the clone, CLI-10); declining discards the temp clone.
  This is the interactive form of `meld`.
- `TUI-31` The Available registry of suggested, not-yet-melded sources is the
  union of the `[discover].sources` entries declared by all melded sources
  (DSC-38), de-duplicated by URL and excluding sources already melded. Expanding a
  registry entry previews it (TUI-30); confirming melds it (TUI-21).

## Terminal handling

- `TUI-40` The TUI enters and leaves the alternate screen / raw mode cleanly and
  restores the terminal on normal exit, on error, and on panic, so a crash never
  leaves the terminal in a broken state.
- `TUI-41` Quitting leaves no partial state: every mutation was already committed
  per-action under the lock (TUI-25), so there is nothing to roll back on exit.
- `TUI-42` Rendering is responsive to the terminal size: the status and key-hint
  lines wrap to the available width (growing to a bounded number of rows), and
  every centered overlay (the confirm modal, the meld and lobe-path input
  dialogs, the lobes modal) is clamped to the terminal width and height. Content
  is never cut off the right edge or pushed off screen on a narrow terminal, so
  there is no minimum-width requirement. The TUI may use Unicode (box drawing,
  geometric node markers) for presentation; the ASCII-only convention applies to
  written prose, not the interface.
