# CLI

The `mind` command surface. Verbs use a knowledge metaphor.

| command | role |
|---------|------|
| `probe [query] [--no-tui]` | interactive browser (default); catalog listing with `--no-tui`/`--json` |
| `meld [<repo>] [--register-only] [--yes] [-N\|--namespace <prefix>] [--root <dir>] [--flat-skills] [--pin <HEAD\|ref\|branch=NAME\|tag=NAME>]` | connect a source (default `.`), then install its items |
| `init-source [<path>] [--template] [--marketplace] [--flat-skills] [-N\|--namespace <prefix>]` | scaffold `mind.toml` + detect references (maintainer) |
| `unmeld <name\|glob> [--keep-items] [--yes] [--uninstall-hook <cmd>] [--dangerously-skip-hook-check]` (alias: `remove`/`rm`) | disconnect a source (or all sources matching a glob) and uninstall its items (`--keep-items` leaves them) |
| `learn <item> [--dangerously-skip-install-hook-check]` (alias: `install`) | install |
| `forget <item> [--dangerously-skip-hook-check]` (aliases: `unlearn`, `uninstall`) | uninstall |
| `sync [source]` (alias: `update`) | refresh sources (all, or one matching `[source]`) |
| `upgrade [--yes] [--no-sync] [item]` | upgrade installed; syncs first by default |
| `recall [item] [--sources] [--kind K] [--source S] [--json]` (aliases: `status`, `list`) | status: sources with their items (install state marked); `--sources` narrows to sources |
| `review [<target>] [-N\|--namespace <prefix>]` (default `.`) / `review --policy <path>` | validate a source / a policy file |
| `introspect` (alias: `doctor`) | diagnose |
| `evolve [--check] [--yes] [--to <v>]` (alias: `self-update`) | upgrade the `mind` binary itself |
| `hooks run <target> [--event E] [--force\|--rerun]` / `hooks list <target>` | run or list a source's or item's hooks on demand |
| `link-project [dir] [--preset <name>] [--subdir <rel>] [--snapshot] [--force]` | shorthand: link installed skills into a project's harness skills directory |
| `absorb <item> [--to <path>]` | claim an unmanaged lobe item into a managed source |
| `dump [--whole-sources] [--output <path>]` | write a super-source `mind.toml` reproducing the melded + installed state |
| `config show` / `config lobes ...` | view/edit config |
| `completions <shell>` | print a shell completion script |
| `man` | print the roff man page |

- `CLI-207` `mind` with no arguments (no verb, no global flag) prints the full
  `--help` text to stdout and exits 0. `command` (the verb) is a required, not
  `Option`al, field, so this is clap's `arg_required_else_help` behavior, not a
  subcommand default: bare `mind` still does not launch the interactive `probe`
  TUI (tui.md TUI-1/TUI-2 stay true -- only `probe` launches it). Without this,
  a required subcommand with no arguments is a clap usage error: 0 bytes on
  stdout, a usage string on stderr, exit 2, which is a poor first command for a
  new install to type.

## Item refs

- `CLI-1` An item ref is one of: `name`, `skill:name`, `agent:name`, `rule:name`,
  or `owner/repo#name` (source-qualified). `name` is the effective (installed)
  name, so a namespaced item is referenced as `<prefix>:<bare>`. Because the same
  `:` separates a kind from a name, the pre-colon token is read as a kind only
  when it is a reserved kind word (NS-26).
- `CLI-2` A bare `name` matches across kinds; a `kind:` prefix narrows to one kind.
- `CLI-3` A ref that matches no catalog item is an error (`ItemNotFound`). A ref
  that matches more than one is an error (`AmbiguousItem`) listing the candidates.
- `CLI-4` A malformed ref is an error (`InvalidItemRef`).
- `CLI-5` The source qualifier in `owner/repo#name` is a **ref**: it matches a
  source by its full `host/owner/repo` identity or any trailing component
  suffix (`repo`, `owner/repo`, `host/owner/repo`), and must resolve to
  exactly one source. An ambiguous suffix leaves multiple matches and resolves
  to `AmbiguousItem` rather than matching (or silently narrowing to) any of
  them. Contrast `sync`'s `[source]` argument (CLI-231), which is a
  **filter**: it intentionally allows more than one match, acting on every
  source it matches. "Ref" and "filter" name this distinction wherever it
  recurs below (`unmeld`'s `<name>`, CLI-20; `upgrade`'s `item`, CLI-63/65).

## meld

- `CLI-10` `meld <repo>` parses the repo spec, clones it under the sources tree,
  records the current commit, reads `[source].description` from `mind.toml` if
  present, and adds it to the registry.
- `CLI-205` A top-level `meld` that discovers zero items gets a non-success
  glyph and an explicit guidance line instead of a `melded <repo> (0 item(s))`
  line that reads identically to a legitimate empty source. The guidance names
  the convention paths (`skills/<name>/SKILL.md`, `agents/<name>.md`,
  `rules/<name>.md`, `tools/<name>/`) and the three escapes (`--root <dir>`,
  `--add-root <dir>`, `--flat-skills`), matching `init-source`'s zero-item
  message. Suppressed for: a nested/curated meld (not the caller's decision to
  act on), an authoritative `mind.toml` (`--root`/`--add-root`/`--flat-skills`
  either don't apply or are ignored there, DSC-52/DSC-76), a pure super-source
  that only curates other sources via `[discover].sources` (zero own items is
  expected, not a discovery failure), and a `.claude-plugin/` plugin or
  marketplace manifest source (which has its own guidance path, MKT-15).
  Suppressed under `--json`; the JSON result shape is unchanged.
- `CLI-11` Accepted repo specs: `owner/repo` and `github:owner/repo` (github.com),
  a full git URL (`https://host/owner/repo[.git]`), an SSH form
  (`git@host:owner/repo[.git]`), and a local path or `file://` URL. A spec that
  parses to none of these is an error (`InvalidRepoSpec`).
- `CLI-204` Each of the `host`, `owner`, and `repo` parts a spec parses to must
  be a single safe path component, or the spec is refused (`UnsafeRepoSpec`,
  naming the offending part). A part is refused when it is empty, is `.` or
  `..`, contains `/` or `\`, or contains a control character. Those three parts
  are load-bearing twice: joined with `/` they are the source identity (STO-13),
  which managed policy matches segment by segment (POL-10, POL-67, POL-68), and
  joined as directory components they are the clone path (STO-11), which `meld`
  deletes and re-clones. Without the check the SSH form, which splits only on
  the first `:`, admits a `host` such as `../../elsewhere` or `evil/host@x`: the
  first escapes the sources tree at `remove_dir_all` time, the second forges
  extra identity segments. `@`/`#` legality is per-part (STO-64): `host` refuses
  both; `owner` refuses `#` (it would sit before the repo segment and confuse
  `#`-splitting in item refs and hook targets) but allows `@` (a local path may
  legitimately carry it, `/src/proj@v2/agents`, and it collides with nothing
  there, since the `@<alias>` identity suffix, STO-58, only ever appends to
  `repo`); `repo` refuses both `@` (collides with that alias suffix and with the
  clone-dir leaf, STO-59) and `#` (collides with the item-link marker, LNK-4).
- `CLI-12` Re-melding a repo whose source identity is already registered is not
  an error and does not re-clone or re-register. The identity includes the
  consumer alias (STO-58), so a re-meld is `meld` of a `(repo, alias)` that
  already exists; a differing `--as` is a fresh meld of a new instance, not a
  re-meld. It ensures the source's items are installed: if any are missing it
  installs them (the default-install flow, CLI-23, honoring `--yes` and the
  non-TTY note). When nothing remains to install (or with `--link-only`) it
  prints a status of the source's items: each item's effective name, whether it
  is installed, and the commit it was installed from, flagging items whose commit
  lags the source. Items are matched by stable identity (source, kind, bare name).
- `CLI-206` A re-meld (CLI-12) never re-clones or re-registers, so `--root`,
  `--add-root`, `--flat-skills`, and `--install-hook` cannot take effect on it:
  they are discovery/hook-setup flags set only at the meld that first
  registers a source (DSC-51/STO-17, DSC-84/STO-55, DSC-75/STO-44, HOOK-20).
  `--pin` is NOT in this list (see CLI-209: a re-meld honors it). When any of
  these remaining flags is given against an already-melded source, a one-line
  stderr-suppressed-under-`--json` note lists exactly which flags were ignored
  and directs the user to `mind unmeld <source>` then `mind meld` again to
  apply them, so the drop is not silent. Follows the CLI-203 note pattern.
- `CLI-209` A re-meld (CLI-12) carrying `--pin` re-pins the already-registered
  source, unlike the CLI-206 discovery/hook flags: it resolves the pin request
  (CLI-200/CLI-201) against the source's currently recorded pin (the base
  point a bare `--pin HEAD` freezes, so it freezes whatever the source is
  pinned/following today rather than needing an `unmeld`/`meld` round trip),
  re-checks-out the existing clone at the resolved point (or, for a source
  that is still linked -- local and never pinned -- clones a fresh snapshot
  into the sources tree, same as a first meld's Step 3/4, so the live working
  tree is never touched), and records the resolved pin and commit. Reports
  `re-pinned <old> -> <new>` when the pin or commit changed, or that the
  source is already pinned to the resolved point when the request is a no-op
  (re-pinning to the value already recorded, with no upstream movement).
  Resolution/checkout failure (a bad ref, a network error) is a `Git` error
  that leaves the source's pin, commit, and clone dir exactly as they were
  (CLI-18) -- every git operation runs before any field on the registered
  source is mutated or the registry is saved. The POL-11 allowlist and POL-20
  require-pinned gates apply to the re-pin exactly as they do at a first meld,
  so a source that fell out of policy since it was melded cannot be silently
  re-pinned around the gate. Installed items whose content moved with the
  commit are then reported out of date by `recall`/`upgrade` exactly as any
  other upstream change is (CLI-75/LIFE-11).
- `CLI-13` `--as <prefix>` sets the source's namespace, overriding any
  `[source].prefix`. It is persisted and is not changed by `sync`. A `--as`
  prefix is an identity alias (STO-58): `meld <repo> --as <prefix>` denotes the
  instance `host/owner/repo@<prefix>`, which coexists with a bare meld of the same
  repo and with other aliases of it. It is never an in-place re-prefixing of a
  differently-aliased instance. To change an instance's prefix, edit it in the
  probe TUI (TUI-53, allowed only while no items are installed, CLI-161), or
  `unmeld` it and `meld` again with the new `--as`. `--as ''` removes the prefix
  (the bare `host/owner/repo` identity).
- `CLI-14` After melding, if a prefix is in effect and `--verbose` is in effect
  (CLI-162), unguarded prose references to siblings are reported as warnings (see
  namespacing.md NS-20). Without `--verbose` the warnings are suppressed. Warnings
  do not fail the command.
- `CLI-15` If the melded repo's `mind.toml` lists `[discover].sources`, each is
  melded recursively (see DSC-38), so one `meld` can pull in a curated set. When
  more than one source is added, `meld` reports the total count.
- `CLI-16` `meld --root <dir>` (repeatable) sets the source's scan roots,
  overriding any `[source].roots` (DSC-51). The roots are persisted on the source
  (STO-17). A root that is not a directory in the clone is `InvalidRoot`.
- `CLI-197` `meld --add-root <dir>` (repeatable) adds convention scan roots that
  compose with the source's authoritative discovery layer instead of replacing
  it (DSC-84): a `.claude-plugin/` manifest or an authoritative `mind.toml`
  keeps its items and each added root is convention-scanned in addition. The
  roots are persisted on the source (STO-55). A value that is not a directory in
  the clone is `InvalidRoot`.
- `CLI-158` `meld --flat-skills` force-enables flat skill discovery for the
  source: skills are bare-name directories at a scan root, with no `skills/`
  container (DSC-74). The flag is one-directional (no `--no-flat-skills`): it turns
  the layout on for a source that did not declare `[source].flat-skills`, but
  cannot disable a source's declared flat layout (DSC-75). It applies to the skill
  kind only; agent, rule, and tool discovery are unaffected. It is persisted on the
  source (STO-44). For an authoritative `mind.toml` it is ignored with a note
  (DSC-76).
