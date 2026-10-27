# Release versions

A version pin follows a source's semver release tags within a compatible range.
The source maintainer declares how release tags are named; the consumer pins a
range. `sync` moves the source to the highest release inside the range, and a
breaking release (outside the range) is reported but applied only on explicit
consent. The range model is cargo's caret requirements.

```toml
[source]
release-tags = "v{version}"
```

```sh
mind meld owner/repo                         # pins ^<highest release> when release-tags is declared
mind meld owner/repo --pin version=^1.2      # >=1.2.0, <2.0.0
mind meld owner/repo --pin version==1.2.3    # hold at exactly 1.2.3
mind upgrade                                 # compatible moves; breaking ones listed, not applied
mind upgrade --allow-breaking --source repo  # take the next major for one source
```

## Declaration

- `VER-1` `[source].release-tags` declares the source's release tag template: a
  string containing exactly one `{version}` placeholder, with an optional literal
  prefix and suffix (`"v{version}"`, `"skills-v{version}"`). `release-tags =
  true` is the default template, which matches both `v{version}` and a bare
  `{version}`. Like `[profiles]`, it is metadata: it does not make the
  `mind.toml` authoritative.
- `VER-2` A template with zero or several `{version}` placeholders, any other
  `{...}` token, or a tag name that fails `validate_ref_value` (DSC-66) once a
  version is filled in (`-v{version}` does, `{version}-final` does not) is a
  `MindToml` error. A non-string, non-`true` value is a `MindToml` error.
- `VER-3` The template is read from the source's default-branch tip, the same
  place a `[source]` pin directive is read (DSC-41), because it must be known
  before a release can be chosen. It is used only to find releases (and the
  yanked list, VER-46). Everything else (inventory, hooks, profiles,
  `min-mind-version`) comes from the selected release's own `mind.toml`.
- `VER-4` A source that declares no `release-tags` can still be version-pinned:
  `--pin version=<range>` resolves against the default template (VER-1). If no
  tag matches, the meld fails with `NoMatchingRelease` and nothing is registered.

Note that `[source]` rejects unknown keys, so a `mind` that predates this feature
fails to load a `mind.toml` declaring `release-tags`, `yanked`, or `pin-version`.
A source adopting them should also set `min-mind-version`, so the older binary's
error names the version gap (DSC-117) instead of a bare schema error.

## Versions and tags

- `VER-10` A tag is a release when stripping the template's prefix and suffix
  leaves a semver version `MAJOR.MINOR.PATCH` with an optional `-prerelease`
  and `+build`. A tag that matches the prefix and suffix but whose remainder is
  not semver (`v1.2`, `v1.2.3.4`) is skipped, not an error. `review` reports it
  (VER-130).
- `VER-11` Releases order by semver precedence, the same rule `evolve` uses for
  its release tags (storage.md). Build metadata does not order: two tags whose
  versions differ only by `+build`, or the `v1.2.3` / `1.2.3` pair under the
  default template, are the same version. When such duplicates resolve to the
  same commit, either tag stands for the version. When they resolve to different
  commits, the version is ambiguous: it is skipped with a warning naming both
  tags.

## Ranges

- `VER-20` `--pin version=<range>` records a version pin. A range is one of:
  - `^X`, `^X.Y`, `^X.Y.Z`: a caret requirement, with cargo semantics (the
    leftmost nonzero component may not change, so `^1.2` is `>=1.2.0, <2.0.0`,
    `^0.3` is `>=0.3.0, <0.4.0`, and `^0.0.3` is exactly `0.0.3`);
  - a bare `X`, `X.Y`, or `X.Y.Z`, which means the same as the caret form;
  - `=X.Y.Z`, an exact version (so the flag reads `--pin version==1.2.3`).
