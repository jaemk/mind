# CLI

The `mind` command surface. Verbs use a knowledge metaphor.

| command | role |
|---------|------|
| `probe [query] [--no-tui]` | interactive browser (default); catalog listing with `--no-tui`/`--json` |
| `meld [<repo>] [--link-only] [--yes] [--as <prefix>] [--root <dir>] [--follow-branch\|--pin-tag\|--pin-ref <ref>]` | connect a source (default `.`), then install its items |
| `init-source [<path>] [--template]` | scaffold `mind.toml` + detect references (maintainer) |
| `unmeld <name> [--unlink-only] [--yes] [--uninstall-hook <cmd>] [--dangerously-skip-install-hook-check]` (alias: `detach`) | disconnect a source and forget its items (`--unlink-only` keeps them) |
| `learn <item> [--dangerously-skip-install-hook-check]` | install |
| `forget <item> [--dangerously-skip-install-hook-check]` (alias: `unlearn`) | uninstall |
| `sync` | refresh sources |
| `upgrade [--yes] [item]` | upgrade installed |
| `recall [item] [--sources] [--kind K] [--source S] [--json]` | status: sources with their items (install state marked); `--sources` narrows to sources |
| `review [<target>] [--as <prefix>]` (default `.`) / `review --policy <path>` | validate a source / a policy file |
| `introspect` | diagnose |
| `evolve [--check] [--yes] [--version <v>]` | update the `mind` binary itself |
| `config show` / `config lobes ...` | view/edit config |
| `completions <shell>` | print a shell completion script |
| `man` | print the roff man page |

## Item refs

- `CLI-1` An item ref is one of: `name`, `skill:name`, `agent:name`, `rule:name`,
  or `owner/repo#name` (source-qualified). `name` is the effective (installed)
  name, so a namespaced item is referenced as `<prefix>-<bare>`.
- `CLI-2` A bare `name` matches across kinds; a `kind:` prefix narrows to one kind.
- `CLI-3` A ref that matches no catalog item is an error (`ItemNotFound`). A ref
  that matches more than one is an error (`AmbiguousItem`) listing the candidates.
- `CLI-4` A malformed ref is an error (`InvalidItemRef`).
- `CLI-5` The source qualifier in `owner/repo#name` matches a source by its full
  `host/owner/repo` identity or any trailing component suffix (`repo`,
  `owner/repo`, `host/owner/repo`). An ambiguous suffix leaves multiple matches
  and resolves to `AmbiguousItem`.

## meld

- `CLI-10` `meld <repo>` parses the repo spec, clones it under the sources tree,
  records the current commit, reads `[source].description` from `mind.toml` if
  present, and adds it to the registry.
- `CLI-11` Accepted repo specs: `owner/repo` and `github:owner/repo` (github.com),
  a full git URL (`https://host/owner/repo[.git]`), an SSH form
  (`git@host:owner/repo[.git]`), and a local path or `file://` URL. A spec that
  parses to none of these is an error (`InvalidRepoSpec`).
- `CLI-12` Re-melding a repo whose source name is already registered is not an
  error and does not re-clone or re-register. It ensures the source's items are
  installed: if any are missing it installs them (the default-install flow,
  CLI-23, honoring `--yes` and the non-TTY note). When nothing remains to install
  (or with `--link-only`) it prints a status of the source's items: each item's
  effective name, whether it is installed, and the commit it was installed from,
  flagging items whose commit lags the source. Items are matched by stable
  identity (source, kind, bare name), so a prefix change does not lose them.
- `CLI-13` `--as <prefix>` sets the source's namespace, overriding any
  `[source].prefix`. It is persisted and is not changed by `sync`. Given on a
  re-meld of an already-melded source (CLI-12), `--as` changes the source's
  prefix and renames its installed items to the new effective names (the upgrade
  rename, matched by stable identity), re-expanding intra-source `{{ns:}}`
  references to those names. `--as ''` removes the prefix.
- `CLI-14` After melding, if a prefix is in effect, unguarded prose references to
  siblings are reported as warnings (see namespacing.md). Warnings do not fail
  the command.
- `CLI-15` If the melded repo's `mind.toml` lists `[discover].sources`, each is
  melded recursively (see DSC-38), so one `meld` can pull in a curated set. When
  more than one source is added, `meld` reports the total count.