- `CLI-17` `meld` takes at most one pin flag: `--pin` (with a required value) or a
  deprecated alias (CLI-202). Supplying more than one (two aliases, or `--pin` plus
  an alias) is `ConflictingPin`. The chosen pin is persisted on the source
  (STO-18). With no pin flag, the source's `[source]` pin directive (DSC-41)
  applies, else the default is following the remote default branch. `--pin`
  requires a value, so there is no bare-flag form that could ambiguously consume
  the following positional argument.
- `CLI-18` `meld` clones at the pinned point and records the resolved HEAD commit.
  A `follow` pin (`default-branch` / `follow-branch` / `tag`) checks out that
  branch or tag; a `ref` pin checks out the commit. For a `--pin` value that
  freezes (`HEAD` or a bare ref, CLI-200), the point is first checked out to
  resolve its current commit, which is then persisted as a `ref` pin. A pin that
  does not resolve in the remote is a `Git` error and nothing is registered.
- `CLI-200` `--pin <value>` where the value freezes to an immutable `ref` pin:
  `--pin HEAD` freezes the current resolved tip (the point that would otherwise be
  melded: the remote default branch, a deep-link branch ref (LNK-3), or a
  `[source]` directive) to its current commit sha. `--pin <ref>` (a bare tag name,
  sha, or branch name) resolves `<ref>` to its current commit and freezes that.
  Freezing a branch or tag snapshots its current tip; it does not track it. On
  `learn`, `--pin` is a bare flag that freezes a deep-link URL's ref (LNK-3); for a
  plain (non-URL) item ref it is a no-op and prints a note, and the install still
  proceeds.
- `CLI-201` `--pin branch=<name>` and `--pin tag=<name>` record a floating pin:
  `branch=<name>` tracks that branch (`follow-branch`); `tag=<name>` tracks that
  tag (`tag`), which re-points on sync if the upstream tag moves (CLI-55). A value
  carrying an unrecognized `key=` form (anything but `branch=` or `tag=`) is a
  `BadPinSpec` usage error, so a mistyped key is not silently taken as a ref name.
- `CLI-202` The former `--follow-branch <branch>`, `--pin-tag <tag>`, and
  `--pin-ref <commit>` flags remain as hidden deprecated aliases mapping onto the
  new model: `--pin branch=<branch>`, `--pin tag=<tag>`, and `--pin <commit>`
  respectively. They are accepted for one release and count toward the
  at-most-one-pin rule (CLI-17), so mixing a deprecated alias with `--pin` is
  `ConflictingPin`.
- `CLI-203` `learn <url> --pin` freezes the link instance's ref only at
  meld/registration (CLI-200, LNK-3). When the link is already melded, the
  meld+pin step is skipped, so `--pin` has no effect; `learn` prints a one-line
  note that `--pin` was ignored because the instance is already melded, rather
  than dropping the flag silently. The install still proceeds. Suppressed under
  `--json`.
- `CLI-19` An explicit `git@host:owner/repo` (or `ssh://`) spec clones over SSH
  using the user's key/agent, with no username/password prompt. With `ssh = true`
  in `~/.mind/config.toml`, `meld` (and `sync` auto-meld) also rewrites an https
  remote to its `git@host:owner/repo` SSH form before cloning, so the `owner/repo`
  shorthand uses SSH too. A local path and an explicit `git@...` / `ssh://` spec
  are left unchanged; the rewritten URL is recorded, so later `sync`s reuse SSH.
  An https remote still authenticates as git normally does (a credential helper,
  or the interactive prompt).
- `CLI-23` By default, after registering the source, `meld` previews its items
  and prompts to install them all (the interactive form of `learn '<source>#*'`),
  installing the whole source on a yes. The prompt defaults to yes (`[Y/n]`, a
  bare Enter installs), since reaching it means the user chose to meld the source;
  it is reversible with `forget`. `--register-only` (deprecated alias:
  `--link-only`, see CLI-165) stops at registering the source; its items remain
  available to `learn` later. `--yes` installs without prompting, including in a
  non-TTY context; without `--yes` a non-TTY `meld` registers only and prints how
  to install later (mirroring the install-hook non-TTY behavior, HOOK-22). Only
  the top-level source is offered (a curated super-source's nested sources are not
  auto-installed), already-installed items are skipped (DEP-23), and a source
  install hook is still handled by its own prompt during the meld (HOOK-20).
- `CLI-156` In `--json` mode, `meld` is fully non-interactive and never prompts.
  When `--yes` is given the items are installed as part of the single meld result:
  the `installed` array in the JSON object lists the effective keys of every item
  installed in that call. When `--yes` is absent, no install prompt is shown and
  no install occurs; instead the JSON result carries a `pending_items` integer with
  the count of items available to install. In both cases exactly ONE top-level JSON
  object is written to stdout (CLI-153).
- `CLI-24` When a source declares `[source].prefix` and no `--as` was given, an
  interactive `meld` prompts whether to namespace its items under that prefix:
  accept it, type a different prefix, or choose none. The prompt previews the
  resulting installed names under the declared prefix (e.g. `skill:jk:foo`) so the
  effect is visible before choosing. The choice becomes the source alias and
  applies to the scan and the install (`<prefix>:<name>`). A non-interactive meld
  accepts the declared prefix as-is. An empty alias (`--as ''` or the "no prefix"
  answer) explicitly overrides a declared prefix to none. A source that declares
  no prefix is not prompted.
- `CLI-25` `meld` with no `<repo>` argument defaults to the current directory
  (`.`), melding the repo the command is run in. Combined with the default
  install (CLI-23), running `mind meld` inside a source repo (e.g. one with a
  `mind.toml`) registers and installs that source.
- `CLI-27` A local-path source with no pin in effect is *linked*: `mind` reads it
  directly from its working tree (the path `meld` was given) rather than cloning
  it, so the maintainer's in-progress edits -- including an untracked or
  gitignored `mind.toml` -- are seen live by `meld`, `sync`, `upgrade`, and
  `recall`. `mind` never deletes a linked source's directory: `unmeld` removes the
  registry entry (and, by default, its installed items) but leaves the working
  tree, and a failed `meld` never touches it. `sync` does not fetch or reset a
  linked source (it only re-reads its HEAD); a deleted working tree is a per-source
  sync error (CLI-54). A pinned local source (any `--pin` or a `[source]`
  directive) is instead cloned as a snapshot at the pin, so pinning still works.

- `CLI-159` `meld --namespace <prefix>` sets the source's namespace, opting the
  source into prefixing (with no flag and no `[source].prefix`, items install bare,
  NS-2). It is the renamed `--as` (CLI-13); `--as` is retained as a hidden,
  deprecated alias so existing invocations keep working. `review` takes the same
  rename (`--namespace` aliasing `--as`, CLI-133). `--namespace ''` removes the
  prefix (the explicit no-prefix override of a declared `[source].prefix`, as
  `--as ''` did, CLI-13). As of CLI-163, the short form is `-N` (uppercase).
- `CLI-161` Changing a registered source's namespace in place (the TUI
  source-details editor, TUI-53, via `set_source_namespace`) is allowed only
  while no items from the source are installed; a change requested while items
  are installed is an error naming those items and directing the user to `forget`
  them first, with the namespace unchanged (NS-30). This changes the source's
  effective display prefix (`alias`), not its identity: the identity alias
  (`as_alias`, STO-58) is fixed at meld, so an in-place namespace change never
  renames the source or relocates its clone. To make a repo a distinct instance
  under a new prefix, `meld` it with a different `--as` (CLI-13), which registers
  a separate `@<alias>` instance.
- Cross-source collision detection (namespacing.md NS-43/44/45): after catalog
  discovery and before install, `meld` checks incoming skills, rules, and tools
  against already-installed items from other sources. An incoming item whose
  effective `(kind, name)` pair matches an installed item from a different source
  is a cross-source collision (distinct from the same-invocation check at CLI-33
  and from NS-41 for agents). In an interactive session (TTY, no `--yes`), `meld`
  pauses and prompts for a namespace prefix, pre-populated with the repo name
  (NS-44). In a non-interactive session (no TTY or `--yes`), `meld` exits
  non-zero, lists the conflicts, and suggests re-running with `--namespace
  <repo-name>` (NS-45). A source already carrying a namespace whose effective
  names do not collide is not affected.
- `CLI-177` When the top-level `meld` command fails to clone a remote source due
  to an authentication error (DSC-68 patterns), it prints a remediation hint to
  stderr before surfacing the error. The hint lists three alternatives: (1) the
  SSH remote form of the URL (e.g. `mind meld git@host:owner/repo`), (2) setting
  `ssh = true` in `~/.mind/config.toml` to always prefer SSH for HTTPS remotes,
  and (3) configuring a git credential helper. The hint is suppressed for local
  sources (no SSH alternative applies).
- `CLI-178` When a `meld` (top-level) or `sync` (per-source) git operation fails
  with a proxy error -- matching "Received HTTP code 407", "The requested URL
  returned error: 407", or "Could not resolve proxy" (case-insensitive) -- a hint
  pointing at `HTTPS_PROXY` and `git http.proxy` is printed to stderr before the
  error is surfaced. The check is applied to the same `MindError::Git` stderr
  matching used by `is_auth_failure`.
- `CLI-180` Top-level `meld` clone errors (both the initial default-branch clone
  and the re-clone at pin) lead with git's stderr as the first line printed to
  stderr, so the actual cause is immediately visible. The reconstructed git
  command line and the internal store path are shown only under `--verbose`
  (CLI-162); without it those details are suppressed. The git stderr always
  appears regardless of `--verbose`. In human mode the error returned to the
  caller has its args reduced to `["clone"]` and its stderr set to
  `"(git output above)"` so the display does not repeat the store path and does
  not show the literal `<no stderr>` when output was already printed (CLI-185).
  Under `--json` the stderr is preserved intact in the returned error (CLI-184).
- `CLI-184` Under `--json`, `handle_top_level_clone_err` skips all stderr
  printing and returns the `MindError::Git` with git's stderr field intact and
  args reduced to `["clone"]`, so the CLI-181 JSON envelope's `message` field
  contains the actual cause from git rather than the literal `<no stderr>`.
- `CLI-185` In human mode, after printing git's stderr to the terminal,
  `handle_top_level_clone_err` sets the returned error's stderr field to
  `"(git output above)"` instead of clearing it, so the error trailer never
  displays the misleading literal `<no stderr>`.
- `CLI-186` Git stderr and error text echoed during meld clone failures (the
  top-level clone path) and sync per-source failures are passed through
  `strip_ansi` before being printed, preventing a hostile remote from embedding
  ANSI escape or Unicode bidi-override sequences to corrupt or spoof terminal
  output.
- `CLI-224` A `review` finding's message (built from source-controlled text: a
  token, a hardcoded path, an `item.key()`) is passed through the same
  `strip_ansi` sanitization CLI-186 applies to git stderr, applied once at
  `Finding::hard`/`Finding::advisory` construction so both the human `error
  [kind]: ...`/`advisory [kind]: ...` text (CLI-131) and the `--json` document
  (CLI-219, including its CLI-221 `details` form on a hard failure) inherit the
  stripped string from the one place rather than each sanitizing separately.
  Serde alone keeps `--json` structurally valid but does not stop an embedded
  ANSI escape or bidi-override sequence from corrupting or spoofing a
  terminal that renders the message. `strip_ansi` (`src/sanitize.rs`) collapses
  a run of C0/DEL/C1 control characters that reach its own filter to a single
  space rather than deleting it -- in practice, the only such character is
  `\n` (every other C0/DEL/C1 control is already consumed with no trace by the
  first pass, `strip_ansi_escapes::strip`, before this function's filter ever
  runs) -- so text built by joining originally-separate lines with `\n` (a
  multi-line hook command embedded in a disclosure, an `item-hook` finding's
  `'{cmd}'`) is not silently fused into one word once the separator is
  stripped. A security-blocked Unicode code point (a bidi override, a
  directional mark U+200E/U+200F/U+061C, or a zero-width character
  U+200B/U+2060/U+FEFF) is still dropped with no space substituted, since none
  of those is a word boundary.
