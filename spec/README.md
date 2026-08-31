# mind spec

The behavioral spec for `mind`, a manager for agent tooling (skills, agents,
rules, commands, tools) that melds arbitrary git repos and links installed items into
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
| Namespace mutable only until items install; changing it after requires forget-first (in-place change renames identity + relocates clone) | done | NS-30, CLI-161 |
| Identity alias (pre-clone `--as`/curated-`as`/marketplace-entry) is part of source identity: melds a distinct `host/owner/repo@<alias>` instance (composes with item-link `#<path>`), coexisting with the bare repo and other aliases at independent pins/clones; a post-clone display prefix (accepted `[source].prefix`/collision) is not identity; forking a new instance prints an explicit note | done | STO-58, STO-59, STO-60 |
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
| Version pinning: single `--pin` (`HEAD`/ref freeze, `branch=`/`tag=` follow) + deprecated `--follow-branch`/`--pin-tag`/`--pin-ref` aliases + `[source]` directive + `learn <url> --pin` note when already melded | done | DSC-41, STO-18, CLI-17, CLI-18, CLI-200, CLI-201, CLI-202, CLI-203, CLI-55 |
| `review` verb: author-side source validation | done | CLI-130, CLI-131, CLI-132, CLI-133 |
| `review` flags path tokens + hardcoded paths + bare tool refs + misplaced `{{ns:}}`; `--fix` rewrites via a CommonMark parse (code spans, fences, containers, link syntax, brace spans) | done | CLI-135, CLI-136, CLI-137, CLI-138, CLI-139, CLI-145, NS-24, NS-46, NS-47, NS-48, NS-49, NS-50, NS-51, NS-52 |
| `{{ns:}}`/path-token expansion, `review --fix`'s rewrites, and the unguarded-reference scan are markdown-file only (an extension test, `namespace::is_markdown`), reading a CommonMark structure map so code spans/blocks and link syntax are never touched; `templatize` also wraps a bare sibling mention in the frontmatter `description:` value, the one frontmatter field that is free prose | done | NS-53, NS-54, NS-55, NS-56, TOOL-19 |
| `review` flags any `{{...}}` token found in a non-markdown item file (`inert-token`), resolvable or not, since none of them expand outside markdown | done | CLI-223 |
| Opt-in token expansion in a non-markdown file: an item's `expand:` frontmatter lists item-relative files scanned like markdown, path tokens rendered absolute; a bad entry is a hard install/`review` error; convention discovery stays on | done | NS-57, TOOL-20, CLI-226 |
| `review` finding messages are sanitized (`strip_ansi`) at construction, so both the text and `--json` output inherit it | done | CLI-224 |
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
| Source-controlled item names sanitized at every CLI human/`--json` print site (display accessors; the `ItemKey` newtype withholds `Display` so the default formatting path can't leak, while `as_str()`/`Into<String>` remain explicit, discipline-guarded escape hatches); scan-error messages sanitize echoed source strings | done | DSC-95 |
| auth failure handling for nested sources: `on-auth-failure = { action, message }` per entry | done | DSC-68, DSC-69 |
| `on-auth-failure` scope: descendant auth failures are not attributed to the entry | done | DSC-70 |
| Rename `[discover].sources` alias key to `namespace`; `as` stays as backward-compat alias; `dump` emits canonical `namespace =` | done | DSC-78 |
| TUI: keep the highlighted row in the middle two-thirds (scroll margin) | done | TUI-16 |
| TUI: Enter opens a details dialog with the node's valid actions | done | TUI-26 |
| Cross-harness lobes: per-lobe `kinds` filter, non-Claude home presets (Gemini/Codex/Windsurf/Antigravity), auto-detect-and-prompt | done | [harness-lobes.md](harness-lobes.md) (HARN-1..9) |
| Project lobes: `link-project`, `--preset`+base, `--subdir`, `--snapshot` freeze, `remove --snapshot`, `introspect` vanished-lobe prune | done | [harness-lobes.md](harness-lobes.md) (HARN-10..13), CLI-198, CLI-199 |
| Consume Claude plugin marketplaces: `.claude-plugin/marketplace.json` + `plugin.json` read as a discovery source, own store+symlink install unchanged | done | [marketplace.md](marketplace.md) (MKT-1..11) |
| Marketplace + curator compose: a co-present `mind.toml` `[discover].sources` layers on a `.claude-plugin/` manifest; `roots`/`flat-skills`/`[[items]]`/`[discover]` globs suppress the manifest's own-item layer | done | MKT-15, MKT-16 |
| Graceful degradation of nested non-auth clone failures (skip + curator-empty guard) | done | DSC-79, DSC-80 |
| Namespace prefix is a safe path component; future kind words reserved (command, hook, mcp, plugin, prompt, mode, output-style) | done | NS-28, NS-29 |
| `is_safe_prefix_component` guard rejects multi-byte control/bidi/zero-width the byte scan misses, for both an auto-generated prefix and a user-supplied `--namespace`/`[source].prefix` (`validate_prefix` shares the same guard) | done | NS-72 |
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
| `evolve` GitHub API auth: send `GH_TOKEN`/`GITHUB_TOKEN` as a bearer header on `api.github.com` to escape the unauthenticated per-IP 403 rate limit, never forwarded to the artifact host across a redirect | done | STO-57 |
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
| `mind hooks run` / `hooks list`: run or inspect a source's and items' hooks on demand (rerun skipped/failed/lost hooks); a target matching a registered source identity resolves as a source even when it contains `#`; an item's install hooks already run at the current commit are recorded and filtered from a later run, so a repeat `hooks run` on that item does not re-offer them | done | [install-hooks.md](install-hooks.md) (HOOK-100..105, HOOK-110), STO-75, CLI-194, CLI-195, CLI-196 |
| `meld --add-root`: compose extra convention roots with a manifest or authoritative source (install items a `marketplace.json` does not list); `dump` round-trips the recorded add-roots | done | DSC-84, DSC-85, DSC-86, DSC-87, MKT-17, STO-55, CLI-197, DUMP-11 |
| Item links: `learn`/`meld` a deep `tree`/`blob` skill URL as a single-item source instance (`host/owner/repo#path` identity, duplicates coexist); a malformed link tail reports the expected URL shapes | done | [item-link.md](item-link.md) (LNK-1..12, LNK-14, LNK-15) |
| Item links in `dump`: emit a link instance as a reconstructed deep-URL source entry | done | LNK-13 |
| Repo-spec identity parts (`host`, `owner`, `repo`) must each be a single safe path component; refused at parse time before any clone or delete | done | CLI-204 |
| TUI stdout capture writes to an exclusively created 0600 file in a 0700 temp dir, not a predictable create-and-truncate path | done | TUI-61 |
| `evolve` token safety: refuse a token carrying characters that would inject curl config directives | removed (STO-62: no curl invocation carries the token since 0.26.0; an unencodable token now fails the request) | STO-62 |
| A zero-item `meld` names the convention paths it scanned and the `--root`/`--add-root`/`--flat-skills` escapes, instead of reporting success with `(0 item(s))` | done | CLI-205 |
| A re-meld notes which discovery flags it ignored, instead of dropping them silently | done | CLI-206 |
| `meld --pin` on an already-melded source re-pins it: resolves against the current pin, re-checks-out the clone if the commit differs, records the new pin and commit | done | CLI-209 |
| `upgrade` reports an item whose recorded source is not registered distinctly from one blocked by the allowlist | done | POL-69 |
| Per-part `@`/`#` legality in `host`/`owner`/`repo`, closing an identity and clone-path collision between a repo named `foo@bar` and repo `foo` aliased `@bar` | done | STO-64, CLI-204 |
| An item link's path may not carry `@` or `#`, closing the same collision one segment over | done | LNK-16 |
| Metadata reads (`mind.toml`, item frontmatter, plugin and marketplace manifests) are size-capped at 8 MiB | done | DSC-91 |
| `evolve` names the resolved release target triple before downloading, so a gnu to musl artifact change is visible up front | done | STO-65 |
| `evolve` verifies the downloaded archive's build-provenance attestation with `gh` when present: a genuine verification failure aborts the swap, a tooling error or absent `gh` proceeds | done | STO-66 |
| Bare `mind` prints help on stdout at exit 0 | done | CLI-207 |
| `learn <source-name>` hints at `learn --all <source>` when the query names a melded source rather than an item | done | CLI-208 |
| The fork note names the pre-existing instances by their registered identities, which are the handles `unmeld` accepts | done | STO-63 |
| A curator's `add-roots` on a `[discover].sources` entry is gated like `roots`, so it cannot override a nested source's authoritative export control | done | DSC-88 |
| Add-root de-duplication keys on (kind, path), so one directory can still contribute both a tool and a skill | done | DSC-89 |
| `hooks run`/`hooks list` error on a target that matches both a registered source and an installed item, with escapes for each reading | done | HOOK-105 |
| Accepted risk: item content read from a source tree, beyond the capped metadata, is not size-capped | done | DSC-90 |
| Managed-policy allowlist matching derives the base identity structurally from the source's fields, never by scanning an identity string for a `#`/`@` marker | done | POL-68 |
| Managed-policy `allow`/`lock` matching is always performed on the base `host/owner/repo` identity, never on an extended instance identity carrying an item-link `#path` and/or consumer `@alias` suffix | done | POL-67 |
| `ssh://user@host/owner/repo` parses: the identity host strips a userinfo prefix (split on the last `@`), the full authority is preserved in the clone url for the ssh scheme; userinfo is refused for `http`/`https` so a credential is never persisted into `sources.json` or emitted by `dump` | done | STO-67 |
| `Registry::load` revalidates identity parts, the identity alias, and pin values, dropping an offending entry with a warning rather than failing | done | STO-68 |
| Clone-path confinement: a destructive or clone operation refuses a resolved clone path that escapes the sources dir | done | STO-69 |
| Per-instance clone dir for item links: the clone leaf mirrors the identity, so link instances at different refs hold independent checkouts | done | STO-70 |
| `install.sh` Linux artifact resolution: musl preferred, one fallback to gnu, because the script is served from `main` but resolves against the latest release | done | STO-71 |
| `introspect` reports a per-source scan failure as an issue and completes the run | done | CLI-210 |
| `upgrade` reports a source it could not scan instead of reporting everything up to date | done | CLI-211 |
| A local-path spec is made absolute before the source is constructed; identity derives from the absolute path | done | STO-72 |
| `Registry::load` migrates a relative local url to absolute when it resolves to an existing directory, rewriting `url` only, never `name` | done | STO-73 |
| `install.sh`'s `fetch_to` requires curl or wget on `PATH`, matching `fetch`'s check, instead of surfacing a misleading generic download failure | done | STO-74 |
| A nested `[discover].sources` relative local path resolves against the declaring `mind.toml`'s directory; a curator's own relative entry that resolves to a never-cloned sibling inside mind's managed sources tree is skipped with a warning rather than hard-failing the meld, since `dump`'s absolute entry for the same nested source still installs it | done | DSC-92, DSC-93 |
| The whole-registry catalog scan skips an unscannable source with a warning and completes on the partial catalog; a targeted single-source scan and `dump` still error | done | CLI-212 |
| A linked source whose working tree is gone is reported with an unmeld-or-restore message when named directly, and `meld` on an already-melded instance names it too, instead of reporting a healthy source with 0 items | done | CLI-213 |
| `review` treats a target naming an existing directory as a local path unless it first matches a melded source's identity, and notes the reading when it is also a valid owner/repo | done | CLI-214 |
| `InvalidRepoSpec` names the local-path forms; an owner/repo spec shadowed by an existing directory gets a note before any clone, printed at most once per ambiguous spec even though multiple call sites parse it | done | CLI-215, CLI-216 |
| Under `--json`, stdout is reserved for the single result document the invoked verb answers with: an advisory note, a progress line, and a nested verb's own output all route to stderr instead of leaking onto stdout ahead of or alongside the result | done | CLI-217 |
| `--json` is universal: every verb answers it with one JSON document, except a closed exclusion list (`dump`, `completions`, `man`, `evolve`, `init-source`); `review` answers with its findings document and folds a hard finding into the error envelope's `details` member, `hooks list` answers with its hooks document, and `hooks run` answers a successful run with an existed/ran/skipped tally | done | CLI-218, CLI-219, CLI-220, CLI-221, CLI-222 |
| A hook skipped for want of a terminal states the cause and the exact `hooks run` remedy (naming the `--event` it selected), for both a source target and an item target | done | HOOK-106, HOOK-107, HOOK-108 |
| Every printed remedy that splices a source-or-user-influenced identity into a runnable copy-paste command shell-quotes it (`mind unmeld`/`forget`/`meld`/`hooks run`), generalizing the `hooks run` rule so a crafted name cannot turn a pasted remedy into an injection | done | CLI-225 |
| `hook::is_tty` honors a `$MIND_TTY` override ahead of inspecting stdin, so the interactive-consent branches are reachable from a headless test | done | HOOK-109 |
| Registering a lobe creates its directory, so a preset for an uninstalled harness is reachable immediately | done | HARN-15 |
| An install fan-out that skips an unreachable lobe says so once per run, naming both remedies | done | HARN-16 |
| Registering a fan-out-mode lobe links every admitted installed item into it immediately; an occupied foreign target is reported with the `--force` remedy, not clobbered | done | HARN-17 |
| `introspect --fix` re-filters findings after repair; a lobe pruned in the same run is not also reported outstanding | done | HARN-18 |
| The TUI's lobes action shares `config lobes add`'s implementation (lobe-dir creation, kinds-filtered backfill, `--force`-gated foreign-file guard), so it carries the same guarantees with no separate implementation | done | TUI-62 |
| Selective lobe mode: `--local` on `learn`/`meld` scopes an install to the single registered project lobe the cwd sits inside, instead of the default fan-out | done | HARN-19, HARN-20, HARN-21 |
| A dangling-symlink path component is reported as a named broken link, not the OS's bare `File exists` | done | HARN-22 |
| `--json` treated as always non-interactive for a destructive confirmation (unmeld, forget --unmanaged, upgrade apply, evolve binary-swap, learn's dependency-closure confirmation) | done | LIFE-45 |
| `upgrade` refuses a rename that would evict a different installed item already occupying the new key, instead of silently deleting it | done | LIFE-46 |
| `upgrade` rename excludes the new install's own links from the old item's link cleanup, so a shared link path (e.g. an agent's bare-name link) is not deleted out from under the new install | done | LIFE-47 |
| A mid-batch `upgrade`/hook-rerun failure saves the manifest/registry for every already-applied item before the error propagates | done | LIFE-48 |
| `sync --upgrade --yes` forwards `--yes` into the upgrade pass instead of forcing it off | done | LIFE-49 |
| mind refuses an item install up front on a non-unix platform instead of falling back to an unrecognized copy | done | LIFE-50 |
| `upgrade`'s rename-collision checks run after the pending report, so an aborting batch still shows what else was pending | done | LIFE-51 |
| Source-derived item descriptions (frontmatter and `[[items]]` overrides) are sanitized (ANSI/control/bidi stripped) at catalog capture, not per display site | done | DSC-94 |
| TUI: installed rows carry a `stale` flag with a drift marker and an explicit per-item upgrade-confirm list | done | TUI-63 |
| TUI: `?` opens a keymap help overlay; hint line names `/`, `h`/`l`, paging, and `?` | done | TUI-64 |
| TUI honors `NO_COLOR` (monochrome style) and falls back to an ASCII glyph set on a non-UTF-8 locale | done | TUI-65 |
| Text measurement for TUI wrapping/sizing uses display-width, not char count; truncated tree-row descriptions get a trailing `...` marker | done | TUI-67 |
| TUI: an empty Installed/Available group shows a call-to-action row instead of a bare blank list | done | TUI-68 |
| TUI: Esc on a settled search filter arms a clear on the first press and only clears on a second consecutive Esc | done | TUI-69 |
| TUI status/error line's row budget scales with terminal height instead of a flat 3-row ceiling | done | TUI-70 |
| `probe` also falls back to the non-interactive listing under `--ascii` or a non-UTF-8 locale | done | TUI-71 |
| `--dangerously-skip-hook-check` renamed from `--dangerously-skip-install-hook-check` for `unmeld`/`forget` (an uninstall-hook flag); old spelling kept as a hidden alias | done | CLI-227 |
| `mind hooks run --rerun` is a visible alias for `--force` | done | CLI-228 |
| `evolve`'s pin-a-version flag renamed `--to <VERSION>`; `--version` kept as a hidden alias | done | CLI-229 |
| `unmeld` accepts `remove`/`rm` as additional visible aliases | done | CLI-230 |
| `sync [source]` accepts an optional source filter, syncing every matching source | done | CLI-231 |
| `sync <filter> --upgrade`'s upgrade pass and its hook re-run are scoped to the filter's matched sources | done | CLI-232 |
| `sync <filter> --upgrade` names the filter's matched sources in the confirmation when it matched more than one | done | CLI-233 |
| Item-name capture rejects control/bidi/zero-width code points: a hard error for a `mind.toml` declaration, skip-with-warning in the convention scan | done | DSC-96 |
| `[[items]] link` target is confined to the item kind's directory; anything else is refused | done | DSC-97 |
| Blocked-character set extended to the Unicode format class (tag block, variation selectors, U+00AD, U+180E, U+2061-2064, U+206A-206F, U+FFF9-FFFB, U+3164, U+115F) | done | NS-73 |
| Broken-symlink diagnosis is substituted only for an `AlreadyExists` mkdir failure whose offending component is a `NotFound` symlink | done | HARN-23 |
| Resolved `evolve` target version is validated as a plausible release tag (which may carry a semver prerelease/build suffix) before any URL is built | done | STO-76 |
| TUI: staleness flag is memoized against a fingerprint of a stat-derived hash | done | TUI-72 |
| TUI: `u` applies pending upgrades without syncing first (the confirm modal says so); the applied set is exactly the confirmed key set by construction, and a confirmed key that became inapplicable before apply is skipped silently | done | TUI-73 |
| TUI: `u` recomputes staleness authoritatively before building the upgrade confirm list, bypassing the display-only hash memo | done | TUI-74 |
| The multi-source disclosure also fires when an `upgrade <filter>` (including the `'<suffix>#*'` form) resolves to installed items spanning more than one source | done | CLI-234 |
| The multi-source disclosure is emitted before the install-hook re-run pass, even with nothing pending, and is not suppressed by `--yes` | done | CLI-235 |
| TUI confirm/dialog strings render the sanitized display key; the identity key on a queued action stays raw | done | TUI-75 |
| TUI: what `u` offers and what applying it does when the recomputed stale set is empty | done | TUI-76 |
| `evolve --to` prerelease target ordering, and the leading-`v` strip | done | STO-77 |
| Dependency-tree rendering is bounded: each node's subtree renders once, a later occurrence is a `(seen)` leaf, with a depth/line cap and a truncation notice | done | DEP-64 |
| `meld <repo> --learn <NAME\|GLOB>` installs only the matching subset (repeatable, matches bare or effective name, dependency closure applies, conflicts with `--register-only`) | done | CLI-236 |
| An item link's unsatisfiable intra-source references: a `requires` entry is dropped with a warning, a sibling-naming token errors; both name the unmeld-then-`meld --learn` remedy | done | LNK-18 |
| A dropped `requires` is recorded on the installed item and surfaced by `recall`, `recall --json`, and `introspect` | done | LNK-19 |
| Ignored files: `[source].ignore` / `[[items]].ignore` plus a built-in VCS set exclude paths from both the store copy and the content hash, so an item can point at a directory holding more than itself | done | [ignore.md](ignore.md) (IGN-1..21) |
| `evolve` verifies TLS against the machine's certificate store, so an intercepting proxy's company CA is trusted; a certificate failure gets its own hint | done | STO-78, STO-79 |
| Update hooks: `event = "update"` runs at `upgrade` instead of re-running install hooks, for a source and for an item; install hooks stay the idempotent default | done | [install-hooks.md](install-hooks.md) (HOOK-120..126), CLI-195 |
| An item declares its own hooks: `install:`/`update:`/`uninstall:` frontmatter on any kind, and a scoped `mind.toml` (`[[hooks]]` only) in a skill or tool directory | done | [install-hooks.md](install-hooks.md) (HOOK-130..134) |
| The `command` item kind: `commands/<name>.md` discovered, stored, linked, namespaced, and upgraded like any other kind | done | [commands.md](commands.md) (CMD-1..9), DSC-14, STO-2 |

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
- [commands.md](commands.md) - the `command` item kind: harness slash commands
  (`commands/<name>.md`) discovered, installed, and namespaced like any item.
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
- [ignore.md](ignore.md) - ignored files: which paths under an item are excluded
  from the store copy and the content hash, so an item can point at a directory
  that holds more than the item itself (a top-level `SKILL.md` at a repo root).

## Conventions

- Each normative statement has a stable ID (e.g. `CLI-30`, `LIFE-14`). Tests cite
  these IDs (in `// spec: ID` comments) so a spec line maps to its verification.
  IDs are append-only: retire an ID by marking it removed, never reuse the number.
- A coverage gate (`tests/spec_coverage.rs`, run by `cargo test` and CI) fails
  when a defined ID is neither cited by a test nor in its ALLOWLIST. Adding a new
  requirement therefore forces a coverage decision: write a citing test, or
  allowlist it with a reason.
- "item" means a skill, agent, rule, command, or tool. "source" means a melded repo. "store"
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
