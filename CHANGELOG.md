# Changelog

All notable changes to `mind` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Update hooks: a `[[hooks]]` entry with `event = "update"` runs at `upgrade` in
  place of re-running the install hooks, for a source and for an item
  (HOOK-120..126). A source or item that declares none keeps the existing
  behavior, so an install hook still re-runs when its source or item advances,
  and is expected to be idempotent; `init-source` now says so in its scaffold and
  shows the update event as the escape for a step that cannot be. `hooks run` and
  `hooks list` accept `--event update`, with the same pending/recorded semantics
  as the install event. The hook consent disclosure now names the lifecycle
  event it is asking about (HOOK-20), so approving an update or an uninstall
  hook is no longer indistinguishable from approving an install hook.
- A Claude plugin's `commands/<name>.md` maps to the `command` kind (MKT-18), on
  both plugin paths: a directly melded `.claude-plugin/plugin.json` and each
  in-repo entry of a `.claude-plugin/marketplace.json`. They are namespaced by
  the plugin name like the plugin's other items, and are no longer counted in
  the "not installed (no mind equivalent)" note, which now covers `hooks/`,
  `.mcp.json`, LSP, monitors, themes, and output styles.
- The `command` item kind (CMD-1..9): a `commands/<name>.md` file in a source is
  discovered, stored at `store/command/<name>`, and linked into each admitting
  lobe at `commands/<name>.md`, so a harness slash command installs and upgrades
  like any other item. A namespace gives `commands/<prefix>:<name>.md`, which is
  the spelling a harness's own command namespacing produces. `--kind command`,
  `command:<name>` refs, `kind = "command"` in `[[items]]`, and
  `[discover].commands` globs all work; the convention scan is flat, so a nested
  `commands/<group>/<name>.md` needs an explicit entry or a glob. The harness
  presets (gemini, codex, universal, windsurf) admit skills only and are
  unaffected. A hand-written command in a lobe now shows up as an unmanaged item
  in `recall`, and can be `absorb`ed.
- An item declares its own hooks, next to the item (HOOK-130..134). The scalar
  `install:` / `update:` / `uninstall:` frontmatter keys are read from any kind's
  meta file (`SKILL.md`, `TOOL.md`, an agent's or rule's `.md`), not only a
  tool's `TOOL.md`, and a skill or tool directory may carry a scoped `mind.toml`
  declaring the full `[[hooks]]` array. A root `[[items]]` entry that declares
  hooks still wins; the three sites never merge. `mind review` and `hooks list`
  report an item's hooks from its resolved list, so array-declared and
  item-declared hooks are disclosed alongside the scalar ones.

## [0.26.1] - 2026-08-30

### Fixed

- `evolve` verifies TLS against the machine's certificate store again (STO-78).
  0.26.0 moved the download onto rustls, whose default root set is Mozilla's
  bundled list and ignores the machine entirely, so `evolve` failed with
  `invalid peer certificate: UnknownIssuer` on any network where an intercepting
  proxy re-signs HTTPS with a company CA. The company CA is installed on the
  machine, which is what curl read before 0.26.0 and what `evolve` reads now: the
  system keychain on macOS, the certificate stores on Windows, and the
  OpenSSL-convention bundle on Linux, where `SSL_CERT_FILE` / `SSL_CERT_DIR`
  also apply. A host with no usable trust store (a scratch container) now fails
  where the bundled roots would have worked; `SSL_CERT_FILE` is the escape.
- A certificate-verification failure carries its own hint naming the trust store
  and `SSL_CERT_FILE`, instead of the bare transport error or the proxy hint,
  whose fix is a different setting (STO-79).
- Documentation caught up with the 0.26.0 transport change, which several pages
  still described in terms of curl and wget. The corrected claims that matter:
  `evolve` does not read `CURL_CA_BUNDLE` or `~/.curlrc`, it no longer exposes a
  `GH_TOKEN`/`GITHUB_TOKEN` on any subprocess command line (so the advice to
  prefer curl over wget on shared hosts is obsolete, and the exposure it warned
  about is gone), and the binary-update trust model now names `gh attestation
  verify` as the only origin check.

## [0.26.0] - 2026-08-28

### Changed

