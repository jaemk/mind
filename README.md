# mind

A manager for agent tooling (skills, agents, rules) that melds arbitrary git
repos and links installed items into your agent directories (default
`~/.claude`). Modeled on Homebrew.

## Install

### Linux (install script)

```
curl -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
```

Downloads the release binary for your platform (x86_64 / aarch64) and installs it
to `~/.local/bin`. Override the target dir with `MIND_INSTALL_DIR` or pin a version
with `MIND_VERSION`:

```
curl -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh \
  | MIND_INSTALL_DIR=/usr/local/bin MIND_VERSION=0.2.0 sh
```

If `~/.local/bin` is not on your `PATH`, the script prints the line to add. The
same script also serves Apple Silicon macOS; Intel macOS should use the tap below.

### Homebrew (macOS / Linux)

```
brew tap jaemk/mind https://github.com/jaemk/mind
brew trust jaemk/mind
brew install mind
```

The repo is not named `homebrew-mind`, so the tap needs its clone URL.

## Quickstart

Meld a source, install an item, and see it linked into `~/.claude`:

```
mind meld owner/repo        # clone and register a source repo
mind probe                  # browse and search available items (interactive)
mind learn <item>           # install one into each agent home
mind recall                 # list what's installed
```

For a self-contained first run with no remote, use the bundled starter source (a
plain convention layout, see [examples/starter/](examples/starter/)):

```
cp -r examples/starter /tmp/starter
cd /tmp/starter && git init -q && git add -A && git commit -qm init
mind meld /tmp/starter
mind learn greet
```

## Mental model

- A *source* is a melded git repo (`mind meld`). It offers *items*: skills,
  agents, and rules, found by convention (`skills/<n>/SKILL.md`, `agents/<n>.md`,
  `rules/<n>.md`) or declared in a `mind.toml`.
- `mind learn <item>` copies the item into the *store* (`~/.mind/store`) and
  symlinks it into each *lobe* (agent home, default `~/.claude`). `forget`
  reverses it.
- `sync` refreshes every source's clone; `evolve` upgrades installed items to the
  refreshed version, reporting hash and commit deltas before changing anything.
- `recall` and `probe` inspect what is installed and what is available;
  `introspect` reports drift and broken links.

## Usage

| command | does |
|---------|------|
| `mind meld <repo> [--as <prefix>] [--root <dir>] [--follow-branch <branch> | --pin-tag <tag> | --pin-ref <commit>]` | clone and register a source (optionally namespaced, subtree-scoped, version-pinned) |
| `mind unmeld <name> [--forget]` (alias `detach`) | drop a source (optionally its items) |
| `mind learn <item>` | install a skill/agent/rule (glob installs many) |
| `mind forget <item>` (alias `unlearn`) | remove an installed item (glob removes many) |
| `mind sync [--evolve]` | refresh every source (optionally upgrade after) |
| `mind evolve [--yes] [item]` | upgrade installed items |
| `mind recall [--sources] [item] [--kind K] [--source S] [--json]` | list installed items / sources / details |
| `mind probe [query] [--kind K] [--source S] [--json] [--no-tui]` | browse and search items (interactive TUI on a terminal) |
| `mind review <target> [--as <prefix>]` | validate a source for publishing (read-only) |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind completions <shell>` / `mind man` | shell completions / man page |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`) or via a `mind.toml`. See
[examples/starter/](examples/starter/) for the plain convention layout,
[examples/namespacing/](examples/namespacing/) for `{{ns:}}` reference tokens
under a prefix, and [spec/](spec/) for the full behavioral spec.

`mind probe` with no flags opens an interactive browser of melded sources and
items (search, install, remove, meld, unmeld, sync, evolve) when stdout is a terminal. `--no-tui`
or `--json`, or a piped/redirected stdout, prints the listing instead.

## Agent directories

`learn` links items into every configured agent home (each is linked at its kind
subdir: `skills/`, `agents/`, `rules/`). The default is `~/.claude`. Configure
more in `~/.mind/config.toml`:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
```

The file is created with the default lobe (`~/.claude`) on first use.

or for one invocation, set `MIND_AGENT_HOMES` to a `:`-separated path list.

## Troubleshooting

- An item didn't show up in `~/.claude`. Run `mind introspect`; it reports
  missing links and drift, and `mind introspect --fix` recreates missing
  symlinks.
- `learn` refused to overwrite a path. mind will not clobber a file or link it
  did not create (the clobber guard). Move the existing one aside, then `learn`
  again.
- Two sources ship an item with the same name. Namespace one with `mind meld
  <repo> --as <prefix>`, so its items install as `<prefix>-<name>`. See
  [examples/namespacing/](examples/namespacing/).
- Where things live: sources clone under `~/.mind`, installed copies in
  `~/.mind/store`, the registry in `~/.mind/sources.json`, config in
  `~/.mind/config.toml`. Override the roots with `MIND_HOME` and `CLAUDE_HOME`.
- Before publishing a source, run `mind review <path>` to check its `mind.toml`,
  item kinds, `{{ns:}}` references, and pin directive.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
