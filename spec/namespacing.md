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
  error for the kind-word/extended reserved list (NS-25/NS-29).
- `NS-72` The low-level `is_safe_prefix_component` guard (used by
  `validate_prefix`, and independently by catalog code that derives a prefix
  from a marketplace/catalog entry name) additionally rejects a multi-byte
  security-blocked Unicode code point the NS-28 byte scan cannot see: a bidi
  override, a directional mark, a zero-width character, or a line/paragraph
  separator (U+2028, U+2029) -- the DSC-94 set, via
  `sanitize::has_blocked_chars`. The live path this guard changes the outcome
  on is `validate_prefix`: a user-supplied `meld --namespace`/`-N` value, or a
  `[source].prefix` declared in a melded repo's `mind.toml`, that carries one
  of these characters is refused with a structured `UnsafePrefix` error rather
  than being accepted as a prefix that would carry a spoofing mark into every
  namespaced ref (tests/cli_prefix_guard.rs). The catalog's entry-name path is
  NOT where this guard bites in practice: `catalog.rs` already runs
  `strip_ansi` on a marketplace/catalog entry name before offering it as a
  prefix candidate, so by the time `is_safe_prefix_component` sees that name
  the DSC-94 set is already gone from it; the guard is defense in depth there,
  not the mechanism that stops a raw entry name from seeding a prefix.
- `NS-73` The security-blocked Unicode set `sanitize::has_blocked_chars` (and
  therefore `is_safe_prefix_component`/`validate_prefix`, NS-72) rejects is
  broadened beyond the original bidi/zero-width/separator set (DSC-94) to
  cover the rest of the Unicode format category (Cf) plus two additional
  invisible blocks: the Unicode tag block (U+E0000--U+E007F) and the
  variation-selector blocks (U+FE00--U+FE0F, U+E0100--U+E01EF). The tag block
  is the standard "invisible ASCII smuggling" vector -- text hidden there
  renders as nothing at a terminal but is plain text to a parser or an AI
  agent reading the same string -- so a prefix or an item name carrying a tag
  character is rejected the same as one carrying a bidi override. The
  broadened set is a strict superset of the original: every code point
  blocked before NS-73 is still blocked.
- `NS-29` The reserved-kind-word list (NS-25) is permanent and append-only.
  The following additional words are reserved against plausible future
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
- `NS-11` At install, each `{{ns:name}}` token in a markdown item file (NS-53) is
  expanded to the effective name: `name` when unprefixed, `p:name` when prefixed.
- `NS-12` A token whose `name` is not a sibling in the same source is an error
  (`BadReference`), naming the referencing item and the bad referent. Applies only
  within a markdown file (NS-53): a token with no resolvable referent in a
  non-markdown file is not an error, since it is never expanded there either.
- `NS-13` Content with no `{{ns:` tokens is copied unchanged. A non-markdown file
  (NS-53) is not scanned at all, so its content -- non-UTF-8 or otherwise -- is
  never read for tokens; a markdown file that is not valid UTF-8 is likewise not
  scanned.
- `NS-14` Expansion runs whether or not a prefix is in effect, so a token-using
  source installs correctly with or without a namespace.
- `NS-15` Token edge cases: whitespace inside a token (`{{ns: name }}`) is
  trimmed before the sibling lookup; an unterminated token (`{{ns:` with no
  closing `}}`) is left verbatim rather than treated as a reference or an error.
