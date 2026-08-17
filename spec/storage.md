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
  `$MIND_DEFAULT_LOBE` if set, else `$CLAUDE_HOME` if set, else `~/.claude`
  (`$MIND_DEFAULT_LOBE` takes precedence over `$CLAUDE_HOME`, CLI-170). All
  overrides are honored everywhere.
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
  Note: a lobe may be a global agent home directory (e.g. `~/.gemini/config`) or
  a project-scoped subdirectory (e.g. `<project>/.windsurf`); both are unified as
  "any install-target directory". See STO-56 for the reachability gate that
  governs project-scoped lobes.
- `STO-15` When `~/.mind/config.toml` does not exist, it is created with the
  default lobe (the `$MIND_DEFAULT_LOBE` override if set, else the `$CLAUDE_HOME`
  override if set, else `~/.claude`; CLI-170) on first use (any command that sets
  up the layout, or any `config` command).
- `STO-16` An agent home given as a relative path (after `~` expansion) is
  resolved to an absolute path against the current directory before items are
  linked, so the link paths recorded in the manifest do not depend on the
  working directory at a later command (e.g. an `uninstall` run elsewhere).

## Source registry (sources.json)

- `STO-10` Each source records: `name`, `url`, `host`, `owner`, `repo`, `commit`
  (last synced, or absent), `description` (from `mind.toml`, optional), `alias`
  (the effective display prefix - `--as`, a curated `as`/`namespace`, a
  marketplace entry name, an accepted `[source].prefix`, or a collision prompt;
  optional), and `as_alias` (the identity alias, STO-58: the subset of `alias`
  known before the clone; optional, absent for a bare meld or a display-only
  prefix).
- `STO-11` A source's clone lives at `sources/<host>/<owner>/<repo>`. For local or
  `file://` specs, host is `local` and owner is the path's parent directory.
- `STO-12` A missing registry file is treated as an empty registry.
- `STO-68` `Registry::load` revalidates every entry it reads, not just its
  schema version (STO-50): each entry's `host`/`owner`/`repo` are re-checked
  with the same per-part rules `make_source` applies at parse time (CLI-204),
  its `as_alias` (when present) is re-checked with `validate_prefix` (NS-25,
  NS-28, NS-29), and its pin value (when the pin carries one - `follow-branch`,
  `tag`, or `ref`; `default-branch` has none) is re-checked with
  `git::validate_ref_value` (DSC-66). This closes the gap a schema-version
  check alone leaves open: an entry can be structurally valid JSON at the
  current schema version while carrying a value an older, looser `parse_spec`
  accepted and a newer one would refuse (e.g. 0.21.0's parser accepted
  `repo: ".."`). A failing entry is DROPPED from the in-memory registry, not
  treated as a load error: hard-erroring would brick every `mind` verb for a
  user who happens to be carrying such a stale entry, whereas dropping it
  degrades gracefully to "that one source is gone, everything else still
  works." Each drop prints one warning to stderr naming `sources.json`, the
  dropped entry's `name`, and which part failed. The drop is not written back
  immediately; it becomes permanent the next time `Registry::save` runs (e.g.
  as part of the same command's normal write-back), same as any other
  in-memory registry mutation.
- `STO-13` A source's identity is its `name`, `host/owner/repo` (equal to its
  clone path under `sources/`, absent an alias or item-link suffix; see STO-58).
  Repos that share a basename, or even an `owner/repo` across different hosts,
  are distinct sources and coexist in one registry.
- `STO-58` The *identity alias* is part of a source's identity. It is the
  namespace declared BEFORE the clone: the consumer `--as <alias>` (STO-10), a
  curated `[discover].sources` entry's `as`/`namespace` (DSC-78), or a marketplace
  entry name (MKT-8). A source with an identity alias carries a trailing
  `@<alias>` segment on its `name` (`host/owner/repo@<alias>`), composing with an
  item-link `#<path>` suffix (LNK-4) as `host/owner/repo#<path>@<alias>` (`@<alias>`
  is always last). So a bare meld of a repo, and one or more identity-aliased melds
  of the same repo under distinct aliases, are distinct registry entries that
  coexist with independent pins, commits, clones, and lifecycles - the mechanism a
  consumer uses to track several branches or versions of one source side by side,
  each installing its items under its own prefix. Two melds that resolve to the
  same `host/owner/repo@<alias>` identity are the same instance (a re-meld, CLI-12).
  A prefix decided AFTER the clone - an accepted `[source].prefix` (CLI-24) or a
  collision-resolved prefix (NS-44) - is a display prefix only (STO-10 `alias`),
  NOT an identity alias: it changes the effective item names but leaves the
  identity and clone at the bare `host/owner/repo`. Managed-policy allowlist
  matching and the compare/browse URLs use the base `host/owner/repo` identity
  (POL-11, LNK-11, CLI-176), never the `@<alias>` form. A source recorded before
  the identity alias existed has only a display `alias` and no identity alias, so
  it keeps its bare identity (no migration, no clone relocation, no change to its
  manifest `source` linkage).
