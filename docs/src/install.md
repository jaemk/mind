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

Downloads the release binary for your platform (x86_64 Linux or aarch64 macOS)
and installs it to `~/.local/bin`. The script verifies the download against the
published `SHA256SUMS` asset before extracting. Override the target dir with
`MIND_INSTALL_DIR` or pin a version with `MIND_VERSION`:

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
bottles are provided for Apple Silicon macOS (arm64) and x86_64 Linux. Intel
macOS is not covered by the tap; use `cargo install mind-cli` instead (see below).

> **Note (migration):** Earlier versions of this page said Intel macOS should use
> the tap. That instruction was wrong: no Intel macOS bottle exists. Use
> `cargo install mind-cli` on Intel macOS.

## cargo install (Linux and macOS)

```
cargo install mind-cli
```

Builds from source using the Rust toolchain. This is the recommended path for
Intel macOS and any other Linux or macOS host not covered by the install script
or Homebrew tap. Requires Rust 1.85 or later (`rustup` is the standard way to
install it).

The supported platforms are Linux and macOS; the binary does not currently build
on Windows. On Windows, run `mind` under WSL (Windows Subsystem for Linux).

## Updating

`mind evolve` updates the binary itself to the latest release. It reports the
target version and prompts before downloading, unless `--yes` is given (`--check`
reports without changing anything, `--version <v>` pins a target). It uses the
same download path as the install script and verifies the `SHA256SUMS` asset
before swapping in the new binary.