- `CLI-225` Every error message or printed note that interpolates a
  source-or-user-influenced identity (a source name, a `meld --as`/
  `[source].prefix` alias, an item-link `#<path>` segment, an agent's harness
  `name:`, a suggested namespace prefix derived from a repo name, or any similar
  value) into a RUNNABLE command a reader is invited to copy and paste (`mind
  unmeld ...`, `mind forget ...`, `mind meld ...`, `mind hooks run ...`, or any
  shell command) shell-quotes that identity -- the POSIX single-quote `'\''`
  idiom (`src/error::shell_quote`) -- before it lands in the command, applied
  unconditionally rather than only when the value "looks dangerous". None of
  these identities is restricted against shell metacharacters (`validate_prefix`
  and `is_safe_manifest_path` reject only path traversal, not `;`/`'`/`$`/a
  backtick/whitespace), so an unquoted splice would let a crafted identity turn
  the pasteable remedy into an injection: a `'` breaks out of a
  single-quote-framed raw value, after which `;`/`$(...)`/a backtick ride along.
  This generalizes the `hooks run` family's rule (HOOK-106) to every printed
  remedy: `LinkedSourceGone` (`mind unmeld`), `AgentCollision` (`mind forget
  agent:<name>`), and `SkillCollision` (`mind meld --namespace <prefix>`) each
  quote their interpolated identity by exactly this rule. A bare mention of the
  same identity in surrounding English prose (naming *what* is wrong, not a
  command to run) is not quoted; only the runnable command is. The rule is not
  limited to a `MindError` variant: an ordinary `println!`/`eprintln!` note in
  `commands.rs`/`install.rs` that offers a pasteable remedy is the same class
  and is quoted the same way -- the re-meld ignored-flags note (`mind unmeld
  <name>`), the emptied item-link forget hint (`mind unmeld <link-id>`), the
  `learn` not-found hint (`mind probe <query>`, echoing the user's own
  argument) and its melded-source hint (`mind learn --all <query>`), the
  unreachable-lobe note (`mind config lobes remove <path>`, a path taken from
  user config), the DSC-60 add-roots-yourself hint (`mind meld <source>
  --add-root <dir>`), `unmeld --unlink-only`'s and the non-TTY post-meld
  install note's remaining-items hints (`mind forget`/`mind learn
  <source>#*`), the agent-collision skip warning (`mind forget
  <kind>:<name>`), a re-meld's per-item "not installed" listing (`mind learn
  <kind>:<name>`), and `init-source`'s `--template` hint (`mind init-source
  <dir> --template`, the user's own path argument).
- `CLI-187` When no sources are melded, all verbs that report the empty state
  (sync, recall, probe) emit the same message: "no sources melded; run `mind
  meld <owner/repo>` to add one". This consistent phrasing always names the
  next command so a first-run user has a clear path forward regardless of which
  verb they tried first.

## unmeld

- `CLI-20` `unmeld <name>` removes the source's clone and registry entry.
  `name` is a **ref** (CLI-5): it must resolve to exactly one source. It is
  the full identity (`host/owner/repo`, plus the `@<alias>` suffix for an
  identity-aliased instance, STO-58) or an unambiguous trailing suffix (e.g.
  `repo`, `owner/repo`, or `repo@<alias>`). The `@<alias>` is part of the
  identity, so a bare suffix reaches only the un-aliased instance: with both a
  bare `repo` and a `repo@jk` melded, `unmeld repo` targets only the bare one and
  `unmeld repo@jk` the aliased one (use a glob, CLI-28, to remove several at
  once). An unknown name is `SourceNotFound` and an ambiguous suffix is
  `AmbiguousSource` -- unlike `sync`'s `[source]` **filter** (CLI-231), which
  intentionally allows a plain name or suffix to match several sources, a
  non-glob `unmeld` ref never widens to more than one. The former visible
  alias `detach` is removed and is now a usage error (CLI-172).
- `CLI-21` `unmeld <name>` by default uninstalls every item installed from the
  source (each via its file registry, then its manifest entry), mirroring meld's
  install-by-default (CLI-23): dropping a source cleans up after itself in one
  step. It first lists the items it will remove; the multi-item confirmation
  (CLI-42) applies, and `--yes` skips it.
- `CLI-22` `unmeld --keep-items` (deprecated alias: `--unlink-only`, see CLI-166)
  removes only the source (clone and registry entry) and leaves its installed items
  in place. It lists those orphaned items and suggests the `forget` command to
  remove them later. This is the opt-out from the default item removal (CLI-21),
  mirroring `meld --register-only` (CLI-165).
- `unmeld` runs the source's uninstall hooks before removal and accepts
  `--dangerously-skip-hook-check` to run them unattended, and
  `--uninstall-hook <cmd>` to supply or override the uninstall hook (see
  install-hooks.md, HOOK-54, HOOK-59).
- `CLI-227` `unmeld` and `forget`'s hook-consent flag is
  `--dangerously-skip-hook-check`. `--dangerously-skip-install-hook-check` (the
  original spelling) is kept as a hidden alias: both gate an UNINSTALL hook on
  these two verbs, despite the "install" in the old name, which the rename
  fixes without breaking a script that already passes the old spelling.
  `meld`/`learn`/`sync`/`upgrade`/`hooks run`, whose install-hook flag really
  does gate an install hook, keep `--dangerously-skip-install-hook-check`
  unchanged.
- `CLI-230` `unmeld` accepts `remove` and `rm` as additional visible aliases,
  for symmetry with `learn`'s `install` alias and `forget`'s `unlearn`/
  `uninstall` aliases.
- `CLI-28` `unmeld <pattern>` accepts a glob (`*`, `?`, `[`) in place of an exact
  name or suffix (CLI-20), matched against each melded source's `host/owner/repo`
  identity and its trailing-suffix forms, mirroring `learn`/`forget` glob selection
  (CLI-31, CLI-41). The pattern is matched against the identity as a plain string,
  so `*` spans any run including `/`: `mind unmeld '*agents'` removes
  `github.com/jaemk/agents`. Every matching source is unmelded, each per its normal
  path (CLI-21 by default, or CLI-22 under `--unlink-only`). A glob is what permits
  a multi-source match: a plain name or suffix (the CLI-20 ref) that resolves to
  several sources is still `AmbiguousSource` (CLI-20), but a glob removes all it
  matches. A glob matching no source is `SourceNotFound`. When a glob matches more
  than one source, `unmeld` lists the matched sources and confirms before removing
  them (the multi-item confirmation of CLI-42, applied at source granularity);
  `--yes` skips the confirmation.

## learn

- `CLI-30` `learn <item>` with an exact ref installs the single matching item:
  it copies the item into the store and links it into every configured agent
  home (see lifecycle.md, STO-14), except a store-only tool, which installs to
  the store with no link (tooling.md TOOL-3). It records the item in the
  manifest; a ref matching none is `ItemNotFound` and one matching several is
  `AmbiguousItem`.
- `CLI-31` When the ref name is a glob (`*`, `?`, `[`), `learn` installs every
  matching item. The kind prefix, source qualifier, and glob compose: `'*'` is
  everything, `'skill:*'` all skills, `'owner/repo#*'` all items of one source,
  `'owner/repo#skill:*'` all skills of one source. A glob matching nothing is
  `ItemNotFound`.
- `CLI-32` `--dry-run` (`-n`) lists the items that would be installed and installs
  nothing.
- `CLI-33` The collision check runs before any install: if the selected set
  contains two items that would install under the same `kind:name`, `learn`
  errors (`AmbiguousItem`) and installs nothing.
- `CLI-34` If a later item in a multi-item `learn` fails, the items already
  installed are still recorded in the manifest (state stays consistent with
  disk) and the failure is reported.
- `CLI-35` `learn --force` (`-f`) overwrites a link target that already exists
  and is not managed by mind (the clobber guard, LIFE-41), replacing the user's
  file, directory, or foreign link. Without `--force`, hitting such a conflict
  prompts on a TTY to overwrite that one target (a yes installs it forced, a no
  refuses it) and, in a non-TTY context, refuses with `LinkOccupied` as before.
  The forced overwrite stays transactional: it is decided before staging, so a
  refusal changes nothing. `meld --force` applies the same to its default
  install.
- `CLI-36` `learn <source> --all` is shorthand for `learn '<source>#*'`: it
  appends the `#*` selector to the positional ref, promoting it from an item name
  to a source qualifier and installing every item of that source (CLI-31), deps
  and all. `--all` is rejected with `InvalidItemRef` when the ref already carries
  a `#` selector, since the selector would be doubled.
- `CLI-157` When every item in a `learn` selection is already installed (the
  dependency closure after DEP-23 filtering is empty), `learn` exits 0 but treats
  this as a distinct no-op, not a silent success. In human output it prints a line
  such as "already installed; nothing to do". Under `--json` the outcome token is
  `"up-to-date"` rather than `"installed"`, so callers can distinguish a real
  install from a re-run that changed nothing.
- `CLI-179` When `learn <name>` finds no item matching the query and at least one
  source is melded (`sources > 0`), a hint is printed to stderr directing the user
  to `mind probe <query>` to search the available catalog. The zero-sources case
  (`sources == 0`) is unchanged: it directs the user to `mind meld` instead.
  `mind sync` is not mentioned because syncing cannot surface an item that does not
  exist in any melded source; only browsing with `probe` can confirm what is
  available.
- `CLI-208` When `learn <name>` finds no item matching the query (`ItemNotFound`)
  and the query names -- exactly, or as an unambiguous trailing suffix (the same
  match rule as a source qualifier, CLI-5) -- exactly one already-melded source, a
  second stderr hint (alongside CLI-179) names that source and points at
  `mind learn --all <name>` to install all of its items. `learn <source-name>`
  reads like `meld`'s `<repo>` argument, and `install` (the `learn` alias) trains
  exactly that habit, so this is the most common way a query resolves to a source
  rather than an item. Suppressed under `--json`, matching the CLI-179 hint. Not
  printed when the query matches zero or more than one melded source (an ambiguous
  suffix is not a case this hint disambiguates).

## forget

- `CLI-40` `forget <item>` (alias: `unlearn`) removes an installed item using its
  file registry and deletes its manifest entry. The ref is matched against the
  manifest by effective name, honoring a `kind:` prefix and an `owner/repo#`
  source qualifier; a bare name that matches more than one installed item (e.g.
  a skill and an agent of the same name) is `AmbiguousItem`, and one matching
  none is `NotInstalled`.
- `CLI-41` When the ref name is a glob, `forget` uninstalls every matching
  installed item, mirroring `learn`'s glob selection (CLI-31). The kind prefix
  and source qualifier compose with the glob. A glob matching no installed item
  is `NotInstalled`.
- `CLI-42` When `forget` would remove more than one item (a glob that matched
  more broadly than intended), it lists the matched items and confirms before
  removing any. `--yes` (`-y`) skips the prompt; a non-TTY run without `--yes`
  refuses (`ConfirmationRequired`) rather than removing silently. Removing a
  single exact match is not prompted.

## sync

- `CLI-50` `sync` fetches every source, resets its clone to the remote default
  branch, and updates the recorded commit and `[source].description`. A linked
  local source (CLI-27) is not fetched or reset: `sync` only re-reads its HEAD
  and updates the recorded commit from the working tree.
- `CLI-231` `sync` accepts an optional `[source]` **filter**: `mind sync
  <source>` fetches every melded source whose name matches it -- exactly
  (`host/owner/repo`), by trailing suffix (e.g. `repo` or `owner/repo`), or by
  glob -- leaving every other source untouched. A `sync` filter is a
  deliberately different concept from `unmeld`'s/`upgrade`'s **ref** (CLI-5,
  CLI-20): a filter is NOT required to be unambiguous, and is not an error
  when it is not. A suffix shared by more than one melded source (e.g. two
  sources both ending in `/tools`) matches and syncs all of them rather than
  erroring, the same way a glob does -- appropriate because `sync` is a
  non-destructive refresh, unlike `unmeld` (destroys) or `upgrade` (mutates a
  specific identity), which both refuse to guess. A filter that matches no
  melded source (against a non-empty registry) is a `SourceNotFound` error,
  so a typo is reported rather than silently syncing nothing; a malformed
  glob is an `InvalidPattern` error.
  With a filter, two whole-registry passes are skipped: the managed-policy
  auto-meld provisioning (POL-32) and the `[discover].sources`/marketplace
  nested-source re-walk (DSC-57) run only for a full `sync`. This does NOT
  extend to the `--upgrade` pass (CLI-53): see `CLI-232`, which scopes that
  pass to the same filter rather than leaving it global, and `CLI-233`, which
  names the filter's matches in the confirmation when it matched more than
  one. With no filter, every source is fetched (CLI-50, the unchanged
  default).
- `CLI-51` With no sources melded, `sync` reports that and exits successfully.
- `CLI-52` `sync` does not change consumer aliases.
- `CLI-53` `sync --upgrade` runs an `upgrade` pass after refreshing sources
  (reporting pending upgrades and prompting before applying, exactly like
  `upgrade`), so a single command both fetches upstream and applies pending
  upgrades.
- `CLI-232` With a `[source]` filter, `sync <source> --upgrade`'s `--upgrade`
  pass is scoped to the filter's matched source(s) (CLI-231), the same set
  `sync` itself just refreshed: only installed items whose recorded source is
  in that set are considered for upgrade, and a source's install hook is
  re-run (HOOK-11) only when its source is in that set. Without this,
  `--upgrade` pass ran unscoped regardless of the filter: `mind sync alpha
  --upgrade --yes` would upgrade every installed item from every melded
  source, and re-run install hooks (arbitrary shell) for sources the caller
  never named -- exactly the protection a scoped `upgrade <item>` already
  provides (HOOK-11), silently lost when the same scoping is expressed
  through `sync <source>` instead. With no filter, the pass is unscoped
  (every source), unchanged.
- `CLI-233` When a `[source]` filter matches more than one source and
  `--upgrade` reaches its confirmation, `sync <source> --upgrade` names every
  matched source in that confirmation, before the user consents: a filter is
  allowed to match more sources than a plain name suggests (CLI-231), so
  applying the upgrade pass to all of them -- and possibly re-running each
  one's install hook (HOOK-11), arbitrary shell -- can silently widen past
  what `mind sync skills --upgrade` reads as. A filter matching exactly one
  source, or no filter at all (an unscoped `--upgrade` pass), leaves the
  confirmation unchanged: naming a single match is not new information. This
  applies in text mode (a line naming the matches, printed before the `[Y/n]`
  prompt) and under `--json` (the matches are folded into the
  `ConfirmationRequired` error's `action` text; `--json` never prompts,
  LIFE-45, so this is the only channel a `--json` caller sees them through).
  `--yes` is unaffected either way: it skips the confirmation step entirely,
  so there is no prompt to name the matches in.
- `CLI-54` A per-source failure (e.g. a network error on one remote) does not
  abort the run: `sync` refreshes each source independently, persists the
  progress made (the recorded commits of the sources that succeeded), reports
  each failure, and exits non-zero (`SyncFailed`). With a failure, the `--upgrade`
  pass is skipped.
- `CLI-55` `sync` resolves each source against its recorded pin (STO-18): a
  `follow-branch` source resets to that branch's current tip and updates the
  recorded commit; a `pin-tag` / `pin-ref` source re-fetches but stays at the
  pinned tag / commit, so its recorded commit moves only if the upstream tag was
  moved (a moved tag is reset to). `sync` never changes the pin itself (cf.
  CLI-52 for aliases). `upgrade` and `introspect` operate on the synced (pinned)
  content, so a `pin-tag` source does not report drift as upstream's default
  branch advances past the tag.

## upgrade

- `CLI-60` `upgrade` reports pending upgrades and, unless `--yes` is given, prompts
  `[Y/n]` (default Yes, a bare Enter applies; EOF counts as No) before applying
  anything. The affirmative is the default because reaching the prompt means the
  user asked to upgrade, and an upgrade is reversible (re-pin and `sync`/`upgrade`,
  or `forget`).
- `CLI-61` The report lists, per item, the hash and commit deltas, and a compare
  URL when the source host supports one. A namespace change is shown as a rename.
- `CLI-176` The compare URL is produced for https remotes using the GitHub
  `/compare/<old>...<new>` URL shape by constructing
  `https://<host>/<owner>/<repo>/compare/<old>...<new>`. This covers GitHub.com,
  GitHub Enterprise Server, and Gitea/Forgejo instances. SSH remotes and
  local/file paths return no compare URL because there is no web host to link to.
- `CLI-188` Hosts whose hostname contains the substring "gitlab" or "bitbucket"
  (case-insensitive) use a different compare URL shape and therefore return no
  compare URL. This supersedes the broader claim in CLI-176 that the GitHub shape
  applies to any https forge: those two forge families are suppressed because the
  GitHub-shaped link would 404. A self-hosted instance on a neutral hostname (e.g.
  `git.corp.internal`) continues to produce the GitHub-shaped link.
- `CLI-62` `--yes` applies upgrades without prompting.
- `CLI-63` An optional `item` limits upgrade to the matching installed item(s),
  matched against the manifest by effective name and honoring a `kind:` prefix
  and an `owner/repo#` source qualifier. `item` is itself a **filter** over
  installed items, not a `unmeld`-style ref: it may match several installed
  items and, unlike `unmeld`'s `<name>` (CLI-20), that is never an error --
  every match is upgraded. The embedded `owner/repo#` qualifier, when present,
  IS a ref, though (CLI-5): it must resolve to exactly one source, or
  `AmbiguousItem`.
- `CLI-64` With nothing pending, `upgrade` reports up to date and changes nothing.
- `CLI-65` When the `item` filter's name is a glob (`*`, `?`, `[`), `upgrade`
  limits the pass to every installed item whose effective name matches the
  glob, mirroring `forget`'s glob selection (CLI-41). The `kind:` prefix and
  `owner/repo#` source qualifier compose with the glob exactly as they do for
  an exact match (CLI-63), so `upgrade 'jk:*'` upgrades a namespace, `upgrade
  'skill:*'` a kind, and `upgrade 'owner/repo#*'` a whole source in one pass.
  Like any filter, a glob that matches no installed item -- or matches only
  items already up to date -- reports nothing pending and changes nothing
  (CLI-64); it is not an error (this is where `upgrade` differs from
  `forget`, whose no-match glob is `NotInstalled`, CLI-41).
- `CLI-211` A source `upgrade` cannot scan (its clone is missing, or scanning it
  otherwise fails, e.g. an item-link instance whose linked path vanished) does not
  silently drop out of the delta computation: `upgrade` prints a warning naming the
  source and pointing at `mind sync` / `mind introspect` to diagnose it, and the run
  is never reported as fully up to date (CLI-64) while any source could not be
  checked, even when no OTHER pending upgrade exists -- an unscannable source is
  itself an actionable condition, not silence.
- `CLI-212` A linked local source (`Source::is_linked`, CLI-27) whose working
  tree has vanished since it was melded (the docs' own `/tmp` recipe with no
  teardown, or a repo directory a user deleted) is a `LinkedSourceGone` error
  naming the source and the missing path, with the exact `mind unmeld <name>`
  remedy, raised by `catalog::scan_source` before a less specific error (a
  convention-scan `InvalidRoot`, or `MindToml::load` silently treating the
  missing directory as "no mind.toml") could obscure the real cause.
- `CLI-213` Listing every source's catalog (`recall`, `probe`, `learn`'s item
  resolution) degrades past a single `LinkedSourceGone` source (CLI-212)
  instead of hard-failing the whole call: the dead source is skipped with a
  warning on stderr, and the sources that DID scan are still shown/searched/
  resolved against. Every OTHER scan failure (a version gate, a bad `--root`, a
  duplicate item, ...) still aborts the whole call exactly as before; this is a
  narrow degradation for the one failure mode a user cannot fix by re-running
  the same command. `meld`'s own scan of the source it just cloned, and
  `dump`'s reconstruction, are unaffected: both still hard-fail on any scan
  error, including this one -- a partial dump is worse than none, and naming a
  specific broken source (rather than merely listing everything) should still
  surface the problem plainly. `introspect` (CLI-210) and `upgrade` (CLI-211)
  already scan per-source independently, so they surface `LinkedSourceGone` as
  their existing `source-scan-failed`/unscannable-source finding with no
  further change.

## hooks

- `CLI-194` `mind hooks run <target>` runs a melded source's hooks -- or a
  named item's hooks -- on demand, outside the meld/learn/forget/upgrade flows, so a
  user can run hooks they earlier skipped (HOOK-21/22/72/83), re-run a hook whose
  effect was later lost (a deleted build output or side effect), or re-run an install
  or uninstall whose prior run failed transiently. Every hook it runs goes through the
  same disclosure and consent prompt as an automatic run (HOOK-100); it never runs a
  hook more silently than meld/learn would, and a required hook's failure or abort is a
  non-zero exit (HOOK-53). `<target>` is a source ref (the source's own hooks,
  HOOK-101) or an item ref `<source>#<item>` (that item's hooks, HOOK-102); a ref that
  matches several sources or items runs each in turn.
- `CLI-195` `mind hooks run` selects the lifecycle event with `--event
  <install|uninstall|build>` (default `install`). `install` and `uninstall` are valid
  for a source or an item target; `build` is valid only for an item target (a source
  has no build hook, HOOK-103). For a source install run, only *pending* install hooks
  run by default (HOOK-55); `--force` also re-runs install hooks already recorded at
  the current commit (HOOK-101). The `--dangerously-skip-install-hook-check` and
  `--dangerously-skip-build-hook-check` flags apply as they do to the automatic flows
  (HOOK-23/74).
- `CLI-228` `mind hooks run --rerun` is a visible alias for `--force` (CLI-195):
  same flag, same field, just a name that reads as "re-run recorded hooks"
  rather than the borrowed `meld --force` clobber sense. Both spellings are
  interchangeable in `--help` and on the command line.
- `CLI-196` `mind hooks list <target>` lists the hooks in effect for a source
  and its installed items -- each hook's event, required/optional flag, command, and,
  for a recorded source install hook, whether it is pending and the commit it last ran
  at -- without running anything (HOOK-104). It is the read-only companion to `hooks
  run` and the detail behind the `recall --sources` hook marker (HOOK-58). Under
  `--json` it answers with a document instead of this text listing (CLI-220).
- `CLI-220` `mind hooks list --json` answers with one JSON document instead of
  the CLI-196 text listing:

  ```json
  {
    "schema": 1,
    "action": "hooks-list",
    "target": "<target as typed>",
    "sources": [
      {
        "source": "<source name>",
        "hooks": [
          {"event": "install|uninstall", "required": true,
           "command": "<run>", "status": "<CLI-196 status text>"}
        ],
        "items": [
          {"item": "<kind:name>", "hooks": [
            {"event": "install|uninstall", "required": true, "command": "<run>"}
          ]}
        ]
      }
    ],
    "items": [
      {"item": "<kind:name>", "source": "<source name>", "hooks": ["..."]}
    ]
  }
  ```

  `sources` is populated when `<target>` resolved as a source (HOOK-104's
  source branch, possibly several under a glob); the top-level `items` is
  populated instead when it resolved as an item ref, one entry per matched
  item, each carrying its own `source`. The two are mutually exclusive (a
  target resolves one way or the other, CLI-194) and whichever is empty is
  omitted rather than serialized as `[]`. That elision applies ONLY to the
  top-level `sources`/`items` pair: a nested `hooks` or `items` array is
  always present, empty when the source declares no hooks or has no installed
  items with hooks. `status` is present only for a
  recorded source install hook (mirrors the text mode's pending/last-ran
  report, HOOK-55); an uninstall hook and every item-level hook carry no
  `status`. A source's nested installed items are objects (an `item` name
  plus its own `hooks` array), not bare strings, so a later addition (e.g. a
  per-item install/uninstall disclosure record) has somewhere to go without a
  breaking shape change.
- `CLI-222` `mind hooks run --json` on a successful run (no `MindError`)
  answers with a count-based result rather than nothing:

  ```json
  {
    "schema": 1,
    "action": "hooks-run",
    "target": "<target as typed>",
    "event": "install|uninstall|build",
    "existed": 0,
    "ran": 0,
    "skipped": 0
  }
  ```

  `existed`/`ran`/`skipped` are the HOOK-107/HOOK-108 tally, accumulated
  across every matched source or item: how many hooks for the selected event
  were considered, how many actually ran, and how many were skipped
  specifically for want of consent. A run with nothing to do (no hooks
  declared, or every install hook already recorded at the current commit)
  still answers with the document, all three counts zero, rather than
  silence. `--event build`'s tally is always zero (HOOK-103's transactional
  reinstall reports success or failure, not a hook-by-hook count). `skipped`
  counts ONLY skips for want of consent, so it is `0` in every printed success
  document: an interactive decline is the user's own choice and is not a
  consent failure (it reports `existed: 1, ran: 0, skipped: 0`), while a
  non-TTY run that could consent to nothing is the HOOK-107 error path below.
  The three counts therefore need not sum to `existed`. On the
  HOOK-107 failure path (`MindError::HooksNotRun`) no success document is
  printed; the CLI-181 error envelope is the one document (CLI-217).

