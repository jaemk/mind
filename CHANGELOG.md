# Changelog

All notable changes to `mind` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-06-21

### Added

- Install hooks: a source declares `[source].install` in `mind.toml`, or a user
  supplies `meld --install-hook <cmd>`, to build the tooling its items rely on.
  Because the hook is arbitrary code, `mind` discloses it and prompts with three
  choices (run / skip but still install / abort). A non-TTY run skips it;
  `--dangerously-skip-install-hook-check` runs it unattended. `evolve` (and
  `sync --evolve`) re-run a hook when the source advances, and `mind review`
  surfaces a declared hook before melding.
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
  remove, meld, unmeld, sync, evolve). Falls back to the listing when piped or
  with `--no-tui`/`--json`.
- `review` verb: author-side source validation of a `mind.toml`, item kinds,
  `{{ns:}}` references, and pin directive, without installing anything.
- Version pinning: `meld --follow-branch`/`--pin-tag`/`--pin-ref` and a
  `[source]` pin directive, recorded per source and honored by `sync`.
- Scan roots for monorepo/subtree sources: `[source].roots` and a repeatable
  `meld --root <dir>`.
- Namespacing: a source `prefix`, `{{ns:}}` reference tokens that expand to the
  effective (prefixed) name on install, and an unguarded-reference warning.
- Curated super-source: `[discover].sources` melds nested sources recursively;
  `[discover]` supports per-kind include/exclude globs.
- Multiple agent homes ("lobes"): `config show` and `config lobes add/list/remove`;
  `learn` links into every configured home.
- `--json` output for `recall`, `probe`, and `introspect`; shell completions
  (`mind completions <shell>`) and a man page (`mind man`).
- `curl | sh` install script and a Homebrew tap.
- Concurrency safety: a global advisory lock (`fd-lock`) and atomic registry and
  config writes via `Paths::atomic_write`.
- Smaller additions: `learn` glob selection and `--dry-run`, `forget` glob,
  `unmeld --forget`, `introspect --fix`, `sync --evolve`, `probe`/`recall`
  `--kind`/`--source` filters, `probe` matching description text,
  `min-mind-version` enforcement, partial-`learn` persistence, and the
  `unlearn`/`detach` aliases.

## [0.1.0] - 2026-06-17

### Added

- Initial release: the core verbs (`meld`, `unmeld`, `learn`, `forget`, `sync`,
  `evolve`, `recall`, `probe`, `introspect`), convention and `mind.toml`
  discovery, frontmatter descriptions, transactional install/upgrade/uninstall
  with a file registry, and a tag-driven release pipeline with a Homebrew tap.

[0.2.0]: https://github.com/jaemk/mind/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jaemk/mind/releases/tag/v0.1.0
