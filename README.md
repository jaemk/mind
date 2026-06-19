# mind

A manager for agent tooling (skills, agents, rules) that melds arbitrary git
repos and links installed items into your agent directories (default
`~/.claude`). Modeled on Homebrew.

## Install

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

The repo is not named `homebrew-mind`, so the tap needs its clone URL.

## Usage

| command | does |
|---------|------|
| `mind meld <repo> [--as <prefix>]` | clone and register a source |
| `mind unmeld <name> [--forget]` (alias `detach`) | drop a source (optionally its items) |
| `mind learn <item>` | install a skill/agent/rule (glob installs many) |
| `mind forget <item>` (alias `unlearn`) | remove an installed item (glob removes many) |
| `mind sync [--evolve]` | refresh every source (optionally upgrade after) |
| `mind evolve [--yes] [item]` | upgrade installed items |
| `mind recall [--sources] [item] [--kind K] [--source S] [--json]` | list installed items / sources / details |
| `mind probe [query] [--kind K] [--source S] [--json]` | search available items |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind completions <shell>` / `mind man` | shell completions / man page |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`) or via a `mind.toml`. See [spec/](spec/) for the
full behavioral spec, and [examples/namespacing/](examples/namespacing/) for a
worked example of `{{ns:}}` reference tokens.

## Agent directories

`learn` links items into every configured agent home (each is linked at its kind
subdir: `skills/`, `agents/`, `rules/`). The default is `~/.claude`. Configure
more in `~/.mind/config.toml`:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
```

The file is created with the default lobe (`~/.claude`) on first use.

or for one invocation, set `MIND_AGENT_HOMES` to a `:`-separated path list.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
