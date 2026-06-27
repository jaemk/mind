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
mind.toml                              [[items]] inventory (authoritative; convention off)
guidelines/style.md                    rule 'style', custom link -> rules/house-style.md
guidelines/internal.md                 shipped but NOT listed -> not offered
components/scan/SKILL.md               skill 'scan', non-conventional dir, install/uninstall hooks
components/scan/hooks/post.sh          per-item install hook (bash ./hooks/post.sh)
components/scan/hooks/pre.sh           per-item uninstall hook (bash ./hooks/pre.sh)
```

The hook scripts live inside the skill directory so they are copied into the
store (`~/.mind/store/skill/scan/`) and resolve from the hook working directory.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/explicit /tmp/explicit-demo
cd /tmp/explicit-demo && git init -q && git add -A && git commit -qm init

mind meld /tmp/explicit-demo
```

List the catalog without launching the TUI:

```
mind probe --no-tui
```

`probe` offers only `style` and `scan`; `internal` never appears because it is
not listed in the authoritative inventory.

Install both items:

```
mind learn style
mind learn scan --dangerously-skip-install-hook-check
```

`learn scan` runs the install hook and prints:

```
explicit-example: scan installed
```

Confirm the custom link target for `style`:

```
ls -l ~/.claude/rules/house-style.md
```

That symlink is the result of the `link = "rules/house-style.md"` override in
`mind.toml`. Without it, convention would have placed the link at
`rules/style.md`.

## Teardown

Remove the learned items in reverse order. Forgetting `scan` fires the uninstall
hook:

```
mind forget scan --dangerously-skip-install-hook-check
```

Output:

```
explicit-example: scan removed
```

Then remove `style` and drop the source:

```
mind forget style
mind unmeld explicit-demo
rm -rf /tmp/explicit-demo
```

## See also

- `../../spec/discovery.md` - authoritative `[[items]]` inventory (DSC-3), custom `path` and `link` fields
- `../../spec/install-hooks.md` - per-item `install`/`uninstall` hooks (HOOK-80, HOOK-81, HOOK-82)

## Verified

`tests/cli.rs::example_explicit_inventory_offers_only_listed` melds this
directory and asserts the catalog offers only the listed items.
`tests/cli.rs::example_explicit_item_hooks_fire` learns and forgets `scan` and
asserts both hook output lines appear, so the example stays correct as the code
changes.