- `NS-53` All four token families -- `{{ns:}}` (this doc), `{{path:}}`,
  `{{tools:}}`, and `{{self}}` (tooling.md, TOOL-19) -- expand only in a markdown
  file: one whose extension, case-insensitively, is `md`, `markdown`, `mdown`, or
  `mkd`. `install` skips any other file before expansion, leaving its content --
  including any token in it, resolvable or not -- exactly as written (NS-12,
  NS-13). This is a deliberate narrowing, not an oversight: the designed use of
  a path token is prose (a skill telling Claude to run `{{tools:detect}}`), and
  templating a file written in a language with its own `{{ }}` convention
  (Jinja, Handlebars, Go templates) is a liability with no offsetting benefit.
  The rule is judged by extension alone, not by content or by item kind: an
  agent or a rule item is always a single markdown file by discovery convention
  and so is unaffected in practice, but a `mind.toml`-declared item is free to
  point at any path, and this still governs it. Widening later (an opt-in
  `mind.toml` glob naming additional files to scan) stays backward-compatible;
  narrowing later would not, which is why it is done now, pre-1.0. The widening
  is NS-57.
- `NS-57` An item may opt specific non-markdown files into expansion with an
  `expand:` frontmatter key: a whitespace-separated list of item-relative file
  paths (the same scalar form `requires:` uses, DEP-4). At install, each listed
  file is scanned and expanded exactly as a markdown file is -- all four token
  families (`{{ns:}}`, `{{path:}}`, `{{tools:}}`, `{{self}}`), the same resolver
  and the same bad-reference rule (NS-11, NS-12) -- overriding the NS-53
  extension test for that file alone. This is the designed escape for a bundled
  script that must reference a sibling tool or an adjacent file in a language
  whose own path handling makes self-location awkward. Properties:
  - The key lives in the item's own frontmatter, not in `mind.toml`'s inventory
    layer, so declaring it on one item leaves convention discovery on and says
    nothing about any other item; a source needing expansion on one skill does
    not become authoritative (DSC-3) and does not have to enumerate the rest.
  - A path token in an expand-listed file renders as an absolute store path, not
    the `~` form TOOL-16 uses for markdown, because such a file is typically read
    by a program that does not itself expand `~` (TOOL-20).
  - An `expand:` entry is a safe relative path within the item: an absolute path,
    a `..` segment, or an entry naming a file the item does not ship fails the
    install with `BadReference` during staging (the transactional pre-swap check,
    like a bad `{{ns:}}` or `requires:` entry), so a typo is caught loudly rather
    than left as an un-expanded literal. A token in an expand-listed file that
    does not resolve is a hard install failure, exactly as in markdown (NS-12),
    not the inert dead text it would be in an unlisted non-markdown file.
  - Only a directory item (a skill or a tool) has bundled files to list. An agent
    or a rule is a single markdown file already covered by NS-53, so an `expand:`
    entry on one resolves to nothing and is a no-op or, if it names a path, the
    same missing-file `BadReference`.

## Unguarded-reference warning

- `NS-20` When melding a source with a prefix in effect and `--verbose` is in
  effect (CLI-162), every text file of each item (the whole skill directory, or
  the agent/rule file) is scanned for sibling names that appear in bare prose
  (outside any `{{ns:}}` token; structure-aware, NS-55); each item with such a
  reference is reported as a warning. Without `--verbose` no scan is performed
  and no warning is emitted. The scan covers every text file regardless of
  extension, unlike install-time expansion (NS-53): a mention in a non-markdown
  file is still worth flagging (the author may move it into markdown, or add a
  reference doc later), even though it is inert either way.
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
  or an inline code span, adjacent to a path separator (`/` or `~`), or in the
  frontmatter `name:` field, the one structured field that is the item's own
  identity -- where name-substitution yields broken code, a broken path, or a
  wrong identity. A token an author wrote in another frontmatter field is an
  ordinary reference and is not reported as misplaced, but only the
  `description:` value is a field wrapping may *create* one in (NS-56). Code and
  paths reference an
  item by path token instead (`{{self}}`, `{{tools:}}`, `{{path:}}`;
  tooling.md), never by `{{ns:}}`. `review` detects misplaced tokens (CLI-139) and
  `init-source --template` does not create them (INIT-5).