## recall

- `CLI-70` `recall` (no argument, alias: `status`) is a status view of everything `mind` manages:
  each melded source, with its catalog items nested beneath it. It shows both
  installed and not-yet-installed items, so a single `recall` answers "what is
  melded, and what is installed". The `--kind` / `--source` filters narrow the
  items shown (CLI-83).
- `CLI-71` `recall <item>` shows one installed item's detail: description, source,
  commit, hash, store path, and link path(s). The ref is matched against the
  manifest by effective name, honoring a `kind:` prefix and an `owner/repo#`
  source qualifier; an ambiguous bare name is `AmbiguousItem` and one matching
  none is `NotInstalled`.
- `CLI-72` `recall --sources` narrows the status view to the source list only
  (name, url, short commit, alias, install-hook token, description), without the
  nested items.
- `CLI-73` `recall --json` emits the data as JSON on stdout instead of the table:
  the default view emits the sources each with their nested items (carrying the
  installed flag and, when installed, the commit); a lookup emits the single item
  as a plain JSON object (not wrapped); `--sources` emits the source list. Array
  outputs (default view, `--sources`) are wrapped in the envelope introduced by
  CLI-167. An empty registry emits `{"schema": 1, "items": []}`.
- `CLI-74` In the default status view, each item line marks its install state
  inline: an installed item shows that it is installed and its short commit; a
  not-installed item is marked available. Items are grouped under their source, so
  the source a given item comes from is unambiguous.
