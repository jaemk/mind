# Claude plugin marketplaces

How `mind` consumes Claude Code's native plugin manifests - a
`.claude-plugin/marketplace.json` (a marketplace catalog) and a
`.claude-plugin/plugin.json` (a single plugin) - as a discovery input, so a repo
published for Claude's built-in plugin system can be melded without the author
re-packaging it for `mind`.

## Position: a marketplace is a source, not a sink

The native plugin system is itself an own-store, copy-based installer: an
installed plugin is copied into a versioned cache (`~/.claude/plugins/cache/...`)
and recorded in Claude's `settings.json` / `known_marketplaces.json`. It is a
parallel install model, not a canonical set of `~/.claude/skills` files that
`mind` could conform to. Conforming would buy neither a shared on-disk format nor
cleaner uninstall, while it would lose `mind`'s namespacing, `{{ns:}}` reference
expansion, broader item taxonomy (`rule`/`tool`), and source-hash drift model,
and couple installed state to one vendor's evolving format.

So `mind` treats a plugin manifest strictly as a *discovery input* that feeds the
existing catalog -> store -> symlink pipeline. It is the same decision the
dominant package managers make (Homebrew kegs, Nix store, pipx venvs, Stow): keep
an independent store and link into the host's discovery location.

- `MKT-1` `mind` recognizes a Claude plugin source by the presence of a
  `.claude-plugin/plugin.json` (a single plugin, rooted at the repo or a scan
  root) or a `.claude-plugin/marketplace.json` (a catalog of plugins) in a melded
  repo. These manifests are read as an additional discovery layer that produces
  ordinary catalog items (bare names, install-time prefix and token transforms
  unchanged). The install path is unchanged: items go to `~/.mind/store` and are
  symlinked into each lobe exactly as a convention- or `mind.toml`-discovered item
  is (storage.md, lifecycle.md). `mind` never writes into Claude's plugin cache,
  `settings.json` `enabledPlugins`, or `known_marketplaces.json`.

- `MKT-2` Precedence relative to the other discovery layers (DSC-3): a source's
  own `mind.toml` that declares `[[items]]` or `[discover]` item globs is
  authoritative and suppresses the plugin-manifest layer for that source (a note
  is printed that a `.claude-plugin/` manifest was found and ignored), the same
  way it suppresses convention discovery. When no authoritative `mind.toml` is
  present, a plugin manifest, if present, is authoritative for the items it
  declares and convention discovery (DSC-1) is skipped for the paths the manifest
  covers. A `[source]`-only `mind.toml` (metadata, no item globs) still composes:
  its `[source]` metadata is read and the plugin manifest supplies the items.

## A single plugin (`plugin.json`)

A plugin is a directory containing `.claude-plugin/plugin.json` whose component
directories sit at the plugin root. Claude's plugin component layout for skills
and agents is byte-for-byte `mind`'s convention layout, so the mapping is direct.

- `MKT-3` A `.claude-plugin/plugin.json` defines a plugin rooted at the manifest's
  parent directory. Its components map to `mind` item kinds by the convention rules
  (DSC-10..12) applied at the plugin root: `skills/<name>/SKILL.md` -> a `skill`,
  `agents/<name>.md` -> an `agent`. Component kinds the native format defines that
  have no `mind` equivalent - `commands/`, `hooks/`, `.mcp.json`, LSP, monitors,
  themes, output-styles - are not installed. A plugin has no `rules` or `tools`
  component, so nothing maps to those `mind` kinds from a plugin.

- `MKT-4` When a plugin declares unsupported components (MKT-3), `mind` reports a
  count of skipped components on meld (e.g. `2 hooks, 1 mcp server not installed
  (no mind equivalent)`), so the user is not misled into believing the plugin is
  fully represented. The projection is intentionally lossy and stated as such; it
  is never a silent drop.

