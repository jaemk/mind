# Pin a source to a version

By default a source follows its remote's default branch: `sync` fetches the
latest commit and `upgrade` offers whatever changed. Pin a source to stay on a
known version, follow a different branch, or roll back.

```
mind meld owner/repo --pin v2.0              # freeze at tag v2.0
mind meld owner/repo --pin branch=stable     # follow the stable branch
```

## Freeze or follow

`--pin` takes one value, and the form decides whether the source moves:

| value | effect |
|-------|--------|
| `HEAD` | freeze at the commit the source currently resolves to |
| `<tag>`, `<sha>`, or `<branch>` | resolve that ref now and freeze at its commit |
| `branch=<name>` | follow that branch; `sync` moves to its latest commit |
| `tag=<name>` | follow that tag; `sync` moves if the tag is re-pointed |

```
# Freeze
mind meld owner/repo --pin HEAD
mind meld owner/repo --pin v2.0
mind meld owner/repo --pin a1b2c3d4e5f6
mind meld owner/repo --pin release-branch    # a snapshot of the branch's tip

# Follow
mind meld owner/repo --pin branch=stable
mind meld owner/repo --pin tag=latest
```

Note that `--pin stable` and `--pin branch=stable` differ. The first takes a
one-time snapshot of `stable` that `sync` never moves; the second keeps
tracking the branch. Use `branch=` to keep following.

A source's own `mind.toml` can declare a pin (`follow-branch`, `pin-tag`, or
`pin-ref` under `[source]`). A `--pin` you pass overrides it.

## Change the pin of a melded source

Run `meld --pin` again on a source you already have:

```
mind meld owner/repo --pin branch=dev        # switch to tracking dev
mind upgrade 'owner/repo#*'                  # bring installed items to it
```

`mind` resolves the new pin, checks out the new commit, and records both.
Nothing changes until every git step succeeds, so a mistyped ref leaves the
source on its old pin. Installed items stay as they are until you `upgrade`.

The terminal UI takes the same flags in its meld input, for example
`owner/repo --pin branch=dev` (see [Browse in the terminal UI](tui.md)).

## Roll back an item

Re-pin the source to the older commit, then upgrade the item. `upgrade` applies
whatever the pin resolves to, older or newer:

```
mind recall --sources                    # find the source's name
mind meld owner/repo --pin <old-sha>
mind upgrade skill:review --yes
```

Pick `<old-sha>` from the forge's history, or run `git log` in the clone under
`~/.mind/sources/`. A tag or branch works too (`--pin tag=v1.4`). To go forward
again, re-pin (`--pin HEAD`, `--pin branch=main`) and `upgrade`.

## Pin a single-item link

An [item link](item-links.md) takes its version from the URL. `learn --pin`
(a flag with no value) freezes a branch link at the branch's current commit:

```
mind learn https://github.com/owner/repo/tree/main/skills/foo --pin
```

## Organization-wide pinning

A [managed policy](policy.md) can require every source to be pinned to a tag
or commit, refusing floating branches.

## Older flag spellings

`--follow-branch <b>`, `--pin-tag <t>`, and `--pin-ref <c>` still work as
hidden aliases for `--pin branch=<b>`, `--pin tag=<t>`, and `--pin <c>`.
