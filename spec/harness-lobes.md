# Cross-harness lobes

Linking installed items into agent homes other than Claude's (`~/.claude`), so a
melded skill or agent is discovered by Codex CLI, Gemini CLI, and Antigravity as
well.

## Motivation

The `SKILL.md` "Agent Skills" format and the markdown-with-frontmatter subagent
format became cross-tool conventions (maintained at agentskills.io; the
instruction-file side, `AGENTS.md`, moved to the Linux Foundation's AAIF). Several
harnesses now discover skills and agents from the same on-disk shapes mind
already produces. So mind can link into those homes with no layout transform; the
only new machinery is choosing which homes get which kinds.

## Layout compatibility

mind links a skill as `<lobe>/skills/<name>` and an agent as
`<lobe>/agents/<name>.md`, where the lobe is the parent of `skills/` / `agents/`
(see storage.md STO-2). Those are exactly the cross-tool conventions:

| Harness | Skills dir | Agents dir | mind lobe (parent) |
|---------|------------|------------|--------------------|
| Claude Code | `~/.claude/skills/<n>/SKILL.md` | `~/.claude/agents/<n>.md` | `~/.claude` |
| Gemini CLI / Antigravity | `~/.gemini/config/skills/` | - | `~/.gemini/config` |
| Codex CLI | `~/.agents/skills/` (native user path; `$CODEX_HOME` toggles, not discovery) | (subagents) | `~/.agents` |
| Windsurf | `<project>/.windsurf/skills/` | - | `<project>/.windsurf` |

`~/.agents/` is the emerging vendor-neutral alias: Codex reads it as its user
skills path and the standard installer targets it.

Windsurf is project-scoped: it has no global skills home, discovering skills only
from a project's `.windsurf/skills/`. Its lobe is therefore a project subdirectory
added via `link-project` (HARN-11), not a global home; its machine marker for
detection is `~/.codeium/windsurf` (HARN-5).

`rules` (`rules/<name>.md`) have no cross-tool directory equivalent: the analog is
a single concatenated context file (`AGENTS.md` / `GEMINI.md`), not a directory of
per-rule files. Rules stay Claude-only here; an `AGENTS.md`-writer is out of scope.

## Requirements

- `HARN-1` A lobe may carry a `kinds` filter (a subset of `skill`, `agent`,
  `rule`, `tool`). Only items of a listed kind are linked into that lobe; an absent
  filter means all kinds (the current behavior, so existing config is unchanged).
  Linking an item into a lobe whose `kinds` excludes its kind is a no-op for that
  lobe, not an error.
- `HARN-2` An item's manifest `links` (STO-21) records only the lobes that
  actually received a link, i.e. lobes whose `kinds` admit the item's kind.
  `forget`, `upgrade`, and `introspect` operate on the recorded links, so a
  `kinds`-filtered lobe is never expected to hold a link it was never given.
- `HARN-3` Skills and agents are linked into a non-Claude lobe with no content
  transform: the layouts (`skills/<n>/SKILL.md`, `agents/<n>.md`) already match the
  cross-tool conventions. Rules are Claude-only and are never linked into a lobe
  added via a non-Claude preset.
- `HARN-4` `config lobes add --preset <name>` adds a lobe with the preset's parent
  path and `kinds`. Presets: `gemini` (`~/.gemini/config`, skill -- detected by the
  presence of `~/.gemini`), `codex` (`~/.agents`, skill), `universal` (`~/.agents`,
  skill), `windsurf` (`<project>/.windsurf`, skill, project-scoped -- detected by
  the presence of `~/.codeium/windsurf`). Global presets (`gemini`, `codex`,
  `universal`) resolve the lobe path under `~`; project presets (`windsurf`) resolve
  under the current working directory when no explicit base is given. A preset
  honors the `CLAUDE_HOME`-style overrides where applicable and resolves
  `~`/relative paths to absolute (STO-16). `--preset` may be combined with a
  positional base directory: the lobe path is then `base/preset.rel_path` instead
  of the default root.
- `HARN-5` Adding non-Claude lobes is opt-in by auto-detect-and-prompt: a setup
  path detects which known harness dirs exist on the machine and offers to add the
  matching presets. For global presets (`gemini`, `codex`, `universal`), the
  detected lobe is added after confirmation (or immediately with `--yes`). For
  project-scoped presets (`windsurf`), detection never auto-adds a lobe because the
  project directory is not known at detect time; instead, guidance is printed to run
  `mind link-project [--preset windsurf]` inside a project directory. The default
  lobe set is unchanged (`~/.claude` only, STO-14/STO-15); detection never mutates
  config for project-scoped presets.
- `HARN-6` mind links the skill/agent files verbatim. Frontmatter portability
  across harnesses (e.g. Gemini's `mcp_*` tool-permission wildcards vs Claude's
  `tools:` schema) is the author's responsibility; mind does not rewrite
  frontmatter to fit a target harness. This is an explicit non-goal.

- `HARN-7` After `config lobes add` (including `--preset`) or `config lobes
  detect` successfully adds one or more lobes, mind offers to backfill
  already-installed items into the new lobe(s): in interactive mode it prompts
  ("Link N installed items into the new lobe(s)?"); `--yes` backfills
  automatically without prompting; in non-interactive mode without `--yes` it
  prints a note suggesting `mind introspect --fix` and skips the backfill. The
  backfill is the same per-item link operation as `learn`, subject to the same
  `kinds` filter (HARN-1), clobber guard (LIFE-41), and manifest update (HARN-2).
  A lobe that was already present before the command runs is not backfilled (only
  newly-added lobes receive the offer); items that fail to link into a new lobe
  are reported individually and do not abort the rest of the backfill.

- `HARN-8` `introspect --fix` (CLI-91) also repairs missing lobe coverage: for
  each installed item and each configured lobe whose `kinds` admits the item's
  kind, if the expected link is absent (not present in the manifest `links` or
  not on disk), the link is created and the manifest entry updated to record it.
  This makes `introspect --fix` the single repair command for both broken
  existing links (its original role) and newly-added lobes whose items were
  installed before the lobe was configured. An item that cannot be linked into a
  lobe (e.g. clobber conflict) is reported as a finding; the fix continues with
  remaining items and lobes. `introspect` (without `--fix`) reports missing lobe
  links as drift findings alongside the existing broken-symlink findings.

- `HARN-9` When `config lobes add` (including `--preset`) or `config lobes
  detect` adds the first explicit lobe to an empty lobes config (i.e. the user
  was relying on the implicit `claude_home` default, STO-14), `claude_home` is
  automatically prepended to the saved `lobes` list before the new lobe(s) are
  appended. This preserves the implicit default as an explicit entry so that
  `agent_homes()` continues to return `claude_home` alongside the new lobes, and
  new installs continue to reach `~/.claude`. The auto-preserved entry is silent
  (no separate confirmation or output line). It is excluded from the HARN-7
  backfill offer because it was already the effective home before the command ran;
  only the newly-configured lobes receive the backfill.

- `HARN-10` `config lobes add` accepts an optional positional base directory
  alongside `--preset`; when given, the lobe path is `base/preset.rel_path`
  regardless of the preset's default scope root. `--subdir <REL>` (conflicts with
  `--preset`) resolves the lobe path as `base/<REL>` with a `[Skill]` kinds filter;
  `base` defaults to cwd when omitted. Both forms route through the unified
  `resolve_lobe` call. A missing explicit base returns `LobeBaseMissing`; no base
  and no preset and no subdir returns `LobeTargetRequired`.

