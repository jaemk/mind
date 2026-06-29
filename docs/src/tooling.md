# Tools and path tokens

Some agent libraries ship helper scripts or compiled binaries that their skills
and agents call at runtime. `mind` handles this with two mechanisms:

- **The `tool` kind**: a store-only installable for helpers that are shared
  across items or do not belong inside a single skill directory.
- **Path-reference tokens**: `{{self}}`, `{{tools:name}}`, and `{{path:ref}}`,
  expanded at install to stable store paths.

See the [worked example](https://github.com/jaemk/mind/tree/main/examples/tooling)
for a complete source you can browse.

## The `tool` kind (TOOL-1..7)

A `tool` is a fourth item kind alongside skill, agent, and rule. Its purpose is
to be referenced by other items; unlike a skill, by default it is not linked into
an agent home and the Claude harness does not discover it directly (TOOL-3). A
tool can opt in to a link with an explicit `link` field (TOOL-4, below).

**Convention discovery** (TOOL-1): every immediate subdirectory of a `tools/`
directory under a scan root is a tool. The whole directory is the item; no anchor
file is required. A bare `tools/detect/` is a tool named `detect`.

**Store path**: `~/.mind/store/tool/<effective-name>/`. The manifest records the
store path with an empty `links` set. `forget` removes the store copy; `recall`
reports it as installed (TOOL-3).

**Optional link**: a tool can declare an explicit `link` field in `mind.toml`
`[[items]]` to also surface it under an agent home, but the default is
store-only (TOOL-4).

**Namespacing** (TOOL-6): a prefix gives a tool the effective name
`<prefix>:<name>`, the same as any kind. Stable identity is
`(source, kind, bare_name)`, so a prefix change is a rename matched by
`evolve`/`introspect`.

### When to use a `tool`

A helper that is bundled with a single skill (a `resources/` script) does not
need to be a tool. Put it in the skill directory and address it with `{{self}}`
(see [source layout](source-layout.md)). Use the `tool` kind when:

- The helper is shared across two or more items.
- The helper stands alone and is not logically part of any one skill.
- The helper must be compiled and the build belongs to the item rather than to a
  source-level install hook.

`mind review` treats a byte-identical helper copied into several items as a
`duplicate-tooling` advisory (informational), suggesting it as a `tool`
candidate. Keeping per-item copies is equally valid.

### `TOOL.md` frontmatter (TOOL-2, TOOL-5)

Place a `TOOL.md` in the tool directory to declare metadata:

```markdown
---
description: Detect the project type from files in the current directory.
bin: detect.sh
build: make
---
```

| key | meaning |
|-----|---------|
| `description` | shown in `recall` / `probe` |
| `bin` | the entrypoint; what `{{tools:name}}` resolves to (TOOL-5) |
| `build` | per-item build command run in staging at install (HOOK-70..73) |

The entrypoint resolution order is (TOOL-5):

1. `mind.toml` `[[items]].bin`
2. `TOOL.md` frontmatter `bin:`
3. Convention: a file named after the tool at the tool dir root (e.g.
   `tools/detect/detect`) when it is present in the source

A tool that nothing invokes as an executable need not resolve a `bin`.

A `mind.toml` `[[items]]` entry overrides `TOOL.md` fields. With neither, a tool
has no description and no explicit `bin` or `build`. For the `[[items]]` form and
`[discover].tools` globs (which match the tool directory, not an anchor file) see
[The `mind.toml` file](mind-toml.md).

The `bin` and `build` fields are valid only on a `tool`; on any other kind they
are a `mind.toml` schema error (TOOL-7).

### Build hooks for compiled tooling

The common case is a source-level install hook (`[[hooks]]` in `mind.toml`) that
builds all tooling once at `meld`, producing artifacts in the working tree; the
tool directory is then copied as its final state at `learn` (TOOL-1). Use a
per-item `build` when you want an isolated, rollback-safe build that runs in
staging before the store swap. See [Install hooks](install-hooks.md) for the full
hook model (HOOK-70..73).

---

## Path-reference tokens (TOOL-10..16)

Tokens expand at install to stable store paths. Expansion runs in the same
staging pass that expands `{{ns:}}` (see
[spec/namespacing.md](https://github.com/jaemk/mind/blob/main/spec/namespacing.md)),
so a bad reference fails before the live install is touched. The recorded content
hash is of the token (source) form, so drift detection compares source with
source (TOOL-13).

Tokens expand in every text file of every item kind, including tool directories
and bundled scripts (TOOL-14). Inner whitespace is trimmed (`{{ path:x }}` works);
an unterminated token (no closing `}}`) is left verbatim; non-UTF-8 files are not
scanned.

References resolve within the same source only: ship a tool in the same source
as the items that use it (TOOL-15).

### `{{self}}` (TOOL-10)

Expands to the item's own store directory. Available in every kind.

```
{{self}}/resources/helper.sh
```

A skill addresses its bundled resources with `{{self}}` without hardcoding its
installed name. Without `{{self}}`, a prefix rename (e.g. `voice` to `jk:voice`)
would silently point at the wrong path.

### `{{tools:name}}` (TOOL-12)

Expands to a sibling tool's entrypoint: the tool's store directory joined with
its resolved `bin` (TOOL-5). The plural `tools:` is distinct from the `tool:`
kind-qualifier used in `{{path:}}`.

```
{{tools:detect}}
```

A `name` that is not a sibling tool, or a tool with no resolvable `bin`, is a
`BadReference` error.

### `{{path:ref}}` (TOOL-11)

Expands to a sibling item's store directory, for reaching non-entrypoint files.
`ref` is a bare sibling name, optionally kind-qualified:

```
{{path:tool:detect}}/lib.sh
{{path:skill:review}}/resources/pr.py
```

An unqualified `ref` that matches items of more than one kind is a `BadReference`
error. A `ref` matching no sibling is also a `BadReference` error.

`{{tools:name}}` is shorthand for `{{path:tool:name}}/<bin>`: use `{{tools:name}}`
when you want the entrypoint, `{{path:tool:name}}` when you want the directory.

### Tilde rendering (TOOL-16)

A token renders the store root with a leading `~` when the store is under the
user's home directory (the default `~/.mind/store`). This keeps the expansion
matchable by a Claude `settings.json` permission glob:

```json
"Bash(~/.mind/store/**)"
```

An absolute path would not match that glob. When `MIND_HOME` points outside
home, or when the home directory cannot be determined, the token expands to the
absolute path.

---

## Worked example

Source tree (see
[examples/tooling](https://github.com/jaemk/mind/tree/main/examples/tooling)):

```
tools/detect/
  TOOL.md          # bin: detect.sh
  detect.sh        # the entrypoint
  lib.sh           # a library sourced by callers

skills/scan/
  SKILL.md
  resources/notes.md
```

`tools/detect/TOOL.md`:

```markdown
---
description: Detect the project type from files in the current directory.
bin: detect.sh
---
```

`skills/scan/SKILL.md` (excerpt):

```markdown
1. Run the shared tool: `{{tools:detect}} .`
2. Source its library: `. {{path:tool:detect}}/lib.sh`
3. Record the result: `{{self}}/resources/notes.md`
```

After `mind learn scan`:

- `{{tools:detect}}` expands to `~/.mind/store/tool/detect/detect.sh` -- the
  entrypoint from `bin:` (TOOL-5, TOOL-12).
- `{{path:tool:detect}}` expands to `~/.mind/store/tool/detect` -- the store
  directory, for non-entrypoint files (TOOL-11).
- `{{self}}` expands to `~/.mind/store/skill/scan` -- the skill's own store
  directory (TOOL-10).

All three use tilde syntax so a `Bash(~/.mind/store/**)` permission rule covers
them (TOOL-16).

Under a prefix (`meld --as jk`), `{{tools:detect}}` expands to
`~/.mind/store/tool/jk:detect/detect.sh` -- no edits to the skill needed.