- `CLI-75` The status view marks an installed item out of date exactly when
  `upgrade` would act on it (LIFE-11): its current source-content hash differs from
  the hash recorded at install (LIFE-15), or its effective name changed (a
  namespace change). The marker is independent of the source commit: a commit that
  advanced without changing an item's content or effective name does NOT mark that
  item, because `upgrade` would report it up to date and the marker must stay
  actionable -- it appears only when `mind upgrade` will change the item. This still
  surfaces drift for a melded local directory (no upstream commit to advance) and
  for a real checkout whose source files were edited in place (commit unchanged,
  content not). The marker points to `mind upgrade` and matches `introspect`'s
  `drifted` finding (CLI-90) and `upgrade`'s pending condition (LIFE-11). It applies
  to `recall` (the default status view and a single-item lookup, CLI-70/71/74) and
  the `probe` non-interactive listing (CLI-80, CLI-81). The marker is a human-view
  concern; the JSON outputs are unchanged.

## probe

`probe` launches the interactive TUI by default (tui.md, TUI-1). The IDs below
define the non-interactive catalog listing, which `probe` prints instead when
`--no-tui` or `--json` is given or stdout is not a TTY (TUI-2).

- `CLI-80` `probe [query]` lists available catalog items (effective name, source,
  one-line description), filtered to those whose effective name contains `query`.
  An empty query lists everything.
- `CLI-81` `probe` marks installed items with a leading `*` and shows each item's
  short content hash.
- `CLI-82` List outputs (`probe`, `recall`) left-align columns padded to the
  widest value in each column, so rows stay aligned regardless of item-name
  length.
- `CLI-83` `probe` and `recall` accept `--kind <skill|agent|rule|tool>` and
  `--source <selector>` filters that narrow the listing, composing with `probe`'s
  substring query. For `recall` they apply to the installed-items listing, not to
  `--sources` or a single-item lookup (use a `kind:` / `owner/repo#` ref there);
  passing them with `--sources` or a single item prints a note that they are
  ignored.
- `CLI-84` `probe --json` emits the rows on stdout instead of the table, wrapped
  in the envelope introduced by CLI-167; each row carries the installed flag,
  kind, effective name, source, content hash, and description.
- `CLI-85` `probe`'s query matches an item whose effective name *or* description
  contains the query, case-insensitively. This supersedes the name-only matching
  of CLI-80 so an item is found by what it does, not only by its name. The
  `--kind` / `--source` filters (CLI-83) still compose with the query.
- `CLI-86` The `probe` / `recall` `--source <selector>` filter (CLI-83) accepts a
  glob (`*`, `?`, `[`), matched against each source's `host/owner/repo` identity
  and its trailing-suffix forms as a plain string (so `*` spans `/`), mirroring
  `unmeld`'s source glob (CLI-28). `--source '*agents'` narrows the listing to
  items from every source whose identity matches. A non-glob value keeps the
  exact/unambiguous-suffix match. Unlike `unmeld`, a multi-source match is the
  normal, non-error case for a filter: every matching source's items are shown,
  with no confirmation. A glob matching no source yields an empty listing, as any
  fully-excluding filter does.

## review

`review` is the author-side counterpart to `introspect`: it validates a source
*before* it is published or melded, surfacing the problems that would otherwise
only appear at meld or install time. It is read-only and installs nothing.

- `CLI-130` `review <target>` validates a source for publishing. `<target>` is a
  local path, a repo spec (the forms accepted by `meld`, CLI-11; cloned to a temp
  area for the check), or the selector of an already-melded source (matched like
  `unmeld`, CLI-20).
- `CLI-214` Between the registry-selector match and repo-spec parsing, `review`
  checks whether `<target>` names an EXISTING directory (resolved against the
  cwd) and, if so, reviews it as that local path regardless of what the string
  would otherwise parse as. This closes a disagreement with `init-source`
  (which takes a bare `PATH` and always treats it as a directory): without
  this check, a two-segment relative path like `skills/greet` parses as
  `owner/repo` (`meld`'s repo-spec grammar recognizes only `/abs/path`,
  `./rel/path`, `../rel/path`, or `file://` as local) and `review`
  shallow-clones it from `github.com` -- a surprise network call for a
  directory the user meant literally. `review` is read-only, so taking this
  branch persists nothing. When `<target>` ALSO parses as a valid remote repo
  spec, `review` prints a note naming which reading it took and the exact
  command to force the remote one instead (`mind review github:<target>`).
  `parse_spec` itself is NOT changed to prefer an existing directory
  globally: `meld` persists an identity and a clone path from it, and a repo
  whose name happens to match a local directory must not silently change
  meaning depending on the caller's cwd (contrast CLI-215, which only warns).
- `CLI-215` `parse_spec`'s bare `owner/repo` / `github:owner/repo` branch
  prints a stderr note, before any clone, when the given spec ALSO names an
  existing directory relative to the cwd, pointing at the `./` form that would
  mean the directory instead (e.g. `mind meld skills/greet` warns and then
  still melds `github.com/skills/greet`, naming `./skills/greet` as the
  local-path alternative). This does not change which reading `parse_spec`
  takes; it is advisory only, so a script relying on the remote reading is
  unaffected, while an interactive user gets a chance to notice the ambiguity
  before the clone runs. `MindError::InvalidRepoSpec`'s message also names the
  local-path forms it previously omitted (`/abs/path`, `./rel/path`,
  `../rel/path`, `file:///abs/path`), matching what `mind review --help` (and
  now CLI-214) already document as accepted.
- `CLI-216` The CLI-215 note is printed at most ONCE per ambiguous spec, by the
  parse that DECIDES which reading gets cloned. A caller that parses a spec to
  answer a question rather than to act on it uses the non-printing
  `parse_spec_quiet`: deriving a registry identity (`instance_name`, the
  already-registered guards, the clone-failure arms that name the entry that
  just failed, the install walks over an already-melded chain), classifying an
  entry (the DSC-93 predicate), or building a different note about a reading
  already taken. The note is about which reading a clone is ABOUT to take, so
  from those sites it is at best a duplicate and at worst a contradiction.
  The motivating case is `review`, which emits at most one note about how it
  read its target: when it takes the CLI-214 local-directory reading, its own
  note (naming that reading and the `mind review github:<target>` escape) is the
  only one, because CLI-215 would tell the user to write `./<target>` to get the
  local reading, which is the reading `review` just took. The same rule bounds
  `meld`: melding a shadowing spec (`mind meld skills/greet` where
  `./skills/greet` exists) notes it exactly once even though the dispatcher, the
  verb, and the recursion each parse it, and a curated `[discover].sources`
  entry that shadows a directory in the CONSUMER's cwd is noted once by the
  recursion that clones it, not again by each identity lookup along the way.
  This is a discipline at the call sites rather than a special case inside
  `parse_spec`, so the parse on the path that really is about to clone a remote
  still notes the ambiguity exactly as CLI-215 requires.
- `CLI-26` `review` with no `<target>` (or an explicit `.`/`./`) validates the
  current directory, so a maintainer can `mind review` in their repo. It is the
  read-only counterpart to `init-source` (init-source.md). `--policy` is the
  separate policy-validation mode and takes no current-directory default.
- `CLI-131` `review` reports, for the source and per item: `mind.toml` parse and
  schema errors (DSC-30, DSC-31), items whose frontmatter yields no description
  (DSC-20), `{{ns:}}` tokens whose referent is not a real sibling (which would be
  `BadReference` at install), and unguarded prose references to siblings under the
  effective prefix (the meld-time heuristic, CLI-14).
- `CLI-132` `review`'s exit status: a hard error (malformed `mind.toml`, an unknown
  item kind, a conflicting `[source]` pin, or an unresolved `{{ns:}}` / path token
  in a markdown item file) exits non-zero; advisory findings only (unguarded
  references, missing descriptions, hardcoded paths, bare tool references, and an
  unresolved `{{ns:}}` / path token in a non-markdown item file) exit zero. It
  changes nothing on disk in either case, except under `--fix` (CLI-138). An
  unresolved `{{ns:}}` token is hard only in a markdown file
  (`namespace::is_markdown`, NS-53): install expands `{{ns:}}` in markdown only,
  so the identical unresolved token in a non-markdown item file (a script, data)
  is dead text that cannot break an install and is downgraded to advisory,
  mirroring the path-token treatment (CLI-135).
- `CLI-133` `review --as <prefix>` evaluates the source under a prospective
  namespace, so token expansion and the unguarded-reference scan are checked as
  they would install under that prefix. With no flag the effective prefix is the
  source's own `[source].prefix` if any, else none.
- `CLI-134` Supplying both `<target>` and `--policy` to `review` is a usage
  error: clap rejects the combination before any logic runs, exits non-zero, and
  prints a conflict diagnostic to stderr.
- `CLI-135` `review` validates an item's path-reference tokens the same way it
  validates `{{ns:}}` (CLI-131): a `{{self}}` / `{{tools:name}}` / `{{path:ref}}`
  token whose referent does not resolve in this source (a `{{tools:}}` naming a
  non-tool or a tool with no entrypoint, a `{{path:}}` miss or cross-kind
  ambiguity) is a hard `bad-reference` finding, which would be a `BadReference` at
  install (tooling.md, TOOL-11/12), in a markdown file (`namespace::is_markdown`,
  NS-53). The identical unresolved token in a non-markdown item file (a script,
  data) is only an advisory `bad-reference` finding, never hard: install never
  expands any token there either, so it is dead text that cannot break an
  install, matching how Check 9 (CLI-136) and Check 11 (CLI-139) already treat a
  non-markdown file. Every bad token is reported, not just the first.
