# Managed policy (enterprise)

Status: planned. A managed policy file, controlled by an organization and not
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

A worked example is at [../examples/policy/](../examples/policy/). The rest of
this document states the rules normatively. Source identity is `host/owner/repo`
(see storage.md).

## The policy file

- `POL-1` `mind` reads a managed policy file on every invocation from a fixed
  per-OS system path: `/etc/mind/policy.toml` (Linux), `/Library/Application
  Support/mind/policy.toml` (macOS), `%PROGRAMDATA%\mind\policy.toml` (Windows).
- `POL-2` The policy path is not relocatable by `MIND_HOME` or other user
  environment. `$MIND_POLICY_FILE`, if set, is honored only when no file exists
  at the system path (for tests and non-managed use); when the system file
  exists it is authoritative and `$MIND_POLICY_FILE` is ignored.
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
- `POL-12` `learn`, `sync`, and `evolve` operate only on sources whose identity
  matches `allow`. A source already in the registry that is no longer allowed is
  reported and skipped, not updated or installed from.
- `POL-13` With `lock` absent or false, `allow` is advisory: a non-matching
  `meld` is warned about but not refused. `lock` is the enforcement switch.

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
- `POL-32` Auto-meld provisioning runs during `sync` (and on first use when an
  entry is missing), using each entry's declared pin. It is idempotent: an entry
  already melded at the declared pin is left unchanged. `auto_meld` may point at a
  curated super-source, which then discovers its nested sources (DSC-38).

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

## Validation (`mind review --policy`)

- `POL-50` `mind review --policy <path>` statically validates a managed policy
  file without cloning: it parses the TOML, rejects unknown keys, checks that
  `allow` patterns are well formed and `auto_meld` repo specs parse, enforces that
  every `auto_meld` entry is pinned when `pinned = true` (POL-21), and that every
  `auto_meld` entry satisfies `allow` when locked (POL-31). It reports hard errors
  and advisories and exits non-zero on a hard error, mirroring source review
  (CLI-130..133).
