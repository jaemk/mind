# init-source (maintainer scaffolding)

Status: done. `mind init-source` helps a source author prepare a repo for
melding: it scaffolds a `mind.toml`, reports the references among the repo's
items (the intra-source dependency graph), and optionally rewrites bare sibling
references into `{{ns:}}` templating so the source stays resolvable under a
prefix. It is the authoring counterpart to `review` (cli.md, CLI-130): `review`
validates a source, `init-source` sets one up.

## Overview

A maintainer runs `init-source` in their repo. It discovers the items the repo
offers exactly as melding would (convention plus `mind.toml`), prints what it
found and which items reference which siblings, and creates a starter `mind.toml`
if none exists. With `--template` it also rewrites the bare sibling mentions it
detected into `{{ns:name}}` tokens, the form that survives namespacing
(namespacing.md, NS-10). It is read-only except for creating an absent
`mind.toml` and, with `--template`, editing item files; it registers nothing,
makes no network calls, and never touches the store or an agent home.

The rest of this document states the rules normatively. "item" and "source" are
as in spec/README.md; references and `{{ns:}}` tokens are defined in
namespacing.md.

## Rules

- `INIT-1` `mind init-source [path]` operates on a source repo directory,
  defaulting to the current directory. It scaffolds `mind.toml`, reports the
  items it would offer and the references among them, and (with `--template`)
  rewrites bare sibling references to `{{ns:}}` tokens. It registers nothing and
  touches no agent home; it only reads the repo and, when asked, edits files in
  it.
- `INIT-2` Item discovery matches melding: `init-source` scans `path` for
  convention items (`skills/<n>/SKILL.md`, `agents/<n>.md`, `rules/<n>.md`) and
  honors an authoritative `mind.toml` (`[[items]]` / `[discover]`) exactly as a
  melded source would (discovery.md), so what it reports is what melding would
  install. A repo that requires a newer `mind` (`min-mind-version`, DSC-40) is
  rejected here too.
- `INIT-3` When `path` has no `mind.toml`, `init-source` creates one with a
  `[source]` table (a `description` placeholder and a commented-out `prefix`),
  leaving discovery to convention. An existing `mind.toml` is never overwritten:
  it is reported and left as-is.
- `INIT-4` `init-source` reports the intra-source dependency graph: for each
  item, the siblings it already references via `{{ns:name}}` tokens (the DEP-1
  edges) and the siblings it mentions in bare prose (the unguarded references,
  NS-21). The bare mentions are emitted as `unguarded-reference` advisory
  findings in the same `advisory [kind]: message` format `review` uses (CLI-131),
  so the two commands' findings read identically. The advisory fires only when an
  effective prefix is in force, since bare mentions only break at runtime under a
  prefix (INIT-9).
- `INIT-5` With `--template`, `init-source` rewrites each bare whole-word sibling
  mention in an item's text to its `{{ns:name}}` token (NS-10), writes the changed
  files, and reports each rewrite. Text already inside a `{{ns:}}` token is left
  untouched, and so are non-prose positions (NS-24): a bare name inside a fenced
  code block, an inline code span, a path, or a frontmatter structured field is
  not wrapped, so the rewrite never turns a keyword or path component into a
  token. The rewrite is still heuristic in prose (a sibling name can be an
  ordinary word), so it is opt-in and the maintainer reviews the result (e.g. via
  `git diff`).
- `INIT-6` `init-source` makes no network calls and does not read or write the
  store or any agent home; it edits only the target repo. Without `--template` it
  is read-only except for creating an absent `mind.toml` (INIT-3).
- `INIT-7` `init-source` reports the same `duplicate-tooling` advisories `review`
  does (CLI-144), in the same finding format: a non-markdown helper file that is
  byte-identical across two or more items, which should live once under
  `tools/<name>/` and be referenced by token. `--template` does not rewrite it
  (the `--template` hint is shown only for bare prose references, INIT-5);
  extracting a shared tool is a structural change the maintainer makes by hand.
- `INIT-8` Removed (never implemented). `init-source` does not scaffold a
  `[source].install` stub: that field is deprecated (HOOK-90), and the scaffold
  already nudges toward install hooks through commented `[[hooks]]` examples
  (HOOK-57), which is the canonical, more expressive form.
- `INIT-9` `init-source` emits the `unguarded-reference` advisory (INIT-4) only
  when an effective prefix is in force (the repo's `[source].prefix`). With no
  prefix it does not flag a sibling named in bare prose, since an unprefixed
  source's bare references resolve as written and the absence of a prefix is taken
  as the maintainer not intending to namespace. This matches `meld` (NS-23) and
  `review` (CLI-133), neither of which flags a bare sibling reference absent a
  prefix. The `{{ns:}}`-token edges (INIT-4) and the `--template` rewrite (INIT-5)
  are unaffected; only the bare-prose advisory is gated.
