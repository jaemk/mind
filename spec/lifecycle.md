# Lifecycle

Install, upgrade, uninstall, and drift detection. Installs are transactional and
preserve the previous version until the new one is proven.

## Install

`install` materializes one catalog item into the store and links it.

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
- `LIFE-6` Re-installing an item replaces its store copy and link cleanly (the
  swap is idempotent for the same effective name).

## Upgrade (evolve)

- `LIFE-10` evolve matches each installed item to a catalog item by stable
  identity `(source, kind, bare_name)`, not by effective name.
- `LIFE-11` An item is pending when its source-content hash changed, or its
  effective name changed (a namespace change), or both.
- `LIFE-12` An installed item with no catalog match is left alone by evolve and is
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

## Drift (introspect)

- `LIFE-30` A recorded link that is missing on disk is reported.
- `LIFE-31` An installed item whose stable identity no longer matches any catalog
  item is reported as no longer present upstream.
- `LIFE-32` An installed item whose catalog match now has a different effective
  name is reported as a namespace change, directing the user to evolve.
- `LIFE-33` An installed item whose source-content hash differs from the recorded
  hash is reported as drifted, directing the user to evolve.
