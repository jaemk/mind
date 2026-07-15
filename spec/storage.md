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
  .lock                         global advisory lock (STO-40)

<agent home>/                   (one or more; default ~/.claude)
  skills/<name>      -> store/skill/<name>
  agents/<name>.md   -> store/agent/<name>
  rules/<name>.md    -> store/rule/<name>
```

- `STO-1` The mind root is `$MIND_HOME` if set, else `~/.mind`. The claude root is
  `$CLAUDE_HOME` if set, else `~/.claude`. Both overrides are honored everywhere.
- `STO-2` The default link target for an item, relative to an agent home, is
  `skills/<name>` (skill), `agents/<name>.md` (agent), or `rules/<name>.md`
  (rule), where `<name>` is the effective name. A tool has no default link
  target: it is store-only (tooling.md TOOL-3). A `mind.toml` item may override
  the link target (applied in every home), which is how a tool opts into a link.
  Note: the `skills/<name>` and `agents/<name>.md` layouts are also the
  cross-tool conventions for Gemini CLI, Codex CLI, and Antigravity, so mind
  links into those homes with no content transform (HARN-3; see
  harness-lobes.md). A lobe may carry a `kinds` filter limiting which item kinds
  are linked into it (HARN-1); rules are not linked into non-Claude preset lobes
  (HARN-3).
- `STO-3` Store and link paths use the effective name, so namespaced items do not
  collide with same-named items from other sources.
- `STO-14` The agent homes ("lobes") items are linked into are, in order:
  `$MIND_AGENT_HOMES` (a `:`-separated path list), else `lobes` in
  `~/.mind/config.toml`, else `[claude root]`. A leading `~` is expanded. An
  unknown key in `config.toml` is an error (`Toml`).
  Note: each lobe entry may carry a `kinds` filter (HARN-1). Non-Claude lobe
  presets (gemini, codex, universal, windsurf) are added via
  `config lobes add --preset <name>` (HARN-4) or the auto-detect-and-prompt path
  `config lobes detect` (HARN-5). See harness-lobes.md for the preset path table
  and per-harness `kinds` defaults.
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
- `STO-17` A source records an optional `roots`: the consumer `--root` override
  (repo-root-relative directories, see DSC-51). Persisted at meld and not changed
  by `sync`. Absent means convention discovery uses `[source].roots` or the repo
  root (DSC-50).
- `STO-55` A source records an optional `add_roots`: the consumer `--add-root`
  roots (repo-root-relative directories, see DSC-84) that compose with the
  source's authoritative discovery layer. Persisted at meld and not changed by
  `sync`. Absent means no additional roots.
- `STO-44` A source records an optional `flat_skills` boolean: the consumer
  `--flat-skills` override (see DSC-75). Persisted at meld and not changed by
  `sync`. Absent or false means convention discovery uses `[source].flat-skills`
  or the `skills/` container (DSC-74).
- `STO-18` A source records its `pin`: the kind (`follow-branch` | `tag` | `ref`)
  and value (see DSC-41, CLI-17). Persisted at meld and not changed by `sync`. The
  implicit default when unset is `follow-branch` tracking the remote default
  branch.

## Manifest (manifest.json)

- `STO-20` The manifest maps `kind:effective_name` to an installed item.
- `STO-21` Each installed item records: `kind`, `name` (effective), `bare_name`,
  `source`, `commit`, `hash` (of source content), `store` (path relative to the
  mind root), `links` (absolute symlink paths, one per agent home; a relative
  lobe is resolved to absolute first, see STO-16), `description`.
- `STO-22` `(source, kind, bare_name)` is the item's stable identity (see
  lifecycle.md). `store` and `links` are its file registry, used by uninstall.
- `STO-23` A missing manifest file is treated as empty.

## Concurrency and durability

mind may be invoked from more than one process at once. State stays consistent
through a single global advisory lock plus atomic file writes; together these
prevent the lost-update and torn-read races a plain read-modify-write would allow.

- `STO-40` A single advisory lock file at `<mind root>/.lock` guards all access to
  mind's persisted state (`sources.json`, `manifest.json`, the store, the links,
  and `config.toml`). A command acquires the lock before it reads state and holds
  it until the command completes, so a mutating command's read-modify-write cycle
  is never interleaved with another process's (and two installs of the same item
  cannot share the `.tmp/staging|backup` scratch). The lock lives under the mind
  root, so a `MIND_HOME` override (e.g. a test's temp home) gets its own isolated
  lock.
- `STO-41` The lock is acquired exclusively by mutating commands (`meld`, `unmeld`,
  `learn`, `forget`, `sync`, `upgrade`, `introspect --fix`, `config lobes add` /
  `remove`) and shared by read-only commands (`recall`, `probe`, `introspect`,
  `config show`). An exclusive holder excludes all others; multiple shared readers
  proceed concurrently but never observe a writer mid-update, so each reader gets a
  consistent cross-file snapshot of the registry and manifest. First-use creation
  of the default `config.toml` (STO-15) is idempotent and written atomically
  (STO-43), so it is safe even when triggered from a shared-lock command.
- `STO-42` Lock acquisition blocks until the lock is available. The lock is
  advisory (it constrains only mind, which always takes it) and is released when
  the holding process exits, including on crash, so an aborted run never wedges the
  next one. A failure to create or lock the file is an `Io` error carrying the lock
  path.
- `STO-43` `sources.json`, `manifest.json`, and `config.toml` are written
  atomically: the new contents are written to a temporary file in the same
  directory and renamed over the target (an atomic replace within one filesystem).
  A reader therefore sees either the old file or the new file, never a partial one,
  and a crash mid-write leaves the previous file intact. This holds independently
  of the lock, so it protects even a lock-less reader.

## `evolve` binary swap

- `STO-45` `evolve` stages the replacement binary in the same directory as the
  running executable under a unique name `.mind-update.<pid>.<nanos>` rather
  than a fixed name. If the staged path already exists before the copy begins,
  `evolve` refuses and returns an `Io` error, preventing a pre-creation race.
- `STO-46` `evolve` holds the global exclusive lock (STO-40) for the entire
  download-and-swap step, serializing concurrent `mind evolve` invocations so
  two processes cannot download and swap over each other.
- `STO-47` Before extracting a release archive, `evolve` downloads the
  `SHA256SUMS` asset for that release and verifies the archive's SHA-256
  digest. The `SHA256SUMS` format is standard `sha256sum` output: lowercase hex
  digest, two spaces, bare filename, one line per file. A digest mismatch, or a
  sums file that has no entry for the archive, is a `DigestMismatch` error and
  the archive is not extracted. Version-pinned `evolve` (`--version V`) verifies
  the pinned release's `SHA256SUMS`.
- `STO-48` `evolve` takes NO outer command lock (its `lock_mode` is `None`). It
  acquires the global exclusive lock itself inside the download-and-swap step
  (STO-46), only after the network-free decision/prompt phase, and `evolve
  --check` takes no lock at all. Classifying `evolve` as an outer exclusive
  command would deadlock: the outer guard holds the lock on one fd and the inner
  step then blocks forever acquiring the same lock on a second fd (flock contends
  across two fds in one process, per STO-41/STO-42). This keeps the lock window
  tight and is the fix for the 0.13.0 self-deadlock regression.

## Network fetch timeouts

- `STO-52` The network fetches in `evolve` (`fetch_to_string` and `fetch_to_file`)
  use a configurable connect timeout (default 15 s, overridable by the
  `MIND_HTTP_TIMEOUT_SECS` environment variable) and a generous max-time ceiling of
  600 s to accommodate slow downloads. For curl, the flags are `--connect-timeout N
  --max-time 600`; for wget, `--timeout=N`. A missing, non-numeric, or zero value
  of `MIND_HTTP_TIMEOUT_SECS` falls back to 15 (zero means "no limit" in both curl
  and wget, which defeats the purpose of the knob). The argument vectors are built
  by pure helper functions (`curl_string_args`, `wget_string_args`,
  `curl_file_args`, `wget_file_args`) and are unit-testable without spawning a
  process. `resources/install.sh` applies the same flags with a fixed 15 s connect
  timeout and 600 s max-time; it does not read `MIND_HTTP_TIMEOUT_SECS`, because it
  runs before `mind` is installed.
- `STO-53` All wget invocations in `evolve` and `resources/install.sh` pass
  `--tries=1`. wget defaults to 20 retries, so without this flag a blackholed
  endpoint can take up to 20 times the configured timeout before failing. curl is
  already a single attempt bounded by `--max-time 600`.
- `STO-54` curl/wget failure output (stderr captured by `fetch_to_string`) is
  sanitized via `strip_ansi` before it is embedded in `DownloadFailed.reason`.
  A MITM'd or hostile endpoint controls stderr bytes and can inject ANSI escape
  sequences or Unicode bidi override characters to spoof terminal output. The
  sanitization is applied before the proxy-hint logic so the reason field and any
  appended hint are both free of hostile control sequences.

## Schema versions

- `STO-50` Both `sources.json` and `manifest.json` carry a top-level `"version"`
  field with value `1`. A reader that finds a version greater than `1` fails with
  a `StateTooNew` error rather than silently misinterpreting the file. A missing
  `"version"` field is treated as `1` (backward compatibility with files written
  before this field existed).
- `STO-51` A `StateTooNew` error names the file (`"sources.json"` or
  `"manifest.json"`), the version found, and the highest version supported, and
  advises the user to upgrade mind.

## Errors

- `STO-30` Filesystem failures carry the offending path (`Io { path, source }`).
- `STO-31` Malformed `sources.json` or `manifest.json` is a `Json` error naming
  the file.
