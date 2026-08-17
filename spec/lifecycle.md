# Lifecycle

Install, upgrade, uninstall, and drift detection. Installs are transactional and
preserve the previous version until the new one is proven.

## Install

`install` materializes one catalog item into the store and links it into the
agent homes (a store-only tool is not linked; tooling.md TOOL-3).

- `LIFE-1` The new copy is built in a staging directory, and its `{{ns:}}`
  references are expanded there, before the live install is touched.
- `LIFE-2` A failure during staging or expansion (e.g. `BadReference`) aborts the
  install and leaves any previously installed version untouched.
- `LIFE-3` After staging succeeds, an existing store copy is moved to a backup,
  staging is moved into the store, and the symlink is ensured.
- `LIFE-4` A failure during the swap restores the previous version from the
  backup. On success the backup is dropped.
- `LIFE-5` Install records the item in the manifest: effective name, bare name,
  source, current commit, hash of the source content, store path, link path(s),
  and description.
- `LIFE-6` Re-installing an item replaces its store copy and link cleanly: the
  swap is idempotent for the same effective name on unix, where the existing
  link is mind's own symlink into the store (see the platform-limitation note;
  the non-unix copy fallback is not recognized as mind's own).
- `LIFE-40` The store copy is linked into every configured agent home (see
  STO-14). If any link cannot be created, the links already made and the store
  swap are rolled back. Uninstall removes the recorded link in every home.
