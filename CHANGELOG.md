# Changelog

All notable changes to `mind` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/jaemk/mind/compare/v0.21.0...HEAD
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
