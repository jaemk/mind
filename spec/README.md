# mind spec

The behavioral spec for `mind`, a manager for agent tooling (skills, agents,
rules, tools) that melds arbitrary git repos and links installed items into
`~/.claude` (a tool is store-only and reached by reference, not linked).
This directory is the reference the implementation and tests verify against.

## Feature status

Every feature is documented here before or as it lands, with its status. Status
values: `done` (implemented and covered by tests), `planned` (documented, not yet
built), `partial` (incomplete). Mark a feature `done` only once it is implemented
and verified.

| Feature | Status | Spec |
|---------|--------|------|
| Core verbs (meld, unmeld, learn, forget, sync, upgrade, recall, probe, introspect) | done | [cli.md](cli.md) |
| `learn` glob selection + `--dry-run`; `probe` install/hash; aligned columns | done | CLI-31, CLI-32, CLI-33, CLI-81, CLI-82 |
| `learn --all`: install every item of a source (sugar for `<source>#*`) | done | CLI-36 |
| On-disk layout, source registry, manifest + file registry | done | [storage.md](storage.md) |
| Multiple agent homes (link into all configured dirs) | done | STO-14, LIFE-40 |
| `config show` + `config lobes` (manage agent homes) | done | CLI-110, CLI-111, CLI-112, CLI-113 |
| Source identity = `host/owner/repo` (collision fix, suffix selectors) | done | STO-13, CLI-5, CLI-20 |
| Convention discovery + frontmatter descriptions | done | [discovery.md](discovery.md) |
| Frontmatter reader interprets folded/literal block scalars (`>-`, `\|-`) | done | DSC-22 |
| `recall`/`probe` mark an item out of date on source-content hash drift (local dirs, manual edits) | done | CLI-75 |
| `mind.toml`: `[source]`, `[[items]]`, `[discover]` item globs | done | [discovery.md](discovery.md) |
| Discover `include`/`exclude` per kind | done | DSC-37 |
| Curated super-source (`[discover].sources`, nested `as`) | done | DSC-38, DSC-39 |
| Super-source meld registers the chain but auto-installs only its own items | done | DSC-54 |
| Super-source: `--recursive`, per-source `install = true`, post-meld `probe` hint, `sync` re-walks the discover chain | done | DSC-55, DSC-56, DSC-57, DSC-58 |
| Curator adopts an un-onboarded nested source: per-entry `follow-branch`/`roots`/`[[hooks]]`, applied only when it has no `mind.toml` | done | DSC-59, DSC-60, DSC-61 |
| Namespacing: prefix, `{{ns:}}` tokens, unguarded-ref warning | done | [namespacing.md](namespacing.md) |
| Transactional install, upgrade, rename, uninstall, drift | done | [lifecycle.md](lifecycle.md) |
| `forget`/`recall`/`upgrade` honor kind + source qualifier, error on ambiguity | done | CLI-40, CLI-63, CLI-71 |
| Clobber guard: refuse to overwrite a non-mind link target | done | LIFE-41 |
| Release pipeline + Homebrew tap (tag-driven) | done | `.github/workflows/release.yml`, `Formula/mind.rb` |
| `curl \| sh` install script (Linux/macOS-arm) | done | `resources/install.sh` |
| `forget` glob selection | done | CLI-41 |
| `unmeld` uninstalls source items by default; `--unlink-only` keeps them | done | CLI-21, CLI-22 |
| `unmeld` glob/partial source selection (multi-source, mirrors `learn`/`forget` globs) | done | CLI-28 |
| `introspect --fix` (re-link missing symlinks) | done | CLI-91 |
| `sync --upgrade` (refresh then upgrade) | done | CLI-53 |
| `recall` (no arg): source+item status view, install state per item | done | CLI-70, CLI-74, CLI-83 |
| `probe`/`recall` `--kind` / `--source` filters | done | CLI-83 |
| `probe`/`recall` `--source` accepts a source glob (mirrors `unmeld` glob) | done | CLI-86 |
| Enforce `min-mind-version` | done | DSC-40 |
| `sync` per-source resilience (continue + report, exit non-zero) | done | CLI-54 |
| `--json` output (recall, probe, introspect) | done | CLI-73, CLI-84, CLI-92 |
| Shell completions + man page | done | CLI-120, CLI-121 |
| Scan roots: `[source].roots` + `meld --root` (monorepo/subtree sources) | done | DSC-50, DSC-51, DSC-52, DSC-53, STO-17, CLI-16 |
| Version pinning: `--follow-branch`/`--pin-tag`/`--pin-ref` + `[source]` directive | done | DSC-41, STO-18, CLI-17, CLI-18, CLI-55 |
| `review` verb: author-side source validation | done | CLI-130, CLI-131, CLI-132, CLI-133 |
| `review` flags path tokens + hardcoded paths + bare tool refs + misplaced `{{ns:}}`; `--fix` rewrites | done | CLI-135, CLI-136, CLI-137, CLI-138, CLI-139, CLI-145, NS-24 |
| `review`/`init-source` flag helper scripts duplicated across items (`duplicate-tooling`) | done | CLI-144, INIT-7 |
| `evolve` verb: in-place upgrade of the `mind` binary | done | CLI-140, CLI-141, CLI-142, CLI-143 |
| Managed policy (enterprise): trusted-source allowlist, require-pinned, auto-meld, lobe lock; `mind review --policy` | done | [policy.md](policy.md) |
| Install hooks: `[source].install` / `meld --install-hook`, safety prompt, `--dangerously-skip-install-hook-check` | done | [install-hooks.md](install-hooks.md) |
| Lifecycle hooks: multiple named `[[hooks]]`, optional hooks, uninstall hooks at `unmeld`, `init-source` scaffold | done | [install-hooks.md](install-hooks.md) (HOOK-50..60) |
| Within-source dependency resolution: a partial `learn` pulls in referenced siblings; dependency-tree display + install order | done | [dependencies.md](dependencies.md) |
| Explicit item dependencies: optional `requires:` frontmatter key, unioned with the `{{ns:}}`-derived edges | done | DEP-4, DEP-5, DEP-6 |
| Dependency-graph operations: `forget` warns about dependents, `recall --tree`, non-interactive `probe` tree + `--json` edges | done | DEP-60, DEP-61, DEP-62 |
| TUI dependency navigation: expand an item to its dependency subtree, Enter on a dependency jumps to its item | done | TUI-50, TUI-51 |
| `recall --tree --json`: structured (nested) dependency forest output | done | DEP-63 |
| `meld` installs by default (`--link-only`/`--yes`); no-arg melds `.`; prefix prompt when declared | done (`--link-only`/`--yes`/prefix) | CLI-23, CLI-24 |
| `meld` with no arg defaults to the current directory | done | CLI-25 |
| `init-source`: scaffold `mind.toml`, detect references, `{{ns:}}` templating (maintainer) | done | [init-source.md](init-source.md) |
| Deprecate `[source].install` (still parsed); `review` advises the `[[hooks]]` form, `init-source` scaffolds only `[[hooks]]` | done | HOOK-90 |
| Item `[[items.hooks]]` array (parity with source `[[hooks]]`); nested lifecycle order `source.install -> item.install* ... item.uninstall* -> source.uninstall` | done | HOOK-86, HOOK-87 |
| `init-source` flags bare sibling references only under a prefix; `hardcoded-path`/`bare-tool` messages note install-hook-populated locations are safe | done | INIT-9, CLI-146 |
| Concurrency: global advisory lock + atomic registry writes (via `fd-lock`) | done | STO-40, STO-41, STO-42, STO-43 |
| `probe` matches description text, not just name | done | CLI-85 |
| README quickstart, mental model, troubleshooting/FAQ | done | [../README.md](../README.md) |
| Starter source example (plain convention layout) | done | [../examples/](../examples/) |
| Interactive TUI: `probe` default, Installed/Available tree, full-parity actions, preview + registry meld | done | [tui.md](tui.md) |
| `probe -n` short form of `--no-tui` | done | TUI-3 |
| `tool` item kind: store-only installable, referenced not discovered | done | [tooling.md](tooling.md) (TOOL-1..7) |
| Path-reference tokens `{{self}}` / `{{tools:name}}` / `{{path:ref}}` | done | [tooling.md](tooling.md) (TOOL-10..16) |
| Item build hooks: per-item `build`, staging-time, transactional | done | [install-hooks.md](install-hooks.md) (HOOK-70..73) |
| Per-item install/uninstall hooks: host side effects at install/removal, re-run on upgrade | done | [install-hooks.md](install-hooks.md) (HOOK-80..85) |
| Polished output: global `--json`/`--yes`/`--ascii`, color+Unicode gate, structured JSON results | done | CLI-150, CLI-151, CLI-152, CLI-153, CLI-154 |
| Unmanaged lobe items: `recall`/`probe` listing + `forget` with a not-managed-by-mind warning | done | [unmanaged.md](unmanaged.md) (UNM-1..5) |
| Unmanaged items in the `probe` TUI group node | done | UNM-6 |
| `forget --unmanaged [glob]`: bulk-remove unmanaged lobe items (the default glob stays managed-only) | done | UNM-7, UNM-8 |
| `absorb`: claim an unmanaged lobe item into a version-controlled source, then install it managed | planned | [absorb.md](absorb.md) |
| `dump`: generate a pinned super-source `mind.toml` from the installed set (`--whole-sources`) | planned | [dump.md](dump.md) |
| `[discover].sources` `install_items`: install only a named subset of a nested source | planned | DSC-62, DSC-63, DSC-64 |
| TUI: keep the highlighted row in the middle two-thirds (scroll margin) | done | TUI-16 |
| TUI: Enter opens a details dialog with the node's valid actions | done | TUI-26 |

