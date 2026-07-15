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
| `recall` uses a distinct left-edge marker (stale `^`/`↑`) for an installed-but-out-of-date item | done | CLI-155 |
| `mind.toml`: `[source]`, `[[items]]`, `[discover]` item globs | done | [discovery.md](discovery.md) |
| Discover `include`/`exclude` per kind | done | DSC-37 |
| Curated super-source (`[discover].sources`, nested `as`) | done | DSC-38, DSC-39 |
| Super-source meld registers the chain but auto-installs only its own items | done | DSC-54 |
| Super-source: `--recursive`, per-source `install = true`, post-meld `probe` hint, `sync` re-walks the discover chain | done | DSC-55, DSC-56, DSC-57, DSC-58 |
| Curator adopts an un-onboarded nested source: per-entry `follow-branch`/`roots`/`[[hooks]]`, applied only when it has no `mind.toml` | done | DSC-59, DSC-60, DSC-61 |
| Namespacing: prefix, `{{ns:}}` tokens, unguarded-ref warning | done | [namespacing.md](namespacing.md) |
| `--verbose`/`-v` global flag; gates unguarded-ref warning | done | CLI-162 |
| Namespace separator is `:` (reserved kind words rejected; ref parser disambiguates; old `-` installs rename on upgrade) | done | NS-25, NS-26, NS-27 |
| `meld`/`review` `--namespace`/`-n` flag (renames `--as`, still a hidden alias) | done | CLI-159 |
| Namespace mutable only until items install; changing it after requires forget-first (revises in-place rename) | done | NS-30, CLI-161 |
| Agents not namespaced: an agent links under its bare frontmatter `name` (the harness keys agents by frontmatter, not filename); same-named agents across sources are a detected collision | done | NS-40, NS-41, NS-42 |
| TUI: show + edit a source's install namespace in the details dialog (editable until items installed) | done | TUI-53 |
| Transactional install, upgrade, rename, uninstall, drift | done | [lifecycle.md](lifecycle.md) |
| `forget`/`recall`/`upgrade` honor kind + source qualifier, error on ambiguity | done | CLI-40, CLI-63, CLI-71 |
| Clobber guard: refuse to overwrite a non-mind link target | done | LIFE-41 |
| Release pipeline + Homebrew tap (tag-driven) | done | `.github/workflows/release.yml`, `Formula/mind.rb` |
| `curl \| sh` install script (Linux/macOS-arm) | done | `resources/install.sh` |
| `forget` glob selection | done | CLI-41 |
| `upgrade` glob selection (mirrors `forget`; namespace/kind/source in one pass) | done | CLI-65 |
| `unmeld` uninstalls source items by default; `--unlink-only` keeps them | done | CLI-21, CLI-22 |
| `unmeld` glob/partial source selection (multi-source, mirrors `learn`/`forget` globs) | done | CLI-28 |
| `introspect --fix` (re-link missing symlinks) | done | CLI-91 |
| `sync --upgrade` (refresh then upgrade) | done | CLI-53 |
| `recall` (no arg): source+item status view, install state per item | done | CLI-70, CLI-74, CLI-83 |
| `probe`/`recall` `--kind` / `--source` filters | done | CLI-83 |
| `probe`/`recall` `--source` accepts a source glob (mirrors `unmeld` glob) | done | CLI-86 |
| Enforce `min-mind-version` | done | DSC-40 |
| `sync` per-source resilience (continue + report, exit non-zero) | done | CLI-54 |
| `--json` output (recall, probe, introspect) | done | CLI-73, CLI-84, CLI-92, CLI-189 |
| Shell completions + man page | done | CLI-120, CLI-121 |
| Scan roots: `[source].roots` + `meld --root` (monorepo/subtree sources) | done | DSC-50, DSC-51, DSC-52, DSC-53, STO-17, CLI-16 |
| Flat skill layout: `[source].flat-skills` + `meld --flat-skills` + per-entry `[[discover.sources]]` flag (skill dirs at a root, no `skills/` container); `dump` propagates it | done | DSC-74, DSC-75, DSC-76, DSC-77, STO-44, CLI-158, DUMP-10 |
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
| `meld` installs by default (`--register-only`/`--yes`); no-arg melds `.`; prefix prompt when declared | done | CLI-23, CLI-24 |
| `meld` with no arg defaults to the current directory | done | CLI-25 |
| `init-source`: scaffold `mind.toml`, detect references, `{{ns:}}` templating (maintainer) | done | [init-source.md](init-source.md) |
| Deprecate `[source].install` (still parsed); `review` advises the `[[hooks]]` form, `init-source` scaffolds only `[[hooks]]` | done | HOOK-90 |
| Item `[[items.hooks]]` array (parity with source `[[hooks]]`); nested lifecycle order `source.install -> item.install* ... item.uninstall* -> source.uninstall` | done | HOOK-86, HOOK-87 |
| `init-source` flags bare sibling references only under a prefix; `hardcoded-path`/`bare-tool` messages note install-hook-populated locations are safe | done | INIT-9, CLI-146 |
| Concurrency: global advisory lock + atomic registry writes (via `fd-lock`) | done | STO-40, STO-41, STO-42, STO-43 |
| `probe` matches description text, not just name | done | CLI-85 |
| README quickstart and mental model; troubleshooting/FAQ on the docs site | done | [../README.md](../README.md), [../docs/src/](../docs/src/) |
| Starter source example (plain convention layout) | done | [../examples/](../examples/) |
| Interactive TUI: `probe` default, Installed/Available tree, full-parity actions, preview + registry meld | done | [tui.md](tui.md) |
| `probe -n` short form of `--no-tui` | removed (TUI-54: `--no-tui` is long-only) | TUI-3 |
| `tool` item kind: store-only installable, referenced not discovered | done | [tooling.md](tooling.md) (TOOL-1..7) |
| Path-reference tokens `{{self}}` / `{{tools:name}}` / `{{path:ref}}` | done | [tooling.md](tooling.md) (TOOL-10..16) |
| `{{tools:name}}` `BadReference` names its cause (a miss vs. a tool with no resolvable bin) | done | [tooling.md](tooling.md) (TOOL-17) |
| `{{path:ref}}` `BadReference` names its cause (a miss vs. an under-qualified cross-kind ambiguity) | done | [tooling.md](tooling.md) (TOOL-18) |
| `requires` install-time `BadReference` names its cause (malformed / cross-source / ambiguous / miss) | done | [dependencies.md](dependencies.md) (DEP-7) |
| `review` `unshipped-tooling`: a tool whose entrypoint resolves only via a git-untracked file (works locally, breaks on clone) | done | CLI-190 |
| `review` `unshipped-tooling` extends to any item's `{{self}}`/`{{path:}}` bundled files git does not track | done | CLI-191 |
| `review` `ns-tool-reference`: a `{{ns:name}}` naming a store-only tool by its bare (non-runnable) name | done | CLI-192 |
| `review` `unshipped-tooling`: an authoritative `mind.toml` git does not track (applies locally, absent from a clone) | done | CLI-193 |
| Item build hooks: per-item `build`, staging-time, transactional | done | [install-hooks.md](install-hooks.md) (HOOK-70..73) |
| Per-item install/uninstall hooks: host side effects at install/removal, re-run on upgrade | done | [install-hooks.md](install-hooks.md) (HOOK-80..85) |
| Polished output: global `--json`/`--yes`/`--ascii`, color+Unicode gate, structured JSON results | done | CLI-150, CLI-151, CLI-152, CLI-153, CLI-154 |
| Unmanaged lobe items: `recall`/`probe` listing + `forget` with a not-managed-by-mind warning | done | [unmanaged.md](unmanaged.md) (UNM-1..5) |
| Unmanaged items in the `probe` TUI group node | done | UNM-6 |
| `forget --unmanaged [glob]`: bulk-remove unmanaged lobe items (the default glob stays managed-only) | done | UNM-7, UNM-8 |
| `absorb`: claim an unmanaged lobe item into a version-controlled source, then install it managed | done | [absorb.md](absorb.md) |
| `dump`: generate a pinned super-source `mind.toml` from the installed set (`--whole-sources`) | done | [dump.md](dump.md) |
| `[discover].sources` `install-items`: install only a named subset of a nested source | done | DSC-62, DSC-63, DSC-64 |
| Pin/ref value validation at parse time + `--` terminator in git subcommands | done | DSC-66 |
| `[[items]]` traversal guard: reject an unsafe `name`, escaping `link`, or out-of-clone `path` | done | DSC-71, DSC-72, DSC-73 |
| auth failure handling for nested sources: `on-auth-failure = { action, message }` per entry | done | DSC-68, DSC-69 |
| `on-auth-failure` scope: descendant auth failures are not attributed to the entry | done | DSC-70 |
| Rename `[discover].sources` alias key to `namespace`; `as` stays as backward-compat alias; `dump` emits canonical `namespace =` | done | DSC-78 |
| TUI: keep the highlighted row in the middle two-thirds (scroll margin) | done | TUI-16 |
| TUI: Enter opens a details dialog with the node's valid actions | done | TUI-26 |
| Cross-harness lobes: per-lobe `kinds` filter, non-Claude home presets (Gemini/Codex/Windsurf/Antigravity), auto-detect-and-prompt | done | [harness-lobes.md](harness-lobes.md) (HARN-1..6) |
| Consume Claude plugin marketplaces: `.claude-plugin/marketplace.json` + `plugin.json` read as a discovery source, own store+symlink install unchanged | done | [marketplace.md](marketplace.md) (MKT-1..11) |
| Marketplace + curator compose: a co-present `mind.toml` `[discover].sources` layers on a `.claude-plugin/` manifest; `roots`/`flat-skills`/`[[items]]`/`[discover]` globs suppress the manifest's own-item layer | done | MKT-15, MKT-16 |
| Graceful degradation of nested non-auth clone failures (skip + curator-empty guard) | done | DSC-79, DSC-80 |
| Namespace prefix is a safe path component; future kind words reserved (command, hook, mcp, plugin, prompt, mode, output-style) | done | NS-28, NS-29 |
| `[discover]` glob confinement: reject absolute/`..` patterns, canonicalize matches into the clone | done | DSC-81 |
| `evolve` integrity: SHA256SUMS verification before extraction, unique staging name, exclusive lock (self-managed, no outer lock) | done | STO-45, STO-46, STO-47, STO-48 |
| Uninstall confinement: recorded paths must resolve under the store or a configured lobe | done | LIFE-44 |
| State-file schema versions in sources.json/manifest.json (absent = 1; newer errors) | done | STO-50, STO-51 |
| Content-hash framing: length-prefixed fields, type-tagged symlinks | done | LIFE-35 |
| TUI sanitization: source-derived strings stripped of ANSI/control/bidi at the model boundary | done | TUI-60 |
| Managed-policy pin values validated as git refs | done | POL-33 |
| Managed-policy `[binary].self-update` control: disable, pin to a version, or allow | done | POL-51, POL-52, POL-53, POL-54 |
| Managed-policy `auto_meld` pin reconciliation: pin-bump propagates to already-provisioned machines on next `sync` | done | POL-55 |
| Managed-policy `[sources].allow-local` knob: forbid local-path and `file://` melds under lock | done | POL-56, POL-57 |
| Managed-policy `auto_meld` `install = true`: headless item install after provisioning; `run-build-hooks` opt-in; per-item soft-fail | done | POL-58, POL-59, POL-60 |
| Managed-policy `min-mind-version` gate: checked before strict parse; gives a clear error on schema skew instead of an opaque unknown-field error | done | POL-61, POL-62, POL-63 |
| Managed-policy permission warning: warn when the system policy file or its parent dir is group/world-writable or not root-owned; skipped for `$MIND_POLICY_FILE` | done | POL-64, POL-65 |
| Managed-policy pin skew warning: when running binary is above the policy pin, print a human-only warning that the pin is an upper bound and does not downgrade; `--json` outcome is the machine hook | done | POL-66 |
| `evolve`/install.sh network fetch timeouts (`MIND_HTTP_TIMEOUT_SECS`) | done | STO-52 |
| Actionable git-failure hints: auth (SSH/config/helper), proxy (407); clone errors lead with stderr, detail behind `--verbose`; `learn` typo points at `probe` | done | CLI-177, CLI-178, CLI-179, CLI-180 |
| `--json` error envelope on stdout (`{"schema":1,"error":{"kind","message"}}`); stable per-variant `kind`; clap usage errors stay text | done | CLI-181, CLI-182, CLI-183 |
| `-n` reserved for `--dry-run`; `-N` short for `--namespace`; `probe --no-tui` long-only | done | CLI-163, CLI-164, TUI-54 |
| `meld --register-only` / `unmeld --keep-items` (old spellings hidden deprecated aliases) | done | CLI-165, CLI-166 |
| JSON envelopes: `{"schema": 1, "items": [...]}` for read verbs; `"schema": 1` on mutating results | done | CLI-167, CLI-168 |
| `upgrade` syncs involved sources first; `--no-sync` opt-out; `sync --upgrade` deprecated sugar | done | CLI-169 |
| `MIND_DEFAULT_LOBE` env var; `CLAUDE_HOME` legacy fallback | done | CLI-170 |
| config.toml `absorb-to` canonical (kebab); `absorb_to` parse alias | done | CLI-171 |
| Conventional verb aliases (add/install/uninstall/update/search/list/doctor/self-update); `detach` and `config target` removed | done | CLI-172 |
| meld/unmeld help states the install/uninstall defaults | done | CLI-173, CLI-174 |
| Exit-code contract: 0 success, 1 runtime error, 2 usage error | done | CLI-175 |
| `--dangerously-skip-build-hook-check`: run item build hooks non-interactively (CI installs) | done | HOOK-74 |
| `[source].namespace` canonical mind.toml key; `prefix` deprecated parse alias; `init-source` rewrites | done | DSC-82 |
| Frontmatter reader strips a leading UTF-8 BOM | done | DSC-23 |
| `compare_url` suppressed for gitlab/bitbucket hosts (GitHub-shaped link was wrong for those forges) | done | CLI-188 |
| `introspect --json` includes `"schema": 1`; shape is `{"schema":1,"issues":[...],"sources":N,"items":N}` | done | CLI-189 |
| Hook consent disclosure adds a commit-pinned version-control browse URL alongside the labeled on-disk clone path | done | HOOK-24 |
| `mind hooks run` / `hooks list`: run or inspect a source's and items' hooks on demand (rerun skipped/failed/lost hooks) | done | [install-hooks.md](install-hooks.md) (HOOK-100..104), CLI-194, CLI-195, CLI-196 |
| `meld --add-root`: compose extra convention roots with a manifest or authoritative source (install items a `marketplace.json` does not list) | done | DSC-84, DSC-85, DSC-86, MKT-17, STO-55, CLI-197 |
| Item links: `learn`/`meld` a deep `tree`/`blob` skill URL as a single-item source instance (`host/owner/repo#path` identity, duplicates coexist) | done | [item-link.md](item-link.md) (LNK-1..12) |
| Item links in `dump`: emit a link instance as a reconstructed deep-URL source entry | planned | LNK-13 |

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
- [harness-lobes.md](harness-lobes.md) - cross-harness lobes: link skills and
  agents into non-Claude agent homes (Gemini CLI, Codex CLI, Antigravity) via a
  per-lobe `kinds` filter and detected-home presets.
- [item-link.md](item-link.md) - item links: a deep `tree`/`blob` URL to one
  skill inside a repo, consumed as its own single-item source instance with an
  extended `host/owner/repo#path` identity; several links into the same repo
  coexist as separate sources.
- [marketplace.md](marketplace.md) - consume Claude Code's native plugin manifests
  (`.claude-plugin/marketplace.json`, `plugin.json`) as a discovery source so a
  repo published for the built-in plugin system melds without re-packaging; the
  manifest is an input (a source), not a sink, and the store+symlink install model
  is unchanged.

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
- effective name: the installed name, `<prefix>:<bare>` when namespaced, else the
  bare name.
- effective prefix: the namespace in force for a source (see namespacing.md).
- stable identity: `(source, kind, bare_name)`. Survives a prefix change.
- file registry: the `store` + `links` paths a manifest entry records for an item.
