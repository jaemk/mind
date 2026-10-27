---
name: release
description: Cut a new `mind` release. Bump the version and update the changelog, either in a new release PR or as a commit on an existing PR, and verify CI; merging that PR is the release, and the release workflow tags, builds, and publishes it. Then confirm the workflow succeeded.
---

# release

Cut a release of `mind`. The version in `Cargo.toml` is the single source of
truth, and merging a version bump to `main` is the release: every push to
`main` runs `.github/workflows/release.yml`, whose `decide` job releases only
when `Cargo.toml` carries a stable version (no `-dev` suffix) that has no
GitHub release yet. The workflow then builds the per-platform binaries, creates
the `v<version>` tag and the GitHub Release, publishes to crates.io,
regenerates the Homebrew formula, and puts `main` back on the next `-dev`
version. Nothing is pushed to `main` or tagged by hand.

Releases are SemVer. Pick the bump from what landed since the last tag: a new
verb/flag or other backward-compatible feature is a minor bump; a bug-fix-only
batch is a patch; a breaking change to the CLI, config, on-disk layout, or
`mind.toml` schema is a major bump.

The bump goes in one of two places:

- **A release PR** (default): a dedicated branch off `main` carrying only the
  bump and the changelog. Releases whatever has already landed on `main`.
- **An existing PR**: a bump commit added on top of an open feature PR, so
  merging that PR lands the change and releases it in one step. Use this when
  asked to "release with PR N" or "add the version bump to this PR".

## Steps

### 1. Get onto the branch

For a release PR, start from an up-to-date `main` on a new release branch:

```bash
git switch main
git pull
git status --porcelain                  # must print nothing
git switch -c YYMMDD.release-X.Y.Z      # today's date, the real version
```

For an existing PR, check out its branch and bring it up to date with `main`,
so the version and changelog are bumped from what `main` carries now:

```bash
gh pr view <N> --json headRefName,mergeable
git switch <head branch>
git pull
git status --porcelain                  # must print nothing
git fetch origin main
git merge origin/main                   # resolve conflicts, then commit
```

### 2. Pick the version and survey the changes

```bash
git describe --tags --abbrev=0 origin/main  # the previous release tag
git log v<prev>..HEAD --format=%s           # substitute the tag printed above
```

On an existing PR this list includes the PR's own commits, which belong in the
release. Decide the new `X.Y.Z` from that list (see the SemVer note above).

### 3. Bump the version

`main` carries a pre-release version (`X.Y.Z-dev`) between releases. Drop the
suffix: edit `Cargo.toml` `[package].version` to the new `X.Y.Z`, then sync the
lockfile so `Cargo.lock`'s `mind` entry matches (the release build uses
`--locked`, so a stale lock fails the release):

```bash
cargo build                             # rewrites Cargo.lock's mind version
```

`tests/changelog.rs` only requires a matching `## [X.Y.Z]` section once the
`-dev` suffix is gone, so this bump is what arms that check.

### 4. Update the changelog

Add a `## [X.Y.Z] - YYYY-MM-DD` section directly under the `## [Unreleased]`
heading in `CHANGELOG.md` (Keep a Changelog format), moving the unreleased
entries into it and leaving `[Unreleased]` empty. Group the changes under
`Added` / `Changed` / `Fixed` / `Removed`, describing user-facing behavior,
not the commit workflow. Update the link references at the bottom: point
`[Unreleased]` at `.../compare/vX.Y.Z...HEAD` and add
`[X.Y.Z]: .../compare/v<prev>...vX.Y.Z`. `tests/changelog.rs` enforces this
(every version section needs a matching ref, `[Unreleased]` must compare from
the newest version, and each ref chains to the next-older one). Keep the voice
plain and factual (see `~/.local/share/agents/voice/voice-profile.md`).

### 5. Verify, commit, and push

```bash
make ci                                 # green on the bumped tree
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release X.Y.Z"           # the real version
```

The commit subject is `release X.Y.Z` (the repo's convention). Then either
open the release PR:

```bash
git push -u origin YYMMDD.release-X.Y.Z
gh pr create --base main --title "release X.Y.Z" --body-file <notes>
```

with the changelog section's bullets as the body, or push the bump onto the
existing PR and note the release in its description:

```bash
git push
gh pr edit <N> --body-file <notes>      # the existing body plus a line: "Releases X.Y.Z on merge."
```

### 6. Stop at the PR

Preparing the PR is the whole job. Do not push to `main`, create a tag, or run
`cargo publish`. Merging the PR is the release; report the PR link and that
merging it releases `X.Y.Z`.

A bumped PR releases on any merge method, since `decide` reads the merged
`Cargo.toml`. Keep it current with `main` until it merges: if another PR lands
first, merge `main` in again and move that PR's changelog entries into the
`X.Y.Z` section. If a different release merges first with the same version,
re-bump this PR to the next version, or `decide` sees an existing release and
skips it.

### 7. After the merge: confirm the release workflow succeeded

When asked to confirm a release, check it rather than assuming:

```bash
gh run list --workflow=release.yml --limit 3
gh run watch <run id>                   # the id the list printed
```

A run can be waiting on the `release` environment's approval gate before
`publish`, which looks like nothing happening. The jobs:

1. `decide`: reads the version and skips a `-dev` or already-released one.
2. `ci`, `ci-macos`, `audit` (parallel): `make ci` on Linux and macOS and a
   RustSec scan of the merged commit, gating everything downstream.
3. `build` (matrix): builds `--release --locked` for `aarch64-apple-darwin`,
   `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`, and the musl legs
   `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` (static, used
   by `install.sh`), attesting build provenance for each tarball.
4. `release`: generates `SHA256SUMS` and runs `gh release create --target`
   with every tarball, which creates the `v<version>` tag.
5. `publish`: publishes to crates.io via trusted publishing (skipped when the
   version is already there), then commits the regenerated `Formula/mind.rb`
   and the next patch's `-dev` version to `main` as `github-actions[bot]`.

Then verify what was published:

```bash
gh release view v<version>              # five .tar.gz assets plus SHA256SUMS
cargo info mind-cli                     # crates.io has the new version
git pull                                # pick up the bot's formula + dev commit
```

## Notes

- The tag is created by the workflow's `GITHUB_TOKEN`, so the "Protected
  release" tag ruleset (`refs/tags/v*`) must list GitHub Actions as a bypass
  actor. Without it, the `release` job's tag creation is refused.
- A run that fails partway is re-run with `gh workflow run release.yml --ref
  main` (`workflow_dispatch`). Every publishing step is idempotent: an existing
  GitHub Release gets its assets re-uploaded, a version already on crates.io is
  not published again, and an up-to-date formula is not re-committed. Fix the
  cause on `main` first if the failure was in the tree; since `main` is then
  past the release commit, the re-run releases that newer commit.
- Known flake: `ci-macos` can fail the timing-based lock test
  `tui::action::tests::execute_lock_is_exclusive_not_shared` under a loaded
  runner. If that single test is the only failure, re-run the failed jobs.
  Investigate only if it fails deterministically across re-runs.
