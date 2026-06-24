# Source layout

A source is a git repo `mind` melds. It offers items, discovered by convention or
declared in a `mind.toml`. The convention layout, which works on any repo with no
config:

```
<repo>/
  skills/<name>/SKILL.md     a skill (the whole directory is the item)
  agents/<name>.md           an agent
  rules/<name>.md            a rule
  tools/<name>/              a tool (the whole directory; no anchor file)
  mind.toml                  optional: metadata, export control, odd layouts
```

The kinds:

- **skill**: a directory with a `SKILL.md` anchor. Bundled files (a `resources/`
  dir, scripts) ship with it.
- **agent** / **rule**: a single markdown file.
- **tool**: a directory of helper scripts or a compiled binary. A tool is
  store-only: other items reference it, but it is never linked into an agent home.

A `mind.toml` is optional enrichment, never a gate. It carries source metadata, a
namespace `prefix`, and (when you need it) explicit `[[items]]` or `[discover]`
globs for non-standard or monorepo layouts. See
[examples/](https://github.com/jaemk/mind/tree/main/examples).

## Where shared helpers belong

A helper used by a single skill lives in that skill's own directory and is
addressed with `{{self}}`:

```
skills/review/
  SKILL.md           # ... run {{self}}/resources/pr.py ...
  resources/pr.py
```

A helper used by more than one item is a **tool**. Put it once under
`tools/<name>/` and reference it by token. Do not copy the same script into each
skill: `mind review` and `mind init-source` flag a byte-identical helper carried
by two or more items as a `duplicate-tooling` advisory.

```
tools/detect/detect          # the shared script, shipped once
skills/a/SKILL.md            # ... {{tools:detect}} ...
skills/b/SKILL.md            # ... {{tools:detect}} ...
```

## Reference items by token, not by path

Items reference each other with tokens that `mind` expands at install. Tokens
survive a namespace prefix and resolve to the right location regardless of how
many agent homes are configured.

| token | expands to |
|-------|-----------|
| `{{ns:name}}` | a sibling item's effective name (use in prose, e.g. "hand off to `{{ns:dev}}`") |
| `{{self}}` | the item's own store directory (its bundled resources) |
| `{{tools:name}}` | a sibling tool's entrypoint |
| `{{path:ref}}` | a sibling item's store directory, for a non-entrypoint file (`{{path:tool:detect}}/lib.sh`) |

References resolve within the same source only: ship a tool in the same source as
the items that use it.

## Why hardcoded paths break

`mind learn` copies an item into the store (`~/.mind/store/<kind>/<name>`) and
symlinks it into each agent home (`~/.claude/skills/<name>`, `agents/<name>.md`,
`rules/<name>.md`). A tool is the exception: it is store-only and never linked
into an agent home.

So a hardcoded path behaves differently depending on what it points at, and `mind
review` classifies each case (the `hardcoded-path` advisory):

- A skill referencing its **own resources** by an agent-home path
  (`~/.claude/skills/<self>/resources/x`) resolves through the skill's symlink
  today, but breaks the moment a prefix renames the item (`<prefix>-<self>`) or a
  second agent home is configured. Fragile, not yet broken.
- A reference to a **tool** by an agent-home path never resolves: a tool is not
  linked there. Broken regardless of prefix.
- Any reference under a **prefix** points at the wrong effective name, since a
  literal path does not track the rename.

Use a token instead. It expands to the correct store path and keeps a leading `~`
when the store is under your home, so a Claude `settings.json` permission glob
such as `Bash(~/.mind/store/**)` matches the expansion.

`mind review` recognizes hardcoded paths written with `~`, `$HOME`, `${HOME}`, or
an absolute `/home/<user>` / `/Users/<user>` root, and `mind review --fix`
rewrites the ones that map confidently to a token.
