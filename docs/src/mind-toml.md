# The `mind.toml` file

A source repo may place a `mind.toml` at its root to declare what it offers and
how `mind` should treat it. It is always optional: a repo with no `mind.toml` is
discovered by [convention](source-layout.md). `mind.toml` is enrichment, never a
gate.

There are three discovery layers, in precedence order:

1. **Convention** (default, no file): the scanner finds `skills/<name>/SKILL.md`,
   `agents/<name>.md`, `rules/<name>.md`, and `tools/<name>/`.
2. **Frontmatter** (always read): each item's `description` (and a tool's `bin` /
   `build`) come from the frontmatter it already carries.
3. **`mind.toml`** (optional): `[source]` metadata is read regardless. Declaring
   `[[items]]` or `[discover]` item globs makes the file **authoritative**:
   convention scanning is turned off and only what the file lists is offered.

## When does `mind.toml` take over discovery?

| `mind.toml` contains | convention scan | result |
|----------------------|-----------------|--------|
| nothing / no file | on | items found by convention |
| `[source]` only | on | convention items, plus metadata (prefix, pin, ...) |
| `[discover].sources` only | **on** | convention items, plus a curated chain |
| `[[items]]` | off | exactly the declared items |
| `[discover]` with kind globs | off | exactly the glob matches |

