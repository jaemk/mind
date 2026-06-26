# Commands

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

## Verbs

| command | does |
|---------|------|
| `mind meld [<repo>] [--link-only] [--yes] [--as <prefix>] [--root <dir>] [--follow-branch <branch> \| --pin-tag <tag> \| --pin-ref <commit>] [--install-hook <cmd>] [--dangerously-skip-install-hook-check]` | clone and register a source (default `.`), then prompt to install its items (`--link-only` registers only; `--yes` installs without prompting). Re-melding an already-melded source installs any missing items, else shows each item's install state and commit |
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
[Source layout](source-layout.md) and the
[examples/](https://github.com/jaemk/mind/tree/main/examples): `starter` for the
plain convention layout, `namespacing` for `{{ns:}}` reference tokens under a
prefix, and `policy` for an enterprise managed policy. The full behavioral spec
is at [spec/](https://github.com/jaemk/mind/tree/main/spec).

## probe

`mind probe` with no flags opens an interactive browser of melded sources and
items (search, install, remove, meld, unmeld, sync, upgrade) when stdout is a
terminal. `-n` / `--no-tui` or `--json`, or a piped or redirected stdout, prints
the listing instead.