## Documents

- [cli.md](cli.md) - the command surface: verbs, flags, output, exit status.
- [storage.md](storage.md) - on-disk layout, the source registry, the manifest.
- [discovery.md](discovery.md) - how a source's items are discovered and described.
- [namespacing.md](namespacing.md) - prefixes, `{{ns:}}` reference tokens, warnings.
- [lifecycle.md](lifecycle.md) - install, upgrade, uninstall, and drift semantics.
- [dependencies.md](dependencies.md) - within-source dependency resolution: a
  partial `learn` pulls in referenced siblings, with a dependency tree and order.
- [tui.md](tui.md) - the interactive TUI (`probe` default): browse, search, and
  the interactive front end for the CLI verbs.
- [policy.md](policy.md) - the enterprise managed policy: a fixed-path,
  admin-controlled file that restricts a client to trusted sources and locks
  related settings.
- [install-hooks.md](install-hooks.md) - install hooks: a source-declared or
  user-supplied build command, gated by a safety prompt before it runs; and
  item-level build hooks (HOOK-70..73) that build an item's tooling at install.
- [tooling.md](tooling.md) - resource and helper tooling: the `tool` item kind,
  path-reference tokens (`{{self}}`, `{{tools:name}}`, `{{path:ref}}`), and how an
  item references the tooling it ships.
