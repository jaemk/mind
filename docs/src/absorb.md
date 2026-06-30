# absorb

`mind absorb <ref>` claims a single unmanaged lobe item (skill, agent, or rule)
into a version-controlled source and installs it as a managed item. It is the
constructive inverse of `forget --unmanaged`: instead of deleting an unmanaged
file, `absorb` moves it into a source you own, commits it there, and replaces
the lobe entry with a managed symlink. After absorb the item participates in
`sync`, `upgrade`, and `forget` like any installed item.

See [Unmanaged items](unmanaged.md) for how unmanaged items are detected and
listed.

## Basic usage

```
mind absorb skill:my-skill
mind absorb agent:my-agent
mind absorb rule:my-rule
mind absorb my-item --to /path/to/my-source
```

The ref must resolve to exactly one unmanaged item. A glob ref is an error;
bulk absorb is not supported. A kind prefix disambiguates when the same name
appears in more than one kind.

Tools are never absorbed: they are store-only and never appear as unmanaged lobe
items (spec ABS-1).

## Choosing the destination

The destination source is resolved in this precedence order:

1. `--to <path>` -- the explicit destination flag.
2. `MIND_ABSORB_TO` environment variable.
3. `absorb_to` key in `~/.mind/config.toml`.

The first one set wins; later sources are not consulted (spec ABS-2).

When none of the three is set and the run is interactive, `absorb` prompts for a
destination and offers `~/.mind/personal` as the built-in default. That directory
is created and `git init`-ed on demand if it does not already exist. After a
prompted choice, `absorb` offers to save it as `absorb_to` in `config.toml` so
future invocations skip the prompt (spec ABS-3, ABS-4).

A non-interactive (non-TTY) run with no destination configured is an error
(`ConfirmationRequired`) and changes nothing (spec ABS-3).

When `--yes` is given on a TTY and no destination is configured, `absorb`
automatically uses (and persists) `~/.mind/personal`. On a non-TTY this does not
apply: with no destination configured the run errors regardless of `--yes` (see
[Running unattended](#running-unattended) below).

## Flags

| flag | effect |
|------|--------|
| `--to <path>` | Destination source directory (highest precedence). |
| `-f` / `--force` | Overwrite the destination convention path if a `kind:name` collision already exists there. Without `--force`, a collision is an error and nothing is changed (spec ABS-6). |

The global `-y` / `--yes` flag skips the `[y/N]` confirmation described below.

## Confirmation and multi-lobe cleanup

An unmanaged item may occupy more than one lobe. `absorb` takes the content from
one occupied path; because `learn` relinks the item into every configured lobe
(spec STO-14), the other unmanaged copies must be removed so the managed link can
take their place.

Before acting, `absorb` lists what it will move and which stray copies it will
delete and prompts `[y/N]` once. This covers deleting files the user owns, so
the prompt runs even when only one lobe is affected (spec ABS-7).

- `--yes` (`-y`) skips the prompt and proceeds.
- A non-TTY run without `--yes` refuses with `ConfirmationRequired` and changes
  nothing.
- Declining the prompt leaves the original lobe entry in place and the manifest
  unchanged.

## What absorb does internally

1. Resolves `<ref>` to a single unmanaged item (ABS-1).
2. Confirms the destination source is a git repository. The `--to` /
   `MIND_ABSORB_TO` / `absorb_to` path must already be a git repo; a non-repo
   path is an error. `~/.mind/personal` is the one path `absorb` will create and
   `git init` on demand (spec ABS-5).
3. Lists the move and any stray copies to delete; prompts `[y/N]` (skipped with
   `--yes`).
4. Moves the item into the destination source at its convention path
   (`skills/<name>/`, `agents/<name>.md`, `rules/<name>.md`). Melds the source
   first if it is not yet registered.
5. Stages and commits the moved item in the destination repo with the message
   `absorb <kind>:<name>` (spec ABS-5).
6. Runs `learn` on the item; the lobe path is now a managed symlink.
7. Records the item in the manifest keyed `kind:effective-name` with the
   destination source as its source (spec ABS-8).

A failure at any step before `learn` completes leaves the original lobe entry in
place and the manifest unchanged. A failed absorb never loses the user's file
(spec ABS-10).

## After absorb

The item's effective name follows the destination source's prefix when one is in
effect (`--namespace` or `[source].prefix` in `mind.toml`). It then appears in
`mind recall`, participates in `mind upgrade` and `mind sync`, and can be removed
with `mind forget <kind:name>` like any managed item (spec ABS-8).

## Running unattended

Pass `--yes` (`-y`) to skip all prompts. In a non-TTY context without `--yes`,
`absorb` refuses at any point that would normally prompt (destination resolution
or the multi-lobe confirmation). You must also supply a destination via `--to` or
`MIND_ABSORB_TO` or `absorb_to` in config, because a missing destination in a
non-TTY context is itself an error (spec ABS-3).
