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
already has tags like `v1.2.0`.

## Name your release tags

`release-tags` is a template: exactly one `{version}`, with any literal text
before or after it.

| `release-tags` | release tags it matches |
|----------------|-------------------------|
| `true` | `v1.2.3` or a bare `1.2.3` |
| `"v{version}"` | `v1.2.3` |
| `"skills-v{version}"` | `skills-v1.2.3` |
| `"release/{version}"` | `release/1.2.3` |
| `"{version}-stable"` | `1.2.3-stable` |

`mind` strips the prefix and suffix as literal text, then reads what is left as
a version:

- The version must be full semver, `MAJOR.MINOR.PATCH`, optionally with a
  prerelease (`-rc.1`) and build metadata (`+build.5`). A tag that fits the
  template but is not semver, such as `v1.2`, is ignored, and `mind review`
  reports it as `non-semver-release-tag`.
- A prerelease goes before any suffix: under `"{version}-stable"`,
  `1.3.0-rc.1-stable` is the prerelease `1.3.0-rc.1` and `1.2.3-stable` is the
  stable `1.2.3`.
- Tags that do not fit the template are not releases, so other tags in the
  repo are left alone. In a repo that tags more than one thing, give each line
  of releases its own prefix and point `release-tags` at the one this source
  ships.
- Two tags for the same version (`v1.2.3` and `1.2.3` under `true`) are fine
  when they point at the same commit. At different commits the version is
  skipped with a warning.

The template must contain exactly one `{version}` and no other `{...}` token,
and a filled-in tag must be a valid git ref name: no whitespace, no `..`, and
no leading `-`. A template that breaks these rules fails the `mind.toml` load,
which `mind review` reports. One source has one template; there is no list of
alternatives, and date or build-number schemes are not versions.

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
stable, non-yanked release (the one a new consumer gets) and reports each of
these as an `apparent-break` advisory. A compatible
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

To have a plain meld track every tagged release instead, new majors included,
set `pin-version = "*"`.

Leave it out otherwise. A consumer's `--pin` overrides it. It is always a
range: an exact version (`"=1.2.3"`) reads as `"^1.2.3"`, and `mind review`
says so, since holding one release is the consumer's choice
(`mind meld <source> --pin version==1.2.3`).

## Recommend a new major

Existing consumers keep the range they melded with, so moving `pin-version`
does not move them. Instead, once `2.0.0` is ready, setting `pin-version =
"^2"` on the default branch adds a recommendation to the breaking-release line
each consumer sees after their next `sync`:

```text
! acme/skills 1.5.0 -> 2.0.0 (breaking, not applied; acme/skills recommends ^2; take it with `mind upgrade --allow-breaking --source 'acme/skills'`)
```

The same hint shows in the `upgrade` prompt and in `recall`. Taking the new
major still needs the consumer's yes or `--allow-breaking`. A recommendation
that does not include the breaking release shows nothing.

## Ask for automatic updates

```toml
[source]
release-tags = "v{version}"
auto-update = "breaking"   # or true / "compatible"
```

This asks consumers who run `mind config auto-update on` to let your source
update without them running `upgrade`. A consumer is asked once, when they meld
on a terminal, and can allow it later with `mind meld <source> --auto-update`.

- `compatible` (or `true`) takes each new release inside the consumer's range.
- `breaking` also moves them to a new major, but only after you recommend it:
  tag `2.0.0`, then set `pin-version = "^2"` on the default branch when they
  should move.

The consumer's choice wins: changing the level never changes what a consumer
already allowed, and raising it only shows them a note. A release whose update would run one of your hooks
is never applied automatically, because a hook needs the consumer's consent.
