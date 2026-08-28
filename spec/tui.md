# Interactive TUI

`mind probe` launches an interactive terminal UI: a browsable, searchable view of
every source and item, and the interactive front end for the rest of the CLI
(meld, learn, sync, upgrade, config). The non-interactive catalog listing
(cli.md, CLI-80..85) remains available behind an opt-out. Built on `ratatui` +
`crossterm`.

This document is the spec for that mode. Verbs it drives are defined in cli.md;
the lock it takes per action is defined in storage.md (STO-40, STO-41).

## Entry and modes

- `TUI-1` `mind probe` with no opt-out launches the interactive TUI. It is the
  primary interactive entry point. Launching the TUI requires a TTY on stdout.
  (Bare `mind`, with no subcommand, does NOT launch the TUI; see TUI-2.)
- `TUI-2` `probe` falls back to the non-interactive catalog listing (CLI-80..85)
  when any of these holds: `--no-tui` is given, `--json` is given, or stdout is not
  a TTY (piped or redirected). The `query`, `--kind`, and `--source` arguments
  apply in both modes: in the listing they filter it (CLI-80, CLI-83); in the TUI
  they seed the initial search and filter state. Bare `mind` (no subcommand) is
  unchanged and does not launch the TUI.
- `TUI-71` `probe` also falls back to the non-interactive listing (extending
  TUI-2's conditions) when the output mode is Unicode-hostile: `--ascii` is given,
  or the active locale is not UTF-8 (`LC_ALL`/`LC_CTYPE`/`LANG`, first set wins;
  none set is treated as non-UTF-8). The TUI draws Unicode box-drawing glyphs, so
  a run that asked for ASCII output, or a terminal whose locale cannot render
  them, must not silently launch it. This is distinct from `NO_COLOR`, which does
  NOT force the fallback: the TUI still launches and renders monochrome (TUI-65).
- `TUI-3` (removed, superseded by TUI-54) `-n` was formerly the short form of
  `--no-tui` on `probe`. Removed to free `-n` globally for `--dry-run` (CLI-163).
  `--no-tui` is now long-only; see TUI-54.

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
  edited value is persisted as the source's display prefix (`alias`) before the
  Install-all action runs (NS-30). This edits the display prefix only, not the
  source's identity alias (STO-58), so it never renames the source or relocates
  its clone. When any of the source's items are installed the namespace is shown
  read-only with a note that it is locked until those items are forgotten (NS-30,
  CLI-161).
- `TUI-54` `probe --no-tui` is long-only; its former short `-n` is removed (CLI-164,
  CLI-163). `mind probe --no-tui` is the only accepted form; `mind probe -n` is now
  a usage error.
- `TUI-63` An installed row carries a `stale` flag, computed at the snapshot
  boundary (data.rs) the same way the CLI's CLI-75 outdated check is computed at
  its three call sites: the source content hash no longer matches the recorded
  manifest hash, or the effective name has changed (a rename, e.g. a prefix
  change). A stale row shows a trailing drift marker (`\u{2191}`, ASCII `^` under
  TUI-65) matching the CLI's CLI-155 glyph, and its details dialog (TUI-26) adds
  an `out of date` status line. The upgrade confirm (`u`, TUI-22) is no longer
  blind: instead of a bare "Upgrade all pending items?", it lists every stale
  item by key and source (mirroring the per-item delta the CLI's `upgrade`
  reports before applying, DEP-40), so the user always confirms a specific,
  named set of changes rather than an opaque bulk action. What the confirm says
  when the stale set is empty, and what applying it actually does, are TUI-73;
  how the stale flag itself is kept cheap to recompute on every poll tick is
  TUI-72.
- `TUI-72` Because the poll (TUI-15) recomputes the TUI-63 stale flag for every
  installed item about once a second, its content hash is memoized on the pair
  (item path, cheap stat fingerprint), the fingerprint coming from
  `hash::stat_fingerprint_ignoring`: the same tree walk [`hash_path`] uses, but
  reading each entry's `mtime`, `size`, and -- under `cfg(unix)` -- `ctime` and
  inode number, plus a symlink's target string, instead of its content. Content
  is re-read only when that fingerprint changes.
  `ctime` cannot be set from userland and changes on every write, closing the
  fingerprint's realistic blind spot: an mtime-PRESERVING replacement at
  unchanged size (`cp -p`, `rsync -a`, `tar -p`, `touch -r`, some FUSE/network
  mounts) -- not, as such a scheme is sometimes assumed to be limited by,
  filesystem mtime granularity. The memo is DISPLAY-ONLY, and its reach is
  strictly bounded to the once-a-second poll's own row marker: every verb that
  ACTS on a hash (`upgrade`, `introspect`, `recall`), and the TUI's own
  authoritative recompute on `u` (TUI-74), calls `hash_path` directly, never the
  memo, so a missed fingerprint change can never make the TUI-63 confirm list
  OMIT an item that is truly stale -- TUI-74's recompute always rechecks any item
  the poll's flag did not already mark stale. (The OPPOSITE direction -- an
  already-`stale`-flagged item staying in the confirm list after its drift was
  reverted -- is a separate, real gap; see TUI-74's false-positive note, which
  the memo is not the cause of.) At most a missed fingerprint change can make a
  row's drift marker (the TUI-63 trailing glyph, between polls) lag behind
  reality until the next poll -- pressing `u` recomputes the CONFIRM LIST only
  (TUI-74) and never writes back into the row's own `stale` flag, so the glyph's
  lag is bounded strictly by the next poll, not by "or the next `u` keypress".
  That row-marker lag is NOT bounded to "one tick" either: the memo has no TTL,
  so once a fingerprint fails to move, its content hash is served -- not
  refreshed on the next poll -- until LRU pressure evicts the entry.
  `stat_fingerprint` is not a content hash and is never compared against a
  recorded manifest hash.
  The fingerprint is part of the memo's KEY, alongside the item path, rather
  than a token stored beside the value and compared by hand: a tree whose
  fingerprint moved is simply a key that misses, so no validity check exists to
  get wrong and a value can never be served for a fingerprint other than the one
  asked for. The item's ignore set is deliberately not part of the key, because
  a set that changes which entries the walk sees already moves the fingerprint,
  and two sets that walk the identical files agree on the content hash too
  (IGN-10). Keying this way means an entry per path per distinct fingerprint
  observed, so the memo carries an LRU bound as well as the prune. Each full
  load drops entries whose path has left the catalog (a source unmelded, an item
  removed upstream) AND resizes the capacity to fit the live set, in both
  directions, so the bound tracks the workload rather than a constant fixed in
  advance and a shrunken install set gives its capacity back. The capacity must
  stay above the installed-item count, which is why it is derived rather than
  chosen: the poll hashes every installed item once per tick in a stable order,
  a cyclic sequential scan, so at more items than capacity each tick evicts
  exactly the entries the next tick reaches for first. The hit rate would not
  degrade gradually there, it would collapse, leaving the memo pure overhead: a
  full re-read of every item tree every second, the cost this rule exists to
  avoid. A hash FAILURE is not cached, so it is retried on the next poll instead
  of being remembered.
