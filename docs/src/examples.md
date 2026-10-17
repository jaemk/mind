# Example sources

The [examples/](https://github.com/jaemk/mind/tree/main/examples) directory in the
repo holds runnable sources, each a small but valid `mind` source with its own
`README.md` and a test that melds it so it cannot rot. This page maps each use
case, consumer and maintainer, to the example that demonstrates it.

Every example is a directory inside the `mind` repo, not its own git repo, so
clone the repo, copy the example out, and init a repo before melding (each
README shows the exact commands):

```
git clone --depth 1 https://github.com/jaemk/mind /tmp/mind-repo
cp -r /tmp/mind-repo/examples/<name> /tmp/<name>-demo
cd /tmp/<name>-demo && git init -q && git add -A && git commit -qm init
mind meld /tmp/<name>-demo
mind probe
```

## Example catalog

| Example | Shows | Key spec |
|---------|-------|----------|
| [starter](https://github.com/jaemk/mind/tree/main/examples/starter) | Convention discovery: a repo with no `mind.toml`, items found by `skills/<n>/SKILL.md`, `agents/<n>.md`, `rules/<n>.md`, `commands/<n>.md`, `workflows/<n>.js` | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [hello](https://github.com/jaemk/mind/tree/main/examples/hello) | The repo-root `mind.toml`'s `[source].roots` scanning `examples/hello`; what `mind meld jaemk/mind` offers | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [tooling](https://github.com/jaemk/mind/tree/main/examples/tooling) | The `tool` kind plus path tokens `{{self}}`, `{{tools:name}}`, `{{path:ref}}` | [tooling.md](https://github.com/jaemk/mind/blob/main/spec/tooling.md) |
| [hooks](https://github.com/jaemk/mind/tree/main/examples/hooks) | Source `[[hooks]]`: build/install tooling at meld, tear down at unmeld, with the disclosure prompt | [install-hooks.md](https://github.com/jaemk/mind/blob/main/spec/install-hooks.md) |
| [explicit](https://github.com/jaemk/mind/tree/main/examples/explicit) | Authoritative `[[items]]` inventory: export control, custom `path`/`link`, per-item install/uninstall hooks | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [monorepo](https://github.com/jaemk/mind/tree/main/examples/monorepo) | `[source].roots`: convention discovery rooted at per-package subtrees | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [discover](https://github.com/jaemk/mind/tree/main/examples/discover) | `[discover]` kind globs with per-kind `include`/`exclude` lists, for layouts `roots` cannot express | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing) | A prefix plus `{{ns:name}}` reference tokens that survive a rename | [namespacing.md](https://github.com/jaemk/mind/blob/main/spec/namespacing.md) |
| [super-source](https://github.com/jaemk/mind/tree/main/examples/super-source) | `[discover].sources`: a curated registry that melds other repos, optionally namespaced or auto-installed, plus `mind curate` reconciling it after the list changes | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md), [curate.md](https://github.com/jaemk/mind/blob/main/spec/curate.md) |
| [marketplace-plugin](https://github.com/jaemk/mind/tree/main/examples/marketplace-plugin) | A Claude `.claude-plugin/plugin.json`: skills, agents, commands, and workflows mapped to items, unsupported components reported, a `workflows` component-path override ignored | [marketplace.md](https://github.com/jaemk/mind/blob/main/spec/marketplace.md) |
| [marketplace-catalog](https://github.com/jaemk/mind/tree/main/examples/marketplace-catalog) | A Claude `.claude-plugin/marketplace.json`: a catalog of in-repo plugins, each a namespaced sub-source | [marketplace.md](https://github.com/jaemk/mind/blob/main/spec/marketplace.md) |
| [marketplace-curator](https://github.com/jaemk/mind/tree/main/examples/marketplace-curator) | A repo that is both a Claude marketplace and a mind curator: `[discover].sources` composes with the plugin manifest instead of suppressing it | [marketplace.md](https://github.com/jaemk/mind/blob/main/spec/marketplace.md) |
| [policy](https://github.com/jaemk/mind/tree/main/examples/policy) | An enterprise managed policy: trusted-source allowlist, require-pinned, lobe lock | [policy.md](https://github.com/jaemk/mind/blob/main/spec/policy.md) |
| [drift](https://github.com/jaemk/mind/tree/main/examples/drift) | Item drift end to end: `recall`/`introspect` flag an out-of-date item, `upgrade` re-links it | [lifecycle.md](https://github.com/jaemk/mind/blob/main/spec/lifecycle.md) |
| [multi-lobe](https://github.com/jaemk/mind/tree/main/examples/multi-lobe) | Linking one item into more than one agent home (lobe) via `config lobes` | [storage.md](https://github.com/jaemk/mind/blob/main/spec/storage.md) |
| [absorb](https://github.com/jaemk/mind/tree/main/examples/absorb) | Claiming an unmanaged lobe item into a version-controlled source with `absorb` | [absorb.md](https://github.com/jaemk/mind/blob/main/spec/absorb.md) |

## Which example for which task

| task | example | guide |
|------|---------|-------|
| install from a plain repo | starter | [Find and install items](installing.md) |
| resolve a name collision | namespacing | [Fix name collisions](collisions.md) |
| follow a curated list | super-source | [Follow a curated collection](curate.md) |
| meld a Claude plugin or marketplace | marketplace-plugin, marketplace-catalog | [Claude plugins and marketplaces](marketplace.md) |
| link into more than one agent home | multi-lobe | [Use more than one agent](lobes.md) |
| see drift and upgrade | drift | [Keep everything up to date](updating.md) |
| adopt a hand-placed item | absorb | [Adopt items you placed by hand](unmanaged.md) |
| run under a managed policy | policy | [Managed policy](policy.md) |
| export only some files | explicit | [The mind.toml file](mind-toml.md#items---explicit-inventory-authoritative) |
| lay out a monorepo | monorepo, discover | [The mind.toml file](mind-toml.md#discover---glob-based-discovery) |
| share a script between skills | tooling | [Share helper scripts](tooling.md) |
| build tooling at meld | hooks | [Run setup with hooks](install-hooks.md) |
| curate other repos | super-source, marketplace-curator | [Curate other repos](curating.md) |