The last two are *authoritative*. A bare `[discover].sources` list (curating
other repos) does **not** turn off convention scanning, so a repo can ship its
own items by convention and curate others at the same time (see
[Regular plus super-source](#regular-plus-super-source)).

## `[source]` — repo metadata

All keys are optional.

```toml
[source]
description = "James's agent library"   # shown in recall/probe
prefix = "jk"                            # namespace: items install as jk-<name>
min-mind-version = "0.5"                 # refuse to scan under an older mind
install = "make build"                   # a source-level install hook (see below)
follow-branch = "main"                   # pin: track a branch ...
# pin-tag = "v2"                          # ... or fix to a tag ...
# pin-ref = "a1b2c3d"                     # ... or to an exact commit (pick one)
roots = ["packages"]                     # scan under these dirs, not the repo root
```

- **`prefix`**: every item installs as `<prefix>-<name>` (identity, store path,
  symlink, and ref). A consumer's `meld --as <prefix>` overrides it; `meld --as ''`
  removes it. See [namespacing](https://github.com/jaemk/mind/blob/main/spec/namespacing.md).
- **`install`**: a shell command run once on `meld`, after checkout, to build or
  install the tooling the source's items rely on. It is disclosed and prompted
  before it runs (`--dangerously-skip-install-hook-check` runs it unattended).
  For more than one command, or an uninstall hook, use `[[hooks]]` below.
- **`follow-branch` / `pin-tag` / `pin-ref`**: how `sync` tracks upstream. Declare
  at most one; two is an error. A consumer `meld --follow-branch|--pin-tag|--pin-ref`
  overrides it.
- **`roots`**: convention discovery scans under each listed directory instead of
  the repo root, for a monorepo or subtree layout. Ignored when the file is
  authoritative (`[[items]]`/`[discover]` paths are always repo-root-relative).

## `[[hooks]]` — lifecycle hooks

Zero or more named hooks the maintainer declares. Each runs on the host (gated by
a disclosure prompt) at the bound lifecycle event.

```toml
[[hooks]]
run = "make build"          # the shell command (required)
name = "build tooling"      # label shown in the disclosure (else the command)
optional = false            # required (run/skip/abort) vs optional (run/skip)
event = "install"           # "install" (on meld, default) or "uninstall" (on unmeld)

[[hooks]]
run = "make clean"
event = "uninstall"
optional = true
```

A single `[source].install` is shorthand for one required install hook; use
`[[hooks]]` when you need several, an uninstall hook, or the optional form.

## `[[items]]` — explicit inventory (authoritative)

List items explicitly when convention does not fit (a non-standard layout, export
control, custom link targets). Declaring any `[[items]]` turns off convention
scanning for the source.

```toml
[[items]]
kind = "rule"                    # skill | agent | rule | tool (required)
name = "style"                   # the bare name (required)
path = "guidelines/style.md"     # path relative to the repo root; a dir for skills/tools (required)
link = "rules/house-style.md"    # optional: link target relative to the agent home
description = "House style"       # optional: overrides frontmatter

[[items]]
kind = "tool"
name = "detect"
path = "tools/detect"
bin = "detect.sh"                # tool only: what {{tools:detect}} resolves to
build = "make"                   # tool only: per-item build, run in staging at install
install = "./setup.sh"           # any kind: host side effect run after install
uninstall = "./teardown.sh"      # any kind: host cleanup run before removal
```

- **`bin`** and **`build`** are valid only on a `tool`; on any other kind they are
  a schema error.
- **`install`** / **`uninstall`** are per-item lifecycle hooks (valid on any kind),
  distinct from `build` (which produces the item's content) and from the
  source-level `[source].install` / `[[hooks]]`.

## `[discover]` — glob-based discovery

For odd or monorepo layouts where the items exist but not at the convention paths.
Declaring any kind globs makes the file authoritative.

```toml
[discover]
skills = { include = ["packages/*/skill"], exclude = ["packages/internal/*"] }
agents = { include = ["agents/**/*.md"] }
rules  = { include = ["rules/*.md"] }
tools  = { include = ["packages/*/tool"] }   # globs match the tool DIRECTORY
```

Within a kind, `include` globs are matched first, then anything also matched by an
`exclude` glob is dropped. Tool globs match the tool directory (its `TOOL.md`, if
present, supplies metadata), not an anchor file.

## `[discover].sources` — curated super-source

A repo can list other repos to meld, acting as a curated registry. Melding it
recursively melds each listed source (skipping any already registered; cycles
terminate). Each nested source is registered independently and tracks its own
upstream commit.

```toml
[discover]
sources = [
  { source = "owner/repo" },                        # melded, items left available
  { source = "github:foo/bar", as = "fb" },         # imposed namespace (like meld --as)
  { source = "owner/recommended", install = true }, # offered for install on meld
]
```

The equivalent table-array form is also valid:

```toml
[[discover.sources]]
source = "owner/recommended"
install = true
```

- **`source`**: any repo spec `meld` accepts (`owner/repo`, a host-qualified
  spec, or a local path).
- **`as`**: impose a namespace prefix on that nested source.
- **`install`** (default `false`): when `true`, melding the super-source offers
  that nested source's items for install (the same preview-and-prompt as the
  top-level source), instead of leaving them registered but not installed.

By default a melded super-source registers the whole chain but installs only its
own items plus the `install = true` entries. `meld --recursive` (`-r`) offers
every nested source for install.

## Scenarios

### A regular source

A repo that ships its own skills, agents, rules, and tools. The simplest form is
no `mind.toml` at all (pure convention). Add a `mind.toml` to attach metadata or a
prefix:

```toml
[source]
description = "James's agent library"
prefix = "jk"
```

Convention scanning still finds `skills/<name>/SKILL.md`, `agents/<name>.md`,
etc., and each installs as `jk-<name>`. Use `[[items]]` or `[discover]` instead
only when the layout is non-standard (those turn convention off).

### A super-source

A curated registry that ships no items of its own, only a list of other repos:

```toml
[source]
description = "Curated agent registry"

[discover]
sources = [
  { source = "jaemk/agents" },
  { source = "acme/skills", as = "acme", install = true },
]
```

Melding it registers `jaemk/agents` and `acme/skills` (the latter namespaced
`acme-` and offered for install). It has no items of its own, so without
`install = true` (or `meld --recursive`) nothing installs; `mind probe` browses
what the chain offers.

### Regular plus super-source

A repo that both ships its own items and curates others. Because a bare
`[discover].sources` is **not** authoritative, convention scanning still runs for
this repo's own items:

```toml
[source]
description = "James's library and registry"
prefix = "jk"

# This repo's own items are still found by convention (skills/, agents/, ...)
# because only [discover].sources is declared, not [[items]] or kind globs.

[discover]
sources = [
  { source = "acme/skills", as = "acme", install = true },
]
```

Melding installs this repo's own `jk-<name>` items (per the default install
prompt) and registers `acme/skills` (offered for install via its `install = true`
flag). If you instead need to declare this repo's items explicitly while also
curating, add `[[items]]` or `[discover]` kind globs alongside `sources`; that
turns convention off, so list every item the repo offers.