- `HARN-11` `link-project [dir] [--preset <name>] [--subdir <rel>] [--snapshot]
  [--force]` is a shorthand for `config lobes add` targeting a project directory.
  `dir` defaults to cwd; `--preset` defaults to `windsurf`. For a managed
  (non-snapshot) add the same HARN-7 backfill contract applies: `--yes` backfills
  immediately, non-TTY without `--yes` prints the `introspect --fix` note. Gitignore
  guidance is printed for a newly added project lobe (the skills dir contains
  symlinks into `~/.mind/store` and should be gitignored).

- `HARN-12` `--snapshot` on `config lobes add` (or `link-project`) writes frozen
  real-file copies of installed items whose kind the resolved lobe admits, instead of
  registering a managed lobe. For each admitted item, the store directory is copied
  recursively to `<lobe>/<link_rel>` (real files, not symlinks). A colliding target
  that was not placed by mind blocks the copy unless `--force` is given. No config
  entry is written. On completion, "wrote N frozen skill(s) to <path>" is printed;
  if the target directory does not appear to be inside a git repo, an advisory note
  is printed recommending the copies be committed. Nothing-installed is a no-op note,
  not an error. `config lobes remove <path> --snapshot` detaches a managed lobe by
  converting its symlinks to frozen copies: for each manifest item link confined
  under the lobe path, the symlink is replaced with a recursive real-file copy of
  the store content, the link is stripped from the manifest, and the config entry is
  removed.

