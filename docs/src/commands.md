# Commands

## Mental model

`mind` sits between *sources* (git repos full of agent tooling) and *lobes*
(your agent homes, like `~/.claude`). Two on-disk areas sit in between: a clone
of each source, and a copy of each item you install. The four terms below name
every moving part.

**Source.** A melded git repo. `mind meld <repo>` clones it into
`~/.mind/sources/<host>/<owner>/<repo>` and records it in `sources.json`. That
clone is a staging area: melding alone links nothing into an agent home, it just
makes the source's items available to install. `sync` (or re-melding) refreshes
the clone.

**Item.** A unit a source offers, of one of four *kinds*:

- `skill` - a `skills/<name>/SKILL.md` directory.
- `agent` - an `agents/<name>.md` file.
- `rule` - a `rules/<name>.md` file.
- `tool` - a `tools/<name>/` directory of helper code. A tool is *store-only*:
  it is never linked into a lobe and the harness never loads it directly. Other
  items reference it by path. Tools exist so a skill or agent can ship supporting
  scripts (a linter, a fetch helper) without the harness mistaking that code for
  a skill of its own.

Items are discovered by convention (the paths above) or declared in a
`mind.toml`.

**Lobe.** An agent home `mind` links items into: the directory holding
`skills/`, `agents/`, and `rules/`. The default lobe is `~/.claude`; you can add
Gemini, Codex, Antigravity, or any directory, each with an optional per-kind
filter (see [Configuration](configuration.md)).

**Learn.** `mind learn <item>` copies the item out of the source clone into the
*store* (`~/.mind/store/<kind>/<name>`) and symlinks that store copy into every
lobe. The store copy is the stable thing your agent homes point at, so a later
`sync` cannot change an installed item under you until you choose to `upgrade`.
`forget` reverses it.

### What each step puts on disk

`mind meld jaemk/agents` clones the source. Nothing is linked yet:

```text
$ mind meld jaemk/agents

~/.mind/
  sources.json                              # registry: `agents` is now melded
  sources/
    github.com/jaemk/agents/                # the clone (a staging area)
      skills/review/SKILL.md
      agents/test.md
      tools/lint/run.sh
  store/                                    # empty - nothing learned yet
```

`mind learn jaemk/agents#skill:review` copies that one item into the store and
symlinks it into the lobe:

```text
$ mind learn jaemk/agents#skill:review

~/.mind/
  manifest.json                             # registry: `review` is now installed
  store/
    skill/review/SKILL.md                   # the copy, taken from the clone
  sources/
    github.com/jaemk/agents/ ...            # clone untouched

~/.claude/                                  # a lobe (agent home)
  skills/
    review -> ~/.mind/store/skill/review    # symlink the harness discovers
```

A `tool` the skill referenced would land in `~/.mind/store/tool/<name>` with no
symlink under `~/.claude`, present for the skill to call but invisible to the
harness.

**Stay current.** `sync` refreshes every source clone; `upgrade` moves installed
items to the refreshed version, reporting hash and commit deltas before changing
anything; `evolve` updates the `mind` binary itself.

**Inspect.** `recall` and `probe` show what is installed and what is available;
`introspect` reports drift and broken links.

## Verbs

