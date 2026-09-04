# Commands

## Mental model

`mind` connects *sources* (git repos full of agent tooling) and *lobes*
(your agent homes, like `~/.claude`/`~/.agents`).

**Source.** A melded git repo. `mind meld <repo>` clones the repo into
`~/.mind/sources/<host>/<owner>/<repo>` and records it in `~/.mind/sources.json`.
This initializes the source and makes its items available to `mind learn`.
`mind sync` (or re-melding, re-running `mind meld <repo>`) refreshes the clone.

**Item.** A unit offered by a source, one of five *kinds*:

- `skill` - a `skills/<name>/` directory containing a `SKILL.md` and any associated
  resources (scripts, templates, etc).
- `agent` - an `agents/<name>.md` file.
- `rule` - a `rules/<name>.md` file.
- `command` - a `commands/<name>.md` file: a harness slash command, offered at the
  prompt as `/<name>` (`/<prefix>:<name>` under a namespace). Claude Code has
  merged custom commands into skills, so a `commands/` file and a skill of the
  same name both produce `/<name>`; commands keep working and `mind` installs
  them, but a skill is the better shape for something new.

  A source that ships both `commands/foo.md` and `skills/foo/SKILL.md`
  installs both: they are distinct items (`command:foo` and `skill:foo`)
  linking into different lobe directories, so `mind` does not report a
  collision the way it does for two agents of the same name. Both offer
  `/foo` to the harness. Install only one unless you mean to have both.
- `tool` - a `tools/<name>/` directory containing a `TOOL.md`, a script/executable, 
  any associated resources. Tools are an optional feature to assist with managing
  and referencing shared scripts/executables utilized by multiple skills.

Items are discovered by convention (the paths above) or declared in a
`mind.toml`.

**Lobe.** A directory `mind` links items into: the directory holding `skills/`,
`agents/`, `rules/`, and `commands/`. A lobe may be a global agent home (`~/.claude`,
`~/.gemini/config`) or a project subdirectory (`.windsurf` inside a project root).
The default lobe is `~/.claude`; you can add Gemini, Codex, Windsurf, Antigravity,
or any directory, each with an optional per-kind filter (see
[Configuration](configuration.md)). The `gemini` preset (path `~/.gemini/config`)
covers both Gemini CLI and the Antigravity IDE. Users of the Antigravity CLI who
previously used an `antigravity-cli` preset should configure a custom lobe path
manually.

**Learn.** `mind learn <item>` copies the item out of the source clone into the
*store* (`~/.mind/store/<kind>/<name>`) and symlinks that store copy into every
lobe. The store copy is the stable thing your agent homes point at, so a later
`sync` cannot change an installed item under you until you choose to `upgrade`.
`forget` reverses it.

### What each step puts on disk

`mind meld jaemk/mind` clones the source. Nothing is linked yet:

```text
$ mind meld jaemk/mind

~/.mind/
  sources.json                              # registry: `mind` is now melded
  sources/
    github.com/jaemk/mind/                  # the clone (a staging area)
      examples/hello/skills/hello-mind/     # the offered skill
        SKILL.md
      ...                                   # the rest of the repo
  store/                                    # empty - nothing learned yet
```

`mind probe` browses what the melded sources offer before you learn anything
(`kind:name`, source, content hash, description):

```text
$ mind probe

  skill:hello-mind  github.com/jaemk/mind  c62e88cc  A hello-world skill; confirms mind melded this repo
```

`mind learn jaemk/mind#skill:hello-mind` copies that one item into the store and
symlinks it into the lobe:

```text
$ mind learn jaemk/mind#skill:hello-mind

~/.mind/
  manifest.json                                  # registry: `hello-mind` installed
  store/
    skill/hello-mind/SKILL.md                    # the copy, taken from the clone
  sources/
    github.com/jaemk/mind/ ...                   # clone untouched

~/.claude/                                       # a lobe (agent home)
  skills/
    hello-mind -> ~/.mind/store/skill/hello-mind # symlink the harness discovers
```

The harness now resolves `/hello-mind` through that symlink. A `tool` the skill
referenced would instead land in `~/.mind/store/tool/<name>` with no symlink
under `~/.claude`, present for the skill to call but invisible to the harness.

**Stay current.** `sync` refreshes every source clone; `upgrade` moves installed
items to the refreshed version, reporting hash and commit deltas before changing
anything; `evolve` updates the `mind` binary itself.

**Inspect.** `recall` and `probe` show what is installed and what is available;
`introspect` reports drift and broken links.

## Verbs

