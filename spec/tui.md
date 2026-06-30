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
- `TUI-3` `-n` is the short form of `--no-tui` on `probe` (TUI-2): `mind probe -n`
  prints the non-interactive catalog listing. The short is subcommand-scoped, so it
  does not clash with `learn`'s `-n` (`--dry-run`, CLI-32); each is local to its own
  command.

## Browse tree

- `TUI-10` The view has two top-level groups: **Installed** and **Available**.
  Each is an independently collapsible tree. A third **Unmanaged** group appears
  below them when the agent home holds items mind did not install (UNM-6).
- `TUI-11` Under each group the hierarchy is source -> kind (skills, agents,
  rules) -> item -> detail. Left/Right (and Space) collapse/expand structural nodes
  (source and kind buckets); Enter opens the details dialog on a source or item
  (TUI-26) and toggles a group header or kind bucket. Collapse state for structural
  nodes is tracked in a per-node collapsed
  set distinct from the item-detail expansion set, so a source's items can be
  hidden and re-shown independently of item detail. Expanding an item shows its
  description and frontmatter, and for a skill its file tree. Navigation is
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
- `TUI-16` The list keeps the highlighted row within the middle
  two-thirds of the visible area: as the selection moves, the view scrolls to
  maintain a margin of about one-sixth of the visible height above and below the
  highlight, so the highlight does not reach the top or bottom edge while there are
  more rows to scroll in that direction. Near the start or end of the list the
  highlight may sit at the edge, since there is nothing further to scroll. The
  margin is derived from the current viewport height, so it adapts to terminal size
  (TUI-42).

## Actions (CLI parity)

Each action invokes the same verb the CLI exposes, against the same registry,
manifest, and store.

- `TUI-20` Install the selected Available item (`learn`, CLI-30); uninstall the
  selected Installed item (`forget`, CLI-40). Installing on a higher node selects
  in bulk, so the user need not name each item: a Source installs every available
  item from it (`learn '<source>#*'`), a kind bucket every item of that kind
  (`<source>#<kind>:*`), and the Available group everything (`learn '*'`). The
  selection flows through the same closure/confirm path, and already-installed
  items are skipped (DEP-23).
- `TUI-21` Meld the selected/entered source (`meld`, CLI-10); unmeld a melded
  source (`unmeld`, CLI-20). The TUI's unmeld uninstalls the source's installed
  items (the `--forget` purge, CLI-22) by default; this is a destructive action
  and is confirmed before applying (TUI-24).
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
- `TUI-26` Pressing Enter on a source or item opens a details dialog: a centered
  overlay (TUI-42) describing the node and listing the actions valid for it as a
  selectable list, each run through the normal confirm-and-execute path (TUI-24).
  For an item it shows its kind, source, the commit when installed, and the
  description, and offers Install when the item is not installed, else Forget (an
  unmanaged item offers Forget with the not-managed warning, UNM-5). For a source
  it shows its name and installed/available item counts, and offers Install all
  available items, Uninstall all installed items, and Unmeld; an action is omitted
  when it would do nothing (no Install-all with nothing available, no Uninstall-all
  with nothing installed). j/k move the highlight, Enter or y runs the highlighted
  action, and Esc, q, or n dismisses without acting. An Install action arms the
  same dependency-closure preview as the direct `i` action (DEP-40). The dialog is
  an additional entry point and does not remove the direct action keys (TUI-20..22).
  On a group header, kind bucket, or suggested source there is no dialog, so Enter
  keeps its TUI-11 toggle/preview; expansion is also on Space and Left/Right.
- `TUI-50` An item node is expandable to show its dependencies. Space (and
  Left/Right) on an item node toggles a dependency subtree: its direct
  dependencies (DEP-4, the union of `{{ns:}}` and `requires` edges) appear as child
  nodes, each itself expandable, so the user walks the graph in place. It is
  cycle-safe: a dependency that would revisit an ancestor on the current path is
  shown as a marked back-edge and not expanded again (DEP-22). This extends the
  TUI-11 expansion (which toggled only source and kind buckets) to item nodes. The
  dependency children are a view of the graph, distinct from each item's own
  canonical line under its source -> kind bucket.