- `CLI-16` `meld --root <dir>` (repeatable) sets the source's scan roots,
  overriding any `[source].roots` (DSC-51). The roots are persisted on the source
  (STO-17). A root that is not a directory in the clone is `InvalidRoot`.
- `CLI-17` `meld` accepts at most one of `--follow-branch <branch>`,
  `--pin-tag <tag>`, `--pin-ref <commit>`; supplying more than one is
  `ConflictingPin`. The chosen pin is persisted on the source (STO-18). With none
  given, the source's `[source]` pin directive (DSC-41) applies, else the default
  is `--follow-branch` tracking the remote default branch.
- `CLI-18` `meld` clones at the pinned point: `--pin-tag` / `--pin-ref` check out
  that tag / commit; `--follow-branch` tracks the named branch (default the remote
  default branch). The recorded commit is the resolved HEAD of that point. A pin
  that does not resolve in the remote is a `Git` error and nothing is registered.
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
  it is reversible with `forget`. `--link-only` stops at registering the
  source; its items remain available to `learn` later. `--yes` installs without
  prompting, including in a non-TTY context; without `--yes` a non-TTY `meld`
  registers only and prints how to install later (mirroring the install-hook
  non-TTY behavior, HOOK-22). Only the top-level source is offered (a curated
  super-source's nested sources are not auto-installed), already-installed items
  are skipped (DEP-23), and a source install hook is still handled by its own
  prompt during the meld (HOOK-20).
- `CLI-24` When a source declares `[source].prefix` and no `--as` was given, an
  interactive `meld` prompts whether to namespace its items under that prefix:
  accept it, type a different prefix, or choose none. The prompt previews the
  resulting installed names under the declared prefix (e.g. `skill:jk-foo`) so the
  effect is visible before choosing. The choice becomes the source alias and
  applies to the scan and the install (`<prefix>-<name>`). A non-interactive meld
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
  sync error (CLI-54). A pinned local source (`--follow-branch`/`--pin-tag`/
  `--pin-ref` or a `[source]` directive) is instead cloned as a snapshot at the
  pin, so pinning still works.

## unmeld

- `CLI-20` `unmeld <name>` (alias: `detach`) removes the source's clone and
  registry entry. `name` is the full `host/owner/repo` or an unambiguous trailing
  suffix (e.g. `repo` or `owner/repo`); an unknown name is `SourceNotFound` and an
  ambiguous suffix is `AmbiguousSource`.
- `CLI-21` `unmeld <name>` by default uninstalls every item installed from the
  source (each via its file registry, then its manifest entry), mirroring meld's
  install-by-default (CLI-23): dropping a source cleans up after itself in one
  step. It first lists the items it will remove; the multi-item confirmation
  (CLI-42) applies, and `--yes` skips it.
- `CLI-22` `unmeld --unlink-only` removes only the source (clone and registry
  entry) and leaves its installed items in place. It lists those orphaned items
  and suggests the `forget` command to remove them later. This is the opt-out from
  the default item removal (CLI-21), mirroring `meld --link-only` (CLI-23).
- `unmeld` runs the source's uninstall hooks before removal and accepts
  `--dangerously-skip-install-hook-check` to run them unattended, and
  `--uninstall-hook <cmd>` to supply or override the uninstall hook (see
  install-hooks.md, HOOK-54, HOOK-59).

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
- `CLI-51` With no sources melded, `sync` reports that and exits successfully.
- `CLI-52` `sync` does not change consumer aliases.
- `CLI-53` `sync --upgrade` runs an `upgrade` pass after refreshing sources
  (reporting pending upgrades and prompting before applying, exactly like
  `upgrade`), so a single command both fetches upstream and applies pending
  upgrades.
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
  `[y/N]` (default No; EOF counts as No) before applying anything.
- `CLI-61` The report lists, per item, the hash and commit deltas, and a compare
  URL when the source host supports one. A namespace change is shown as a rename.
- `CLI-62` `--yes` applies upgrades without prompting.
- `CLI-63` An optional `item` limits upgrade to the matching installed item(s),
  matched against the manifest by effective name and honoring a `kind:` prefix
  and an `owner/repo#` source qualifier. The ref may match several installed
  items, all of which are upgraded.
- `CLI-64` With nothing pending, `upgrade` reports up to date and changes nothing.

## recall

- `CLI-70` `recall` (no argument) is a status view of everything `mind` manages:
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
  installed flag and, when installed, the commit); a lookup emits the single
  item; `--sources` emits the source array. An empty registry is `[]`.
