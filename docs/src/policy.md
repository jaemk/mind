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
is a hard error on every command (fail closed), naming the problem; `mind` does
not silently fall back to unmanaged (POL-5). Validate a policy with
`mind review --policy` before deploying it.

## Schema

```toml
[sources]
# Allowlist matched against a source's host/owner/repo identity. `*` matches
# within one path segment.
allow = ["github.com/acme/*", "github.example.com/platform/*"]

# Refuse to meld any source whose identity is not in `allow`. Without lock, the
# allowlist is advisory (a non-matching meld warns but proceeds).
lock = true

# Every meld must resolve to a tag or ref; floating branches are refused. When
# pinned, every auto_meld entry below must declare a tag or ref.
pinned = true

# Sources mind provisions automatically (melds if not already present), during
# `sync`. `repo` is a repo spec as `meld` accepts (owner/repo, a URL, git@, or a
# path); its derived host/owner/repo identity must satisfy `allow` under lock.
[[sources.auto_meld]]
repo = "acme/agent-baseline"
tag = "v1.4.0"

[[sources.auto_meld]]
repo = "https://github.example.com/platform/security-rules"
ref = "9f3a1c2e7b1d0a4c5e6f8a9b0c1d2e3f40516273"

[lobes]
# Lock the agent homes: `config lobes` edits and $MIND_AGENT_HOMES are refused,
# and the effective homes are exactly `targets`. With lock off, `targets` is a
# base set the user's configured lobes are unioned onto.
lock = true
targets = ["~/.claude"]
```

The scalar `[sources]` keys precede the `[[sources.auto_meld]]` tables, per TOML
ordering. Source identity is `host/owner/repo`.

### Trusted-source allowlist

`[sources].allow` is a list of patterns matched against a source's
`host/owner/repo` identity, where `*` matches within a path segment, so
`github.com/acme/*` matches every repo under `acme` (POL-10).

`[sources].lock` is the enforcement switch:

- With `lock = true`, `meld` refuses any repo whose identity does not match
  `allow` (`SourceNotAllowed`); nothing is cloned or registered (POL-11). `learn`,
  `sync`, and `upgrade` operate only on allow-matching sources. A registered
  source that is no longer allowed is reported and skipped, not updated or
  installed from (POL-12).
- With `lock` absent or false, `allow` is advisory: a non-matching `meld` is
  warned about but not refused, and the verbs above are not restricted (POL-13).

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

### Lobe lock

With `[lobes].lock = true`, the effective agent homes are exactly
`[lobes].targets`; `config lobes add` / `config lobes remove` are refused and the
user's `lobes` (from `~/.mind/config.toml`) and `MIND_AGENT_HOMES` are ignored.
With `targets` absent under a lock, the lock pins the default `~/.claude`
(POL-40).

With `[lobes].lock` absent or false, `[lobes].targets` is a base set the user
extends: the effective agent homes are `targets` unioned with the user's
configured `lobes` (POL-41).

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
