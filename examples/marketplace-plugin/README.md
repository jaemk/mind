# marketplace-plugin

A single-plugin fixture for testing `mind`'s Claude plugin manifest support (MKT-1..6).

Layout mirrors a real Claude plugin:
- `.claude-plugin/plugin.json` - plugin name (`acme-tools`), version, description
- `skills/greet/SKILL.md` - a skill; installs as `acme-tools:greet` by default
- `agents/helper.md` - an agent; installs as `helper` (bare frontmatter name, NS-40)
- `commands/hello.md` - a command; installs as `acme-tools:hello` (MKT-18)
- `workflows/deploy.js` - a workflow; installs as `acme-tools:deploy` (WF-40). Its
  `meta.name` is the `{{ns:deploy}}` token, so it expands to the same
  `acme-tools:deploy` the harness would name a plugin workflow (WF-42).
- `hooks/` - an unsupported component kind; `mind` reports a skipped count (MKT-4)

The manifest's `workflows` key points somewhere else on purpose: it is a
component-path override `mind` ignores, as it ignores every other one, so the
convention directory is still what is scanned (WF-41).

Used by `Sandbox::from_example("marketplace-plugin")` in `tests/cli.rs`.
