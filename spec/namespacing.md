# Namespacing

Prefixing a source so its items do not collide with same-named items from other
sources, and keeping intra-source references resolvable across a prefix.

## Effective prefix

- `NS-1` A source's effective prefix is, in order: its consumer `alias` (from
  `meld --as`), else its `mind.toml` `[source].prefix`, else none.
- `NS-2` With prefix `p`, an item's effective name is `p-<bare>`; with no prefix
  it is the bare name. Prefixing applies to every item of every kind in the
  source.
- `NS-3` The prefix is applied at install time, not stored in the catalog. The
  catalog and the stable identity `(source, kind, bare_name)` are prefix-free.

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