- `LIFE-41` Before anything is staged or swapped, every planned link target is
  checked. A target that already exists and is not mind's own symlink (a regular
  file, a directory, or a symlink pointing outside the store) is the user's, so
  the install fails with `LinkOccupied` and touches nothing. A target that is
  absent, or is a symlink into the store (mind's own, e.g. a reinstall), is
  free to write. This keeps `learn` from silently deleting a user's file at the
  link path. The guard is overridable per install with force (`learn --force`,
  CLI-35): when forced, the check is skipped and the conflicting target is
  replaced.
- `LIFE-42` A melded source's item file tree must not contain symlinks. During
  staging, `copy_recursive` uses `symlink_metadata` (which does not follow
  symlinks) and rejects any entry that is a symlink with a clear `Io` error
  carrying the offending path. This prevents a crafted source from exfiltrating
  files outside the item tree or causing unbounded recursion via a symlink to an
  ancestor directory.
- `LIFE-43` A forced install (`learn --force`, CLI-35) that replaces a
  pre-existing foreign target at one link path and then fails on a later link
  leaves every clobbered foreign target restored to its original content. Before
  `ensure_link` removes a foreign target under force, the target is moved to a
  transactional stash inside `~/.mind/.tmp/foreign-stash/`. On any failure
  during the link loop, each stashed target is renamed back to its original link
  path as part of the rollback (alongside removing the partial symlinks and
  restoring the store backup). On success, the stashes are dropped. The
  non-force path is unchanged: `ensure_unoccupied` prevents any foreign target
  from being touched, so no stashing is needed there.

> Platform limitation (non-unix): links are realized as real symlinks only on
> unix. On platforms without symlink support the install falls back to copying
> the item into the link location. Because the clobber guard (LIFE-41) and the
> idempotent-reinstall path recognize ownership by "is a symlink into the store",
> that fallback copy is not recognized as mind's own: a reinstall or `upgrade`
> over it reports `LinkOccupied`, and `introspect`/`forget` cannot tell it apart
> from a user's file. Rather than that best-effort breakage, mind refuses cleanly
> on a non-unix platform (LIFE-50).

- `LIFE-50` mind supports only unix-like platforms, where installed items are
  linked with real symlinks. On a non-unix platform an item install is refused up
  front at the single item-install chokepoint with a clear "unsupported platform"
  error, rather than falling back to the unrecognized copy that later breaks
  reinstall/upgrade with `LinkOccupied`. The refusal is `cfg`-gated and a no-op on
  unix; only the guard's unix (supported) branch is exercised by the test suite,
  as CI does not run on a non-unix host.

## Upgrade

- `LIFE-10` upgrade matches each installed item to a catalog item by stable
  identity `(source, kind, bare_name)`, not by effective name.
- `LIFE-11` An item is pending when its source-content hash changed, or its
  effective name changed (a namespace change), or both.
- `LIFE-12` An installed item with no catalog match is left alone by upgrade and is
  reported by introspect.
- `LIFE-13` Applying a content-only upgrade reinstalls under the same effective
  name (transactional swap).
- `LIFE-14` Applying a rename installs the new effective name first, then removes
  the old item via its file registry and re-keys the manifest entry. The old
  version is not removed until the new install succeeds.
- `LIFE-15` The hash recorded and compared is of the source content, not the
  expanded store copy, so detection compares source with source.
- `LIFE-46` Before applying a rename (LIFE-14), `upgrade` looks up the rename's
  new effective key in the manifest. If an item is already installed under that
  key with a DIFFERENT stable identity `(source, kind, bare_name)` than the item
  being renamed, `upgrade` refuses with `UpgradeRenameCollision` -- naming the
  colliding key, the existing occupant's source, and the incoming (pre-rename)
  item and its source, plus both real remedies in its own message: `mind forget
  <key>` to remove the occupant, or re-namespace the incoming source with `mind
  meld -N <prefix> <repo>` to avoid the collision -- instead of evicting the
  occupant. `upgrade` ALSO refuses when two items in the SAME batch resolve to
  one target key with different identities (e.g. two sources each dropping
  their namespace prefix in one upgrade so both land on the bare key): the
  pre-existing-manifest check alone would pass both, since neither key is
  occupied yet, and the first install would then become the second's evicted
  "previous version". Both checks run after the pending report is printed
  (LIFE-51: a human run always sees the full pending list before an abort, not
  just the collision) but before any item in the batch is applied, so a
  detected collision leaves nothing changed. There is no single-item workaround
  that resolves the SAME-BATCH collision by scoping to one identity at a time:
  once the first `upgrade <item>` lands on the shared target key, the
  pre-existing-manifest check then refuses the second identity permanently --
  only the two remedies named in the error (`mind forget`, or re-namespacing
  with `mind meld -N`) actually clear it. This mirrors `learn`'s own collision
  guard (DEP-23): a third-party source's `mind.toml` edit must not let an
  upgrade silently delete a different, unrelated source's installed item -- no
  hook, no prompt.
- `LIFE-47` When applying a rename (LIFE-14) whose new install's links overlap
  the old item's links (e.g. an agent, which links under its bare harness name
  regardless of the item's effective name, NS-40), the old item's link removal
  excludes any link the new install already owns. This mirrors the in-place
  upgrade's own link cleanup (LIFE-13): without it, removing the old item's
  full link set would delete the link the new install just created, leaving no
  link on disk for a key the manifest claims is installed.
- `LIFE-48` A failure partway through applying a batch of upgrades (the
  `install_item`/`uninstall_item` calls for one item out of several pending)
  saves the manifest with every upgrade already applied in this pass before
  the error propagates, mirroring `learn`/`forget`'s own failure-path save.
  Without this, an earlier item in the same batch is correct on disk but
  unrecorded: a retry re-runs its install hook, and a completed rename's old
  entry is left pointing at removed paths. The same persist-before-propagate
  rule applies to a source's re-run install hooks (a source-level pass
  separate from the per-item loop) and to a re-meld's hook re-run: an earlier
  hook's recorded run must not be lost when a later hook in the same pass
  fails, or its side effect is silently re-offered next time (mirroring
  HOOK-53's item-level guarantee at the source level). When the save ITSELF
  also fails (the double-failure outcome), the root cause still propagates
  unchanged; the save failure is reported as a separate sanitized warning
  (not a second line starting `error: `, which would read as a second
  candidate for "the" error alongside the one `main.rs` prints for the
  propagated root cause) naming `mind introspect --fix` as the remedy for the
  resulting drift between disk and `manifest.json`.
- `LIFE-49` `sync --upgrade` forwards the same `--yes` the CLI already threads
  into `mind upgrade`/`mind forget` to the `--upgrade` pass, instead of forcing
  it off. `sync --upgrade --yes` therefore applies pending upgrades without
  prompting, exactly like `mind upgrade --yes` would after a `sync`; without
  this, `--yes` is silently dropped and a non-interactive `sync --upgrade` run
  either prompts (a TTY) or refuses (LIFE-45, `--json` or non-TTY) regardless
  of the flag.
- `LIFE-51` `upgrade`'s two `UpgradeRenameCollision` checks (LIFE-46) run AFTER
  the pending-upgrade report is printed, not before: a batch that hits either
  collision still shows the full pending list first, so a human run always
  sees what else was pending before the abort, rather than aborting on the
  collision line alone with no visibility into the rest of the batch. Both
  checks still run before any item in the batch is applied (LIFE-46), so a
  detected collision leaves nothing changed either way.

## Non-interactive confirmation (`--json`)

- `LIFE-45` `--json` is always treated as a non-interactive session for a
  destructive confirmation, the same rule DEP-60 already establishes for
  `forget`'s dependent-item warning: a run with `--json` and no `--yes` refuses
  with `ConfirmationRequired` rather than falling through to a prompt (which
  `--json` cannot answer) or, worse, proceeding unprompted. This applies to
  `unmeld`'s multi-source and multi-item confirmations (CLI-28, CLI-21),
  `forget --unmanaged`'s single-item and bulk confirmations (UNM-5, UNM-8) --
  the worst case, since an unmanaged item is the user's own file or directory,
  not a mind-owned symlink -- `upgrade`'s apply confirmation (LIFE-14),
  `learn`'s dependency-closure confirmation (DEP-31: a `learn` whose closure
  pulls in dependencies beyond the explicit selection), and `evolve`'s
  binary-swap confirmation. `upgrade`'s own text-mode (non-`--json`)
  prompt is unaffected: it reads stdin directly and already treats EOF/no-input
  as a safe decline, so it does not additionally require a real TTY the way the
  other sites above do (mirroring DEP-60's TTY check) -- only `--json` could
  previously bypass it, by skipping the confirm call entirely rather than by a
  TTY distinction.

## Uninstall

- `LIFE-20` Uninstall removes exactly the paths in the item's file registry (its
  links, then its store copy), then deletes the manifest entry.
