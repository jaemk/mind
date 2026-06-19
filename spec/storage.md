# Storage

The on-disk layout and the two persisted JSON files.

## Layout

```
~/.mind/
  sources.json                  source registry
  manifest.json                 installed-item manifest
  sources/<host>/<owner>/<repo> clone of each melded repo
  store/<kind>/<name>/          installed copy of each item (name is effective)
  .tmp/staging|backup/...        scratch for transactional installs

<agent home>/                   (one or more; default ~/.claude)
  skills/<name>      -> store/skill/<name>
  agents/<name>.md   -> store/agent/<name>
  rules/<name>.md    -> store/rule/<name>
```

- `STO-1` The mind root is `$MIND_HOME` if set, else `~/.mind`. The claude root is
  `$CLAUDE_HOME` if set, else `~/.claude`. Both overrides are honored everywhere.
- `STO-2` The default link target for an item, relative to an agent home, is
  `skills/<name>` (skill), `agents/<name>.md` (agent), or `rules/<name>.md`
  (rule), where `<name>` is the effective name. A `mind.toml` item may override
  the link target (applied in every home).
- `STO-3` Store and link paths use the effective name, so namespaced items do not
  collide with same-named items from other sources.
- `STO-14` The agent homes ("lobes") items are linked into are, in order:
  `$MIND_AGENT_HOMES` (a `:`-separated path list), else `lobes` in
  `~/.mind/config.toml`, else `[claude root]`. A leading `~` is expanded. An
  unknown key in `config.toml` is an error (`Toml`).
- `STO-15` When `~/.mind/config.toml` does not exist, it is created with the
  default lobe (the `$CLAUDE_HOME` override if set, else `~/.claude`) on first
  use (any command that sets up the layout, or any `config` command).
- `STO-16` An agent home given as a relative path (after `~` expansion) is
  resolved to an absolute path against the current directory before items are
  linked, so the link paths recorded in the manifest do not depend on the
  working directory at a later command (e.g. an `uninstall` run elsewhere).

## Source registry (sources.json)

- `STO-10` Each source records: `name`, `url`, `host`, `owner`, `repo`, `commit`
  (last synced, or absent), `description` (from `mind.toml`, optional), `alias`
  (consumer `--as`, optional).
- `STO-11` A source's clone lives at `sources/<host>/<owner>/<repo>`. For local or
  `file://` specs, host is `local` and owner is the path's parent directory.
- `STO-12` A missing registry file is treated as an empty registry.
- `STO-13` A source's identity is its `name`, `host/owner/repo` (equal to its
  clone path under `sources/`). Repos that share a basename, or even an
  `owner/repo` across different hosts, are distinct sources and coexist in one
  registry.

## Manifest (manifest.json)

- `STO-20` The manifest maps `kind:effective_name` to an installed item.
- `STO-21` Each installed item records: `kind`, `name` (effective), `bare_name`,
  `source`, `commit`, `hash` (of source content), `store` (path relative to the
  mind root), `links` (absolute symlink paths, one per agent home; a relative
  lobe is resolved to absolute first, see STO-16), `description`.
- `STO-22` `(source, kind, bare_name)` is the item's stable identity (see
  lifecycle.md). `store` and `links` are its file registry, used by uninstall.
- `STO-23` A missing manifest file is treated as empty.

## Errors

- `STO-30` Filesystem failures carry the offending path (`Io { path, source }`).
- `STO-31` Malformed `sources.json` or `manifest.json` is a `Json` error naming
  the file.