- `MKT-5` A plugin's `name` (from `plugin.json`) is the default effective prefix
  (namespacing.md) for that plugin's items, mirroring Claude's mandatory
  `plugin:skill` namespacing. Unlike the native system the prefix stays optional
  and consumer-overridable: `meld --namespace <p>` (CLI-159) overrides it, and a
  consumer may clear it. The prefix keeps two marketplaces that each ship a `review`
  skill from colliding. It does not reach agents: per NS-40 an agent links under its
  bare frontmatter `name` regardless of the prefix, and a same-named agent from
  another source is a detected collision (NS-41), since a flat-installed agent is
  keyed by the harness on its frontmatter `name`, not on the plugin scope it carried
  in Claude's plugin system. So melding a marketplace namespaces its skills and
  flattens its agents to bare names, with collisions surfaced rather than silently
  resolved.

- `MKT-6` A plugin's `version` and `description` are read as metadata
  (`description` overrides frontmatter per the DSC-32 precedence; the declared
  `version` is recorded for display). Drift and upgrade continue to compare source
  content hash and commit (lifecycle.md, namespacing.md drift note), not the
  declared plugin version: the manifest version is informational, not the upgrade
  trigger.

## A marketplace catalog (`marketplace.json`)

A marketplace lists multiple plugins, each either in-repo (a path within the
catalog repo) or an external git source. This is the native analog of a `mind`
curated super-source, so it reuses that machinery.

- `MKT-7` A `.claude-plugin/marketplace.json` is consumed as a curated
  super-source (the `[discover].sources` model, DSC-38). Each listed plugin becomes
  a discoverable sub-source: an in-repo plugin is a scan root within the catalog
  repo (DSC-50) read per MKT-3; an external plugin source is a nested source melded
  and registered like any `[discover].sources` entry, tracking its own upstream
  commit. Registration and install defaults follow the super-source rules: the
  catalog repo's own in-repo plugins are offered for install on meld like a normal
  source's items (CLI-23), external nested plugins are registered and left
  available (DSC-54), and `--recursive` (DSC-55) extends install to the whole
  chain. The post-meld `probe` hint (DSC-56) and `sync` re-walk (DSC-57) apply.

- `MKT-8` A marketplace entry's per-plugin `name` namespaces that plugin's items
  per MKT-5; a marketplace entry may carry a declared `version` read as metadata
  per MKT-6. Where a marketplace entry and an in-repo `plugin.json` both supply a
  field, the entry is authoritative (it mirrors Claude's `"strict": false` mode
  where the marketplace overrides the plugin manifest).

## Safety

The manifests are attacker-controlled content shipped by a melded repo and are
held to the same guards as `mind.toml`.

- `MKT-9` Plugin and marketplace manifests are parsed strictly: unknown required
  shapes are an error (`MindToml`-class), and a manifest is rejected rather than
  partially trusted. Every path a manifest contributes - a plugin root, an in-repo
  plugin path, a component path, a link target - is validated by the same
  safe-relative-path rule as `[[items]]` `path`/`link` (DSC-71..73): a value that
  is absolute, begins with `~`, contains a `..` component, or contains a NUL byte
  is rejected, so a melded marketplace cannot read host files outside its clone or
  place a symlink outside a lobe. An external plugin `source` is parsed and pinned
  through the same path as a `[discover].sources` spec, including pin-value
  validation (DSC-66). Names and descriptions taken from a manifest have ANSI
  escapes and control characters stripped before display (DSC-69 rule), preventing
  terminal injection from catalog-controlled text.

## Provenance

- `MKT-10` `recall --sources` and the `probe` source view label a source whose
  items came from a plugin manifest with its manifest origin (`claude-plugin` or
  `claude-marketplace`), so the provenance of a melded source is visible and a user
  can tell a native-plugin source from a convention or `mind.toml` source.

## Non-goals

- `MKT-11` Consuming a marketplace does not make `mind` a Claude plugin publisher:
  `mind` does not emit `.claude-plugin/` manifests, register a marketplace with
  Claude, or install into Claude's plugin cache. Producing native plugin output
  (a translate-on-export step, the chezmoi/skillkit analog) is explicitly out of
  scope for this feature and, if ever added, belongs with `dump`-style export, kept
  separate from the authoritative store - never as the install model.
