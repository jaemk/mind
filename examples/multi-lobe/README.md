# Multi-lobe example

A lobe is a configured agent home (default `~/.claude`); when more than one is
configured, a single `learn` links the item into every one and `forget` removes
it from all of them.

## Layout

```
skills/recap/SKILL.md    skill, description in frontmatter
```

This is a minimal convention-discovered source: one skill, no `mind.toml`. The
point of the example is the lobe fan-out, not the inventory.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding. The two lobes here are plain temp directories so
the demo does not touch your real `~/.claude`:

```
cp -r examples/multi-lobe /tmp/multi-lobe
cd /tmp/multi-lobe && git init -q && git add -A && git commit -qm init

# Point the default lobe at /tmp/lobe-a, then add /tmp/lobe-b as a second home.
mkdir -p /tmp/lobe-a /tmp/lobe-b
export CLAUDE_HOME=/tmp/lobe-a
mind config show                      # lobes = ["/tmp/lobe-a"]
mind config lobes add /tmp/lobe-b
mind config lobes list                # /tmp/lobe-a and /tmp/lobe-b

mind meld /tmp/multi-lobe
mind learn recap                      # links skills/recap into both lobes
```

Confirm the skill is symlinked into both homes, each pointing at the one store
copy:

```
ls -l /tmp/lobe-a/skills/recap        # -> .../store/skill/recap
ls -l /tmp/lobe-b/skills/recap        # -> .../store/skill/recap
```

`config target` is a visible alias for `config lobes`, so
`mind config target list` prints the same homes.

### Teardown

Undo the demo in reverse order. `forget` removes the skill from both lobes:

```
mind forget recap                     # drops skills/recap from both homes
mind config lobes remove /tmp/lobe-b
mind unmeld local/tmp/multi-lobe
rm -rf /tmp/multi-lobe /tmp/lobe-a /tmp/lobe-b
unset CLAUDE_HOME
```

## Cross-harness lobes

A lobe entry in `~/.mind/config.toml` may carry a `kinds` filter. Use this
to add a non-Claude harness home that receives only the item kinds it understands
(skills and agents; not rules, which are Claude-only). The easiest path is the
`--preset` flag, which sets the canonical path and `kinds` for a known harness:

```
mind config lobes add --preset gemini
mind config lobes list
# ~/.claude          (kinds: all)
# ~/.gemini/config   (kinds: skill)
```

After adding the preset, `learn` links a skill into both `~/.claude/skills/` and
`~/.gemini/config/skills/`; rules are only linked into `~/.claude`. To detect which
harnesses are installed and choose presets interactively:

```
mind config lobes detect
```

Available preset names: `gemini`, `codex`, `universal`. See
`../../spec/harness-lobes.md` for the per-preset path and `kinds` table
(HARN-4/HARN-5).

Note: lobe config lives in `~/.mind/config.toml`, not in a source's `mind.toml`.
This example has no `mind.toml` because none is needed to demonstrate lobe fan-out.

## See also

`../../spec/storage.md` - STO-14 (the agent homes, "lobes", an item is linked
into: `$MIND_AGENT_HOMES`, else `lobes` in `config.toml`, else the claude root),
STO-15 (the default lobe written on first use).

`../../spec/lifecycle.md` - LIFE-40 (the store copy is linked into every
configured agent home; uninstall removes the recorded link in every home).

`../../spec/harness-lobes.md` - cross-harness lobe spec (HARN-1..6): the `kinds`
filter, preset table, and auto-detect behavior.

## Verified

`tests/cli.rs::example_multi_lobe_links_into_all_homes` configures two lobes,
melds this directory, and asserts `learn` links `recap` into both and `forget`
removes it from both, so the example stays correct as the code changes.
