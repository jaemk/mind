# Configuration and paths

`mind` reads its settings from `~/.mind/config.toml`, created on first use. An
unknown key is an error. `mind config show` prints the file's location and the
settings in effect.

```toml
lobes = ["~/.claude", { path = "~/.gemini/config", kinds = ["skill"] }]
ssh = true
absorb-to = "~/dev/my-agent-library"
max-metadata-size = "32MiB"
hook-history = 100
hook-log = false
hook-log-keep = "all"
```

| key | meaning | see |
|-----|---------|-----|
| `lobes` | agent homes to link into, each optionally limited by `kinds` | [Use more than one agent](lobes.md) |
| `ssh` | clone the `owner/repo` shorthand over SSH | [SSH cloning](#ssh-cloning) |
| `absorb-to` | default destination for `mind absorb` | [Adopt items you placed by hand](unmanaged.md#where-it-goes) |
| `max-metadata-size` | cap on each metadata file read from a source | [Metadata size cap](#metadata-size-cap) |
| `hook-history` | how many hook results to keep (default 100; `0` for none) | [Hook results and logs](install-hooks.md#hook-results-and-logs) |
| `hook-log` | save hook output: `true` for `~/.mind/hooks/logs`, or a directory (relative to `~/.mind`) | [Hook results and logs](install-hooks.md#hook-results-and-logs) |
| `hook-log-keep` | which logs to keep when logging is on: `all` (default) or `failed` | [Hook results and logs](install-hooks.md#hook-results-and-logs) |
| `mind-homes` | extra mind homes a source may be melded into (relative to `~/.mind`); setting it restricts homes and lobes to this list | [Mind homes](#mind-homes) |
| `source-homes` | mind homes `mind` recorded for sources while `mind-homes` and `MIND_HOMES` are unset | [Mind homes](#mind-homes) |

The legacy spelling `absorb_to` is still read.

## Environment variables

| variable | effect |
|----------|--------|
| `MIND_HOME` | root of the `~/.mind` tree |
| `MIND_HOMES` | a `:`-separated list of extra mind homes, before `mind-homes` |
| `MIND_DEFAULT_LOBE` | the default lobe instead of `~/.claude` (`CLAUDE_HOME` is a legacy alias) |
| `MIND_AGENT_HOMES` | a `:`-separated list of lobes for this command, replacing `lobes` |
| `MIND_ABSORB_TO` | destination for `mind absorb`, above `absorb-to` |
| `MIND_MAX_METADATA_SIZE` | metadata size cap, above `max-metadata-size` |
| `MIND_POLICY_FILE` | a managed policy file, used only when none exists at the system path |
| `MIND_HTTP_TIMEOUT_SECS` | request budget for `mind evolve` (default 15) |
| `MIND_MAX_PARALLEL_FETCHES` | most sources `sync`/`upgrade` fetch at once (default 8; `1` fetches one at a time) |
| `MIND_PROGRESS` | `never` hides the stderr progress spinner, `always` draws it even off a terminal or in CI |
| `MIND_TTY` | decides whether hook prompts see a terminal: `0`, `false`, `no`, `off`, or empty means not a terminal, anything else means one; unset, stdin decides |
| `MIND_DETECT_HOME` | the directory `config lobes detect` scans for harness homes, instead of `~` |
| `GH_TOKEN`, `GITHUB_TOKEN` | token `evolve` sends to `api.github.com` |
| `NO_COLOR` | plain output |

`mind` sets `MIND_IN_HOOK` and the other [hook variables](install-hooks.md#hook-environment)
for the hooks it runs; they are not settings.

## SSH cloning

To authenticate with an SSH key instead of an https username/password, meld the
`git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so the
`owner/repo` shorthand clones over SSH. An https remote still prompts (or uses a
credential helper) as git normally does.

While the progress spinner is on, a clone or fetch first runs with prompts off.
When it needs a passphrase, a host-key confirmation, or credentials, it runs
again with the spinner cleared and prompts on, so the prompt appears after the
first attempt fails. `--no-progress` makes git prompt on the first attempt.

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
                                max-metadata-size, hook-history, hook-log,
                                hook-log-keep)
  hooks/history.jsonl           the newest hook results (`mind hooks history`)
  hooks/logs/<run>/             saved hook output (`--hook-log`)
  sources.json                  source registry (melded repos)
  manifest.json                 installed-item manifest and file registry
  sources/<host>/<owner>/<repo> clone of each melded repo
  store/<kind>/<name>/          installed copy of each item (name is effective)
  personal/                     built-in absorb destination, created on demand
  auto-update.json              last auto-update run and reports not yet shown
  auto-update.log               every auto-update run's moves, prunes, holds,
                                and failures
  .tmp/staging/                 scratch for new copies during transactional installs
  .tmp/backup/                  previous copy held during a swap, for rollback
  .lock                         global advisory lock
  .lock.holder                  the command holding .lock exclusively, while it runs
```

Override the roots with `MIND_HOME` (the `~/.mind` tree) and `MIND_DEFAULT_LOBE`
(the default lobe). `CLAUDE_HOME` is a legacy alias for `MIND_DEFAULT_LOBE`;
`MIND_DEFAULT_LOBE` takes precedence when both are set.

## Mind homes

A source can live in a mind home other than `~/.mind`, e.g. a directory a
sandbox allows writes to. `meld --home` names it, or the source names its own
in its `mind.toml`; `--home` outranks it:

```bash
mind meld owner/repo --home ~/dev/project/.mind
```

```toml
[source]
mind-home = "~/dev/project/.mind"
```

With no `MIND_HOMES` and no `mind-homes` config key, any directory that is
missing, empty, or already a mind home is allowed. The first time a source
is melded into one, `mind` records it under `source-homes` in
`~/.mind/config.toml`, so later runs read it, and says so in bold.

To restrict homes, list them in `MIND_HOMES` or the `mind-homes` config key.
Then only `~/.mind` and the listed homes are allowed, any other is refused,
`source-homes` is not read, and a source's lobe must lie under an allowed
home:

```bash
export MIND_HOMES=~/dev/project/.mind
```

Note that without `--home` the source is cloned into `~/.mind` before its
`mind.toml` is read, so where `~/.mind` is not writable pass `--home`.

Each home is a full tree (`sources.json`, `manifest.json`, `sources/`,
`store/`), and the sources a curator lists follow it unless they name their
own. Every command sees all homes as one set and writes each source and item
back to its own home, so working with a source in one home writes nothing in
another. `config.toml`, the hook history, the hook logs, and the auto-update state stay
in `~/.mind`. When it cannot be written, a hook record or the auto-update state
is dropped with a warning, and `review` and the `probe` preview clone into the
first home that can be written.
To move a melded source, meld it again with `--home`:

```bash
mind meld owner/repo --home ~/dev/project/.mind
```

Its clone, registry entry, and installed items move to that home, along with
the sources it curates that have no home of their own; the items are listed
and confirmed as an upgrade is. Running it again moves nothing.

`mind upgrade` does the same when a source's own `mind-home` names another
home in a new commit, so an existing install follows it: the source, its
items, and their links (into its `lobe`, if it has one) move in one pass. A
source whose home you chose with `--home` stays where you put it. A move
that cannot happen, e.g. from a home the sandbox cannot write, leaves the
source in place with a warning; run `mind upgrade` again where it can be
written.

A move reinstalls each item from the source's clone, so its path tokens
(`{{tools:}}`, `{{resources:}}`, `{{self}}`, `{{path:}}`) name the new store,
and a tool with a build hook is built again there. When that build hook
cannot run (no terminal to ask, and no `--accept-hooks=build`), the move
waits: `meld --home` is refused, `upgrade` leaves the source in place, and
`auto-update run` holds the release.
`learn <url> --home` places an [item link](item-links.md) the same
way, a curator entry can name a source's `mind-home`, and `dump` records each
source's extra home. `recall --sources` shows `home:<dir>` for a source in an
extra home, as does its details dialog in `probe`. See [spec/homes.md](https://github.com/jaemk/mind/blob/main/spec/homes.md).

## Source lobes

A source can link its items into a lobe of its own instead of `~/.claude`, so
`mind upgrade` can keep it current from a sandbox that cannot write
`~/.claude`. The source declares it in its `mind.toml`, or the consumer names
it with `--lobe`, which outranks the source's:

```toml
[source]
mind-home = "~/dev/project/.mind"
lobe = "claude"                     # ~/dev/project/.mind/claude
```

```bash
mind meld owner/repo --home ~/dev/project/.mind --lobe claude
```

Its items link into `~/dev/project/.mind/claude/skills/<name>` and so on;
every other lobe, project lobes included, still gets its links. Point the
agent at that directory yourself, e.g. from your `CLAUDE.md`. A relative lobe
is taken against the source's mind home and moves with it. When extra mind
homes are set (`MIND_HOMES` or `mind-homes`), the lobe must lie under one of
them; with none set it may be any directory. Either way the install
confirmation names the lobe in bold, and says so when it is outside every
mind home. `upgrade` moves the links when a new commit adds, changes, or
drops the source's lobe, and holds back a source whose new lobe is not
allowed while upgrading the rest; `meld --lobe <dir>` on a melded source moves them
at once, and `--lobe ''` clears the consumer's lobe. `learn <url> --lobe`
does the same for an [item link](item-links.md). `recall --sources` shows
`lobe:<dir>` for a source with a lobe in effect (`lobe_path` under `--json`).

For every source, a lobe that cannot be written (permission denied, a
read-only filesystem) is skipped with a note instead of failing the command.
A link already in place there is left alone, and a run that can write there
later brings it up to date (`upgrade`, or `introspect --fix` for a missing
link). See [spec/source-lobes.md](https://github.com/jaemk/mind/blob/main/spec/source-lobes.md).

## Concurrency

A global advisory lock (`~/.mind/.lock`, plus each extra mind home's `.lock`) is held by every mutating command
(`meld`, `unmeld`, `learn`, `forget`, `sync`, `upgrade`, `introspect --fix`,
`config lobes add|remove`, `config auto-update`). A second concurrent `mind`
invocation blocks until the first finishes, except `auto-update run`, which
exits without doing anything when the lock is held. The lock is released when the holding process exits, even on
crash, so an aborted run never wedges the next one. Read-only commands (`recall`,
`probe`, `introspect`, `config show`) take a shared lock and proceed concurrently
with each other, but never observe a writer mid-update.

A command holding the lock exclusively records itself in `.lock.holder` next
to the lock (pid, command line with any URL credentials removed, working
directory, start time) and removes it before letting go. A blocked command
names it in its progress line:

```
waiting for another mind command to finish: mind upgrade --yes (pid 4242, running 1m15s)
```

The progress line shows only on a terminal (see `MIND_PROGRESS`), so in a
script a blocked command waits silently; read `.lock.holder` to see what it
waits on. A command holding the lock shared records nothing, so waiting
behind one shows the bare message. A hook runs while its `mind` holds the
lock, so a hook cannot run `mind`; see
[Hook environment](install-hooks.md#hook-environment).

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
