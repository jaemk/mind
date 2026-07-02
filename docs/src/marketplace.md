# Claude plugin marketplaces

Claude Code ships its own plugin system: a repo can carry a
`.claude-plugin/plugin.json` (a single plugin) or a `.claude-plugin/marketplace.json`
(a catalog of plugins). `mind` reads these manifests as a discovery input, so a
repo published for Claude's plugin system melds like any other source, with no
re-packaging by its author.

```
mind meld owner/claude-plugin-repo   # a repo with .claude-plugin/plugin.json
mind probe                           # its skills and agents show up as items
mind learn <plugin-name>:<skill>     # install one, same as any source
```

## A marketplace is a source, not a sink

`mind` treats a plugin manifest strictly as a way to *discover* items. Discovered
items go to `~/.mind/store` and are symlinked into each lobe exactly like a
convention- or `mind.toml`-discovered item. `mind` never writes into Claude's
plugin cache (`~/.claude/plugins/cache/...`), its `settings.json`
`enabledPlugins`, or `known_marketplaces.json`. Consuming a marketplace does not
turn `mind` into a plugin publisher: it does not emit `.claude-plugin/` manifests
or register anything with Claude.

This keeps everything `mind` gives a source: namespacing, `{{ns:}}` reference
expansion, the broader `rule`/`tool` taxonomy, and the source-hash drift model
that `sync`, `upgrade`, and `introspect` use.

## A single plugin (`plugin.json`)

A plugin is a directory with `.claude-plugin/plugin.json` whose component
directories sit at the plugin root. Claude's layout for skills and agents is
byte-for-byte `mind`'s convention layout, so the mapping is direct:

| Plugin component | `mind` item |
|------------------|-------------|
| `skills/<name>/SKILL.md` | a `skill` |
| `agents/<name>.md` | an `agent` |
| `commands/`, `hooks/`, `.mcp.json`, LSP, monitors, themes, output-styles | not installed (no `mind` equivalent) |

A plugin has no `rules` or `tools` component, so nothing maps to those kinds.

The projection is lossy and says so: when a plugin declares components `mind`
cannot represent, `meld` prints a count of what it skipped, for example
`2 hooks, 1 mcp server not installed (no mind equivalent)`. You are never left
believing the plugin installed in full when part of it was dropped.

### Naming: the plugin name is the default prefix

A plugin's `name` becomes the default [namespace prefix](namespacing.md) for its
items, mirroring Claude's mandatory `plugin:skill` naming. So a plugin named
`acme-tools` shipping a `greet` skill installs as `acme-tools:greet`. Unlike the
native system the prefix stays optional and consumer-overridable:
`meld --namespace <p>` overrides it and `meld --namespace ''` clears it.

The prefix does **not** reach agents. An agent links under its bare frontmatter
`name` regardless of the prefix (the harness keys agents by that name, not by the
plugin scope), so two sources shipping a same-named agent collide and `mind`
surfaces the collision rather than silently resolving it. See
[namespacing](namespacing.md).

A plugin's `version` and `description` are read as metadata (the `description`
overrides frontmatter; the `version` is recorded for display). Drift and upgrade
still compare source content, not the declared version, so bumping `version`
alone is not what triggers an upgrade.

## A marketplace catalog (`marketplace.json`)