- `TUI-73` Applying the TUI-63 upgrade confirm (`u`, TUI-22) for a NON-EMPTY
  confirmed key set runs the NO-SYNC, KEY-SCOPED upgrade
  (`commands::upgrade_no_sync_keys`, extending CLI-169's no-sync form with an
  exact-key restriction), never the plain sync-first `upgrade`: a sync-first
  apply could pull new upstream commits between the confirm and the apply and
  act on an item the modal never named. (An EMPTY confirmed set takes a
  different path; see TUI-76.) Refreshing drift is the job of the separate `s`
  (Sync) action plus the ~1s re-poll (TUI-15) or the `u` keypress's own
  recompute (TUI-74), not of the apply step. The applied set equals the
  confirmed set BY CONSTRUCTION, not by two independent computations happening
  to agree: `initiate_upgrade` (TUI-74) stashes the exact item keys its confirm
  list names onto the pending action, and the apply is scoped to precisely that
  key set, so it can act on strictly fewer items than a fresh recompute would
  find stale (one drifted since the confirm) but never on an item the modal
  never named. A confirmed key that has become inapplicable by apply time --
  forgotten, its source unmelded, or the drift that made it stale already
  resolved -- is silently skipped rather than aborting the batch (the same
  "silent skip" the CLI already gives an out-of-scope item under a
  glob-filtered `upgrade <item>`); when every confirmed key has become
  inapplicable this reports the ordinary "everything is up to date", not an
  error. Because upgrade itself never fetches, the confirm's empty-state
  wording says so explicitly rather than implying full currency: "nothing is
  out of date since the last sync" (naming `s` as the remedy), not a bare
  "nothing is out of date" that a user who applies immediately after opening
  the TUI could misread as "you are current" even when upstream has moved.