- `TUI-51` Pressing Enter on a dependency child node (TUI-50) moves the cursor to
  that dependency's canonical item line (its node under its source -> kind bucket),
  expanding any collapsed ancestors needed to reveal it, rather than opening the
  details dialog. Enter on a normal item line keeps the TUI-26 details dialog. So
  Enter on a dependency navigates to the real item, where its own actions and
  dependency subtree are then available.

## Preview and registry (browsing the not-yet-melded)

- `TUI-30` Melding a hand-entered repo spec (the `m` action; any form `meld`
  accepts, CLI-11) runs the interactive `meld` directly: the TUI suspends to the
  normal terminal (TUI-44) and runs `meld` (CLI-10) so the clone and every prompt
  -- the namespace prompt (CLI-24), the install-hook disclosure (HOOK-20), the
  install-items confirmation (CLI-23), and any SSH passphrase or host-key prompt
  (TUI-45) -- behave exactly as they do from the CLI. No preview is pre-cloned: the
  source is already named. This is the interactive form of `meld`.
- `TUI-31` The Available registry of suggested, not-yet-melded sources is the
  union of the `[discover].sources` entries declared by all melded sources
  (DSC-38), de-duplicated by URL and excluding sources already melded. Expanding a
  registry entry shallow-clones it to a temporary preview area and shows its
  catalog tree under Available without registering it; confirming promotes the
  preview to a real meld (the suspended interactive flow, TUI-44/TUI-21) and
  declining discards the temp clone.

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
- `TUI-43` Ctrl-C is a force-exit available from every mode (the search box, the
  spec-input and lobe-path inputs, and the modals), not only the normal-mode `q`.
  It is intercepted before mode routing, so a `Char('c')` is never entered as
  text. One Ctrl-C arms and shows a hint; a second consecutive Ctrl-C exits, so a
  single accidental Ctrl-C while typing does not quit. Any other key disarms.
- `TUI-44` Actions whose verbs may prompt interactively run with the TUI
  suspended: `mind` leaves raw mode and the alternate screen, runs the verb on the
  normal terminal so its prompts read stdin and write stdout exactly as the CLI
  does (never captured, never blocked behind raw mode), then restores the alternate
  screen and redraws. After the verb the user presses Enter to return, so the
  verb's output is readable before the browser redraws. The verbs that suspend are
  `meld` (install-hook disclosure HOOK-20, install confirm CLI-23, SSH passphrase
  TUI-45) and `unmeld` (uninstall-hook prompt HOOK-54). Non-prompting mutations
  (learn/forget/sync/upgrade) instead run with stdout captured (TUI-24) and do not
  suspend.
- `TUI-52` When forgetting a single installed item that other installed items
  depend on (DEP-60), the TUI surfaces the warning in the confirmation
  description -- listing the dependent keys -- before the user confirms. The
  action still proceeds on confirmation; the TUI does not block the removal.
  This mirrors the CLI's DEP-60 warning, adapted to the TUI's confirm-then-act
  flow (TUI-24) rather than a stdin prompt.
- `TUI-45` While the TUI holds the terminal, every `git` child runs
  non-interactively (`GIT_TERMINAL_PROMPT=0` and an ssh `BatchMode=yes` wrapper),
  so an auth-required remote -- a private SSH repo whose key needs a passphrase, or
  an unknown host key -- fails fast with an error surfaced inline (TUI-24) instead
  of hanging the UI on a hidden prompt the user cannot see or answer. The suspended
  interactive meld (TUI-44) restores interactive git for its duration, so that same
  passphrase or host-key prompt works on the normal terminal. This is why a typed
  spec (TUI-30) can meld a private SSH source while a background preview, sync, or
  upgrade of one fails fast rather than freezing.
- `TUI-53` The source details dialog (TUI-26) shows the namespace the
  source will install under (its effective prefix: the consumer `--namespace` alias,
  else `[source].prefix`, else none; NS-1). When none of the source's items are
  installed, the namespace is editable in the dialog (an input field), and the
  edited value is persisted as the source alias before the Install-all action runs
  (NS-30). When any of the source's items are installed the namespace is shown
  read-only with a note that it is locked until those items are forgotten (NS-30,
  CLI-161).
