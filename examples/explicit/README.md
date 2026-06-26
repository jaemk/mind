# Explicit inventory example

A source repo with a non-standard layout that declares its inventory explicitly
with `[[items]]` in `mind.toml`.

## What it shows

Declaring any `[[items]]` makes the file authoritative: convention scanning is
turned off and only the listed entries are offered.

* Export control via omission. `guidelines/internal.md` ships in the repo but is
  not listed, so it is not in the catalog. Presence on disk does not imply
  availability.
* Custom path and link. The `style` rule lives at `guidelines/style.md` (not the
  conventional `rules/style.md`) and installs its symlink as
  `rules/house-style.md` in the agent home via a custom `link`.
* Per-item install/uninstall hooks. The `scan` skill declares `install` and
  `uninstall` commands that run host side effects: `install` after the store
  copy and links are in place, `uninstall` before the item's paths are removed.
  These are per-item and distinct from source-level `[[hooks]]`, which run on
  meld/unmeld for the whole source.

## Layout

```
mind.toml                  [[items]] inventory (authoritative; convention off)
guidelines/style.md        rule 'style', custom link -> rules/house-style.md
guidelines/internal.md     shipped but NOT listed -> not offered
components/scan/SKILL.md    skill 'scan', non-conventional dir, install/uninstall hooks
hooks/post.sh              per-item install hook (bash ./hooks/post.sh)
hooks/pre.sh               per-item uninstall hook (bash ./hooks/pre.sh)
```

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/explicit /tmp/explicit-demo
cd /tmp/explicit-demo && git init -q && git add -A && git commit -qm init

mind meld /tmp/explicit-demo
mind probe        # lists only 'style' and 'scan', not 'internal'
```

`probe` offers the two listed items; `internal` never appears because the
authoritative inventory does not list it.

## Verified

`tests/cli.rs::example_explicit_inventory_offers_only_listed` melds this
directory and asserts the catalog offers only the listed items, so the example
stays correct as the code changes.
