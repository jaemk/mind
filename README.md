# mind

A manager for agent tooling (skills, agents, rules, tools) that melds arbitrary git
repos and links installed items into your agent directories (default
`~/.claude`). Modeled on Homebrew.

Documentation: https://jaemk.github.io/mind/

## Install

### Linux (install script)

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
  agents, rules, and tools, found by convention (`skills/<n>/SKILL.md`,
  `agents/<n>.md`, `rules/<n>.md`, `tools/<n>/`) or declared in a `mind.toml`. A
  tool is store-only helper tooling that other items reference, not linked into a
  lobe.
- `mind learn <item>` copies the item into the *store* (`~/.mind/store`) and
  symlinks it into each *lobe* (agent home, default `~/.claude`). `forget`
  reverses it.
- `sync` refreshes every source's clone; `upgrade` upgrades installed items to the
  refreshed version, reporting hash and commit deltas before changing anything.
  `evolve` updates the mind binary itself.
- `recall` and `probe` inspect what is installed and what is available;
  `introspect` reports drift and broken links.

## Usage

| command | does |
|---------|------|
| `mind meld [<repo>] [--link-only] [--yes] [--as <prefix>] [--root <dir>] [--follow-branch <branch> | --pin-tag <tag> | --pin-ref <commit>] [--install-hook <cmd>] [--dangerously-skip-install-hook-check]` | clone and register a source (default `.`), then prompt to install its items (`--link-only` registers only; `--yes` installs without prompting). Re-melding an already-melded source installs any missing items, else shows each item's install state and commit |
| `mind init-source [<path>] [--template]` | scaffold `mind.toml` + report references; `--template` rewrites bare refs as `{{ns:}}` (maintainer) |
| `mind unmeld <name> [--unlink-only] [--yes] [--uninstall-hook <cmd>] [--dangerously-skip-install-hook-check]` (alias `detach`) | drop a source and forget its items (`--unlink-only` keeps them) |
| `mind learn [--yes] [--force] <item>` | install a skill/agent/rule/tool (glob installs many); a partial selection also pulls in the source siblings it references. `--force` overwrites a conflicting non-mind link target (without it, a conflict prompts on a TTY) |
| `mind forget [--yes] <item>` (alias `unlearn`) | remove an installed item (glob removes many; a multi-match glob confirms first, `--yes` skips) |
| `mind sync [--upgrade] [--dangerously-skip-install-hook-check]` | refresh every source (optionally upgrade after; flag allows unattended hook re-runs) |
| `mind upgrade [--yes] [--dangerously-skip-install-hook-check] [item]` | upgrade installed items to their latest source version (re-runs install hooks on sources that advance) |
| `mind evolve [--check] [--yes] [--version <v>]` | update the mind binary itself to the latest release (or --version) |
| `mind recall [item] [--sources] [--kind K] [--source S] [--json]` | status: each source with its items, marked installed or available; `--sources` narrows to sources; `<item>` shows one item's details |
| `mind probe [query] [--kind K] [--source S] [--json] [--no-tui]` | browse and search items (interactive TUI on a terminal) |
| `mind review <target> [--as <prefix>]` / `mind review --policy <path>` | validate a source for publishing, or validate a managed policy file (read-only) |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind completions <shell>` / `mind man` | shell completions / man page |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`, `tools/<n>/`) or via a `mind.toml`. See
[examples/starter/](examples/starter/) for the plain convention layout,
[examples/namespacing/](examples/namespacing/) for `{{ns:}}` reference tokens
under a prefix, [examples/policy/](examples/policy/) for an enterprise managed
policy, and [spec/](spec/) for the full behavioral spec.

`mind probe` with no flags opens an interactive browser of melded sources and
items (search, install, remove, meld, unmeld, sync, upgrade) when stdout is a terminal. `--no-tui`
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

A source can declare install hooks in `mind.toml`: shell commands that build or install the
tooling its items rely on. A user can supply or override them with `meld --install-hook <cmd>`.

The full form is a `[[hooks]]` array-of-tables. Each entry has a required `run` field (the shell
command), an optional `name` label shown in the disclosure, an `optional` bool (default `false`),
and an `event` field (`"install"` for `meld`, `"uninstall"` for `unmeld`; default `"install"`).
The legacy `[source].install` string is shorthand for one required install hook:

```toml
# mind.toml in a source repo

# shorthand (one required install hook)
[source]
install = "make build"

# full form: multiple hooks, mixed events and optionality
[[hooks]]
run = "make build"
name = "build tooling"
event = "install"

[[hooks]]
run = "pip install -r requirements.txt"
name = "python deps"
optional = true
event = "install"

[[hooks]]
run = "make clean"
name = "cleanup"
event = "uninstall"
```

Because a hook is arbitrary code, `mind` discloses the source identity, pin, commit, clone path,
and exact command before running anything, and prompts `[Y/n/a]` with three choices: run the hook
(the default, a bare Enter), skip it but still install the source and its items, or abort and
install nothing. In a non-TTY context (CI, scripts) the hook is skipped and a note is printed;
`--dangerously-skip-install-hook-check` runs it unattended. Overriding a source's declared
`[source].install` with `--install-hook` is announced in the prompt, which shows both the declared
and the overriding command.

A skipped hook is recorded and re-offered by `mind upgrade`, so you can run it later without the
source needing to advance first. On an `upgrade` re-run the source is already installed, so abort
is treated as skip. `upgrade` also re-runs the hook when a source advances to a new commit.
`sync --upgrade` accepts `--dangerously-skip-install-hook-check` so a CI pipeline can run hook
re-runs unattended. Without the flag, a non-TTY `sync --upgrade` skips hook re-runs (the same as
`upgrade`).

Uninstall hooks (`event = "uninstall"`) run at `unmeld`, in the source's clone, before the clone
is removed. They use the same safety-prompt model as install hooks: required hooks prompt
run / skip / abort-the-unmeld; optional hooks prompt run / skip; a non-TTY `unmeld` skips them
and notes it. `unmeld --uninstall-hook <cmd>` supplies or overrides the source's declared
uninstall hooks. `unmeld --dangerously-skip-install-hook-check` runs them unattended (the flag
name is reused deliberately). A required uninstall hook that fails or is aborted leaves the
source melded.

`recall --sources` marks a source that carries hooks with a count-aware token in its status
bracket (e.g. `1 hook` or `3 hooks`). `mind review <repo>` lists every declared hook (install
and uninstall), showing each hook's command, event, and whether it is required or optional. See
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
- To authenticate with an SSH key instead of an https username/password, meld
  the `git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so
  the `owner/repo` shorthand clones over SSH. An https remote still prompts (or
  uses a credential helper) as git normally does.
- Stuck in the TUI. Press `q` in the main view, or Ctrl-C twice from anywhere
  (search box, dialogs) to force exit.

## Develop

```
cargo test
```

Releases are tag-driven: pushing `v*` builds per-platform binaries, creates the
GitHub Release, and regenerates `Formula/mind.rb`.
