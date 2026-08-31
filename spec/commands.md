# The `command` item kind

Status: done. Harnesses expose user-authored slash commands as markdown files in
a `commands/` directory of the agent home (Claude Code reads
`~/.claude/commands/<name>.md` and offers it as `/<name>`). A source that ships
skills and agents usually ships commands next to them, and before this kind they
were the one component `mind` could see in a repo but not install: melding the
repo left the commands behind, and a consumer copied them by hand, outside the
store, the manifest, and `upgrade`.

`command` is a fifth item kind alongside skill, agent, rule, and tool. It is an
ordinary linked kind: discovered by convention, copied into the store, symlinked
into every agent home that admits it, namespaced, upgraded, and removed exactly
as the other kinds are. This document states only what is specific to it;
everything else follows the general rules in discovery.md, storage.md,
lifecycle.md, and namespacing.md.

## The kind

- `CMD-1` `command` is an item kind. By convention a command is a file
  `commands/<name>.md` under a scan root, and its name is the file stem, the same
  shape as an agent (DSC-11) and a rule (DSC-12). A missing `commands/` directory
  yields no items, not an error (DSC-13).
- `CMD-2` The convention scan is flat: it reads the immediate `.md` children of
  `commands/`, and does not descend into subdirectories. A harness that
  namespaces commands by subdirectory (`commands/frontend/component.md` ->
  `/frontend:component`) is therefore not discovered by convention below the
  first level. Such a layout is reachable through the explicit inventory: a
  `[[items]]` entry naming the nested `path`, with a `link` pointing at the
  nested target when the subdirectory itself carries meaning, or a
  `[discover].commands` glob. This matches agents and rules, whose convention
  scans are flat for the same reason: a name derived from a nested path would
  have to encode the separator, and every name `mind` accepts must be a single
  safe path component (DSC-71).
- `CMD-3` A command's description comes from the top-level `description` in its
  own frontmatter, as for an agent or rule (DSC-20). A harness's other command
  frontmatter keys (`argument-hint`, `allowed-tools`, `model`) are content:
  `mind` neither reads nor validates them, and copies the file unchanged apart
  from the token expansion every item gets (NS-11).
- `CMD-4` `mind.toml` accepts `kind = "command"` wherever a kind is named: an
  `[[items]]` entry, a `[discover].commands` glob list (matching the command
  FILE, as the agent and rule globs do), and a lobe's `kinds` filter (HARN-1).
  `--kind command` selects the kind wherever the CLI takes one, and
  `command:<name>` is an item ref wherever a ref is taken.

## Storage and linking

- `CMD-5` A command installs to `~/.mind/store/command/<effective_name>` (the
  file itself, as for an agent or rule) and links into each admitting agent home
  at `commands/<effective_name>.md` (STO-10, LIFE-1). It is a linked kind: unlike
  a tool (TOOL-3) it is discovered by the harness, and unlike an agent (NS-40) it
  is linked under its effective name, not a frontmatter-declared one.
- `CMD-6` A namespace prefix applies to a command as to any kind: the effective
  name is `<prefix>:<name>` and the link is `commands/<prefix>:<name>.md`, so the
  harness offers the command as `/<prefix>:<name>`. That is the same
  `namespace:command` spelling a harness's own subdirectory namespacing produces,
  so a prefixed command reads naturally at the prompt rather than as a mangled
  name. A command's stable identity is `(source, kind, bare_name)`, so a prefix
  change is a rename matched on identity by `upgrade`/`introspect`
  (namespacing.md, lifecycle.md).
- `CMD-7` The harness presets (HARN-4: gemini, codex, universal, windsurf) admit
  skills only, so they are unchanged by this kind: a command links into the
  default Claude lobe and into any lobe whose `kinds` filter names `command`. A
  lobe with no filter admits every linked kind, commands included.

## Everything else follows the general rules

- `CMD-8` A command participates in every kind-generic mechanism with no
  command-specific behavior: `learn`/`forget`/`upgrade`/`introspect`, drift
  hashing (LIFE-15), `{{ns:}}` and path-token expansion (NS-11, TOOL-13),
  `requires` dependencies (DEP-4), item lifecycle hooks including the
  frontmatter scalars (HOOK-80, HOOK-130), ignore patterns (IGN-1), `absorb`
  (whose convention path for the kind is `commands/<name>.md`), `dump`,
  `review`, `probe`/`recall` listing, and unmanaged-item detection in a lobe's
  `commands/` directory (UNM-1). A `[[items]].link` for a command is confined to
  a kind directory like any other link (DSC-97), with `commands/` now among
  them.
- `CMD-9` `command` stays a reserved namespace prefix. It was already reserved
  against a future kind (NS-29); it is now reserved as an actual kind word
  (NS-25). The rejection and its error are unchanged, so no existing source's
  prefix becomes newly invalid or newly valid.