| command | does |
|---------|------|
| `mind meld [<repo>] [--register-only] [--kind <agent\|rule\|command>] [--learn <NAME\|GLOB>] [--yes] [-f\|--force] [-r\|--recursive] [-N\|--namespace <ns>] [--flat-skills] [--root <dir>] [--add-root <dir>] [--pin <HEAD\|ref\|branch=NAME\|tag=NAME>] [--install-hook <cmd>] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check]` | clone and register a source (default `.`), then prompt to install its items (`--register-only` registers without installing; `--learn <NAME\|GLOB>` installs only the items matching the pattern instead of offering the whole set, repeatable, and each match brings its dependencies with it, see [Installing part of a source](#installing-part-of-a-source); `--yes` installs without prompting; `-f`/`--force` overwrites conflicting non-mind link targets; `-r`/`--recursive` offers to install items from every nested source a super-source curates). `--pin` sets the version tracked (see [Pinning a source version](#pinning-a-source-version)). `--root` replaces the scan roots; `--add-root` adds roots that compose with the source's own discovery (a `marketplace.json`/`plugin.json` or an authoritative `mind.toml` keeps its items and the added roots are scanned in addition, see [Claude plugin marketplaces](marketplace.md#installing-items-the-manifest-does-not-list)). `<repo>` may also be a deep `tree`/`blob` URL to one item (an [item link](#item-links-install-one-item-by-url)); `--kind <agent\|rule\|command>` declares what a linked `.md` file is when its directory and frontmatter do not. A meld that discovers no items reports the convention paths it scanned and the flags that reach items laid out elsewhere in the repo. Re-melding an already-melded source installs any missing items, else shows each item's install state and commit. `--pin` on a re-meld re-pins the source: it resolves the new pin, re-checks-out the clone if the commit differs, and records both, leaving the source untouched if anything fails. `--root`, `--add-root`, `--flat-skills`, and `--install-hook` apply only at the meld that first registers a source, since they change what is discovered, so a re-meld notes which of them it ignored (`unmeld`, then meld again, to change them). Re-melding with a different `-N`/`--namespace` registers a new coexisting `host/owner/repo@<prefix>` instance (its own clone, pin, and items) rather than changing the existing source's prefix in place; to change a source's prefix without forking a new instance, use `mind probe`'s **Set namespace** action in the source's details dialog (only offered while the source has no installed items; see [Details dialog](tui.md#details-dialog)) |
| `mind init-source [<path>] [--template] [-N\|--namespace <ns>] [--marketplace] [--flat-skills]` | scaffold `mind.toml` + report references; `--template` rewrites bare refs as `{{ns:}}` (maintainer); `-N`/`--namespace` sets `[source].namespace` in the scaffold; `--marketplace` emits a `.claude-plugin/` marketplace scaffold; `--flat-skills` uses a flat skill layout |
| `mind unmeld <name> [--keep-items] [--yes] [--uninstall-hook <cmd>] [--dangerously-skip-hook-check]` | uninstall every item the source installed and drop the source (`--keep-items` skips the uninstall step). `<name>` is a ref: it must resolve to exactly one source (a non-glob name matching several errors instead of guessing; use a glob to act on several). It may be a bare source name or, to address one specific instance, its full identity: `host/owner/repo@<prefix>` for an aliased instance, `host/owner/repo#<path>` for an item-link instance. The same identity forms select an instance for `upgrade` and `recall`, e.g. `mind unmeld github.com/acme/skills@jk`. Contrast `sync [source]` below, a filter that may match, and act on, several sources at once |
| `mind learn [--yes] [-f\|--force] [-n\|--dry-run] [--all] [--pin] [--kind <agent\|rule\|command>] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check] <item>` | install a skill/agent/rule/command/tool (glob installs many); a partial selection also pulls in the source siblings it references. `<item>` may also be a deep `tree`/`blob` URL to one item in a repo: the repo registers as a single-item [item link](#item-links-install-one-item-by-url) and the item installs in the same step (`--kind <agent\|rule\|command>` declares what a linked `.md` file is). `--force` overwrites a conflicting non-mind link target (without it, a conflict prompts on a TTY); `--all` installs every item of the named source (shorthand for `<source>#*`); `--pin` freezes a deep-link URL to the ref's current commit (ignored for a plain ref); `-n`/`--dry-run` previews the dependency closure without installing anything; under `--json`, installing an item whose closure pulls in dependencies beyond the explicit selection requires `--yes`, since there is no prompt to answer (refuses with `confirmation-required` otherwise) |
| `mind forget [--yes] [-f\|--force] [--unmanaged] [--dangerously-skip-hook-check] [<item>]` (alias `unlearn`) | remove an installed item (glob removes many; a multi-match glob confirms first, `--yes` skips). `--unmanaged` scopes removal to unmanaged lobe items only; with no `<item>`, removes every unmanaged item across all lobes. `-f`/`--force` skips the dependents confirmation when the item being removed has dependents. `--dangerously-skip-hook-check` runs uninstall hooks without the safety prompt |
| `mind sync [source] [--upgrade] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check]` | refresh source clones (all, or every source whose name matches the optional `[source]` filter: exactly, by trailing suffix, or by glob; a suffix shared by several sources matches, and syncs, all of them, unlike `unmeld`'s ref-style source selection, since `sync` has no ambiguity check and is a non-destructive refresh). `--upgrade` is deprecated sugar for `sync` followed by `upgrade`, scoped to the matched sources when `[source]` is given; when the scope spans more than one source, a disclosure names every matched source before the install-hook re-run pass -- even with nothing pending -- and this is not suppressed by `--yes` (the two hook-check flags are valid only with `--upgrade`) |
| `mind upgrade [--yes] [--no-sync] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check] [item]` | fetch each involved source, then upgrade installed items to their latest version (re-runs install hooks on sources that advance, or their update hooks when they declare any); `item` is a filter over installed items (a glob matches several, and a bare non-glob name multi-matches only across kinds, e.g. `upgrade greet` hitting both `skill:greet` and `agent:greet`); an embedded `owner/repo#` source qualifier is also a filter, not a ref -- a trailing-suffix match with no ambiguity check, so it can span several sources and re-run each one's install hook; when the scope spans more than one source, a disclosure names every matched source before the hook re-run pass, even with nothing pending, and `--yes` does not suppress it; `--no-sync` skips the fetch step |
| `mind curate [--check] [--yes] [--prune] [--no-sync] [--adopt <identity>] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check]` (alias: `reconcile`) | apply what your curators declare now: fetch every curator and curated source, then report one plan (entries listed upstream but not registered, declared items not installed, pins that no longer match the curator's directive, curated sources whose items are out of date, sources no curator lists any more) and offer to apply it. `--check` reports and changes nothing; `--yes` applies without asking; `--prune` also applies the `unlist` changes, which uninstall a source's items and drop it; `--no-sync` plans against the clones already on disk; `--adopt <identity>` brings a pre-existing source under a curator's ownership. See [Curate: follow your curators](#curate-follow-your-curators) |
| `mind hooks run <target> [--event install\|update\|uninstall\|build] [--force\|--rerun] [--dangerously-skip-install-hook-check] [--dangerously-skip-build-hook-check] [--json]` / `mind hooks list <target> [--json]` | run a source's or an item's hooks on demand (outside meld/learn/forget/upgrade), or list the hooks in effect without running any. `--rerun` is a visible alias for `--force` (re-run a hook already recorded as run, mirroring `meld --force`). `<target>` is a source filter or an `owner/repo#item` ref; there is no ambiguity check, so a filter matching several sources, or a ref matching several items, runs the hook for each in turn. See [Install hooks](install-hooks.md#running-hooks-on-demand) |
| `mind evolve [--check] [--yes] [--to <v>]` | upgrade the mind binary itself to the latest release, or to a pinned `--to <v>` (accepts a prerelease like `1.2.3-rc1` -- the only way to reach one, since the latest-release lookup never surfaces a prerelease -- and strips a single leading `v`; `--version` is a deprecated alias) |
| `mind recall [item] [--sources] [--kind K] [--source S] [--tree] [--json]` (alias `status`) | status: each source with its items, marked installed or available; `--sources` narrows to sources; `<item>` shows one item's details; `--tree` renders installed items as a dependency forest (with an item ref, scopes to that item's subtree) |
| `mind probe [query] [--kind K] [--source S] [--json] [--no-tui]` | browse and search items (interactive TUI on a terminal) |
| `mind review <target> [-N\|--namespace <ns>] [--json]` / `mind review --policy <path> [--json]` | validate a source for publishing, or validate a managed policy file (read-only); a `<target>` naming an existing directory is read as that local path unless it first matches a melded source's identity, even when it also looks like a valid `owner/repo` spec |
| `mind introspect [--fix] [--json]` | report drift and broken links (optionally repair) |
| `mind config show` / `mind config lobes add [<dir>] [--preset <name>] [--subdir <rel>] [--snapshot] [--force]\|list\|remove <path> [--snapshot]\|detect [--yes] [--json]` | view config and manage lobes. `add --preset <name>` adds a preset lobe; `--preset` and a base `<dir>` are composable (e.g. `add . --preset windsurf` registers a lobe at `./.windsurf`). `--subdir <rel>` targets an arbitrary subdirectory under `<dir>`. `--snapshot` on `add` materializes a one-time frozen copy instead of registering a managed lobe; on `remove`, detaches a managed lobe by replacing its symlinks with real-file copies before unregistering. `--force` overwrites a colliding foreign file. `detect` reports which known harness homes exist; Windsurf is detected via `~/.codeium/windsurf` but prints guidance to run `link-project` instead of auto-adding a lobe. `config lobes list` and `config show` include the kinds filter for each lobe, e.g. `~/.gemini/config [skill]`. See [Configuration](configuration.md) for the preset table and per-harness path details. |
| `mind link-project [<dir>] [--preset <name>] [--subdir <rel>] [--snapshot] [--force]` | convenience alias for `config lobes add`, with `<dir>` defaulting to `.` and `--preset` defaulting to `windsurf`; links installed skills into `./.windsurf/skills/` and registers a managed lobe so future `mind learn` fans new skills into it automatically. `--snapshot` writes real-file copies instead of a managed lobe: unmanaged, committable, and not updated by a later `mind learn` (see [Presets](configuration.md#presets)) |
| `mind dump [--output <path>] [--whole-sources]` | write a super-source `mind.toml` reproducing the current melded and installed state (to stdout or `--output <path>`); each source is pinned to its recorded commit; item-filtered by default (`--whole-sources` emits `install = true` for every source regardless of install count) |
| `mind absorb <ref> [--to <path>] [-f\|--force]` | claim a single unmanaged lobe item into a version-controlled source and install it as a managed item; `--to` sets the destination (see [absorb](absorb.md) for full destination precedence); `--force` overwrites a `kind:name` collision at the destination |
| `mind completions <shell>` / `mind man` | shell completions / man page |

A source repo exposes items by convention (`skills/<n>/SKILL.md`,
`agents/<n>.md`, `rules/<n>.md`, `commands/<n>.md`, `tools/<n>/`), via a `mind.toml`, or via a Claude
`.claude-plugin/` manifest (see [Claude plugin marketplaces](marketplace.md)). See
[Source layout](source-layout.md) and the
[examples/](https://github.com/jaemk/mind/tree/main/examples): `starter` for the
plain convention layout, `namespacing` for `{{ns:}}` reference tokens under a
namespace, and `policy` for an enterprise managed policy. The full behavioral spec
is at [spec/](https://github.com/jaemk/mind/tree/main/spec).

### Verb aliases

Each primary verb has a visible alias for users familiar with conventional package-manager vocabulary. The primary verb is always preferred in docs and scripts.

| alias | primary verb |
|-------|-------------|
| `add` | `meld` |
| `install` | `learn` |
| `uninstall` | `forget` |
| `update` | `sync` |
| `search` | `probe` |
| `list` | `recall` |
| `doctor` | `introspect` |
| `self-update` | `evolve` |

> **Note (migration):** `--link-only` on `meld` is now `--register-only`; `--unlink-only` on `unmeld` is now `--keep-items`. The old spellings continue to work as hidden deprecated aliases. The `config target` and `unmeld detach` aliases are removed.

> **Note (migration):** `upgrade` now syncs the involved sources before computing deltas (equivalent to running `sync` then `upgrade` on those sources). Pass `--no-sync` to skip the fetch step and get the old behavior. `sync --upgrade` is kept as deprecated sugar but `upgrade` alone is the preferred form.

## Pinning a source version

`meld --pin <value>` records how a source tracks upstream. The value is required
and decides freeze vs follow:

| value | effect |
|-------|--------|
| `HEAD` | freeze the current resolved tip to its commit (immutable) |
| `<tag\|sha\|branch>` | resolve that ref to its current commit and freeze it |
| `branch=<name>` | follow that branch (floating; `sync` advances it) |
| `tag=<name>` | follow that tag (re-points on `sync` if it moves) |

A freeze records an immutable `ref` that `sync` never moves. Freezing a branch or
tag snapshots its current tip; it does not keep tracking it. With no `--pin`, a
source follows the remote default branch.

To change a pin later, run `meld --pin` again against the already-melded source.
It resolves the new value against the pin currently in effect, re-checks-out the
clone when the commit differs, and records both. Nothing is mutated until every
git step has succeeded, so a bad ref leaves the source on its old pin.

> **Common mistake:** `--pin stable` and `--pin branch=stable` are different.
> `--pin stable` takes a one-time snapshot of `stable` (frozen; `sync` never
> updates it); `--pin branch=stable` tracks the branch (floating; `sync` advances
> it). Use the `branch=` form to keep following a branch.

```
# Freeze the current tip.
mind meld owner/repo --pin HEAD

# Freeze a specific point: a tag, a commit sha, or a branch's current tip.
mind meld owner/repo --pin v2.0
mind meld owner/repo --pin a1b2c3d4e5f6...
mind meld owner/repo --pin release-branch

# Follow a moving point. sync advances these to the tip on each run.
mind meld owner/repo --pin branch=stable
mind meld owner/repo --pin tag=latest
```

A `--pin` value overrides the repo's `[source]` pin directive (see
[The mind.toml file](mind-toml.md)). `--pin` requires a value, so a bare `--pin`
is a usage error rather than silently consuming the next argument.

For a deep-link URL, `learn --pin` (a bare flag on `learn`) freezes the link's ref
to its current commit in the same step:

```
# Without --pin the link follows the branch in the URL; with it the link is
# frozen at that branch's current commit.
mind learn https://github.com/owner/repo/tree/main/skills/foo --pin
```

> **Note (migration):** the old `--follow-branch <b>` / `--pin-tag <t>` /
> `--pin-ref <c>` flags map to `--pin branch=<b>` / `--pin tag=<t>` / `--pin <c>`.
> They still work as hidden deprecated aliases.

## probe

`mind probe` with no flags opens an interactive browser of melded sources and
items (search, install, remove, meld, unmeld, sync, upgrade) when stdout is a
terminal. `--no-tui` or `--json`, or a piped or redirected stdout, prints
the listing instead.

## Selecting items (globs)

`learn`, `forget`, `upgrade`, `unmeld`, `probe`, `recall`, and `sync` all
accept a glob in place of an exact ref or filter argument. The kind prefix,
source qualifier, and glob compose:

| pattern | selects |
|---------|---------|
| `'*'` | every item across all sources |
| `'skill:*'` | all skills |
| `'owner/repo#*'` | all items of one source |
| `'review*'` | items whose name starts with `review` |

The glob is matched against the effective (installed) name. A glob matching
nothing is `ItemNotFound` (for items) or `SourceNotFound` (for sources). The
exception is `upgrade`: a glob (or exact ref) that matches no installed item
reports up-to-date rather than erroring, since upgrading nothing is a no-op.

Shell-quoting caveat: quote the glob so the shell does not expand it before
`mind` sees it:

```
mind learn 'skill:*'
mind forget 'owner/repo#*'
```

Spec: CLI-31, CLI-41, CLI-65.

### Refs vs filters

- **Ref** must resolve to exactly one target, erroring rather than guessing:
  `unmeld <name>` (a non-glob source name; `AmbiguousSource`), and a bare
  non-glob item name given to `learn`/`forget`/`upgrade` (`AmbiguousItem`).
  Item names are never suffix-matched, so this last case only fires across
  kinds (the manifest is keyed `kind:name`), e.g. `upgrade greet` hitting
  both `skill:greet` and `agent:greet`.
- **Filter** may match several targets with no ambiguity check, and every
  match is acted on: `sync`'s `[source]`, `recall`/`probe`'s `--source
  <filter>`, `hooks run <target>`'s source form, and any glob (`'*'`,
  `'skill:*'`, `'owner/repo#*'`) given to any of the verbs above. The
  embedded `owner/repo#` source qualifier inside `upgrade`'s item argument is
  a filter too, not a ref: it is a trailing-suffix match with no ambiguity
  check, so it can span several sources, upgrading each one's items and
  re-running each one's install hook.

No-match behavior differs by verb:

- `sync <filter>` with no match: a hard `SourceNotFound` error.
- `upgrade <filter>` with no match: reports up to date and exits 0.
- `--source <filter>` (`recall`/`probe`) with no match: an empty listing.
- `meld --learn <pattern>` with no match: a hard error naming the source, and
  the source stays melded (see [Installing part of a source](#installing-part-of-a-source)).

## Installing part of a source

By default `meld` offers the source's whole set. `--learn` narrows that to the
items you name, in one command:

```
mind meld owner/repo --learn review --yes
mind meld owner/repo --learn 'skill:*' --learn agent:dev --yes
```

The pattern selects within the source being melded, so pass an item name or
glob, never a source-qualified ref. Quote a glob so the shell does not expand it
first. `*`, `?`, and `[` are always glob syntax here, so an item whose name
contains one of them cannot be selected as a literal. A name matches whether or
not the source namespaces its items, so `--learn review` finds an item that
installs as `team:review`.

Each match installs through the ordinary `learn` path, so the siblings it
references come with it (see [Dependencies](dependencies.md)). The rest of the
source stays registered and available to `learn` later. This is a
consumer-side selection with its own matching rules; a super-source's own
`install-items` list (see
[mind.toml](mind-toml.md#discoversources---curated-super-source)) is
different: literal `kind:name` refs only, no globs.

Notes:

- `--yes` matters. Like the install-all offer it replaces, `--learn` prompts on a
  terminal and installs nothing off one, so a scripted run without `--yes`
  registers the source and exits 0 having installed nothing.
- A pattern matching nothing in the source is an error naming that source, and
  the source stays melded. An item the source's inventory does not declare is
  not reachable this way; use `--add-root <dir>` for those, at the meld that
  first registers the source (it is ignored on a re-meld, so an already-melded
  repo must be unmelded first).
- Scoped to the melded source's own items. The sources a super-source curates are
  still registered, but none of their items are installed, so `--recursive` has
  nothing to do here and says so.
- Conflicts with `--register-only`.

Spec: CLI-236.

## Item links: install one item by URL

Paste the URL of a skill directory (or its `SKILL.md`) straight from GitHub or
GitLab and `learn` installs just that skill:

```
mind learn https://github.com/owner/repo/tree/main/skills/foo
mind learn https://github.com/owner/repo/blob/main/skills/foo/SKILL.md
```

A `blob` (or `tree`) URL to any other `.md` file installs that one file as an
agent, rule, or command:

```
mind learn https://github.com/owner/repo/blob/main/agents/reviewer.md
mind learn https://github.com/owner/repo/blob/main/rules/style.md
```

The kind is resolved in three steps, first hit wins: `--kind
<agent|rule|command>` on the command, else the containing directory
(`agents/`, `rules/`, `commands/`), else a `kind:` key in the file's own
frontmatter. So a link into a conventional layout needs no annotation, and a
file that sits elsewhere needs one of the other two:

```
mind learn https://github.com/owner/repo/blob/main/vendor/reviewer.md --kind agent
```

`--kind` applies to item links only, and is fixed at meld/registration time:
only the explicit form is recorded on the instance (a directory- or
frontmatter-resolved kind is re-read from the pinned clone on every scan), and
re-running `--kind` against an already-melded link changes nothing (noted,
not silently dropped). Skills and tools are directories, so naming one on a
file link is refused, and anything but `skill` is refused on a link naming a
directory; the CLI flag itself only ever accepts `agent`, `rule`, or `command`
(distinct from `recall`/`probe --kind`'s five-kind filter), so `--kind
skill`/`--kind tool` there is rejected before any clone.

The repo registers as its own single-item source instance with the identity
`host/owner/repo#<path>`: it clones, syncs, and upgrades like any source, but
offers exactly the linked item. Several links into the same repo (and a plain
meld of it) coexist as separate sources. The URL's ref supplies the pin: a
branch name follows that branch, a 40-hex commit pins it. Add `--pin` (a bare flag
on `learn`) to freeze a branch-ref link at the branch's current commit (see
[Pinning a source version](#pinning-a-source-version)).

Because the consumer names the exact path, the link bypasses the repo's
declared inventory: it reaches an item an authoritative `mind.toml` or a
`.claude-plugin/marketplace.json` does not list (see
[Claude plugin marketplaces](marketplace.md#installing-items-the-manifest-does-not-list)).

A curator can list an item link in `[discover].sources`, with `kind = "agent"`
when the file needs it:

```toml
[[discover.sources]]
source = "https://github.com/owner/repo/blob/main/vendor/reviewer.md"
kind = "agent"
install = true
```

`mind meld <url>` accepts the same form and follows the standard meld flow
(`--register-only`, `--namespace`, `--kind`, pin flags). `forget` of the item leaves the
instance registered and hints at `mind unmeld <identity>` to drop it. A local
repo is addressable through `file:///path/to/repo/tree/<branch>/<path>`.

A link instance offers exactly the linked item, so an item that references a
sibling (a `requires:` frontmatter entry, or a `{{ns:}}` / `{{tools:}}` /
`{{path:}}` token, see [Dependencies](dependencies.md)) cannot get it this way. A
`requires:` entry is metadata, so the item installs, mind warns which entries
were dropped, and the drop is recorded on the item (`mind recall <item>` and
`mind introspect` show it afterwards). A token is rewritten into the item's
text, so it is an error and nothing is installed. Both name the same
kind-qualified remedy, which replaces the link with the whole repo and
installs that one item with its dependencies. `mind` checks whether a plain
meld of the repo would find the item on its own and prints one of two forms
accordingly:

```
mind unmeld 'github.com/owner/repo#skills/foo' --yes && \
  mind meld https://github.com/owner/repo --learn 'skill:foo' --yes
```

or, when an authoritative `mind.toml` or `.claude-plugin` manifest does not
declare the skill:

```
mind unmeld 'github.com/owner/repo#skills/foo' --yes && \
  mind meld https://github.com/owner/repo --add-root '.' --learn 'skill:foo' --yes
```

The `unmeld` step is what lets the command run as printed: the link instance is
registered either way, and in the `requires` case it has already installed the
skill the second half would install again.

The pattern carries the linked item's own kind (`skill:foo`, `agent:reviewer`).
The scan root is derived from the link's own path, so the second form works at
any depth: a link to `packages/foo/skills/bar` prints `--add-root
'packages/foo'`. One shape has no working remedy and prints none: a file link
outside its kind's directory (`vendor/reviewer.md`), which convention discovery
never finds, so the error says to drop the reference or move the file under
`agents/` upstream. What it does not carry over is the rest of the source's
configuration, since it melds the repo fresh: a `--namespace`, `--pin`, or extra
`--add-root` the original link was melded with has to be restated by hand.

Spec: [spec/item-link.md](https://github.com/jaemk/mind/blob/main/spec/item-link.md).

## Filtering with --kind and --source

`recall` and `probe` accept two composable filters:

- `--kind <skill|agent|rule|command|tool>` narrows to one item kind.
- `--source <filter>` narrows to items from a matching source. The filter is
  an exact name, a trailing suffix (`repo` or `owner/repo`; a multi-source
  match is normal, not an error), or a glob matched against the full
  `host/owner/repo` identity (so `*` spans `/`):

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

**`--json` output.** `--json` is universal: every verb answers it with exactly
one JSON document on stdout, except a closed exclusion list -- `dump` (always
emits TOML, CLI-153 does not apply), `completions`/`man` (print their script or
roff page as the entire output), `evolve` (writes its own result document from
`selfupdate.rs` rather than through the shared JSON machinery -- the document
IS JSON, just emitted on a separate path), and `init-source` (a maintainer
scaffolder with no JSON result to offer). Every other verb answers with a
document.

`recall` and `probe` emit `{"schema": 1, "items": [...]}`.
`introspect` emits `{"schema": 1, "issues": [...], "sources": N, "items": N}`
where `issues` is an array of findings and `sources`/`items` are integer counts.
Every mutating verb (`meld`, `learn`, `forget`,
`sync`, `upgrade`, `unmeld`, `config lobes add`/`remove`) emits a structured
result object with `"schema": 1` and at minimum `action`, `target`, and `outcome`
fields (CLI-153).

`review --json` answers with `{"schema": 1, "action": "review", "outcome":
"clean|advisory|failed", "hard": [...], "advisory": [...], "fixed": [...]}`,
where `hard`/`advisory` are arrays of `{"kind": "<slug>", "message": "<text>"}`
findings and `fixed` lists the files `--fix` rewrote. A hard finding still
fails `review` (CLI-132 is unconditional): the document is not printed as a
success envelope in that case, but folded into the error envelope's `details`
member instead (see below).

`hooks list --json` answers with a document giving each matched source's or
item's hooks (event, required/optional flag, command, and, for a recorded
source install hook, its pending/last-ran status). `hooks run --json` on a
successful run answers with a tally rather than nothing: `{"schema": 1,
"action": "hooks-run", "target": "...", "event": "...", "existed": N, "ran": N,
"skipped": N}`.

When an error occurs under `--json`, the process emits a JSON error envelope on
stdout instead of plain text on stderr, then exits 1 (unchanged):

```json
{"schema": 1, "error": {"kind": "item-not-found", "message": "..."}}
```

The `kind` field is a stable kebab-case slug per `MindError` variant (e.g.
`"item-not-found"`, `"source-not-found"`, `"git"`, `"digest-mismatch"`).
Scripts may branch on `kind` to handle specific failures. The `message` field
is the full display text. Exit code is always 1 for runtime errors; clap usage
errors (exit 2) remain plain text and are not enveloped (CLI-181, CLI-182,
CLI-183).

The envelope carries an optional `details` member when a verb has more to say
than the `kind`/`message` pair: a failed `review --json` records its findings
document there instead of printing it as a success envelope (CLI-221).

## Exit status

Exit 0 on success. Any `MindError` exits 1; under `--json` it is written to
stdout as the error envelope above instead of stderr (CLI-100, CLI-181).

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

`meld` is the exception: a non-TTY `meld` without `--yes` registers the source
only, prints a note that nothing was installed, and exits 0. Pass `--yes` to
install its items non-interactively.

Install and uninstall hooks are skipped in non-TTY contexts and a note is
printed. To run them unattended, pass `--dangerously-skip-install-hook-check`
(`meld`/`learn`/`sync`/`upgrade`/`hooks run`) or `--dangerously-skip-hook-check`
(`unmeld`/`forget`, which gate an uninstall hook). This executes arbitrary code
from the source; only use it for sources you trust (HOOK-22).

Item build hooks (the per-item build command, distinct from a source's install
hook) are likewise skipped in non-TTY contexts, so an item's tooling is not
built. To run them unattended, pass `--dangerously-skip-build-hook-check`
(available on `meld`, `learn`, `upgrade`, and `sync --upgrade`). It too executes
arbitrary code from the source; only use it for sources you trust.

For an end-to-end CI provisioning recipe, see [Team / CI provisioning
recipe](enterprise.md#team--ci-provisioning-recipe).

## Curate: follow your curators

A *curator* is a melded source that lists other sources: a `mind.toml` with
`[discover].sources` (a super-source), or a Claude plugin marketplace catalog.
`mind curate` is the command to run when one of them changes:

```
mind curate            # report the plan, ask once, apply
mind curate --check    # report only
mind curate --yes      # apply without asking
mind curate --no-sync  # plan against the clones on disk, fetch nothing
```

It fetches every curator and the sources it owns, then reports one plan:

| change | what it means | applying it |
|--------|---------------|-------------|
| `register` | the curator lists a source you do not have | registers it, then installs what the entry declares |
| `install` | the entry declares items (`install = true` / `install-items`) that are not installed | installs them |
| `repin` | the entry's `pin-ref`/`follow-branch`/`pin-tag` no longer matches the recorded pin | re-pins and re-checks-out the clone |
| `upgrade` | a curated source's installed items are out of date | runs the upgrade pass over those sources |
| `unlist` | a source you registered from a curator's list is no longer on it | `--prune` only: uninstalls its items and drops the source |
| `namespace` | the entry declares a different namespace than the instance carries | reported only: the namespace is part of a source's identity, so the report prints the `unmeld` + `meld` pair that adopts it |
| `adopt` | a curator lists a source you already have, but does not (yet) own it | reported only: run the `mind curate --adopt <identity>` command shown to let that curator start managing it |

`unlist` is never applied by `--yes` alone. A curator dropping an entry would
otherwise uninstall your items in an unattended run, so it takes `--prune`.
`namespace` and `adopt` are always advisory: neither is ever applied by
`curate` itself, `--yes` included, only by running the command the report
names.

**`curate` only ever changes a source it (or a curator) actually
registered.** A source you melded directly, or one a curator's entry happens
to name without having registered it, is never mutated: it shows up as
`adopt` instead of `install`/`repin`/`upgrade`, so you can see it and opt in
rather than have it silently skipped or silently taken over. This also means
a source curated before you first ran `curate` (there is no such thing yet in
an existing registry -- `curated_by` and `curate` ship together) needs one
`--adopt` per source before `curate` starts managing it; after that it is
managed like anything registered through a curator from the start.

A per-source failure (an unreadable curator, a bad pin directive, a scan that
fails on one curated source) is reported in the `skipped` list and does not
abort the run; on a non-TTY run (CI, a script) with no `--yes`, `curate`
reports the plan and applies nothing rather than hanging on a prompt.
Registering and installing run the same consent-gated hooks any meld/learn
does, so `--dangerously-skip-install-hook-check` and
`--dangerously-skip-build-hook-check` exist on `curate` too.

What `curate` covers that the other verbs do not: `sync` registers a newly
listed entry but installs nothing from it, even when the curator marked it
`install = true`, and neither `sync` nor `upgrade` re-reads a curator's pin or
notices an entry the curator dropped. Changes apply in a fixed order
(`register`, `install`, `repin`, `upgrade`, `unlist`), pending changes exit 0,
and `--json` answers with one document (`outcome` is `clean`, `pending`, or
`applied`).

Spec: [spec/curate.md](https://github.com/jaemk/mind/blob/main/spec/curate.md).

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
namespace, scan `roots`, added roots, and the resolved commit pin (DUMP-4,
DUMP-11).

An [item link](#item-links-install-one-item-by-url) instance is emitted as a
deep URL rebuilt from its recorded parts -- `tree` for a skill link, `blob` for
a file link (agent/rule/command), carrying `kind = "..."` when the instance
recorded an explicit one (LNK-23) -- pinned by `pin-ref` like any other entry,
so an item installed from a pasted URL is reproduced by melding the dump
(LNK-13).

**Item filtering.** By default each source entry is stamped with the install
directive that reproduces exactly which items are installed (DUMP-2):

- Every offered item installed: `install = true`
- No items installed: `install = false`
- A subset installed: `install-items = [...]` listing those items by `kind:name`

`--whole-sources` disables this filtering and emits `install = true` for every
source, offering the full catalog instead of the recorded subset (DUMP-3).

With no melded sources, `dump` emits a valid super-source with an empty
`[discover].sources` and exits 0 (DUMP-8).
