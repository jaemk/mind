# Changelog

All notable changes to `mind` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.0] - 2026-07-02

### Added

- A repo can be both a Claude plugin marketplace and a `mind` curator. A bare
  `[discover].sources` list in a co-present `mind.toml` composes with a
  `.claude-plugin/marketplace.json` (or `plugin.json`) instead of suppressing it:
  the manifest defines the repo's own items and the curated chain layers on top
  (MKT-16). New `marketplace-curator` example.

### Changed

- An own-item source-discovery directive now suppresses only a co-present
  `.claude-plugin/` manifest's own-item layer, and the set of such directives is
  broadened. A `mind.toml` `[source].roots`/`flat-skills`, or a consumer `meld
  --root`/`--flat-skills` flag, suppresses the manifest and runs convention
  discovery instead (with a note), so `--root` is no longer a silent no-op on a
  manifest source (MKT-15).

## [0.11.0] - 2026-07-01

### Added

- Global `--verbose` (`-v`) flag, accepted before or after the verb like
  `--json`/`--yes`/`--ascii`. It enables extra advisory output and does not
  affect the color/Unicode capability gate (CLI-162).

### Changed

- The unguarded-reference warning emitted during `meld` (when a prefix is in
  effect) is now shown only under `--verbose`; the default meld is silent
  (CLI-14, NS-20, NS-22).

## [0.10.0] - 2026-07-01

### Added

- `init-source --marketplace` scaffolds a `.claude-plugin/marketplace.json`
  (via a new `scaffold` module); `--flat-skills` sets `flat-skills = true` in
  `mind.toml` and, combined with `--marketplace`, populates the plugin `skills`
  array from flat-skill discovery. Plugin-name precedence is `--namespace` >
  `[source].prefix` > directory name (INIT-10, INIT-11, INIT-12).
- Cross-source collision detection at `meld` for skills, rules, and tools: when
  a melded source would install an item that collides with an existing one, the
  non-interactive path errors with `SkillCollision` and suggests `--namespace
  <repo-name>`, and an interactive TTY prompts for a prefix (NS-43, NS-44,
  NS-45).
- `config lobes add`/`detect` backfills already-installed items into a
  newly-added lobe: `--yes` backfills automatically, an interactive TTY prompts,
  and a non-interactive run prints a note pointing at `introspect --fix`
  (HARN-7).
- `introspect --fix` repairs missing lobe coverage, creating links for items not
  yet linked into a configured lobe and updating the manifest (HARN-8).

### Changed

- The gemini and antigravity harness lobes are unified to `~/.gemini/config`,
  the skill directory both Gemini CLI and Antigravity read. The `gemini` preset
  now targets `.gemini/config` with `kinds = [skill]` (was `.gemini` with
  `[skill, agent]`); the redundant `antigravity` and `antigravity-cli` presets
  are removed (HARN-4, HARN-5).
- The `[discover].sources` entry key `as` is renamed `namespace` (`as` remains a
  backwards-compatible parse alias). `dump` emits `namespace`, `review` advises
  migrating, and `recall --sources` displays `namespace:<prefix>` instead of
  `as:<prefix>` (DSC-78).
- A `[discover].sources` entry whose clone fails for a non-auth reason (network
  error, not-found) now warns and skips rather than failing the whole meld; the
  primary source and successfully-cloned nested sources stay registered, and the
  skipped entry is recorded with `reason="clone_failure"`. The same skip applies
  during `sync` re-walk. The one hard-fail case is a pure curator (no items of
  its own) whose nested sources all fail, which errors with
  `CuratorAllNestedFailed` (DSC-79, DSC-80).

### Fixed

- Adding the first explicit lobe to an empty lobes config via `config lobes
  add`/`detect` now prepends `claude_home` to the saved list. Previously the
  implicit `~/.claude` default was silently dropped from `agent_homes()`, so new
  installs stopped reaching Claude and `introspect --fix` could not see the
  Claude home as a coverage target (HARN-9).
