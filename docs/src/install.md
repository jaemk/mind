# Install

## Requirements

`mind` runs `git` to clone and sync sources, so `git` must be installed and on
your `PATH`. The methods below fetch the `mind` binary itself; they do not install
git. Without git, `meld` and `sync` fail with a clear "git executable not found"
error.

## Linux (install script)

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

Downloads the release binary for your platform (x86_64 / aarch64) and installs it
to `~/.local/bin`. Override the target dir with `MIND_INSTALL_DIR` or pin a version
with `MIND_VERSION`:

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh \
  | MIND_INSTALL_DIR=/usr/local/bin MIND_VERSION=0.2.0 sh
```

If `~/.local/bin` is not on your `PATH`, the script prints the line to add. The
same script also serves Apple Silicon macOS; Intel macOS should use the tap below.

## Homebrew (macOS / Linux)

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

The repo is not named `homebrew-mind`, so the tap needs its clone URL.

## Updating

`mind evolve` updates the binary itself to the latest release. It reports the
target version and prompts before downloading, unless `--yes` is given (`--check`
reports without changing anything, `--version <v>` pins a target). It uses the
same download path as the install script.
