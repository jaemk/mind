# Managed policy (enterprise)

A managed policy file lets an administrator constrain `mind` machine-wide. It
restricts the client to a trusted set of sources and locks related settings,
modeled on Claude Code's managed settings: a file at a fixed system path takes
precedence over user configuration and the user cannot override it.

This is a compliance guardrail, not a security sandbox. `mind` runs with the
user's own privileges, so a user can still place agent files under an agent home
by hand without invoking `mind`; the policy cannot prevent that. What it provides
is a policy the user cannot edit (enforced by file permissions), refusal of
disallowed operations through `mind`, and auditability.

## The policy file

`mind` reads a managed policy file on every invocation from a fixed per-OS system
path (POL-1):

| OS | Path |
|----|------|
| Linux | `/etc/mind/policy.toml` |
| macOS | `/Library/Application Support/mind/policy.toml` |
| Windows | `%PROGRAMDATA%\mind\policy.toml` |

Windows-native use is not a supported configuration. For WSL environments, deploy
the policy to `/etc/mind/policy.toml` within the WSL distribution (the Linux path
above applies inside WSL).

The path is owned by an administrator and world-readable but not user-writable.
It is not relocatable by `MIND_HOME` or other user environment (POL-2).

`MIND_POLICY_FILE` overrides the path only when no file exists at the system path
(for tests and non-managed use). When the system file exists it is authoritative
and `MIND_POLICY_FILE` is ignored. The env path is honored only when the file it
names exists; a set-but-missing `MIND_POLICY_FILE` is treated as no policy, not a
hard error (POL-2).

When a policy is in effect it is authoritative over user configuration
(`~/.mind/config.toml`) and the user's registry: a user cannot widen what the
policy restricts (POL-3).

With no policy file present, `mind` is unmanaged and behaves exactly as it does
without this feature. Every control below is opt-in and absent unless the policy
sets it, so the feature is inert by default (POL-4).

A policy file that does not parse, has unknown keys, or violates an internal rule
is a hard error on every command that loads it (fail closed), naming the problem;
`mind` does not silently fall back to unmanaged (POL-5). The one exception is
plain `mind recall` (read-only, no agent-home access): it does not load the
policy and remains usable as an escape hatch during a botched rollout.
`mind review --policy <file>` bypasses the system policy path and can diagnose a
broken deployed policy without needing to fix it first. Validate a new policy
with `mind review --policy` before deploying it.

## Schema

```toml
[sources]
# Allowlist matched against a source's host/owner/repo identity. `*` matches
# within one path segment.
allow = ["github.com/acme/*", "github.example.com/platform/*"]

# Refuse to meld any source whose identity is not in `allow`. Without lock, the
# allowlist is advisory (a non-matching meld warns but proceeds).
lock = true

# Forbid local-path and file:// melds under lock regardless of allow patterns.
# Default is true (local-path melds are evaluated against allow like any source).
allow-local = false

# Every meld must resolve to a tag or ref; floating branches are refused. When
# pinned, every auto_meld entry below must declare a tag or ref.
pinned = true

# Sources mind provisions automatically (melds if not already present), during
# `sync`. `repo` is a repo spec as `meld` accepts (owner/repo, a URL, git@, or a
# path); its derived host/owner/repo identity must satisfy `allow` under lock.
#
# `install = true` (default false): after provisioning, install every item the
# source offers headlessly (equivalent to `learn '<source>#*' --yes`).
# `run-build-hooks = true` (default false): also run item build hooks during the
# headless install; equivalent to --dangerously-skip-build-hook-check. Only
# meaningful when `install = true`.
[[sources.auto_meld]]
repo = "acme/agent-baseline"
tag = "v1.4.0"
install = true

[[sources.auto_meld]]
repo = "https://github.example.com/platform/security-rules"
ref = "9f3a1c2e7b1d0a4c5e6f8a9b0c1d2e3f40516273"
install = true

[lobes]
# Lock the agent homes: `config lobes` edits and $MIND_AGENT_HOMES are refused,
# and the effective homes are exactly `targets`. With lock off, `targets` is a
# base set the user's configured lobes are unioned onto.
lock = true
targets = ["~/.claude"]

[binary]
# Control `mind evolve` (binary self-update). Absent or true: unrestricted.
# false: evolve and evolve --check fail before any network call.
# A version string: evolve resolves to that version offline; --version with a
# different value is refused; an invalid string fails policy parse.
self-update = false     # or true, or "0.14.0"
```

The scalar `[sources]` keys precede the `[[sources.auto_meld]]` tables, per TOML
ordering. Source identity is `host/owner/repo`.

### Trusted-source allowlist

`[sources].allow` is a list of patterns matched against a source's
`host/owner/repo` identity, where `*` matches within a path segment, so
`github.com/acme/*` matches every repo under `acme` (POL-10). When GHES runs on
a non-standard port, the identity includes the port
(e.g. `github.example.com:8443/owner/repo`); patterns must include the port to
match, for example `github.example.com:8443/platform/*`.

