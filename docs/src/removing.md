# Remove items and sources

`forget` uninstalls items; `unmeld` drops a whole source.

```
mind forget skill:review          # uninstall one item
mind unmeld acme/skills           # drop a source and uninstall its items
```

## Uninstall items

```
mind forget review                # by name
mind forget skill:review          # when a name exists in more than one kind
mind forget 'acme/skills#*'       # every item from one source
mind forget 'skill:*' --yes       # every skill, without the confirmation
```

`forget` removes the item's store copy and its link in every lobe. A glob that
matches more than one item lists them and asks once; `--yes` skips that.

Removing an item that another installed item depends on asks first, since the
dependent may stop working. `--force` skips only that question. `forget` never
removes dependencies along with an item; uninstall them separately if you no
longer want them.

If the item declares an uninstall hook, `forget` runs it after a prompt. A
non-interactive run skips it unless you pass `--accept-hooks`.

## Clear items a source dropped

```
mind forget --dropped              # every item marked "removed upstream"
mind forget --dropped 'agent:*'    # only the dropped agents
```

An item a source stops offering stays installed, and `recall` marks it
"removed upstream". `--dropped` removes those, plus the items of a source you
dropped with `unmeld --keep-items`.

## Turn off an install profile

```
mind forget acme/skills --profile full
```

This forgets the items installed only for that profile and keeps the rest. See
[Choose what a source installs](choosing-items.md#change-profiles-later).

## Drop a source

```
mind unmeld acme/skills                  # uninstall its items, then drop it
mind unmeld acme/skills --keep-items     # drop it, leave its items installed
mind unmeld '*agents' --yes              # every source matching a glob
```

`unmeld` needs its name to match exactly one source. `repo` or `owner/repo` is
enough when it is unambiguous; a name matching several sources is an error
rather than a guess. Use the full identity to pick one instance, such as
`github.com/acme/skills@jk` for a namespaced instance or
`github.com/acme/skills#skills/foo` for an [item link](item-links.md).

A source can declare uninstall hooks, which run in its clone before the clone
is removed. They prompt like install hooks; `--uninstall-hook <cmd>` replaces
them and `--accept-hooks` runs them without a prompt. A required
uninstall hook that fails or is aborted leaves the source melded.

## Remove things mind did not install

`forget` touches only what `mind` installed unless you ask for more. To remove
a hand-placed skill or agent from a lobe, see
[Adopt items you placed by hand](unmanaged.md#remove-unmanaged-items).

## Stop linking into an agent home

```
mind config lobes remove ~/.gemini/config              # unregister; links stay
mind config lobes remove ~/.gemini/config --snapshot   # replace links with copies, then unregister
```

See [Use more than one agent](lobes.md).

## Uninstall mind entirely

1. `mind unmeld '*' --yes` uninstalls every item and drops every source. Do
   this first: deleting `~/.mind` alone leaves dangling links in your lobes.
2. Optionally `mind forget --unmanaged` to remove items you placed by hand.
3. `rm -rf ~/.mind` (or your `MIND_HOME`) removes the store, clones, and
   config.
4. Remove the binary: delete `~/.local/bin/mind` (or `$MIND_INSTALL_DIR/mind`)
   for the install script, `brew uninstall mind`, or
   `cargo uninstall mind-cli`.
