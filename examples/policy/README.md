# Managed policy example

A managed `policy.toml` (see [policy.toml](policy.toml)) that restricts a `mind`
client to trusted sources and locks related settings. Modeled on Claude Code
managed settings: an admin-controlled file at a fixed system path that the user
cannot edit. See [../../spec/policy.md](../../spec/policy.md) for the normative
rules.

## Deploy

Push the file out of band (MDM, Ansible, Intune) to the fixed per-OS path, owned
by an administrator and world-readable but not user-writable:

```
Linux:   /etc/mind/policy.toml
macOS:   /Library/Application Support/mind/policy.toml
Windows: %PROGRAMDATA%\mind\policy.toml
```

The file permission is what enforces the lock: the user can read the policy but
not change it. With no policy file present, `mind` is unmanaged and every control
is off.

This is a compliance guardrail, not a security sandbox: a user can still place
agent files under an agent home directly without `mind`. What it provides is a
policy the user cannot edit, refusal of disallowed operations through `mind`, and
auditability.

## What this example sets

- `[sources].allow` + `lock`: only sources under `github.com/acme/*` or
  `github.example.com/platform/*` may be melded; anything else is refused.
- `[sources].pinned`: every meld must resolve to a tag or ref (no floating
  branches), so what installs is reproducible.
- `[[sources.auto_meld]]`: two org sources are provisioned automatically on
  `sync`, each at a fixed tag/ref. `repo` is a repo spec; its identity must be in
  `allow`.
- `[lobes]` lock + targets: the agent homes are pinned to `~/.claude` and
  `config lobes` edits are refused.

## Validate

Check a policy before deploying it (this runs the same validation `mind` applies
at load, but as a report):

```
mind review --policy examples/policy/policy.toml
```

It rejects unknown keys, an `auto_meld` entry left unpinned while `pinned = true`,
and an `auto_meld` source outside `allow` while `lock = true`.

### Make it fail

To see a rejection, remove the `tag` field from the first `auto_meld` entry so
it has no pin:

```toml
[[sources.auto_meld]]
repo = "acme/agent-baseline"
# tag = "v1.4.0"   <- removed
```

Re-run validate:

```
mind review --policy examples/policy/policy.toml
```

Expected output (exit non-zero):

```
error: auto_meld entry "acme/agent-baseline" has no pin (tag or ref) but pinned = true
```

Restore `tag = "v1.4.0"` before committing; `tests/cli.rs::example_policy_validates`
asserts the file validates clean.

## Verified

`tests/cli.rs::example_policy_validates` runs `mind review --policy` against this
file and asserts it validates clean, so the example stays correct as the code
changes.
