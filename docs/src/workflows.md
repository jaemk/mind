# Workflows

A workflow is a JavaScript file the Claude Code harness loads from
`~/.claude/workflows/` and offers to its `Workflow` tool. It declares
`export const meta = {...}` and a body that orchestrates subagents with
`agent()`, `parallel()`, `pipeline()`, `phase()`, and `log()`.

The harness wraps that body when it runs a workflow, so top-level `await`, a
top-level `return`, and references to harness-injected globals (`phase`,
`agent`, `parallel`, and the rest) are normal there, even though they make the
raw file invalid as a standalone ES module: a `node --check` or bundler run
against it rejects it for exactly that reason. `mind` never runs or validates
the body, so this only explains why the shipped example files "don't validate"
as plain JS.

`mind` treats it as a sixth item kind, discovered and installed like any other:

```
<repo>/
  workflows/review-changes.js
```

```text
$ mind learn workflow:review-changes

~/.mind/store/workflow/review-changes     the copy
~/.claude/workflows/review-changes.js     the symlink
```

The scan is flat and the extension is exactly `.js`, matching the harness's own
loader: a `workflows/nested/deep.js`, a `.mjs`, or a `.ts` is not an item.

## Descriptions come from `meta`

A `.js` file has no YAML frontmatter, so the item's description comes from the
`meta` object instead. `probe` shows `whenToUse` beside it, the way the
harness's own workflow list renders the pair; `recall` reads an installed
item's description from the manifest, where `whenToUse` is never recorded, and
shows the description alone.

```js
export const meta = {
  name: 'review-changes',
  description: 'Review changed files across dimensions',
  whenToUse: 'before opening a PR',
}
```

```text
$ mind probe --no-tui review
  workflow:review-changes   agents  ab12cd3  Review changed files across dimensions - before opening a PR
```

A `mind.toml` `[[items]].description` still overrides the description, and leaves
`whenToUse` standing.

The reader is minimal, not a JavaScript parser, and it recognizes exactly one
opening form: a literal `export const meta = {` (whitespace or a `//` / `/* */`
comment between the tokens is fine), immediately followed by an object
literal. A wrapping call (`export const meta = Object.freeze({...})`), a
missing `export`, a spread expression, or an interpolated/templated value are
not recognized: each of these reads as no `meta` found, the same as a file that
has none at all. Where it cannot read a value the item simply lists without a
description; it never fails a scan or an install on the shape of a workflow's
code. The harness's own reader is the authority on what actually loads.

## What mind reports but will not enforce

The harness skips a workflow with no readable `meta`, a missing or empty `name`
or `description`, or a file over 524288 bytes. `review` reports each as a
`workflow-unloadable` advisory, and `learn` and `upgrade` both warn and install
it anyway: an upstream source that introduces the defect between two syncs is
the case most likely to surface it at `upgrade`.

```text
$ mind review ./agents
advisory [workflow-unloadable]: workflow:deploy: the harness will not load this
  workflow: `meta.description` is missing
```

`review` also emits a `workflow-content` advisory for every workflow item,
unconditionally: it is a disclosure that mind neither reads nor validates a
workflow's body, not a defect signal, and it fires whether or not the file has
any problem at all. It will inflate any CI gate that just counts `review`
advisories.

Nothing here is a gate. mind does not judge item content for any other kind,
and a disagreement between the two readers is not a reason to refuse an
install. The phrasing above ("the harness will not load this workflow")
describes what mind itself could or could not read, not a guarantee about the
harness's own behavior on that file: mind has no way to run the harness's
loader to check. The size cap is the harness's, so mind reports an overage
rather than enforcing a limit of its own.

## The name the harness answers to

The harness keys a workflow by the `name` in its `meta` object, not by its file
name. An item's name in `mind` is its file stem, as for every other file-shaped
kind, so the two can diverge.

Write the namespace token in `meta.name` to keep them in step:

```js
export const meta = {
  name: '{{ns:review-changes}}',
  description: 'Review changed files across dimensions',
}
```

It expands at install to the effective name (`review-changes` unprefixed,
`jk:review-changes` under a namespace), which is also the spelling the harness
uses for a plugin's workflow. So the installed file carries a plain string
literal that matches the name `mind recall` reports.

When the two do not match, `learn` and `upgrade` both say so and install
anyway, `review` reports it as `workflow-name`, and `mind recall
workflow:<name>` keeps showing it:

