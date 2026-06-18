# CLI

The `mind` command surface. Verbs use a knowledge metaphor.

| command | role |
|---------|------|
| `meld <repo> [--as <prefix>]` | connect a source |
| `unmeld <name>` (alias: `detach`) | disconnect a source |
| `learn <item>` | install |
| `forget <item>` (alias: `unlearn`) | uninstall |
| `sync` | refresh sources |
| `evolve [--yes] [item]` | upgrade installed |
| `recall [--sources] [item]` | list / info |
| `probe [query]` | search |
| `introspect` | diagnose |
| `config show` / `config lobes ...` | view/edit config |

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
- `CLI-12` Melding a repo whose source name is already registered is an error
  (`SourceExists`); nothing is changed.
- `CLI-13` `--as <prefix>` sets the source's namespace, overriding any
  `[source].prefix`. It is persisted and is not changed by `sync`.
- `CLI-14` After melding, if a prefix is in effect, unguarded prose references to
  siblings are reported as warnings (see namespacing.md). Warnings do not fail
  the command.
- `CLI-15` If the melded repo's `mind.toml` lists `[discover].sources`, each is
  melded recursively (see DSC-38), so one `meld` can pull in a curated set.

## unmeld

- `CLI-20` `unmeld <name>` (alias: `detach`) removes the source's clone and
  registry entry. `name` is the full `host/owner/repo` or an unambiguous trailing
  suffix (e.g. `repo` or `owner/repo`); an unknown name is `SourceNotFound` and an
  ambiguous suffix is `AmbiguousSource`.
- `CLI-21` `unmeld` does not remove items already installed from the source;
  those are removed with `forget`.

## learn

- `CLI-30` `learn <item>` with an exact ref installs the single matching item
  into every configured agent home (see lifecycle.md, STO-14), recording it in
  the manifest; a ref matching none is `ItemNotFound` and one matching several is
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

## forget

- `CLI-40` `forget <item>` (alias: `unlearn`) removes an installed item using its
  file registry and deletes its manifest entry. An item that is not installed is
  an error (`NotInstalled`).

## sync

- `CLI-50` `sync` fetches every source, resets its clone to the remote default
  branch, and updates the recorded commit and `[source].description`.
- `CLI-51` With no sources melded, `sync` reports that and exits successfully.
- `CLI-52` `sync` does not change consumer aliases.

## evolve

- `CLI-60` `evolve` reports pending upgrades and, unless `--yes` is given, prompts
  `[y/N]` (default No; EOF counts as No) before applying anything.
- `CLI-61` The report lists, per item, the hash and commit deltas, and a compare
  URL when the source host supports one. A namespace change is shown as a rename.
- `CLI-62` `--yes` applies upgrades without prompting.
- `CLI-63` An optional `item` limits evolve to the matching installed item(s).
- `CLI-64` With nothing pending, `evolve` reports up to date and changes nothing.

## recall

- `CLI-70` `recall` lists installed items (effective name, source, short commit,
  one-line description).
- `CLI-71` `recall <item>` shows one installed item's detail: description, source,
  commit, hash, store path, and link path(s).
- `CLI-72` `recall --sources` lists melded sources (name, url, short commit,
  alias, description).

## probe

- `CLI-80` `probe [query]` lists available catalog items (effective name, source,
  one-line description), filtered to those whose effective name contains `query`.
  An empty query lists everything.
- `CLI-81` `probe` marks installed items with a leading `*` and shows each item's
  short content hash.
- `CLI-82` List outputs (`probe`, `recall`) left-align columns padded to the
  widest value in each column, so rows stay aligned regardless of item-name
  length.

## introspect

- `CLI-90` `introspect` reports: sources with no clone or never synced, installed
  items whose links are missing, items no longer present upstream, items whose
  namespace changed, and items whose source content drifted. It reports a clean
  summary when there are no issues.

## config

- `CLI-110` `config show` creates the config if absent (STO-15), then prints the
  config file path and its key/value pairs (`lobes`, with the default shown when
  unset). It also notes when `MIND_AGENT_HOMES` is set and overrides `lobes`.
- `CLI-111` `config lobes list` (alias `config target list`) lists the configured
  agent homes, or the default home when none are configured.
- `CLI-112` `config lobes add <path>` appends an agent home to `config.toml`,
  creating the file if needed; adding one already present is a no-op.
- `CLI-113` `config lobes remove <path>` drops a configured agent home; a path
  that is not configured is an error (`UnknownLobe`).

## Exit status

- `CLI-100` A command that completes its work exits 0. Any `MindError` is printed
  to stderr (with its source chain) and exits non-zero.

