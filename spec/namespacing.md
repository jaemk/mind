# Namespacing

Prefixing a source so its items do not collide with same-named items from other
sources, and keeping intra-source references resolvable across a prefix.

## Overview

Two melded sources can each ship an item of the same name (both a `review`),
which would collide at the same install path. A prefix namespaces a source so
every item from it installs under `<prefix>:<name>`, keeping the two distinct.
The separator is a colon, matching the harness's own namespace convention
(`plugin:skill`). Because mind already uses `:` to separate a kind from a name in
an item ref (`skill:review`), a prefix may not be a reserved kind word and the
ref parser disambiguates the two readings (NS-25, NS-26).

The prefix is an install-time transform, not part of an item's identity. The
catalog holds bare names; the prefix is applied when an item is installed, so its
effective name, store path, symlink, and ref all use it. An item's stable
identity stays `(source, kind, bare_name)`, so a later prefix change reads as a
rename of the same item rather than a new one (see lifecycle.md).

Prefixing breaks references between items in the same source: the Claude harness
resolves a skill by its directory name, so "the dev skill" no longer resolves once
`dev` installs as `jk:dev`. Authors write such references as `{{ns:name}}` tokens
instead. Agents are the exception. The harness keys an agent by its frontmatter
`name`, not its filename, so a prefix on the link does not change the resolved
name and mind does not prefix an agent's harness identity at all (it links under
the bare name and detects collisions; see "Agent identity" below). The token rules
here therefore govern skill references; an agent reference stays bare. On install, each token is expanded to the
referent's effective name (bare when unprefixed, `<prefix>:name` when prefixed)
and validated against the source's siblings. Expansion happens in the staging
copy during the transactional install, so a bad reference fails before the live
install is touched. The recorded content hash is of the source (token) form, not
the expanded copy, so drift detection compares source with source.

In practice most sources give their items unique, descriptive names and are never
prefixed, so the token machinery below does not come up: an unprefixed source's
references resolve as written. Tokens matter only when a source is namespaced (to
avoid a collision) and its items reference each other by name. They are a tool for
that case, not a general requirement: a source with no intra-source references, or
one that is never prefixed, needs none.

A source whose items reference siblings in bare prose (no token) breaks under a
prefix. mind does not guess and rewrite prose, since sibling names are often
common words; instead `meld` warns when it sees a likely unguarded reference and
leaves the fix to the author. The warning is advisory and only fires under a
prefix.

The rest of this document states these rules normatively.

## Effective prefix

- `NS-1` A source's effective prefix is, in order: its consumer `alias` (from
  `meld --namespace`, CLI-159), else its `mind.toml` `[source].prefix`, else none.
- `NS-2` With prefix `p`, an item's effective name is `p:<bare>`; with no prefix
  it is the bare name. Prefixing applies to every item of every kind in the
  source.
- `NS-3` The prefix is applied at install time, not stored in the catalog. The
  catalog and the stable identity `(source, kind, bare_name)` are prefix-free.
- `NS-25` A prefix may not be a reserved kind word (`skill`, `agent`, `rule`,
  `tool`). `meld --as <prefix>` and a `mind.toml` `[source].prefix` declaring
  such a value are rejected, since the resulting `skill:foo` effective name would
  be indistinguishable from a kind-qualified ref (NS-26).
- `NS-26` An item ref's pre-colon token is read as a kind only when it is a
  reserved kind word; otherwise the whole ref is an effective name. So
  `jk:review` resolves by effective name while `skill:review` stays
  kind-qualified. This keeps prefixed effective names usable as refs in
  `forget`/`recall`/`upgrade` despite the shared `:` separator.
- `NS-27` An item installed under the former `-` separator keeps its stable
  identity `(source, kind, bare_name)`, so after the switch to `:` it is matched
  by identity and `upgrade`/`introspect` report the move from `p-<bare>` to
  `p:<bare>` as a rename (lifecycle.md), not as an orphan plus a new item.

## Agent identity

The harness keys an agent differently from a skill, which bounds how a prefix can
apply. A skill is keyed by its directory name (the frontmatter `name` is
display-only), so prefixing a skill's directory and link changes the name the
harness resolves. An agent is keyed by the `name` field in its frontmatter, not
its filename, so renaming the link to `<prefix>:<name>` does not change the
resolved name; only rewriting the frontmatter `name` would, and that would break
every reference to the agent not written as a `{{ns:}}` token. mind does not do
that rewrite, so a prefix cannot transparently namespace an agent.

- `NS-40` mind does not apply a source's prefix to an agent's harness identity. An
  agent links into each agent home under its bare frontmatter `name` even when the
  source has a prefix in effect. The prefix still applies to the agent's store path
  and manifest key, so mind's stable identity `(source, kind, bare_name)` and the
  store stay collision-free and a prefix change is still a rename (lifecycle.md);
  only the harness-visible link name is bare. This narrows NS-2 for the agent kind's
  link target. Skills (directory-keyed) and tools (store-only, token-referenced)
  are unaffected: a skill's prefix applies to its directory and link as before.