- [init-source.md](init-source.md) - `init-source`, the maintainer scaffolder:
  generate a `mind.toml`, report the intra-source reference graph, and add
  `{{ns:}}` templating.
- [absorb.md](absorb.md) - `absorb`: claim an unmanaged lobe item into a
  version-controlled source the user owns, then install it through the managed path.
- [dump.md](dump.md) - `dump`: generate a pinned super-source `mind.toml` from the
  melded and installed state, to reproduce or share an agent home.
- [unmanaged.md](unmanaged.md) - unmanaged lobe items: skills/agents/rules present
  in an agent home that `mind` did not install, surfaced in `recall`/`probe` and
  removable via `forget` with a distinct not-managed-by-mind warning.

## Conventions

- Each normative statement has a stable ID (e.g. `CLI-30`, `LIFE-14`). Tests cite
  these IDs (in `// spec: ID` comments) so a spec line maps to its verification.
  IDs are append-only: retire an ID by marking it removed, never reuse the number.
- A coverage gate (`tests/spec_coverage.rs`, run by `cargo test` and CI) fails
  when a defined ID is neither cited by a test nor in its ALLOWLIST. Adding a new
  requirement therefore forces a coverage decision: write a citing test, or
  allowlist it with a reason.
- "item" means a skill, agent, rule, or tool. "source" means a melded repo. "store"
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