- `evolve` downloads, verifies, extracts, and swaps the binary through the
  `self_update` crate instead of shelling out to curl or wget and `tar`. Neither
  downloader has to be on `PATH` any more; TLS is rustls, so a static musl build
  no longer depends on the system OpenSSL layout. The verification chain is
  unchanged in what it enforces: the `SHA256SUMS` digest for the release asset
  (STO-47) and `gh attestation verify` over the downloaded archive (STO-66),
  both before extraction, and the swap stays atomic behind the global exclusive
  lock (STO-45, STO-46). GitHub's per-asset digest is now verified as well.
  Behavior differences worth knowing: an unwritable install path is reported
  before the download rather than after it, the API token is read as `GH_TOKEN`
  then `GITHUB_TOKEN` (the `gh` CLI's order, previously the reverse), and a
  token that cannot be encoded as an HTTP header fails the request instead of
  degrading to an unauthenticated one. `MIND_HTTP_TIMEOUT_SECS` still sets the
  request timeout. `resources/install.sh` is unchanged and still uses curl or
  wget, since it runs before `mind` exists.

## [0.25.0] - 2026-08-28

### Added

- `[source].ignore` and `[[items]].ignore` in `mind.toml`: glob patterns,
  relative to each item's own path, excluded from both the store copy and the
  content hash. An item's own list replaces the source's for that item. Version
  control directories (`.git`, `.hg`, `.svn`, `.bzr`) are excluded with or
  without a declaration; build output is not implied and must be listed. This
  makes an item whose path holds more than the item workable, such as a
  top-level `SKILL.md` declared with `path = "."`, which previously copied the
  repo's `.git/` into the store and reported the item as drifted after every
  commit in the source clone. An item installed before this that has a VCS
  directory in its tree reports as out of date once, and `upgrade` reinstalls it
  without that directory (IGN-1..21). Three new `--json` error `kind` codes
  join the CLI-181 envelope: `bad-ignore-pattern`, `ignores-own-anchor`, and
  `expands-ignored-file`.
- `meld <repo> --learn <NAME|GLOB>` installs only the items matching the pattern
  instead of offering the source's whole set. The flag is repeatable; a pattern
  matches an item's bare name as well as its effective (prefixed) name, and is
  scoped to the source being melded: a super-source's nested `install = true`
  sources still register, but none of their items install under `--learn`.
  Each match installs through the ordinary `learn` path, so its dependency
  closure comes with it. A pattern matching nothing in the source is an error
  naming that source, an unusable pattern is rejected before the clone, and the
  flag conflicts with `--register-only`. A re-meld honors it too (CLI-236).
- `recall <item>`, `recall <item> --json`, and `introspect` report a `requires:`
  entry that was dropped at install (LNK-19).

### Changed

- SOURCE BUILDS ONLY: the minimum supported Rust version is now 1.98 (it was
  1.88), so `cargo install --locked mind-cli` needs a toolchain at least that
  new. The install script and the Homebrew tap ship prebuilt binaries and are
  unaffected. `mind` ships as a binary, so this floor tracks recent stable and
  may rise again in any release; it is a build requirement, not a compatibility
  promise.
- Internal: the `probe` TUI's staleness memo is now backed by the `cached`
  crate, a new dependency. No change to what the TUI reports.
- Dependencies refreshed to their latest semver-compatible releases.
- Lifecycle hooks (install, uninstall, build) stream their output: stdout and
  stderr are inherited, so a build or package install is visible as it runs
  instead of appearing all at once when the command exits. The run is framed by
  `====== (hook: <name>) ======` and `====== (end hook: <name>) ======`. Two
  visible differences: the streams interleave rather than appearing under
  separate `hook-stdout`/`hook-stderr` blocks, and a hook's stderr now goes to
  mind's stderr instead of being replayed onto stdout, so redirecting one stream
  no longer moves the other's output with it. Under `--json` a hook's output
  stays off the result document, as before. A failing hook's error points at the
  frame without claiming output exists ("its output, if any, is in the frame
  above"): with the streams inherited mind never sees them, so it cannot tell a
  silent hook from a talkative one (HOOK-30, HOOK-32). In the `probe` TUI, a
  hook's stderr is captured alongside its stdout, so hook output cannot smear
  the browser's alternate screen.
- An item link's unsatisfiable intra-source references are reconciled instead of
  failing with a generic missing-reference error. A link instance offers exactly
  one skill, so a reference to a sibling can never resolve: a `requires:` entry
  is now dropped (with a warning and a durable record) and the skill installs,
  rather than being a hard `bad-reference`. A token stays a hard error, now
  `link-ref-unsatisfiable`, and this covers `{{tools:}}` and `{{path:}}` as well
  as `{{ns:}}`, including tokens in `expand:`-listed files. Both paths print a
  remedy that works as pasted: `mind unmeld <identity> --yes && mind meld <url>
  --learn 'skill:<skill>' --yes`, or, when a plain meld of the repo would not
  discover the skill on its own, the same command with `--add-root <dir>` added
  before `--learn`, where the directory is derived from the linked skill's path
  (LNK-18).

### Fixed

- Under `--json`, the saved real stdout descriptor is now close-on-exec, so a
  hook cannot write to it (fd 3) and forge the result document (HOOK-32).

### Security

- `lru` updated to 0.18.3, clearing RUSTSEC-2026-0253 (a use-after-free from
  missing panic safety in `LruCache::pop`). It reaches `mind` through
  `ratatui-core`, so it is compiled into every shipped binary; the released
  artifacts are rebuilt with it.

## [0.24.0] - 2026-08-17

### Changed

- Breaking for JSON consumers: `learn --json` of an item that pulls in
  dependencies beyond the explicit selection now refuses with
  `confirmation-required` unless `--yes` is also given, instead of installing
  the closure unprompted. A script driving `mind learn --json <item>` against an
  item with dependencies must add `--yes` (DEP-31).
- `meld --add-root` items on a single-plugin source now install under the
  plugin-name default prefix alongside the plugin's own items, instead of
  unprefixed; a marketplace source's per-entry namespaces still do not reach
  add-root items (DSC-86).
- Source-controlled item names render through sanitizing display accessors at
  every CLI and TUI print site, and an item key is a dedicated type without
  `Display`, so printing a raw key no longer compiles; identity and matching
  stay raw (DSC-95, TUI-75).
- `is_safe_item_name` rejects control, bidi, zero-width, and other invisible
  code points, and an `[[items]] link` target is confined to its kind
  directory, so a source cannot plant a file at the agent home root (DSC-96,
  DSC-97). The blocked-character set covers the Unicode format class, the tag
  block, and variation selectors (NS-73).
- `sync <source> --upgrade` scopes the upgrade pass and its install-hook
  re-runs to the matched sources; a scope spanning more than one source is
  disclosed before its hooks run, `--yes` included (CLI-232..235).
- The `probe` TUI applies upgrades without syncing first, applies exactly the
  key set the confirm modal listed, and memoizes the ~1s staleness poll
  against a stat fingerprint instead of re-hashing every item tree each tick
  (TUI-63, TUI-72..76).
- `evolve` offers a source-built dev binary its own base release, orders
  same-base prereleases by semver precedence, and validates the resolved
  target tag before building download URLs (CLI-140, STO-76, STO-77).
- `mind man` emits one roff page per visible subcommand after the top-level
  page, so the SUBCOMMANDS cross-references (`mind-meld(1)`, ...) resolve
  within the output (CLI-121).
- The recursive item-tree walks (staging copy, file registry, content hash)
  are depth-capped at 128 nested directories; a deeper tree errors instead of
  overflowing the stack (LIFE-52).

### Fixed

- `NO_COLOR=1 mind probe` keeps the TUI's Unicode markers and drops only
  color, as TUI-65 specifies; previously it drew the ASCII fallback glyphs.
- A `forget` whose uninstall hook fails and whose failure-path manifest save
  also fails now propagates the hook error and reports the save failure as a
  warning, instead of masking the root cause (LIFE-48).
- `install` records the source content hash before the store swap drops its
  backup, closing a window where a source read error left the new copy
  installed but unrecorded.
- A failed `absorb` unregisters a destination source it melded itself and
  names the residual destination commit in a warning (ABS-12).
- `recall --tree` no longer renders duplicate nodes when two melded sources
  offer the same unprefixed name and one copy is installed; graph membership
  matches the manifest entry's source as well as its key (DEP-61).
- An item-link URL with `.` path segments (`tree/main/./skills/foo`) parses to
  the same source instance as the plain form instead of registering a
  duplicate (LNK-17).
- Two items in one `upgrade` batch renaming onto the same key are refused
  before any item is applied, instead of the second install silently evicting
  the first (LIFE-46).
- The TUI poll rebuilds the tree only when the snapshot actually changed;
  previously a fresh generation counter forced a rebuild every tick (TUI-15).
- The dependency tree renders a diamond DAG once per subtree with a `(seen)`
  marker and a depth cap, instead of expanding exponentially before the
  install-consent prompt (DEP-64).
- A dangling symlink in an agent-home path is diagnosed with the link and its
  missing target instead of a bare `File exists` (HARN-22, HARN-23).
- `review` findings and error messages sanitize each source-derived field
  before composing the line, so a dangling escape sequence in one field cannot
  swallow the disclosure that follows it.

## [0.23.0] - 2026-08-05

### Added

- Selective lobe mode: `learn --local` and `meld --local` install an item into
  only the current project's lobe instead of the global fan-out set, when run
  inside a registered project lobe. From a directory not inside any registered
  lobe, `--local` is an error naming the fix rather than a silent fall-back
  (HARN-19, HARN-20, HARN-21).
- `sync [source]` fetches a single source instead of every melded source
  (CLI-231).
- `evolve --to <VERSION>` replaces `evolve --version`, which shadowed the global
  `--version`; the old flag stays as a hidden alias. `unmeld` gains `remove`/`rm`
  aliases, and `hooks run` gains a `--rerun` alias for `--force` (CLI-229,
  CLI-230, CLI-228).
- The `probe` TUI marks out-of-date items, lists the pending items in the
  upgrade confirmation, offers a `?` help overlay, honors `NO_COLOR`, and falls
  back to ASCII glyphs on a non-UTF-8 locale. `probe` under `--ascii` or a
  non-UTF-8 locale now shows the plain listing instead of launching the Unicode
  TUI (TUI-63, TUI-64, TUI-65, TUI-71).
- `meld`, `learn`, `forget`, and `unmeld` carry an EXAMPLES section in `--help`.
- The docs document the item downgrade recipe (`meld --pin <old-sha>` then
  `upgrade`).

### Changed

- `--dangerously-skip-install-hook-check` is renamed to
  `--dangerously-skip-hook-check` on `unmeld`/`forget`, where it gates uninstall
  hooks; the old spelling is kept as a hidden alias (CLI-227).
- Source-derived descriptions are sanitized at the point they are read from a
  catalog or `mind.toml`, so a crafted `description` can no longer inject ANSI or
  bidi control sequences into `probe`, `recall`, or `--json` output (DSC-94).
- The `min-mind-version` comparator preserves a nonzero pre-release patch: a
  `0.23.1-dev` binary is no longer read as `0.23.0` by the source gate, the
  policy gate, or `evolve`.
- `config lobes detect` output uses "agent home (lobe)" consistently.

### Fixed

- `--json` without `--yes` no longer bypasses a destructive confirmation on a
  TTY: `forget`, `forget --unmanaged`, `unmeld`, `upgrade`, and `evolve` now
  refuse with `confirmation-required` instead of acting unprompted. In
  particular `forget --unmanaged <ref> --json` no longer deletes the target
  file without consent (LIFE-45).
- `upgrade` renaming an item (for example after an upstream prefix change) no
  longer evicts a different source's item registered under the new name, and no
  longer removes the freshly created link when the old and new link paths
  coincide (LIFE-46, LIFE-47).
- `upgrade` and re-meld save the manifest and hook registry when a batch fails
  partway, so a retry does not re-run hook side effects or leave the manifest
  disagreeing with what is on disk (LIFE-48).
- `sync --upgrade` honors `--yes` (LIFE-49).
- `--local` lobe detection resolves a symlinked ancestor when the project lobe
  directory does not exist yet, so it no longer refuses on a temp directory
  reached through a symlink.
- The `IncompatibleVersion`, `StateTooNew`, and managed-policy remedies point at
  `mind evolve`, the binary self-update verb, not `mind upgrade`.
- `forget`'s per-source hints print in a deterministic order.
- `atomic_write` fsyncs before the rename, for crash-safety of `manifest.json`
  and `sources.json`.

### Security

- Release binaries embed a dependency SBOM (`cargo-auditable`), so
  `cargo audit bin mind` can scan a downloaded release against the RustSec
  advisory database.
- `anyhow` (a transitive dependency) is bumped to 1.0.104, clearing
  RUSTSEC-2026-0190 (unsoundness in `Error::downcast_mut`).

## [0.22.0] - 2026-07-30

### Fixed

- `mind review --fix` was deleting valid `{{ns:}}` tokens out of prose,
  producing bare names that no longer resolve under a prefixed meld. The
  hand-rolled scanners that decided whether a token sat in prose or code are
  replaced by a `pulldown-cmark` parse, shared by `--fix` and `init-source
  --template`'s wrapper, so code spans, fenced and indented blocks, list and
  blockquote containers, and backslash escapes are read as CommonMark defines
  them (NS-46, NS-47, NS-49, NS-50). Ten misclassifications are closed,
  including several the hand-rolled scanners never saw: a fence opened on a
  list-marker line, a code span crossing a thematic break or setext underline,
  a fence inside a blockquote, and a leading UTF-8 BOM displacing the first
  line's structure.
- `review --fix` no longer nests a `{{ns:}}` token inside one that spans a line
  break, which produced `{{ns:\n{{ns:name}} }}` and made the source fail to
  install (NS-51).
- `review --fix` no longer rewrites link syntax. A sibling name used as a
  reference label or a relative destination was wrapped, so the reference
  stopped resolving and rendered literally; destinations, titles, reference
  labels, and link reference definitions are now syntax, while a name in a
  link's visible text is still a prose reference and is still wrapped (NS-52).
- `meld` on an already-melded source whose linked working tree has since
  vanished no longer reports it as a healthy source with `0 item(s)` at exit
  0; it now names the gone working tree the same way `recall`/`probe`
  already did (CLI-212, CLI-213).
- `review`'s CLI-215 ambiguity note is now printed at most once per target.
  Taking the CLI-214 local-directory reading no longer also prints the
  CLI-215 note, which would tell the user to write `./<target>` to get the
  very reading `review` had just taken (CLI-216).
- A relative `[discover].sources` entry that only resolves inside a curator's
  own working tree, and inside a clone resolves to a sibling mind never
  created, no longer hard-fails the whole meld under the DSC-80 curator
  guard; it is skipped with a warning, and the absolute entry `dump` always
  emits for the same nested source still installs it (DSC-93).
- `resources/install.sh`'s `fetch_to` (the release-asset and `SHA256SUMS`
  downloader) now checks for `curl`/`wget` on `PATH` itself, instead of
  surfacing a misleading `download failed: <url>` when neither is present and
  `MIND_VERSION` skipped the earlier check that would have caught it (STO-74).
- `--force`'s `--help` text on `config lobes add`/`link-project` now names
  both effects it has: overwriting a snapshot target, and overriding the
  backfill guard on a foreign file at a lobe-add target. It previously named
  only the snapshot one.
- A local-path source is now recorded as an absolute path, so melding by a
  relative path and then working from another directory no longer breaks the
  source. An existing relative path is migrated to absolute on load when it
  still resolves to an existing directory (STO-72, STO-73).
- A source whose linked working tree is gone is now reported and skipped by
  the commands that list the catalog (`recall`, `probe`, `introspect`,
  `upgrade`) instead of failing the whole command. Naming such a source
  directly still errors (CLI-212, CLI-213).
- A relative local path in a curated mind.toml's `[discover].sources` now
  resolves against the directory that declares it, not the consumer's working
  directory (DSC-92).
- `review` now accepts any target naming an existing directory that does not
  first match a melded source, so a bare or two-segment relative path is
  reviewed locally instead of being treated as owner/repo and cloned
  (CLI-214, CLI-215).
- Registering a lobe now creates its directory and links the already-installed
  items into it, so `link-project` and `config lobes add --preset` no longer
  register a lobe that silently never receives anything (HARN-15, HARN-17).
- `introspect --fix` no longer reports a lobe it pruned in the same run as an
  outstanding issue (HARN-18).
- `hooks run` that could not run anything because it had no terminal now names
  the cause and an exact, copy-pasteable command (naming the `--event` it
  selected) to run it unattended, and exits non-zero instead of reporting
  success. This now covers an item target (`<source>#<item>`) the same as a
  source target (HOOK-106, HOOK-107, HOOK-108).
- Policy allowlist matching now compares the base `host/owner/repo` identity at
  every gate, so an instance admitted at meld time is no longer skipped by
  `sync`, `upgrade`, and install-hook gating under a locked allowlist.
  Previously no writable `allow` pattern could admit an item-link (`#<path>`) or
  alias (`@<prefix>`) instance after meld (POL-67).
- A source whose `owner` legitimately contains `@`, such as a local path under
  `proj@v2/`, is no longer refused by a locked allowlist entry that matches it
  verbatim (POL-68).
- `upgrade` now reports an installed item whose recorded source is no longer
  registered as exactly that, instead of claiming the managed policy's allowlist
  refused it. It is still not upgraded (POL-69).
- `mind evolve` no longer fails outright when the curl config file holding the
  GitHub token cannot be written; it proceeds unauthenticated (STO-62).
- `mind hooks run` / `hooks list` now resolve a target that exactly matches a
  registered source identity as that source, even when the identity contains
  `#` (an item-link instance's own identity), instead of parsing it as an item
  ref that matches nothing. An item-link instance's source-level hooks are now
  addressable by its own identity, not only via an over-broad glob (HOOK-105).
- `mind dump` now emits an `add-roots` key for a source melded with
  `--add-root`, and the emitted super-source threads it through nested melds,
  so re-melding a dump no longer silently drops items an added root
  contributed (DUMP-11).
- A deep `tree`/`blob` URL whose tail is not a valid item link (a `blob` URL
  not ending in `/SKILL.md`, a `tree` URL with no skill path) now reports a
  specific error naming the URL and the two expected link shapes, instead of
  the generic "not a valid repo spec" (LNK-14).
- Two `--add-root` roots that surface the same on-disk item through different
  scan passes now de-duplicate instead of erroring `DuplicateItem`; the error
  remains for a genuine same-name collision at distinct paths (DSC-87).
- `ssh://user@host/owner/repo` parses again. The identity host now strips a
  userinfo prefix (split on the last `@`) before validation; it was refused by
  the identity-part validation added earlier this cycle (STO-67).
- `install.sh` now falls back to the `gnu` Linux artifact when the `musl` asset
  is unavailable, so it still works against a release that predates the musl
  build legs (STO-71).
- `introspect` now reports a source it cannot scan as an issue and completes
  the run, instead of aborting; `upgrade` names such a source instead of
  reporting everything up to date (CLI-210, CLI-211).
- `dump` now reconstructs an item-link URL from the recorded
  `host`/`owner`/`repo`, instead of from the source's `url` (which an
  SSH-preferring config rewrites), so a dump taken with `ssh = true` re-melds
  instead of aborting the super-source meld (LNK-13).
- Each item-link instance now gets its own clone directory, so two links from
  one repo at different refs no longer clobber each other's checkout, and
  `unmeld` of one no longer breaks the other (STO-70).
- `mind hooks run <source>#<item> --event install` no longer errors on every
  repeat run once the hook already ran. An installed item now records which
  install hooks ran at which commit, the same way a source does, so a run
  against an item already up to date settles to exit 0 instead of returning
  `HooksNotRun` forever (HOOK-110).
- `HooksNotRun`'s remedy no longer echoes a multi-match selector as an
  unquoted shell glob (e.g. `hooks run --event install '*'` printing a
  command whose bare `*` expands against the caller's cwd if pasted). When
  several sources or items matched, the message now lists their resolved
  identities instead of synthesizing one fake command (HOOK-106).

### Added

- An item may opt specific non-markdown files into token expansion with an
  `expand:` frontmatter key: a whitespace-separated list of item-relative paths
  (the same scalar form `requires:` uses). Each listed file is expanded at
  install exactly as a markdown file is (`{{ns:}}`, `{{path:}}`, `{{tools:}}`,
  `{{self}}`), so a bundled script can reference a sibling tool or adjacent file
  without a language-specific self-locate. A path token in an expand-listed file
  renders as an absolute store path, not the `~` form markdown uses, since the
  file is program input. The key lives on the item, not in `mind.toml`'s
  inventory, so declaring it keeps convention discovery on and does not force a
  source to enumerate its other items. A bad entry (an absolute path, a `..`
  segment, or a file the item does not ship) fails the install as a
  `BadReference` during staging, and `review` reports the same as a hard
  `bad-expand` finding; an expand-listed file is no longer flagged `inert-token`,
  and an unresolved token in it is a hard `bad-reference` rather than dead-text
  advisory (NS-57, TOOL-20, CLI-226).
- `hook::is_tty` now honors a `$MIND_TTY` override (falsy: empty, `0`,
  `false`, `no`, `off`; anything else is truthy), read before inspecting
  stdin, so the interactive-consent branches are reachable from a headless
  test (HOOK-109).
- `SECURITY.md` (reporting channel, trust model), `CONTRIBUTING.md`,
  `CODE_OF_CONDUCT.md`, issue and pull-request templates, and a `dependabot.yml`
  covering both `cargo` and `github-actions`.
- Linux release artifacts for `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl`. `install.sh` and `evolve` now prefer the musl
  build, which starts on distributions older than the build runner's glibc; the
  gnu artifacts remain for the Homebrew formula.
- Build provenance attestation on release artifacts, soft-verified by
  `install.sh` when `gh` is available.
- CI jobs for the declared MSRV (1.88) and for RustSec advisories.
- `--json` now answers every verb with one JSON document on stdout, except a
  closed exclusion list (`dump`, `completions`, `man`, `evolve`,
  `init-source`). `review` answers with a findings document and folds a hard
  finding into the error envelope's new optional `details` member; `hooks
  list` answers with a hooks document; `hooks run` answers a successful run
  with an existed/ran/skipped tally (CLI-218, CLI-219, CLI-220, CLI-221,
  CLI-222).
- `review` reports, as an advisory `inert-token` finding, any `{{...}}` token
  found in a non-markdown item file, whether or not it names a real referent.
  Previously it only flagged an unresolved token there; a token that WOULD
  resolve (e.g. `{{tools:name}}` naming a real sibling tool, in a bundled
  `.sh`) went unreported, even though it never expands outside markdown
  either and is left literal at install (CLI-223).

### Changed

- `{{ns:}}`, `{{path:}}`, `{{tools:}}`, and `{{self}}` tokens now expand only
  in a markdown file (`md`, `markdown`, `mdown`, `mkd`, case-insensitively),
  not every text file of an item. A token left in a non-markdown file (a
  script, data) is no longer expanded and is left exactly as written; an
  unresolvable `{{ns:}}` there no longer fails the install, since nothing
  there was ever expanded (NS-53, TOOL-19).
- `review --fix` no longer rewrites a non-markdown item file. It still
  reports a finding there (a hardcoded path, a misplaced token, an unguarded
  reference); it just leaves the file unrewritten (NS-54).
- `dump` now emits an item-link instance as a deep `tree` URL rebuilt from its
  recorded parts and pinned by `pin-ref`, instead of skipping it with a note. A
  skill installed from a pasted URL is reproduced by melding the dump, including
  an aliased instance and a `file://` link (LNK-13).
- `evolve` names the resolved target triple in its report and `--json` output
  before downloading, so the Linux gnu to musl artifact change is visible up
  front. The existing keys are unchanged; `target_triple` is added (STO-65).
- `meld --pin` on an already-melded source now re-pins it, rather than printing
  that the flag was ignored. It resolves the pin against the source's current
  one, re-checks-out the clone when the commit differs, and records the new pin
  and commit; a failure to resolve or clone leaves the source untouched. There
  was previously no command that re-pinned a source (CLI-209).
- `meld` refuses a repo spec whose `repo` part contains `@` or `#`, or whose
  `owner` contains `#`. A repo literally named `foo@bar` and the repo `foo`
  melded as `@bar` computed the same identity and the same clone directory, so
  the second was treated as a re-meld of the first and the two shared one clone.
  `@` stays legal in `owner`, where it collides with nothing (STO-64). The same
  rule applies to an item link's path (LNK-16).
- A curator's suppressed `add-roots` is now named in the DSC-60 warning, with a
  pointer to the un-gated consumer-side `meld --add-root` (DSC-88).
- A top-level `meld` that discovers zero items now says what it scanned and how
  to reach items elsewhere in the repo, naming the convention paths and
  `--root`/`--add-root`/`--flat-skills`, instead of reporting success with
  `(0 item(s))` (CLI-205).
- A re-meld now notes which of `--root`, `--add-root`, `--flat-skills`, `--pin`,
  and `--install-hook` it ignored and what to do instead. Those flags apply only
  at the meld that first registers a source, and were previously dropped
  silently (CLI-206).
- Bare `mind` prints help on stdout and exits 0, instead of a clap usage error
  on stderr with exit 2 (CLI-207).
- `learn <name>` hints at `mind learn --all <name>` when the query names a
  melded source rather than an item (CLI-208).
- The fork note names the pre-existing instances by their registered
  identities. It previously named the bare `host/owner/repo`, which is not
  necessarily a registered source and so was not a handle `unmeld` accepts
  (STO-63).
- `hooks run`/`hooks list` error on a target that matches both a registered
  source identity and an installed item, naming both disambiguated forms, rather
  than silently resolving to the source. `source:<target>` forces the source
  reading and `<source>#<kind>:<name>` forces the item reading (HOOK-105).
- A git auth failure now leads with the possibility that the repo does not
  exist, is private, or is misspelled, before the SSH and credential-helper
  remedies. GitHub answers an unauthenticated request for a missing repo with a
  credential prompt, so a typo'd repo was reported purely as an auth problem.
- `SourceNotFound` and `UnknownLobe` name the command that lists the valid
  values, matching `ItemNotFound`.
- `install.sh` reports an explicit error when the install directory is not
  writable, instead of exiting silently with no binary, and matches the
  checksum line exactly rather than by an unanchored pattern.
- The minimum supported Rust version is now 1.88 (was 1.85), to use let-chains.
  Building from source needs a 1.88+ toolchain; installing a released binary,
  the `install.sh` script, or Homebrew is unaffected.
- `spec/` is included in the published crate, so `cargo test` works against an
  unpacked `.crate`; `.claude` is excluded.
- `meld` prints an explicit note when a differing `--namespace` forks a new
  coexisting instance of an already-melded repo (STO-60). `learn <url> --pin`
  on an already-melded link prints a note that the pin was ignored instead of
  silently dropping it (CLI-203).

### Security

- `--json` output is now produced from a single reserved channel: `main`
  redirects the process's real stdout to stderr for the whole run and prints
  only the recorded result document (or the CLI-181 error envelope) at the
  very end, instead of relying on discipline at each call site. This closes
  several concrete leaks (an unconverted progress line printed ahead of the
  envelope on the install-hook, uninstall-hook, item-build-hook, and
  item-install-hook paths) and a distinct class (a nested verb call -- `sync`'s
  `auto_meld` install pass, `learn <url>`, `absorb` -- printing its own extra
  result object ahead of the caller's, so stdout under `--json` could carry
  more than one document). It also forecloses the general case: a source's
  install hook is arbitrary output chosen by its author, and previously any
  unrouted print anywhere in the call graph could reach `--json` stdout,
  including a well-formed forgery of the result envelope (CLI-217).
- `evolve` verifies the downloaded archive's build-provenance attestation with
  `gh attestation verify` when `gh` is on PATH, after the checksum check and
  before extraction. An absent `gh` or a tooling failure (no `attestation`
  subcommand, no network, auth required) proceeds; a genuine verification
  failure aborts and leaves the running binary in place. This is deliberately
  stricter than `install.sh`, which never blocks: `gh` reports a substituted
  artifact with the same "no attestations found" wording it uses for an artifact
  that was never attested, so treating that as benign would defeat the check for
  the case it exists to catch. No released version this can download predates
  attestation, since the download runs only when the target is newer than the
  running binary (STO-66).
- Metadata read from a source is now size-capped at 8 MiB: `mind.toml`, item
  frontmatter, and Claude plugin and marketplace manifests. An oversized file is
  refused naming the file and the limit, and is never fully read into memory.
  Item content (`{{ns:}}` expansion, the reference scan, `review`, the TUI
  preview, hashing) stays uncapped by decision (DSC-90, DSC-91).
- A repo spec is now refused when the `host`, `owner`, or `repo` it parses to is
  not a single safe path component (empty, `.`/`..`, containing `/` or a control
  character; `host` also rejects `@` and `#`). Those parts are joined into both
  the source identity and the clone path, and the SSH form splits only on the
  first `:`, so `git@../../elsewhere:owner/repo` resolved the clone path outside
  the sources tree, which `meld` deletes before cloning. A melded repo could
  reach this through a nested `[[discover.sources]]` entry, with no per-entry
  consent, and again on every `sync` (CLI-204). The `host` rule also refused
  the standard `ssh://user@host/owner/repo` spelling; the identity host now
  strips a userinfo prefix (split on the last `@`) before validation,
  restoring it, while the full authority is still used for the clone url
  (STO-67).
- Clone paths are now confined to the sources dir before any destructive or
  clone operation, and `Registry::load` revalidates identity parts, the
  identity alias, and pin values on load, dropping an offending entry with a
  warning rather than failing. A stale entry written by an older version could
  otherwise direct a delete outside `~/.mind` (STO-68, STO-69).
- Managed-policy allowlist matching derives the base identity structurally from
  the source's own fields rather than by scanning the identity for the first
  `#`/`@`. `owner` and `repo` may legitimately contain those characters, so a
  string scan admitted a repo named `blessed@evil` under a locked allowlist
  naming `blessed` (POL-68).
- `evolve` refuses a `GITHUB_TOKEN`/`GH_TOKEN` containing characters that would
  inject directives into the generated curl config file (STO-62).
- The TUI's stdout capture file is created exclusively in a 0700 temp directory
  with mode 0600, instead of being created and truncated at a predictable path
  in the shared temp dir, where a pre-planted symlink redirected the write
  (TUI-61).
- `evolve` now passes the `GITHUB_TOKEN`/`GH_TOKEN` bearer header to curl via
  a private 0600 config file instead of the command line, so it is no longer
  visible in the process table. The `wget` fallback still passes it on the
  command line; the shared-host exposure difference is now documented
  (STO-61).
- `review` finding messages are now passed through `strip_ansi` at
  construction, so an ANSI escape or Unicode bidi-override sequence embedded
  in source-controlled text (a token, a hardcoded path) is stripped from both
  the human `error [kind]: ...`/`advisory [kind]: ...` output and the
  `--json` document; previously neither was sanitized (CLI-224).
- Every printed remedy that splices a source- or user-influenced identity into
  a runnable `mind ...` command now shell-quotes that identity, not only the
  `MindError` variants an earlier pass reached: the re-meld ignored-flags and
  `--keep-items` notes, the curator add-root hint, the collision and non-TTY
  install hints, the `source_status` listing, the `probe`/`learn`/`config
  lobes remove` hints, and the `review` shadow note for a directory that also
  parses as a remote spec. A name carrying a quote, `$`, or backtick can no
  longer break out of a command a user pastes (CLI-225).
- `commands.rs` no longer carries its own `strip_ansi`, which stripped only
  bidi overrides and deleted control runs; every caller now routes through the
  shared `sanitize::strip_ansi`, which also strips directional marks
  (U+200E/U+200F/U+061C) and zero-width code points (U+200B/U+2060/U+FEFF) and
  collapses a control run to a space. A hostile plugin or marketplace
  description could previously carry those code points through the weaker copy
  onto stdout and into a `--json` document.

### Documentation

- Tooling guide: documented the `expand:` frontmatter key next to the
  markdown-only rule it relaxes, with the absolute-path rendering, the
  bad-entry failure, and the cross-source caveat; the source-layout page points
  at it from the token section (NS-57, TOOL-20).
- Authoring guide and commands reference: corrected `review`'s target
  precedence to "a target naming an existing directory is read as that local
  path unless it first matches a melded source's identity" (was stated as
  unconditional).
- Configuration guide: corrected lobe backfill to say only a managed lobe
  (not `--snapshot`) gets its directory created immediately, and that
  backfill covers already-installed items of the kinds the new lobe admits,
  not every installed item.
- Enterprise guide: added a `GITHUB_TOKEN`/`GH_TOKEN` visibility note for
  shared hosts, covering the curl-vs-wget token exposure difference (STO-61).
- Policy reference: documented that allow/lock matching runs against the base
  `host/owner/repo` identity, so an item-link or aliased instance inherits its
  repo's allow decision (POL-67).
- Commands reference and configuration guide: documented the `@<prefix>` and
  `#<path>` instance selectors for `unmeld`, `upgrade`, and `recall`, and the
  `meld --namespace` fork-a-new-instance behavior (STO-60).
- Configuration guide and `link-project --snapshot` help text: state the
  frozen-copy caveat that a snapshot is not updated by a later `mind learn`.
- Introduction: added Windsurf to the harness list, noting it is
  project-scoped.
- Spec: corrected STO-56 (`link_into_new_lobes` is intentionally not
  reachability-gated); added LNK-15, recording that a bare and an aliased
  link instance of the same path coexist; reworded the accepted-risks note to
  record that unbounded metadata reads are the norm for this class of tool.

### Migration notes

- A `{{...}}` token in a non-markdown item file (a script, data) no longer
  expands, and `review` now flags every one it finds there, resolvable or
  not (`inert-token`). Move the reference into markdown prose, have the script
  self-locate its own resources, or list the file in the item's `expand:`
  frontmatter to keep expanding its tokens (NS-57).

## [0.21.0] - 2026-07-23

### Added

- Per-instance source aliasing: melding the same repo again under a different
  `--namespace`/`--as` prefix now registers a separate `host/owner/repo@<prefix>`
  instance that coexists with the original, each with its own version pin,
  recorded commit, clone, and installed items. The prefix composes with an
  item-link path as `host/owner/repo#<path>@<prefix>`. So one repo can be melded
  several times under distinct prefixes and their items install side by side
  (STO-58, STO-59).
- `evolve` sends `GITHUB_TOKEN` (or `GH_TOKEN`) as a bearer header on its
  `api.github.com` release lookup, so a shared workplace egress IP no longer hits
  GitHub's unauthenticated 60/hour per-IP rate limit and its 403. The token is
  applied only to the API host, never to the artifact download (STO-57).

### Changed

- `meld <repo> --namespace <prefix>` on an already-melded repo now forks a new
  aliased instance instead of re-prefixing the existing source in place.
  Changing a melded source's prefix in place is now the TUI source-editor's job,
  and stays subject to the mutability lock (no change while items are installed;
  CLI-13, CLI-161, NS-30, TUI-53).

## [0.20.0] - 2026-07-17

### Changed

- Reworked `meld` version pinning into a single `--pin <value>` flag (value
  required). `--pin HEAD` freezes the current resolved tip to its commit;
  `--pin <tag|sha|branch>` resolves that ref and freezes it; `--pin branch=<name>`
  follows a branch and `--pin tag=<name>` follows a moving tag. With no `--pin`, a
  source follows the remote default branch. The old `--follow-branch` /
  `--pin-tag` / `--pin-ref` flags remain as hidden deprecated aliases mapping to
  `--pin branch=` / `--pin tag=` / `--pin <ref>` (CLI-200, CLI-201, CLI-202).

### Added

- `learn <url> --pin` freezes a deep-link's branch ref to its current commit when
  registering the single-item source, instead of tracking the branch.

## [0.19.0] - 2026-07-15

### Added

- Project-scoped lobes: a lobe may be any install-target directory, a global
  agent home or a project subdirectory. `mind config lobes add [<dir>] --preset
  <name>` now combines `--preset` with a base path (previously exclusive), and
  `--subdir <rel>` targets an arbitrary harness subdir under the base (skill-only).
  A registered project lobe is managed like any other, so a later `mind learn`
  fans new skills into it and `forget`/`upgrade`/`introspect` maintain its links.
  A lobe receives links only while its parent directory exists, so a moved or
  deleted project contributes nothing and is never recreated; `introspect --fix`
  prunes a vanished lobe from config and the manifest (HARN-10, HARN-13, STO-56).
- `mind link-project [<dir>]`: shorthand for `config lobes add` targeting a
  project, with `<dir>` defaulting to the current directory and `--preset` to
  `windsurf` (HARN-11, CLI-198).
- `--snapshot` on `config lobes add` / `link-project` writes a one-time frozen
  real-file copy of the installed skills into the target and registers no lobe
  (committable, no auto-propagation); `config lobes remove <path> --snapshot`
  detaches a managed target by converting its symlinks to frozen copies before
  unregistering. Under `--json` a snapshot emits a machine-readable result
  (`outcome` `snapshot`/`no-op` with `count` and frozen keys; `detached` with
  `count` on remove) (HARN-12, HARN-14, CLI-199).
- `windsurf` preset for the Windsurf editor. It is project-scoped: Windsurf reads
  skills only from a project's `.windsurf/skills/`, so `config lobes add --preset
  windsurf` (or `link-project`) targets a project directory, and `config lobes
  detect` recognizes an installed Windsurf via `~/.codeium/windsurf` and prints
  `link-project` guidance instead of adding a global lobe (HARN-4, HARN-5).

## [0.18.0] - 2026-07-15

### Added

- `meld --add-root <dir>` (repeatable): convention-scan roots that compose with
  the source's own discovery instead of replacing it. A `marketplace.json` /
  `plugin.json` or an authoritative `mind.toml` keeps defining its items and
  each added root is scanned in addition (both skill layouts at once, plus
  agents/rules/tools), so items the source does not declare become installable.
  Overlapping paths de-duplicate with the manifest entry winning its namespace
  (DSC-84..86, MKT-17, STO-55, CLI-197).
- Item links: `mind learn <url>` with a deep `tree`/`blob` URL to one skill
  (`https://host/owner/repo/tree/<ref>/<path>`, the `blob/.../SKILL.md` form,
  the GitLab `/-/` variants, or `file://` for a local repo) registers the repo
  as a single-item source instance with identity `host/owner/repo#<path>` and
  installs that skill in one step. The link bypasses the repo's declared
  inventory (a marketplace manifest or authoritative `mind.toml` does not gate
  it), the URL ref supplies the pin (branch follows, 40-hex commit pins), and
  several links into the same repo coexist alongside a plain meld of it.
  `meld <url>` registers through the standard flow; a `[discover].sources`
  entry may also be a link (spec/item-link.md, LNK-1..12).