- `CLI-136` `review` reports, as an advisory `hardcoded-path` finding, an item
  file that hardcodes a mind install path that a path token should replace. It
  recognizes the three install layouts (`.mind/store/<kind>/...`, the agent-home
  `.claude/<kinddir>/...`, and `.agents/<kinddir>/...`) under any home-root
  spelling: a leading `~`, `$HOME`, `${HOME}`, or an absolute `/home/<user>` or
  `/Users/<user>` path. When the path maps confidently to a token (the item's own
  dir -> `{{self}}`, a sibling tool's entrypoint -> `{{tools:name}}`, another
  sibling -> `{{path:kind:name}}`) the finding names the suggested token. The
  message reflects what the path resolves to at runtime (CLI-145). Advisory, not
  hard: `--fix` rewrites the confidently-mapped ones, but only in a markdown file
  (CLI-138, NS-54); the identical finding in a non-markdown file is reported and
  left unrewritten. It is
  non-prescriptive about resources an item bundles: keeping a helper in the item's
  own directory, or having an install hook place it at a fixed location and
  referencing it there, are equally valid; the advisory points out only that a
  literal mind install path is fragile, not that tokens are required. (CLI-146
  adds the install-hook-safe note to the message.)
- `CLI-137` `review` reports, as an advisory `bare-tool-reference` finding, a
  sibling tool named in an item's prose without a token. Unlike the unguarded
  sibling-reference scan (CLI-131), which only matters under a prefix, a bare tool
  reference is flagged regardless of prefix, since a tool item is reached by a path
  token, never by name. Non-prescriptive: a source need not adopt the `tool` kind
  at all. Bundling the helper with the item, or installing it to a well-known
  location via an install hook and calling it there, are equally valid; the
  advisory only flags a `tool` item named by bare name where a token would be
  needed. (CLI-146 adds the install-hook-safe note to the message.)
- `CLI-138` `review --fix` rewrites the source in place and is the sole exception
  to `review` being read-only (CLI-132). It applies only to a local-path target;
  a registry selector or a repo spec (whose clone is a discarded temp) refuses
  `--fix` and changes nothing. For each markdown item file (NS-53, NS-54) it
  rewrites confidently recognized hardcoded install paths into the matching
  token (CLI-136), un-wraps misplaced `{{ns:}}` tokens (CLI-139) back to the
  bare name, and templatizes bare sibling names into `{{ns:}}` (the
  `init-source --template` transform, INIT-5), then reports each file it
  changed. A non-markdown item file is never rewritten, since its content never
  expands out of the token form (NS-53): a finding there is still reported by
  the CLI-135..139 checks, which scan every text file regardless of extension,
  it is just left unrewritten (NS-54).
- `CLI-139` `review` flags a misplaced `{{ns:}}` token -- one in a non-prose
  context (NS-24) where name-substitution is wrong. A token inside a fenced code
  block, an inline code span, or adjacent to a path separator is an advisory
  `misplaced-reference` (a name token belongs in prose; code and paths use the
  path tokens, tooling.md). A token in the frontmatter `name:` field is a hard
  `misplaced-reference`: an item must not namespace its own name. This is the dual
  of the unguarded-reference scan (CLI-131): one finds a bare name that should be
  a token, the other a token that should be a bare word. The finding is reported
  in any text file, markdown or not, but `--fix` only un-wraps it in a markdown
  file (CLI-138, NS-54); in a non-markdown file the misplaced token is left as
  written, since it never expanded there either (NS-53).
- `CLI-223` `review` reports, as an advisory `inert-token` finding, every
  `{{...}}` token found in a non-markdown item file, regardless of whether the
  token would resolve: no token family expands outside markdown (NS-53), so a
  token there never reaches install either way, and one that would resolve if
  the file were markdown (e.g. a `{{tools:name}}` naming a real sibling tool,
  in a bundled `.sh`) is otherwise silently left literal and breaks at
  runtime -- CLI-135 only flags an unresolved one, and CLI-139 only covers
  `{{ns:}}`, so a resolvable path/self token in a non-markdown file was
  previously unreported by either. The finding names the file and the
  token(s), states that tokens expand in markdown only, and names the three
  remedies: move the reference into markdown prose, have the script
  self-locate, or list the file in the item's `expand:` frontmatter to expand
  it there (NS-57). Never hard, and `--fix` never rewrites the file (CLI-138,
  NS-54). A token this generic net
  would otherwise re-report is excluded when another check already reported
  the same span for the same file, so a single broken or dead reference draws
  one finding, not two or three: every `{{ns:...}}` token in a non-markdown
  file is excluded (CLI-139's `misplaced-reference` unconditionally covers all
  of them there, whether or not they resolve, which also subsumes what
  CLI-135's downgraded `bad-reference` would report, since that is always an
  `{{ns:}}` token too), and the one path token (`{{tools:}}`/`{{path:}}`/
  `{{self}}`) CLI-135 reported as `bad-reference` for the file, if any, is
  excluded as well. A resolvable path/self token that no other check mentions
  -- the case this finding exists for -- is unaffected and still reported.
- `CLI-226` A file listed in an item's `expand:` frontmatter (NS-57) is treated
  as markdown by every `review` token check: its tokens do expand at install, so
  no `inert-token` finding (CLI-223) is raised for it, an unresolved token in it
  is a hard `bad-reference` (CLI-135) rather than the downgraded dead-text
  advisory a non-markdown file gets, and a `{{ns:}}` in it is not `misplaced`
  (CLI-139). An `expand:` entry naming a file the item does not ship is itself a
  hard `bad-expand` finding, matching the install-time `BadReference` (NS-57), so
  `review` catches a typo before a meld does. `--fix` still never rewrites a
  non-markdown file, `expand:`-listed or not (NS-54).
- `CLI-144` `review` reports, as an advisory `duplicate-tooling` finding, a
  non-markdown helper file whose contents are byte-identical across two or more
  items. The finding names the file and the items that carry it and notes the
  duplicate COULD be shared once as a `tool` referenced by a path token
  (`{{tools:name}}` / `{{path:}}`), while stating that keeping the per-item copies
  is equally valid: a source that namespaces its items and deliberately silos each
  helper with the skill that uses it is not doing anything wrong, and adopting a
  tool means buying into mind's token references. The message is non-prescriptive
  (it presents both as acceptable), not a defect to fix. Markdown is excluded (it
  is prose, not tooling) and empty files are ignored. Advisory only, and `--fix`
  never touches it: adopting a shared tool is an opt-in structural change the
  author re-references deliberately.
- `CLI-145` The `hardcoded-path` advisory (CLI-136) classifies the reference by
  what it resolves to at runtime, because the cases differ in severity. A skill
  that hardcodes its OWN resources (the `{{self}}` case) works as written but
  assumes every install lands at that exact agent-home path; it breaks once a
  prefix renames the item or a second home is configured, and `{{self}}`
  generalizes it (fragile, not broken). A reference to a
  `tool` is broken regardless of prefix: a tool is store-only and never linked
  into an agent home (tooling.md TOOL-3), so the hardcoded location does not
  exist. Any other hardcoded item path is reached by a token, not an install
  path. The advisory's message states which of the three cases it is.
- `CLI-146` The `hardcoded-path` (CLI-136) and `bare-tool-reference` (CLI-137)
  advisory messages note that a location the source's own install hook populates
  is safe: when a `[source].install` or `[[hooks]]` step installs the resource or
  tool to that path, referencing it there is intentional, not a defect. The
  findings stay advisory and are still emitted regardless of prefix (CLI-137), so
  the maintainer keeps the visibility, but the message no longer reads as a flaw
  for a source that deliberately installs to a fixed location. The `{{self}}`
  self-resource case (CLI-145), which a hook does not populate, keeps its
  fragile-not-broken wording.
- `CLI-190` `review` reports, as an advisory `unshipped-tooling` finding, a `tool`
  whose entrypoint resolves in the author's working tree only through a file git
  does not track, so the tool ships without that file and `{{tools:name}}`
  references to it break on a clone/remote meld though they resolve locally. It
  checks the resolved entrypoint script and, when present, the tool's `TOOL.md`
  (which declares the `bin`), and names each untracked file. It applies only to a
  local working-tree target that is a git repository: a remote/clone target
  already contains only what ships, so the discrepancy cannot arise there (a
  genuinely-missing entrypoint is a plain `bad-reference`, CLI-135), and a non-repo
  local dir cannot be assessed for shippability. A tool with a per-item build hook
  (HOOK-70) is skipped -- its entrypoint is generated in staging, so its absence
  from git is intentional. Advisory, and `--fix` never touches it: committing the
  file or adding a build hook is the author's decision. This is the working-tree
  counterpart of the install-time entrypoint `BadReference` (tooling.md TOOL-17),
  caught before a push rather than at a consumer's meld.
- `CLI-191` `review` extends the `unshipped-tooling` advisory (CLI-190) to any
  item's bundled files, not just a tool's entrypoint: a file addressed by a
  `{{self}}/...` or `{{path:[kind:]name}}/...` reference (tooling.md TOOL-10/TOOL-11)
  that is present in the author's working tree but git does not track is flagged,
  because the whole item directory is copied on a local meld (picking up the
  gitignored file) while a clone contains only tracked files, so the reference
  resolves locally and breaks on a remote meld. It names the untracked file and
  the token that addresses it. A token with no `/`-path remainder addresses the
  item directory itself, not a specific file, and is not checked. Local
  working-tree git target only and `--fix` never touches it, exactly as CLI-190.
  A file that does not exist at all is a plain `bad-reference` handled elsewhere,
  not this advisory.
- `CLI-192` `review` reports, as an advisory `ns-tool-reference` finding, a
  `{{ns:name}}` reference (namespacing.md) whose only matching sibling is a `tool`.
  A tool is store-only (tooling.md TOOL-3), reached by `{{tools:name}}` for its
  entrypoint or `{{path:tool:name}}` for its directory; its bare name is not a
  runnable path and is never linked into an agent home, so a `{{ns:tool}}` expands
  to a name that resolves to nothing at runtime -- the silent failure mode of a
  `{{tools:}}` mistyped or "fixed" into a `{{ns:}}`. It fires only when the name
  matches no non-tool sibling, so a genuine skill/agent/rule reference that merely
  shares a name with a tool is not flagged. It applies to any target (a
  wrong-token content smell, not a shippability check).
- `CLI-193` `review` reports, as an advisory `unshipped-tooling` finding, an
  authoritative `mind.toml` (one declaring `[[items]]` or `[discover]` globs, so it
  turns off convention scanning) that git does not track. A linked local source
  reads even an untracked/gitignored `mind.toml` live (CLI-27), so its declared
  inventory, prefix, and bins apply to the author's working tree yet are absent
  from a clone, which falls back to convention discovery or a different item set --
  the source-wide form of the working-tree-vs-clone discrepancy. A metadata-only
  `[source]` block changes no discovery and is not flagged. Local working-tree git
  target only.
