# Managed policy (enterprise)

Status: done. A managed policy file, controlled by an organization and not
editable by regular users, restricts a `mind` client to a trusted set of sources
and locks related settings. Modeled on Claude Code's managed settings: a file at
a fixed system path takes precedence over user configuration and the user cannot
override it.

## Overview

An organization distributes a policy file out of band (MDM, configuration
management) to a fixed system path, owned by an administrator and world-readable
but not user-writable. `mind` reads it on every invocation and enforces it. The
controls are a trusted-source allowlist, a require-pinned rule, an auto-meld set
of org-provisioned sources, and a lobe lock.

This is a compliance guardrail, not a security sandbox. `mind` runs with the
user's own privileges, so a user can still place agent files under an agent home
directly without invoking `mind`; the policy cannot prevent that, exactly as
Claude Code's managed settings are bypassable with local administrator rights.
What it provides is: a policy the user cannot edit (enforced by file
permissions), refusal of disallowed operations through `mind`, and auditability.

The TOML shape (scalar `[sources]` keys precede the `[[sources.auto_meld]]`
tables, per TOML ordering):

```toml
[sources]
allow  = ["github.com/acme/*", "github.example.com/platform/agents"]
lock   = true
pinned = true

[[sources.auto_meld]]
repo = "acme/agent-baseline"
tag  = "v1.4.0"

[[sources.auto_meld]]
repo = "acme/security-rules"
ref  = "9f3a1c2e"

[lobes]
lock    = true
targets = ["~/.claude"]
```

