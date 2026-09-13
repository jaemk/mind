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

All four token families expand in a workflow file, not just in `meta`, since a
workflow's strings are agent prompts:

```js
agent(`follow the checklist in {{path:skill:review}}/SKILL.md`)
```

A path token renders in the `~` home form there, the reading an agent gets, not
the absolute form an [`expand:`-listed script](tooling.md) gets.

One consequence: a literal `{{` in a workflow's code is read as a token. That is
the same rule markdown items have lived under.

## Lobes

A workflow links into the default Claude lobe and into any lobe whose `kinds`
filter names `workflow`. A lobe with no filter admits it. The non-Claude harness
presets (gemini, codex, universal, windsurf) admit skills only, so they are
unaffected. A project lobe needs nothing special: the harness reads project
workflows from `<project>/.claude/workflows/`, which is where a project lobe
already links one.
