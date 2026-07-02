# mind

[![docs](https://img.shields.io/badge/docs-jaemk.github.io%2Fmind-blue)](https://jaemk.github.io/mind/)
[![release](https://img.shields.io/github/v/release/jaemk/mind)](https://github.com/jaemk/mind/releases/latest)

A manager for agent tooling (skills, agents, rules, tools) that melds arbitrary git
repos and links installed items into your agent directories (default
`~/.claude`). Modeled on Homebrew.

Full documentation: https://jaemk.github.io/mind/

## Install

`mind` shells out to `git` to clone and sync sources, so `git` must be on your
`PATH`. The methods below install the `mind` binary itself, not git.

Linux and Apple Silicon macOS:

```
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

Homebrew (macOS / Linux):

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

See the [install guide](https://jaemk.github.io/mind/install.html) for version
pinning, target-dir overrides, and Intel macOS.

## Quickstart

```
mind meld owner/repo        # clone and register a source repo
mind probe                  # browse and search available items (interactive)
mind learn <item>           # install one into each agent home
mind recall                 # list what's installed
```

Agent homes can be Claude Code, Gemini CLI, Codex CLI, or Antigravity -- not just
`~/.claude`. See
[configuration](https://jaemk.github.io/mind/configuration.html#cross-harness-lobes)
for details.

[![asciicast](https://asciinema.org/a/qcAxP5PD7H6cuLTE.svg)](https://asciinema.org/a/qcAxP5PD7H6cuLTE)

The [documentation](https://jaemk.github.io/mind/) is the full reference: the
[command reference](https://jaemk.github.io/mind/commands.html),
[configuration](https://jaemk.github.io/mind/configuration.html),
[install hooks](https://jaemk.github.io/mind/install-hooks.html),
[authoring a source](https://jaemk.github.io/mind/authoring.html), and
[troubleshooting](https://jaemk.github.io/mind/troubleshooting.html). The
[spec/](spec/) directory is the normative behavioral spec.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