- `CLI-219` `mind review --json` (either mode: a `<target>` source, or
  `--policy <path>`) answers with one JSON document instead of the CLI-131/132
  text findings:

  ```json
  {
    "schema": 1,
    "action": "review",
    "outcome": "clean|advisory|failed",
    "hard": [{"kind": "<slug>", "message": "<text>"}],
    "advisory": [{"kind": "<slug>", "message": "<text>"}],
    "fixed": ["<path>"]
  }
  ```

  `outcome` is `"clean"` when both `hard` and `advisory` are empty,
  `"advisory"` when only `advisory` findings exist, and `"failed"` when any
  hard finding is present (the last value appears only inside the CLI-221
  `details` member, never in a printed success document); `kind` is the same
  machine-stable finding tag the text mode prints in `error [kind]: ...` /
  `advisory [kind]: ...` (e.g. `bad-reference`, `missing-description`). `fixed`
  lists the files `--fix` rewrote (CLI-138), empty otherwise. When any hard
  finding is present, `review` still exits non-zero (CLI-132 is unconditional):
  the document above is not printed as a success envelope in that case, but
  recorded and folded into the CLI-181 error envelope's `details` member
  (CLI-221) instead, so a machine caller sees exactly what failed without
  scraping stderr's `error [kind]: message` lines. The envelope's own `kind`
  (CLI-182) is the fixed slug `review-failed` (`MindError::ReviewFailed`), so
  a machine caller can branch on the failure before even looking at
  `details`. `review --policy`'s document
  uses the same shape (`hard`/`advisory` only; `fixed` is always empty, since
  `--policy` has no `--fix` mode).

## introspect

- `CLI-90` `introspect` reports: sources with no clone or never synced, installed
  items whose links are missing, items no longer present upstream, items whose
  namespace changed, and items whose source content drifted. It reports a clean
  summary when there are no issues.
- `CLI-91` `introspect --fix` repairs what it can without changing versions: it
  recreates missing link(s) for installed items from their file registry
  (re-linking the existing store copy). If the store copy itself is gone the link
  is left reported, not recreated. Drifted or renamed items are still left to
  `upgrade`.
- `CLI-92` `introspect --json` emits the findings as JSON on stdout: an object
  with an `issues` array (each carrying a stable `kind` tag, a `target`, and a
  `message`) plus the source and item counts. An empty `issues` array means clean.
- `CLI-189` The `introspect --json` output includes a top-level `"schema": 1`
  field, matching the envelope version used by other `--json` verbs (CLI-167).
  The full shape is `{"schema": 1, "issues": [...], "sources": N, "items": N}`
  where `sources` and `items` are integer counts, not arrays. This field is
  additive; existing consumers keying on `issues`, `sources`, or `items` are
  unaffected.
- `CLI-210` A source `introspect` cannot scan (its clone is missing, or scanning
  it otherwise fails, e.g. an item-link instance whose linked path vanished) does
  not abort the run: `introspect` scans each source independently, reports the
  failure as a `source-scan-failed` issue naming the source, and completes with
  the findings computed from the sources that DID scan successfully (CLI-90/92).

## evolve

`evolve` upgrades the `mind` executable itself (distinct from `upgrade`, which
upgrades installed items, and `sync`, which refreshes sources). It uses the same
native curl/wget downloader as `resources/install.sh` and resolves the same
release artifacts as the install script and the Homebrew formula.

