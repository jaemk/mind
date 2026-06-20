# mind spec

The behavioral spec for `mind`, a manager for agent tooling (skills, agents,
rules) that melds arbitrary git repos and links installed items into `~/.claude`.
This directory is the reference the implementation and tests verify against.

## Feature status

Every feature is documented here before or as it lands, with its status. Status
values: `done` (implemented and covered by tests), `planned` (documented, not yet
built), `partial` (incomplete). Mark a feature `done` only once it is implemented
and verified.

| Feature | Status | Spec |
|---------|--------|------|
| Core verbs (meld, unmeld, learn, forget, sync, evolve, recall, probe, introspect) | done | [cli.md](cli.md) |
| `learn` glob selection + `--dry-run`; `probe` install/hash; aligned columns | done | CLI-31, CLI-32, CLI-33, CLI-81, CLI-82 |
| On-disk layout, source registry, manifest + file registry | done | [storage.md](storage.md) |
| Multiple agent homes (link into all configured dirs) | done | STO-14, LIFE-40 |
| `config show` + `config lobes` (manage agent homes) | done | CLI-110, CLI-111, CLI-112, CLI-113 |
| Source identity = `host/owner/repo` (collision fix, suffix selectors) | done | STO-13, CLI-5, CLI-20 |
| Convention discovery + frontmatter descriptions | done | [discovery.md](discovery.md) |
| `mind.toml`: `[source]`, `[[items]]`, `[discover]` item globs | done | [discovery.md](discovery.md) |
| Discover `include`/`exclude` per kind | done | DSC-37 |
| Curated super-source (`[discover].sources`, nested `as`) | done | DSC-38, DSC-39 |
| Namespacing: prefix, `{{ns:}}` tokens, unguarded-ref warning | done | [namespacing.md](namespacing.md) |
| Transactional install, upgrade, rename, uninstall, drift | done | [lifecycle.md](lifecycle.md) |
| `forget`/`recall`/`evolve` honor kind + source qualifier, error on ambiguity | done | CLI-40, CLI-63, CLI-71 |
| Clobber guard: refuse to overwrite a non-mind link target | done | LIFE-41 |
| Release pipeline + Homebrew tap (tag-driven) | done | `.github/workflows/release.yml`, `Formula/mind.rb` |
| `curl \| sh` install script (Linux/macOS-arm) | done | `resources/install.sh` |
| `forget` glob selection | done | CLI-41 |
| `unmeld --forget` (purge installed items) | done | CLI-22 |
| `introspect --fix` (re-link missing symlinks) | done | CLI-91 |
| `sync --evolve` (refresh then upgrade) | done | CLI-53 |
| `probe`/`recall` `--kind` / `--source` filters | done | CLI-83 |
| Enforce `min-mind-version` | done | DSC-40 |
| `sync` per-source resilience (continue + report, exit non-zero) | done | CLI-54 |
| `--json` output (recall, probe, introspect) | done | CLI-73, CLI-84, CLI-92 |
| Shell completions + man page | done | CLI-120, CLI-121 |
| Scan roots: `[source].roots` + `meld --root` (monorepo/subtree sources) | done | DSC-50, DSC-51, DSC-52, DSC-53, STO-17, CLI-16 |
| Version pinning: `--follow-branch`/`--pin-tag`/`--pin-ref` + `[source]` directive | done | DSC-41, STO-18, CLI-17, CLI-18, CLI-55 |
| `review` verb: author-side source validation | done | CLI-130, CLI-131, CLI-132, CLI-133 |
| `self-update` verb: in-place upgrade of the `mind` binary (via the `self_update` crate) | planned | CLI-140, CLI-141, CLI-142, CLI-143 |
| Concurrency: global advisory lock + atomic registry writes (via `fd-lock`) | done | STO-40, STO-41, STO-42, STO-43 |
| `probe` matches description text, not just name | done | CLI-85 |
| README quickstart, mental model, troubleshooting/FAQ | planned | [../README.md](../README.md) |
| Starter source example (plain convention layout) | planned | [../examples/](../examples/) |
| Interactive TUI: `probe` default, Installed/Available tree, full-parity actions, preview + registry meld | done | [tui.md](tui.md) |

## Documents

- [cli.md](cli.md) - the command surface: verbs, flags, output, exit status.
- [storage.md](storage.md) - on-disk layout, the source registry, the manifest.
- [discovery.md](discovery.md) - how a source's items are discovered and described.
- [namespacing.md](namespacing.md) - prefixes, `{{ns:}}` reference tokens, warnings.
- [lifecycle.md](lifecycle.md) - install, upgrade, uninstall, and drift semantics.
- [tui.md](tui.md) - the interactive TUI (`probe` default): browse, search, and
  the interactive front end for the CLI verbs.

## Conventions

- Each normative statement has a stable ID (e.g. `CLI-30`, `LIFE-14`). Tests cite
  these IDs (in `// spec: ID` comments) so a spec line maps to its verification.
  IDs are append-only: retire an ID by marking it removed, never reuse the number.
- A coverage gate (`tests/spec_coverage.rs`, run by `cargo test` and CI) fails
  when a defined ID is neither cited by a test nor in its ALLOWLIST. Adding a new
  requirement therefore forces a coverage decision: write a citing test, or
  allowlist it with a reason.
- "item" means a skill, agent, or rule. "source" means a melded repo. "store"
  means `~/.mind/store`. "link" means a symlink under `~/.claude`.
- Statements use present-tense declaratives ("`mind learn` installs ..."). Where
  ordering matters it is stated explicitly.
- Paths honor the `MIND_HOME` and `CLAUDE_HOME` overrides (see storage.md).

## Glossary

- bare name: an item's name as it appears in its source repo.
- effective name: the installed name, `<prefix>-<bare>` when namespaced, else the
  bare name.
- effective prefix: the namespace in force for a source (see namespacing.md).
- stable identity: `(source, kind, bare_name)`. Survives a prefix change.
- file registry: the `store` + `links` paths a manifest entry records for an item.
