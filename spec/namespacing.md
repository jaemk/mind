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
- `NS-28` A namespace prefix must be a single safe path component: it must not
  be empty; must not equal `.` or `..`; must not start with `~`; must not
  contain a path separator (`/` or `\`), a colon (`:`), a NUL byte, or a
  control character (U+0000--U+001F or U+007F). Every ingress that accepts a
  prefix -- `meld --namespace`, `[source].prefix` / `[source].namespace`,
  `config` -- calls `validate_prefix` before persisting the value. A violation
  is a structured `UnsafePrefix` error, distinct from the `ReservedPrefix`
  error for the kind-word/DEC-9 reserved list (NS-25/NS-29).
- `NS-29` The reserved-kind-word list (NS-25) is permanent and append-only
  (DEC-9). The following additional words are reserved against plausible future
  item kinds or CLI subsystem names: `command`, `hook`, `mcp`, `plugin`,
  `prompt`, `mode`, `output-style`. A prefix equal to any of these is rejected
  with the same `ReservedPrefix` error as the kind-word list.

## Collision-triggered namespace prompt

- `NS-43` At `meld`, after catalog discovery and before install, mind checks the
  incoming source's effective items (skills, rules, and tools -- agents are handled
  separately by NS-41) against all already-installed items from other sources. An
  incoming item whose effective `(kind, name)` pair matches an already-installed
  item from a different source is a cross-source collision. The check uses the
  effective names that would result from the incoming source's current alias, if
  any; it does not pre-apply a namespace. A source that already has a namespace in
  effect (from `--namespace` or `mind.toml`) whose effective names do not collide
  is not prompted. This check is distinct from the same-invocation check at
  CLI-33 (two items in one `learn` call) and from NS-41 (agent collisions by bare
  name).

- `NS-44` When one or more cross-source collisions are detected (NS-43) in an
  interactive session (TTY and no `--yes`), `meld` pauses and prompts the user to
  enter a namespace prefix, listing the colliding items and their existing source.
  The prompt pre-populates the repo name (the last path or URL component of the
  source) as the suggested value. Accepting the suggestion or typing a different
  prefix is equivalent to re-running with `--namespace <prefix>` and continues the
  meld under that prefix. Entering an empty value explicitly clears the namespace
  and continues (the user acknowledges the collision and proceeds without one).
  Aborting stops `meld` with a non-zero exit and no source registration.

- `NS-45` When one or more cross-source collisions are detected (NS-43) in a
  non-interactive session (no TTY or `--yes`), `meld` errors (`SkillCollision`)
  and lists the conflicting items and their existing source. The error message
  suggests re-running with `--namespace <prefix>`, using the repo name as the
  example value. No source is registered and no items are installed.

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
  an installed agent from a different source is refused at `learn` with an
  `AgentCollision` error that tells the user to `mind forget` the existing agent
  first. The collision is also surfaced at `meld` as an advisory warning (does not
  prevent the source from being melded). A prefix does not avert it (the prefix
  does not reach the agent link), so two same-named agents from different sources
  cannot both be active; this is an inherent limit of the harness's global agent
  namespace, made explicit instead of mishandled.
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

- `NS-30` A registered source's namespace (its display prefix, set by
  `--namespace`, NS-1/CLI-159) is changeable in place (the TUI source-details
  editor, TUI-53) only while none of its items are installed: a `--link-only`
  meld (CLI-23), or a super-source whose nested sources are registered but not
  installed. Once any of the source's items are installed the namespace is locked:
  changing it requires forgetting the source's installed items first. A requested
  change on a source with installed items is refused with guidance to uninstall
  first (CLI-161), not applied as an in-place rename. This changes the effective
  display prefix only; it does not change the source's identity or relocate its
  clone (the identity alias is fixed at meld, STO-58). Melding the same repo under
  a different `--namespace` is a distinct instance, not a namespace change
  (CLI-13). This is distinct from the one-time `-`->`:` separator migration
  (NS-27), which `upgrade` applies to already-installed items without a namespace
  change.

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

- `NS-20` When melding a source with a prefix in effect and `--verbose` is in
  effect (CLI-162), every text file of each item (the whole skill directory, or
  the agent/rule file) is scanned for sibling names that appear in bare prose
  (outside any `{{ns:}}` token), matching the breadth of install-time expansion;
  each item with such a reference is reported as a warning. Without `--verbose`
  no scan is performed and no warning is emitted.
- `NS-21` Matching is whole-word (alphanumeric, `_`, and `-` are word
  characters); an item's own name is not reported against itself.
- `NS-22` The warning is advisory and heuristic: it does not fail `meld`, does not
  rewrite anything, and is only emitted under `--verbose` (CLI-162).
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

### Structural scanning

Deciding whether a token sits in code (NS-24) is a question about markdown
structure, and the answer is a property of the whole document, not of the line
the token sits on: an inline code span may close on a later line, a fenced block
may quote a fence of its own, and either may be nested inside a list item or a
blockquote. The structural read is therefore CommonMark, as implemented by a
conformant parser: the requirements below state which structures decide a
token's context, not how those structures are recognized. Where this spec and
CommonMark could differ, CommonMark wins, and a shape it disagrees with is a
defect in this scan rather than a documented behavior.

One structure map is derived per document and answers, per byte position, what
that position is. The same map drives un-wrapping (`review --fix`, CLI-138) and
wrapping (`templatize`, INIT-5), so the two passes cannot disagree about what is
code: every token wrapping creates is a token the scan calls prose.

- `NS-46` A `{{ns:}}` token is in a code span when it falls inside an inline code
  span, and a bare sibling name inside one is never wrapped. Code spans are read
  as CommonMark defines them, so a token following a span that closed on a
  continuation line is prose rather than a misplaced reference, a backtick run
  that no run matches is literal text, and a run cannot pair with one in a
  different block.
- `NS-47` A `{{ns:}}` token is in a code block when it falls inside the content
  of a code block -- fenced or indented, at the top level or inside a list item
  or a blockquote -- and a bare sibling name there is never wrapped. Code blocks
  are read as CommonMark defines them, so an inner example fence does not close
  the block quoting it, and an unclosed fence runs to the end of whatever
  contains it. A block's delimiter lines are structure and not content: a token
  in an info string is not a reference, so `review` neither reports nor rewrites
  it (`expand` still substitutes it at install time, NS-11). The leading
  `--- ... ---` frontmatter block is not CommonMark and is read separately: it
  opens only on the document's first line, after a leading UTF-8 BOM if the file
  carries one (stripped exactly as discovery strips it, DSC-23, so a BOM-prefixed
  item file mind installs normally is one whose frontmatter this read still
  sees); its delimiters carry no content; and the structural read covers the body
  after it.
- `NS-49` Indentation and containers are read as CommonMark defines them. An
  indented code block is a code block (NS-47), so a document showing a bare
  ` ``` ` inside one opens nothing and the prose after it stays prose; a fence
  indented to its list item's content column is still a fence; an over-indented
  continuation of an open paragraph is prose, because an indented code block
  cannot interrupt a paragraph; and a block ends where the container holding it
  ends, so an unclosed fence inside a list item stops with the item instead of
  running to the end of the document.
- `NS-50` Backslash escapes are read as CommonMark defines them: an escaped
  backtick is literal text and opens no code span (NS-46), while escapes do not
  apply inside a code span, so an escaped backtick still closes a span that is
  already open.
- `NS-52` Everything a link or an image is made of except its visible text is
  markdown syntax, not prose: the destination, the title, the reference label,
  and the whole of a link reference definition (`[label]: url "title"`). A bare
  sibling name in any of them is never wrapped, because wrapping it edits syntax
  rather than prose and the link stops resolving (a reference renders as a
  literal `[{{ns:name}}]`, a destination points at a name that is not a path). A
  token there is misplaced for the same reason a token beside a path separator is
  (NS-24): it expands to the referent's effective name (NS-11), which under a
  prefix is a destination or a label that no longer resolves, so `review` reports
  it as a path and `--fix` un-wraps it. The visible text of an inline
  (`[text](url)`) or full reference (`[text][label]`) link is prose and stays
  wrappable, since a sibling named there is a real name reference. A shortcut
  (`[label]`), collapsed (`[label][]`), or autolink (`<https://x/y>`,
  `<a@b.c>`) link has no text distinct from its label or destination, so its text
  is syntax too. Links are read as CommonMark defines them, so text that no
  definition resolves is not a link at all and stays ordinary prose.
- `NS-51` Wrapping copies every `{{...}}` brace span verbatim, whatever it
  contains and whether or not it fits on one line, so it never creates a token
  inside an existing one. In particular a token written across a line break is
  left exactly as written rather than having the name inside it wrapped again,
  which would produce a nested token that `install` rejects as a bad reference
  (NS-12) and so would stop the source installing at all.
- `NS-48` `review --fix` (CLI-138) leaves no unguarded reference in prose: for
  content whose sibling mentions all sit in prose (already tokenized or bare),
  the rewritten content is reported clean by the unguarded-reference check
  (NS-20, NS-21). A prose token misclassified as code would otherwise be
  un-wrapped into exactly the bare name that check reports, and under a prefix
  that name no longer resolves (NS-11); reading the document as CommonMark
  (NS-46, NS-47, NS-49, NS-50) is what closes that class of misclassification.
  The requirement is scoped to prose because the unguarded-reference check is
  context-free while both `--fix` passes are not: un-wrapping a token that really
  is in code restores a bare name that NS-20 still reports, and wrapping declines
  to re-wrap it (NS-24), so `--fix` cannot clear it. The same holds for a sibling
  name in a frontmatter field, which NS-20 reports and wrapping never touches;
  for one abutting a path separator, which NS-20 reports (a `/` is not a word
  character) and NS-24 forbids wrapping; for one reachable only as a link
  destination, title, or reference label, which NS-20 reports and NS-52 forbids
  wrapping; and for one in a fence info string, which NS-20 reports (a backtick
  is not a word character) and which NS-47 calls delimiter structure rather than
  content, so wrapping declines it. Those six are known false positives of the
  advisory, not failures of this requirement.
