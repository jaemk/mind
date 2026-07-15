# Configuration

## Agent homes (lobes)

`learn` links items into every configured install-target directory (a *lobe*).
A lobe is any directory mind links items into -- a global agent home such as
`~/.claude` or `~/.gemini/config`, or a project subdirectory such as `.windsurf`
inside a project root. Each item is linked under its kind subdirectory: `skills/`,
`agents/`, `rules/`. The default lobe is `~/.claude`. Configure more in
`~/.mind/config.toml`:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
```

The file is created with the default lobe (`~/.claude`) on first use. For a single
invocation, set `MIND_AGENT_HOMES` to a `:`-separated path list instead.

**Lobe precedence (STO-14):** `MIND_AGENT_HOMES` wins over `lobes` in
`config.toml`, which wins over the default `~/.claude`. An unknown key in
`config.toml` is a hard error.

Use `mind config lobes add <path>` and `mind config lobes remove <path>` to
manage lobes without hand-editing the file; see [Commands](commands.md) for the
full verb list.

### Kinds filter

A lobe may carry a `kinds` list so only items of the listed kinds link into it.
A lobe without a `kinds` field receives all kinds (existing behavior; a bare string
is equivalent):

```toml
lobes = ["~/.claude", { path = "~/.gemini/config", kinds = ["skill"] }]
```

Linking an item into a lobe whose `kinds` excludes its kind is a no-op for that
lobe, not an error. The manifest records only the lobes that actually received a
link, so `forget`, `upgrade`, and `introspect` never expect a link a filtered lobe
was never given (HARN-1, HARN-2).

### Cross-harness lobes

Skills (`skills/<n>/SKILL.md`) and agents (`agents/<n>.md`) use layouts that are
now cross-harness conventions. mind links them verbatim -- no content transform is
needed. Rules (`rules/<name>.md`) have no cross-harness directory equivalent (the
analog in other harnesses is a single concatenated context file like `AGENTS.md`
or `GEMINI.md`, not a directory of per-rule files), so rules are Claude-only and
are never linked into a lobe added via a non-Claude preset (HARN-3).

Per-harness path table:

| Harness | Skills dir | Agents dir | mind lobe (parent) |
|---------|------------|------------|--------------------|
| Claude Code | `~/.claude/skills/<n>/SKILL.md` | `~/.claude/agents/<n>.md` | `~/.claude` |
| Gemini CLI / Antigravity | `~/.gemini/config/skills/` | - | `~/.gemini/config` |
| Codex CLI | `~/.agents/skills/` | (subagents) | `~/.agents` |
| Windsurf | `<project>/.windsurf/skills/` | - | `<project>/.windsurf` |

`~/.agents` is a vendor-neutral alias: Codex reads it as its user skills path, so
one `~/.agents` lobe serves Codex and any harness that follows the same convention.

Windsurf has no global skills directory; it reads skills only from a project's
`.windsurf/skills/` folder. A Windsurf lobe is therefore a project lobe, not a
global one. Use `mind link-project` (or `mind config lobes add . --preset windsurf`)
in a project root to register one. Windsurf is detected via `~/.codeium/windsurf`;
see the Presets section below for how `config lobes detect` handles it.

### Presets

`mind config lobes add [<dir>] --preset <name>` adds a lobe at the preset's target
path with its `kinds` filter in one step. `--preset` and a base `<dir>` are
composable:

| preset | target path | kinds |
|--------|-------------|-------|
| `gemini` | `~/.gemini/config` | skill |
| `codex` | `~/.agents` | skill |
| `universal` | `~/.agents` | skill |
| `windsurf` | `<dir>/.windsurf` (default: `<cwd>/.windsurf`) | skill |

`gemini`, `codex`, and `universal` target a fixed global path. `windsurf` is
project-scoped: `mind config lobes add --preset windsurf` (no `<dir>`) targets the
current directory.

Additional flags for `config lobes add`:

- `--subdir <rel>` - target an arbitrary subdirectory under `<dir>` (e.g.
  `--subdir .cursor` for a Cursor project directory) instead of the preset's default.
- `--snapshot` - materialize a one-time frozen copy (real files, not symlinks) into
  the target and do not register a lobe. A snapshot is not managed: no
  auto-propagation on future `mind learn`, and the copied files are committable to
  the repo. Managed lobes use symlinks into `~/.mind/store` and should be gitignored.
- `--force` - overwrite a colliding foreign file at the target path.

Examples:

```
mind config lobes add --preset gemini
# + added gemini lobe ~/.gemini/config [skill]

