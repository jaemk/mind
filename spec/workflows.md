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
  among them. `workflows/` carries DSC-97's extra condition, the one `commands/`
  carries: only an item of kind `workflow` may name it. The reason is the same
  one DSC-97 gives. A file there is not content the harness merely offers, it is
  something the harness runs, so without the condition a source could declare
  `kind = "rule", link = "workflows/deploy.js"` and turn prose into an
  orchestration a consumer who filtered their install to rules never asked for.

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
  expansion) is a string that differs from the item's effective name, mind warns:
  once at `learn`, as a `review` finding beside the WF-30 ones, and in the
  `recall <item>` detail view. That workflow is installed and healthy by every
  check mind makes, and the harness answers to it under a different name than
  `mind recall` reports. The warning is advisory: the divergence is legal, and a
  source may want it.

  It is absent from the `recall` listing and from `introspect`. Introspect's
  subject is drift, broken symlinks, and unsynced sources, and its `--fix`
  repairs exactly those; a divergence is none of them and is nothing mind could
  repair, so reporting it there would put unactionable noise in the one command
  whose output is meant to be acted on.

  The warning is quiet in the common case. The harness writes a workflow it saves
  to `.claude/workflows/<slug(meta.name)>.js`, so an unprefixed source's file stem
  already equals its `meta.name`, and a prefixed source that tokenizes `meta.name`
  (WF-23) matches after expansion. What is left to warn about is a hand-authored
  file whose stem was chosen independently of `meta.name`, and a prefixed source
  that never tokenized.
- `WF-25` All four token families expand in a workflow file: `{{ns:}}`,
  `{{path:}}`, `{{tools:}}`, and `{{self}}`, over the whole file, with the same
  resolver and the same hard bad-reference failure as a markdown file (NS-11,
  NS-12). The extension test alone would leave a `.js` file untouched, so NS-53's
  gate reads the item's kind as well and grants this one. It is granted on the
  kind, not the extension: a workflow's content is agent prompts, so a sibling reference in
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
  Tokens expand there (WF-25), so the finding would be false. The same follows
  for every other check NS-53 gates on whether a file expands: an unresolved
  token in a workflow is the hard `bad-reference` a markdown file's would be,
  not the downgraded advisory a dead one gets, and a `{{ns:}}` token in one is
  not misplaced (NS-24).

  What does NOT follow is the automatic REWRITING: `review --fix` and
  `init-source --template` leave a workflow file alone, as they leave every
  non-markdown file alone (NS-54). NS-54's own reason does not survive here (a
  token written into a workflow would expand), but the other one does: those two
  rewrite bare prose into tokens by matching sibling names as words, and a
  workflow file is code, where a sibling name may be an identifier, a key, or a
  substring of one. The unguarded-reference scan still reports what it sees
  there (WF-28); an author acts on it by hand.
- `WF-28` The unguarded-reference scan (NS-20) covers a workflow file, as it
  covers every text file of an item.
- `WF-29` Two installed workflows whose effective `meta.name` (WF-5, after
  expansion) is the same string are one name to the harness (WF-20), whichever
  sources they came from. mind warns at `learn` and reports a `review` finding,
  and installs both. This is the same check as WF-24 with the comparison made
  against every other installed workflow instead of against the item's own
  effective name, and it is reported at the same three sites.

  It is deliberately softer than the agent collision it otherwise resembles
  (NS-41), for two reasons. It is not a link-path collision: the two items link
  under distinct names (`workflows/<a>.js`, `workflows/<b>.js`), both installs
  are correct by every check mind makes, and there is no link being silently
  repointed to prevent. And the duplicate is read out of file content by a reader
  that is not the harness's (WF-5), which WF-31 forbids from blocking an install.
  A prefix does not avert it, as WF-22 records.

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

Not a requirement, recorded so the omission is deliberate: the harness also
declines to load workflows for reasons that have nothing to do with a file. The
feature is gated by a session toggle and can be turned off wholesale by managed
settings (`disableWorkflows`) or by an org policy (`allow_workflows`), and under
any of those a correctly installed, correctly named workflow is simply absent.
mind does not read the harness's settings and reports none of this. The flag is
not a property of the item, and predicting an experimental feature's gating would
couple the store to a config shape mind has never depended on. This is the WF-11
posture: state the assumption, do not defend it.

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
  (HOOK-80), ignore patterns (IGN-1), `absorb` (whose convention path for the
  kind is `workflows/<name>.js`), `dump`, `probe`/`recall` listing, and
  unmanaged-item detection in a lobe's `workflows/` directory (UNM-1). As with
  commands (CMD-8), `absorb` and unmanaged detection see only the immediate
  `.js` children of a lobe's `workflows/` directory, matching the flat convention
  scan (WF-2).

  A workflow's hooks can come only from a root `mind.toml` `[[items]]` entry.
  The frontmatter scalars (HOOK-130) need frontmatter, which a `.js` file has
  none of, and a scoped item `mind.toml` (HOOK-131) is read only from a
  directory-backed item's own directory, which a one-file kind does not have.
  This is the agent, rule, and command position, not a new restriction.
- `WF-51` `probe` surfaces a workflow's `whenToUse` (WF-5) where an item's
  description is shown, appended to the description as the harness's own
  workflow list renders it (`<description> - <whenToUse>`). It is a field of its
  own on the catalog item, not folded into the description at scan time: a
  `mind.toml` `[[items]].description` override (WF-4, DSC-32) replaces the
  description and leaves `whenToUse` standing, which folding would make
  impossible to express. The field is display-only. It is not recorded in the
  manifest, and `dump` does not emit it, `dump` emitting no item description at
  all. No other kind has a second description field; a workflow without one is
  unaffected.

  The appending is done once, at the one accessor every display surface reads,
  `--json` included. `whenToUse` is in no persisted file, so a JSON consumer
  given the bare description would have no remaining way to see it.

  `recall` does not show it. `recall` reports INSTALLED items and reads them
  from the manifest, which is where the description it prints was captured at
  install time; it does not scan the catalog, so there is no `whenToUse` in
  hand at that point. Surfacing it there would mean recording it in the
  manifest -- persisting a display string, for one kind, that nothing else
  reads -- and that is a worse trade than the omission. `probe`, which scans,
  is the browsing surface the harness's own workflow list corresponds to.
- `WF-52` `workflow` becomes a reserved namespace prefix, appended to the NS-29
  list, which is append-only for exactly this case. A source whose `[source]
  .prefix` is the literal string `workflow` is refused from that point on, with
  the same `ReservedPrefix` error the other kind words produce.
