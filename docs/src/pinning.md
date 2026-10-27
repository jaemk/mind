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
| `version=<range>` | follow semver release tags in a range; `sync` moves to the highest release in range |

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

A source's own `mind.toml` can declare a pin (`follow-branch`, `pin-tag`,
`pin-ref`, or `pin-version` under `[source]`). A `--pin` you pass overrides it.

## Follow release versions

`--pin version=<range>` follows a source's semver release tags. A range is any
cargo version requirement: a caret, a comparator or comma-separated list of
them, a tilde, a wildcard, or an exact version:

```
mind meld owner/repo --pin version=^1.2          # >=1.2.0, <2.0.0
mind meld owner/repo --pin version=0.3           # >=0.3.0, <0.4.0
mind meld owner/repo --pin 'version=<2.0'        # any release below 2.0.0
mind meld owner/repo --pin 'version=>=1.2, <1.5' # 1.2.0 up to, not including, 1.5.0
mind meld owner/repo --pin version=~1.2          # >=1.2.0, <1.3.0
mind meld owner/repo --pin 'version=*'           # every stable release, new majors included
mind meld owner/repo --pin version==1.2.3        # hold at exactly 1.2.3
```

Quote a range that contains `<`, `>`, `*`, or a space so the shell passes it
through.

`sync` (and `upgrade`, which syncs first) moves the source to the highest
release in range. A prerelease is selected only when the range names one
(`^1.3.0-rc.1`). A release whose `min-mind-version` this `mind` does not meet
is skipped in favor of the next-highest one in range.

`upgrade` shows the release delta next to the commit delta
(`version 1.1.0 -> 1.2.0`). It also warns when a release inside the range looks
breaking: it drops an installed item, changes an installed item's kind, or
removes the target of a `{{ns:}}` reference. The warning does not stop the
upgrade. A dropped item stays installed, so the warning repeats until you
remove it with `mind forget --dropped`.

A release outside the range, such as `2.0.0` under `^1.2` or `<2.0`, is a
breaking release. `upgrade` lists it and does not apply it unless you accept:

```
mind upgrade                                      # on a terminal: [y/N] per breaking release
mind upgrade --allow-breaking --source owner/repo # take it without a prompt
```

The prompt shows the release tag's message when the tag is annotated.
Accepting moves the range to the new major (`^2.0.0`) and upgrades the
source's items. `--yes` alone never takes a breaking release, and without a
terminal it is listed and skipped. An exact hold (`=1.2.3`) is never offered
one. No release move, compatible or breaking, consents to the source's hooks:
they are prompted as usual, or skipped without a terminal.

A release tag that is re-pointed after `mind` resolved it is not followed:
`sync` warns and keeps the recorded commit. Re-run `meld --pin` to accept it.

The source declares how its tags are named with `release-tags` under
`[source]`, read from its default branch:

```toml
[source]
release-tags = "skills-v{version}"   # default: v{version} or a bare {version}
yanked = ["1.4.1"]                   # never selected; a source on it stays put
pin-version = "^1"                   # optional: the range a plain meld pins
min-mind-version = "0.33"            # older mind versions reject these keys
```

A plain `mind meld owner/repo` of a source that declares `release-tags` pins
the caret of its highest stable release (`^1.4.2` for `1.4.2`) instead of
following the default branch. A release whose `min-mind-version` this `mind`
does not meet is passed over for the next-highest one. The meld prints the
range it chose. With no stable release yet it follows the default branch and
says so. A local path meld stays linked to the working tree; pass
`--pin version=<range>` to pin it.

`recall` shows a version-pinned source's release and range
(`release 1.4.2 (^1.4.2)`), and marks a breaking release available or a
resolved release that was since yanked. Maintainers: see
[Publish versioned releases](releasing.md).

A source pinned with `--pin tag=<tag>` to one of its releases gets a hint in
`recall` with the `meld --pin version=^<version>` command that follows
compatible releases instead.

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

A [managed policy](policy.md) can require every source to be pinned to a tag,
a commit, or an exact release version, refusing floating branches. A version
range counts as pinned only when the policy sets `allow-version-ranges`.

## Older flag spellings

`--follow-branch <b>`, `--pin-tag <t>`, and `--pin-ref <c>` still work as
hidden aliases for `--pin branch=<b>`, `--pin tag=<t>`, and `--pin <c>`.