A `marketplace.json` lists several plugins, each either in-repo (a path inside the
catalog repo) or an external git source. This is the native analog of a `mind`
[curated super-source](mind-toml.md#discoversources---curated-super-source), so it
reuses that machinery: each listed plugin becomes a sub-source.

- An **in-repo plugin** is a scan root inside the catalog repo, read per the
  single-plugin rules above. Like a normal source's own items, in-repo plugins are
  offered for install on `meld`.
- An **external plugin** is a nested git source, melded and registered like any
  `[discover].sources` entry, tracking its own upstream commit. Like other nested
  sources it is registered and left available; `meld --recursive` extends install
  to the whole chain.

Each entry's `name` namespaces that plugin's items, and an entry may carry a
`version` read as metadata. Where a marketplace entry and an in-repo `plugin.json`
supply the same field, the entry wins (mirroring Claude's non-strict mode, where
the marketplace overrides the plugin manifest).

The post-meld `probe` hint and the `sync` re-walk apply to a marketplace exactly
as they do to any super-source.

## Precedence: when a plugin manifest is used

The plugin-manifest layer slots alongside the [other discovery
layers](mind-toml.md). A source's own `mind.toml` still wins:

- A `mind.toml` that declares `[[items]]` or `[discover]` kind globs is
  **authoritative** and suppresses the plugin-manifest layer, the same way it
  suppresses convention scanning. `meld` prints a note that a `.claude-plugin/`
  manifest was found and ignored.
- A `[source]`-only `mind.toml` (metadata, no item globs) composes: its metadata
  is read and the plugin manifest supplies the items.
- With no authoritative `mind.toml`, a plugin manifest (if present) is
  authoritative for the items it declares, and convention scanning is skipped for
  the paths it covers.

## Safety

Manifests are attacker-controlled content shipped by a melded repo and are held
to the same guards as `mind.toml`:

- They are parsed strictly; a malformed manifest is rejected rather than partially
  trusted.
- Every path a manifest contributes (a plugin root, an in-repo plugin path, a
  component path, a link target) is validated by the same safe-relative-path rule
  as `[[items]]` `path`/`link`: an absolute path, a leading `~`, a `..` component,
  or a NUL byte is rejected. A melded marketplace cannot read files outside its
  clone or place a symlink outside a lobe.
- An external plugin source is pinned and validated through the same path as a
  `[discover].sources` spec.
- Names and descriptions taken from a manifest have ANSI escapes and control
  characters stripped before display, so catalog-controlled text cannot inject
  terminal sequences.

## Provenance

`recall --sources` and the `probe` source view label a source whose items came
from a plugin manifest with its origin (`claude-plugin` or `claude-marketplace`),
so you can tell a native-plugin source from a convention or `mind.toml` source at
a glance.

## Authoring a plugin or marketplace

You don't need a `mind.toml` to make a Claude plugin repo meldable: the manifest
layer is enough on its own, and everything below is Claude's own format, not
something `mind` adds.

**A single plugin.** Put `.claude-plugin/plugin.json` at the repo root (or at a
scan root) with `name`, and optionally `version` and `description`:

```json
{
  "name": "acme-tools",
  "version": "1.0.0",
  "description": "Acme developer tools plugin"
}
```

Lay out `skills/<name>/SKILL.md` and `agents/<name>.md` next to it, same as any
`mind` source ([Source layout](source-layout.md)). `commands/`, `hooks/`,
`.mcp.json`, and the other Claude-only component kinds are fine to keep in the
repo for Claude users; `mind` just skips them (with a printed count) rather than
erroring, so one repo layout serves both consumers. There is no plugin-level
place for a `rule` or a `tool` - if you want `mind` users to get those, add a
`mind.toml` with `[[items]]` for them; it composes with the plugin manifest as
long as it stays metadata-only or covers different items ([Precedence](#precedence-when-a-plugin-manifest-is-used)).

Pick `name` deliberately: it becomes every consumer's default namespace prefix
(`acme-tools:greet`), so treat it like a package name - short, unique enough to
avoid colliding with other plugins someone might meld, and stable across
releases (renaming it renames every item for existing consumers).

**A marketplace catalog.** Put `.claude-plugin/marketplace.json` at the repo
root listing each plugin by `name` and `source` (a repo-relative path for an
in-repo plugin, or a git URL for an external one), plus optional `version` /
`description`:

```json
{
  "name": "Acme Marketplace",
  "plugins": [
    { "name": "alpha", "source": "./plugins/alpha", "description": "Alpha plugin" },
    { "name": "beta", "source": "./plugins/beta", "description": "Beta plugin" }
  ]
}
```

Each in-repo entry's `source` must resolve inside the repo (no `..`, no
absolute path, no `~`) or `mind` rejects it ([Safety](#safety)). An entry's
`version`/`description` override the same fields in that plugin's own
`plugin.json` if it has one, so a curator can relabel a plugin without editing
its manifest.

Validate either shape the same way you'd validate any `mind` source: run
`mind meld <path-or-repo>` against a local clone and `mind probe` to confirm the
items and namespacing came out as expected.

## Try it

Two runnable fixtures live in the repo:

- [examples/marketplace-plugin](https://github.com/jaemk/mind/tree/main/examples/marketplace-plugin)
  - a single plugin: one skill (namespaced by the plugin name), one agent (bare),
  and unsupported `commands/`/`hooks/` that report a skipped count.
- [examples/marketplace-catalog](https://github.com/jaemk/mind/tree/main/examples/marketplace-catalog)
  - a catalog listing two in-repo plugins, each with its own name and items.

The normative behavior is [spec/marketplace.md](https://github.com/jaemk/mind/blob/main/spec/marketplace.md)
(MKT-1..11).