**Local-path identity.** A `meld` of an absolute path, a `./`/`../` relative
path, or a `file://` URL gets identity `local/<parent-dir>/<repo>` where
`<parent-dir>` is the last path component above the repo directory. For example,
`/srv/mirrors/agent-baseline` gets identity `local/mirrors/agent-baseline`. To
allow a mirror directory under lock, add a pattern like `local/mirrors/*` to
`allow`.

`[sources].lock` is the enforcement switch:

- With `lock = true`, `meld` refuses any repo whose identity does not match
  `allow` (`SourceNotAllowed`) before any clone or egress (POL-11). `learn`,
  `sync`, and `upgrade` operate only on allow-matching sources. A registered
  source that is no longer allowed is reported and skipped, not updated or
  installed from (POL-12).
- With `lock` absent or false, `allow` is advisory: a non-matching `meld` is
  warned about but not refused, and the verbs above are not restricted (POL-13).

### Local-path control (`allow-local`)

```toml
[sources]
allow-local = false   # default: true
```

`allow-local = false` forbids all local-path and `file://` melds when `lock =
true`, regardless of what `allow` patterns match (POL-56). The error names
`allow-local = false` as the reason, distinct from the generic pattern-miss
message. With `lock` absent or false, `allow-local` has no effect.

`allow-local = true` (the default) preserves existing behavior: local-path and
`file://` melds are evaluated against `allow` like any other source under lock
(POL-57).

**When to use `allow-local = false`.** Admitting local paths under a lock
delegates control to whoever can write that directory: a user who can clone any
repo locally and meld it from a path matching `local/*/*` bypasses the network
restriction. `allow-local = false` closes that gap for machines where the lock
is meant to enforce source origin, not just pattern matching. Combine it with
filesystem permissions on any mirror directory that employees can read but not
write to.

### Require pinned

With `[sources].pinned = true`, every `meld` must resolve to a tag or ref pin
(`--pin-tag` / `--pin-ref`, or a `[source]` pin directive that resolves to a tag
or ref); a floating branch (`--follow-branch` or the default branch) is refused
(`UnpinnedSourceForbidden`) (POL-20).

With `pinned = true`, every `[sources].auto_meld` entry must declare a `tag` or
`ref`. A policy whose `auto_meld` contains an unpinned entry or a `follow_branch`
entry is invalid and is reported by `mind review --policy` (POL-21).

### Auto-meld (org provisioning)

`[sources].auto_meld` is a list of tables, each with `repo` (a repo spec as
`meld` accepts: `owner/repo`, a URL, `git@...`, or a path) and an optional pin:
`tag`, `ref`, or `follow_branch`. `mind` provisions these by melding any that are
not already melded (POL-30). It is a base set, not an exclusive one: with `lock`
off the user may meld additional sources beyond it; when locked, only
allow-matching sources are permitted.

Every `auto_meld` entry must satisfy `allow` when `lock` is true, matched on the
`host/owner/repo` identity derived from its `repo` spec. An entry outside the
allowlist, or whose `repo` does not parse, is an invalid policy (POL-31).

Auto-meld provisioning runs during `sync`, using each entry's declared pin. It is
idempotent: an entry already melded at the declared pin is left unchanged
(POL-32).

**Item install.** By default, provisioning registers the source only; no items
are installed. Set `install = true` on an entry to also install every item the
source offers, headlessly, after successful provisioning (POL-58). This is
equivalent to running `mind learn '<source>#*' --yes` for that source.

Build hooks for items installed via the `install = true` pass are skipped by
default (HOOK-72 non-TTY path). Set `run-build-hooks = true` on the same entry
to run them, equivalent to `--dangerously-skip-build-hook-check` (POL-59). Only
set this for sources you control or have audited, since build hooks are arbitrary
code.

Install hooks follow the same non-TTY path: they are skipped in a non-interactive
context. A failed item install is soft-failed per POL-34 (warn, record, continue);
other items and sources still process, and `sync` exits non-zero if any item
failed (POL-60).

### Lobe lock

With `[lobes].lock = true`, the effective agent homes are exactly
`[lobes].targets`; `config lobes add` / `config lobes remove` are refused and the
user's `lobes` (from `~/.mind/config.toml`) and `MIND_AGENT_HOMES` are ignored.
With `targets` absent under a lock, the lock pins the default `~/.claude`
(POL-40).

With `[lobes].lock` absent or false, `[lobes].targets` is a base set the user
extends: the effective agent homes are `targets` unioned with the user's
configured `lobes` (POL-41).

### Binary self-update

`[binary].self-update` controls `mind evolve` (and its `self-update` alias). A
missing `[binary]` table or missing key leaves `evolve` unrestricted (POL-51).

- `self-update = false`: both `mind evolve` and `mind evolve --check` fail before
  any network call with a policy error. The disable applies to `--check` as well
  so the binary does not nag about available updates (POL-52).