- `LIFE-21` Removing a path that is already absent is not an error.
- `LIFE-44` Before removing any path from the file registry, `uninstall`
  verifies that each store path is lexically under the mind store root
  (`<mind_home>/store`) and that each link path is lexically under one of the
  configured agent home (lobe) roots. A path that does not satisfy this check is
  skipped with a stderr warning naming the path; it is not removed. `..`
  components in a recorded path are treated as a violation regardless of the
  apparent `starts_with` result. This prevents a doctored manifest from causing
  `forget` to delete files outside mind's ownership.

## Drift (introspect)

- `LIFE-30` A recorded link that is missing on disk is reported.
- `LIFE-31` An installed item whose stable identity no longer matches any catalog
  item is reported as no longer present upstream.
- `LIFE-32` An installed item whose catalog match now has a different effective
  name is reported as a namespace change, directing the user to upgrade.
- `LIFE-33` An installed item whose source-content hash differs from the recorded
  hash is reported as drifted, directing the user to upgrade.
- `LIFE-34` The source-content hash walk (`hash_path` / `collect_files` in
  `hash.rs`) uses `symlink_metadata` (which does not follow symlinks) at every
  step. A symlink entry is included in the hash by its relative path and its
  link-target string, so a retargeting is detected and a symlink cycle cannot
  cause unbounded recursion or a stack overflow.
- `LIFE-35` Each entry in the directory hash uses length-prefixed fields (8-byte
  LE u64 for path length, then path bytes, then 8-byte LE u64 for content
  length, then content bytes) and a 1-byte type tag (`F` for a regular file,
  `S` for a symlink). Together these ensure that distinct `(type, path, content)`
  triples always produce distinct byte streams: a file named `"symlink:foo"` and
  a symlink named `"foo"` with the same target cannot collide, and two entries
  `("ab", "c")` and `("a", "bc")` cannot collide. Single-file and single-symlink
  hashes also carry a type-tag prefix so a symlink hash is always distinct from a
  regular-file hash with matching bytes. Note: this framing change alters every
  stored hash; after upgrading `mind`, all previously-installed items will report
  drift on the next `recall` or `upgrade` run until they are re-installed or
  upgraded (a one-time event).