- `CLI-74` In the default status view, each item line marks its install state
  inline: an installed item shows that it is installed and its short commit; a
  not-installed item is marked available. Items are grouped under their source, so
  the source a given item comes from is unambiguous.

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
- `CLI-83` `probe` and `recall` accept `--kind <skill|agent|rule>` and
  `--source <selector>` filters that narrow the listing, composing with `probe`'s
  substring query. For `recall` they apply to the installed-items listing, not to
  `--sources` or a single-item lookup (use a `kind:` / `owner/repo#` ref there);
  passing them with `--sources` or a single item prints a note that they are
  ignored.
- `CLI-84` `probe --json` emits the rows as a JSON array on stdout instead of the
  table; each row carries the installed flag, kind, effective name, source,
  content hash, and description.
- `CLI-85` `probe`'s query matches an item whose effective name *or* description
  contains the query, case-insensitively. This supersedes the name-only matching
  of CLI-80 so an item is found by what it does, not only by its name. The
  `--kind` / `--source` filters (CLI-83) still compose with the query.

## review

`review` is the author-side counterpart to `introspect`: it validates a source
*before* it is published or melded, surfacing the problems that would otherwise
only appear at meld or install time. It is read-only and installs nothing.

- `CLI-130` `review <target>` validates a source for publishing. `<target>` is a
  local path, a repo spec (the forms accepted by `meld`, CLI-11; cloned to a temp
  area for the check), or the selector of an already-melded source (matched like
  `unmeld`, CLI-20).
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
  item kind, a conflicting `[source]` pin, or an unresolved `{{ns:}}` / path token)
  exits non-zero; advisory findings only (unguarded references, missing
  descriptions, hardcoded paths, bare tool references) exit zero. It changes
  nothing on disk in either case, except under `--fix` (CLI-138).
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
  install (tooling.md, TOOL-11/12). Every bad token is reported, not just the
  first.
- `CLI-136` `review` reports, as an advisory `hardcoded-path` finding, an item
  file that hardcodes a mind install path that a path token should replace. It
  recognizes the three install layouts (`.mind/store/<kind>/...`, the agent-home
  `.claude/<kinddir>/...`, and `.agents/<kinddir>/...`) under any home-root
  spelling: a leading `~`, `$HOME`, `${HOME}`, or an absolute `/home/<user>` or
  `/Users/<user>` path. When the path maps confidently to a token (the item's own
  dir -> `{{self}}`, a sibling tool's entrypoint -> `{{tools:name}}`, another
  sibling -> `{{path:kind:name}}`) the finding names the suggested token. The
  message reflects what the path resolves to at runtime (CLI-145). Advisory, not
  hard: `--fix` rewrites the confidently-mapped ones (CLI-138).
- `CLI-137` `review` reports, as an advisory `bare-tool-reference` finding, a
  sibling tool named in an item's prose without a token. Unlike the unguarded
  sibling-reference scan (CLI-131), which only matters under a prefix, a bare tool
  reference is flagged regardless of prefix, since a tool is reached by a path
  token, never by name.
- `CLI-138` `review --fix` rewrites the source in place and is the sole exception
  to `review` being read-only (CLI-132). It applies only to a local-path target;
  a registry selector or a repo spec (whose clone is a discarded temp) refuses
  `--fix` and changes nothing. For each item file it rewrites confidently
  recognized hardcoded install paths into the matching token (CLI-136), un-wraps
  misplaced `{{ns:}}` tokens (CLI-139) back to the bare name, and templatizes
  bare sibling names into `{{ns:}}` (the `init-source --template` transform,
  INIT-5), then reports each file it changed.
- `CLI-139` `review` flags a misplaced `{{ns:}}` token -- one in a non-prose
  context (NS-24) where name-substitution is wrong. A token inside a fenced code
  block, an inline code span, or adjacent to a path separator is an advisory
  `misplaced-reference` (a name token belongs in prose; code and paths use the
  path tokens, tooling.md). A token in the frontmatter `name:` field is a hard
  `misplaced-reference`: an item must not namespace its own name. This is the dual
  of the unguarded-reference scan (CLI-131): one finds a bare name that should be
  a token, the other a token that should be a bare word.
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