- `STO-59` A source with an identity alias (STO-58) clones at
  `sources/<host>/<owner>/<repo>@<alias>` (the alias is a single safe path
  component, NS-28, so the leaf is filesystem-safe), giving each instance an
  independent checkout so instances pinned to different branches do not share a
  working tree. Because the identity alias is known before the clone, the clone
  lands at this path directly. A source with no identity alias AND no item_path -
  a bare meld, or one whose only prefix is a post-clone display prefix - clones at
  `sources/<host>/<owner>/<repo>` (STO-11) and is the only case that can ever
  share a checkout with another instance (see STO-70 for the item-path leg,
  which also contributes to the leaf and therefore also gets an independent
  checkout).
- `STO-69` A destructive or clone-then-use filesystem operation on a source's
  clone path (`Source::clone_dir`) - a fresh clone, a re-clone that first
  removes the existing directory, or `unmeld`'s cleanup - refuses to proceed
  when the resolved path escapes the managed sources tree: either it contains a
  `ParentDir` (`..`) path component, or, after that check, it does not start
  with `paths.sources_dir()`. The refusal is a structured error naming the
  offending path and the source's identity, raised before any filesystem
  mutation is attempted at that path. This guards a hand-edited or corrupted
  `sources.json` entry (e.g. a `host`/`owner`/`repo` part containing `..`) from
  making `mind` write or delete content outside `~/.mind/sources` - the
  registry-level counterpart to `STO-68`'s revalidation, for the specific
  destructive operations that touch disk at the clone path. The check is
  **lexical** (component/prefix inspection of the path string), not a
  filesystem-resolved (canonicalized) check: a symlink planted inside the
  sources tree that points outside it is therefore out of scope of this check
  and is not caught by it. A linked local source (`Source::is_linked`) is
  exempt: its "clone dir" is the user's own working tree by design (CLI-27),
  not a path under the sources tree, so confining it here would be both
  incorrect and unnecessary.
- `STO-70` The clone-dir leaf (STO-11, STO-59) also incorporates a source's
  `item_path` (LNK-4) when it has one, not just its identity alias, so that an
  item-link instance always gets an independent checkout: two item-link
  instances into the same repo at different paths, and a plain (non-link) meld
  of that repo, previously all resolved to the identical
  `sources/<host>/<owner>/<repo>` clone path, so re-melding a second link
  deleted-and-reprovisioned the first link's (or the plain meld's) checkout out
  from under it, and `unmeld` of either broke every other instance sharing that
  clone. The full leaf formula, combining STO-59's alias suffix with the
  item-path segment:
  - `<repo>` - no item_path, no alias (STO-11, unchanged).
  - `<repo>@<alias>` - alias only (STO-59, unchanged).
  - `<repo>#<enc>` - item_path only.
  - `<repo>#<enc>@<alias>` - both.

  `<enc>` percent-encodes `item_path`: `%` to `%25` first, then `/` to `%2F`.
  This is injective (so two distinct item_paths can never collide on the same
  encoded segment) and stays human-readable in a directory listing; no other
  character needs escaping because an item_path can never itself contain `@`,
  `#`, a `..` component, or NUL (LNK-10, LNK-16). If the encoded leaf would
  exceed 120 bytes, the encoded segment is replaced with the 16-hex FNV of
  `item_path` (`hash::hash_str`) instead, so an unusually deep or long skill
  path still produces a short, filesystem-safe, and still-deterministic leaf.

  **Migration.** Existing clones are never relocated to match the new formula:
  doing so could steal the shared directory out from under whichever registry
  entry legitimately still owns `sources/<host>/<owner>/<repo>` (a plain,
  non-link meld of the same repo, if one is registered). Instead, a source
  instance registered before this change simply now resolves to a leaf that
  has never been cloned there; the existing `.git`-absent branches already
  present in `sync` and `upgrade` re-clone it fresh at the new, isolated path,
  and `introspect` reports it as no-clone in the interim (exactly as it would
  for a source that had never synced). The old shared directory itself is not
  cleaned up by this change; it stays owned by the bare (non-aliased,
  non-linked) instance if one is registered at that identity, or is simply
  orphaned disk usage if every instance that used to share it was a link
  instance (an `introspect`/manual cleanup concern, not something this change
  addresses).