| command | does |
|---------|------|
| `mind meld [<repo>] [--link-only] [--yes] [-f\|--force] [-r\|--recursive] [--as <prefix>] [--root <dir>] [--follow-branch <branch> \| --pin-tag <tag> \| --pin-ref <commit>] [--install-hook <cmd>] [--dangerously-skip-install-hook-check]` | clone and register a source (default `.`), then prompt to install its items (`--link-only` registers only; `--yes` installs without prompting; `-f`/`--force` overwrites conflicting non-mind link targets; `-r`/`--recursive` offers to install items from every nested source a super-source curates). Re-melding an already-melded source installs any missing items, else shows each item's install state and commit |
| `mind init-source [<path>] [--template]` | scaffold `mind.toml` + report references; `--template` rewrites bare refs as `{{ns:}}` (maintainer) |
| `mind unmeld <name> [--unlink-only] [--yes] [--uninstall-hook <cmd>] [--dangerously-skip-install-hook-check]` (alias `detach`) | drop a source and forget its items (`--unlink-only` keeps them) |
| `mind learn [--yes] [-f\|--force] [-n\|--dry-run] [--all] <item>` | install a skill/agent/rule/tool (glob installs many); a partial selection also pulls in the source siblings it references. `--force` overwrites a conflicting non-mind link target (without it, a conflict prompts on a TTY); `--all` installs every item of the named source (shorthand for `<source>#*`); `-n`/`--dry-run` previews the dependency closure without installing anything |
| `mind forget [--yes] [-f\|--force] [--unmanaged] [--dangerously-skip-install-hook-check] [<item>]` (alias `unlearn`) | remove an installed item (glob removes many; a multi-match glob confirms first, `--yes` skips). `--unmanaged` scopes removal to unmanaged lobe items only; with no `<item>`, removes every unmanaged item across all lobes. `-f`/`--force` skips the dependents confirmation when the item being removed has dependents. `--dangerously-skip-install-hook-check` runs uninstall hooks without the safety prompt |
| `mind sync [--upgrade] [--dangerously-skip-install-hook-check]` | refresh every source (optionally upgrade after; flag allows unattended hook re-runs) |
| `mind upgrade [--yes] [--dangerously-skip-install-hook-check] [item]` | upgrade installed items to their latest source version (re-runs install hooks on sources that advance) |
| `mind evolve [--check] [--yes] [--version <v>]` | update the mind binary itself to the latest release (or --version) |
| `mind recall [item] [--sources] [--kind K] [--source S] [--tree] [--json]` (alias `status`) | status: each source with its items, marked installed or available; `--sources` narrows to sources; `<item>` shows one item's details; `--tree` renders installed items as a dependency forest (with an item ref, scopes to that item's subtree) |
| `mind probe [query] [--kind K] [--source S] [--json] [--no-tui]` | browse and search items (interactive TUI on a terminal) |
| `mind review <target> [--as <prefix>]` / `mind review --policy <path>` | validate a source for publishing, or validate a managed policy file (read-only) |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind config show` / `mind config lobes add [<path> \| --preset <name>]\|list\|remove <path>\|detect [--yes] [--json]` (alias `config target`) | view config and manage agent homes (lobes). `add --preset <name>` adds a preset lobe with a preconfigured path and kinds filter (presets: `gemini`, `codex`, `antigravity`, `antigravity-cli`, `universal`). `detect` reports which known harness homes exist on the machine and offers to add their presets; adds only with `--yes` or an interactive TTY confirm; `--json` emits detection results as structured JSON. `config lobes list` and `config show` include the kinds filter for each lobe, e.g. `~/.gemini [skill, agent]`. See [Configuration](configuration.md) for the preset table and per-harness path details. |
| `mind dump [--output <path>] [--whole-sources]` | write a super-source `mind.toml` reproducing the current melded and installed state (to stdout or `--output <path>`); each source is pinned to its recorded commit; item-filtered by default (`--whole-sources` emits `install = true` for every source regardless of install count) |
| `mind absorb <ref> [--to <path>] [-f\|--force]` | claim a single unmanaged lobe item into a version-controlled source and install it as a managed item; `--to` sets the destination (see [absorb](absorb.md) for full destination precedence); `--force` overwrites a `kind:name` collision at the destination |
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

## Selecting items (globs)

`learn`, `forget`, `unmeld`, `probe`, and `recall` all accept a glob in place of
an exact item ref. The kind prefix, source qualifier, and glob compose:

| pattern | selects |
|---------|---------|
| `'*'` | every item across all sources |
| `'skill:*'` | all skills |
| `'owner/repo#*'` | all items of one source |
| `'review*'` | items whose name starts with `review` |

The glob is matched against the effective (installed) name. A glob matching
nothing is `ItemNotFound` (for items) or `SourceNotFound` (for sources).

Shell-quoting caveat: quote the glob so the shell does not expand it before
`mind` sees it:

```
mind learn 'skill:*'
mind forget 'owner/repo#*'
```

Spec: CLI-31, CLI-41.

## Filtering with --kind and --source

`recall` and `probe` accept two composable filters:

- `--kind <skill|agent|rule|tool>` narrows to one item kind.
- `--source <selector>` narrows to items from a matching source. The selector is
  an exact name, an unambiguous trailing suffix (`repo` or `owner/repo`), or a
  glob matched against the full `host/owner/repo` identity (so `*` spans `/`):

```
mind recall --kind skill
mind probe --source '*agents'
mind recall --source my-repo --kind rule
```

For `recall`, these filters apply to the installed-items listing only, not to
`--sources` or a single-item lookup. Spec: CLI-83, CLI-86.

## Global flags and output

`--json`, `--yes` (`-y`), and `--ascii` are global flags accepted before or after
any verb. Position does not matter: `mind --json recall` and `mind recall --json`
are equivalent (CLI-150).

**Color and Unicode.** Output uses ANSI color and Unicode glyphs when all of the
following hold: stdout is a TTY, the locale is UTF-8, `NO_COLOR` is unset, and
neither `--json` nor `--ascii` is in effect. Any one of those conditions being
false forces plain ASCII output with no color escapes. The ASCII fallback
substitutes visually equivalent characters (`+` installed, `!` warning, `x`
error, `-` available) so no information is lost (CLI-151, CLI-152, CLI-154).

`NO_COLOR` set to any value (including empty), a non-UTF-8 or unset locale, or
`--ascii` each independently force plain ASCII regardless of the others.

**`--json` output.** Read-only verbs (`recall`, `probe`, `introspect`) emit their
existing JSON shapes. Every mutating verb (`meld`, `learn`, `forget`, `sync`,
`upgrade`, `unmeld`, `config lobes add`/`remove`) emits a structured result
object with at minimum `action`, `target`, and `outcome` fields (CLI-153).

## Exit status

Exit 0 on success. Any `MindError` is printed to stderr and exits non-zero
(CLI-100).

`sync` exits non-zero (`SyncFailed`) when any per-source fetch fails, even if
other sources succeeded; successfully fetched commits are persisted and reported
(CLI-54).

`review` distinguishes hard errors (malformed `mind.toml`, unknown item kind,
unresolved `{{ns:}}` token) from advisory findings (unguarded references, missing
descriptions). Hard errors exit non-zero; advisory-only exits zero. Neither mode
writes to disk, except `review --fix` on a local-path target (CLI-132).

## Running unattended / in CI

Pass `--yes` (`-y`) to skip confirmation prompts. Without it, any command that
would prompt on a TTY instead exits non-zero with `ConfirmationRequired` when
stdin is not a TTY (CLI-23, CLI-42).

Install and uninstall hooks are skipped in non-TTY contexts and a note is
printed. To run them unattended, pass `--dangerously-skip-install-hook-check`.
This executes arbitrary code from the source; only use it for sources you trust
(HOOK-22).

## dump

`mind dump` writes a super-source `mind.toml` to stdout (or `--output <path>`)
that reproduces the current melded and installed state. Melding the output
recreates the same source set at the same revisions. It is the inverse of
melding a curated super-source.

```
mind dump                        # write to stdout
mind dump --output snapshot.toml # write to a file
mind dump --whole-sources        # include all items, not just installed ones
```

Each entry in the emitted `[discover].sources` references a melded source and
pins it to its currently recorded commit as a `pin-ref`, overriding any pin the
source itself declares (DUMP-1). The meld-time settings are carried through:
prefix (`as`), scan `roots`, and the resolved commit pin (DUMP-4).

**Item filtering.** By default each source entry is stamped with the install
directive that reproduces exactly which items are installed (DUMP-2):

- Every offered item installed: `install = true`
- No items installed: `install = false`
- A subset installed: `install_items = [...]` listing those items by `kind:name`

`--whole-sources` disables this filtering and emits `install = true` for every
source, offering the full catalog instead of the recorded subset (DUMP-3).

With no melded sources, `dump` emits a valid super-source with an empty
`[discover].sources` and exits 0 (DUMP-8).