## [0.17.0] - 2026-07-11

### Added

- `mind hooks run <target>` runs a source's or an item's hooks on demand, outside
  the meld/learn/forget/upgrade flows, reusing the same disclosure and consent
  machinery: run a hook you earlier skipped, re-run one whose effect was lost, or
  retry one that failed transiently. A source install run executes only pending
  install hooks by default (`--force` runs all); an item target (`owner/repo#item`)
  runs the item's install/uninstall hooks in place, and `--event build` rebuilds
  the item through the transactional install path so a failed rebuild leaves the
  existing copy untouched. `mind hooks list <target>` reports the hooks in effect
  -- each hook's event, required/optional flag, command, and the pending/last-ran
  state of recorded install hooks -- without running any (HOOK-100..104,
  CLI-194, CLI-195, CLI-196).

### Changed

- The hook consent disclosure now shows a version-control browse URL pinned to the
  disclosed commit alongside the on-disk clone path, so the exact code a hook will
  run can be read in the forge or locally before approving. The URL is derived with
  the same host rules as the compare URL (GitHub-shaped `https` remotes only; no
  URL for GitLab/Bitbucket hosts, SSH remotes, or local/`file://` sources) and is
  sanitized like the other consent fields (HOOK-24).

## [0.16.0] - 2026-07-09

