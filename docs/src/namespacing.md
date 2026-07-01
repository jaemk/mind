# Namespacing

Two melded sources can each ship an item of the same name (both a `review`).
Without a prefix they would land at the same install path. A prefix namespaces a
source so every item from it installs as `<prefix>:<name>`: the effective name,
store path, symlink, and the name `mind` uses in the ref all carry the prefix.
Agents are the one exception, because the harness keys them by frontmatter name
rather than by their link path; see [Agents are not
namespaced](#agents-are-not-namespaced). Most sources give their items unique,
descriptive names and never need a prefix; this page covers the case where a
prefix is needed.

## Setting a prefix (NS-1, NS-2)

Two ways, in precedence order:

1. **Consumer-side**: `meld --namespace <prefix>` (short `-n`). Stored as the
   source alias and takes priority over anything the repo declares.
   `meld --namespace ''` removes a prefix. (`--as <prefix>` is a deprecated alias
   for `--namespace`.)
2. **Repo-side**: `[source].prefix` in `mind.toml`.

```toml
[source]
prefix = "jk"
```

With prefix `jk`, every item in the source installs as `jk:<bare-name>`. The
catalog and the item's stable identity keep the bare name; the prefix is applied
at install time (NS-3). To change a prefix after items are installed, forget
the installed items first (`mind forget`) and then re-meld with the new prefix.

## Agents are not namespaced

A prefix reaches skills, rules, and tools but not an agent's harness identity
(NS-40, NS-41, NS-42). A
skill is keyed by its directory name, so prefixing its directory and link changes
the name the harness resolves. An agent is keyed by the `name` field in its
frontmatter, not its filename, so renaming the link to `<prefix>:<name>` would
not change the resolved name. mind therefore links an agent under its bare
frontmatter `name` in each lobe even when the source has a prefix in effect
(NS-40). The prefix still applies to the agent's store path and manifest key, so
its stable identity stays collision-free and a prefix change is still a rename;
only the harness-visible link is bare.

Because agents link under their bare name, two melded sources that each ship an
agent with the same frontmatter `name` resolve to the same lobe link. mind
detects this instead of silently repointing: `learn`ing an agent whose bare name
already maps to an installed agent from a different source fails with an
`AgentCollision` error telling you to `mind forget` the existing one first, and
`meld` surfaces it as an advisory warning (NS-41). A prefix does not avert the
collision, since it does not reach the agent link.

A sibling agent's name is the same bare name with or without a prefix, so a bare
prose reference to it resolves either way and the unguarded-reference warning does
not fire for it (NS-42). A `{{ns:}}` token naming a sibling agent still expands
(to the bare name) and is not an error, so tokenizing an agent reference is
harmless.

## Why `{{ns:name}}` tokens exist (NS-10, NS-11)

The Claude harness resolves a skill at runtime by the name in the text, and a
prefix changes that name. A plain-prose reference like "run the review skill"
breaks once `review` installs as `jk:review`. Authors write intra-source
references as `{{ns:name}}` instead, where `name` is the sibling's bare name.

At install time, mind expands each token to the referent's effective name:

| source installed as | `{{ns:review}}` expands to |
|---------------------|----------------------------|
| unprefixed          | `review`                   |
| `--namespace jk`    | `jk:review`                |

Expansion runs whether or not a prefix is in effect (NS-14), so a token-using
source installs correctly in both cases. A token whose referent is an agent
expands to the bare name in both cases, since agents are not namespaced ([Agents
are not namespaced](#agents-are-not-namespaced)).

### Minimal example

A source with an agent `lead` that references a skill `review`:

```markdown
<!-- agents/lead.md -->
When implementing, run the {{ns:review}} skill.
```

Installed unprefixed:

```
When implementing, run the review skill.
```

Installed with `meld --namespace jk`:

```
When implementing, run the jk:review skill.
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
item's own name is not reported against itself (NS-21). A reference whose
referent is a sibling agent is not reported either, since agents keep their bare
name under a prefix and so the reference does not break (NS-42). The warning is
advisory and does not fail `meld`; it does not rewrite anything (NS-22). No
warning is emitted when no prefix is in effect (NS-23).

## Authoring tools

`mind init-source --template` rewrites bare sibling references to `{{ns:}}` tokens.
`mind review . --fix` does the same for a working tree. Both are covered in
[Authoring a source](authoring.md), which also describes how `init-source` and
`review` surface advisories when a prefixed source's items reference each other in
bare prose.