- `VER-21` A prerelease is never selected unless the range's own version carries
  one, and then only prereleases of that same `MAJOR.MINOR.PATCH` (cargo's rule).
  `^1.3.0-rc.1` admits `1.3.0-rc.2` and `1.3.0`, never `1.4.0-rc.1`.
- `VER-22` A range that does not parse is a `BadPinSpec` usage error, so a
  mistyped range is never taken as a tag name.

## Storage

- `VER-30` A version pin is stored as pin kind `version` (extending STO-18).
  It records the range, the resolved version and the tag name it came from, and
  the highest stable, non-yanked release seen at the last resolution (version
  and tag), whether or not that release is in range. The resolved commit is
  recorded like any other pin's.

## Resolution

- `VER-40` A version pin resolves by listing the remote's tags with `git
  ls-remote --tags` (the shallow clones and fetches `mind` makes carry no tag
  list), selecting the highest release inside the range, then shallow-fetching
  and checking out that tag. An annotated tag resolves to the commit it peels to
  (`<tag>^{}`).
- `VER-41` Resolution honors `min-mind-version` (DSC-40). If the highest
  in-range release declares a `min-mind-version` the running binary does not
  meet, resolution tries the next-highest, in descending order, and selects the
  first one the binary meets. Each skipped release is reported (`skipped 1.4.0:
  needs mind 0.35`). If no in-range release fits, the highest is selected and
  the DSC-40 warning applies. Each release's `min-mind-version` is read
  leniently from its own `mind.toml`, since that file may use keys the running
  binary predates. A release whose `mind.toml` cannot be fetched is warned
  about and treated as fitting, so a failed fetch never fails the resolution.
- `VER-42` `meld` resolves the pin at meld time (CLI-18). If no release is in
  range, the meld fails with `NoMatchingRelease`, which names the range and the
  highest release found, and nothing is registered.
- `VER-43` `sync` re-resolves every version-pinned source (CLI-55). It moves the
  source to the highest in-range release and updates the recorded commit and
  resolved version. It also updates the highest-seen release (VER-30). `sync`
  never changes the range itself.
- `VER-44` A release tag never moves silently. If the resolved release's tag now
  peels to a different commit than the one recorded, `sync` does not follow it.
  It keeps the recorded commit and warns that the tag moved, naming both
  commits. This differs from a `tag` pin, which resets to a moved tag (CLI-55).
  Re-melding with `--pin` accepts the moved tag.
- `VER-45` A moved tag on a release other than the resolved one is not
  detected, since only the resolved release's commit is recorded.
- `VER-46` `[source].yanked` lists versions (`yanked = ["1.2.3"]`), read from
  the default-branch tip (VER-3). An entry that is not a semver version is a
  `MindToml` error. Resolution never selects a yanked version. A
  source already resolved to a yanked version stays there until a `sync` finds a
  newer in-range release; it never steps down to an older one. `recall` marks
  the yanked version (VER-90).

## Default pin

- `VER-50` When the default-branch `mind.toml` of a remote source declares
  `release-tags`, and no other layer supplies a pin (a consumer `--pin`, a
  `[source]` pin directive, a curator pin, or an item link's ref), `meld` pins
  `version=^<highest stable release>` instead of following the default branch.
  The highest stable release skips prereleases and yanked versions (VER-46),
  and a release whose `min-mind-version` the binary does not meet, as VER-41
  skips one; when no stable release fits, the highest is used and the DSC-40
  warning applies. For `1.4.2` that is `^1.4.2`; for `0.3.1` it is `^0.3.1`.
  If the template matches no stable release, `meld` follows the default branch
  as before and says so. A local source gets no default: it stays linked to its
  working tree (CLI-27), and an explicit version pin still applies to it
  (VER-81).
- `VER-51` `[source]` gains a fourth pin directive, `pin-version = "<range>"`.
  It joins the DSC-41 one-of rule (declaring it with another pin directive is a
  `MindToml` error), and a consumer `--pin` overrides it. A range that does not
  parse (VER-20) is a `MindToml` error.
- `VER-52` When VER-50 applies, `meld` notes on stderr that the source follows
  releases, naming the range and the selected release, and that `upgrade`
  takes compatible releases and asks before a new major:
  `note: <source> follows releases ^1.4.2 (now 1.4.2); ...`. The note follows
  the managed-policy check (VER-100), so a meld the policy refuses does not
  print it.

## Upgrade

- `VER-60` A compatible move (`sync` landed on a newer in-range release)
  appears in `upgrade` as an ordinary content change: the CLI-60 prompt, the
  CLI-62 `--yes`, and the CLI-61 report apply unchanged. The report shows the
  version delta (`version 1.4.2 -> 1.5.0`) next to the commit delta. Each
  installed item records the release it was installed from in the manifest;
  an item installed before that record existed shows `?` as its old version.
- `VER-61` When the highest-seen release (VER-30) is newer than the resolved
  one and outside a caret range, `upgrade` lists it, one line per source:
  `<source> 1.5.0 -> 2.0.0 (breaking, not applied; ...)`, naming the
  `--allow-breaking --source` command that takes it. An exact hold (`=X.Y.Z`)
  is never offered a breaking move.
- `VER-62` Breaking moves are settled before the upgrade pass. On a TTY without
  `--yes`, each is offered with its own `[y/N]` prompt (default No). Accepting
  rewrites the stored range to the caret of the new version (`^2.0.0`) and
  resolves it, so that source's items then upgrade through the ordinary pass
  (the CLI-60 prompt, `--yes`). If the new range resolves to nothing, the old
  pin stays and the failure is reported.
- `VER-63` `upgrade --allow-breaking` accepts every listed breaking move without
  prompting. The `[item]` and `--source` filters (CLI-63) narrow it like any
  other upgrade. `--yes` alone never accepts a breaking move: with `--yes`, or
  without a TTY, and without `--allow-breaking`, breaking moves are listed and
  skipped.
- `VER-64` The breaking prompt shows the target tag's annotation message when it
  is an annotated tag, sanitized like other source-derived text (`sanitize.rs`),
  without a signed tag's signature block and capped at 20 lines. A lightweight
  tag shows no notes, and neither does a message that cannot be fetched: the
  notes never block the prompt.
- `VER-65` A compatible move that looks breaking is warned about: one that drops
  an installed item from the catalog, changes an installed item's kind, or
  removes the referent of a `{{ns:}}` token in an installed item. The warning
  names each case and the release, one line each:
  `<source> 1.0.0 -> 1.1.0 looks breaking: drops agent:gone`. It is advisory
  and does not block the upgrade.

## Hooks

- `VER-70` A version move is a source move like any other. HOOK-55, HOOK-121,
  and HOOK-122 decide which hooks are pending, and each pending hook gets its
  consent prompt (HOOK-20) or the non-TTY skip (HOOK-22). Neither the automatic
  compatible move, `--yes`, nor `--allow-breaking` grants hook consent. Only
  `--dangerously-skip-install-hook-check` (HOOK-23) does.

## Re-pinning, holds, and other source forms

- `VER-80` Re-pinning an existing source with `meld <source> --pin ...` (the
  re-pin path in cli.md) switches between a range, an exact hold
  (`version==1.2.3`), and any other pin kind without an `unmeld`. Stepping back
  to an older release is a re-pin to an exact version. Returning to a range is
  another re-pin.
- `VER-81` A local source accepts a version pin like any other pin: it is
  snapshotted into the sources tree (CLI-27) and resolved against the local
  repo's tags. A local directory that is not a git repository fails with the
  git error. An item-link instance
  (item-link.md) accepts one: it resolves against the repo's default-branch
  `release-tags` and checks out the item's path at the selected release.
- `VER-82` `recall` adds a one-line suggestion for a source with a `tag` pin
  whose tag is a release under its declared `release-tags`:
  `mind meld <source> --pin version=^<version>`. `recall` does not fetch, so
  the template is read from the clone's checkout (the tagged release) rather
  than the default-branch tip (VER-3). It shows in `recall --sources` and under
  the source in the status view.

## Reporting

- `VER-90` `recall` (the status view and `--sources`) shows a version-pinned
  source's resolved version and range in its header (`release 1.4.2 (^1.4.2)`).
  It marks a source with a breaking release (VER-61) with a line naming the
  release and the `upgrade --allow-breaking --source` command that takes it
  ("breaking release 2.0.0 available"), and a source resolved to a since-yanked
  version (`release 1.1.0 is yanked`). `recall` does not fetch: whether the
  resolved version is yanked is recorded on the pin at the last `sync`, from
  the yanked list VER-46 reads.
- `VER-91` `--json` output gives each version-pinned source a `version` object:
  `range`, `resolved`, `tag`, `highest_available` (the highest stable release
  seen, VER-30), `breaking_available` (bool, VER-61), and `yanked` (bool,
  VER-90). `recall --sources` adds it to each source. A `sync` or `upgrade`
  result carries a `sources` array of `{source, version}` for the
  version-pinned sources it covered. The `upgrade` result lists its breaking
  moves in their own `breaking` array, apart from the applied changes:
  `{source, from, to, applied}`, `applied` false for one listed and left. A
  `sync --upgrade` result carries its upgrade pass's `sources` and `breaking`.

## Policy, curators, and dump

- `VER-100` Under `[sources].pinned = true` (POL-20), an exact version pin
  (`=X.Y.Z`) counts as pinned. A range counts as pinned only when the policy
  also sets `[sources].allow-version-ranges = true`. Otherwise it is refused
  with `UnpinnedSourceForbidden`.
- `VER-101` A `[[sources.auto_meld]]` entry accepts `version = "<range>"` as its
  pin (POL-30). The value is validated as a range (VER-22) at policy parse time,
  and POL-21 accepts it under the same terms as VER-100. POL-55's drift check
  compares the range only, so the resolution recorded beside it is not drift.
- `VER-110` A curator's `[[discover.sources]]` entry accepts `pin-version`
  (DSC-41's one-of rule). `curate` re-pins a curated source whose declared range
  changed, as it does for other pin changes.
- `VER-120` `dump` keeps freezing every source to its recorded commit (DUMP-1).
  `dump --ranges` emits `pin-version = "<range>"` for a version-pinned source
  instead, so the output tracks compatible releases. An item-link instance
  keeps its `pin-ref`, since its URL's ref segment is that commit (LNK-13).

## Author side

- `VER-130` `review` checks a source's `release-tags`: that it parses (VER-2),
  that at least one tag is a release, and which tags match the prefix and suffix
  but are not semver (VER-10). It also runs the VER-65 checks between the
  highest release and the reviewed tree, so an author sees an apparent break
  before tagging a compatible version. A template that does not parse fails the
  `mind.toml` load, the existing `toml-parse-error` hard finding; the rest are
  advisories: `no-release-tags`, `non-semver-release-tag`, `apparent-break`,
  and `release-tags-unchecked` when the tags cannot be listed or the highest
  release cannot be checked out. Tags come from the local repo for a local
  target and from the remote for a remote or melded one.
- `VER-131` `init-source` adds `release-tags = "v{version}"` to the generated
  `mind.toml` when the repo already has tags that are releases under that
  template.

## Not covered

- Cross-source version requirements (source A needs source B `^1`). Dependency
  resolution stays within one source (dependencies.md).
- Per-item versions inside one repo, beyond what a tag prefix plus an item-link
  instance gives.
- Signed-tag verification. A `require-signed-tags` policy is a possible
  addition. Nothing here rules it out.
