# marketplace-plugin

A single-plugin fixture for testing `mind`'s Claude plugin manifest support (MKT-1..6).

Layout mirrors a real Claude plugin:
- `.claude-plugin/plugin.json` - plugin name (`acme-tools`), version, description,
  and a `workflows` component-path override (see below)
- `skills/greet/SKILL.md` - a skill; installs as `acme-tools:greet` by default
- `agents/helper.md` - an agent; installs as `helper` (bare frontmatter name, NS-40)
- `commands/hello.md` - a command; installs as `acme-tools:hello` (MKT-18)
- `workflows/deploy.js` - a workflow; installs as `acme-tools:deploy` (WF-40). Its
  `meta.name` is the `{{ns:deploy}}` token, so it expands to the same
  `acme-tools:deploy` the harness would name a plugin workflow (WF-42). The token
  only resolves when the plugin is consumed through mind; a plugin also installed
  natively by the harness should keep a literal `meta.name`.
- `elsewhere/workflows/release.js` - a second workflow, at the path the manifest's
  `workflows` key names instead of the conventional `workflows/` directory
- `hooks/` - an unsupported component kind; `mind` reports a skipped count (MKT-4)

The manifest's `workflows` key points at `./elsewhere/workflows`, not the
conventional `workflows/` directory, and `mind` ignores it as it ignores every
other component-path override (WF-41). The two readers therefore load different
files: the real Claude harness loads `release` from the declared
`elsewhere/workflows/`, while `mind` loads only `deploy` from the conventional
`workflows/` directory it always scans. `release` is invisible to `mind`
entirely -- not installed, and not counted in the skipped-component total
either, since that count is for entries under the directory `mind` actually
scans. Setting the key is not harmless: it silently splits what the harness
offers from what `mind` manages.

Used by `Sandbox::from_example("marketplace-plugin")` in `tests/cli.rs`.
