---
name: release
description: Cut a new `mind` release. Bump the version, update the changelog, verify CI, tag with `make release`, and confirm the GitHub release workflow builds and uploads the binaries.
---

# release

Cut a tagged release of `mind`. The version in `Cargo.toml` is the single source
of truth: `make release` tags `v<version>` and pushes it, and that tag push
triggers `.github/workflows/release.yml`, which builds the per-platform binaries,
publishes the GitHub Release with the tarballs, and regenerates the Homebrew
formula.

Releases are SemVer. Pick the bump from what landed since the last tag: a new
verb/flag or other backward-compatible feature is a minor bump; a bug-fix-only
batch is a patch; a breaking change to the CLI, config, on-disk layout, or
`mind.toml` schema is a major bump.

## Steps

### 1. Confirm a clean, green starting point

Work from `main` with everything merged and pushed.

```bash
git switch main && git pull
test -z "$(git status --porcelain)" || echo "tree is dirty; commit or stash first"
make ci    # fmt-check + clippy (-D warnings) + test; must pass
```

### 2. Pick the version and survey the changes

```bash
git describe --tags --abbrev=0          # the previous release tag
git log "$(git describe --tags --abbrev=0)..HEAD" --format='%s'
```

Decide the new `X.Y.Z` from that list (see the SemVer note above).

### 3. Bump the version

Edit `Cargo.toml` `[package].version` to the new `X.Y.Z`, then sync the lockfile
so `Cargo.lock`'s `mind` entry matches (the release build uses `--locked`, so a
stale lock fails CI):

```bash
cargo build        # rewrites Cargo.lock's mind version
```

### 4. Update the changelog

Add a `## [X.Y.Z] - YYYY-MM-DD` section at the top of `CHANGELOG.md` (Keep a
Changelog format), grouping the changes under `Added` / `Changed` / `Fixed` /
`Removed`. Describe user-facing behavior, not the commit workflow. Update the
link references at the bottom (`[X.Y.Z]: .../compare/v<prev>...vX.Y.Z`). Keep the
voice plain and factual (see `~/.local/share/agents/voice/voice-profile.md`).

### 5. Commit the release prep

```bash
make ci                                 # green on the bumped tree
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release 0.0.0"           # use the real version
git push origin main
```

The commit subject is `release X.Y.Z` (matches the repo's convention).

### 6. Tag and trigger the release workflow

`make release` requires a clean tree and an unused tag. It tags `v<Cargo.toml
version>` and pushes the tag:

```bash
make release                            # tags v<version> from Cargo.toml and pushes it
# override only if needed: make release VERSION=1.2.3   (or TAG=v1.2.3)
```

### 7. Watch the GitHub workflow succeed

The tag push starts `release.yml`. It must finish green before the release is
real. Watch it:

```bash
gh run watch "$(gh run list --workflow=release.yml --limit=1 --json databaseId -q '.[0].databaseId')"
# or: gh run list --workflow=release.yml --limit=3
```

The workflow has three jobs, in order:

1. `build` (matrix): builds `--release --locked` for `aarch64-apple-darwin`,
   `aarch64-unknown-linux-gnu`, and `x86_64-unknown-linux-gnu`, and uploads each
   `mind-<version>-<target>.tar.gz` as an artifact. A `--locked` failure here
   means step 3 was skipped; fix `Cargo.lock` and re-tag.
2. `release`: downloads the artifacts and creates the GitHub Release with all
   three tarballs (`fail_on_unmatched_files: true`, so a missing target fails the
   job).
3. `formula`: regenerates `Formula/mind.rb` from the tarball checksums via
   `resources/update-formula.sh` and commits it back to `main` as the
   `github-actions[bot]`.

### 8. Verify the published release

```bash
gh release view "v<version>"            # three .tar.gz assets attached
git pull                                # pick up the bot's formula commit
grep -n 'version\|sha256' Formula/mind.rb   # points at the new version
```

Optionally smoke-test the install path (`resources/install.sh`, or
`brew upgrade mind` from the tap).

## Notes

- The version lives only in `Cargo.toml`; `Makefile` and the workflow derive the
  tag from it. Do not hand-write the tag except via the `make release` override.
- If the workflow fails after the tag was pushed, delete the tag
  (`git push origin :refs/tags/v<version>` and `git tag -d v<version>`), fix the
  cause on `main`, and re-run from step 5. A partially-created GitHub Release may
  need to be deleted in the UI or with `gh release delete`.
- The release is outward-facing and hard to reverse (it publishes binaries and
  pushes a formula commit). Confirm steps 1-5 are correct before running
  `make release`.