### Added

- `review` `unshipped-tooling` advisory: flags anything that resolves in the
  author's working tree but is git-untracked, so it is absent from a clone and
  breaks on a remote meld though it works locally. It covers a tool's entrypoint
  script or its `TOOL.md` (CLI-190), any item's `{{self}}/...` or `{{path:...}}/...`
  bundled files (CLI-191), and an authoritative `mind.toml` that declares the
  source's inventory (CLI-193).
- `review` `ns-tool-reference` advisory: a `{{ns:name}}` whose only match is a
  store-only tool. A tool's bare name is not runnable, so this is the silent
  failure mode of a `{{tools:name}}` written as `{{ns:name}}`; the advisory points
  at `{{tools:name}}` / `{{path:tool:name}}` instead (CLI-192).

### Changed

- Bad-reference errors name their specific cause instead of the blanket "does not
  match any item": a `{{tools:name}}` naming a tool with no resolvable entrypoint
  (TOOL-17), a `{{path:ref}}` that is an under-qualified cross-kind ambiguity
  rather than a miss (TOOL-18), and an install-time `requires` entry that is
  malformed, cross-source, or ambiguous (DEP-7). The install-time message now
  matches the cause `review` reports.
- A malformed `plugin.json` / `marketplace.json` reports an error naming the
  actual file, instead of mislabeling it "mind.toml at ...".