- In-repo marketplace entries with `source: "./"` no longer drop all but the
  first plugin; each plugin is scanned as its own catalog root. Plugin repos
  used as nested `[discover].sources` entries inherit the plugin `name` as their
  default namespace, and marketplace-as-nested-source preserves per-plugin
  namespacing (MKT-12, MKT-13, MKT-14).

## [0.9.0] - 2026-07-01

### Added

- Consume Claude Code plugin manifests as a discovery source. A melded repo with
  a `.claude-plugin/plugin.json` (a single plugin) or `.claude-plugin/marketplace.json`
  (a catalog) has its skills and agents mapped to `mind` items and installed
  through the usual store-and-symlink path; `mind` never writes Claude's plugin
  cache or `settings.json`. The plugin `name` is the default namespace prefix
  (agents stay bare per NS-40); unsupported components (`commands`, `hooks`,
  `.mcp.json`, ...) report a skipped count on meld. A marketplace is consumed as a
  curated super-source, one sub-source per listed plugin, in-repo or external.
  Manifests are held to the same path-safety and strict-parse guards as
  `mind.toml`, and `recall --sources` labels a source's manifest origin
  (`claude-plugin` / `claude-marketplace`) (MKT-1..11).
- `upgrade` accepts a glob in place of an exact item ref, mirroring `forget`; the
  kind prefix and source qualifier compose (`upgrade 'jk:*'`, `upgrade
  'skill:*'`, `upgrade 'owner/repo#*'`). A glob (or exact ref) that matches no
  installed item reports up-to-date rather than erroring (CLI-65).

### Changed

- The namespace separator is `:` instead of `-`: a prefixed item installs as
  `<prefix>:<name>`. `upgrade` migrates already-installed items from the old
  `<prefix>-<name>` form in place, without a namespace change.
- `meld --as` is renamed `--namespace` (short `-n`); `--as` stays as a deprecated
  alias. A source's namespace is locked once any of its items are installed:
  changing it requires forgetting those items first, rather than an in-place
  rename of installed items (NS-30, CLI-161).
- Agents are no longer namespaced by a source prefix. An agent links into each
  lobe under its bare frontmatter `name` (the harness keys agents by that name,
  not the filename), so a prefix reaches only its store path and manifest key.
  Two sources shipping a same-named agent now collide: `learn` refuses with an
  `AgentCollision` error and `meld` emits an advisory warning (NS-40, NS-41,
  NS-42).

## [0.8.0] - 2026-06-28

### Added

- A `[discover].sources` entry may carry `on-auth-failure`, an inline table with
  a required `action` (`"error"` or `"skip"`) and an optional `message`, to
  declare how a nested source's clone failure is handled when it is caused by an
  authentication failure. `"skip"` warns and continues, leaving the source
  unregistered; `"error"` exits non-zero with the standardized message. Auth
  failure is detected from git stderr credential-denial patterns; the same
  handling applies during `sync`, which re-walks `[discover].sources`. Without
  the directive an auth failure stays a generic git error. The policy governs
  only the entry's own clone; auth failures from transitive descendants
  propagate as hard errors (DSC-68, DSC-69, DSC-70).

### Changed

- When forgetting a single installed item that other installed items depend on,
  the TUI surfaces the dependent keys in the confirmation description before the
  user confirms, mirroring the CLI's DEP-60 warning (TUI-52).
- `strip_ansi` now uses the `strip-ansi-escapes` crate instead of a hand-rolled
  parser, and additionally drops bidi-override and separator control characters,
  hardening display of curator-controlled content against terminal injection.

## [0.7.0] - 2026-06-27

### Added

- `absorb <ref>` claims an unmanaged lobe item (a hand-written skill/agent/rule)
  into a version-controlled source: it moves the item out of the lobe, commits
  it, melds the source if needed, and learns it as a managed item. The
  destination resolves from `--to`, then `MIND_ABSORB_TO`, then the `absorb_to`
  config key, and falls back to a built-in `~/.mind/personal` (git-init on
  demand). The inverse of `forget --unmanaged`.
