# Install

## Requirements

`mind` runs `git` to clone and sync sources, so `git` must be installed and on
your `PATH`. The methods below fetch the `mind` binary itself; they do not install
git. Without git, `meld` and `sync` fail with a clear "git executable not found"
error.

## Install script (Linux and macOS)

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

Downloads the release binary for your platform (x86_64 or aarch64 Linux or
macOS) and installs it to `~/.local/bin`. On Linux it fetches the
`musl` build, which is statically linked and runs on any glibc version (Ubuntu
22.04, Debian 12, RHEL 9, Amazon Linux 2, and newer). The script verifies the
download against the published `SHA256SUMS` asset before extracting. Override
the target dir with `MIND_INSTALL_DIR` or pin a version with `MIND_VERSION`:

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh \
  | MIND_INSTALL_DIR=/usr/local/bin MIND_VERSION=0.12.0 sh
```

If `~/.local/bin` is not on your `PATH`, the script prints the line to add.

When `gh` is installed, the script also checks the archive's build provenance
(`gh attestation verify`) and reports the result, without failing the install.
Set `MIND_REQUIRE_ATTESTATION=1` to make it mandatory: the install stops if `gh`
is missing or the check fails.

## Homebrew (macOS and Linux)

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

The repo is not named `homebrew-mind`, so the tap needs its clone URL. Homebrew
bottles are provided for macOS (arm64 and x86_64) and Linux (x86_64 and
aarch64, glibc-linked).

## cargo install (Linux and macOS)

```
cargo install --locked mind-cli
```

Builds from source using the Rust toolchain. This is the path for any Linux or
macOS host not covered by the install script or Homebrew tap. Requires Rust 1.98 or later (`rustup` is the standard way to
install it).

Use `--locked`. Without it `cargo install` re-resolves dependencies instead of
using the published `Cargo.lock`, and that lockfile is the one tested against
Rust 1.98 in CI. A fresh resolve can pick a newer dependency needing a newer
compiler, which fails the build with a rustc-version error naming a crate you
did not ask for.

The supported platforms are Linux and macOS; the binary does not currently build
on Windows. On Windows, run `mind` under WSL (Windows Subsystem for Linux).

## Updating

`mind evolve` updates the binary itself to the latest release. It reports the
target version and the resolved target triple (the exact artifact it would
fetch) and prompts before downloading, unless `--yes` is given (`--check`
reports without changing anything, `--to <v>` pins a target). It downloads the
same release artifacts the install script uses and verifies the `SHA256SUMS`
asset before swapping in the new binary. Unlike the install script, it makes the
request itself rather than shelling out to `curl` or `wget`, so neither needs to
be on `PATH`, and it verifies TLS against the machine's certificate store.

`evolve` also verifies the archive's signature against the release key built
into `mind`, with no external tool, and aborts the swap, leaving the existing
binary in place, when the signature is missing or does not verify. Releases are
also attested for `gh attestation verify`, which `install.sh` runs when `gh` is
on `PATH`.

A Homebrew install is updated with `brew upgrade mind`. `evolve` refuses to
replace a binary that resolves into a Homebrew `Cellar`, and `evolve --check`
points at `brew upgrade mind` when an update is available.

## Uninstall

Remove installed items and sources first, then the binary.

- `mind forget <item>` removes one installed item: its store copy
  (`~/.mind/store/<kind>/<name>`) and its symlink in every lobe. `mind forget
  --unmanaged` also removes lobe items mind did not install (with no `<item>`,
  every unmanaged item across all lobes).
- `mind unmeld <name>` drops a melded source and uninstalls the items it
  installed; `--keep-items` drops the source without touching installed items.
- `rm -rf ~/.mind` (or the `MIND_HOME` override) removes the store, source
  clones, manifest, registry, and config -- everything mind tracks on disk.
  `forget`/`unmeld` the items you want cleaned up first: deleting `~/.mind`
  leaves lobe symlinks dangling, and `mind introspect` on a lobe with no
  `~/.mind` reports every link as broken.
- Remove the `mind` binary itself:
  - Install script: delete it from `~/.local/bin/mind`, or from
    `$MIND_INSTALL_DIR/mind` if you overrode the target directory.
  - Homebrew: `brew uninstall mind`.
  - `cargo install`: `cargo uninstall mind-cli`.
