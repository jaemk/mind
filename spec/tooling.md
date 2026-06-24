# Resource and helper tooling

Status: done. Some agent libraries ship helper tooling -- shell or Python
scripts, or a compiled binary -- that their skills and agents invoke at runtime.
This document specifies how a source ships that tooling, how `mind` installs it
(including an optional build step for compiled tooling), and how an item
references it by a stable, namespace-safe path.

## Overview

A skill or agent often calls a helper script it ships next to itself
(`skills/voice/resources/mine-commits.sh`) or a shared helper used by several
items (`resources/detect-project.sh`). Copying the files is already handled --
`learn` copies a skill's whole directory -- so the unsolved parts are two others:

- **Reference.** The item must name the tooling's installed path. A hardcoded
  `~/.claude/skills/voice/resources/...` breaks the moment a prefix renames the
  item to `jk-voice`, and is ambiguous when more than one agent home is
  configured (which home?). This is the same fragility `{{ns:name}}` already
  fixes for sibling *names* (namespacing.md), one level down: a path, not a name.

- **Standalone and compiled tooling.** A helper shared across items is neither a
  skill, an agent, nor a rule, so no kind installs and tracks it. And compiled
  tooling must be built, with the output landing somewhere stable to reference.

`mind` answers these with: a `tool` item kind (a store-only installable that
exists to be referenced), path-reference tokens (`{{self}}`, `{{tools:name}}`,
`{{path:ref}}`, expanded at install like `{{ns:}}`), and an optional per-item
build hook (install-hooks.md, HOOK-70..73) for tooling that must be compiled.
Tokens expand to the store path (`~/.mind/store/...`), which is single and
home-independent, so a reference resolves the same regardless of how many agent
homes are configured or what prefix is in force.

The rest of this document states the rules normatively.

## The `tool` kind

- `TOOL-1` `tool` is a fourth item kind alongside skill, agent, and rule. By
  convention, every immediate subdirectory of a `tools/` directory (under each
  scan root) is a tool; the whole directory is the item. Unlike a skill
  (`skills/<name>/SKILL.md`), a tool needs no anchor file: a bare `tools/<name>/`
  directory is a tool, and its contents are the final tool state as installed (no
  build is implied). An authoritative `mind.toml` overrides convention scanning
  as for any kind (discovery.md).
- `TOOL-2` A tool's metadata comes from an optional `TOOL.md` in its directory:
  frontmatter `description`, `bin`, and `build`. A `mind.toml` `[[items]]` entry
  overrides those fields. With neither a `TOOL.md` nor a `mind.toml` entry, a tool
  has no description and `bin`/`build` come only from convention (TOOL-5).
- `TOOL-3` A tool installs to the store (`~/.mind/store/tool/<effective_name>/`)
  like any item, but by default is NOT linked into any agent home: it carries no
  symlink and the Claude harness does not discover it. Its manifest entry records
  the store path with an empty `links` set. `forget` removes the store copy;
  `recall` reports it installed. A tool is reached only by reference (TOOL-10..12).
- `TOOL-4` A tool MAY declare an explicit `link` (`mind.toml` `[[items]].link`)
  to also surface it under each agent home, for the rare tool that should be
  discoverable in place; absent a `link`, a tool is store-only (TOOL-3).
- `TOOL-5` A tool's entrypoint -- what `{{tools:name}}` resolves to -- is the
  `bin` resolved in order: an explicit `mind.toml` `[[items]].bin`, else `TOOL.md`
  frontmatter `bin:`, else the convention default `<name>` (a file named after
  the tool at the tool dir root, e.g. `tools/shard-plan/shard-plan`) when that
  file is present in the source. A tool that nothing invokes as an executable
  need not resolve a `bin`.
- `TOOL-6` Namespacing applies to tools as to any kind: a prefix gives the
  effective name `<prefix>-<name>`, and a tool's stable identity is
  `(source, kind, bare_name)` (namespacing.md), so a prefix change is a rename
  matched on identity by `evolve`/`introspect` (lifecycle.md).
- `TOOL-7` `mind.toml` accepts `kind = "tool"` wherever a kind is named
  (`[[items]]`, and `[discover].tools` globs, which match the tool DIRECTORY
  rather than an anchor file). The `bin` and `build` item fields are valid only on
  a tool; on any other kind they are a `mind.toml` schema error.

## Path-reference tokens