- `dump` writes a super-source `mind.toml` reproducing the current melded and
  installed state: each source is referenced by spec, pinned to its recorded
  commit, and stamped with an install directive (`install = true`/`false` or
  `install_items = [...]` for a subset). `--whole-sources` emits every source as
  `install = true`.
- `forget --unmanaged` scopes `forget` to unmanaged lobe items: a glob removes
  every match, an exact `kind:name` removes one, and no ref removes all
  unmanaged across lobes. Managed items are never matched.
- `requires:` frontmatter key declares explicit intra-source dependencies
  (whitespace-separated `kind:name`/bare names), unioned with the `{{ns:}}`
  derived edges. Unlike a token, it is metadata and is not rewritten into the
  item body.
- A dependency graph over installed items, surfaced across the verbs: `forget`
  warns when removal breaks a dependent's reference (no cascade); `recall --tree`
  renders the installed items as a dependency forest and `recall <item> --tree`
  scopes to one subtree; the non-interactive `probe` listing nests each item's
  transitive dependencies, with `probe --json` adding a flat `dependencies`
  adjacency field; the TUI expands an item to its dependency subtree and jumps to
  a dependency's canonical line on Enter.
- `recall --tree --json` emits the installed dependency forest as nested JSON
  (`{"key": ..., "dependencies": [...]}`, cycle back-edges as `{"key": ...,
  "cycle": true}`).
- A `[discover].sources` entry may set `install_items = ["kind:name", ...]` to
  install only a named subset of a nested source's items.
- A `[discover].sources` entry may carry `follow-branch`, `roots`, and
  `[[discover.sources.hooks]]` to support an un-onboarded nested source without
  forking it. The curator-supplied values apply only when the nested source
  ships no `mind.toml` of its own.
- Documentation pages for the interactive TUI, managed policy, tooling (the
  `tool` kind and path tokens), namespacing, dependencies, unmanaged items, and
  `init-source`, plus the global flags, the color/Unicode gate, exit-status
  semantics, the on-disk layout, and troubleshooting.

### Changed

- `recall` marks an installed-but-out-of-date item with a distinct left-edge
  marker (`↑` in yellow, ASCII `^`) instead of the installed `✓`/`+`, so the
  stale state is visible from the marker alone.
- A nested `[discover].sources` pin directive (`follow-branch`, `pin-tag`, or
  `pin-ref`) is authoritative: it overrides the nested source's own `[source]`
  pin, ranking just below a consumer meld flag.
- `absorb` is transactional: a commit, meld, or learn failure restores the
  original lobe entry and leaves the manifest unchanged. `absorb` and `forget`
  refuse a destructive confirmation in `--json` mode without `--yes` rather than
  proceeding silently.

### Security

- Pin and ref values are validated at parse time: a value beginning with `-` (or
  containing whitespace, `..`, or control characters) is rejected, and `git
  fetch` invocations use a `--` terminator. This prevents an untrusted cloned
  `mind.toml` pin or a `--follow-branch`/`--pin-tag`/`--pin-ref` flag from
  injecting git options.

## [0.6.2] - 2026-06-26

### Added

- A published documentation site at <https://jaemk.github.io/mind/>, with a guide
  (install, quickstart, commands, configuration, install hooks, troubleshooting),
  authoring docs, and an examples page mapping each consumer and maintainer use
  case to a runnable example.
- Example sources for the `tool` kind and path tokens, source lifecycle hooks,
  `[source].roots` subtree discovery, an authoritative `[[items]]` inventory, and
  a `[discover].sources` super-source, each verified by a test.
- The crate publishes to crates.io on release (`cargo install mind-cli` installs
  the `mind` binary), and carries `repository`, `homepage`, and `documentation`
  metadata.

### Changed

- The README is a concise landing page; the documentation site is the primary
  reference.

## [0.6.1] - 2026-06-25

### Changed

- Release tooling only: the GitHub release is created with the GitHub CLI and its
  notes are taken from this changelog. No change to the `mind` binary.

## [0.6.0] - 2026-06-25

### Added

