# The `workflow` item kind

Status: planned. Claude Code reads user-authored workflows from a `workflows/`
directory of the agent home (`~/.claude/workflows/<file>.js`) and offers each one
to the `Workflow` tool by name. A workflow is a JavaScript file that orchestrates
subagents: it declares `export const meta = {...}` and a body that calls
`agent()`, `parallel()`, `pipeline()`, `phase()`, and `log()`. Sources that ship
skills, agents, and commands are the same sources that will ship workflows, and
without this kind a melded repo's workflows are left behind, outside the store,
the manifest, and `upgrade`.

`workflow` is a sixth item kind alongside skill, agent, rule, command, and tool.
It is an ordinary linked kind: discovered by convention, copied into the store,
symlinked into every agent home that admits it, namespaced, upgraded, and removed
as the other kinds are. This document states only what is specific to it;
everything else follows the general rules in discovery.md, storage.md,
lifecycle.md, and namespacing.md.

The harness behavior recorded here was read from the Claude Code 2.1.236 bundle,
not from its documentation, which does not describe the loader. Each observation
that mind depends on is called out at the requirement that depends on it.

## The kind

- `WF-1` `workflow` is an item kind. By convention a workflow is a file
  `workflows/<name>.js` under a scan root, and its name is the file stem, the
  same shape as an agent (DSC-11), a rule (DSC-12), and a command (CMD-1). A
  missing `workflows/` directory yields no items, not an error (DSC-13).
- `WF-2` The convention scan is flat: it reads the immediate `.js` children of
  `workflows/`, and does not descend into subdirectories. This matches agents,
  rules, and commands (CMD-2), and it matches the harness, whose own loader is a
  single non-recursive `readdir` of the directory. Unlike a command (CMD-2)
  there is no nested-name convention to converge on: the harness derives no name
  from a workflow's path at all (WF-20), so a subdirectory is simply invisible to
  it.
- `WF-3` The extension is exactly `.js`, compared case-sensitively, as the
  harness compares it. The harness counts a `.mjs`, `.cjs`, or `.ts` sibling as a
  near miss and skips it, and ignores every other extension outright; a file mind
  installed under one of those names would therefore never load. mind does not
  discover them either, so the two agree on what a workflow is.
- `WF-4` A workflow's description comes from the `description` in its `meta`
  object (WF-5), not from YAML frontmatter: a `.js` file has none. `mind.toml`
  `[[items]].description` still overrides it, as for every kind (DSC-32).
- `WF-5` mind reads a workflow's `meta` with a dedicated minimal reader, in the
  same spirit as the frontmatter reader (`frontmatter.rs`): it recognizes the
  `export const meta = { ... }` object-literal form the harness requires and
  extracts the string-valued `name`, `description`, and `whenToUse` keys from it.
  It is not a JavaScript parser. Where it cannot read a value it yields nothing
  and the item lists without a description; it never fails a scan, an install, or
  a meld on the shape of a workflow's code.

  The harness's own reader is stricter and is the authority on what loads: it
  parses the file to an AST and requires a single `const` declarator named `meta`
  whose initializer is an object literal, admitting only literals, arrays, nested
  objects, uninterpolated template literals, and negated numbers as values, and
  rejecting computed keys, methods, accessors, spread, and `__proto__`. mind's
  reader recognizing a `meta` the harness would reject (or the reverse) affects
  only the description mind displays and the WF-30 finding, never whether the
  file installs.
- `WF-6` `mind.toml` accepts `kind = "workflow"` wherever a kind is named: an
  `[[items]]` entry, a `[discover].workflows` glob list (matching the workflow
  FILE, as the agent, rule, and command globs do), and a lobe's `kinds` filter
  (HARN-1). `--kind workflow` selects the kind wherever the CLI takes one, and
  `workflow:<name>` is an item ref wherever a ref is taken.
- `WF-7` The harness caps a workflow file at 524288 bytes and skips a larger one.
  mind installs it regardless and reports the overage (WF-30); see WF-32 for why
  the cap is reported rather than enforced.
- `WF-8` A `[[items]]` entry may declare a workflow at any path, as for any kind.
  Its `link` is confined to a kind directory (DSC-97), with `workflows/` now
  among them.