- A `mind.toml` hook-event error names the offending item or section, not just
  the file.

## [0.15.0] - 2026-07-07

### Added

- Managed-policy `auto_meld` entries can install items during provisioning:
  `install = true` installs every item the source offers after it is provisioned,
  confirmed already registered, or re-pinned, so `mind sync` on a policy-managed
  machine yields a working agent home with no second command. Build hooks are
  skipped by default and opted in per entry with `run-build-hooks = true`;
  install hooks remain skipped in the non-TTY provisioning context (HOOK-22).
  Per-item failures soft-fail (warn, record, continue, non-zero exit) like other
  provisioning errors (POL-58, POL-59, POL-60).
- `mind sync` reconciles a policy `auto_meld` pin change: when a source is
  already registered but its recorded pin differs from the policy's declared pin,
  the recorded pin is updated and the fetch lands the new ref, reported as
  `re-pinned <name> <old> -> <new>`. A fleet pin bump in policy now reaches
  already-provisioned machines instead of applying only to fresh ones (POL-55).
- A `[sources] allow-local` policy knob (default `true`). With `allow-local =
  false` under `lock = true`, local-path and `file://` melds are refused
  regardless of allow patterns, closing the accidental-bypass where a
  `local/*/*` pattern admits anything a user can clone locally. The refusal names
  the reason and the policy file path (POL-56, POL-57).
