# FAQ

## Why "meld", "learn", and "forget"?

They are `mind`'s names for add, install, and uninstall. Each verb also answers
to the conventional name (`add`, `install`, `uninstall`, `list`, ...); see
[Verb aliases](concepts.md#verb-aliases).

## Does a source repo need a manifest?

No. `mind` finds items by their layout (`skills/<name>/SKILL.md`,
`agents/<name>.md`, ...). A `mind.toml` is optional and adds metadata, profiles,
hooks, or an explicit inventory. See [Source layout and item kinds](source-layout.md).

## Why does `mind` copy items instead of linking straight into the clone?

So a `sync` never changes what your agent loads. The store copy changes only
when you `upgrade`, after `mind` shows you what changed. It also lets an
upgrade roll back if the new version fails to install.

## What is the difference between `sync` and `upgrade`?

`sync` fetches sources; installed items stay as they are. `upgrade` fetches the
sources it needs and then updates installed items, after confirmation.
`mind update` is an alias for `sync`. See [Keep everything up to date](updating.md).

## Can I edit an installed skill?

Edits to the store copy show up as drift and are replaced on the next upgrade.
To change a skill, edit it in a repo you own: [`absorb`](unmanaged.md) moves a
hand-made item into one, and for someone else's skill, fork the repo and meld
your fork.

## Does it work with harnesses other than Claude Code?

Yes, for skills: Gemini CLI, Antigravity, Codex CLI, Windsurf, and any harness
that reads `skills/<name>/SKILL.md` from a directory you name. Agents link
anywhere a lobe admits them. Rules, commands, and workflows are Claude-only.
See [Use more than one agent](lobes.md).

## Can I use a Claude plugin marketplace?

Yes. A repo with `.claude-plugin/plugin.json` or `marketplace.json` melds as it
is, and its skills, agents, commands, and workflows become items. See
[Claude plugins and marketplaces](marketplace.md).

## Does it run code from the repos I meld?

Only hooks a source declares, and only after showing you the exact command and
asking. Without a terminal, hooks are skipped unless you pass
`--accept-hooks`. `mind review <repo>` lists a source's hooks
before you meld it. See [Run setup with hooks](install-hooks.md).

## Does it make network calls on its own?

Only `git` operations against the sources you meld, and `evolve` against GitHub
releases. There is no telemetry. See [Restricted networks](enterprise.md).

## Can my organization restrict what gets installed?

Yes, with a managed policy file: a source allowlist, required pins, locked
agent homes, and provisioned baseline sources. See [Managed policy](policy.md).

## Does it work on Windows?

Under WSL. `mind` links items with symlinks and supports Linux and macOS.

## Where does it keep things?

`~/.mind`: the config, the store, and source clones. See
[Paths](configuration.md#paths).