- `TUI-74` Pressing `u` (`initiate_upgrade`, TUI-22) does not read the TUI-63
  confirm list off the last poll snapshot's `stale` flags verbatim: it
  recomputes staleness for every installed item first, bypassing the TUI-72
  memo for any item the last poll's flag did not already catch. An item whose
  flag is already `stale` (a rename, or content drift the memo did catch) stays
  stale WITH NO REVERIFICATION; for every other item, the keypress reads the
  item's own live catalog path and recorded manifest hash (carried on
  `SnapshotInstalled` itself, set at the same load/poll that computed the
  `stale` flag) and calls `hash_path` on that path directly, exactly as
  `upgrade`/`introspect`/`recall` do (TUI-72), rather than trusting the memo.
  FALSE POSITIVE: because an already-`stale` item is trusted without
  reverification, an item whose memo-served drift was since REVERTED (edited,
  then edited back, before the next poll) stays in the confirm list as a
  phantom until the next poll clears the flag; the no-sync apply then silently
  drops it as no longer actually stale (CLI-75), so this is a display
  inaccuracy in the confirm list, not a correctness bug in what gets applied.
  COST: this recompute pays one extra content hash per installed item the
  poll's flag had not already flagged, on the event thread, synchronously,
  before the confirm modal is even shown -- unlike the no-sync apply (TUI-73),
  which only hashes the CONFIRMED subset (`upgrade_item_disposition` returns
  `OutOfScope` for any key outside the confirmed set before it ever reaches
  `hash_path`), this recompute hashes every not-already-stale installed item,
  wholesale, whether or not the user goes on to confirm. That cost is wasted
  entirely if the user cancels the modal. It closes a usability gap TUI-72's
  memo would otherwise leave open: without this recompute, a memo-lagged item's
  drift is invisible to the TUI until the next poll happens to catch it, so the
  confirm list can omit an item that IS actually out of date and the user has
  no way to upgrade it from the TUI in the meantime. The recomputed set is
  exactly what TUI-63's confirm list names and, when non-empty, exactly what
  TUI-73's apply is scoped to (an empty recomputed set instead takes the
  TUI-76 path).