- A policy may declare `min-mind-version = "X.Y.Z"`. A binary that understands
  the key but is older reports `managed policy requires mind >= X, running Y;
  upgrade mind` instead of an opaque unknown-field error, and the check runs
  before the strict parse so it wins over any newer key the old binary does not
  know (POL-61, POL-62, POL-63).

### Security

- `mind` warns when the system managed-policy file or its parent directory is
  writable by a non-root user, since a local user could otherwise alter enforced
  policy. The check is a warning, never a refusal (a misprovisioned fleet stays
  functional while the misconfiguration is visible), and is skipped for a
  `MIND_POLICY_FILE` path, which is user-trust by definition (POL-64, POL-65).
- A policy-disallowed `meld` is now refused before any clone. The allow/lock
  check runs on the parsed source identity ahead of the network fetch, so a
  source outside the allowlist produces no egress and no repo content lands on
  disk (the pinned-ref check still runs post-clone, since it needs
  `mind.toml`). Previously the full clone happened and was then deleted
  (POL-36).
- Git stderr echoed on a clone or `sync` failure, and curl/wget output echoed on
  a self-update failure, are stripped of ANSI escapes, control characters, and
  bidi overrides before printing. A hostile source or endpoint can no longer use
  those bytes to spoof or hide the displayed error (CLI-186, STO-54).

