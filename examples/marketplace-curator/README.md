# marketplace-curator

A repo that is both a Claude plugin marketplace and a `mind` curator: a
`.claude-plugin/marketplace.json` defines its own items, and a co-present
`mind.toml` `[discover].sources` curates a chain of other sources. Fixture for
MKT-15/MKT-16 (marketplace + curator compose).

Layout:
- `.claude-plugin/marketplace.json` - lists one in-repo plugin, `toolkit`
- `plugins/toolkit/` - the plugin: skill `toolkit:format`, agent `reviewer`
- `mind.toml` - `[source]` metadata plus a `[discover].sources` curator chain
  (local sibling examples: `../starter`, `../explicit`)

## Compose model

The manifest defines this repo's own items (the `toolkit` plugin). The
`mind.toml` `[discover].sources` layers a curated super-source on top: to the
native Claude plugin system this is a standard marketplace, and when melded with
`mind` it is additionally a curator. `[discover].sources` is a curator directive
over other repos, not an own-item directive, so it does not suppress the manifest
(MKT-16).

To instead define this repo's own items yourself and suppress the manifest's
own-item layer, declare an own-item directive in `mind.toml`: `[[items]]`, a
`[discover]` item glob, `[source].roots`, or `[source].flat-skills` (MKT-15).
Convention discovery (honoring roots/flat-skills) then supplies the items and
`meld` prints a note that the manifest's plugin components are ignored.

Melding this example over the network is avoided by using local sibling paths in
`[discover].sources`; a real curator lists remote specs there. Used by
`Sandbox::from_example("marketplace-curator")` in `tests/cli.rs`.