- `self-update = "X.Y.Z"`: `evolve` resolves to that exact version offline (no
  `api.github.com` call), behaving as if `--version X.Y.Z` were passed. The pin
  is validated as a dotted-numeric version at policy-parse time (POL-5 fail
  closed); a leading `v` is stripped before validation. If `--version V` is also
  passed and V differs from the pin, the command fails with a policy error naming
  the conflict. `evolve --check` reports against the pinned version and respects
  the no-downgrade logic (POL-53).
- `self-update = true`: identical to the absent key; `evolve` is unrestricted.
  Exists so a policy can explicitly re-enable updates in a layered deployment
  (POL-54).

The pin is an upper bound for `evolve`, not a fleet version enforcement (POL-66).
When IT distributes a binary newer than the pin (e.g. pin `0.14.0`, running binary
`0.15.0`), `evolve` and `evolve --check` print a human-readable warning and exit 0,
rather than downgrading:

```
warning: running 0.15.0 differs from the managed policy pin 0.14.0; the policy
pin is an upper bound and does not downgrade
```

The `--json` path emits no warning text on stdout; the structured `outcome` field
(`not-downgrading`) is the machine-readable hook for fleet skew monitoring. See
[IT-managed binaries](enterprise.md#it-managed-binaries-no-self-update) in the
enterprise guide for deployment guidance and the trust model for binary updates.

## Schema evolution and deployment ordering

A policy file may only use keys the oldest deployed binary understands. Adding a
new policy key (such as `[binary]` in 0.14.0) to a policy file while some fleet
machines still run an older binary causes every `mind` command on those machines
to fail with an "unknown field" error (fail-closed per POL-5).

**Upgrade binaries before deploying a policy that uses new keys.**

Starting with 0.15.0, a policy may declare `min-mind-version` at the top level:

```toml
min-mind-version = "0.15.0"

[sources]
lock = true
```

`mind` checks this key before the strict field-validation parse (POL-61). When a
binary older than `min-mind-version` loads the policy, it reports:

```
error: invalid managed policy at /etc/mind/policy.toml:
managed policy requires mind >= 0.15.0, running 0.14.0; upgrade mind
```

instead of an opaque "unknown field" error (POL-62). The gate only helps binaries
built after 0.15.0; the ordering constraint still applies to older binaries. An
invalid `min-mind-version` value (not a dotted-numeric string) is a hard parse
error consistent with POL-5 (POL-63). Validate any policy with
`mind review --policy <path>` before deploying it.

## Deploying the policy file

The policy file should be owned by root and not writable by non-root users.
Correct deployment on Linux:

```sh
sudo mkdir -p /etc/mind
sudo cp policy.toml /etc/mind/policy.toml
sudo chown root:root /etc/mind/policy.toml
sudo chmod 644 /etc/mind/policy.toml
sudo chown root:root /etc/mind
sudo chmod 755 /etc/mind
```

For macOS (`/Library/Application Support/mind/`):

```sh
sudo mkdir -p "/Library/Application Support/mind"
sudo cp policy.toml "/Library/Application Support/mind/policy.toml"
sudo chown root:wheel "/Library/Application Support/mind/policy.toml"
sudo chmod 644 "/Library/Application Support/mind/policy.toml"
sudo chown root:wheel "/Library/Application Support/mind"
sudo chmod 755 "/Library/Application Support/mind"
```

An Ansible task:

```yaml
- name: deploy mind policy
  copy:
    src: policy.toml
    dest: /etc/mind/policy.toml
    owner: root
    group: root
    mode: "0644"
- name: secure policy directory
  file:
    path: /etc/mind
    owner: root
    group: root
    mode: "0755"
```

When `mind` loads the system policy file, it warns to stderr if the file or its
parent directory is group/world-writable or not root-owned (POL-64). The warning
is not a refusal: the policy still loads and enforces, so a misprovisioned fleet
stays functional while the misconfiguration is visible. Example:

```
warning: managed policy /etc/mind/policy.toml is group/world-writable; a local
user could alter enforced policy. chown root and chmod 644.
```

The check is skipped when the policy was located via `$MIND_POLICY_FILE`
(user-trust path, POL-65).

## Validation

`mind review --policy <path>` statically validates a managed policy file without
cloning: it parses the TOML, rejects unknown keys, checks that `allow` patterns
are well formed and `auto_meld` repo specs parse, enforces that every `auto_meld`
entry is pinned when `pinned = true` (POL-21), and that every `auto_meld` entry
satisfies `allow` when locked (POL-31). It reports hard errors and advisories and
exits non-zero on a hard error (POL-50).

## Caveat: install hooks are not gated

The policy does not currently gate install-hook execution. A source's build
command is arbitrary code, and it still runs under the normal hook safety prompt
rather than under any policy control. How a policy should govern install hooks is
an open research item, not yet specified. See [Install hooks](install-hooks.md)
for the disclosure and prompt model that applies.

## Example

A worked example is at
[examples/policy](https://github.com/jaemk/mind/tree/main/examples/policy).
