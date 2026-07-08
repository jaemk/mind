# mind

A manager for agent tooling (skills, agents, rules, tools) that melds with
arbitrary git repos and links installed items into one or more agent homes
(default `~/.claude`; see `Paths::agent_homes`). A tool is store-only: it is
referenced by other items, not linked into an agent home.

The behavioral spec lives in [spec/](spec/) and is the reference to verify
against. Each statement there has a stable ID (e.g. `LIFE-4`); tests cite the IDs
they cover in `// spec: ID` comments. Keep the spec and the implementation in
step: when behavior changes, update the spec doc and the citing tests in the same
change.

`tests/spec_coverage.rs` is a coverage gate run by `cargo test` (and CI): it
fails when a defined spec ID is neither cited by a test nor listed in its
ALLOWLIST, and when a test cites an ID that the spec does not define. So a new
spec ID forces a coverage decision - add a citing test or an allowlist entry with
a reason - and a test cannot cite an undocumented behavior. CI is
`.github/workflows/ci.yml` (runs `make ci`: fmt-check + clippy + test). Locally,
use `make ci-local` (alias: `make check`): it runs the same clippy + test gate
but formats in place (`cargo fmt`) instead of `fmt-check`, so one command both
fixes formatting and runs the full gate. Prefer `make ci-local` over chaining
`cargo fmt` and `make ci`.

## Spec is mandatory for features

Every feature addition MUST be documented in [spec/](spec/) in the same change:

- Add normative requirement(s) with new stable IDs to the relevant spec doc.
- Add or update the row in the "Feature status" table in spec/README.md, and set
  its status (`done` only when implemented and covered by tests; otherwise
  `planned` or `partial`).
- Cite the new IDs from the tests that verify them (or allowlist with a reason).

A feature is not complete until its spec is updated and its status reflects
reality. The coverage gate enforces the test/spec linkage; the status column is
maintained by hand.

## Verb model

| command | does |
|---------|------|
| `meld <repo>` | clone a source repo and register it |
| `unmeld <name>` | drop a source |
| `learn <item>` | copy item to the store, symlink into each agent home (lobe) |
| `forget <item>` | remove symlink + store copy |
| `sync [--upgrade]` | fetch every source, refresh recorded commit (`--upgrade` then runs an upgrade pass) |
| `upgrade [item]` | report each installed item's hash/commit delta, prompt, then re-link the changed ones (syncs involved sources first; `--no-sync` opts out) |
| `evolve [--check] [--version V]` | update the `mind` binary itself to the latest release (or a pinned version) |
| `recall [--sources] [item]` | what's installed / source list / item details (marks out-of-date items) |
| `probe [query]` | search melded catalogs (interactive TUI by default; `--no-tui` for plain output) |
| `review <repo>` | author/consumer-side source validation: report a source's declared hooks and risky references before melding (`--fix` rewrites; `--policy` checks against managed policy) |
| `introspect [--fix]` | report drift, broken symlinks, unsynced sources (`--fix` recreates missing links) |
| `init-source [dir]` | maintainer scaffolder: generate a `mind.toml`, report the intra-source reference graph, add `{{ns:}}` templating |
| `absorb <item> [--to PATH]` | claim an unmanaged lobe item into a managed source, then install it |
| `dump [--whole-sources]` | write a super-source `mind.toml` reproducing the melded + installed state |
| `config show` / `config lobes ...` | view config / manage agent homes (lobes); a lobe may carry a `kinds` filter limiting which item kinds link there; `--preset <name>` adds a non-Claude harness lobe (gemini/codex/universal); `detect` auto-detects installed harnesses and prompts |
| `completions <shell>` / `man` | shell completion script / roff man page |

## Layout

Grouped by concern. Each verb starts in `commands.rs` (or `dump.rs`/`review.rs`)
and fans out to the modules below.

CLI surface and output:
- `src/cli.rs` - clap command/flag definitions (the `Command` enum). Doc comments here are the `--help` text.
- `src/commands.rs` - the per-verb implementations (one function per CLI verb).
- `src/main.rs` - entrypoint: parse, acquire the lock, dispatch (`match cli.command`) to `commands.rs`, map errors to exit codes.
- `src/render.rs` - output context: color, Unicode glyphs, and the `--json` emitter.
- `src/sanitize.rs` - ANSI/control/bidi stripping for source-derived strings (shared by CLI and TUI).
- `src/tui/` - the interactive `probe` TUI (`app`, `event`, `render`, `tree`, `preview`, ...).

