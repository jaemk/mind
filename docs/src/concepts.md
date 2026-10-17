# Core concepts

`mind` connects *sources* (git repos full of agent tooling) to *lobes* (your
agent homes, like `~/.claude`). This page defines the words the rest of the
docs use and shows what each step puts on disk.

## Sources, items, lobes, and the store

**Source.** A git repo you have melded. `mind meld <repo>` clones it into
`~/.mind/sources/<host>/<owner>/<repo>` and records it in
`~/.mind/sources.json`. Melding makes its items available; it does not have to
install any. `mind sync` refreshes the clone later.

**Item.** One thing a source offers, of one of seven *kinds*:

| kind | in a source repo | what the harness does with it |
|------|------------------|-------------------------------|
| `skill` | `skills/<name>/SKILL.md` plus any bundled files | loads the skill; offers it as `/<name>` |
| `agent` | `agents/<name>.md` | a subagent the harness can delegate to |
| `rule` | `rules/<name>.md` | context the harness reads |
| `command` | `commands/<name>.md` | a slash command, `/<name>` |
| `workflow` | `workflows/<name>.js` | a script offered to the harness's `Workflow` tool (see [Workflows](workflows.md)) |
| `tool` | `tools/<name>/` | nothing directly: a helper other items call (see [Share helper scripts](tooling.md)) |
| `resource` | `resources/<file-or-dir>` | nothing directly: data other items read, offered only when something references it (see [Share data files](resources.md)) |

Items are found by these conventional paths, by a Claude plugin manifest, or by
an explicit list in the source's `mind.toml`. See
[Source layout and item kinds](source-layout.md).

Claude Code treats custom commands and skills alike: both appear as `/<name>`.
A source that ships `commands/foo.md` and `skills/foo/SKILL.md` offers two
distinct items (`command:foo` and `skill:foo`) that link into different
directories, so `mind` installs both if you ask for both. Install only one
unless you mean to have both.

**Lobe.** A directory `mind` links items into: an agent home holding `skills/`,
`agents/`, `rules/`, `commands/`, and `workflows/`. The default lobe is
`~/.claude`. You can add Gemini CLI, Codex CLI, Windsurf, or any directory, each
optionally limited to the kinds that harness understands. See
[Use more than one agent](lobes.md).

**Store.** `~/.mind/store/<kind>/<name>`, where `mind` keeps the installed copy
of each item. Lobes link to the store, not to the source clone, so a `sync`
never changes an installed item under you. Only `upgrade` does.

**Learn and forget.** `mind learn <item>` copies an item from the clone into the
store and links it into every lobe. `mind forget <item>` removes both. A tool is
the exception: it lives in the store only and is not linked into a lobe.

## What each step puts on disk

`mind meld jaemk/mind` clones the source. Nothing is linked yet:

```text
~/.mind/
  sources.json                              # registry: `mind` is now melded
  sources/
    github.com/jaemk/mind/                  # the clone
      examples/hello/skills/hello-mind/     # the offered skill
        SKILL.md
      ...
  store/                                    # empty: nothing learned yet
```

`mind probe` lists what the melded sources offer (`kind:name`, source, content
hash, description):

```text
$ mind probe --no-tui
  skill:hello-mind  github.com/jaemk/mind  c62e88cc  A hello-world skill; confirms mind melded this repo
```

`mind learn skill:hello-mind` copies that item into the store and links it into
the lobe:

```text
~/.mind/
  manifest.json                                  # registry: `hello-mind` installed
  store/
    skill/hello-mind/SKILL.md                    # the copy, taken from the clone
  sources/
    github.com/jaemk/mind/ ...                   # clone untouched

~/.claude/                                       # a lobe
  skills/
    hello-mind -> ~/.mind/store/skill/hello-mind # the link the harness finds
```

The harness now resolves `/hello-mind` through that link. A tool the skill used
would land in `~/.mind/store/tool/<name>` with no link under `~/.claude`.

## Staying current

- `mind sync` fetches every source. Installed items do not change.
- `mind upgrade` shows what changed upstream for each installed item and, once
  you confirm, swaps in the new versions. It syncs the sources it needs first.
- `mind evolve` updates the `mind` binary itself.

See [Keep everything up to date](updating.md).

## Naming things

Commands take *refs* to name items and sources.

| form | names |
|------|-------|
| `review` | an item named `review`, of any kind |
| `skill:review` | the skill named `review` |
| `owner/repo#review` | the item `review` from that source |
| `'skill:*'` | every skill (a glob; quote it) |
| `owner/repo` or `repo` | a source, by its trailing path |
| `github.com/owner/repo` | a source, by its full identity |
| `github.com/owner/repo@jk` | the instance of that repo melded under namespace `jk` |
| `github.com/owner/repo#skills/foo` | a single-item source created from an [item link](item-links.md) |

A source's identity is `host/owner/repo`, so the same repo name on GitHub and
on an internal host never collide. See [Commands](commands.md#selecting-items-and-sources)
for which verbs need an exact match and which accept several.

## Namespaces

If two sources each ship a `review`, they would install to the same path. A
*namespace* (also called a prefix) makes one of them install as `jk:review`. A
source can declare its own; you can impose one with `meld --namespace`. See
[Fix name collisions](collisions.md).

## Glossary

| term | meaning |
|------|---------|
| meld | register a source (clone it) and optionally install its items |
| unmeld | drop a source and, by default, uninstall its items |
| learn | install an item |
| forget | uninstall an item |
| recall | list what is installed |
| probe | browse and search what sources offer |
| introspect | check for drift and broken links |
| evolve | update the `mind` binary |
| lobe | an agent home `mind` links into |
| store | `~/.mind/store`, the installed copies |
| curator | a source that lists other sources (a super-source or a Claude marketplace) |
| profile | a named set of a source's items, chosen at meld time |
| unmanaged item | something in a lobe that `mind` did not install |
| drift | an installed item whose content differs from its source |

## Verb aliases

Each verb also answers to a conventional package-manager name. These docs use
the primary verbs.

| alias | primary verb |
|-------|--------------|
| `add` | `meld` |
| `remove`, `rm` | `unmeld` |
| `install` | `learn` |
| `uninstall`, `unlearn` | `forget` |
| `update` | `sync` |
| `list`, `status` | `recall` |
| `search` | `probe` |
| `doctor` | `introspect` |
| `reconcile` | `curate` |
| `self-update` | `evolve` |

Note that `update` is `sync` (it refreshes sources) and does not upgrade
installed items; `upgrade` does that.
