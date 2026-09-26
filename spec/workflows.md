# The `workflow` item kind

Status: done. Claude Code reads user-authored workflows from a `workflows/`
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

  The declaration is recognized at STATEMENT POSITION only (WF-57): the reader
  is a token scanner rather than a parser, so that rule is what keeps it from
  mistaking a member expression for a declaration.

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
  (HARN-1). `workflow` is also the `probe`/`recall --kind` filter value, and
  `workflow:<name>` is an item ref wherever a ref is taken.

  One CLI kind flag excludes it: `--kind` on `meld`/`learn` is the ITEM-LINK
  kind flag, and it takes `agent`, `rule`, or `command` only. An item link's
  blob form names a single `.md` file (LNK-20), and a workflow is a `.js` file,
  so a workflow cannot be item-linked at all. An item-link path naming one is
  refused by name, saying workflows are deliberately unsupported and pointing at
  melding the repo or `learn workflow:<name>`, rather than reporting the
  directory as unrecognized.

  "Naming one" is the convention shape (WF-1): a `.js` file directly under a
  `workflows/` directory. Any OTHER `.js` path is refused as well, since the
  link form takes `.md` files only, but with a message that says just that and
  does not call the file a workflow. Nothing about `lib/util.js` says workflow,
  and the workflow message's remedy (`learn workflow:<name>`) would name an
  item that does not exist.
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
  lobe already links a `workflow` item by WF-10. It has to be a lobe that admits
  the kind, and `link-project` never produces one: it resolves to a preset
  (windsurf by default, CLI-198) or, with `--subdir`, to a skill-only filter, and
  both admit skills alone by WF-12. The project lobe that links a workflow is the
  bare `config lobes add <project>/.claude`, which carries no filter. There is no
  `claude` preset, so this is not a gap a preset could close.

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
  once at `learn`, once at `upgrade` (where a new version of a source can
  introduce the divergence into an item that did not have one), as a `review`
  finding beside the WF-30 ones, and in the
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

  A workflow with no readable `meta.name` does not diverge: it is unloadable,
  which WF-30 reports, and a second warning saying it also answers to the wrong
  name would be one defect reported twice. The same holds for a `meta.name` whose
  token resolves to no sibling, which is already the hard `bad-reference` of
  WF-27.

  At `learn`, `upgrade`, and `recall <item>` the comparison reads the INSTALLED
  copy, where the tokens are already expanded, so it is against the literal
  string the harness will read and there is no second expansion to keep in step with
  `install.rs`. `review` has no installed copy and expands `meta.name` itself,
  against the prefix and sibling set it already validated the file's other tokens
  with. The `review` finding is tagged `workflow-name`, and WF-29's
  `workflow-name-collision`.

  The warning carries the remedy as a token to write, and that token names the
  item's BARE name: `{{ns:}}` resolves against bare sibling names (NS-11), so
  the prefixed spelling would name no sibling and an author who followed the
  advice would trade an advisory warning for a hard `bad-reference` that fails
  the install (NS-12). The token renders as the effective name once expanded,
  which is what makes it the fix.
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
  sources they came from. mind warns at `learn` and at `upgrade` and reports a
  `review` finding, and installs both. This is the same check as WF-24 with the
  comparison made against every other installed workflow instead of against the
  item's own effective name, and it is reported at the same four sites.

  It is deliberately softer than the agent collision it otherwise resembles
  (NS-41), for two reasons. It is not a link-path collision: the two items link
  under distinct names (`workflows/<a>.js`, `workflows/<b>.js`), both installs
  are correct by every check mind makes, and there is no link being silently
  repointed to prevent. And the duplicate is read out of file content by a reader
  that is not the harness's (WF-5), which WF-31 forbids from blocking an install.
  A prefix does not avert it, as WF-22 records.

  The comparison set differs by site, because the installed set is not every
  site's subject. `learn`, `upgrade`, and `recall <item>` compare against
  everything installed, which is what "two installed workflows" means. `review`
  compares the reviewed source's own workflows against each other: its target is
  a source, not the host, and by design usually a repo the user has not yet decided to trust,
  so reporting what it would collide with once installed would answer a question
  `review` was not asked. Two workflows in one source that already share a name
  are a defect in the source, which is exactly what `review` is for.

  One shared name is ONE report, naming the claimants, not one report per
  claimant (WF-59).

## Reporting a workflow the harness will not load

- `WF-30` `review` reports a workflow the harness would skip, as a
  `workflow-unloadable` advisory finding alongside its existing hook and
  reference findings: no readable `meta`, a missing or empty `name` or
  `description`, a `meta.name` mind cannot use (WF-58), or a file over the WF-7
  size cap. Such a workflow does not exist
  as far as the harness is concerned, and nothing else in mind's model would say
  so. Each condition is its own finding, except an unreadable `meta`, which is
  one finding and not also a missing `name` and a missing `description`.

  An empty finding list is not a promise the file loads. The harness's reader is
  stricter than mind's and is the authority (WF-5), so this reports what mind can
  see and no more. That asymmetry is why it is a report and not a gate.

  Each reason says what MIND read, not what the file contains. An unreadable
  `meta` reports that mind read no `name`, `description`, or `whenToUse` from
  it, rather than that the file declares no `meta` object: `export const meta =
  {}` does declare one, and a file mind did not read at all is WF-56's case, not
  this one.
