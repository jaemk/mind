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
resolves agents and skills by the name in the text at runtime, so "the dev agent"
no longer resolves once `dev` installs as `jk:dev`. Authors write such references
as `{{ns:name}}` tokens instead. On install, each token is expanded to the
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
  `meld --as`), else its `mind.toml` `[source].prefix`, else none.
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

## Reference tokens

Items reference each other by name, and the Claude harness resolves those names
at runtime. Prefixing changes installed names, so references must be rewritten.

- `NS-10` An intra-source reference is written `{{ns:name}}`, where `name` is a
  sibling's bare name.
- `NS-11` At install, each `{{ns:name}}` token in the item's text files is
  expanded to the effective name: `name` when unprefixed, `p-name` when prefixed.
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