### Fixed

- Under `--json`, a `meld` clone failure now carries git's stderr as the cause
  in the error envelope instead of the placeholder `<no stderr>`; the
  human-mode trailer reads `(git output above)` rather than swallowing the cause
  that was just streamed (CLI-184, CLI-185).
- `MIND_HTTP_TIMEOUT_SECS=0` is clamped to the default 15s instead of being
  passed through as "no timeout", which had silently defeated the knob whose
  purpose is bounding a blackholing firewall (STO-52).
- Every `wget` invocation (self-update and `install.sh`) now passes `--tries=1`,
  so a blackholed endpoint no longer takes ~20x the intended timeout bound via
  wget's default 20 retries (STO-53).
- A failed policy `auto_meld` provisioning entry no longer persists a
  partially-registered source: `sync` snapshots and rolls back the in-memory
  source list around each entry, so a later save cannot record a partial entry
  (POL-35).
- The `evolve` proxy hint no longer suggests configuring git's `http.proxy`,
  which has no effect on the curl/wget subprocesses `evolve` uses; it now points
  at `HTTPS_PROXY`/`HTTP_PROXY` and the `~/.curlrc` `proxy-negotiate` escape
  hatch for NTLM/Kerberos proxies.

### Changed

- `learn <typo>` with sources melded no longer suggests `mind sync` in the base
  error; it points only at `mind probe`, since `sync` cannot conjure an item
  name that does not exist (CLI-179).
- The "no sources melded" message is now identical across `sync`, `recall`,
  `recall --sources`, and `probe`, always naming `mind meld <owner/repo>` as the
  next step (CLI-187).
- A `SourceNotAllowed` refusal now names the active policy file path, so a
  developer behind a locked policy can see what refused them (POL-37).
- `mind introspect --json` now includes a `"schema": 1` field alongside the
  existing `issues`, `sources`, and `items` fields, matching the envelope that
  `recall` and `probe` emit. The existing fields are unchanged, so scripts
  keying on them keep working (CLI-189).
- When a managed policy pins `[binary] self-update` below the running binary,
  `evolve` (and `evolve --check`) prints a warning that the running version
  differs from the policy pin, which is an upper bound and does not downgrade.
  The exit code is unchanged and `--json` output is unaffected, so
  `evolve --check --json`'s `outcome` field stays the hook for fleet skew
  monitoring (POL-66).

### Documentation

- Enterprise guide: corrected the release-download CDN host to
  `release-assets.githubusercontent.com` (recommending `*.githubusercontent.com`;
  the previously listed `objects.githubusercontent.com` blocked `evolve` and
  `install.sh` at the redirect); added a binary-update trust-model note
  (checksums verify integrity, not origin; a TLS-terminating proxy can
  substitute both, so the posture for untrusted egress is `self-update = false`
  plus IT-distributed binaries); corrected the proxy env-var guidance (curl
  ignores uppercase `HTTP_PROXY`, wget reads lowercase only) and added the
  `~/.curlrc` escape hatch, a `known_hosts` pre-seed step, a `recall --json` jq
  example, and a GHES non-standard-port note.
- Policy reference: documented the `[binary]` self-update table and its
  `POL-51..54` semantics; corrected the fail-closed claim (plain `recall` and
  `review --policy` remain usable against a malformed deployed policy); reworded
  the POL-11 note to "refused before any clone".
- Install guide: the Updating section warns that a 0.13.0 binary self-deadlocks
  on `evolve` and must be reinstalled.
- Commands reference: the `dump` section names the carried-through key as
  `namespace`, not the stale `as`; the `--json` section now documents
  `introspect`'s real shape (`issues` array with `sources` and `items` integer
  counts) separately from `recall`/`probe`, instead of implying `introspect`
  emits an `items` array.
- Policy reference: documented the `auto_meld` `install` and `run-build-hooks`
  entries, the `[sources] allow-local` knob with the local-path identity shape
  and mirror-directory guidance, `min-mind-version` and the deployment-ordering
  constraint (upgrade binaries before deploying a policy that uses new keys),
  the pin's upper-bound semantics for `evolve`, and a deployment section for the
  policy file's ownership and permissions. The enterprise guide's CI recipe now
  uses `install = true` so `mind sync` followed by `mind recall --json` is a
  complete provisioning step.

## [0.14.0] - 2026-07-06

### Added

- Managed policy can control `mind evolve` via a `[binary]` table:
  `self-update = false` disables self-update entirely (both `evolve` and `evolve
  --check` fail before any network call), `self-update = "X.Y.Z"` pins evolve to
  a version (resolved offline, a conflicting `--version` is rejected), and
  `true`/absent leaves it unrestricted (POL-51, POL-52, POL-53, POL-54).
- Under `--json`, a `MindError` is emitted to stdout as
  `{"schema":1,"error":{"kind":"...","message":"..."}}` instead of only plain
  text on stderr, so a script parsing stdout gets a machine-readable reason. The
  exit code is unchanged, `kind` is a stable per-variant slug, and clap usage
  errors (exit 2) stay plain text (CLI-181, CLI-182, CLI-183).
- Documentation: a "Restricted networks and enterprise" guide page (egress
  endpoints, proxy/CA/private-repo config, the self-update policy knob,
  air-gapped installs, a worked `policy.toml`, and a team/CI provisioning
  recipe), plus `--dangerously-skip-build-hook-check` and troubleshooting
  entries for proxy/CA/auth failures.

### Security

- Source-derived fields in the hook consent disclosure (command, identity, pin
  description, commit, clone path) are stripped of ANSI escapes, control
  characters, and bidi overrides before the prompt is shown, so a malicious
  source can no longer rewrite the warning line or reorder the displayed command
  on the surface where the user consents to run hook code (HOOK-91).
- `[discover]` skill globs now validate the parent directory name (the bare
  skill name) instead of the always-"SKILL" file stem, so a hostile skill
  directory name is rejected at discovery time as the `[[items]]` path already
  was (DSC-83).

### Fixed

- `mind evolve` no longer self-deadlocks on the real update path. `evolve` takes
  no outer command lock; it acquires the exclusive lock itself inside the
  download-and-swap step (STO-46, STO-48). The 0.13.0 classification took the
  lock on one fd and then blocked forever re-acquiring it on a second fd, so any
  `evolve` that reached the download hung. A 0.13.0 binary cannot self-update
  past this hang: reinstall via install.sh, Homebrew, or `cargo install` to get
  this fix.
- `upgrade` produces a `/compare/` link for any https remote, including GitHub
  Enterprise Server, instead of only github.com (CLI-176).
- `upgrade` no longer prints a 404-prone GitHub-shaped compare link for GitLab
  and Bitbucket remotes (hosts containing `gitlab` or `bitbucket`); the link is
  suppressed for those forges and unchanged for GitHub/GHES/Gitea (CLI-188).
- `evolve` network fetches now carry a connect timeout (default 15s, override
  via `MIND_HTTP_TIMEOUT_SECS`) and a generous max-time, so a blackholing
  firewall no longer hangs the update indefinitely; `install.sh` gets the same
  flags. The wget string-fetch path no longer suppresses stderr, so a failure
  reports a real reason, and proxy failures (HTTP 407) carry a
  `HTTPS_PROXY`/`git http.proxy` hint (STO-52).
- `sync` soft-fails individual policy `auto_meld` provisioning entries (warn,
  record, continue) instead of aborting the whole command, so already-melded
  sources still sync when an entry is unreachable; `sync` exits non-zero when
  any entry failed (POL-34, supersedes the POL-32 failure mode).