An item references tooling by a token that expands, at install, to the tooling's
store path. Expansion runs in the same staging pass that expands `{{ns:}}`
(namespacing.md), so a bad reference fails before the live install is touched and
the recorded content hash is of the source (token) form.

- `TOOL-10` `{{self}}` in an item's text expands to that item's own store
  directory (a path under the store root, `~/.mind/store` honoring `MIND_HOME`,
  rendered with a leading `~` per TOOL-16). It is available in every kind, so a
  skill addresses its own bundled resources as `{{self}}/resources/<script>`
  without hardcoding its installed name.
- `TOOL-11` `{{path:ref}}` expands to a sibling item's store directory, for
  reaching non-entrypoint files in a tool (`{{path:tool:x}}/lib/helper.sh`).
  `ref` is a sibling's bare name, optionally kind-qualified
  (`{{path:tool:detect}}`, `{{path:skill:review}}`). An unqualified `ref`
  matching items of more than one kind is an error (`BadReference`); a `ref`
  matching no sibling is an error (`BadReference`), as for `{{ns:}}` (NS-12).
- `TOOL-12` `{{tools:name}}` expands to a sibling tool's entrypoint: the tool's
  store directory joined with its resolved `bin` (TOOL-5). The plural `tools:` is
  distinct from the `tool:` kind-qualifier used in `{{path:}}`. A `name` that is
  not a sibling tool, or a tool with no resolvable `bin`, is an error
  (`BadReference`).
- `TOOL-13` Path tokens expand in the staging copy during the transactional
  install, alongside `{{ns:}}` (NS-11). The recorded content hash is of the token
  (source) form, not the expanded copy, so drift detection compares source with
  source (NS-13). Both prefixed and unprefixed installs expand tokens (NS-14): a
  store path is prefix-aware via the referent's effective name.
- `TOOL-14` Path tokens expand in every text file of every item kind, including
  tool directories and a skill's bundled scripts (so a bundled `pr.py` may
  contain `{{tools:shard-plan}}`), matching the scan breadth of `{{ns:}}`
  (NS-20). Token edge cases mirror `{{ns:}}` (NS-15): inner whitespace is trimmed
  (`{{ path:x }}`); an unterminated token (no closing `}}`) is left verbatim;
  non-UTF-8 files are not scanned; text with no `{{` token is copied unchanged.
- `TOOL-15` Path tokens resolve within the source only (sibling scope), as
  `{{ns:}}` does, so tool-to-tool and bundled-script-to-tool references resolve
  when both items ship in the same source. Cross-source tooling references are out
  of scope: ship a tool in the same source as the items that use it.
- `TOOL-16` A path token renders the store root with a leading `~` when the store
  lies under the user's home directory (the default `~/.mind/store`): the home
  prefix is written as a literal `~`, not spelled out absolutely. This keeps the
  expansion matchable by a Claude `settings.json` permission glob, which uses
  tilde syntax (`Bash(~/.mind/store/**)`) that an absolute path would not match.
  When the store root is not under home (a `MIND_HOME` pointing elsewhere) or the
  home directory cannot be determined, the token expands to the absolute path.

Because every token expands under `~/.mind/store`, an item's invocations of its
tooling share one stable path prefix regardless of agent home or prefix, so a
permission allowlist can target that prefix (a `Bash(~/.mind/store/**)` rule,
matched by the `~` rendering of TOOL-16) rather than chase per-item installed
paths.

## Build hooks for compiled tooling

Tooling that must be compiled or fetched before it runs uses a build step. The
expected default is the maintainer's project-level build (a Makefile or build
script) run via a source-level install hook (install-hooks.md, HOOK-50) at meld,
which produces the artifacts in the working tree; the tool directory is then
copied as the final state at `learn` (TOOL-1), no per-tool config needed. A tool
that wants its own isolated, rollback-safe build instead declares a per-item
`build` (install-hooks.md, HOOK-70..73), run in staging before the store swap;
`{{tools:name}}` (TOOL-12) then resolves to the built artifact.

## Relationship to existing mechanisms

- A skill's bundled `resources/` continue to install as part of the skill
  directory (no `tool` kind needed); `{{self}}` is what makes them addressable
  under a prefix. The `tool` kind is for tooling shared across items or shipped
  on its own.
- The executable bit on scripts is preserved by the copy into the store (the
  install copies file permissions), so a `+x` helper stays executable without a
  hook.
