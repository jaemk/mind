# Keep everything up to date

Installed items do not change on their own. `mind` copies each one into its
store, so upstream changes reach you only when you ask for them.

```
mind upgrade              # fetch, show what changed, confirm, apply
```

## Upgrade installed items

`mind upgrade` fetches every source an installed item came from, then reports
each item whose content changed, with its content hash and commit before and
after. Nothing changes until you confirm.

```
mind upgrade                       # everything
mind upgrade skill:review          # one item
mind upgrade 'owner/repo#*'        # every item from one source
mind upgrade --no-sync             # skip the fetch; compare against the clones on disk
mind upgrade --yes                 # apply without asking
```

A filter that matches no installed item reports that everything is up to date
rather than erroring. Each upgrade is transactional: the new copy is built
first and the old one is kept until the swap succeeds, so a failed upgrade
leaves the previous version in place.

`upgrade` also:

- offers items a source added to an [install profile](choosing-items.md#install-profiles)
  you enabled, and reports items that left one without removing them. This
  runs on a plain `upgrade` or a source-wide one, not on `upgrade <item>`.
- re-runs a source's install hooks when the source moved to a new commit, or
  its update hooks if it declares any. See [Run setup with hooks](install-hooks.md).
- reports a rename when an item's installed name changed (for example after a
  source added a namespace) and installs the new name before removing the old.

When the scope spans several sources, `upgrade` names each one before running
any hooks, even with `--yes`.

## Refresh sources without upgrading

```
mind sync                 # fetch every source
mind sync acme            # only sources whose name matches acme
```

`sync` updates the clones and catalogs, so `probe` and `recall` show what is
new, but installed items do not change. `recall` and `probe` mark installed
items that are out of date. `sync` exits non-zero if any fetch failed, after
saving what did succeed.

`mind update` is an alias for `sync`, not for `upgrade`. `sync --upgrade` still
works as shorthand for a sync followed by an upgrade, but plain `upgrade`
already syncs first.

## Check for drift and broken links

```
mind introspect           # report drift, broken links, unsynced sources
mind introspect --fix     # recreate missing links
```

Drift means an installed item's content no longer matches its source. Editing a
store copy by hand causes it, and so does a source that changed without an
upgrade. `introspect --fix` repairs links but never changes versions; use
`upgrade` for that.

## Follow curators

If you melded a curated list of sources, `mind curate` applies the curator's
own changes (new entries, re-pins, removals). `sync` and `upgrade` do not. See
[Follow a curated collection](curate.md).

## Update the mind binary

```
mind evolve --check       # is there a newer release?
mind evolve               # download, verify, and replace the binary
mind evolve --to 0.29.0   # a specific version
```

See [Install](install.md#updating) for how `evolve` verifies the download, and
[Restricted networks](enterprise.md) for proxies and IT-managed binaries.

## Automate it

```
mind upgrade --yes
```

A non-interactive run skips install and build hooks unless you pass
`--dangerously-skip-install-hook-check` and
`--dangerously-skip-build-hook-check`. Both run code from the source, so use
them only for sources you trust. See [Scripts and CI](scripting.md).
