# mind

A manager for agent tooling (skills, agents, rules) that melds arbitrary git
repos and links installed items into `~/.claude`. Modeled on Homebrew.

## Install

```
brew tap jaemk/mind
brew install mind
```

## Usage

| command | does |
|---------|------|
| `mind meld <repo> [--as <prefix>]` | clone and register a source (like `brew tap`) |
| `mind unmeld <name>` | drop a source |
| `mind learn <item>` | install a skill/agent/rule |
| `mind forget <item>` | remove an installed item |
| `mind sync` | refresh every source |
| `mind evolve [--yes] [item]` | upgrade installed items |
| `mind recall [--sources] [item]` | list installed items / sources / details |
| `mind probe [query]` | search available items |
| `mind introspect` | report drift and broken links |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`) or via a `mind.toml`. See [spec/](spec/) for the
full behavioral spec.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