- `NS-41` Because agents link under their bare name (NS-40), two melded sources
  that each ship an agent with the same frontmatter `name` resolve to the same
  agent-home link regardless of their prefixes. mind detects this rather than
  silently repointing the link: installing an agent whose bare name already maps to
  an installed agent from a different source is reported as a collision and the user
  is prompted to resolve it (keep one, or forget the other). The collision is also
  surfaced at `meld` of a source that would introduce it. A prefix does not avert
  it (the prefix does not reach the agent link), so two same-named agents from
  different sources cannot both be active; this is an inherent limit of the
  harness's global agent namespace, made explicit instead of mishandled.
- `NS-42` An agent's effective harness name is always bare (NS-40), so a bare prose
  reference to a sibling agent resolves correctly with or without a prefix. The
  unguarded-reference warning (NS-20) therefore does not fire for a reference whose
  referent is a sibling agent (it would be a false positive); the warning and the
  `{{ns:}}` token machinery apply to references whose referent is prefixed (a
  sibling skill). A `{{ns:}}` token naming a sibling agent still expands (to the
  bare name) and is not an error, so an over-cautious author who tokenizes an agent
  reference is not penalized.

## Namespace mutability

The ID below extends the namespacing rules above. Namespacing stays opt-in: with
no `--namespace` (NS-1, CLI-159) and no `[source].prefix`, a source's items
install under their bare names (NS-2).

- `NS-30` A source's namespace (set by `--namespace`, NS-1/CLI-159) is mutable
  only while none of its items are installed: a `--link-only` meld (CLI-23), or a
  super-source whose nested sources are registered but not installed. Re-melding
  such a source with a different `--namespace` updates the persisted alias. Once
  any of the source's items are installed the namespace is locked: changing it
  requires forgetting the source's installed items first. A re-meld that would
  change the namespace of a source with installed items is refused with guidance to
  uninstall first (CLI-161), not applied as an in-place rename. This supersedes
  CLI-13's rename of installed items. It is distinct from the one-time `-`->`:`
  separator migration (NS-27), which `upgrade` applies to already-installed items
  without a namespace change.

## Reference tokens

Items reference each other by name, and the Claude harness resolves those names
at runtime. Prefixing changes installed names, so references must be rewritten.

- `NS-10` An intra-source reference is written `{{ns:name}}`, where `name` is a
  sibling's bare name.
- `NS-11` At install, each `{{ns:name}}` token in the item's text files is
  expanded to the effective name: `name` when unprefixed, `p:name` when prefixed.
- `NS-12` A token whose `name` is not a sibling in the same source is an error
  (`BadReference`), naming the referencing item and the bad referent.
- `NS-13` Content with no `{{ns:` tokens is copied unchanged. Non-text (non-UTF-8)
  files are not scanned.
- `NS-14` Expansion runs whether or not a prefix is in effect, so a token-using
  source installs correctly with or without a namespace.
- `NS-15` Token edge cases: whitespace inside a token (`{{ns: name }}`) is
  trimmed before the sibling lookup; an unterminated token (`{{ns:` with no
  closing `}}`) is left verbatim rather than treated as a reference or an error.

## Unguarded-reference warning

- `NS-20` When melding a source with a prefix in effect, every text file of each
  item (the whole skill directory, or the agent/rule file) is scanned for sibling
  names that appear in bare prose (outside any `{{ns:}}` token), matching the
  breadth of install-time expansion; each item with such a reference is reported
  as a warning.
- `NS-21` Matching is whole-word (alphanumeric, `_`, and `-` are word
  characters); an item's own name is not reported against itself.
- `NS-22` The warning is advisory and heuristic: it does not fail `meld` and does
  not rewrite anything.
- `NS-23` No warning is emitted when no prefix is in effect, since bare references
  are then correct.

## Prose-only scope

- `NS-24` A `{{ns:name}}` token is a prose name reference: it expands to the
  referent's effective name (NS-11), which is correct only where an item name
  belongs. It is *misplaced* in a non-prose context -- inside a fenced code block
  or an inline code span, adjacent to a path separator (`/` or `~`), or in a
  frontmatter structured field such as `name:` -- where name-substitution yields
  broken code, a broken path, or (under a prefix) a wrong identity. Code and paths
  reference an item by path token instead (`{{self}}`, `{{tools:}}`, `{{path:}}`;
  tooling.md), never by `{{ns:}}`. `review` detects misplaced tokens (CLI-139) and
  `init-source --template` does not create them (INIT-5).
