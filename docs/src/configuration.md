# Configuration and paths

`mind` reads its settings from `~/.mind/config.toml`, created on first use. An
unknown key is an error. `mind config show` prints the file's location and the
settings in effect.

```toml
lobes = ["~/.claude", { path = "~/.gemini/config", kinds = ["skill"] }]
ssh = true
absorb-to = "~/dev/my-agent-library"
max-metadata-size = "32MiB"
```

| key | meaning | see |
|-----|---------|-----|
| `lobes` | agent homes to link into, each optionally limited by `kinds` | [Use more than one agent](lobes.md) |
| `ssh` | clone the `owner/repo` shorthand over SSH | [SSH cloning](#ssh-cloning) |
| `absorb-to` | default destination for `mind absorb` | [Adopt items you placed by hand](unmanaged.md#where-it-goes) |
| `max-metadata-size` | cap on each metadata file read from a source | [Metadata size cap](#metadata-size-cap) |

The legacy spelling `absorb_to` is still read.

## Environment variables

| variable | effect |
|----------|--------|
| `MIND_HOME` | root of the `~/.mind` tree |
| `MIND_DEFAULT_LOBE` | the default lobe instead of `~/.claude` (`CLAUDE_HOME` is a legacy alias) |
| `MIND_AGENT_HOMES` | a `:`-separated list of lobes for this command, replacing `lobes` |
| `MIND_ABSORB_TO` | destination for `mind absorb`, above `absorb-to` |
| `MIND_MAX_METADATA_SIZE` | metadata size cap, above `max-metadata-size` |
| `MIND_POLICY_FILE` | a managed policy file, used only when none exists at the system path |
| `MIND_HTTP_TIMEOUT_SECS` | request budget for `mind evolve` (default 15) |
| `GH_TOKEN`, `GITHUB_TOKEN` | token `evolve` sends to `api.github.com` |
| `NO_COLOR` | plain output |

## SSH cloning

To authenticate with an SSH key instead of an https username/password, meld the
`git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so the
`owner/repo` shorthand clones over SSH. An https remote still prompts (or uses a
credential helper) as git normally does.

## Metadata size cap

Every metadata file `mind` reads from a source is size-capped: a `mind.toml`, an
item's frontmatter, a plugin or marketplace manifest, and a workflow's `meta`
object (which is its whole `.js` body). The default is 8 MiB, and a file past it
is refused rather than read.

Raise it for a source with a legitimately large metadata file, or lower it to
bound how much a source you do not trust can make `mind` allocate while scanning
it:

```bash
mind meld owner/repo --max-metadata-size 32MiB
```

```toml
max-metadata-size = "32MiB"
```

The flag outranks `MIND_MAX_METADATA_SIZE`, which outranks the config key. A
value is a byte count (`16777216`), a binary size (`32MiB`, `512KiB`, `2GiB`, or
the bare `32M`/`512K`/`2G`), a decimal size (`16MB` = 16000000), an explicit
`512B`, or `unlimited`, `none`, or any zero value (`0`, `0B`, `0MiB`) for no
ceiling. Case and internal whitespace are not significant. A fractional size
(`1.5MiB`) is refused rather than rounded; write a whole number of a smaller
unit (`1536KiB`). `mind config show` reports the cap in force.

An unparseable value is an error naming which of the three origins supplied it,
never a silent fall back to the next one down. That includes a
`MIND_MAX_METADATA_SIZE` whose value is not valid UTF-8; only an unset, empty,
or whitespace-only variable reads as unset. Only the origin that wins the
precedence above is read at all, so a bad `MIND_MAX_METADATA_SIZE` (undecodable
or not) is ignored on a run that passes `--max-metadata-size`.

Only the workflow kind treats an over-cap file as recoverable: it lists with no
description and installs, as any workflow whose `meta` cannot be read does. For
every other kind an over-cap file fails the scan of its source.

## Paths

```
~/.mind/
  config.toml                   persistent settings (lobes, ssh, absorb-to,
                                max-metadata-size)
  sources.json                  source registry (melded repos)
  manifest.json                 installed-item manifest and file registry
  sources/<host>/<owner>/<repo> clone of each melded repo
  store/<kind>/<name>/          installed copy of each item (name is effective)
  personal/                     built-in absorb destination, created on demand
  .tmp/staging/                 scratch for new copies during transactional installs
  .tmp/backup/                  previous copy held during a swap, for rollback
  .lock                         global advisory lock
```

Override the roots with `MIND_HOME` (the `~/.mind` tree) and `MIND_DEFAULT_LOBE`
(the default lobe). `CLAUDE_HOME` is a legacy alias for `MIND_DEFAULT_LOBE`;
`MIND_DEFAULT_LOBE` takes precedence when both are set.

## Concurrency

A global advisory lock (`~/.mind/.lock`) is held by every mutating command
(`meld`, `unmeld`, `learn`, `forget`, `sync`, `upgrade`, `introspect --fix`,
`config lobes add|remove`). A second concurrent `mind` invocation blocks until
the first finishes. The lock is released when the holding process exits, even on
crash, so an aborted run never wedges the next one. Read-only commands (`recall`,
`probe`, `introspect`, `config show`) take a shared lock and proceed concurrently
with each other, but never observe a writer mid-update.

## Install and upgrade are transactional

A failed `learn` or `upgrade` never leaves you worse off. The new copy is built
in a staging directory first; the previous version is moved to a backup and only
dropped after the swap succeeds. A failure at any point restores the previous
version from backup.

When an installed item's effective name changes, `upgrade` reports
`rename old -> new` and handles it the same way: the new name is installed
before the old one is removed. A different `--namespace` on an already-melded
repo does not rename anything; it registers a separate instance (see
[Fix name collisions](collisions.md)).

For diagnosing a failed install or broken links, see
[Troubleshooting](troubleshooting.md).
