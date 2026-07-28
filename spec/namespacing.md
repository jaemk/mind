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
structure, and both constructs that answer it are document-wide, not line-local:
an inline code span may close on a later line, and a fenced block may contain a
fence of its own. The scan reads them that way. The same structure map drives
un-wrapping (`review --fix`, CLI-138) and wrapping (`templatize`, INIT-5), so the
two passes cannot disagree about what is code.

- `NS-46` An inline code span is a run of backticks matched by a later run of
  exactly the same length. The span may cross a line break inside a paragraph and
  is terminated by a blank line or the start of a new block (a heading, list
  item, blockquote, or table row); a run with no matching run is literal text,
  and scanning resumes after it. A `{{ns:}}` token is in a code span only when it
  falls inside a matched span, so a token following a span that closed on a
  continuation line is prose, not a misplaced reference.
- `NS-47` A fenced code block opens on a line whose leading run is at least three
  backticks or tildes, followed only by an info string, and closes only on a run
  of the same character that is at least as long with nothing else on the line. A
  shorter run, a run of the other character, or a run carrying an info string is
  block content, so an inner example fence does not close the block that contains
  it and a four-backtick fence is not closed by a three-backtick one. A backtick
  opener's info string may not itself contain a backtick, so a line that leads
  with a backtick run and carries more backticks after it is text with code spans
  (NS-46), not a fence.
- `NS-49` Indentation is measured against the content column of the innermost
  open list item, which is the column that item's own content starts in (zero
  when no list item is open); a tab advances to the next multiple of four
  columns. A fence delimiter may be indented at most three columns past that
  baseline, so a fence nested in a list item is still a fence. A line indented
  four or more columns past the baseline is never a delimiter: it is an indented
  code block when no paragraph is open above it (its tokens are in code), and a
  lazy continuation of that paragraph otherwise (its tokens are prose). So a
  document showing a bare ` ``` ` inside an indented code block opens no block,
  and the prose after it stays prose. A fenced block opened inside a list item
  ends with the item: a non-blank line that dedents past the item's content
  column closes it, so an unclosed fence there does not run to the end of the
  document the way an unclosed fence at column zero does (NS-47).
- `NS-50` A backtick preceded by an odd number of backslashes is escaped: it is
  literal text and never opens a code span (NS-46). Backslash escapes do not
  apply inside a code span, so an escaped backtick still closes a span that is
  already open.
- `NS-48` `review --fix` (CLI-138) leaves no unguarded reference in prose: for
  content whose sibling mentions all sit in prose (already tokenized or bare),
  the rewritten content is reported clean by the unguarded-reference check
  (NS-20, NS-21). A prose token misclassified as code would otherwise be
  un-wrapped into exactly the bare name that check reports, and under a prefix
  that name no longer resolves (NS-11). The requirement is scoped to prose
  because the unguarded-reference check is context-free while both `--fix`
  passes are not: un-wrapping a token that really is in code restores a bare
  name that NS-20 still reports, and wrapping declines to re-wrap it (NS-24), so
  `--fix` cannot clear it. The same holds for a sibling name in a frontmatter
  field, which NS-20 reports and wrapping never touches. Those are known false
  positives of the advisory, not failures of this requirement.
