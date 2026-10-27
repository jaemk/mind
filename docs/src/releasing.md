# Publish versioned releases

Declare `release-tags` and tag semver releases, and consumers follow your
releases instead of your default branch. A meld installs your highest release,
`upgrade` takes compatible releases, and a new major waits for each consumer to
accept it.

## Opt in

```toml
# mind.toml, on the default branch
[source]
release-tags = "v{version}"   # or true for v{version} or a bare {version}
min-mind-version = "0.33"     # older mind versions reject release-tags
```

Then tag releases as usual:

```
git tag v1.0.0
git push origin v1.0.0
```

`mind init-source` adds `release-tags = "v{version}"` for you when the repo
already has tags like `v1.2.0`. In a repo that tags more than one thing, give
the template a prefix (`"skills-v{version}"`) so only those tags count.

## What consumers get

```
$ mind meld owner/repo
note: github.com/owner/repo follows releases ^1.4.2 (now 1.4.2); `mind upgrade` takes compatible releases and asks before a new major
```

- `meld` installs from your highest stable release, `1.4.2` here, and pins the
  caret range `^1.4.2`. Prereleases (`1.5.0-rc.1`) are skipped, and so is a
  release whose `min-mind-version` the consumer's `mind` does not meet.
- `upgrade` moves to your newest `1.x` release and shows
  `version 1.4.2 -> 1.6.0` next to each changed item.
- `2.0.0` is a breaking release. `upgrade` lists it and leaves it, prompts for
  it on a terminal (showing your tag message if the tag is annotated), and takes
  it with `mind upgrade --allow-breaking --source owner/repo`.
- `recall` shows the release each source is on and marks one with a breaking
  release available.

A consumer can still pin your source to anything else with `--pin`. See
[Pin a source to a version](pinning.md#follow-release-versions) for their side.

## What a release contains

`release-tags`, `yanked`, and `pin-version` are read from your default branch,
since `mind` needs them before it can pick a release. Everything else (items,
hooks, profiles, `min-mind-version`) comes from the tagged commit, so a release
is exactly the tree you tagged.

Do not move a release tag. A consumer already on it keeps the old commit and
gets a warning; tag a new patch release instead.

## When to bump the major

A consumer on `^1.4.2` takes every later `1.x` release without being asked, so
anything that breaks them belongs in a new major. `mind` treats these as
breaking:

- dropping an item, including renaming one
- changing an item's kind (a `rule` that becomes a `skill`)
- removing the target of a `{{ns:}}` reference an item uses

Run `mind review` before you tag. It compares your tree with your highest
release and reports each of these as an `apparent-break` advisory. A compatible
release that does one anyway still installs, with a warning; the dropped item
stays installed until the consumer runs `mind forget --dropped`.

## Yank a release

```toml
[source]
release-tags = "v{version}"
yanked = ["1.4.1"]
```

Commit this to the default branch; no retag is needed. `mind` never selects a
yanked version. A consumer already on it stays there until you tag a newer
release in their range, and after their next `sync` or `upgrade`, `recall`
marks it.

## Steer the default range

`pin-version` sets the range a plain meld pins instead of the caret of your
highest release, for example to keep new consumers on `1.x` while `2.0.0`
settles:

```toml
[source]
release-tags = "v{version}"
pin-version = "^1"
```

Leave it out otherwise. A consumer's `--pin` overrides it.