- `STO-60` When a `meld` forks a NEW identity-aliased instance (STO-58) of a repo
  that already has one or more melded instances (any registry entry sharing the
  same base `host/owner/repo`), `meld` prints an explicit one-line note that a new
  instance was registered and the existing one remains, in addition to the
  `melded <name>@<alias>` line. The trailing `@<alias>` is otherwise the only
  signal that a coexisting instance (a second clone, STO-59) was created rather
  than an existing source's display prefix being changed. Suppressed under
  `--json`.
- `STO-63` The STO-60 note names the actual registered `name`(s) of the
  pre-existing instance(s) sharing the base identity, not the bare
  `host/owner/repo` itself. The bare base identity is not necessarily a
  resolvable source: if every pre-existing instance carries an identity alias
  (e.g. melding a repo first as `@a` then as `@b`, with no bare instance ever
  registered), the bare base never appears in the registry and naming it would
  point at a name `unmeld` (or any other verb) cannot resolve. With exactly one
  pre-existing instance the note reads "the existing `<name>` remains"; with more
  than one it reads "the existing instances `<name1>, <name2>` remain".
- `STO-64` `@`/`#` legality in `host`/`owner`/`repo` (CLI-204) is per-part, not a
  blanket rule, because each character's danger is a specific downstream
  collision, not general hygiene:
  - `repo` rejects both `@` and `#`. The identity alias suffix (STO-58) and the
    per-instance clone-dir leaf (STO-59) both append `@<alias>` directly to
    `repo`, and the item-link marker (LNK-4) appends `#<path>` the same way. A
    `repo` value that itself contains `@` or `#` can therefore coincide with
    the identity AND the clone path of a genuinely different, unrelated
    instance: a repo directory literally named `foo@bar` (melded unaliased)
    and repo `foo` melded with an identity alias `bar` (`meld --as bar`) would
    both build the identity `host/owner/foo@bar` and clone at the same
    `foo@bar` leaf, so the second meld would be read back as a re-meld of the
    first (CLI-12) and the two would share one clone on disk.
  - `owner` rejects `#` (new) but keeps `@` legal. `#` in `owner` would place
    the item-link marker before the repo segment, confusing `#`-splitting in
    item refs (`owner/repo#name`) and hook targets (CLI-194). `@` collides
    with nothing in `owner`: the `@<alias>` suffix (STO-58) only ever appends
    to `repo`, never `owner`, so an owner directory legitimately named with an
    `@` (e.g. `/src/proj@v2/agents`) stays admitted.
  - `host` rejects both, unchanged from CLI-204's original statement.
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
- `STO-67` `ssh://` userinfo handling in the URL form of `parse_spec` (CLI-19):
  the authority (the segment between `scheme://` and the first `/`) may carry a
  userinfo prefix, `user@host`. It is split on the LAST `@`, not the first, so a
  host containing no `@` of its own splits unambiguously. The part after the
  split is the identity `host` (validated with the same rules as any other host,
  CLI-204) and feeds the source's identity (`host/owner/repo`, STO-13) and clone
  path (STO-11); the FULL authority, userinfo included, is what is written into
  `url`, so `git clone` still authenticates as that user. This applies only to
  scheme `ssh`. Any other scheme (`http`, `https`, and anything else) refuses an
  embedded userinfo outright as `UnsafeRepoSpec` (part `host`, value the full
  authority, reason naming the credential): an http(s) credential in the URL
  would otherwise be persisted verbatim into `sources.json` and echoed back by
  `dump`, unlike an ssh userinfo which is never itself a secret (SSH auth is
  keyed, not password-in-URL). The userinfo itself is validated for empty value
  and control characters before it is trusted into `url`, since the whole
  authority reaches `git clone` unescaped. An authority with no `@` at all (the
  ordinary `ssh://host/owner/repo` form) is unaffected: it parses exactly as
  before this rule existed.