- `CLI-140` `evolve` compares the running version against the latest published
  release. With nothing newer it reports up to date and changes nothing. With a
  newer release it replaces the running executable in place with the release binary
  for the current platform. The target passed to `decision()` is not a plain
  dotted-numeric string: it is the release tag `evolve` resolved (the latest
  `releases/latest` tag, or an explicit `--to`), validated by
  `mindfile::is_plausible_release_tag` -- not `is_plausible_version`, which still
  governs `min-mind-version` and policy version pins (STO-76) -- before
  `decision()` ever sees it, so it may itself carry a semver prerelease/build
  suffix: `evolve --to 1.2.3-rc1` is accepted. A prerelease running version (a
  version string carrying a `-suffix`, e.g. `0.23.1-dev` or the dotted
  `1.0.0-rc.2` -- this repo's own releases never carry one, but a fork's or
  packager's build might) is treated as strictly below a release target of the
  same numeric version: `evolve` on a `0.23.1-dev` build offers the `0.23.1`
  release rather than reporting up to date, and an explicit `--to 0.23.1` onto
  its own base is an update, not a no-op. This tie-break requires the target to
  NOT itself be a prerelease (the `!is_prerelease(target)` guard at
  `decision()`'s numeric-tie branch): a target that is itself a prerelease is
  excluded from the tie-break and falls through to the ordinary numeric
  comparison instead, rather than being offered as an update onto another
  prerelease at the same numeric version.
  The numeric comparison strips the `-suffix` from the WHOLE version string
  before splitting on `.` (not per dotted component), so a dotted prerelease
  like `1.0.0-rc.2` parses as `[1, 0, 0]` and ties with `1.0.0` in both
  directions, rather than a per-component strip mis-parsing it as `[1, 0, 0,
  2]` (reading numerically ABOVE the release it should tie with, which would
  report up to date instead of offering the update). A numerically newer
  prerelease (`0.24.0-dev` vs release `0.23.1`) is unaffected: see `CLI-147`,
  which governs the opposite direction (an explicit `--to` strictly below the
  running version) and would otherwise read as the wrong case for a newer
  prerelease build.
- `CLI-141` Unless `--yes` is given, `evolve` prompts `[y/N]` (default No, EOF
  counts as No) before replacing the binary, mirroring `upgrade` (CLI-60). `--check`
  reports the latest available version and whether an update is pending, then exits
  without downloading or replacing anything.
- `CLI-142` The release artifact is selected exactly as the install script and the
  Homebrew formula select it (`mind-<version>-<target>.tar.gz` from the GitHub
  release for the running platform), so every install path resolves the same
  binary. A platform with no published artifact is an error and nothing is changed.
- `CLI-143` The replacement is atomic: the new binary is downloaded and verified,
  then swapped for the running executable, so any failure leaves the existing
  binary intact. `evolve` replaces whatever binary it runs from and does not
  detect or coordinate with a package manager: there is no Homebrew (or other)
  special case in the code, and a Cellar binary is user-writable on macOS, so
  `evolve` will happily replace a brew-managed `mind`, which the next
  `brew upgrade` then replaces again. Recommending `brew upgrade` for a
  brew-managed install is install-path guidance, not behavior, and lives in
  docs/src/install.md. A target path that is not writable is
  `TargetNotWritable` and nothing is changed.
- `CLI-147` `evolve` never downgrades the binary. When `--to V` is given
  explicitly and V is strictly below the running version, `evolve` exits 0 without
  downloading anything and reports that the pinned version is below the running
  version (e.g. "pinned 0.1.0 is below the running 0.3.0; not downgrading"). This
  is distinct from the "up to date" message, which applies when V equals the running
  version or when no `--to` is given and the running version is already current.
  `--check` surfaces the same message. Under `--json`, the outcome is
  `"not-downgrading"` rather than `"up-to-date"`, so callers can distinguish the
  two cases.
- `CLI-229` `evolve`'s pin-a-version flag is `--to <VERSION>`.
  `--version <VERSION>` (the original spelling) is kept as a hidden alias: on
  every other verb `--version`/`-V` is the global flag that prints the running
  `mind` version and exits, so an arg of the same name taking a value on
  `evolve` read as a collision even though `evolve` disables the global flag
  for itself (`disable_version_flag`) and never actually conflicted at the
  parser level.

## config

- `CLI-110` `config show` creates the config if absent (STO-15), then prints the
  config file path and its key/value pairs (`lobes`, with the default shown when
  unset). It also notes when `MIND_AGENT_HOMES` is set and overrides `lobes`.
- `CLI-111` `config lobes list` lists the configured agent homes, or the default
  home when none are configured. `target` was formerly a visible alias of `lobes`;
  as of CLI-172 it is removed and `config target` is a usage error.
- `CLI-112` `config lobes add <path>` appends an agent home to `config.toml`,
  creating the file if needed; adding one already present is a no-op.
- `CLI-113` `config lobes remove <path>` drops a configured agent home; a path
  that is not configured is an error (`UnknownLobe`).

`config lobes add` also accepts `--preset <name>` to add a non-Claude harness
home with its canonical path and `kinds` filter in one step, and `config lobes
detect` scans the machine for known harness directories and offers to add the
matching presets (opt-in; nothing is added without confirmation). Both are
covered by HARN-4 and HARN-5; see harness-lobes.md for the preset names, paths,
and per-harness `kinds` defaults.

- `CLI-198` `link-project [dir] [--preset <name>] [--subdir <rel>] [--snapshot]
  [--force]` is a shorthand for `config lobes add` aimed at a project directory.
  `dir` defaults to cwd; `--preset` defaults to `windsurf`. It is subject to the
  same HARN-7 backfill contract and the same HARN-9 claude_home preservation as
  `config lobes add`. For a managed add, gitignore guidance is printed (the skills
  dir contains symlinks into `~/.mind/store`). `--snapshot` writes frozen real-file
  copies instead of registering a lobe (HARN-12). The command takes the exclusive
  lock (STO-41).
- `CLI-199` `config lobes add` is extended with three new optional flags: `--subdir
  <REL>` (the lobe path is `base/<REL>`, `kinds = [skill]`; conflicts with
  `--preset`), `--snapshot` (write frozen copies, no config entry; HARN-12), and
  `--force` (overwrite a colliding target in snapshot mode). `--preset` no longer
  conflicts with the positional base path: `config lobes add <dir> --preset
  windsurf` resolves the lobe as `<dir>/.windsurf`. `config lobes remove` gains
  `--snapshot` (convert symlinks to frozen copies before dropping the config entry;
  HARN-12).

## completions / man

- `CLI-120` `completions <shell>` writes a shell completion script for the named
  shell (bash, zsh, fish, elvish, powershell) to stdout, generated from the
  command tree.
- `CLI-121` `man` writes the roff man page for `mind` to stdout, generated from
  the command tree.

## Output and global flags

- `CLI-150` `--json`, `--yes`, and `--ascii` are global flags accepted before or
  after the verb. They apply uniformly to every command: the parser resolves them
  at the top level so no verb needs to declare them individually, and a flag given
  in any position (e.g. `mind --json recall` or `mind recall --json`) is
  equivalent.

- `CLI-151` The color/Unicode capability gate is ON when ALL of the following hold:
  stdout is a TTY; the locale is UTF-8 (the first of `LC_ALL`, `LC_CTYPE`, `LANG`
  that is set contains the substring `UTF-8` or `utf8`, case-insensitively); the
  environment variable `NO_COLOR` is unset; the `--json` flag is not in
  effect; and the `--ascii` flag is not in effect. An unset locale (none of the
  three variables is set) is treated as non-UTF-8. When the gate is OFF, all output
  is plain ASCII with no ANSI escape sequences. (`NO_COLOR` set to an empty string
  still forces the gate OFF, same as any other value: see CLI-154.)

- `CLI-152` When the capability gate (CLI-151) is ON, output uses ANSI color and
  Unicode glyphs with these semantics: green = installed / ok; yellow = warning /
  drift / removed-upstream / installed-but-stale; red = error; dim = available /
  inactive. When the gate
  is OFF, output uses a plain-ASCII fallback for every glyph and no color escapes.
  The ASCII fallback replaces each glyph with a visually equivalent ASCII character
  or short string (e.g. `+` for installed, `^` for installed-but-stale, `!` for
  warning, `x` for error, `-` for available), so all information is preserved
  without terminal support.

- `CLI-153` Every mutating verb (`meld`, `learn`, `forget`, `sync`, `upgrade`,
  `unmeld`, and `config lobes add`/`remove`) emits a structured JSON result object
  on stdout under `--json` and writes nothing else on stdout. The stable fields of
  this object are:

  ```json
  {
    "action":  "<verb>",
    "target":  "<item-or-source ref>",
    "outcome": "<short verb-specific token; see below>"
  }
  ```

  `action` is the CLI verb (e.g. `"learn"`, `"forget"`, `"meld"`); `config lobes
  add`/`remove` report `action` as `"lobe-add"`/`"lobe-remove"`. `target` is the
  effective name of the item or source the verb acted on (e.g. `"skill:review"`,
  `"github.com/owner/repo"`). `outcome` is a short token describing what the verb
  did. The tokens by verb are: `meld` -> `"melded"`, or `"already-melded"` when the
  source was already registered with nothing new to install; `learn` -> `"installed"`,
  `"up-to-date"` (already installed), or `"dry-run"` (`--dry-run`); `forget` and
  `unmeld` -> `"removed"`, with `unmeld --unlink-only` -> `"unlinked"`; `sync` ->
  `"synced"`, or `"no-op"` when there are no sources; `upgrade` -> `"upgraded"`,
  `"renamed"`, or `"up-to-date"`; `absorb` -> `"absorbed"`; `config lobes add`/`remove`
  -> `"added"`/`"removed"`, or `"no-op"` when the lobe was already in the desired
  state. `"up-to-date"` means the verb completed successfully but every item was
  already at the requested state; `"no-op"` means it completed successfully but had
  nothing to act on. A verb MAY add extra fields where it
  genuinely returns more data (for example, `learn` MAY include an `"installed"`
  array listing the effective names of all items installed in that call, including
  dependency-closure items). The read-only verbs (`recall`, `probe`, `introspect`)
  keep their existing JSON shapes (CLI-73, CLI-84, CLI-92) and are not affected by
  CLI-153. `absorb` is also a mutating verb covered by CLI-153; see ABS-11 for its
  specific extra field. `hooks run` is a mutating verb too (HOOK-101/HOOK-103) but
  its result is a count-based tally rather than an `action`/`target`/`outcome`
  object; see CLI-222 for its distinct shape.

- `CLI-217` Under `--json`, stdout carries exactly one JSON document and nothing
  else, and that document answers the verb the user INVOKED. Two failure modes
  are ruled out, and both are enforced structurally rather than by a rule each
  call site has to remember:

  1. *Anything that is not the document.* An advisory note (`note: lobe '...' is
     unreachable`, `note: skipped install hook ...`), a warning about a step that
     failed without stopping the verb (HARN-17's unlinkable backfill target), a
     progress line (`running install hook '...' for ...`), and the output of a
     hook mind ran on the source's behalf all belong on stderr while `--json` is
     in effect. Printed ahead of the JSON they leave stdout holding prose
     followed by an object, which no longer parses; a hook's output is the worst
     case, since it is arbitrary text chosen by the source author and could
     otherwise forge a result envelope. Mechanically, a verb covered by this
     statement runs with the process's stdout pointed at stderr for its whole
     duration, so no `println!` on any path -- and no child process that inherits
     stdout -- can reach stdout at all. Notes are not dropped: they stay visible
     on stderr. `render::note` / `render::warn` remain the way to write a line
     that belongs on stdout in text mode (byte for byte what a `println!` would
     have written) and on stderr under `--json`.
  2. *A second document.* A verb that performs another verb's work internally
     (`meld` and `sync` installing items through `learn`, `sync --upgrade`
     running the upgrade pass) must fold that outcome into its own CLI-153
     object rather than letting the inner verb emit one: `mind meld <repo>
     --json` answers with a `meld` object whose `installed` array names what the
     install step did, `mind sync --json` with one `sync` object covering the
     POL-58 provisioning pass and the `--upgrade` pass, never N+1 objects. This
     holds on the already-melded (re-meld) branch as well as the fresh one, and
     a re-meld that installs nothing still answers with its object rather than
     with silence -- a CLI-153 mutating verb always tells a machine caller what
     happened. On failure the CLI-181 error envelope REPLACES whatever a verb
     recorded before failing, so stdout is one document either way.

  The statement binds the verbs that answer `--json` with a document. It does
  not apply where another statement makes stdout a different product: `dump`
  writes TOML (DUMP-9), `completions` and `man` write their script and roff
  page, and `evolve` writes its own document from `selfupdate.rs` on a path
  that never goes through this module. `init-source` defines no JSON output at
  all, so it prints its human text on stdout in every mode. CLI-218 states the
  exclusion list this paragraph draws from as a closed boundary rather than an
  enumeration; `review` (CLI-219) and `hooks list` (CLI-220) are NOT
  exclusions, and both answer `--json` with a document like every other verb.

- `CLI-218` `--json` is universal: every verb answers it with exactly one JSON
  document on stdout (CLI-217), except a closed, named exclusion list -- the
  boundary is a rule, not an enumeration of what a given release happens to
  implement. The excluded verbs, and why each is excluded: `dump` writes TOML
  by design (DUMP-9); `completions` and `man` print the artifact itself (a
  shell script, a roff page) as their entire output; `evolve` writes its own
  result document from `selfupdate.rs` rather than through the CLI-153/CLI-217
  machinery (its document IS JSON, just emitted on a separate path); and
  `init-source` is a maintainer scaffolder that edits the target repo in place
  and has no JSON result to offer. Every other verb answers with a document,
  including `review` (CLI-219) and `hooks list` (CLI-220), which for earlier
  releases had none. A verb added later that is neither given a JSON shape nor
  added to this exclusion list is a bug, not a silent gap: the boundary is
  meant to be closed at every point in time, not merely today.

- `CLI-154` `NO_COLOR` being set (to any value, including empty) forces the
  capability gate (CLI-151) OFF regardless of TTY or locale. A non-UTF-8 locale or
  an unset locale also forces the gate OFF even on a TTY. `--ascii` forces the gate
  OFF regardless of `NO_COLOR`, locale, or TTY state. These conditions are
  independent: any one of them alone is sufficient to disable color and Unicode
  glyphs.

- `CLI-155` In the `recall` status views (the default forest and `recall <source>`),
  an installed-but-out-of-date item (CLI-75) uses a distinct left-edge marker from a
  current install: the stale glyph (Unicode `↑` in yellow, ASCII `^`) rather than the
  installed glyph (Unicode `✓` in green, ASCII `+`). This marks a third state
  between installed-and-current and not-installed, so the out-of-date condition is
  visible from the marker alone and not only from the trailing `(outdated)` text.
  The marker is a human-view concern; the JSON output is unchanged.

- `CLI-157` `learn` when every item in the requested set is already installed (the
  closure is empty after DEP-23 exclusion, with no dry-run in effect) prints
  "already installed; nothing to do" to stdout and under `--json` emits a single
  result object with `outcome: "up-to-date"` (distinct from `"installed"`, which
  requires at least one item was actually installed). Exit 0 in both cases.

- `CLI-162` `--verbose` (short `-v`) is a global flag accepted before or after the
  verb, resolved at the top level like `--json`, `--yes`, and `--ascii` (CLI-150).
  It enables extra advisory output that is otherwise suppressed: the unguarded-
  reference warning emitted during `meld` when a prefix is in effect (CLI-14,
  NS-20). It does not affect the color/Unicode capability gate (CLI-151).

- `CLI-163` The short flag `-n` is reserved for `--dry-run` on `learn` (CLI-32),
  which already owned it. As a consequence, `--namespace` on `meld`, `review`, and
  `init-source` moves to short `-N` (uppercase). No other short is assigned to
  `--namespace`; the long form and `-N` are the two accepted spellings.

- `CLI-164` `probe --no-tui` is long-only; its former short `-n` (TUI-3) is removed
  to free `-n` globally (CLI-163). See also TUI-54.

- `CLI-165` `meld --register-only` replaces `--link-only` as the canonical name for
  "register the source without installing its items" (CLI-23). `--link-only` is
  retained as a hidden deprecated alias and continues to work; it does not appear in
  `--help` output.

- `CLI-166` `unmeld --keep-items` replaces `--unlink-only` as the canonical name for
  "remove the source but leave its installed items in place" (CLI-22).
  `--unlink-only` is retained as a hidden deprecated alias.

- `CLI-167` `probe --json` and `recall --json` (default view, `--sources`) wrap their
  array output in a versioned envelope:

  ```json
  {"schema": 1, "items": [...]}
  ```

  The top-level `"schema"` field is a monotonically increasing integer; readers
  should treat an absent field as `1`. Single-item `recall <item> --json` is already
  a plain JSON object and is not wrapped. This supersedes the bare-array form of
  CLI-73 and CLI-84.

- `CLI-168` The mutation result envelope (CLI-153) gains a top-level `"schema": 1`
  field. Existing stable fields (`action`, `target`, `outcome`, and verb-specific
  extras) are unchanged; `"schema"` is additive.

- `CLI-169` `upgrade` fetches each involved source before computing deltas (syncs
  first by default). The sync uses the same per-source resilience as CLI-54:
  individual source failures are reported and skipped; the upgrade pass runs on
  the sources that did succeed. Pass `--no-sync` to skip the fetch and compute
  deltas from the current (potentially stale) clone. `sync --upgrade` continues to
  work but its `--upgrade` flag is noted as deprecated in help text; prefer
  `upgrade` (which now syncs) or `upgrade --no-sync` (to match the old
  `sync --upgrade` behavior of explicit sync then upgrade).

- `CLI-170` `MIND_DEFAULT_LOBE` is the primary environment variable for setting the
  default agent home (lobe). When set, it takes precedence over `CLAUDE_HOME`.
  `CLAUDE_HOME` is kept as a documented legacy fallback: if `MIND_DEFAULT_LOBE` is
  unset, `CLAUDE_HOME` is used; if neither is set, the default is `~/.claude`.

- `CLI-171` The `absorb-to` config key in `~/.mind/config.toml` is the canonical
  (kebab-case) spelling. The underscore form `absorb_to` is accepted as a
  backwards-compatible alias during parsing; new writes always emit `absorb-to`.

- `CLI-172` Visible aliases added: `add` for `meld`, `install` for `learn`,
  `uninstall` for `forget`, `update` for `sync`, `search` for `probe`, `list` for
  `recall`, `doctor` for `introspect`, `self-update` for `evolve`. Former aliases
  `detach` (for `unmeld`) and `target` (for `config lobes`) are removed and are
  now usage errors; `unlearn` (for `forget`) and `status` (for `recall`) remain
  visible.

- `CLI-173` The one-line help for `meld` reflects that melding installs items by
  default (interactive prompt): "Meld with a source repo and install its items."

- `CLI-174` The long help (`--help` body) for `unmeld` leads with: "Unmelds a
  source and uninstalls every item the source installed; use `--keep-items` to keep
  them."

## Exit status

- `CLI-100` A command that completes its work exits 0. Any `MindError` is printed
  to stderr (with its source chain) and exits non-zero.
- `CLI-175` The exit-code contract: 0 for success, 1 for a runtime error
  (`MindError`), and 2 for a usage error (clap parse failure). Clap handles code 2
  automatically. Code 1 comes from `ExitCode::FAILURE` in `main`.
- `CLI-181` Under `--json`, when a `MindError` occurs the process emits a JSON
  error envelope on stdout and exits with code 1 (unchanged). The envelope shape
  is `{"schema": 1, "error": {"kind": "<slug>", "message": "<display-text>"}}`.
  The `schema` field matches the success-envelope version (currently 1). The
  `message` field is the full `Display` text of the error. Nothing is written to
  stderr in this path. In non-json mode the existing behavior is unchanged: the
  error message and its source chain are printed to stderr and the process exits 1.
- `CLI-182` The `kind` field in the JSON error envelope (CLI-181) is a stable
  kebab-case slug assigned once per `MindError` variant and never changed. Scripts
  may branch on `kind` to handle specific failures. Example slugs: `ItemNotFound`
  -> `"item-not-found"`, `DigestMismatch` -> `"digest-mismatch"`,
  `SelfUpdatePolicy` -> `"self-update-policy"`, `ReviewFailed` ->
  `"review-failed"` (the `review --json` hard-failure kind, CLI-219). The slug
  set is exhaustive: every variant has exactly one slug.
- `CLI-221` The CLI-181 error envelope carries an optional `details` member when
  the failing verb recorded structured findings before returning its error (a
  hard-finding `review` under `--json`, CLI-219, is the first user). `details`
  is whatever that verb recorded, verbatim; its shape is verb-specific and
  documented at the verb's own JSON-shape entry (e.g. CLI-219), not here.
  Absent entirely -- not `null` -- for the ordinary case of a `MindError` with
  no recorded findings. This gives a machine caller the full structured
  picture on the non-zero exit, not just the CLI-182 `kind` slug and a prose
  `message`.
- `CLI-183` Clap argument-parse failures (exit code 2) are not JSON-enveloped.
  They occur before flag parsing settles and are rendered by clap as plain text to
  stderr. Scripts may treat exit 2 as a usage error without inspecting stdout.
  Only the post-parse `MindError` path (exit 1) emits the envelope.