## evolve

`evolve` upgrades the `mind` executable itself (distinct from `upgrade`, which
upgrades installed items, and `sync`, which refreshes sources). It uses the same
native curl/wget downloader as `resources/install.sh` and resolves the same
release artifacts as the install script and the Homebrew formula.

- `CLI-140` `evolve` compares the running version against the latest published
  release. With nothing newer it reports up to date and changes nothing. With a
  newer release it replaces the running executable in place with the release binary
  for the current platform.
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
  binary intact. A Homebrew-managed install is upgraded with `brew upgrade` instead;
  `evolve` replaces the binary it runs from and does not coordinate with a
  package manager.

## config

- `CLI-110` `config show` creates the config if absent (STO-15), then prints the
  config file path and its key/value pairs (`lobes`, with the default shown when
  unset). It also notes when `MIND_AGENT_HOMES` is set and overrides `lobes`.
- `CLI-111` `config lobes list` lists the configured agent homes, or the default
  home when none are configured. `target` is a visible alias of the whole `lobes`
  subcommand, so `config target list` / `add` / `remove` all work too.
- `CLI-112` `config lobes add <path>` appends an agent home to `config.toml`,
  creating the file if needed; adding one already present is a no-op.
- `CLI-113` `config lobes remove <path>` drops a configured agent home; a path
  that is not configured is an error (`UnknownLobe`).

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
  environment variable `NO_COLOR` is unset or empty; the `--json` flag is not in
  effect; and the `--ascii` flag is not in effect. An unset locale (none of the
  three variables is set) is treated as non-UTF-8. When the gate is OFF, all output
  is plain ASCII with no ANSI escape sequences.

- `CLI-152` When the capability gate (CLI-151) is ON, output uses ANSI color and
  Unicode glyphs with these semantics: green = installed / ok; yellow = warning /
  drift / removed-upstream; red = error; dim = available / inactive. When the gate
  is OFF, output uses a plain-ASCII fallback for every glyph and no color escapes.
  The ASCII fallback replaces each glyph with a visually equivalent ASCII character
  or short string (e.g. `+` for installed, `!` for warning, `x` for error, `-` for
  available), so all information is preserved without terminal support.

- `CLI-153` Every mutating verb (`meld`, `learn`, `forget`, `sync`, `upgrade`,
  `unmeld`, and `config lobes add`/`remove`) emits a structured JSON result object
  on stdout under `--json` and writes nothing else on stdout. The stable fields of
  this object are:

  ```json
  {
    "action":  "<verb>",
    "target":  "<item-or-source ref>",
    "outcome": "<installed|removed|upgraded|synced|renamed|no-op|failed>"
  }
  ```

  `action` is the CLI verb (e.g. `"learn"`, `"forget"`, `"meld"`). `target` is the
  effective name of the item or source the verb acted on (e.g. `"skill:review"`,
  `"github.com/owner/repo"`). `outcome` is one of the values above; `"no-op"` is
  used when the verb completed successfully but changed nothing (e.g. re-melding an
  already-registered source with nothing to install). A verb MAY add extra fields
  where it genuinely returns more data (for example, `learn` MAY include an
  `"installed"` array listing the effective names of all items installed in that
  call, including dependency-closure items). The read-only verbs (`recall`, `probe`,
  `introspect`) keep their existing JSON shapes (CLI-73, CLI-84, CLI-92) and are
  not affected by CLI-153.

- `CLI-154` `NO_COLOR` being set (to any value, including empty) forces the
  capability gate (CLI-151) OFF regardless of TTY or locale. A non-UTF-8 locale or
  an unset locale also forces the gate OFF even on a TTY. `--ascii` forces the gate
  OFF regardless of `NO_COLOR`, locale, or TTY state. These conditions are
  independent: any one of them alone is sufficient to disable color and Unicode
  glyphs.

## Exit status

- `CLI-100` A command that completes its work exits 0. Any `MindError` is printed
  to stderr (with its source chain) and exits non-zero.

