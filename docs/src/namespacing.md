# Namespacing

Two melded sources can each ship an item of the same name (both a `review`).
Without a prefix they would land at the same install path. A prefix namespaces a
source so every item from it installs as `<prefix>:<name>`: the effective name,
store path, symlink, and the name `mind` uses in the ref all carry the prefix.
Most sources give their items unique, descriptive names and never need a prefix;
this page covers the case where a prefix is needed.

## Setting a prefix (NS-1, NS-2)

Two ways, in precedence order:

1. **Consumer-side**: `meld --as <prefix>`. Stored as the source alias and takes
   priority over anything the repo declares. `meld --as ''` removes a prefix.
2. **Repo-side**: `[source].prefix` in `mind.toml`.

```toml
[source]
prefix = "jk"
```

With prefix `jk`, every item in the source installs as `jk:<bare-name>`. The
catalog and the item's stable identity keep the bare name; the prefix is applied
at install time (NS-3), so a later change reads as a rename of the same item
rather than an orphan plus a new item.

## Why `{{ns:name}}` tokens exist (NS-10, NS-11)

The Claude harness resolves agents and skills at runtime by the name in the text.
A plain-prose reference like "delegate to the dev agent" breaks once `dev`
installs as `jk:dev`. Authors write intra-source references as `{{ns:name}}`
instead, where `name` is the sibling's bare name.

At install time, mind expands each token to the referent's effective name:

| source installed as | `{{ns:dev}}` expands to |
|---------------------|-------------------------|
| unprefixed          | `dev`                   |
| `--as jk`           | `jk:dev`                |

Expansion runs whether or not a prefix is in effect (NS-14), so a token-using
source installs correctly in both cases.

### Minimal example

A source with two items, `lead` (agent) and `dev` (agent):

```markdown
<!-- agents/lead.md -->
Delegate the implementation to the {{ns:dev}} agent.
```

Installed unprefixed:

```
Delegate the implementation to the dev agent.
```

Installed with `meld --as jk`:

```
Delegate the implementation to the jk:dev agent.
```

A worked multi-item source is at
[examples/namespacing](https://github.com/jaemk/mind/tree/main/examples/namespacing).

## Validation at install time (NS-12, NS-13)

A token whose `name` is not a sibling in the same source is a `BadReference`
error at install time, naming the referencing item and the bad referent.
Expansion runs in a staging copy during the transactional install, so a bad
reference fails before the live install is touched.

Content with no `{{ns:` tokens is copied unchanged. Non-UTF-8 files are not
scanned (NS-13).

Whitespace inside a token (`{{ns: name }}`) is trimmed before the sibling
lookup. An unterminated token (`{{ns:` with no closing `}}`) is left verbatim
rather than treated as a reference or an error (NS-15).

## Scope: prose only (NS-24)

`{{ns:name}}` is a prose name reference. It is misplaced in a fenced code block,
an inline code span, adjacent to a path separator (`/` or `~`), or in a
frontmatter structured field like `name:`, where name-substitution would yield
broken code, a broken path, or (under a prefix) a wrong identity. For code and
paths, use the path tokens (`{{self}}`, `{{tools:name}}`, `{{path:ref}}`
described in [Tooling](tooling.md)) instead of `{{ns:}}`.

`mind review` flags misplaced `{{ns:}}` tokens (CLI-139); `init-source
--template` does not create them (INIT-5).

## Unguarded-reference warning (NS-20 to NS-23)

A source whose items reference siblings in bare prose (no token) breaks at
runtime under a prefix. `mind` does not guess and rewrite prose, because sibling
names are often common words. Instead, when melding a source with a prefix in
effect, `meld` scans each item's text files for sibling names that appear outside
any `{{ns:}}` token and warns for each item where it finds one (NS-20).

Matching is whole-word (alphanumeric, `_`, and `-` are word characters); an
item's own name is not reported against itself (NS-21). The warning is advisory
and does not fail `meld`; it does not rewrite anything (NS-22). No warning is
emitted when no prefix is in effect (NS-23).

## Authoring tools

`mind init-source --template` rewrites bare sibling references to `{{ns:}}` tokens.
`mind review . --fix` does the same for a working tree. Both are covered in
[Authoring a source](authoring.md), which also describes how `init-source` and
`review` surface advisories when a prefixed source's items reference each other in
bare prose.