- `TUI-76` When `initiate_upgrade`'s TUI-74 recompute finds nothing stale, it
  still arms a confirm ("nothing is out of date since the last sync ... Proceed
  with upgrade anyway?", TUI-73) with an EMPTY `upgrade_keys`. Applying that
  confirm runs the UNSCOPED no-sync upgrade (`commands::upgrade_no_sync(..,
  None, ..)`), never the KEY-SCOPED form (TUI-73): a key-scoped apply given an
  empty key set builds `key_scope = Some(<empty set>)`, under which every
  installed item is out of scope, so "anyway" would be a guaranteed no-op
  regardless of what had actually drifted on disk. The unscoped call
  re-derives staleness itself at apply time, so it can still catch and apply a
  drift the last poll/sync missed -- restoring what "anyway" meant before
  TUI-72/TUI-73 introduced the key-scoped path (the prior form was a bare
  `upgrade_no_sync(paths, true, None, ..)`, unconditionally). A NON-empty
  `upgrade_keys` always takes the TUI-73 key-scoped path instead.
- `TUI-64` Pressing `?` in normal mode opens a keymap help overlay listing every
  binding grouped by category (navigation, actions, general); any key closes it,
  intercepted ahead of normal-mode routing so it never leaks into search or an
  action. The bottom hint line (HINTS) additionally names `/` (search), `h/l`
  (collapse/expand), paging, and `?` (help), which it previously omitted
  alongside the always-abbreviated action keys.
- `TUI-65` The TUI honors the same output-capability signals the CLI's
  `render::OutputCtx` already resolves once at startup (CLI-150, CLI-151,
  CLI-154): `NO_COLOR` (any value) disables every `fg`/`bg` color in the tree
  rows, selection highlight, modal borders, and status/hint text -- a monochrome
  style set that keeps BOLD/DIM/REVERSED (video attributes, not colors) so
  structure and selection stay visible without emitting color codes. A non-UTF-8
  locale (the same `detect_utf8_locale` check) disables the Unicode geometric
  markers, disclosure triangles, and highlight symbol in favor of an ASCII
  fallback (`+`/`o`/`*`/`-`/`?`/`^`/`>` for node kinds, `v`/`>` for disclosure,
  `>` for the highlight symbol and the dialog action marker), so a non-UTF-8
  terminal never renders mojibake in place of a box-drawing or geometric glyph.
- `TUI-67` Text measurement for wrapping and modal sizing (TUI-42) is in display
  columns (`unicode-width`), not a raw char count, so a line containing a wide
  CJK/emoji character is not under-counted and does not wrap or size later than
  it actually would on screen. A truncated item description in a tree row
  (`truncate`, 50-char budget) carries a trailing `...` marker, so a cut
  description reads as "more text exists here" rather than looking like the
  description simply ends there; untruncated text is returned unchanged.
- `TUI-68` An empty Installed or Available group shows a call-to-action row
  instead of a bare, unexplained blank list. When no source is melded at all,
  the wording matches the CLI's plain listing (CLI-187): `no sources melded;
  run \`mind meld <owner/repo>\` to add one`. When a search matched nothing, the
  wording is GROUP-SCOPED -- `no installed items match '<query>'` or `no
  available items match '<query>'` -- a deliberate divergence from CLI-187's
  ungrouped `no items match '<query>'`: the CLI's plain listing has no separate
  groups to scope by, but the TUI does, and an ungrouped message on an empty
  Installed group would misleadingly read as "nothing matched anywhere" while
  the search may still have matches sitting in Available (or vice versa). A
  group that is empty for a legitimate reason (sources are melded, nothing has
  been installed yet, and no search is active) gets no synthetic row -- that
  state needs no explanation.
- `TUI-69` Esc on a settled search filter (non-empty, not focused -- the user
  already submitted it with Tab/Enter, TUI-14) is a two-step clear: the first
  Esc in normal mode arms the clear and shows a status hint instead of wiping
  the filter immediately, and only a second CONSECUTIVE Esc clears it. Any
  other intent in between (moving the selection, an action key, and so on)
  disarms the pending clear, so a later, unrelated Esc is treated as a fresh
  first press rather than completing an old one. Esc still clears immediately
  in the two cases where that is the expected, unsurprising behavior: while
  actively typing in the search box (`search_focused`, bailing out of entry),
  and when there is no active filter to protect (nothing to arm).
- `TUI-70` The status/error line's row budget scales with the terminal height
  rather than a flat 3-row ceiling: a chained `MindError` can run several
  sentences, and clamping it to 3 wrapped rows regardless of terminal size cut
  it off with no way to read the rest. A fixed reserve (the search bar, a
  minimum tree row, a minimum hint row) is always held back, so a very long
  message still cannot starve the tree pane on a short terminal, but a taller
  terminal shows correspondingly more of a long message.
- `TUI-62` The lobes action (TUI-23) shares its implementation with the CLI:
  `ActionKind::LobeAdd` dispatches to `commands::lobe_add`, the same function
  `mind config lobes add <path>` calls, with `force` hardcoded to `false`. So the
  CLI's guarantees around it apply to the TUI path with no separate
  implementation: the resolved lobe directory is created before the config entry
  is written (HARN-15), already-installed items of admitted kinds are backfilled
  into it unconditionally (HARN-7, HARN-17), and a pre-existing foreign file at a
  backfill target is reported as a failure -- naming the `--force` remedy -- and
  never clobbered, because the TUI's call site never passes `force`.

## Security

- `TUI-60` All source-derived strings entering the TUI snapshot model --
  item names, descriptions, source names, and any other catalog-controlled text --
  are normalized through `strip_ansi` (the same sanitization applied at the CLI's
  display sites, DSC-69 / MKT-9) before being stored in the `Snapshot`. This
  prevents terminal injection from catalog-controlled content on the TUI's
  default interactive surface, including during destructive-action confirms.
- `TUI-61` The stdout-capture file `with_captured_stdout` uses to keep a mutating
  action's verb output off the alternate screen (TUI-24) is created with the same
  exclusive-create scheme as the `evolve` download/auth-config temp files
  (STO-45, STO-61): an unpredictably-named, mode-0700 temp directory
  (`mktemp_dir_prefixed("mind-tui-capture")`, `create_dir` not `create_dir_all`)
  holding a file opened with `create_new(true)` and mode 0600. A predictable path
  opened with plain `create(true).truncate(true)` (the pre-fix behavior) lets a
  local attacker who pre-creates that path as a symlink get the symlink target
  truncated and overwritten the next time any user on the host runs a capturing
  TUI action, and `remove_file` afterward unlinks only the symlink, leaving the
  target's damage in place. `create_new` refuses to open through a pre-existing
  path (symlink or otherwise), so the write never follows an attacker-planted
  symlink.
- `TUI-75` An item's KEY (`kind:name`) is identity, not display, and is never
  rendered directly: every `SnapshotInstalled`/`SnapshotAvailable`/
  `SnapshotUnmanaged` also carries a `display_key` (`ItemKey::display`, DSC-95)
  computed alongside `key` at snapshot-load time, and every confirm-modal
  description, dependents-list (TUI-52), and other rendered composition built
  from an item's key uses `display_key`, never `key`. `key` itself still drives
  action dispatch (`ActionKind::Learn`/`Forget`'s `item_key`, the TUI-63/TUI-72
  drift lookup, tree node ids): sanitizing the identity field in place would
  break dispatch, and could collapse two distinct hostile names into one --
  itself a vulnerability, since map/set membership and dispatch depend on exact
  identity. UNMANAGED item names (UNM-6) are the one name class with NO
  validation gate equivalent to DSC-96's catalog-scan rejection:
  `unmanaged::scan` reads lobe directory entries straight off the filesystem,
  so a third-party skill pack unzipped into a lobe directory (a normal
  workflow) can carry a name embedding ANSI cursor-repositioning or OSC 52
  clipboard-write escapes that would otherwise reach the unmanaged-forget
  confirm ("Forget {key} (NOT managed by mind: deletes your own file)?")
  unsanitized -- letting a crafted name repaint the disclosure or exfiltrate
  via the terminal while the modal is up.