A worked example is at [../examples/policy/](../examples/policy/). How a policy
should govern install hooks (a source's build command, arbitrary code) is an open
research item, not yet specified here; see the research section in
[install-hooks.md](install-hooks.md). The rest of this document states the rules
normatively. Source identity is `host/owner/repo` (see storage.md).

## The policy file

- `POL-1` `mind` reads a managed policy file on every invocation from a fixed
  per-OS system path: `/etc/mind/policy.toml` (Linux), `/Library/Application
  Support/mind/policy.toml` (macOS), `%PROGRAMDATA%\mind\policy.toml` (Windows).
- `POL-2` The policy path is not relocatable by `MIND_HOME` or other user
  environment. `$MIND_POLICY_FILE`, if set, is honored only when no file exists
  at the system path (for tests and non-managed use); when the system file
  exists it is authoritative and `$MIND_POLICY_FILE` is ignored. The env path is
  honored only when the file it names exists; a set-but-missing `$MIND_POLICY_FILE`
  is treated as no policy (POL-4 inert), not a hard error, mirroring the
  system-path existence check.
- `POL-3` When a policy is in effect it is authoritative over user configuration
  (`~/.mind/config.toml`) and the user's registry: a user cannot widen what the
  policy restricts.
- `POL-4` With no policy file present, `mind` is unmanaged and behaves exactly as
  it does today. Every control below is opt-in and absent unless the policy sets
  it, so the feature is inert by default.
- `POL-5` A policy file that does not parse, has unknown keys, or violates an
  internal rule (e.g. POL-21) is a hard error on every command (fail closed),
  naming the problem; `mind` does not silently fall back to unmanaged. Validate a
  policy with `mind review --policy` (POL-50) before deploying it.

## Trusted-source allowlist

- `POL-10` `[sources].allow` is a list of patterns matched against a source's
  `host/owner/repo` identity, where `*` matches within a path segment (e.g.
  `github.com/acme/*` matches every repo under `acme`).
- `POL-11` With `[sources].lock = true`, `meld` refuses any repo whose identity
  does not match `allow` (`SourceNotAllowed`); nothing is cloned or registered.
  The check fires before any git network call: the source identity is derived
  from the parsed repo spec alone, so no clone is needed and no egress occurs
  for a refused source (POL-36).
- `POL-36` The allow/lock refusal (POL-11) is enforced before any git network
  contact or directory creation. Only the pinned check (POL-20) remains
  post-clone because it requires reading the source's `mind.toml`. A refused
  source leaves no clone or intermediate staging directory on disk.
- `POL-12` When `lock` is true (POL-13), `learn`, `sync`, and `upgrade` operate
  only on sources whose identity matches `allow`. A source already in the registry
  that is no longer allowed is reported and skipped, not updated or installed from.
  With `lock` off, `allow` is advisory and these verbs are not restricted.
- `POL-13` With `lock` absent or false, `allow` is advisory: a non-matching
  `meld` is warned about but not refused. `lock` is the enforcement switch.
- `POL-37` When `meld` returns `SourceNotAllowed` (POL-11), the effective policy
  file path is printed to stderr as a hint so a developer behind a locked policy
  can locate the file that refused them.
- `POL-56` When `[sources].allow-local = false`, a local-path meld (identity
  prefixed `local/`, derived from an absolute path or `./`/`../` relative path)
  and a `file://` remote meld are refused under a locked policy (`lock = true`)
  regardless of `allow` patterns. The refusal names `allow-local = false` as the
  reason (distinct from the `SourceNotAllowed` pattern-miss message) and prints
  the policy file path as a hint (same POL-37 hint mechanism). With `lock`
  absent or false, `allow-local` has no effect regardless of its value.
- `POL-57` `[sources].allow-local = true` is the default and preserves the
  existing behavior: local-path and `file://` melds still require an `allow`
  pattern match when `lock = true` (POL-11 applies). Absent `allow-local` is
  equivalent to `allow-local = true`.

## Require pinned

- `POL-20` With `[sources].pinned = true`, every `meld` must resolve to a tag or
  ref pin (`--pin-tag` / `--pin-ref`, or a `[source]` pin directive that resolves
  to a tag or ref, see DSC-41); a floating branch (`--follow-branch` or the
  default branch) is refused (`UnpinnedSourceForbidden`).
- `POL-21` With `pinned = true`, every `[sources].auto_meld` entry must declare a
  `tag` or `ref`. A policy whose `auto_meld` contains an entry with no pin or with
  `follow_branch` is invalid (POL-5) and is reported by `mind review --policy`.

## Auto-meld (org provisioning)

- `POL-30` `[sources].auto_meld` is a list of tables, each with `repo` (a repo
  spec as `meld` accepts: `owner/repo`, a URL, `git@...`, or a path) and an
  optional pin: `tag`, `ref`, or `follow_branch`.
  `mind` provisions these by melding any that are not already melded. It is a base
  set, not an exclusive one: when `[sources].lock` is off the user may meld
  additional sources beyond it (POL-13); when locked, only `allow`-matching
  sources are permitted (POL-11).
- `POL-31` Every `auto_meld` entry must satisfy `allow` when `lock` is true,
  matched on the `host/owner/repo` identity derived from its `repo` spec (the
  same form POL-11 enforces at meld time, so a shorthand spec validates against a
  host-qualified pattern). An entry whose identity is outside the allowlist, or
  whose `repo` does not parse, is an invalid policy (POL-5).
- `POL-32` Auto-meld provisioning runs during `sync`, using each entry's declared
  pin. It is idempotent: an entry already melded at the declared pin is left
  unchanged. `auto_meld` may point at a curated super-source, which then discovers
  its nested sources (DSC-38). (Superseded for failure mode by POL-34.)
- `POL-33` The `tag`, `ref`, and `follow_branch` values in each
  `[[sources.auto_meld]]` entry are validated by the same `validate_ref_value`
  rule as `[source]` pin values declared in `mind.toml` (DSC-66). A value that
  begins with `-`, contains ASCII whitespace or control characters, or uses `..`
  is rejected as an invalid policy (POL-5). This prevents a hostile value such as
  `--upload-pack=...` from reaching the git subprocess.
- `POL-34` Auto-meld provisioning failures during `sync` are soft: a failure to
  provision an entry (e.g. the host is unreachable) is warned about on stderr
  naming the entry and the error, recorded, and the provisioning loop continues to
  the next entry. Sources already melded before the provisioning pass still sync
  normally. If any provisioning entry failed, `sync` exits non-zero after all
  sources have been synced, reporting the combined count of provisioning and
  per-source sync failures.
- `POL-35` When an auto-meld provisioning entry fails after `meld_recursive` has
  already pushed sources into the in-memory registry, the registry is rolled back
  to its pre-entry state before the soft-fail handler records the error. This
  prevents a subsequent `registry.save` from persisting a partial or failed entry
  alongside successfully provisioned ones.
- `POL-55` When an already-registered source's recorded pin differs from the
  `auto_meld` entry's declared pin, `sync` treats the discrepancy as pin drift
  and updates the recorded pin to the declared value; the subsequent per-source
  sync fetch for that source lands the new ref. The re-pin is reported in the
  sync output as `re-pinned <name> <old> -> <new>`. The declared pin was already
  validated by `validate_ref_value` at policy parse time (POL-33), so no
  re-validation is performed here. A failure during reconciliation is handled as
  a provisioning error per POL-34 (soft-fail: warn, record, continue, non-zero
  exit). An entry whose recorded pin already equals the declared pin is left
  unchanged. This supersedes the unconditional idempotency of POL-32: that rule's
  "already melded at the declared pin is left unchanged" wording now applies only
  when the recorded and declared pins match.
- `POL-58` An `[[sources.auto_meld]]` entry may declare `install = true` (boolean,
  default `false`). When `true`, after the source ends the sync registered -- by
  fresh provisioning, same-pin idempotent confirmation (POL-32), or pin-drift
  reconciliation (POL-55) -- `sync` installs every item the source offers headlessly
  (equivalent to `mind learn '<source>#*' --yes`). Repeat syncs are idempotent:
  already-installed items are skipped so no reinstall occurs. When `false` or absent,
  `sync` registers the source only and installs nothing; this is the default and
  preserves the existing behavior.
- `POL-59` Build hooks for items installed via the POL-58 pass are skipped by
  default, following the standard non-TTY skip path (HOOK-72). Setting
  `run-build-hooks = true` on the auto_meld entry enables them, equivalent to
  `--dangerously-skip-build-hook-check`. Install hooks follow the same non-TTY
  path: they are not prompted and are skipped in a non-interactive context.
- `POL-60` Per-item install failures during the POL-58 pass are soft-failed in the
  same manner as provisioning failures (POL-34): each failed item is warned about on
  stderr (naming the entry and the item key), recorded, and installation continues
  for the remaining items and sources. Any item install failure causes `sync` to
  exit non-zero, combined with provisioning failures in the total reported.

## Schema evolution and min-mind-version

- `POL-61` The policy file may declare an optional top-level key
  `min-mind-version = "X.Y"` (or `"X.Y.Z"`) naming the minimum `mind` version the
  policy requires. When present it is checked BEFORE the strict `deny_unknown_fields`
  parse (POL-5): a permissive intermediate deserialization reads `min-mind-version`
  and ignores all other keys, the version comparison runs, and only on success does
  the strict parse proceed. The ordering ensures an old binary that does not yet know
  a new key still sees the version error rather than an opaque unknown-field error.
- `POL-62` If the declared `min-mind-version` exceeds the running binary version,
  `mind` returns `InvalidPolicy` naming the policy file path with the message:
  "managed policy requires mind >= <X.Y.Z>, running <current>; upgrade mind". This
  replaces the opaque unknown-field failure for schema-skew scenarios (see DSC-40
  for the same gate on source `mind.toml` files; POL-5 for the fail-closed rule).
- `POL-63` A `min-mind-version` value that is not a valid dotted-numeric version
  string (e.g. `"not-a-version"`, `""`, `"1.x"`) is a hard policy-parse error
  (fail closed, consistent with POL-5). Format validation mirrors the source-side
  `min-mind-version` check (DSC-40).

## Policy file security

- `POL-64` On unix, when loading the SYSTEM policy file, `mind` checks the file
  and its parent directory for unsafe ownership or permissions. If either the policy
  file or its parent dir is group/world-writable (mode bits 0o022) or owned by a
  non-root uid (uid != 0), `mind` emits a warning to stderr naming the path and the
  problem (e.g. "warning: managed policy <path> is group/world-writable; a local
  user could alter enforced policy. chown root and chmod 644."). The warning is
  never a refusal: the policy loads and enforces regardless, so a misprovisioned
  fleet stays functional while the misconfiguration is visible.
- `POL-65` The permission check (POL-64) is skipped when the policy was located
  via `$MIND_POLICY_FILE` (that path is user-trust by definition) and is a no-op
  on non-unix platforms.

## Lobe lock

- `POL-40` With `[lobes].lock = true`, the effective agent homes are exactly
  `[lobes].targets`; `config lobes add` / `config lobes remove` are refused and
  the user's `lobes` (from `~/.mind/config.toml`) and `$MIND_AGENT_HOMES` are
  ignored. With `targets` absent under a lock, the lock pins the default
  (`~/.claude`).
- `POL-41` With `[lobes].lock` absent or false, `[lobes].targets` is a base set
  the user extends: the effective agent homes are `targets` unioned with the
  user's configured `lobes` (the policy's targets are always present, and the user
  may add more). This mirrors `auto_meld` (POL-30): a policy-provided base that
  the user can add to when the corresponding lock is off.

## Binary self-update control

- `POL-51` The `[binary]` table in the managed policy controls the `mind evolve`
  command (and its `self-update` alias). A missing `[binary]` table, or a missing
  `self-update` key within it, leaves `evolve` unrestricted (default, unchanged
  behavior; POL-4 inert).
- `POL-52` `[binary].self-update = false` disables `evolve` entirely. Both
  `mind evolve` and `mind evolve --check` fail before any network call with a
  `SelfUpdatePolicy` error. Rationale: an organization that disables updates does
  not want version-nagging either.
- `POL-53` `[binary].self-update = "<version>"` pins the target to that version
  string, behaving exactly as if `--version <pin>` were passed (offline resolution;
  the GitHub API is not consulted for "latest"). The pinned version string is
  validated as a dotted numeric value (e.g. `"0.14.0"`) at policy-parse time
  (POL-5 fail closed); a leading `v` is stripped before validation. If the user
  also passes `--version V` where V differs from the pin, the command fails with
  `SelfUpdatePolicy` naming the conflict. `evolve --check` reports against the
  pinned version and respects the existing `PinnedBelowCurrent` no-downgrade logic.
- `POL-54` `[binary].self-update = true` is identical to the absent key: `evolve`
  is allowed to any version. It exists so a policy file can explicitly re-enable
  updates after a `false` entry in a layered deployment.
- `POL-66` The policy pin is an upper bound for `evolve`, not a fleet-version
  enforcement mechanism. When the running binary version is strictly above the
  policy pin (the `PinnedBelowCurrent` case: IT distributed a binary newer than
  the pin), `evolve` and `evolve --check` print a human-readable warning to stdout
  stating that the running version differs from the policy pin and that the pin
  does not downgrade (e.g. "warning: running <running> differs from the managed
  policy pin <pin>; the policy pin is an upper bound and does not downgrade").
  The warning is printed only in human mode; the `--json` path emits no warning
  text on stdout (the structured `outcome` field, e.g. `not-downgrading`, is the
  machine-readable detection hook for fleet skew monitoring). The exit code is
  unchanged (0) -- this is a visibility warning, not an error, and must not break
  an `evolve` invocation in a cron job.

## Validation (`mind review --policy`)

- `POL-50` `mind review --policy <path>` statically validates a managed policy
  file without cloning: it parses the TOML and rejects unknown keys (a parse
  failure is a hard error), then emits advisories for a policy that would block
  every meld (`lock = true` with an empty `allow`) or let org-provisioned sources
  float (`auto_meld` entries present with `pinned = false` and `lock = false`). It
  reports hard errors and advisories and exits non-zero on a hard error, mirroring
  source review (CLI-130..133). The pinned-when-`pinned` (POL-21) and
  satisfies-`allow`-when-locked (POL-31) constraints are enforced at meld time, not
  re-checked here.