## Storage and linking

- `WF-10` A workflow installs to `~/.mind/store/workflow/<effective_name>` (the
  file itself, as for an agent, rule, or command) and links into each admitting
  agent home at `workflows/<effective_name>.js` (STO-10, LIFE-1). It is a linked
  kind: unlike a tool (TOOL-3) the harness discovers it.
- `WF-11` mind's install model works for this kind because the harness's loader
  admits a symlink explicitly: it keeps a directory entry when
  `entry.isFile() || entry.isSymbolicLink()`, then reads through it. A workflow
  linked from the store is therefore loaded exactly as a regular file in the
  agent home is. This is the single assumption the kind rests on. If it ever
  changes, every managed workflow disappears from the harness at once while
  `mind recall` keeps reporting it installed and healthy, the same exposure
  CMD-6 records for a namespaced command.
- `WF-12` The harness presets (HARN-4: gemini, codex, universal, windsurf) admit
  skills only, so they are unchanged by this kind: a workflow links into the
  default Claude lobe and into any lobe whose `kinds` filter names `workflow`. A
  lobe with no filter admits every linked kind, workflows included.
- `WF-13` A project lobe (HARN-19) needs no special handling: the harness reads
  project workflows from `<project>/.claude/workflows/`, which is where a project
  lobe already links a `workflow` item by WF-10.

## Identity and namespacing

- `WF-20` The harness resolves a workflow by the `name` in its `meta` object, not
  by its file name. The file name selects nothing: the loader reads every `.js`
  file in the directory and keys the result by `meta.name`. Renaming the file
  therefore does not rename the workflow, and two files whose `meta.name` agree
  are two workflows with one name.
- `WF-21` An item's name is nonetheless its file stem (WF-1), as for every other
  file-shaped kind. mind's names must be a single safe path component (DSC-71),
  which `meta.name` is not required to be, and an item's stable identity is
  `(source, kind, bare_name)` across every verb (namespacing.md, lifecycle.md);
  deriving it from file content would make it neither. The divergence WF-20
  creates is reported (WF-24), not resolved by changing what a name is.
- `WF-22` A namespace prefix applies to a workflow as to any kind: the effective
  name is `<prefix>:<name>`, the store path and the link are
  `workflows/<prefix>:<name>.js`. By WF-20 this alone does not namespace the
  workflow as the harness sees it, which is what WF-23 is for. Prefixing the file
  is still correct on its own terms: it keeps two sources' same-named workflows
  from colliding on one link path, and it keeps a prefix change a rename matched
  on identity by `upgrade`/`introspect` rather than an orphan plus a new item.
- `WF-23` An author namespaces the harness-facing name by writing the `{{ns:}}`
  token in `meta.name`:

  ```js
  export const meta = {
    name: '{{ns:review}}',
    description: 'Review changed files across dimensions',
  }
  ```

  Install-time expansion (NS-11) renders it as the effective name (`review` when
  unprefixed, `jk:review` when prefixed), so the stored file carries a plain
  string literal and the harness sees the same name mind installed. A
  self-reference resolves like any other: the sibling set an item is expanded
  against includes the item itself. The harness's own convention for a namespaced
  workflow is the same spelling (it names a plugin's workflow
  `<plugin>:<meta.name>`), so an expanded name reads as the harness's own, not as
  a mangled one.
- `WF-24` When an installed workflow's `meta.name` (as read by WF-5, after
  expansion) is a string that differs from the item's effective name, mind warns.
  That workflow is installed and healthy by every check mind makes, and the
  harness answers to it under a different name than `mind recall` reports. The
  warning is advisory: the divergence is legal, and a source may want it.
- `WF-25` All four token families expand in a workflow file: `{{ns:}}`,
  `{{path:}}`, `{{tools:}}`, and `{{self}}`, over the whole file, with the same
  resolver and the same hard bad-reference failure as a markdown file (NS-11,
  NS-12). This is an exception to NS-53, which expands by extension alone and
  would otherwise leave a `.js` file untouched. It is granted on the kind, not
  the extension: a workflow's content is agent prompts, so a sibling reference in
  one is the designed use of the token and not the incidental `{{ }}` of a
  templating language that NS-53 declines to fight. It also makes WF-23 work,
  which the meta-only alternative would do at the cost of leaving a token in a
  prompt as dead text. The cost accepted is that a literal `{{` in a workflow's
  code is now read as a token; an unterminated or unresolvable one behaves as it
  does in markdown (NS-12, NS-13).
