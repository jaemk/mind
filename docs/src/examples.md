# Examples

The [examples/](https://github.com/jaemk/mind/tree/main/examples) directory in the
repo holds runnable sources, each a small but valid `mind` source with its own
`README.md` and a test that melds it so it cannot rot. This page maps each use
case, consumer and maintainer, to the example that demonstrates it.

Every example is a directory inside the `mind` repo, not its own git repo, so copy
it out and init a repo before melding (each README shows the exact commands):

```
cp -r examples/<name> /tmp/<name>-demo
cd /tmp/<name>-demo && git init -q && git add -A && git commit -qm init
mind meld /tmp/<name>-demo
mind probe
```

## Example catalog

| Example | Shows | Key spec |
|---------|-------|----------|
| [starter](https://github.com/jaemk/mind/tree/main/examples/starter) | Convention discovery: a repo with no `mind.toml`, items found by `skills/<n>/SKILL.md`, `agents/<n>.md`, `rules/<n>.md` | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [tooling](https://github.com/jaemk/mind/tree/main/examples/tooling) | The `tool` kind plus path tokens `{{self}}`, `{{tools:name}}`, `{{path:ref}}` | [tooling.md](https://github.com/jaemk/mind/blob/main/spec/tooling.md) |
| [hooks](https://github.com/jaemk/mind/tree/main/examples/hooks) | Source `[[hooks]]`: build/install tooling at meld, tear down at unmeld, with the disclosure prompt | [install-hooks.md](https://github.com/jaemk/mind/blob/main/spec/install-hooks.md) |
| [explicit](https://github.com/jaemk/mind/tree/main/examples/explicit) | Authoritative `[[items]]` inventory: export control, custom `path`/`link`, per-item install/uninstall hooks | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [monorepo](https://github.com/jaemk/mind/tree/main/examples/monorepo) | `[source].roots`: convention discovery rooted at per-package subtrees | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing) | A prefix plus `{{ns:name}}` reference tokens that survive a rename | [namespacing.md](https://github.com/jaemk/mind/blob/main/spec/namespacing.md) |
| [super-source](https://github.com/jaemk/mind/tree/main/examples/super-source) | `[discover].sources`: a curated registry that melds other repos, optionally namespaced or auto-installed | [discovery.md](https://github.com/jaemk/mind/blob/main/spec/discovery.md) |
| [policy](https://github.com/jaemk/mind/tree/main/examples/policy) | An enterprise managed policy: trusted-source allowlist, require-pinned, lobe lock | [policy.md](https://github.com/jaemk/mind/blob/main/spec/policy.md) |

## Consumer use cases

You are installing and managing tooling that other people authored.

- **Install an item from a source.** Meld a repo, then learn an item. See the
  [Quickstart](quickstart.md) and the [starter](https://github.com/jaemk/mind/tree/main/examples/starter)
  example (`mind meld`, `mind learn`).
- **Browse and search what is available.** `mind probe` opens an interactive
  browser, or prints a listing when piped. See [Commands](commands.md#probe).
- **Resolve a name collision between two sources.** Namespace one on install with
  `mind meld <repo> --as <prefix>`, so its items install as `<prefix>:<name>`. See
  [namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing) and
  [Troubleshooting](troubleshooting.md).
- **Pull from a curated registry.** Meld a super-source to register a whole chain
  of repos at once; `meld --recursive` offers every nested source for install. See
  [super-source](https://github.com/jaemk/mind/tree/main/examples/super-source).
- **Install into more than one agent home.** Configure `lobes` in
  `~/.mind/config.toml`. See [Configuration](configuration.md#agent-homes-lobes).
- **Stay up to date.** `mind sync` refreshes every source; `mind upgrade` upgrades
  installed items and reports deltas first. See [Commands](commands.md#verbs).
- **Run under an enterprise policy.** A fixed-path managed policy restricts a
  client to trusted sources and locks related settings. See
  [policy](https://github.com/jaemk/mind/tree/main/examples/policy).

## Maintainer use cases

You are authoring a source repo for others to meld.

- **Ship items with zero config.** Use the convention layout; no `mind.toml`
  needed. See [Source layout](source-layout.md) and
  [starter](https://github.com/jaemk/mind/tree/main/examples/starter).
- **Declare an explicit inventory, or control what is exported.** List items with
  `[[items]]` to turn convention off, set custom `path`/`link`, and omit files you
  do not want offered. See [explicit](https://github.com/jaemk/mind/tree/main/examples/explicit).
- **Lay out a monorepo or subtree.** Point `[source].roots` at the package
  subtrees, or use `[discover]` kind globs for layouts roots cannot express. See
  [monorepo](https://github.com/jaemk/mind/tree/main/examples/monorepo).
- **Share helper tooling across items.** Ship a `tool` item and reference it with
  `{{tools:name}}` / `{{path:ref}}`, or bundle a script with one skill and address
  it with `{{self}}`. See [tooling](https://github.com/jaemk/mind/tree/main/examples/tooling)
  and [Source layout](source-layout.md).
- **Build or install tooling at meld.** Declare a source `[[hooks]]` install entry
  (and an uninstall entry for teardown). See
  [Install hooks](install-hooks.md) and [hooks](https://github.com/jaemk/mind/tree/main/examples/hooks).
- **Run a host side effect per item.** Declare per-item `install`/`uninstall`
  hooks (or `[[items.hooks]]`). See [explicit](https://github.com/jaemk/mind/tree/main/examples/explicit).
- **Make intra-source references survive a prefix.** Write sibling references as
  `{{ns:name}}` tokens. See [namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing)
  and [Authoring a source](authoring.md#namespacing).
- **Curate other repos into a registry.** List them in `[discover].sources`; a bare
  list keeps your own convention items too. See
  [super-source](https://github.com/jaemk/mind/tree/main/examples/super-source).
- **Validate and scaffold before publishing.** `mind init-source` scaffolds a
  `mind.toml` and reports references; `mind review` validates a source. See
  [Authoring a source](authoring.md).