Sources and discovery:
- `src/source.rs` - repo-spec parsing + the melded-source registry (`sources.json`).
- `src/catalog.rs` - convention scan for `skills/<n>/SKILL.md`, `agents/<n>.md`, `rules/<n>.md`, `tools/<n>/`.
- `src/frontmatter.rs` - minimal reader for an item's leading `--- ... ---` block (descriptions).
- `src/mindfile.rs` - the optional `mind.toml` a source repo may ship to declare inventory.
- `src/plugin_manifest.rs` - Claude plugin manifests (`.claude-plugin/marketplace.json`) read as a source.
- `src/resolve.rs` - item-ref parsing (`name`, `skill:name`, `owner/repo#name`) + resolution.
- `src/namespace.rs` - source prefixing and `{{ns:}}` reference expansion.
- `src/deps.rs` - within-source dependency resolution + the installed-item dependency graph.

Install, lifecycle, and state:
- `src/install.rs` - transactional copy into the store + symlink into each lobe; `{{ns:}}` expansion.
- `src/manifest.rs` - installed-item manifest (`manifest.json`), keyed `kind:name`, with the file registry.
- `src/hook.rs` - source/item install, build, and uninstall hooks (the safety-prompted shell commands).
- `src/unmanaged.rs` - lobe items `mind` did not install (surfaced in `recall`/`probe`, removable via `forget`).
- `src/hash.rs` - content hashing (drift detection). `src/git.rs` - the git CLI wrapper.
- `src/selfupdate.rs` - `evolve`: in-place upgrade of the `mind` binary.
- `src/scaffold.rs` - pure helpers for `init-source` scaffolding.

Foundations and cross-cutting:
- `src/error.rs` - structured errors (`thiserror`). No `anyhow`; every fallible path returns `MindError`.
- `src/paths.rs` - `~/.mind` and `~/.claude` roots (overridable via `MIND_HOME` / `CLAUDE_HOME`, used for test isolation).
- `src/config.rs` - user config at `~/.mind/config.toml` (lobes, default lobe, `absorb-to`).
- `src/lock.rs` - advisory file-lock + atomic registry writes guarding all persisted state.
- `src/policy.rs` - enterprise managed policy (trusted sources, pins, lobe lock, self-update control).
- `src/dump.rs` - `dump`: emit a pinned super-source `mind.toml` from the installed set.
- `src/review.rs` - `review`: author/consumer-side source validation.

## Inventory & discovery

How a source repo declares what it offers. Design rule: melding *arbitrary*
repos is the headline feature, so **convention discovery is always the zero-config
default** and a manifest is only ever optional enrichment, never a gate.

Three layers, in precedence order:

1. **Convention** (default, no file). The scanner finds `skills/<n>/SKILL.md`,
   `agents/<n>.md`, `rules/<n>.md`, and `tools/<n>/` (a tool dir needs no anchor
   file). Works on any repo, including `~/dev/agents`.
2. **Frontmatter** (always read). Each item's description comes from the YAML
   frontmatter it already carries (`description:` in `SKILL.md` / the agent or
   rule `.md`). Metadata lives next to the thing it describes; nothing duplicated.
3. **`mind.toml`** at the repo root (optional). `[source]` metadata is read
   regardless. If it declares `[[items]]` or `[discover]` it becomes
   *authoritative*: convention scanning is turned off and only what it lists is
   offered. Use it for export control, non-standard / monorepo layouts, custom
   link targets, and repo-level metadata.

```toml
[source]
description = "James's agent library"
prefix = "jk"               # namespace: every item installs as jk:<name> (see below)
min-mind-version = "0.2"    # version gate: meld refuses a source the binary is too old for (DSC-40)

# Explicit inventory (authoritative). Omit [[items]] and [discover] to keep
# convention scanning while still supplying [source] metadata.
[[items]]
kind = "rule"                       # skill | agent | rule
name = "style"
path = "guidelines/style.md"        # relative to repo root; a dir for skills
link = "rules/style.md"             # optional: link target relative to ~/.claude
description = "House style"         # optional: overrides frontmatter

# ...or glob-based discovery for odd layouts:
[discover]
skills = ["packages/*/SKILL.md"]
agents = ["agents/*.md"]
```

The frontmatter reader (`frontmatter.rs`) is intentionally minimal: top-level
scalar keys only, no block scalars. If items grow richer metadata needs, swap in
a real YAML parser rather than extending the hand-rolled scanner.

## Namespacing (`namespace.rs`)

