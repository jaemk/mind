# mind

[![docs](https://img.shields.io/badge/docs-jaemk.github.io%2Fmind-blue)](https://jaemk.github.io/mind/)
[![release](https://img.shields.io/github/v/release/jaemk/mind)](https://github.com/jaemk/mind/releases/latest)
[![crates.io](https://img.shields.io/crates/v/mind-cli)](https://crates.io/crates/mind-cli)

A manager for agent tooling (skills, agents, rules, tools) that melds arbitrary git
repos and links installed items into your agent directories (default
`~/.claude`). Modeled on Homebrew.

Full documentation: https://jaemk.github.io/mind/

## Install

`mind` shells out to `git` to clone and sync sources, so `git` must be on your
`PATH`. The methods below install the `mind` binary itself, not git.

Linux and Apple Silicon macOS (install script):

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

The script verifies the download against the published `SHA256SUMS` asset before
extracting.

Homebrew (Apple Silicon macOS and Linux):

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

Bottles are provided for Apple Silicon macOS (arm64) and x86_64 Linux. Intel
macOS is not covered; use `cargo install mind-cli` instead.

Any platform (requires the Rust toolchain):

```
cargo install mind-cli
```

See the [install guide](https://jaemk.github.io/mind/guide/install.html) for
version pinning, target-dir overrides, and platform details.

## Quickstart

```
mind meld owner/repo   # clone and prompt to install items
mind recall            # list what's installed
```

`meld` presents available items and prompts to install. To register without
installing and choose items individually:

```
mind meld owner/repo --register-only   # register only, skip install prompt
mind probe                              # browse available items (interactive)
mind learn <item>                       # install a specific item
```

Agent homes can be Claude Code, Gemini CLI, Codex CLI, or Antigravity -- not just
`~/.claude`. See
[configuration](https://jaemk.github.io/mind/guide/configuration.html#cross-harness-lobes)
for details.

[![asciicast](https://asciinema.org/a/qcAxP5PD7H6cuLTE.svg)](https://asciinema.org/a/qcAxP5PD7H6cuLTE)

The [documentation](https://jaemk.github.io/mind/) is the full reference: the
[command reference](https://jaemk.github.io/mind/guide/commands.html),
[configuration](https://jaemk.github.io/mind/guide/configuration.html),
[install hooks](https://jaemk.github.io/mind/guide/install-hooks.html),
[authoring a source](https://jaemk.github.io/mind/guide/authoring.html), and
[troubleshooting](https://jaemk.github.io/mind/guide/troubleshooting.html). The
[spec/](spec/) directory is the normative behavioral spec.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
