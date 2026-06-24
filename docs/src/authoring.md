# Authoring a source

Two commands help a maintainer prepare a repo for melding: `init-source`
scaffolds and reports, `review` validates.

## init-source

Run `mind init-source` in the repo. It discovers the items the repo offers
(exactly as melding would), reports the references among them, scaffolds a
`mind.toml` if none exists, and surfaces advisories:

```
mind init-source                 # report + scaffold
mind init-source --template      # also rewrite bare sibling refs to {{ns:}}
```

It is read-only except for creating an absent `mind.toml` and, with
`--template`, editing item files. It registers nothing and touches no agent home.

## review

`mind review <target>` validates a source for publishing without changing
anything (`--fix` is the one exception). `<target>` is a melded source name, a
local path, or a repo spec; with no target it reviews the current directory.

Findings are **hard** (non-zero exit) or **advisory**:

- hard: a malformed `mind.toml`, an unknown item kind, a `min-mind-version` the
  running `mind` does not meet, or an unresolved `{{ns:}}` / `{{self}}` /
  `{{tools:}}` / `{{path:}}` token.
- advisory: a missing description, a hardcoded install path (`hardcoded-path`,
  classified by what it resolves to), a sibling tool named in prose without a
  token, a misplaced `{{ns:}}` token, a helper script duplicated across items
  (`duplicate-tooling`), and each declared install/uninstall hook (so a consumer
  sees the source will run code).

```
mind review                      # review the current dir
mind review owner/repo           # review a melded source or repo spec
mind review . --fix              # rewrite confidently-mapped findings in place
```

`--fix` rewrites a local working tree only: recognized hardcoded paths become
tokens, misplaced `{{ns:}}` tokens are un-wrapped, and bare sibling names are
templatized. Structural advisories like `duplicate-tooling` are left for you to
resolve by hand.

## Namespacing

Two sources can both ship a `review`; they would collide. A *prefix* namespaces a
source so every item installs as `<prefix>-<name>`. The effective prefix is, in
order: the consumer's `meld --as <prefix>`, the repo's `[source].prefix`, else
none.

A prefix renames items, so intra-source references must be tokens, not bare
names. Write them as `{{ns:name}}` (prose) and the path tokens
(`{{self}}` / `{{tools:name}}` / `{{path:ref}}`) for code and paths; `mind`
expands each to the effective name at install. `review` and `init-source` warn
when a source references a sibling in bare prose that a prefix would break.

See the [spec](https://github.com/jaemk/mind/tree/main/spec) for the normative
rules and [examples/namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing)
for a worked source.