mind config lobes add . --preset windsurf
# + added windsurf lobe ./.windsurf [skill]
```

`mind config lobes remove <path> [--snapshot]` unregisters a lobe. `--snapshot`
detaches a managed lobe: it replaces the managed symlinks with frozen real-file
copies and then unregisters the lobe.

> **Note (migration):** The `gemini` preset path changed from `~/.gemini` to
> `~/.gemini/config` in a previous release. If you added this preset earlier, your
> `~/.mind/config.toml` may still have the old path. Update it by running:
> ```
> mind config lobes remove ~/.gemini && mind config lobes add --preset gemini
> ```
> Or hand-edit `~/.mind/config.toml` and replace `~/.gemini` with `~/.gemini/config`.

`mind config lobes detect` detects which known harness homes exist on the machine
and reports the matching presets it could add. It never mutates config on its own:
it only adds a lobe with `--yes` or an interactive TTY confirm. `--json` emits the
detection result as structured JSON (HARN-5). Windsurf is detected via
`~/.codeium/windsurf`; because Windsurf is project-scoped, `detect` prints guidance
to run `mind link-project` in the project root rather than auto-adding a lobe.

`mind config lobes list` shows the `kinds` filter for each lobe (e.g.
`~/.gemini/config [skill]`); a lobe with no filter shows just the path. `mind
config show` uses the same format.

### link-project

`mind link-project [<dir>] [--preset <name>] [--subdir <rel>] [--snapshot] [--force]`
is a convenience alias for `config lobes add`, with `<dir>` defaulting to the current
directory and `--preset` defaulting to `windsurf`. Running it in a project root links
installed skills into `./.windsurf/skills/` and registers a managed lobe so future
`mind learn` fans new skills into it automatically:

```
cd ~/projects/myapp
mind link-project
# + added windsurf lobe ./.windsurf [skill]
```

Managed lobes use symlinks into `~/.mind/store`; add `.windsurf/` to the project's
`.gitignore`. Use `--snapshot` to materialize real-file copies instead (committable
to the repo without gitignoring).

### Frontmatter portability

mind links skill and agent files verbatim. Frontmatter portability across
harnesses -- for example, Gemini's `mcp_*` tool-permission wildcards vs Claude's
`tools:` schema -- is the author's responsibility; mind does not rewrite
frontmatter to fit a target harness (HARN-6). An item whose frontmatter uses
Claude-specific keys will link into a Gemini or Codex lobe correctly, but those
keys may be ignored or produce a warning in the target harness.

## Absorb destination

`mind absorb` moves an unmanaged item into a version-controlled source you own.
The destination source is resolved from three places, in order -- the first one
set wins:

1. `--to <path>` flag on the command line.
2. `MIND_ABSORB_TO` environment variable.
3. `absorb-to` key in `~/.mind/config.toml`.

When none of the three is set and the run is interactive, `absorb` prompts and
offers `~/.mind/personal` as the default. That directory is created and
`git init`-ed on demand if it does not exist. After an interactive resolution,
`absorb` offers to save the chosen path as `absorb-to` in `config.toml` so
future runs skip the prompt. A `--to` flag, `MIND_ABSORB_TO`, or an existing
`absorb-to` value is used as-is and never triggers a save.

A non-TTY run with no destination configured (none of the three sources set) is
an error; there is no silent default to assume.

Set the persistent default in `~/.mind/config.toml`:

```toml
absorb-to = "~/dev/my-agent-library"
```

`~` is expanded at use. The destination must be a git repository; a path that is
not a git repo is an error.

Note: the legacy key spelling `absorb_to` (underscore) is still accepted when
reading the file. New writes and the interactive save always use `absorb-to`.

## SSH cloning

To authenticate with an SSH key instead of an https username/password, meld the
`git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so the
`owner/repo` shorthand clones over SSH. An https remote still prompts (or uses a
credential helper) as git normally does.

## Config example

A single `~/.mind/config.toml` may contain any combination of the keys:

```toml
lobes = ["~/.claude", { path = "~/.gemini/config", kinds = ["skill"] }]
ssh = true
absorb-to = "~/dev/my-agent-library"
```

## Paths

```
~/.mind/
  config.toml                   persistent settings (lobes, ssh, absorb-to)
  sources.json                  source registry (melded repos)
  manifest.json                 installed-item manifest and file registry
  sources/<host>/<owner>/<repo> clone of each melded repo
  store/<kind>/<name>/          installed copy of each item (name is effective)
  personal/                     built-in absorb destination, created on demand
  .tmp/staging/                 scratch for new copies during transactional installs
  .tmp/backup/                  previous copy held during a swap, for rollback
  .lock                         global advisory lock
```

Override the roots with `MIND_HOME` (the `~/.mind` tree) and `MIND_DEFAULT_LOBE`
(the default lobe). `CLAUDE_HOME` is a legacy alias for `MIND_DEFAULT_LOBE`;
`MIND_DEFAULT_LOBE` takes precedence when both are set.

## Concurrency

A global advisory lock (`~/.mind/.lock`) is held by every mutating command
(`meld`, `unmeld`, `learn`, `forget`, `sync`, `upgrade`, `introspect --fix`,
`config lobes add|remove`). A second concurrent `mind` invocation blocks until
the first finishes. The lock is released when the holding process exits, even on
crash, so an aborted run never wedges the next one. Read-only commands (`recall`,
`probe`, `introspect`, `config show`) take a shared lock and proceed concurrently
with each other, but never observe a writer mid-update (STO-40..43).

## Install and upgrade are transactional

A failed `learn` or `upgrade` never leaves you worse off. The new copy is built
in a staging directory first; the previous version is moved to a backup and only
dropped after the swap succeeds. A failure at any point restores the previous
version from backup (LIFE-1..4).

A prefix change (adding or removing `--namespace <prefix>` on a source) causes `upgrade`
to report `rename old -> new` and is handled the same way: the new name is
installed before the old one is removed (LIFE-14). This is normal, not an error.

For diagnosing a failed install or broken links, see
[Troubleshooting](troubleshooting.md).
