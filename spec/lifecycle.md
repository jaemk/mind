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
> from a user's file. mind is therefore supported on unix; non-unix is
> copy-only and best-effort. This is a documented limitation, not yet addressed.

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
