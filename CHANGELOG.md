# Changelog

All notable changes to `mind` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/jaemk/mind/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/jaemk/mind/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/jaemk/mind/compare/v0.3.1...v0.4.1
[0.3.1]: https://github.com/jaemk/mind/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/jaemk/mind/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/jaemk/mind/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jaemk/mind/releases/tag/v0.1.0
