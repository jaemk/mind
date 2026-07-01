# marketplace-plugin

A single-plugin fixture for testing `mind`'s Claude plugin manifest support (MKT-1..6).

Layout mirrors a real Claude plugin:
- `.claude-plugin/plugin.json` - plugin name (`acme-tools`), version, description
- `skills/greet/SKILL.md` - a skill; installs as `acme-tools:greet` by default
- `agents/helper.md` - an agent; installs as `helper` (bare frontmatter name, NS-40)
- `commands/` and `hooks/` - unsupported component kinds; `mind` reports a skipped count (MKT-4)

Used by `Sandbox::from_example("marketplace-plugin")` in `tests/cli.rs`.
