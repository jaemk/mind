# Commands

Every verb, its flags, and where to read more. Run `mind <verb> --help` for the
same text in the terminal. For the vocabulary, see [Core concepts](concepts.md).

| verb | does |
|------|------|
| [`meld`](#meld) | register a source repo and offer to install its items |
| [`unmeld`](#unmeld) | drop a source and uninstall its items |
| [`learn`](#learn) | install items |
| [`forget`](#forget) | uninstall items |
| [`recall`](#recall) | list what is installed, or one item's details |
| [`probe`](#probe) | browse and search what sources offer |
| [`sync`](#sync) | fetch sources |
| [`upgrade`](#upgrade) | bring installed items up to date |
| [`curate`](#curate) | apply what your curators declare |
| [`introspect`](#introspect) | report drift and broken links |
| [`hooks`](#hooks) | run or list hooks on demand |
| [`config`](#config) | view config and manage agent homes |
| [`link-project`](#link-project) | add a project's agent home |
| [`absorb`](#absorb) | move a hand-placed item into a source you own |
| [`dump`](#dump) | write your setup as a `mind.toml` |
| [`review`](#review) | validate a source or a policy file |
| [`init-source`](#init-source) | scaffold a source's `mind.toml` |
| [`evolve`](#evolve) | update the `mind` binary |
| [`completions`, `man`](#completions-and-man) | shell completions and the man page |

## Global flags

Accepted before or after any verb.

| flag | effect |
|------|--------|
| `--json` | one JSON document on stdout; see [Scripts and CI](scripting.md#json-output) |
| `-y`, `--yes` | answer every confirmation |
| `--ascii` | plain ASCII output, no color or Unicode |
| `-v`, `--verbose` | extra advisory output, such as unguarded-reference warnings |
| `--max-metadata-size <SIZE>` | metadata size cap for this run; see [Configuration](configuration.md#metadata-size-cap) |

## Selecting items and sources

Verbs take either a *ref*, which must match exactly one thing, or a *filter*,
which may match several.

| pattern | selects |
|---------|---------|
| `review` | the item named `review` |
| `skill:review` | the skill named `review` |
| `owner/repo#review` | `review` from one source |
| `'*'` | every item |
| `'skill:*'` | every skill |
| `'owner/repo#*'` | every item of one source |
| `'review*'` | items whose name starts with `review` |

Quote globs so the shell does not expand them. A glob matches the installed
(effective) name.

- **Refs** error rather than guess: `unmeld <name>` must match one source, and a
  bare item name given to `learn`, `forget`, or `upgrade` that exists in two
  kinds (`skill:greet` and `agent:greet`) must be qualified.
- **Filters** act on every match: `sync [source]`, `--source <filter>` on
  `upgrade`, `recall`, and `probe`, `hooks run <target>`, any glob, and the
  `owner/repo#` part of an `upgrade` argument.
- A source filter is an exact name, a trailing part (`repo`, `owner/repo`), or a
  glob over the full `host/owner/repo` identity.

With no match: `sync` and `upgrade --source` error, `upgrade <item>` reports up
to date, `recall`/`probe --source` list nothing, and `meld --learn` errors with
the source left melded.

## meld

```
mind meld [REPO] [OPTIONS]
```

Register a source and offer to install its items. `REPO` defaults to `.` and may
be `owner/repo`, an `https://` or `git@` URL, a local path, a `file://` URL, or
an [item link](item-links.md). Melding a source you already have installs any
missing items, or shows each item's state. Alias `add`.

| flag | effect |
|------|--------|
| `--register-only` | register without offering to install |
| `--learn <NAME\|GLOB>` | install only matching items (repeatable); see [Choose what a source installs](choosing-items.md) |
| `--profile <NAME>` | enable an install profile in addition to `default` (repeatable or comma list) |
| `--no-default-profile` | leave the `default` profile out |
| `--all-profiles` | enable every declared profile |
| `--pin <HEAD\|REF\|branch=NAME\|tag=NAME>` | set the version tracked; also re-pins a melded source; see [Pin a source to a version](pinning.md) |
| `-N`, `--namespace <PREFIX>` | install items as `<PREFIX>:<name>`; a different namespace registers a separate instance; see [Fix name collisions](collisions.md) |
| `-r`, `--recursive` | offer the items of every source a curator lists |
| `-f`, `--force` | overwrite link targets `mind` did not create; on a re-meld, also re-offer every install hook |
| `--local` | install into the current project's lobe only |
| `--root <DIR>` | scan these directories instead of the source's own roots (repeatable) |
| `--add-root <DIR>` | scan these directories in addition to the source's own discovery (repeatable) |
| `--flat-skills` | skills are directories at the root, with no `skills/` container |
| `--install-hook <CMD>` | supply or replace the source's install and update hooks |
| `--kind <agent\|rule\|command>` | the kind of a file named by an item link |
| `--dangerously-skip-install-hook-check` | run install hooks without asking |
| `--dangerously-skip-build-hook-check` | run item build hooks without asking |

`--root`, `--add-root`, `--flat-skills`, and `--install-hook` apply only at the
meld that first registers a source; a re-meld notes them as ignored. Without a
terminal and without `--yes`, `meld` registers the source and installs nothing.

```
mind meld owner/repo
mind meld owner/repo --learn review --yes
mind meld owner/repo --profile full --pin branch=stable
mind meld https://github.com/owner/repo --namespace acme
mind meld . --register-only
```

## unmeld

```
mind unmeld <NAME> [OPTIONS]
```

Uninstall every item a source installed, then drop it. `NAME` must match one
source unless it is a glob. Use the full identity for one instance:
`github.com/acme/skills@jk`, `github.com/acme/skills#skills/foo`. Alias
`remove`, `rm`.

| flag | effect |
|------|--------|
| `--keep-items` | drop the source, leave its items installed |
| `--uninstall-hook <CMD>` | supply or replace the source's uninstall hooks |
| `--dangerously-skip-hook-check` | run uninstall hooks without asking |

```
mind unmeld acme/agents
mind unmeld acme/agents --keep-items
mind unmeld '*agents' --yes
```

See [Remove items and sources](removing.md).

## learn

```
mind learn <ITEM> [OPTIONS]
```

Install items into every lobe. `ITEM` is a ref, a glob, a source name with
`--all` or `--profile`, or an [item link](item-links.md) URL. Installing part of
a source also installs the siblings it depends on, after showing the tree.
Alias `install`.

| flag | effect |
|------|--------|
| `--all` | every item of the named source |
| `--profile <NAME>` | install and enable that profile of a melded source (repeatable or comma list) |
| `--all-profiles` | install and enable every declared profile |
| `-n`, `--dry-run` | show what would install, change nothing |
| `-f`, `--force` | overwrite a link target `mind` did not create |
| `--local` | install into the current project's lobe only |
| `--pin` | for an item link, freeze the URL's branch at its current commit (takes no value) |
| `--kind <agent\|rule\|command>` | the kind of a file named by an item link |
| `--dangerously-skip-install-hook-check` | run an item's install hook without asking |
| `--dangerously-skip-build-hook-check` | run an item's build hook without asking |

`--no-default-profile` is accepted for symmetry with `meld` and changes nothing.
Installing an item by exact name marks it as asked for, so disabling a profile
later keeps it.

```
mind learn review
mind learn 'skill:*'
mind learn owner/repo --all
mind learn owner/repo --profile full
mind learn https://github.com/owner/repo/tree/main/skills/foo
```

See [Find and install items](installing.md).

## forget

```
mind forget [ITEM] [OPTIONS]
```

Uninstall items: the store copy and every link. Alias `uninstall`, `unlearn`.

| flag | effect |
|------|--------|
| `--profile <NAME>` | disable a profile of the source named by `ITEM`, forgetting items installed only for it |
| `--unmanaged` | act on unmanaged lobe items only; with no `ITEM`, all of them |
| `--dropped` | remove items their source no longer offers, or whose source is no longer melded; `ITEM` narrows them |
| `-f`, `--force` | skip the confirmation when other items depend on the one removed |
| `--dangerously-skip-hook-check` | run uninstall hooks without asking |

A glob matching several items asks once; `--yes` skips that and the dependents
question.

```
mind forget review
mind forget 'skill:*' --yes
mind forget owner/repo --profile full
mind forget --unmanaged
mind forget --dropped
```

## recall

```
mind recall [ITEM] [OPTIONS]
```

List each source with its items, marked installed, available, or out of date,
followed by unmanaged items. With `ITEM`, show that item's details. Alias
`list`, `status`.

| flag | effect |
|------|--------|
| `--sources` | list sources instead, with their hooks and profiles |
| `--kind <KIND>` | only items of this kind |
| `--source <FILTER>` | only items from matching sources |
| `--tree` | installed items as a dependency forest (scoped to `ITEM` if given) |

`--kind` and `--source` apply to the item listing only.

## probe

```
mind probe [QUERY] [OPTIONS]
```

Browse and search every melded source's items. On a terminal it opens the
[terminal UI](tui.md); with `--no-tui`, `--json`, or piped output it prints a
listing. `QUERY` is a case-insensitive substring of names and descriptions.
Alias `search`.

| flag | effect |
|------|--------|
| `--kind <KIND>` | only items of this kind |
| `--source <FILTER>` | only items from matching sources |
| `--no-tui` | print the listing instead of opening the UI |

## sync

```
mind sync [SOURCE] [OPTIONS]
```

Fetch every source, or those matching `SOURCE`, and refresh their catalogs.
Installed items do not change. Alias `update`.

| flag | effect |
|------|--------|
| `--upgrade` | then run `upgrade` over the same sources (prefer plain `upgrade`, which syncs first) |
| `--dangerously-skip-install-hook-check` | with `--upgrade`, run hook re-runs without asking |
| `--dangerously-skip-build-hook-check` | with `--upgrade`, run build hooks without asking |

An unscoped `sync` also provisions [managed policy](policy.md#auto-meld-org-provisioning)
sources and registers newly listed curated sources. `sync` exits 1 if any fetch
failed.

## upgrade

```
mind upgrade [ITEM] [OPTIONS]
```

Fetch the involved sources, report each installed item that changed, and apply
on confirmation. `ITEM` is a filter.

| flag | effect |
|------|--------|
| `--source <filter>` | fetch and upgrade only the matching sources; combines with `ITEM` |
| `--no-sync` | skip the fetch; compare against the clones on disk |
| `--dangerously-skip-install-hook-check` | run install or update hook re-runs without asking |
| `--dangerously-skip-build-hook-check` | run build hooks without asking |

See [Keep everything up to date](updating.md).

## curate

```
mind curate [OPTIONS]
```

Apply what your curators declare: register newly listed sources, install
declared items, re-pin, upgrade curated sources, and report dropped entries.
Alias `reconcile`.

| flag | effect |
|------|--------|
| `--check` | report the plan, change nothing |
| `--prune` | also uninstall and drop sources a curator no longer lists |
| `--no-sync` | plan against the clones on disk |
| `--adopt <IDENTITY>` | hand an existing source to the curator that lists it |
| `--dangerously-skip-install-hook-check` | run install and update hooks without asking |
| `--dangerously-skip-build-hook-check` | run build hooks without asking |

See [Follow a curated collection](curate.md).

## introspect

```
mind introspect [--fix]
```

Report drift, broken links, and unsynced sources. `--fix` recreates missing
links without changing versions. Alias `doctor`.

## hooks

```
mind hooks run <TARGET> [OPTIONS]
mind hooks list <TARGET>
```

Run a source's or item's hooks outside the usual verbs, or list them. `TARGET`
is a source filter or a `source#item` ref.

| flag (`run`) | effect |
|--------------|--------|
| `--event <install\|update\|uninstall\|build>` | which hooks (default `install`; `build` is for items) |
| `--force`, `--rerun` | re-run hooks already recorded at the current commit |
| `--dangerously-skip-install-hook-check` | run install or update hooks without asking |
| `--dangerously-skip-build-hook-check` | run a build hook without asking |

See [Run setup with hooks](install-hooks.md#running-hooks-on-demand).

## config

```
mind config show
mind config lobes list
mind config lobes add [PATH] [--preset NAME] [--subdir REL] [--snapshot] [-f]
mind config lobes remove <PATH> [--snapshot]
mind config lobes detect
```

`show` prints the config file's location and effective settings. `lobes`
manages agent homes:

| flag | effect |
|------|--------|
| `--preset <NAME>` | `gemini`, `codex`, `universal`, or `windsurf` |
| `--subdir <REL>` | a subdirectory under `PATH` (skills only) |
| `--snapshot` | `add`: write real-file copies, register nothing; `remove`: replace links with copies first |
| `-f`, `--force` | overwrite a colliding file |

`detect` reports which harnesses are installed and offers to add them. See
[Use more than one agent](lobes.md) and [Configuration and paths](configuration.md).

## link-project

```
mind link-project [DIR] [--preset NAME] [--subdir REL] [--snapshot] [-f]
```

Shorthand for `config lobes add DIR --preset windsurf`, with `DIR` defaulting to
the current directory. See [Project lobes](lobes.md#project-lobes).

## absorb

```
mind absorb <ITEM> [--to PATH] [-f]
```

Move one unmanaged item into a git repo you own, commit it, and install it as a
managed item. `--to` sets the destination; `--force` overwrites a collision
there. See [Adopt items you placed by hand](unmanaged.md).

## dump

```
mind dump [--output PATH] [--whole-sources]
```

Write a `mind.toml` that reproduces your sources, pins, and installed items.
`--whole-sources` marks every source `install = true`. See
[Back up and reproduce your setup](dump.md).

## review

```
mind review [TARGET] [-N PREFIX] [--fix]
mind review --policy <PATH>
```

Validate a source (a path, a melded source, or a repo to shallow-clone; default
`.`) or a managed policy file. Exits 1 on a hard finding. See
[Validate before publishing](validate.md).

## init-source

```
mind init-source [PATH] [--template] [--marketplace] [--flat-skills] [-N NAME]
```

Scaffold a `mind.toml` and report the references between items. See
[Validate before publishing](validate.md#init-source).

## evolve

```
mind evolve [--check] [--to VERSION]
```

Update the `mind` binary to the latest release, or to `VERSION` (the only way
to reach a prerelease). `--check` reports without changing anything. Alias
`self-update`. See [Install](install.md#updating).

## completions and man

```
mind completions <bash|elvish|fish|powershell|zsh>
mind man
```

Print a shell completion script or the roff man page.

## Older spellings

These still work as hidden aliases:

| old | current |
|-----|---------|
| `meld --link-only` | `meld --register-only` |
| `unmeld --unlink-only` | `unmeld --keep-items` |
| `--follow-branch B`, `--pin-tag T`, `--pin-ref C` | `--pin branch=B`, `--pin tag=T`, `--pin C` |
| `--as PREFIX` | `--namespace PREFIX` |
| `evolve --version V` | `evolve --to V` |
| `--dangerously-skip-install-hook-check` on `unmeld` or `forget` | `--dangerously-skip-hook-check` |
