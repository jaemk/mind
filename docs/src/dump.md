# Back up and reproduce your setup

`mind dump` writes your current sources and installed items as one `mind.toml`.
Melding that file on another machine, or on this one after a reset, recreates
the same sources at the same commits with the same items installed.

```
mind dump --output my-setup.toml     # write it
mind meld ./my-setup-repo --yes      # reproduce it elsewhere (see below)
```

## What it writes

The output is a super-source: a `mind.toml` whose `[discover].sources` lists
each melded source. Each entry:

- pins the source to the commit it is at now (`pin-ref`), overriding any pin
  the source declares, so the reproduction gets exactly what you have.
- carries the meld-time settings: namespace, scan roots, and added roots.
- records which items are installed:
  - every item: `install = true`
  - none: `install = false`
  - some: `install-items = ["skill:review", "agent:dev"]`
  - for a source installed through [profiles](choosing-items.md#install-profiles):
    `profiles = [...]` (and `default-profile = false` when `default` is off),
    plus `install-items` for anything you installed by name on top.

An [item link](item-links.md) source is written back as its URL, so a skill you
installed from a pasted link reproduces too.

```
mind dump                          # to stdout
mind dump --output snapshot.toml   # to a file
mind dump --whole-sources          # install = true for every source
```

`--whole-sources` drops the per-item filtering and offers each source's full
catalog instead of the subset you installed. With no sources melded, `dump`
writes a valid empty super-source.

## Reproduce it

A meld takes a repo, so put the file at the root of a git repo as `mind.toml`:

```
mkdir my-setup && cd my-setup
mind dump --output mind.toml
git init -q && git add mind.toml && git commit -qm "agent setup"
mind meld . --yes
```

Push that repo somewhere private and any machine can reproduce the setup with
`mind meld <url> --yes`. To keep teammates in step as the list changes, see
[Share a setup with your team](team-setup.md); `mind curate` applies the
changes on each machine.

## Keep a backup current

Run `dump` again after changes and commit the result. The diff shows what was
added, removed, or moved to a new commit since the last snapshot.