- Item-level lifecycle hooks: an item may declare `[[items.hooks]]` (with `run`,
  `name`, `optional`, and `event` = `install`/`uninstall`), the same shape as a
  source's `[[hooks]]`. The scalar `install`/`uninstall` fields remain as
  shorthand. Item install hooks run after the source install hook and item
  uninstall hooks run before the source uninstall hook, so teardown is the
  reverse of install.
- `unmeld` accepts a glob or partial source name and removes every matching
  source, mirroring the glob selection in `learn`/`forget` (e.g.
  `unmeld '*agents'`).
- `probe` and `recall` accept a glob for `--source`.
- `-n` as a short form of `probe --no-tui`.

### Changed

- `recall` and the `probe` listing mark an installed item out of date exactly
  when `mind upgrade` would act on it: its source content changed, or its
  effective (namespaced) name changed. A source commit that advances without
  changing an item's content or name no longer marks it, and a hash failure now
  flags the item rather than reporting it up to date. The recall status view
  shows a renamed item as out of date instead of as removed upstream.
- `[source].install` is deprecated in favor of `[[hooks]]`. `mind review`
  reports the deprecated field and `init-source` scaffolds only `[[hooks]]`.
- `init-source` flags a bare sibling reference only when an effective prefix is
  in force; `review`'s hardcoded-path and bare-tool advisories note that a
  location populated by an install hook is safe.
- A malformed glob selector reports an invalid-pattern error instead of a
  no-source-found error.
- Renamed the crate package to `mind-cli`; the installed binary stays `mind`.
  Updated dependencies (`toml` 1, `ratatui` 0.30, `crossterm` 0.29, `dirs` 6,
  `clap_mangen` 0.3).

## [0.5.2] - 2026-06-25

### Added

- The frontmatter reader interprets folded (`>`, `>-`, `>+`) and literal (`|`,
  `|-`, `|+`) block scalars, so a multi-line `description:` renders in
  `recall`/`probe` instead of being dropped.

### Changed

- `recall` and the `probe` listing mark an installed item out of date when its
  current source content differs from the installed copy, not only when the
  source commit advanced. This surfaces drift for a melded local directory and a
  source checkout edited in place.

## [0.5.1] - 2026-06-25

### Fixed

- A `$MIND_POLICY_FILE` naming a file that does not exist no longer hard-errors
  every command with a not-found error; a missing env-pointed policy file is now
  treated as no policy (unmanaged), mirroring the system-path existence check.

## [0.5.0] - 2026-06-25

### Added

- A `[discover].sources` entry in a super-source's `mind.toml` may set
  `install = true` to recommend a nested source for install: melding the
  super-source offers that source's items for install (the same preview-and-prompt
  as the top-level source), instead of leaving them only registered and available.
- The interactive browser keeps the highlighted row within the middle two-thirds
  of the list, scrolling before it reaches the top or bottom edge.

### Changed

- `meld --install-super-sources` is renamed `meld --recursive` (`-r`). It installs
  every nested source in the curated chain, now beyond the per-source
  `install = true` defaults.
- In the interactive browser, Enter opens a details dialog for the focused source
  or item listing its valid actions (Install/Forget, or install-all/uninstall-all/
  unmeld for a source) instead of toggling expansion; expansion moves to Space and
  the Left/Right arrows.

## [0.4.1] - 2026-06-25

### Added

- `tool` item kind: a store-only installable that other items reference instead of
  linking into an agent home, with path-reference tokens (`{{self}}`,
  `{{tools:name}}`, `{{path:ref}}`) expanded at install like `{{ns:}}`, and an
  optional per-item `build` hook for compiled tooling. Path tokens render the store
  root with a leading `~` when it lies under the home directory.