- `NS-54` `review --fix` (CLI-138) rewrites only a markdown file (NS-53): for a
  file the extension test rejects, every rewrite pass (un-wrapping a misplaced
  `{{ns:}}` token, wrapping a bare prose mention, rewriting a hardcoded install
  path into a token) is skipped, and the file is reported on instead --
  `review`'s other checks (CLI-135..139) already scan every text file regardless
  of extension, so nothing about a non-markdown file's tokens goes unreported,
  only unrewritten. Rewriting a non-markdown file would write, or leave behind,
  text that this file's contents never expand out of (NS-53), which is a
  regression relative to the file's previous, unexpanded state.

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
- `NS-48` `review --fix` (CLI-138) leaves no unguarded reference: for content
  whose sibling mentions all sit where wrapping can reach them (prose or the
  frontmatter `description:` value, NS-56), the rewritten content is reported
  clean by the unguarded-reference check (NS-20, NS-21). A prose token
  misclassified as code would otherwise be un-wrapped into exactly the bare
  name that check reports, and under a prefix that name no longer resolves
  (NS-11); reading the document as CommonMark (NS-46, NS-47, NS-49, NS-50) is
  what closes that class of misclassification. This now holds without
  exception: the unguarded-reference check is structure-aware (NS-55), reading
  the same map both `--fix` passes read, so it never reports a mention in a
  code span, a code block, a fence's delimiter structure, link syntax, or a
  path-adjacent position -- none of those was ever a real reference, so
  reporting one was always a false positive of the advisory, not a use of it
  `--fix` failed to clear. A mention in the frontmatter `description:` value is
  wrapped, same as prose (NS-56), so it clears too; a mention anywhere else in
  a frontmatter block is never reported at all, because wrapping must never
  create a token in a structured field (NS-56).
- `NS-55` The unguarded-reference check (NS-20) is structure-aware: it reads
  the same [`Structure`] map `templatize` (NS-46, NS-47, NS-52) and the
  `{{ns:}}` misplaced-token scan (CLI-139) read, through the identical
  wrappable-position test `templatize` wraps with, so it reports exactly the
  set `templatize` can clear and no more. A sibling mention inside a code span,
  a fenced or indented code block, link syntax, a fence's delimiter structure
  (its info string), or a path-adjacent position is not reported, matching
  `templatize`'s refusal to wrap any of them (NS-24, NS-46, NS-47, NS-52), and
  neither is one in a structured frontmatter field (NS-56); one in prose or in
  the frontmatter `description:` value is reported, matching `templatize`'s
  willingness to wrap it (NS-56). Applying the markdown
  structure map to a non-markdown file (a script, data) is not gated on the
  file's extension: it is still a heuristic read of a file that is not
  markdown, but it only ever suppresses a report an author wrote as, say, an
  indented shell command, never adds a false one, so it moves scan accuracy in
  the right direction even there.
- `NS-56` `templatize` (INIT-5) wraps a bare whole-word sibling mention in the
  *value* of the frontmatter `description:` field, the same as it wraps one in
  the markdown body. `description:` is the only frontmatter field this applies
  to, because it is the only one that is free prose. Every other line of a
  frontmatter block is left exactly as written -- including the `description:`
  key itself, and including a line carrying no `key:` at all (what an
  unterminated block swallows) -- because frontmatter is structure that mind and
  the harness parse out of the *source* file, where no token is ever expanded:
  `requires:` is a list of item refs (DEP-4, DEP-5), `build:`, `install:`,
  `uninstall:` and `bin:` are shell commands run verbatim (tooling.md), and
  `name:` is the item's own identity (NS-24). Wrapping a sibling name into any
  of those writes a token that nothing expands, which breaks the field rather
  than templating it: a wrapped `requires:` entry stops resolving and fails the
  install with a `BadReference` (NS-12). A wrapped `description:` is what a
  display surface (`recall`, `probe`) would otherwise show verbatim as a raw
  `{{ns:name}}` token to a human, so that surface flattens the token back to
  the bare `name` for display (`namespace::flatten_display`) at the point it
  reads the description from the catalog, while the installed item's store
  copy still expands the token to the full effective name (NS-11) as normal.
