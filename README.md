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
| `mind meld <repo> [--as <prefix>] [--root <dir>] [--follow-branch <branch> | --pin-tag <tag> | --pin-ref <commit>] [--install-hook <cmd>] [--dangerously-skip-install-hook-check]` | clone and register a source (optionally namespaced, subtree-scoped, version-pinned, with an install hook) |
| `mind unmeld <name> [--forget]` (alias `detach`) | drop a source (optionally its items) |
| `mind learn [--yes] <item>` | install a skill/agent/rule (glob installs many); a partial selection also pulls in the source siblings it references |
| `mind forget <item>` (alias `unlearn`) | remove an installed item (glob removes many) |
| `mind sync [--evolve]` | refresh every source (optionally upgrade after) |
| `mind evolve [--yes] [--dangerously-skip-install-hook-check] [item]` | upgrade installed items (re-runs install hooks on sources that advance) |
| `mind recall [--sources] [item] [--kind K] [--source S] [--json]` | list installed items / sources / details |
| `mind probe [query] [--kind K] [--source S] [--json] [--no-tui]` | browse and search items (interactive TUI on a terminal) |
| `mind review <target> [--as <prefix>]` / `mind review --policy <path>` | validate a source for publishing, or validate a managed policy file (read-only) |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind completions <shell>` / `mind man` | shell completions / man page |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`) or via a `mind.toml`. See
[examples/starter/](examples/starter/) for the plain convention layout,
[examples/namespacing/](examples/namespacing/) for `{{ns:}}` reference tokens
under a prefix, [examples/policy/](examples/policy/) for an enterprise managed
policy, and [spec/](spec/) for the full behavioral spec.

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

## Install hooks

A source can declare an install hook in `mind.toml`: a shell command that builds or installs
the tooling its items rely on. A user can supply or override it with `meld --install-hook <cmd>`.

```toml
# mind.toml in a source repo
[source]
install = "make build"
```

Because a hook is arbitrary code, `mind` discloses the source identity, pin, commit, clone path,
and exact command before running anything, and prompts with three choices: run it, skip it but
still install the source (the default), or abort and install nothing. In a non-TTY context (CI,
scripts) the hook is skipped and a note is printed; `--dangerously-skip-install-hook-check` runs
it unattended. `evolve` re-runs the hook when a source advances to a new commit. `recall
--sources` marks sources that carry a hook with a ` hook` token in their bracketed column.
`mind review <repo>` shows a source's declared hook before you meld it. See
[spec/install-hooks.md](spec/install-hooks.md) for the full behavior.

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
