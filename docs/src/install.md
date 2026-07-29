# Install

## Requirements

`mind` runs `git` to clone and sync sources, so `git` must be installed and on
your `PATH`. The methods below fetch the `mind` binary itself; they do not install
git. Without git, `meld` and `sync` fail with a clear "git executable not found"
error.

## Install script (Linux and Apple Silicon macOS)

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

Downloads the release binary for your platform (x86_64 or aarch64 Linux, or
aarch64 macOS) and installs it to `~/.local/bin`. On Linux it fetches the
`musl` build, which is statically linked and runs on any glibc version (Ubuntu
22.04, Debian 12, RHEL 9, Amazon Linux 2, and newer). The script verifies the
download against the published `SHA256SUMS` asset before extracting. Override
the target dir with `MIND_INSTALL_DIR` or pin a version with `MIND_VERSION`:

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh \
  | MIND_INSTALL_DIR=/usr/local/bin MIND_VERSION=0.12.0 sh
```

If `~/.local/bin` is not on your `PATH`, the script prints the line to add.

## Homebrew (Apple Silicon macOS and Linux)

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

The repo is not named `homebrew-mind`, so the tap needs its clone URL. Homebrew
bottles are provided for Apple Silicon macOS (arm64) and Linux (x86_64 and
aarch64, glibc-linked). Intel macOS is not covered by the tap; use
`cargo install mind-cli` instead (see below).

> **Note (migration):** Earlier versions of this page said Intel macOS should use
> the tap. That instruction was wrong: no Intel macOS bottle exists. Use
> `cargo install mind-cli` on Intel macOS.

## cargo install (Linux and macOS)

```
cargo install mind-cli
```

Builds from source using the Rust toolchain. This is the recommended path for
Intel macOS and any other Linux or macOS host not covered by the install script
or Homebrew tap. Requires Rust 1.88 or later (`rustup` is the standard way to
install it).

The supported platforms are Linux and macOS; the binary does not currently build
on Windows. On Windows, run `mind` under WSL (Windows Subsystem for Linux).

## Updating

`mind evolve` updates the binary itself to the latest release. It reports the
target version and the resolved target triple (the exact artifact it would
fetch) and prompts before downloading, unless `--yes` is given (`--check`
reports without changing anything, `--version <v>` pins a target). It uses the
same download path as the install script and verifies the `SHA256SUMS` asset
before swapping in the new binary.

If a `gh` binary is on `PATH`, `evolve` also verifies the downloaded archive's
build-provenance attestation (`gh attestation verify`) before swapping. Without
`gh`, this step is skipped and `evolve` proceeds as it always has. Unlike
`install.sh` (which never blocks on this check), `evolve` aborts the swap,
leaving the existing binary in place, when `gh` runs the check and reports the
artifact does not verify; it still proceeds, with a note, when `gh` itself
could not complete the check (no network, or a `gh` build without the
`attestation` subcommand).

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