```text
$ mind recall workflow:review-changes
workflow:review-changes
  ...
  harness the harness resolves it as 'deploy', not 'review-changes' -- ...
```

Two workflows whose `meta.name` agree are one workflow to the harness. Both
install; both are reported, the same four places -- `review` names the finding
`workflow-name-collision`. One shared name is one report, naming the other
claimants; past a few it names the first three and counts the rest, so a source
with many same-named workflows produces one bounded message rather than one
report per file.

A workflow has no frontmatter, so the `requires:` key
([Dependencies](dependencies.md)) is not available to it. Its only dependency
channel is a `{{ns:}}` token in its body: that is the one token family that
pulls a referenced sibling into the install closure, the way `requires:` does
for a markdown item. All four token families still expand in a workflow file,
not just in `meta`, since a workflow's strings are agent prompts:

```js
agent(`follow the checklist in {{path:skill:review}}/SKILL.md`)
```

A path token renders in the `~` home form there, the reading an agent gets, not
the absolute form an [`expand:`-listed script](tooling.md) gets. Note the
footgun in the example above: `{{path:}}`, unlike `{{ns:}}`, forms no
dependency edge, so this workflow can install with `skill:review` absent.
Write `{{ns:review}}` instead of, or alongside, the path token if the workflow
should actually depend on that skill.

One consequence: only four openers are live tokens in a workflow's code --
`{{ns:`, `{{self}}`, `{{tools:`, and `{{path:` -- the same families a markdown
item expands. Anything else starting with `{{` (a template literal's `${{`, or
ordinary JS object-literal syntax that happens to contain `{{`) passes through
untouched: it is not scanned for and never read as a token. That is the same
rule markdown items have lived under, and it has the same consequence for
these four forms: one that resolves to no sibling is a hard install failure
(`BadReference`), not inert text.

`review` reports a bad reference in a workflow the same as in any markdown
item, but `review --fix` will not rewrite a `.js` file: the rewrite passes
match sibling names as words, and a workflow is code, so an automated rewrite
risks corrupting it. Fix a flagged workflow reference by hand.

## Plugins

A Claude plugin's `workflows/` directory maps to the kind the same way its
`commands/` does, on a directly melded `.claude-plugin/plugin.json` and on each
in-repo entry of a marketplace catalog:

```text
$ mind probe --no-tui deploy
  workflow:acme-tools:deploy   acme-tools  ab12cd3  Stage, verify, and cut a release - when a release is ready to go out
```

The harness names a plugin's workflow `<plugin>:<meta.name>`, and a plugin's
items take the plugin name as their default namespace. So the `{{ns:}}` token
expands to exactly that spelling. See
`examples/marketplace-plugin/workflows/deploy.js`.

A manifest's `workflows` key points elsewhere on purpose, and `mind` ignores it
as it ignores every other component-path override, so the harness loads from
the declared path while `mind` scans only `workflows/`, and the two disagree.
A plugin that sets the key contributes no workflow items from the declared
path, and the skipped-components note says nothing about it. Setting the key
is not harmless.

A workflow cannot be installed by item link either: a deep link's `blob` form
names a single `.md` file and its `tree` form names a skill directory, and
neither matches a `.js` workflow file, so `mind` refuses the link by name. Meld
the repo, or `learn workflow:<name>`, instead.

## Lobes

A workflow links into the default Claude lobe and into any lobe whose `kinds`
filter names `workflow`. A lobe with no filter admits it. The non-Claude harness
presets (gemini, codex, universal, windsurf) admit skills only, so they are
unaffected. That includes `link-project`, which resolves to one of those presets
(windsurf by default) or, with `--subdir`, to a skill-only filter: neither links
a workflow.

A project lobe still needs nothing special, but it has to be one that admits the
kind. The harness reads project workflows from `<project>/.claude/workflows/`,
and the lobe that links one there is the bare form:

```text
$ mind config lobes add ./myproject/.claude
```

Be aware this registers an unfiltered lobe: with no `kinds` restriction it also
backfills every already-installed skill, agent, rule, and command into
`./myproject/.claude/`, not just workflows -- a bigger effect than "add a
project workflow lobe" asks for. No CLI flag narrows this: every preset and
every `--subdir` form is skill-only. To get a workflow-only project lobe, add
the entry by hand in `~/.mind/config.toml` with a `kinds` restriction instead:

```toml
lobes = ["~/.claude", { path = "./myproject/.claude", kinds = ["workflow"] }]
```