- `WF-31` `learn` warns on the same condition and installs anyway. mind does not
  gatekeep the validity of item content for any other kind, and the harness's own
  reader is the authority (WF-5); a reader disagreement must not be able to
  block an install.
- `WF-32` The size cap is reported, not enforced, for the same reason: DSC-90
  records that mind does not cap the size of item content it reads, and a cap
  mind enforced would be mind's cap, not the harness's.
- `WF-55` A workflow past mind's own metadata read cap (DSC-91, 8 MiB by
  default) does not
  fail the scan. The read stays capped, since a source is untrusted and bounding
  it is the point of the cap, but for this kind an over-cap file reads as NO
  readable metadata rather than as an error: the item is catalogued with no
  description, it is reported as the unread file it is (WF-56), and it installs
  like any other unloadable workflow (WF-31).

  The kind needs the exception because a workflow's metadata IS its whole `.js`
  body (WF-4, WF-5). For every other kind the capped read covers a small header
  file beside the content, where an 8 MiB one is a defect worth stopping on; here
  it covers a large program, which WF-5 forbids failing a scan over. The failure
  was also not confined to the file: a scan builds a source's entire item list in
  one pass, so one oversized `.js` took every other item of that source with it,
  for `review` and for every verb that scans a catalog. A healthy sibling
  workflow got no disclosure and no check at all.

  Only the workflow kind is excepted. DSC-91's hard `metadata-too-large` stands
  for every other kind, and for every other capped read (`mind.toml`, a plugin
  manifest, an item link's frontmatter probe).
- `WF-56` An over-cap read (WF-55) is reported as MIND's cap, separately from
  every WF-30 reason and never as one of them. `review` emits it as its own
  `workflow-unread` advisory finding and `learn`/`upgrade`/`recall <item>` say
  it in their own words; the message names the effective cap and the
  `--max-metadata-size` flag that raises it, and asserts nothing about whether
  the harness would load the file.

  The distinction is not cosmetic. The cap is configurable and DSC-103 exists
  partly so a cautious operator can LOWER it for untrusted sources, including
  below the harness's own 524288-byte limit (WF-7). Under a lowered cap a
  perfectly loadable workflow is unread, and reporting "the harness will not
  load this workflow: it declares no `meta`" would be mind stating, as a fact
  about someone else's file, a consequence of mind's own configuration. What
  mind can honestly say is that it read nothing.

  The WF-7 overage still reports beside it when the file is also over the
  harness's cap: that reason is read off the file's size, not its content, so it
  survives a read that never happened. The other WF-30 reasons do not: mind read
  no `meta`, so it knows nothing about the `name` or `description` in it.
- `WF-57` mind's `meta` reader recognizes the `export const meta` declaration at
  STATEMENT POSITION only: the `export` must be the first token of a statement
  (start of file, or after `;`, `}`, or a line break) at bracket-nesting depth
  0, and nothing but whitespace and comments may separate the three words.

  The reader is a token scanner, not a parser (WF-5), so without this rule it
  matched the three identifiers with any punctuation between them skipped, and
  a member expression assignment matched:

  ```js
  shim.export.const.meta = { name: 'decoy', description: 'x' }
  export const meta = { name: 'real', description: 'y' }   // never reached
  ```

  The scan stops at its first hit, so the decoy did not merely add a wrong
  reading, it replaced the right one: mind reported `decoy` and checked WF-24
  and WF-29 against it while the harness registered the workflow as `real`. A
  hostile source could therefore choose what mind believes a workflow answers
  to. The depth-0 half of the rule carries the same weight in the other
  direction: a `meta` declared inside a block or a call argument is not the
  module-level declaration the harness reads, so it must not stand in for one.

  The depth half is a bracket count, and the reader does not model every
  construct a bracket can hide in (a regex character class, above all), so a
  bracket it never sees closed leaves the count above zero and a declaration
  after it reads as absent. That is the tolerable direction of error, and the
  one WF-5 already admits: mind reports the file as one it read no `meta` from
  (WF-30), the harness loads it regardless, and nothing fails. The error this
  rule prevents is the other one, where mind reports a name the harness does not
  use.
- `WF-58` A `meta.name` carrying a control or invisible code point is not a
  usable harness name: mind treats it as absent (no WF-24 divergence, no WF-29
  claim) and WF-30 reports it as its own reason.

  This is the same rule `catalog.rs` already applies to an item name (DSC-95's
  character class), applied at the one other place a source-controlled name
  enters mind's model. It has to be, because the WF-29 comparison is exact while
  every message prints the name sanitized: `revi<U+200B>ew` prints as `review`,
  collides with nothing, and composes the self-contradicting "the harness
  resolves it as 'review', not 'review'". Reporting it as unusable says what is
  actually wrong.

  The test applies to the name mind would USE, which is the trimmed one (WF-20):
  surrounding whitespace is not part of the name, and a `meta.name` written as a
  template literal spanning lines carries a leading and trailing newline as a
  matter of course. Reporting one of those as a name mind cannot use, while mind
  goes on using its trimmed form, would be the same kind of self-contradiction
  this requirement exists to remove.
- `WF-59` One shared harness name produces ONE report, whatever the number of
  claimants: the report names the colliding name, a bounded list of the
  claimants, and a count of any it did not name.

  The alternative reports each claimant separately, so n workflows sharing a
  name cost n findings each carrying the other n-1 keys: quadratic output from
  linear input, in a check whose input is a file count an untrusted source
  chooses. One name is one defect, and a reader acts on the first few claimants.
- `WF-60` `recall <item> --json` carries the workflow's harness-facing name and
  the findings about it: `harness_name` (null when mind read no usable one) and
  `workflow_findings`, an array of the same message strings the text view
  prints on its `harness` lines.

  The manifest records neither `meta.name` nor the resolved harness name
  (WF-51's reasoning about `whenToUse` applies: a display string for one kind is
  not persisted state), and the `--json` document is built from the manifest
  entry. Without these fields the machine-readable surface was the one place a
  WF-24 divergence and a WF-29 collision were invisible, which is backwards: a
  scripted consumer is exactly who cannot notice a warning it was never sent.
- `WF-53` `review` reports EVERY workflow item as a `workflow-content` advisory
  finding, the workflow counterpart of the command disclosure (CLI-237, DSC-91).
  A workflow is not content the harness offers, it is JavaScript the harness
  evaluates to drive subagents, and mind neither reads nor validates its body:
  the WF-5 reader looks at one object literal and nothing else. So a source
  shipping workflows must not review as a clean bill of health, and the finding
  is unconditional rather than pattern-triggered, since there is no subset of a
  program that is the dangerous part. It is disclosure, not a gate: it refuses
  nothing, and like every other check here it cannot block an install (WF-31).
  Being unconditional, the finding needs no read of the file to produce it, so
  an over-cap `.js` is disclosed like any other (WF-55). The one read mind makes
  of a workflow, the catalog scan's, stays size-capped where it happens
  (DSC-91), which is what `review`'s untrusted, not-yet-melded target requires.
- `WF-54` `review` does not emit the generic `missing-description` finding
  (CLI-132) for a workflow. A workflow's description comes from `meta` (WF-4)
  and a `.js` file has no frontmatter, so that finding's wording points the
  author at a site the kind does not have, and WF-30 already reports a missing,
  empty, or unreadable one as `workflow-unloadable` in the terms the harness
  applies. One defect is reported once, and `workflow-unloadable` is the single
  report. A `mind.toml` `[[items]].description` still overrides (DSC-32); it
  changes what mind displays, not what the harness reads, so supplying one does
  not clear a WF-30 finding.

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
  home's (WF-2, WF-3, WF-11). This holds on every path that reads a plugin: a
  directly melded `.claude-plugin/plugin.json` (MKT-3) and each in-repo entry of
  a marketplace catalog (MKT-14).

  The scan is the convention scan, so it is flat and `.js`-exact (WF-2, WF-3),
  and what it does not map is counted as a skipped component rather than dropped
  in silence (MKT-4): a `workflows/` subdirectory or a `workflows/deploy.ts` is
  reported as an unmapped `workflows/` entry. A mapped `workflows/<name>.js` is
  never counted, for the reason MKT-4 gives about commands: it is installed, and
  naming it in the message whose job is to say what was dropped would be false.
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

  This section holds only IDs in the WF-50 range. The reporting requirements
  continue at WF-53 under "Reporting a workflow the harness will not load"
  above, which is where WF-53..WF-60 live.

  A workflow's hooks can come only from a root `mind.toml` `[[items]]` entry.
  The frontmatter scalars (HOOK-130) need frontmatter, which a `.js` file has
  none of, and a scoped item `mind.toml` (HOOK-131) is read only from a
  directory-backed item's own directory, which a one-file kind does not have.
  This is the agent, rule, and command position, not a new restriction.

  A workflow's `requires` is restricted the same way, and more sharply: it can
  be declared only where a workflow can carry metadata, and there is no such
  site. The scan reads `requires` from an item's frontmatter, which a `.js` file
  has none of, and a root `[[items]]` entry has no `requires` key for any kind.
  A workflow therefore participates in DEP-4 with nothing to declare: it is
  resolved as a dependency of another item, and its own dependency list is
  always empty. Recorded here so the emptiness reads as a consequence of the
  kind's shape rather than as a scan that missed something.
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
