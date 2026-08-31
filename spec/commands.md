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

Note on where the harness is going: Claude Code has merged custom commands into
skills. A file at `commands/deploy.md` and a skill at `skills/deploy/SKILL.md`
both produce `/deploy`, existing `commands/` files keep working, and skills are
what its documentation now recommends for new work (a skill can carry supporting
files and be invoked by Claude as well as by the user). The `command` kind
exists because sources ship `commands/` today and the harness keeps reading
them, not because it is the form to steer authors toward; `init-source` and the
guide recommend a skill for new items.

## The kind

- `CMD-1` `command` is an item kind. By convention a command is a file
  `commands/<name>.md` under a scan root, and its name is the file stem, the same
  shape as an agent (DSC-11) and a rule (DSC-12). A missing `commands/` directory
  yields no items, not an error (DSC-13).
- `CMD-2` The convention scan is flat: it reads the immediate `.md` children of
  `commands/`, and does not descend into subdirectories. This matches agents and
  rules, whose convention scans are flat for the same reason: a name derived
  from a nested path would have to encode the separator, and every name `mind`
  accepts must be a single safe path component (DSC-71).

  A nested layout still reaches the same end. The Claude harness derives a
  command's name from a subdirectory as `<dir>:<name>`
  (`commands/frontend/component.md` is offered as `/frontend:component`;
  verified against a live session, and NOT what its documentation's
  name-derivation table states, which says a command file's name is its file
  name without extension). `mind` produces that spelling by a different route:
  a colon is legal in a file name, so an item whose effective name is
  `frontend:component` links to `commands/frontend:component.md` and the
  harness offers it as `/frontend:component` too. The two layouts converge on
  one command name, so nothing is lost by not descending. A source that wants
  the grouped name under convention discovery ships
  `commands/<group>:<name>.md`; one that wants to keep a nested directory
  reaches it with a `[[items]]` entry (with a `link` when the subdirectory
  itself must be preserved) or a `[discover].commands` glob.

  The group segment must not be a reserved kind word (`skill`, `agent`, `rule`,
  `command`, `tool`): the item-ref parser reads a pre-colon token as a KIND
  when it parses as one, so `commands/tool:build.md` installs correctly but is
  unaddressable as `tool:build` (parsed as kind `tool`, name `build`, failing
  with an error naming `build`, a string the user never typed); only the
  fully kind-qualified `command:tool:build` resolves it.
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
  name is `<prefix>:<name>` and the link is `commands/<prefix>:<name>.md`, so
  the harness is expected to offer the command as `/<prefix>:<name>`. The
  single-colon case was verified against a live session: a colon in a command
  file's name is accepted, through a symlink as `mind` installs it, and the
  resulting command carries the colon in its invoked name. `mind` cannot verify
  harness pickup on an ongoing basis: it installs the file and the symlink and
  nothing more, so if this behavior ever changes, every namespaced command
  would silently disappear from the prompt while `mind recall` keeps reporting
  it installed and healthy. Check with `/help` in Claude Code, which lists the
  command under its colon name. The spelling is the harness's own for a
  namespaced command (`/<plugin>:<skill>` for a plugin skill, `/<dir>:<name>`
  for a grouped command), so a prefixed command is expected to read naturally
  at the prompt rather than as a mangled name. A command's stable identity is
  `(source, kind, bare_name)`, so a prefix change is a rename matched on
  identity by `upgrade`/`introspect` (namespacing.md, lifecycle.md). A bare
  name that already contains a colon (`commands/frontend:component.md`,
  CMD-2) is carried through unchanged by `mind`'s own linking and composes
  with a prefix as `<prefix>:<group>:<name>` on disk; this double-colon
  composition has NOT been verified against a live session, unlike the
  single-colon case above.
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
  `commands/` directory (UNM-1). Both `absorb` and unmanaged-item detection see
  only the immediate `.md` children of a lobe's `commands/` directory, matching
  the flat, non-recursive convention scan (CMD-2): a grouped layout on the
  consumer side (`~/.claude/commands/frontend/component.md`) is not walked into,
  so it is invisible to `recall`'s unmanaged listing and `mind absorb
  frontend:component` fails as not found. A `[[items]].link` for a command is
  confined to a kind directory like any other link (DSC-97), with `commands/`
  now among them.
- `CMD-9` `command` stays a reserved namespace prefix. It was already reserved
  against a future kind (NS-29); it is now reserved as an actual kind word
  (NS-25). The rejection and its error are unchanged, so no existing source's
  prefix becomes newly invalid or newly valid.
