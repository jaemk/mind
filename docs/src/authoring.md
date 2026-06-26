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

## Resources and tooling

Most sources keep an item's resources next to the item and never think about any
of this. Two patterns cover nearly everything:

- **Install to a known location.** Tooling shared across items, or anything with a
  build step, is best handled by an install hook: declare a `[[hooks]]` install
  entry to run your install script, which puts the tooling wherever you like (a
  fixed path under the user's home, a venv, a PATH entry), and have your items call
  it there. The source "onboards" its build once and the items just use it.
- **Bundle with the item.** A script a single skill uses lives in that skill's
  own directory and is addressed with `{{self}}` (e.g. `{{self}}/resources/pr.py`).
  It ships and installs with the skill; nothing else is needed.
  ```bash
  skills/github/SKILL.md
  skills/github/resources/pr.py   # referenced as {{self}}/resources/pr.py
  ```
- The `tool` item kind and the `{{tools:name}}` / `{{path:ref}}` tokens are a third
  option for sharing a helper through `mind`'s store. This method centralizes a shared
  tool from your repo's `tools/` directory into mind's store.
  These tools are expected to each live in their own directory, e.g.
  - `tools/my-tool/my-tool.sh`
  - `tools/my-tool/TOOL.md`
    ```
    ---
    description: My tool
    bin: my-tool.sh
    ---
    Shared project tool. Skills and agents invoke it via {{tools:my-tool}}
    ```

`mind review`'s `duplicate-tooling`, `bare-tool-reference`, and `hardcoded-path`
findings are advisory, and bundling or an install hook are equally valid.

## Namespacing

Most sources give their items unique, descriptive names and are never prefixed, so
this does not come up. A *prefix* exists only for the collision case: two sources
that both ship a `review` would land at the same path, so a prefix namespaces one
(`<prefix>-<name>`). The effective prefix is, in order: the consumer's
`meld --as <prefix>`, the repo's `[source].prefix`, else none.

A prefix renames items, so if a prefixed source's items reference each other by
name, those references must be tokens: `{{ns:name}}` in prose, and the path tokens
(`{{self}}` / `{{tools:name}}` / `{{path:ref}}`) for code and paths. `mind`
expands each at install. `review` and `init-source` warn (advisory) when a source
that is being prefixed references a sibling in bare prose. An unprefixed source,
or one with no intra-source references, needs none of this.

See the [spec](https://github.com/jaemk/mind/tree/main/spec) for the normative
rules and [examples/namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing)
for a worked source.