Two melded sources can both ship a `review`; they would collide at
`~/.claude/skills/review`. A *prefix* namespaces a source so every item from it
installs under `<prefix>:<name>` (identity, store path, symlink, and ref). The
effective prefix is, in order: the consumer's `meld --as <prefix>` (persisted as
`Source.alias`), else the repo's `[source].prefix`, else none.

The catalog is source truth: `CatalogItem.name` is the *bare* name, and the
prefix is an install-time transform (`CatalogItem::effective_name()`), not baked
in during the scan. So an item's stable identity is `(source, kind, bare_name)`,
which `upgrade`/`introspect` match on. That is what lets a prefix change be seen
as a *rename* of the same item rather than an orphan plus a new item.

Prefixing changes an item's identity, and the Claude harness resolves agents and
skills at runtime by that identity (the name in the text). So a source whose
items reference each other (e.g. `dev` -> `test`) breaks under a prefix unless
those references are rewritten. We do NOT guess in prose (sibling names like
`do`, `test`, `review` are common words). Instead:

- **Reference token.** Authors write intra-source references as `{{ns:name}}`.
  On install, `install.rs` expands each token to the effective name (`name` when
  unprefixed, `prefix:name` when prefixed) and validates the referent is a real
  sibling (errors via `MindError::BadReference` on a typo). Unprefixed sources
  with tokens still work: the token expands to the bare name. Existing repos with
  no tokens are unaffected (expansion is a no-op when no `{{ns:` is present).
- **Unguarded-reference warning.** When melding a source that has a prefix in
  effect, `meld` scans each item for sibling names appearing in bare prose
  (outside a token) and warns. Advisory and heuristic, but it surfaces refs that
  will break at runtime instead of letting them fail silently.

Drift note: the manifest records the hash of the *source* content, not the
expanded store copy, so `upgrade`/`introspect` compare source-vs-source.

## Install, upgrade, and uninstall (`install.rs`)

Installs are transactional and preserve the previous version until the new one
is proven:

1. Build the new copy in a staging dir (`~/.mind/.tmp/staging/...`) and expand
   its `{{ns:}}` references there. The likeliest failure (a bad reference) occurs
   now, while the live install is untouched.
2. Move any existing store copy aside to a backup, move staging into place, and
   ensure the symlink.
3. On failure during the swap, restore from backup; on success, drop it.

A failed upgrade therefore never leaves you worse off than before it started.

Each installed item records a **file registry** in the manifest (`store` plus
`links`, relative to `~/.mind` / `~/.claude`). `uninstall` removes exactly those
recorded paths rather than recomputing them from kind+name, so it stays correct
even if link conventions change.

`upgrade` matches installed items to the catalog by stable identity. When the
effective name changed (a prefix change), it builds the new name first, then
removes the old item via its registry and re-keys the manifest entry, reporting
`rename old -> new`. When only content changed, it swaps in place under the same
name.

## Conventions

- Errors are structured (`thiserror`), never stringly-typed. Add a `MindError`
  variant rather than formatting a message into a generic error.
- Tag I/O failures with the path via `MindError::io(path, e)`.

## Testing (important)

Encode manual checks as formal tests; do not leave them as one-off shell probes.
Any behavior you would verify by hand on the CLI MUST be added as a unit or
integration test unless it is genuinely impossible to automate (e.g. it requires
real network or interactive auth that cannot be faked). If you must skip
automation, say so explicitly and explain why.

- Pure logic (spec/ref parsing, hashing, URL building) -> unit tests in the
  module's `#[cfg(test)]` block.
- CLI behavior -> integration tests in `tests/`, which drive the real binary
  (`env!("CARGO_BIN_EXE_mind")`) against a hermetic fixture: a local git repo
  melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed at a temp
  dir. No network. `tests/cli.rs` holds the general assertions; topical suites
  live in siblings (`cli_absorb.rs`, `cli_dump.rs`, `cli_lobes.rs`,
  `cli_install_items.rs`, `cli_build_hooks.rs`, `review_hooks.rs`,
  `item_lifecycle.rs`, ...). Add a new assertion to the matching topical file, or
  to `cli.rs` if none fits. `Sandbox::from_example(<name>)` drives a shipped
  `examples/<name>` the same way.

Run everything with `cargo test`.

To capture real CLI output for docs without a permission prompt, this repo ships
a gitignored `scripts/probe.sh` (recreated from the `hermetic-verify` skill) that
melds a shipped example in a throwaway isolated home, e.g.
`bash scripts/probe.sh drift -- recall -- upgrade --yes`. Probing is for
capture/exploration only; promote anything worth keeping into a `from_example`
test. The verify-by-test-first discipline and the global runner permission live
in the merged `~/.claude/CLAUDE.md`.