- `STO-72` A local-path repo spec (`/abs/path`, `./rel/path`, `../rel/path`, or
  the local branch of a `file://` item link) is resolved to an absolute path
  BEFORE `owner`/`repo` are derived from it, and that absolute form - not the
  spec as typed - is what is persisted into `Source.url`. Resolution joins the
  process's current working directory onto a relative spec, then resolves
  `.`/`..` path components LEXICALLY (string manipulation only: no
  `canonicalize`, no filesystem stat beyond reading the cwd itself), so
  `parse_spec` stays stat-free and still works for a path that does not exist
  yet. Without this, a relative spec's literal `.`/`..` segment reached the
  owner-directory derivation (which filters `.` and `..`, falling back to the
  `local` placeholder), so `./foo` produced the wrong `local/local/foo`
  identity instead of naming the real parent directory as owner; and the
  literal relative string, recorded verbatim into `url`, stopped resolving to
  the melded directory after any later `cd` (the clone path for a linked local
  source, CLI-27, is `url` itself). Because resolution happens first, an
  input like `/a/b/..` never reaches identity derivation carrying a literal
  `..` segment at all: it resolves to `/a` first, so the identity is built
  from a real, traversal-free path component - a strictly safer outcome than
  refusing the syntax after the fact.
- `STO-73` `Registry::load` migrates a local source's relative `url` to an
  absolute form, using the same resolution `STO-72` applies at parse time,
  once the process cwd is known. This covers a source melded before STO-72
  existed, or a hand-edited registry, that still carries a literal relative
  path. The rewrite happens ONLY when the resolved absolute path currently
  exists as a directory: when it does not, the recorded `url` is left exactly
  as written, so the "linked source is gone" finding (CLI-212, CLI-213) can
  name what the user actually typed rather than a confusing absolutized guess
  for a path that was never real to begin with. Only `url` is ever rewritten,
  never `name`: a source's `name` is the manifest's back-reference key
  (STO-22), and rewriting it would orphan every item installed from that
  source. The rewrite is in-memory only at load time; it is written back to
  disk the next time `Registry::save` runs (e.g. as part of the same
  command's normal write-back), the same as any other in-memory registry
  mutation (mirroring STO-68's revalidation).

## Manifest (manifest.json)

- `STO-20` The manifest maps `kind:effective_name` to an installed item.
- `STO-21` Each installed item records: `kind`, `name` (effective), `bare_name`,
  `source`, `commit`, `hash` (of source content), `store` (path relative to the
  mind root), `links` (absolute symlink paths, one per agent home; a relative
  lobe is resolved to absolute first, see STO-16), `description`, and
  `install_hooks` (STO-75, HOOK-110).
- `STO-22` `(source, kind, bare_name)` is the item's stable identity (see
  lifecycle.md). `store` and `links` are its file registry, used by uninstall.
- `STO-23` A missing manifest file is treated as empty.
- `STO-75` An installed item records an `install_hooks` set (HOOK-110), the
  per-item counterpart of a source's `install_hooks` (STO-10's `RecordedHook`
  set, HOOK-55): each entry is an effective install-hook command plus the
  commit it last ran at, or `None` when it was offered and skipped rather than
  run. Absent in a manifest written before this field existed, which
  deserializes as an empty set (no behavior change for an item installed by an
  older binary: its install hooks are simply treated as never offered). The
  set is discarded and rebuilt from scratch whenever the item is (re)installed
  (`learn`, or `upgrade`'s fresh install cycle) -- it is never merged with a
  prior set, so a record tied to an old commit can never suppress a hook at a
  new one. `mind hooks run` (HOOK-102) is the only reader: it filters a hook
  already recorded as run at the item's current commit out of what it offers,
  the item-level mirror of HOOK-101's source-side filter. `forget` removes the
  whole manifest entry, so the record is removed with it.

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
  the archive is not extracted. Version-pinned `evolve` (`--to V`) verifies
  the pinned release's `SHA256SUMS`.
- `STO-76` The resolved `evolve` target version -- from an explicit `--to`, a
  managed-policy pin, or a fetched `releases/latest` `tag_name` -- is validated
  as safe to use as a single URL path segment
  (`mindfile::is_plausible_release_tag`) before it is interpolated into either
  URL it drives: the release asset URL (STO-47's download) and the
  `SHA256SUMS` URL. This runs before either URL is built and before any
  download-step network call. Without it, a value carrying path segments (e.g.
  `1/../../../../attacker/mind/releases/download/v1`, from a repo/release
  takeover, a TLS-intercepting proxy, or a `--to` value copied from a
  malicious "install these steps" doc) re-points BOTH URLs at the same
  attacker-controlled location once curl normalizes the `..` segments, so the
  `SHA256SUMS` digest check would compare the attacker's binary against the
  attacker's own digest file and silently pass. A rejected value fails with a
  structured error naming the value (`SelfUpdateInvalidTarget`, a DIFFERENT
  kind from a managed-policy refusal -- see STO-77), refused before any
  network call the download step would make.

  A single leading `v` on the raw `--to` value (or a managed-policy pin) is
  stripped BEFORE this validation runs (and before the `decision` comparison),
  so `evolve --to v1.2.3` behaves identically to `--to 1.2.3`; only one
  leading `v` is stripped, not repeated ones (`--to vv1.2.3` still fails
  validation, since the un-stripped second `v` is not a digit).

  `is_plausible_release_tag` is a purpose-built sibling of
  `mindfile::is_plausible_version` (used for `min-mind-version` and policy
  version pins, spec/discovery.md DSC-40 and spec/policy.md POL-5x), not a
  reuse of it: that validator is digits-and-dots only, which is correct for a
  version pin but is stricter than a release tag needs to be, since it also
  refuses a legitimate semver prerelease/build suffix (e.g. `1.2.3-rc1`).
  Testing a prerelease before promoting it is a legitimate thing to want, and
  since GitHub's `releases/latest` never surfaces a prerelease, an explicit
  `--to` is the only way to reach one, so the release-tag validator accepts an
  IDENTIFIER-LIST grammar, not a bare character class: a dotted-numeric base
  (`\d+(\.\d+)*`) optionally followed by a semver-shaped prerelease suffix
  (`-...`) and/or build-metadata suffix (`+...`), each suffix itself
  `ident(\.ident)*` with `ident = [0-9A-Za-z-]+` -- i.e. EVERY dot-separated
  identifier in a suffix must be non-empty. This is deliberately not the same
  shape as "one run of `[0-9A-Za-z.-]+`" (a bare character class): since `.`
  is itself a member of that charset, a bare-character-class reading would
  also "match" a suffix built of nothing but dots, saying nothing about a
  component between two dots being non-empty. The identifier-list grammar
  rejects `/`, `\`, whitespace, control characters, and -- via the
  non-empty-identifier rule -- any `..` run anywhere in the string, including
  inside the prerelease/build suffix (e.g. `1.0.0-../..`, `1.0.0-..`), which
  would otherwise smuggle a traversal segment (or an ambiguous empty
  identifier) past a naive "digits-and-dots is gone, so a dash is safe"
  reading of the fix.
- `STO-77` Two follow-on corrections to `evolve`'s target-version handling,
  both driven by the STO-76 prerelease grammar:

  - **A malformed target is a distinct error from a policy refusal.** A
    `--to` (or resolved) value that fails STO-76's `is_plausible_release_tag`
    check is `SelfUpdateInvalidTarget`, never `SelfUpdatePolicy`. The two are
    different failures: `SelfUpdatePolicy` (JSON `kind`
    `self-update-policy`) means the managed policy disabled self-update
    (POL-52) or the requested version conflicts with a policy pin (POL-53);
    `SelfUpdateInvalidTarget` (JSON `kind` `self-update-invalid-target`)
    means the value never had a chance to reach policy evaluation at all --
    it is not a plausible version/tag shape. Reporting the malformed-value
    case under the policy kind would read, to a human or a CI log parser
    matching on `kind`, as "the managed policy blocks self-update" when the
    real problem is a bad argument.

  - **A numeric tie between `current` and `target` is resolved by prerelease
    precedence, not assumed up to date.** `decision` (STO-76's caller) treats
    two versions sharing the same dotted numeric base as a "tie". Before this
    fix, the tie was broken in only ONE direction: a prerelease `current`
    moving onto its own (non-prerelease) base `target` offered `Update`.
    Two other same-base pairings fell through to `UpToDate` even under an
    EXPLICIT `--to`, silently doing nothing: two prereleases of the same base
    pinned against each other (e.g. running `0.24.0-rc1`, `--to
    0.24.0-rc2` -- the main reason to pin an rc release at all), and a
    plain-release `current` explicitly pinned onto a same-base prerelease
    `target` (e.g. running the released `0.24.0`, `--to 0.24.0-rc1`). Both
    are now resolved: a prerelease predates its own base release (in either
    direction), and two prereleases of the same base are ordered against
    each other by semver precedence (semver.org precedence rule #11 --
    dot-separated identifiers compared pairwise, a purely-numeric identifier
    compared numerically and always ordering below a non-numeric one, a
    shorter identifier list ordering below one that extends it). The target
    ordering ABOVE `current` is `Update`; ordering BELOW `current` is a
    refused explicit downgrade (`PinnedBelowCurrent`, CLI-147) when
    `explicit` is true, or `UpToDate` when it is not (the non-explicit path,
    a fetched `latest` tag, never carries a prerelease in practice, so this
    arm is a safety net, not a normally-reached case). Byte-identical
    `current`/`target` strings, and two same-base versions differing only in
    build metadata (`+...`, which semver does not order on), stay
    `UpToDate`.
- `STO-65` `evolve --check` (and the run path's equivalent report) names the
  resolved release target triple (`target_triple`, e.g.
  `x86_64-unknown-linux-musl`) alongside the version comparison, so the exact
  artifact that would be fetched is visible before any network call -- e.g. a
  change in artifact resolution (the Linux legs moved from a `gnu` to a `musl`
  build) is caught at `--check` time rather than discovered after the swap.
  Each existing human-readable message keeps its exact original wording as a
  prefix, with `-- target <triple>` appended. Under `--json`, the same value is
  exposed as an additional `target_triple` key; the existing `action`, `target`
  (the version), and `outcome` keys are unchanged -- `--json` consumers depend
  on those names, so a new field is added rather than any key renamed.
- `STO-71` `resources/install.sh`'s Linux artifact resolution prefers the musl
  release leg (statically linked, so it runs on any glibc, STO-65's rationale)
  and falls back to the gnu leg exactly once if the musl asset download fails.
  This fallback exists because of a specific consistency gap install.sh sits
  in: the script itself is served and fetched straight off the `main` branch
  (`raw.githubusercontent.com/.../main/resources/install.sh`), but the
  *version* it installs is resolved separately, from the latest published
  GitHub release. Those two can disagree about which artifact legs exist for
  that version - `main` can already build (and this script can already know
  how to request) a target whose musl leg the latest release does not carry
  yet, if the musl leg was added to the build matrix after that release was
  cut. Without the fallback, running the always-current installer against an
  artifact-matrix-stale release would hard-fail with a 404 on the musl asset;
  with it, the script degrades to the gnu asset, which every published release
  carries, and installation still succeeds. The fallback triggers only on
  Linux and only when the failed asset was the musl one (so it fires exactly
  once, never loops), and prints a note naming the version and the fallback
  before retrying.
- `STO-74` `resources/install.sh`'s `fetch_to` (the downloader used for the
  release asset and `SHA256SUMS`) requires curl or wget on `PATH`, matching
  `fetch` (the downloader used for the `releases/latest` lookup): both run the
  same `command -v curl || command -v wget || err ...` check and both end the
  script with the same non-zero exit and the same `need curl or wget on PATH`
  message. The *output* is not identical, because of where each is called:
  `fetch_to` is invoked directly in a top-level `if`, so its `err` exits the
  script immediately with that one line on stderr. `fetch` is invoked inside a
  pipeline within a command substitution
  (`tag="$(fetch ... | sed ... | head -n 1)"`), and in `sh`/`dash` each
  pipeline stage runs in its own subshell, so `err`'s `exit 1` there kills only
  that subshell; the script falls through to
  `[ -n "$tag" ] || err "could not determine the latest release; set
  MIND_VERSION"`, so stderr shows that message too, after the real cause. The
  true cause always prints first and is never masked. This matters because
  `fetch_to` can be the *first* downloader call: when `MIND_VERSION` is set,
  the `releases/latest` lookup that goes through `fetch` is skipped entirely,
  so `fetch_to`'s own missing-downloader check is the only thing standing
  between a downloader-less environment and a misleading `download failed:
  <url>` error that hides the real cause.
- `STO-66` `evolve`'s download path soft-verifies the downloaded release
  archive's GitHub build-provenance attestation (`actions/attest-build-provenance`)
  via `gh attestation verify <archive> --repo jaemk/mind` when a `gh` binary is
  present on PATH, mirroring `resources/install.sh`'s `gh attestation verify`
  step. The check runs after the `SHA256SUMS` digest check (STO-47) and before
  extraction, inside the same transactional download-and-swap step (STO-46): a
  failure here leaves the existing binary untouched, exactly like every other
  failure in that step.
  - `gh` absent from PATH: `evolve` proceeds silently, with no note, matching
    install.sh's `if command -v gh` gate exactly.
  - `gh` present but the check could not be attempted (a `gh` build with no
    `attestation` subcommand, or a network-level failure reaching GitHub, e.g. DNS
    or connection failure) -- classified as a TOOLING error, not a statement
    about the artifact: `evolve` proceeds with a note and does not block.
  - `gh` present, the check ran, and it reported a positive result: `evolve`
    proceeds and prints a confirmation.
  - `gh` present, the check ran, and it reported the artifact does NOT verify
    (no matching attestation for the digest, a signer/repo mismatch, or an
    explicit signature failure): `evolve` aborts with `AttestationVerificationFailed`
    before extraction, leaving the existing binary in place.
  - **Difference from `resources/install.sh`:** install.sh treats every
    `gh attestation verify` failure, for any reason, as "could not be verified,
    continuing" (fully soft, never blocks). `evolve` is deliberately narrower:
    the failure classification above treats "no attestations found" (the
    message `gh` also uses for the ordinary, benign case of an artifact
    predating provenance attestations) as a GENUINE failure that aborts, not a
    tooling error that proceeds. This is intentional, not an oversight: an
    attacker who substitutes the release artifact cannot forge a validly signed
    attestation for the new digest, so a substituted artifact surfaces through
    the exact same "no attestations found" wording as a merely-absent
    attestation -- there is no way to tell the two apart from `gh`'s output.
    Treating that ambiguous case as a pass-through tooling error (as install.sh
    does) would defeat the entire point of the check for the one case it exists
    to catch, so `evolve` fails closed on it instead.
  - Failing closed does not strand anyone on a release that predates
    provenance: the download step is reached only on `Decision::Update`
    (CLI-147 refuses a downgrade without downloading), and a release older than
    provenance is by construction older than any binary carrying this check, so
    it never reaches the verification. Every version this check can download is
    one the release workflow attested.
  - **Accepted risk:** the tooling-error classification is a hand-curated list
    of `gh` output markers, so a `gh` version that words a tooling failure
    differently is read as a genuine failure and aborts an upgrade that should
    have proceeded. That is the safe direction (fail closed, with a message
    naming the reason), and widening the list on speculation would trade a
    refused upgrade for a missed substitution. The decision is to leave the list
    as it stands and widen it against a real report rather than a hypothetical
    one. Recorded so it is not re-raised as a finding.
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
- `STO-57` The `evolve` GitHub REST API fetch (the release-latest lookup on
  `api.github.com`) sends an `Authorization: Bearer <token>` header when a token is
  present in the environment: `GITHUB_TOKEN` first, else `GH_TOKEN` (matching the
  `gh` CLI), first non-empty (after trimming surrounding whitespace) wins. This
  moves the caller out of GitHub's unauthenticated per-IP rate limit (60/hr, shared
  across a NAT egress and easily exhausted on a workplace network, where the API
  returns HTTP 403) into the authenticated 5000/hr tier. The header is applied ONLY
  to `api.github.com` requests, never to the release-artifact or `SHA256SUMS`
  download on the `github.com` / CDN host, so the token is not forwarded across a
  cross-host redirect. With no token set, the request is byte-for-byte unchanged.
  The header args are built by pure helpers (`curl_auth_args`, `wget_auth_args`)
  and are unit-testable without touching the environment or spawning a process.
- `STO-61` For curl, `evolve` passes the `Authorization: Bearer <token>` header
  via a `--config` file rather than on argv, so the token is not exposed in
  `/proc/<pid>/cmdline` to a local co-tenant during the brief API call. The
  header is written as `header = "Authorization: Bearer <token>"` to a temp file
  created mode 0600 inside a fresh 0700 temp directory (the same directory scheme
  as the download, STO-45), curl is invoked with `--config <file>`, and the file
  is removed (best effort) after the invocation. The `api.github.com`-only host
  gating is unchanged (STO-57): the config file is written only for an
  `api.github.com` URL with a non-empty token. wget keeps the `--header=...` argv
  form. The config-file content and the `--config` arg are built by pure helpers
  (`curl_auth_config_content`, `curl_auth_args`) so they are unit-testable
  without spawning a process.
- `STO-62` `evolve`'s GitHub token handling fails CLOSED and OPEN, never hard:
  - **Fails closed on an unsafe token (B10).** A candidate token (`GITHUB_TOKEN`
    then `GH_TOKEN`) is rejected -- and the next candidate tried, else no
    authentication -- if it contains a control character (a `\n` could inject
    an additional `key = value` directive, such as `output = ...` or
    `url = ...`, into the curl `--config` file from STO-61) or a `"` / `\`
    (unescaped by the config file's quoted-string syntax, so either could break
    out of the quoted header value). A warning is printed to stderr naming which
    env var was rejected; the request proceeds unauthenticated rather than
    `evolve` erroring out over a malformed token.
  - **Fails open on a temp-file write failure (C19).** If writing the STO-61
    auth config file fails (e.g. a read-only or full `TMPDIR`), `evolve` warns
    on stderr and proceeds with an unauthenticated request instead of
    propagating the error. An unauthenticated request still succeeds below
    GitHub's per-IP rate limit, so degrading is strictly better than turning a
    working `evolve` into a hard failure over an unrelated temp-dir problem.
  - **Residual exposure (accepted).** The 0600 file inside its 0700 temp
    directory (STO-61) is removed on both the success and failure branch of the
    curl invocation, but a `SIGINT` (or `kill -9`) arriving during the API call
    itself skips that cleanup and can leave the token-bearing file on disk. This
    is not fully mitigated: `mind` adds no signal handler for it. The exposure
    window is bounded by the directory's 0700 mode (no other user can read it)
    and by the file being written fresh per invocation inside a per-process,
    unpredictably-named directory (STO-61) rather than a shared/reused path, so
    the residual risk is an orphaned file surviving until the temp dir is
    reaped, not a cross-user read or a predictable target.

## Schema versions

- `STO-50` Both `sources.json` and `manifest.json` carry a top-level `"version"`
  field with value `1`. A reader that finds a version greater than `1` fails with
  a `StateTooNew` error rather than silently misinterpreting the file. A missing
  `"version"` field is treated as `1` (backward compatibility with files written
  before this field existed).
- `STO-51` A `StateTooNew` error names the file (`"sources.json"` or
  `"manifest.json"`), the version found, and the highest version supported, and
  advises the user to run `mind evolve` (the binary self-update verb; `upgrade`
  is the item verb, so the remedy names `evolve` explicitly), or, if `evolve`
  itself reports up to date, to use the newer `mind` binary that wrote the
  file instead. The second clause covers the realistic trigger for this error:
  a locally built or newer-than-release `mind` wrote the state, so `evolve`
  cannot supply a newer release to fix it, and the first clause alone would
  strand the user with every command failing and no next step.

## Per-project lobes

- `STO-56` A lobe receives links only while its parent directory exists (the
  reachability gate). The leaf `skills/` directory is created on link, but a
  missing parent is never fabricated: a moved or deleted project lobe contributes
  no links and is not recreated by `introspect --fix`. The gate is checked at
  the reachability-sensitive write sites (the install fan-out's `planned_links`
  and `relink`) and not inside `agent_homes()`, which returns the complete
  configured list so uninstall confinement and `~/.claude` auto-create-on-link
  remain correct. It is intentionally NOT checked at `link_into_new_lobes`: that
  path backfills links into a newly added lobe (e.g. a preset base such as
  `base/.gemini/config`) whose parent may not exist yet, so gating there would
  suppress the very links it exists to create.

## Errors

- `STO-30` Filesystem failures carry the offending path (`Io { path, source }`).
- `STO-31` Malformed `sources.json` or `manifest.json` is a `Json` error naming
  the file.
