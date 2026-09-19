# Workflows

A workflow is a JavaScript file the Claude Code harness loads from
`~/.claude/workflows/` and offers to its `Workflow` tool. It declares
`export const meta = {...}` and a body that orchestrates subagents with
`agent()`, `parallel()`, `pipeline()`, `phase()`, and `log()`.

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

A `.js` file has no YAML frontmatter, so `probe` and `recall` read the item's
description out of the `meta` object instead. `whenToUse` is shown beside it, the
way the harness's own workflow list renders the pair:

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

The reader is minimal, not a JavaScript parser. Where it cannot read a value the
item simply lists without a description; it never fails a scan or an install on
the shape of a workflow's code. The harness's own reader is the authority on what
actually loads.

## What mind reports but will not enforce

The harness skips a workflow with no readable `meta`, a missing or empty `name`
or `description`, or a file over 524288 bytes. `review` reports each as a
`workflow-unloadable` advisory, and `learn` warns and installs it anyway:

```text
$ mind review ./agents
advisory [workflow-unloadable]: workflow:deploy: the harness will not load this
  workflow: `meta.description` is missing
```

Nothing here is a gate. mind does not judge item content for any other kind, its
`meta` reader is looser than the harness's, and a disagreement between two
readers is not a reason to refuse an install. The size cap is the harness's, so
mind reports an overage rather than enforcing a limit of its own.

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

When the two do not match, `learn` says so and installs anyway, `review` reports
it as `workflow-name`, and `mind recall workflow:<name>` keeps showing it:

```text
$ mind recall workflow:review-changes
workflow:review-changes
  ...
  harness the harness resolves it as 'deploy', not 'review-changes' -- ...
```

Two workflows whose `meta.name` agree are one workflow to the harness. Both
install; both are reported, the same three places.

All four token families expand in a workflow file, not just in `meta`, since a
workflow's strings are agent prompts:

```js
agent(`follow the checklist in {{path:skill:review}}/SKILL.md`)
```

A path token renders in the `~` home form there, the reading an agent gets, not
the absolute form an [`expand:`-listed script](tooling.md) gets.

One consequence: a literal `{{` in a workflow's code is read as a token. That is
the same rule markdown items have lived under.

## Plugins

A Claude plugin's `workflows/` directory maps to the kind the same way its
`commands/` does, on a directly melded `.claude-plugin/plugin.json` and on each
in-repo entry of a marketplace catalog:

```text
$ mind probe --no-tui deploy
  workflow:acme-tools:deploy   acme-tools  ab12cd3  Stage, verify, and cut a release
```

The harness names a plugin's workflow `<plugin>:<meta.name>`, and a plugin's
items take the plugin name as their default namespace. So the `{{ns:}}` token
expands to exactly that spelling. See
`examples/marketplace-plugin/workflows/deploy.js`.

A manifest's `workflows` key is a component-path override, and `mind` ignores it
as it ignores every other one: the convention directory is what gets scanned.
What that flat `.js` scan does not map (a subdirectory, a `.ts`) is counted in
the skipped-components note rather than dropped in silence.

## Lobes

A workflow links into the default Claude lobe and into any lobe whose `kinds`
filter names `workflow`. A lobe with no filter admits it. The non-Claude harness
presets (gemini, codex, universal, windsurf) admit skills only, so they are
unaffected. A project lobe needs nothing special: the harness reads project
workflows from `<project>/.claude/workflows/`, which is where a project lobe
already links one.
