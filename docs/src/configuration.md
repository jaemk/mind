# Configuration

## Agent homes (lobes)

`learn` links items into every configured agent home (a *lobe*). Each item is
linked under its kind subdirectory: `skills/`, `agents/`, `rules/`. The default
lobe is `~/.claude`. Configure more in `~/.mind/config.toml`:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
```

The file is created with the default lobe (`~/.claude`) on first use. For a single
invocation, set `MIND_AGENT_HOMES` to a `:`-separated path list instead.

**Lobe precedence (STO-14):** `MIND_AGENT_HOMES` wins over `lobes` in
`config.toml`, which wins over the default `~/.claude`. An unknown key in
`config.toml` is a hard error.

Use `mind config lobes add <path>` and `mind config lobes remove <path>` to
manage lobes without hand-editing the file; see [Commands](commands.md) for the
full verb list.

## SSH cloning

To authenticate with an SSH key instead of an https username/password, meld the
`git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so the
`owner/repo` shorthand clones over SSH. An https remote still prompts (or uses a
credential helper) as git normally does.

## Config example

A single `~/.mind/config.toml` may contain both `lobes` and `ssh` together:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
ssh = true
```

## Paths

```
~/.mind/
  config.toml                   persistent settings (lobes, ssh)
  sources.json                  source registry (melded repos)
  manifest.json                 installed-item manifest and file registry
  sources/<host>/<owner>/<repo> clone of each melded repo
  store/<kind>/<name>/          installed copy of each item (name is effective)
  .tmp/staging/                 scratch for new copies during transactional installs
  .tmp/backup/                  previous copy held during a swap, for rollback
  .lock                         global advisory lock
```

Override the roots with `MIND_HOME` (the `~/.mind` tree) and `CLAUDE_HOME` (the
default lobe).

## Concurrency

A global advisory lock (`~/.mind/.lock`) is held by every mutating command
(`meld`, `unmeld`, `learn`, `forget`, `sync`, `upgrade`, `introspect --fix`,
`config lobes add|remove`). A second concurrent `mind` invocation blocks until
the first finishes. The lock is released when the holding process exits, even on
crash, so an aborted run never wedges the next one. Read-only commands (`recall`,
`probe`, `introspect`, `config show`) take a shared lock and proceed concurrently
with each other, but never observe a writer mid-update (STO-40..43).

## Install and upgrade are transactional

A failed `learn` or `upgrade` never leaves you worse off. The new copy is built
in a staging directory first; the previous version is moved to a backup and only
dropped after the swap succeeds. A failure at any point restores the previous
version from backup (LIFE-1..4).

A prefix change (adding or removing `--as <prefix>` on a source) causes `upgrade`
to report `rename old -> new` and is handled the same way: the new name is
installed before the old one is removed (LIFE-14). This is normal, not an error.

For diagnosing a failed install or broken links, see
[Troubleshooting](troubleshooting.md).
