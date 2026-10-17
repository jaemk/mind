# Share data files

A resource is a file or directory a source's items read at runtime: a JSON
allowlist, a prompt template, a reference document, a folder of examples. It
installs to the store and is reached by the `{{resources:name}}` token, the same
way a [tool](tooling.md) is reached by `{{tools:name}}`. The
[tooling example](https://github.com/jaemk/mind/tree/main/examples/tooling)
ships one.

## Layout

Put each resource directly under `resources/`:

```
resources/
  allowlist.json        resource:allowlist.json
  reviewer-prompt.md    resource:reviewer-prompt.md
  templates/            resource:templates (the whole directory)
    pr.md
skills/review/
  SKILL.md              ... {{resources:allowlist.json}} ...
```

- One entry is one resource. A file keeps its full name, extension included,
  so `allowlist.json` and `allowlist.yaml` are different resources. A directory
  is one resource holding everything under it.
- Dot-entries (`.gitkeep`) and symlinks are skipped.
- A resource has no metadata file. Nothing is read from its content: a
  `description:` line in a markdown resource's frontmatter is just text.

## Referencing a resource

Write `{{resources:name}}` where an item needs the path:

```markdown
Check the command against {{resources:allowlist.json}} before running it.
Fill in the template at {{resources:templates}}/pr.md.
```

After `mind learn skill:review`, those read:

```markdown
Check the command against ~/.mind/store/resource/allowlist.json before running it.
Fill in the template at ~/.mind/store/resource/templates/pr.md.
```

The token follows the same rules as the other path tokens (see
[Share helper scripts](tooling.md#path-reference-tokens)):

- It expands in markdown files, in a workflow's `.js`, and in files listed
  under `expand:`.
- It renders with a leading `~` so a `Bash(~/.mind/store/**)` permission rule
  matches.
- It resolves within the same source only.
- A name that matches no resource fails the install, and `mind review` reports
  it as `bad-reference`.

`{{path:resource:name}}` is a longer spelling of the same path.

## Which resources are offered

Under convention discovery (no authoritative `mind.toml`), a resource is
offered only when an item in the same source references it, directly or
through another offered resource. `resources/` is a common directory name, and
an entry nothing points at (a project's install script, images for a README)
would have no reader once installed.

`mind review` lists each entry it skipped:

```
advisory [unreferenced-resource]: resources/install.sh is not referenced by any item, so it is not offered; an item reaches it by writing {{resources:install.sh}}
```

If your prose mentions `resources/allowlist.json` as a plain path, that advisory
is the signal to switch it to `{{resources:allowlist.json}}`.

An authoritative `mind.toml` offers every resource it declares, referenced or
not:

```toml
[[items]]
kind = "resource"
name = "notes"
path = "data/notes.txt"
description = "Team notes"

[discover]
resources = { include = ["assets/*"] }   # each match is a file or directory
```

A resource cannot declare `link`, `bin`, or `build`.

## Installing and updating

A resource is a dependency of every item that references it, so learning the
item installs it:

```
$ mind learn skill:review
skill:review  [selected]
  resource:allowlist.json  [dependency]
  resource:templates  [dependency]

Install these 3 items? [y/N]
```

From there it behaves like any installed item:

- It is store-only: the store copy lives at `~/.mind/store/resource/<name>`,
  and nothing is linked into an agent home.
- `mind upgrade` picks up content changes.
- `mind forget resource:<name>` removes it.
- A namespace prefix applies, for example `resource:jk:allowlist.json`. The
  tokens follow the rename.

## Resource or tool?

| | tool | resource |
|-|------|----------|
| shape | a directory | a file or a directory |
| token | `{{tools:name}}` (the entrypoint) | `{{resources:name}}` (the path) |
| metadata | optional `TOOL.md` (`description`, `bin`, `build`) | none |
| offered by convention | always | only when referenced |

Use a tool for something an item runs, a resource for something it reads. A
file only one skill uses can stay in that skill's directory and be reached with
`{{self}}`.