- `HARN-14` Under `--json`, a snapshot emits a machine-readable [`MutationResult`]
  (CLI-153, schema 1) instead of the prose lines. `config lobes add --snapshot`
  (and `link-project --snapshot`) emits `action = "lobe-add"`, `target` = the lobe
  path, `outcome = "snapshot"` when at least one item was frozen (else `"no-op"`),
  `count` = the number of items frozen, and `installed` = their `kind:name` keys
  (sorted). `config lobes remove <path> --snapshot` emits `action = "lobe-remove"`,
  `outcome = "detached"`, and `count` = the number of links frozen (a plain remove
  keeps `outcome = "removed"` with no `count`). The advisory git/gitignore notes are
  suppressed under `--json`.

- `HARN-13` `introspect` reports a configured lobe whose parent directory does not
  exist (`!lobe.reachable()`) as a `vanished-lobe` finding. `introspect --fix`
  additionally prunes the entry from `config.lobes` and strips its links from every
  manifest item's `links` list. Vanished lobes are checked before the missing-link
  and missing-lobe-link loops so that a removed project directory does not produce
  spurious repair errors for the links that lived inside it.

## Documentation map

Places that explain lobes / agent homes and reference this feature.

- `spec/storage.md` (STO-2, STO-14): note the `kinds` filter on a lobe and that
  the skill/agent layouts double as the cross-tool conventions. `spec/cli.md`: the
  `config lobes add --preset` flag and the auto-detect-and-prompt behavior, under
  the existing `config lobes` section (CLI-110..113).
- `docs/src/configuration.md` "Agent homes (lobes)": the canonical user-facing
  explanation. Add the preset list with the per-harness path table, the `kinds`
  filter, and the auto-detect prompt. This is the primary doc; others point here.
- `docs/src/commands.md`: extend the `config lobes` row with `--preset` and the
  detected-home prompt.
- `docs/src/quickstart.md` and `README.md`: the "install one into each agent home"
  lines should mention that homes can be Gemini/Codex/Windsurf/Antigravity, not just Claude.
- `docs/src/introduction.md` (mental model) and `docs/src/troubleshooting.md`: a
  note on cross-harness homes and the rules-are-Claude-only / frontmatter-portability
  caveats (HARN-3, HARN-6).
- `docs/landing/index.html`: the landing page, if lobes/agent homes are described
  there.
- `examples/multi-lobe/`: extend (or add a sibling example) to show a non-Claude
  preset lobe with a `kinds` filter.
- Generated artifacts (regenerate, do not hand-edit): the man page and shell
  completions (CLI-120, CLI-121) from the clap surface in `src/cli.rs`; the mdBook
  under `docs/book/` from `docs/src/`. `CHANGELOG.md` gets an entry at release.
- Project meta: `CLAUDE.md` and the root doc comment in `src/paths.rs` describe
  `Paths::agent_homes` and the default `~/.claude`; update both to mention the
  `kinds` filter and presets.
