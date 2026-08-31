# Source layout

A source is a git repo `mind` melds. It offers items, discovered by convention or
declared in a `mind.toml`. The convention layout, which works on any repo with no
config:

```
<repo>/
  skills/<name>/SKILL.md     a skill (the whole directory is the item)
  agents/<name>.md           an agent
  rules/<name>.md            a rule
  commands/<name>.md         a slash command
  tools/<name>/              a tool (the whole directory; no anchor file)
  mind.toml                  optional: metadata, export control, odd layouts
```

The kinds:

- **skill**: a directory with a `SKILL.md` anchor. Bundled files (a `resources/`
  dir, scripts) ship with it.
- **agent** / **rule** / **command**: a single markdown file. A command is what
  the harness offers at the prompt as `/<name>`.
- **tool**: a directory of helper scripts or a compiled binary. A tool is
  store-only: other items reference it, and by default it is not linked into an
  agent home (a tool can opt in with an explicit `link`, see [Tooling](tooling.md)).

The command scan is flat: `commands/<name>.md`, no subdirectories. Claude Code
reads a grouped `commands/frontend/component.md` as `/frontend:component`, and
you get the same command name from a flat file by putting the colon in the name
(`commands/frontend:component.md`), which is also what a `mind` namespace
produces (`jk:review` links as `commands/jk:review.md` and runs as
`/jk:review`). Keep a real subdirectory only if you need it for Claude users
directly; then declare the item in `mind.toml` with a full `[[items]]` entry
(`kind`, `name`, `path`, and a `link` that preserves the nested path). Note
that adding any `[[items]]` or `[discover]` entry makes the manifest
authoritative for the whole repo: convention scanning turns off and only what
the manifest lists is offered, so a one-entry manifest silently drops every
other item. Declare everything you ship, or reach the nested file another
way. See [The mind.toml file](mind-toml.md). Note that Claude Code now treats
custom commands as skills: a `commands/` file and a `skills/<name>/SKILL.md`
both produce `/<name>`, and a skill is the better shape for something new.
See [Commands](commands.md#mental-model) for what happens when a source ships
both.

A group segment must not be a reserved kind word (`skill`, `agent`, `rule`,
`command`, `tool`). `commands/tool:build.md` installs as `tool:build`, which
an item ref reads as kind `tool`, name `build`, so `mind learn tool:build`
fails with a not-found error naming `build`. Spell it `command:tool:build`,
or pick another group name.

A `mind.toml` is optional enrichment, never a gate. It carries source metadata, a
namespace `prefix`, and (when you need it) explicit `[[items]]` or `[discover]`
globs for non-standard or monorepo layouts. See
[examples/](https://github.com/jaemk/mind/tree/main/examples).

A repo published for Claude Code's plugin system needs no changes either: a
`.claude-plugin/plugin.json` or `.claude-plugin/marketplace.json` is read as a
discovery input. The manifest supplies the plugin's name and metadata; `mind`
maps the plugin root's conventional `skills/`, `agents/`, and `commands/`
directories to items. See
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

Tokens expand only in markdown files. A token in a bundled script (a
`resources/pr.py`) is left literal by default. To expand it there, list the file
in the item's `expand:` frontmatter, so a script can locate its tooling without a
language-specific self-locate; see [Tooling and shared scripts](tooling.md).

## Hardcoded paths

`mind learn` copies an item into the store (`~/.mind/store/<kind>/<name>`) and
symlinks it into each agent home (`~/.claude/skills/<name>`, `agents/<name>.md`,
`rules/<name>.md`, `commands/<name>.md`). A tool is the exception: it is
store-only and, by default, not linked into an agent home.

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