- `WF-26` A path token in a workflow renders in the `~` home form (TOOL-16), not
  the absolute form NS-57 uses for an expand-listed script. A workflow never
  executes a path itself: it has no filesystem access, and its strings are
  prompts read by an agent, which is the reader TOOL-16's form is for.
- `WF-27` `review`'s `inert-token` finding (CLI-223), which reports any `{{...}}`
  in a non-markdown item file as dead text, does not apply to a workflow file.
  Tokens expand there (WF-25), so the finding would be false.
- `WF-28` The unguarded-reference scan (NS-20) covers a workflow file, as it
  covers every text file of an item.

## Reporting a workflow the harness will not load

- `WF-30` `review` reports a workflow the harness would skip, as a finding
  alongside its existing hook and reference findings: no readable `meta`, a
  missing or empty `name` or `description`, or a file over the WF-7 size cap.
  Such a workflow does not exist as far as the harness is concerned, and nothing
  else in mind's model would say so.
- `WF-31` `learn` warns on the same condition and installs anyway. mind does not
  gatekeep the validity of item content for any other kind, and the harness's own
  reader is the authority (WF-5); a reader disagreement must not be able to
  block an install.
- `WF-32` The size cap is reported, not enforced, for the same reason: DSC-90
  records that mind does not cap the size of item content it reads, and a cap
  mind enforced would be mind's cap, not the harness's.

## Plugins

- `WF-40` A plugin root's `workflows/` directory maps to the `workflow` kind and
  its files install as items, the way `commands/` does (MKT-18). The harness
  loads a plugin's workflows from that directory under the same rules as an agent
  home's (WF-2, WF-3, WF-11).
- `WF-41` A plugin manifest's `workflows` key (a path or list of paths, and the
  `experimental.workflows` spelling beside it) is a component-path override that
  mind ignores, as it ignores `commands`, `outputStyles`, and the rest (MKT-3).
  A plugin that sets it is loaded by the harness from the declared path and by
  mind from the convention directory, and the two disagree. This is the existing
  treatment of every component override, recorded here because `workflows` is the
  newest of them, not because the kind changes it.
- `WF-42` The harness names a plugin's workflow `<plugin>:<meta.name>`. mind
  installs a plugin's items under the plugin name as the default effective prefix
  (MKT-5), so an expanded `{{ns:}}` name (WF-23) produces the same string the
  harness would produce for that plugin. A plugin workflow with a bare literal
  `meta.name` diverges by WF-24.

## Everything else follows the general rules

- `WF-50` A workflow participates in every kind-generic mechanism with no
  workflow-specific behavior: `learn`/`forget`/`upgrade`/`introspect`, drift
  hashing (LIFE-15), `requires` dependencies (DEP-4), item lifecycle hooks
  including the frontmatter scalars (HOOK-80, HOOK-130), which for a workflow can
  only come from an `[[items]]` entry or a scoped `mind.toml` since the file
  carries no frontmatter, ignore patterns (IGN-1), `absorb` (whose convention
  path for the kind is `workflows/<name>.js`), `dump`, `probe`/`recall` listing,
  and unmanaged-item detection in a lobe's `workflows/` directory (UNM-1). As
  with commands (CMD-8), `absorb` and unmanaged detection see only the immediate
  `.js` children of a lobe's `workflows/` directory, matching the flat convention
  scan (WF-2).
- `WF-51` `probe` and `recall` surface a workflow's `whenToUse` (WF-5) where an
  item's description is shown, appended to the description as the harness's own
  workflow list renders it (`<description> - <whenToUse>`). No other kind has a
  second description field; a workflow without one is unaffected.
- `WF-52` `workflow` becomes a reserved namespace prefix, appended to the NS-29
  list, which is append-only for exactly this case. A source whose `[source]
  .prefix` is the literal string `workflow` is refused from that point on, with
  the same `ReservedPrefix` error the other kind words produce.
