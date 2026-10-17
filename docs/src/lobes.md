# Use more than one agent

`mind` links every installed item into each configured agent home, called a
*lobe*. The default is `~/.claude`. Add Gemini CLI, Codex CLI, Windsurf, or any
directory, and items you install appear in all of them.

```
mind config lobes detect          # find the harnesses installed here and offer to add them
mind config lobes add --preset codex
mind config lobes list
```

## Add a harness with a preset

A preset knows a harness's directory and which item kinds it understands:

| preset | lobe path | kinds linked |
|--------|-----------|--------------|
| `gemini` | `~/.gemini/config` (Gemini CLI and Antigravity) | skill |
| `codex` | `~/.agents` | skill |
| `universal` | `~/.agents` | skill |
| `windsurf` | `<dir>/.windsurf`, default the current directory | skill |

```
mind config lobes add --preset gemini
# + added gemini lobe ~/.gemini/config [skill]

mind config lobes add . --preset windsurf
# + added windsurf lobe ./.windsurf [skill]
```

Adding a lobe creates its directory and links every already-installed item of
the kinds it admits, in the same command. A preset for a harness you have not
installed yet still works.

`~/.agents` is a vendor-neutral path: Codex reads it for user skills, and so
does any harness following the same convention, so one lobe serves them all.

`mind config lobes detect` reports which known harness homes exist on this
machine and offers to add them (`--yes` adds without asking, `--json` reports
only). It never changes config without one of those.

| harness | skills directory | agents directory | lobe |
|---------|------------------|------------------|------|
| Claude Code | `~/.claude/skills/<n>/SKILL.md` | `~/.claude/agents/<n>.md` | `~/.claude` |
| Gemini CLI / Antigravity | `~/.gemini/config/skills/` | - | `~/.gemini/config` |
| Codex CLI | `~/.agents/skills/` | (subagents) | `~/.agents` |
| Windsurf | `<project>/.windsurf/skills/` | - | `<project>/.windsurf` |

## Which kinds link where

Skills and agents use layouts several harnesses now share, so `mind` links them
unchanged. Other kinds are Claude-specific:

- **Rules** have no directory equivalent elsewhere (other harnesses use one
  concatenated file such as `AGENTS.md` or `GEMINI.md`), so presets never link
  them.
- **Commands** and **workflows** are not linked by any preset either.

A lobe with no kinds filter receives every kind. `~/.claude` has none, and
neither does a lobe added by bare path:

```
mind config lobes add ~/.config/some-agent      # all kinds
```

To limit a lobe to particular kinds, write it by hand in
`~/.mind/config.toml` (`config lobes add` has no flag for it):

```toml
lobes = ["~/.claude", { path = "~/.gemini/config", kinds = ["skill"] }]
```

Linking an item into a lobe whose `kinds` excludes it does nothing for that
lobe. That is not an error, and `forget`, `upgrade`, and `introspect` know not
to expect the link.

## Project lobes

A lobe can live inside a project instead of your home directory. Windsurf only
reads skills from a project's `.windsurf/skills/`, so a Windsurf lobe is always
a project lobe:

```
cd ~/projects/myapp
mind link-project
# + added windsurf lobe ./.windsurf [skill]
```

`link-project` is shorthand for `config lobes add . --preset windsurf`. Use
`--preset` for another harness or `--subdir <dir>` for an arbitrary
subdirectory (skills only), for example `--subdir .cursor`.

A managed project lobe holds symlinks into `~/.mind/store`, so add its
directory to the project's `.gitignore`. To commit the skills with the project
instead, take a snapshot: real-file copies that `mind` does not manage or
update.

```
mind link-project --snapshot
```

`link-project` never creates a lobe that links workflows, since every preset
and `--subdir` admit skills only. To link workflows into a project's
`.claude/`, add it by path:

```
mind config lobes add ./.claude              # all kinds
```

Inside a project lobe, `learn --local` and `meld --local` install into that
project's lobe only, instead of every lobe.

## Remove a lobe

```
mind config lobes remove ~/.gemini/config              # unregister; links are left in place
mind config lobes remove ./.windsurf --snapshot        # replace links with copies, then unregister
```

## Frontmatter is not translated

`mind` links files as they are. A skill or agent whose frontmatter uses
Claude-specific keys (such as the `tools:` allow-list) links into Gemini or
Codex correctly, but that harness may ignore those keys or warn about them.
Adapting frontmatter for another harness is up to the item's author.

## Per-invocation and organization overrides

- `MIND_AGENT_HOMES` (a `:`-separated list) replaces the configured lobes for
  one command. It wins over `lobes` in `config.toml`, which wins over the
  default `~/.claude`.
- A [managed policy](policy.md#lobe-lock) can lock the lobes so users cannot
  change them.