- A top-level `meld` that fails to clone now leads with git's stderr (the real
  cause) and hints at the SSH remote form, `ssh = true`, or a credential helper
  on an auth failure, and at `HTTPS_PROXY`/`git http.proxy` on an HTTP 407; the
  reconstructed clone command and internal store path move behind `--verbose`
  (CLI-177, CLI-178, CLI-180).
- `learn <typo>` with sources melded points at `mind probe <partial>` to search
  instead of `mind sync`, which cannot conjure a nonexistent item (CLI-179).
- The note printed when `meld` registers only over non-TTY stdin now says
  explicitly "registered only, nothing installed", so a CI run does not mistake
  the exit-0 success for an install.

## [0.13.0] - 2026-07-04

### Security

- The namespace prefix is validated as a single safe path component at every
  ingress (`[source]` declaration, marketplace entry name, `--namespace` flag
  and prompt), and `install()` re-checks the effective name before building
  store/staging/link paths. A hostile prefix like `../../x` can no longer write
  or later delete outside the store and lobes (NS-28, LIFE-44).
- `[discover]` glob patterns are rejected when absolute or `..`-bearing, and
  every match is canonicalized and confined to the source clone, closing an
  arbitrary-file-read into the store (DSC-81).
- `mind evolve` verifies the downloaded tarball against the release's
  `SHA256SUMS` asset before extraction (fails closed), stages the replacement
  binary under a unique non-clobbering name, and holds an exclusive lock across
  the swap (STO-45, STO-46, STO-47). `install.sh` performs the same checksum
  verification.
- All source-derived strings are stripped of ANSI escapes, control characters,
  and bidi overrides before entering the TUI model (TUI-60).
- Managed-policy `auto_meld` pin values (`tag`/`ref`/`follow_branch`) pass
  through git ref validation, rejecting values like `--upload-pack=...`
  (POL-33).

### Added

- Conventional verb aliases: `add` (meld), `install` (learn), `uninstall`
  (forget), `update` (sync), `search` (probe), `list` (recall), `doctor`
  (introspect), `self-update` (evolve). All visible in `--help` (CLI-172).
- `learn`/`upgrade`/`meld`/`sync --upgrade` accept
  `--dangerously-skip-build-hook-check` to run item build hooks
  non-interactively, making built items installable in CI (HOOK-74).
- `MIND_DEFAULT_LOBE` sets the default agent home; `CLAUDE_HOME` remains a
  documented legacy fallback (CLI-170).
- `sources.json` and `manifest.json` carry a `"version": 1` schema field
  (absent = 1 on read); a file written by a newer `mind` produces a clean
  error telling the user to upgrade (STO-50, STO-51).
- The exit-code contract is specified and tested: 0 success, 1 runtime error,
  2 usage error, others reserved (CLI-175).
- Release pipeline: a `SHA256SUMS` asset covering all tarballs, a macos-14 test
  job in CI and the release gate, a tag-vs-Cargo.toml version guard, a pinned
  release toolchain, and a daily canary workflow that melds `jaemk/mind` to
  catch flagship layout drift.
- `cargo install mind-cli` documented as a first-class install method (README,
  install guide, landing page) with a crates.io badge; Intel macOS installs
  this way (no Intel darwin binaries are published).

### Changed

- Breaking: the `-n` short flag now consistently means `--dry-run`.
  `--namespace` is `-N` on `meld`/`review`/`init-source`; `probe --no-tui` is
  long-only (CLI-163, CLI-164, TUI-54).
- `meld --link-only` is renamed `--register-only` and `unmeld --unlink-only`
  is renamed `--keep-items`; the old spellings keep working as hidden
  deprecated aliases (CLI-165, CLI-166).
- Breaking for JSON consumers: `probe --json` and `recall --json` emit
  `{"schema": 1, "items": [...]}` instead of a bare top-level array, and the
  mutating-verb JSON envelope gains `"schema": 1` (CLI-167, CLI-168).
- Breaking for scripts: `upgrade` now fetches each involved source before
  computing deltas (per-source failures are reported and skipped);
  `--no-sync` restores the old fetch-free behavior. `sync --upgrade` remains
  as deprecated sugar (CLI-169).
- `[source].namespace` is the canonical mind.toml key for the namespace
  prefix; `prefix` still parses as a deprecated alias and `init-source`
  rewrites it on update (DSC-82).
- config.toml `absorb-to` (kebab-case) is the canonical key; `absorb_to`
  still parses (CLI-171).
- Future kind words are reserved as namespaces: command, hook, mcp, plugin,
  prompt, mode, output-style (NS-29).
- Onboarding docs teach meld's install-by-default flow: `mind meld <repo>`
  previews the catalog and prompts to install everything; the granular
  register-then-learn path uses `--register-only`. The meld and unmeld help
  text states the install/uninstall defaults (CLI-173, CLI-174).
- `cargo publish` runs from the tagged commit instead of main.
- Cargo.toml carries publish metadata: `keywords`, `categories`, `readme`,
  `rust-version = "1.85"`, and excludes spec/ and docs/ from the crate.

### Removed

- Breaking: the `unmeld detach` and `config target` synonym aliases; both are
  usage errors now. `unlearn` and `status` remain (CLI-172).

### Fixed

- The six README deep links into the docs site 404'd; they now point under
  `/guide/` where the mdBook deploys.
- `ItemNotFound` suggests `mind meld <repo>` when no sources are melded
  instead of the unhelpful sync/probe hint; `UnknownPreset` lists the real
  presets (gemini, codex, universal); `LinkOccupied` names the `--force`
  remedy.
- Content hashing length-prefixes fields and type-tags symlinks so contrived
  (path, content) splits and file/symlink pairs cannot collide (LIFE-35).
  Every stored hash changes: each installed item reports drift once after
  upgrading; run `mind upgrade --yes` to re-record.
- The frontmatter reader strips a leading UTF-8 BOM, so BOM-prefixed items
  keep their descriptions (DSC-23).
- Stale terminology: the old item-upgrade sense of `evolve` replaced with
  `upgrade` across spec and docs; `as =` examples replaced with
  `namespace =`; the formula and about strings list all four item kinds.

### Migration notes

- Replace `meld -n <ns>` / `review -n <ns>` / `init-source -n <ns>` with `-N`
  or `--namespace`; replace `probe -n` with `probe --no-tui`.
- Replace `--link-only` with `--register-only` and `--unlink-only` with
  `--keep-items` (old spellings still work, hidden).
- JSON consumers of `probe --json` / `recall --json` must read the `items`
  field of the new envelope.
- Scripts relying on `upgrade` not fetching should pass `--no-sync`.
- Replace `unmeld detach` with `unmeld`; `config target` with `config lobes`.
- After upgrading, every installed item reports drift once (hash framing
  change); `mind upgrade --yes` re-records the new hashes.
- In mind.toml, prefer `[source].namespace` over `prefix`, and in
  config.toml `absorb-to` over `absorb_to`; the old keys still parse.

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

[Unreleased]: https://github.com/jaemk/mind/compare/v0.26.1...HEAD
[0.26.1]: https://github.com/jaemk/mind/compare/v0.26.0...v0.26.1
[0.26.0]: https://github.com/jaemk/mind/compare/v0.25.0...v0.26.0
[0.25.0]: https://github.com/jaemk/mind/compare/v0.24.0...v0.25.0
[0.24.0]: https://github.com/jaemk/mind/compare/v0.23.0...v0.24.0
[0.23.0]: https://github.com/jaemk/mind/compare/v0.22.0...v0.23.0
[0.22.0]: https://github.com/jaemk/mind/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/jaemk/mind/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/jaemk/mind/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/jaemk/mind/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/jaemk/mind/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/jaemk/mind/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/jaemk/mind/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/jaemk/mind/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/jaemk/mind/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/jaemk/mind/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/jaemk/mind/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/jaemk/mind/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/jaemk/mind/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/jaemk/mind/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/jaemk/mind/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/jaemk/mind/compare/v0.6.2...v0.7.0
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