- Per-item install/uninstall hooks: an item declares `install`/`uninstall` shell
  commands (in `mind.toml` `[[items]]` or a tool's `TOOL.md`) that run on install
  and removal, gated by a disclosed safety prompt;
  `--dangerously-skip-install-hook-check` runs them unattended.
- Lifecycle hooks: multiple named `[[hooks]]`, optional hooks, and uninstall hooks
  that run at `unmeld`. Local source repos can be melded by filesystem path.
- Unmanaged lobe items: skills, agents, and rules present in an agent home that
  mind did not install are listed in `recall`/`probe` and removable via `forget`
  with a distinct not-managed-by-mind warning, including an "Unmanaged" group in
  the interactive browser.
- Curated super-sources: a source's `[discover].sources` registers a chain of
  other sources; `meld --install-super-sources` installs their items, a post-meld
  hint points to `probe`, and `sync` re-walks the chain to pick up newly listed
  nested sources.
- `review` flags path-token and tooling issues (unresolved tokens, hardcoded
  install paths, bare tool references, misplaced `{{ns:}}`, and helpers duplicated
  across items), and `--fix` rewrites the confidently-mappable ones; `init-source`
  reports the duplicate-tooling advisories too.
- `learn --all` installs every item of a source (sugar for `<source>#*`).
- Global `--json`, `--yes`, and `--ascii` flags, with color and Unicode glyph
  output gated on terminal capability and an ASCII fallback.
- `status` as an alias for `recall`.
- An mdBook documentation site (`make docs` builds and serves it locally).
- A multi-item `forget` confirms before removing.

### Changed

- `recall` with no argument is a status view of every melded source with its items
  and per-item install state; `recall --sources` narrows to the source list.
- `unmeld` uninstalls the source's installed items by default; `--unlink-only`
  keeps them.
- The `upgrade` "apply these upgrades?" prompt defaults to yes (a bare Enter
  applies; EOF still declines).
- `review`'s duplicate-tooling and own-resource advisories are non-prescriptive:
  sharing a helper as a `tool` and keeping the per-item copy are presented as
  equally valid, and a hardcoded own-resource path is noted to work but assume a
  fixed install location.

## [0.3.1] - 2026-06-22

### Fixed

- `meld --as <prefix>` on an already-melded source was ignored, leaving its items
  at their plain names. A re-meld with `--as` now updates the source's prefix and
  renames its installed items (and re-expands intra-source `{{ns:}}` references) to
  the new effective names; `--as ''` removes the prefix.

## [0.3.0] - 2026-06-22

### Added

- `learn --force` (`-f`) and `meld --force` overwrite a link target that already
  exists and is not managed by mind (a user's file, directory, or foreign link).
  Without `--force`, hitting such a conflict prompts on a TTY to overwrite that
  target and otherwise refuses, as before. The overwrite stays transactional:
  it is decided before staging, so a refusal changes nothing.

## [0.2.0] - 2026-06-22

### Added

- `meld` installs the source's items by default: it previews them and prompts,
  installing the whole source (the interactive form of `learn '<source>#*'`).
  `--link-only` registers without installing; `--yes` installs without
  prompting. Re-melding an already-melded source installs any missing items, or
  prints each item's install state and the commit it was installed from.
- `meld` with no repo argument melds the current directory, so running it inside
  a source repo registers and installs that source.
- `init-source`: a maintainer command that scaffolds a `mind.toml`, reports the
  references among a source's items, and (with `--template`) rewrites bare
  sibling references into `{{ns:}}` tokens so the source stays resolvable under a
  prefix.
- Namespacing: a source `prefix`, `{{ns:}}` reference tokens that expand to the
  effective (prefixed) name on install, and an unguarded-reference warning. When
  a source declares `[source].prefix`, an interactive `meld` previews the
  resulting names and asks whether to use that prefix, a different one, or none.
- Install hooks: a source declares `[source].install` in `mind.toml`, or a user
  supplies `meld --install-hook <cmd>`, to build the tooling its items rely on.
  Because the hook is arbitrary code, `mind` discloses it and prompts with three
  choices (run / skip but still install / abort). A non-TTY run skips it;
  `--dangerously-skip-install-hook-check` runs it unattended. `upgrade` (and
  `sync --upgrade`) re-run a hook when the source advances, and `mind review`
  surfaces a declared hook before melding.
- `evolve` updates the `mind` binary itself in place, resolving the same release
  artifact as the install script (no external crate). `--check` reports whether
  an update is available without changing anything; `--version <v>` targets an
  exact release.
- Enterprise managed policy: an admin-controlled file at a fixed system path
  restricts a client to a trusted-source allowlist, can require pinned sources,
  provisions an auto-meld base set, and locks the agent homes. Validate one with
  `mind review --policy <path>`. A worked example ships in `examples/policy/`.
- Within-source dependency resolution: selecting a subset of a source's items
  with `learn` also pulls in the source siblings those items reference (the
  `{{ns:}}` closure), printing a dependency tree and installing in dependency
  order. `--dry-run` previews it; `--yes` skips the prompt.
- Interactive TUI: `probe` with no flags opens a browser (Installed/Available
  tree, search, item preview) with full parity to the CLI verbs (install,
  remove, meld, unmeld, sync, upgrade). Installing on a source or group installs
  everything under it without naming each item. It is responsive to the terminal
  size with Unicode styling, and a double Ctrl-C force-exits from any mode. Falls
  back to the listing when piped or with `--no-tui`/`--json`.
- `review` validates a source for publishing (its `mind.toml`, item kinds,
  `{{ns:}}` references, and pin directive) without installing anything; with no
  target it validates the current directory. `review` and `init-source` share
  one finding-output format.
- SSH remotes: meld a `git@host:owner/repo` spec, or set `ssh = true` in the
  config so the `owner/repo` shorthand clones over SSH.
- Version pinning: `meld --follow-branch`/`--pin-tag`/`--pin-ref` and a
  `[source]` pin directive, recorded per source and honored by `sync`.
- Scan roots for monorepo/subtree sources: `[source].roots` and a repeatable
  `meld --root <dir>`.
- Curated super-source: `[discover].sources` melds nested sources recursively;
  `[discover]` supports per-kind include/exclude globs.
- Multiple agent homes ("lobes"): `config show` and `config lobes add/list/remove`;
  `learn` links into every configured home.
- `--json` output for `recall`, `probe`, and `introspect`; shell completions
  (`mind completions <shell>`) and a man page (`mind man`).
- `curl | sh` install script (with explicit https) and a Homebrew tap.
- Concurrency safety: a global advisory lock (`fd-lock`) and atomic registry and
  config writes via `Paths::atomic_write`.
- Smaller additions: `learn` glob selection and `--dry-run`, `forget` glob,
  `unmeld --forget`, `introspect --fix`, `sync --upgrade`, `probe`/`recall`
  `--kind`/`--source` filters, `probe` matching description text,
  `min-mind-version` enforcement, partial-`learn` persistence, and the
  `unlearn`/`detach` aliases.

### Changed

- Renamed the item-upgrade verb `evolve` to `upgrade` (and the `sync --evolve`
  flag to `sync --upgrade`), freeing `evolve` for binary self-update.
- Re-melding an already-melded source is no longer an error: it installs missing
  items or reports the source's item status instead.

### Fixed

- `evolve` detected `curl`/`wget` by spawning `command -v`, a shell builtin with
  no executable, so it always reported "need curl or wget on PATH" even with curl
  installed. The check now runs in a shell.

## [0.1.0] - 2026-06-17

### Added

- Initial release: the core verbs (`meld`, `unmeld`, `learn`, `forget`, `sync`,
  `evolve`, `recall`, `probe`, `introspect`), convention and `mind.toml`
  discovery, frontmatter descriptions, transactional install/upgrade/uninstall
  with a file registry, and a tag-driven release pipeline with a Homebrew tap.

[Unreleased]: https://github.com/jaemk/mind/compare/v0.6.2...HEAD
[0.6.2]: https://github.com/jaemk/mind/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/jaemk/mind/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/jaemk/mind/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/jaemk/mind/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/jaemk/mind/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/jaemk/mind/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/jaemk/mind/compare/v0.3.1...v0.4.1
[0.3.1]: https://github.com/jaemk/mind/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/jaemk/mind/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jaemk/mind/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jaemk/mind/releases/tag/v0.1.0
