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
  store-only: other items reference it, and by default it is not linked into an
  agent home (a tool can opt in with an explicit `link`, see [Tooling](tooling.md)).

A `mind.toml` is optional enrichment, never a gate. It carries source metadata, a
namespace `prefix`, and (when you need it) explicit `[[items]]` or `[discover]`
globs for non-standard or monorepo layouts. See
[examples/](https://github.com/jaemk/mind/tree/main/examples).

A repo published for Claude Code's plugin system needs no changes either: a
`.claude-plugin/plugin.json` or `.claude-plugin/marketplace.json` is read as a
discovery input, mapping the plugin's skills and agents to `mind` items. See
[Claude plugin marketplaces](marketplace.md).

## Where shared helpers belong

A helper used by a single skill lives in that skill's own directory and is
addressed with `{{self}}`:

```
skills/review/
  SKILL.md           # ... run {{self}}/resources/pr.py ...
  resources/pr.py
```

A helper used by more than one item has two good homes. Either:

- **An install hook puts it in a known location.** Declare a `[[hooks]]` install
  entry to run your install script, which installs the shared tooling wherever you
  want, and have your items call it there. This suits anything with a build step or
  a dependency to fetch, and a source onboards its build once.
- **A `tool` item shares it through the store.** Put it once under `tools/<name>/`
  and reference it by token (`{{tools:name}}`). `mind` carries it in the store and
  expands the token at install.

```
tools/detect/detect          # the shared script, shipped once
skills/a/SKILL.md            # ... {{tools:detect}} ...
skills/b/SKILL.md            # ... {{tools:detect}} ...
```

Copying a byte-identical helper into several items works too; `mind review` and
`mind init-source` note it as a `duplicate-tooling` advisory (informational, not a
defect) in case you would rather share it once.

## Referencing items and resources

To reference one item from another, `mind` provides tokens it expands at
install. They are useful mainly under a namespace prefix (which renames items) or
across multiple agent homes; an unprefixed single-home source can often just use
the name or a bundled path.

| token | expands to |
|-------|-----------|
| `{{ns:name}}` | a sibling item's effective name (use in prose, e.g. "hand off to `{{ns:dev}}`") |
| `{{self}}` | the item's own store directory (its bundled resources) |
| `{{tools:name}}` | a sibling tool's entrypoint |
| `{{path:ref}}` | a sibling item's store directory, for a non-entrypoint file (`{{path:tool:detect}}/lib.sh`) |

References resolve within the same source only: ship a tool in the same source as
the items that use it.

## Hardcoded paths

`mind learn` copies an item into the store (`~/.mind/store/<kind>/<name>`) and
symlinks it into each agent home (`~/.claude/skills/<name>`, `agents/<name>.md`,
`rules/<name>.md`). A tool is the exception: it is store-only and, by default,
not linked into an agent home.

A path you control is fine: pointing at a location your install hook populates
works as long as your hook and your items agree on it. What is fragile is
hardcoding `mind`'s OWN install layout, since that layout shifts under you. `mind
review` classifies those as the advisory `hardcoded-path` finding:

- A skill referencing its **own resources** by an agent-home path
  (`~/.claude/skills/<self>/resources/x`) resolves through the skill's symlink
  today, but breaks the moment a prefix renames the item (`<prefix>:<self>`) or a
  second agent home is configured. `{{self}}` generalizes it. Fragile, not broken.
- A reference to a **tool** item by an agent-home path never resolves: a tool is
  not linked there. Use `{{tools:name}}` (or install it elsewhere via a hook).
- Any reference under a **prefix** points at the wrong effective name, since a
  literal path does not track the rename.

A token keeps a leading `~` when the store is under your home, so a Claude
`settings.json` permission glob such as `Bash(~/.mind/store/**)` matches the
expansion.

`mind review` recognizes these install paths written with `~`, `$HOME`, `${HOME}`,
or an absolute `/home/<user>` / `/Users/<user>` root, and `mind review --fix`
rewrites the ones that map confidently to a token. The finding is advisory, so a
deliberate fixed-location-via-install-hook layout is your call.
