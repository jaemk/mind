# Examples

Worked examples of mind features; each subdirectory has its own README with a
"Try it" section you can run.

## Start here: [starter/](starter/)

The most common use of mind is melding an arbitrary existing repo you did not
author and did not modify. Convention discovery finds skills, agents, rules, and
tools by directory layout alone - no `mind.toml` and no source changes required.
`starter/` shows this from scratch.

## Reading order

Simplest to most advanced:

- [starter/](starter/) - zero-config: meld an unmodified repo, items found by
  convention. START HERE.
- [hello/](hello/) - the `hello-mind` hello-world skill the repo-root
  `mind.toml` exposes via `[source].roots`; what `mind meld jaemk/mind` offers.
- [namespacing/](namespacing/) - prefix namespacing and `{{ns:}}` reference
  tokens for colliding item names.
- [explicit/](explicit/) - an authoritative `[[items]]` inventory with a custom
  link target and install hooks.
- [monorepo/](monorepo/) - items under per-package subtrees, discovered via
  `[source].roots`.
- [discover/](discover/) - `[discover]` kind globs with include/exclude.
- [tooling/](tooling/) - a store-only `tool` referenced by a skill through path
  tokens.
- [hooks/](hooks/) - source build hooks that run on install, with interactive
  disclosure.
- [policy/](policy/) - a managed `policy.toml` validated by `review --policy`.
- [super-source/](super-source/) - a curated registry that references and pins
  nested sources; `dump` output.
- [marketplace-plugin/](marketplace-plugin/) - a Claude `.claude-plugin/plugin.json`
  melded as a source: skills and agents become items, unsupported components report
  a skipped count.
- [marketplace-catalog/](marketplace-catalog/) - a Claude `.claude-plugin/marketplace.json`
  catalog of in-repo plugins, each a namespaced sub-source.

Operational walkthroughs (lifecycle verbs):

- [drift/](drift/) - detect and resolve item drift: `recall` flags an out-of-date item, `upgrade` re-links it, `introspect` reports it.
- [multi-lobe/](multi-lobe/) - link items into more than one agent home (lobe) via `config lobes`.
- [absorb/](absorb/) - claim an unmanaged lobe item into a version-controlled source with `absorb`.
