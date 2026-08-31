# Install hooks

A source can declare install hooks in `mind.toml`: shell commands that build or
install the tooling its items rely on. A user can supply or override them with
`meld --install-hook <cmd>`.

The full form is a `[[hooks]]` array-of-tables. Each entry has a required `run`
field (the shell command), an optional `name` label shown in the disclosure, an
`optional` bool (default `false`), and an `event` field (`"install"` for `meld`,
`"update"` for `upgrade`, `"uninstall"` for `unmeld`; default `"install"`). The
legacy `[source].install` string is shorthand for one required install hook:

```toml
# mind.toml in a source repo

[[hooks]]
run = "make build"
name = "build tooling"
event = "install"

[[hooks]]
run = "pip install -r requirements.txt"
name = "python deps"
optional = true
event = "install"

[[hooks]]
run = "make upgrade"
name = "upgrade tooling"
event = "update"

[[hooks]]
run = "make clean"
name = "cleanup"
event = "uninstall"
```

## The safety prompt

Because a hook is arbitrary code, `mind` discloses the source identity, pin,
commit, clone path, and exact command before running anything, and prompts
`[Y/n/a]` with three choices: run the hook (the default, a bare Enter), skip it
but still install the source and its items, or abort and install nothing. In a
non-TTY context (CI, scripts) the hook is skipped and a note is printed;
`--dangerously-skip-install-hook-check` runs it unattended. Overriding a source's
declared install hook with `--install-hook` is announced in the prompt, which
shows both the declared and the overriding command.

The disclosure also shows a version-control browse URL pinned to the disclosed
commit alongside the on-disk clone path, so you can read the exact code that will
run either locally or in the forge before approving. The URL is produced only for
GitHub-shaped `https` remotes; a GitLab or Bitbucket host, an SSH remote, or a
local/`file://` source shows the clone path alone (no correct web link exists for
those).

## Re-runs

A skipped hook is recorded and re-offered by `mind upgrade`, so you can run it
later without the source needing to advance first. On an `upgrade` re-run the
source is already installed, so abort is treated as skip. `upgrade` also re-runs
the hook when a source advances to a new commit. `sync --upgrade` accepts
`--dangerously-skip-install-hook-check` so a CI pipeline can run hook re-runs
unattended. Without the flag, a non-TTY `sync --upgrade` skips hook re-runs (the
same as `upgrade`).

Because the default is a re-run, write an install hook to be idempotent: safe to
run again on a host it already set up.

## Update hooks

An `event = "update"` hook runs at `upgrade` INSTEAD of re-running the install
hooks. Declare one when a step cannot be made idempotent (a first-run migration,
a one-shot registration):

```toml
[[hooks]]
run = "make install"          # runs at meld

[[hooks]]
run = "make upgrade"          # runs at upgrade, and `make install` does not
event = "update"
```

A source that declares no update hook is unaffected: its install hooks re-run as
before. An update hook never runs at `meld` or on a re-meld, is recorded against
the commit it ran at exactly as an install hook is (so it is not re-offered until
the source advances again), and is disclosed and prompted the same way.

Update hooks supersede install hooks for the source that declares them: while
a source declares any update hook, `upgrade` offers its pending update hooks
and does not re-offer that source's install hooks. An install hook skipped at
`meld` (the default in a non-TTY run such as CI) is therefore not re-offered
by a later `upgrade`. Run it explicitly: `mind hooks run <source> --event
install --force`.

The same applies per item: an item that declares an update hook runs it in
place of its install hooks when the item is re-installed over an existing
install, which in practice is `upgrade`'s in-place swap. A `learn` naming an
already-installed item is a no-op and runs no hook (HOOK-125); to run an
item's update hooks on demand, use `mind hooks run <source>#<item> --event
update`. A `build` hook is unaffected and always runs, since it produces the
item's content.

## Uninstall hooks

Uninstall hooks (`event = "uninstall"`) run at `unmeld`, in the source's clone,
before the clone is removed. They use the same safety-prompt model as install
hooks: required hooks prompt run / skip / abort-the-unmeld; optional hooks prompt
run / skip; a non-TTY `unmeld` skips them and notes it. `unmeld --uninstall-hook
<cmd>` supplies or overrides the source's declared uninstall hooks. `unmeld
--dangerously-skip-hook-check` runs them unattended (`--dangerously-skip-install-hook-check`,
the original spelling, is kept as a hidden alias). A required uninstall hook that
fails or is aborted leaves the source melded.

## Running hooks on demand

Hooks normally run as a step of another verb: install hooks at `meld`/`upgrade`,
uninstall hooks at `unmeld`, and item hooks at `learn`/`forget`/`upgrade`.
`mind hooks run` runs them outside those flows, so you can run a hook you earlier
skipped, re-run one whose effect was later lost (a deleted build output or side
effect), or retry one that failed transiently, without a full re-meld or
reinstall. Every hook it runs goes through the same disclosure and consent prompt
as an automatic run.

```
mind hooks run <source>                     # the source's pending install hooks
mind hooks run <source> --force             # every install hook, even already-run ones
mind hooks run <source> --event uninstall   # the source's uninstall hooks
mind hooks run <source>#<item>              # an installed item's install hooks
mind hooks run <source>#<item> --event build  # rebuild the item (transactional)
mind hooks list <source>                    # list hooks in effect, run nothing
```

`<target>` is a source filter (the source's own `[[hooks]]`) or an
`owner/repo#item` ref (that item's hooks); there is no ambiguity check, so a
filter matching several sources, or a ref matching several items, runs the
hook for each in turn. `--event` selects the lifecycle event (`install`,
`update`, `uninstall`, or `build`); `build` is valid only for an item target.
`--event update` behaves as `--event install` does at that target: only pending
hooks run, and `--force` runs them all.

An item-link instance's own identity is `owner/repo#<path>` (see
[Item links](commands.md#item-links-install-one-skill-by-url)), which is spelled
the same way as "item `<path>` in source `owner/repo`". When a target matches
both a registered source and an installed item, `mind` refuses it rather than
picking one, and names both disambiguated forms. Force either reading:

```
mind hooks run source:<target>              # the source's own hooks
mind hooks run <source>#<kind>:<name>       # that item's hooks
```

For a source install run, only *pending* install hooks run by default (a hook
that never ran, was skipped, or whose recorded commit is behind the source's
current commit); `--force` re-runs every install hook regardless. An item target
runs the item's hooks in place against its installed store copy and requires the
item to be installed. `--event build` rebuilds the item through the normal
transactional install path, so a failed rebuild leaves the existing copy
untouched.

The `--dangerously-skip-install-hook-check` and `--dangerously-skip-build-hook-check`
flags apply exactly as they do to the automatic flows: without them a non-TTY run
skips the hooks, and a required hook's failure or abort is a non-zero exit.

`mind hooks list <target>` reports the hooks in effect for a source and its
installed items -- each hook's event, required/optional flag, and command, and for
a recorded source install or update hook whether it is pending and the commit
it last ran at -- without running any. It is the read-only companion to
`hooks run`.

## Visibility

`recall --sources` marks a source that carries hooks with a count-aware token in
its status bracket (e.g. `1 hook` or `3 hooks`). `mind review <repo>` lists every
declared hook (source and item, whichever event), showing each hook's command,
event, and whether it is required or optional. `mind hooks list <target>` shows the same
detail plus the pending/last-ran state of recorded install and update hooks.

## Where an item declares its hooks

An item's own lifecycle hooks (`install` / `update` / `uninstall`) can be
declared in three places. They do not merge: the first one that declares
anything supplies the item's hooks. A tool's `build` hook is not one of the
three: it is declared only as the `[[items]].build` field in the source's
root `mind.toml` or as the `build:` key in a tool's `TOOL.md` frontmatter,
and a scoped item `mind.toml` rejects `event = "build"`.

1. The source's root `mind.toml`, in the item's `[[items]]` entry (scalar
   `install`/`update`/`uninstall`, or an `[[items.hooks]]` array).
2. A scoped `mind.toml` in the item's own directory, for a skill or a tool. Its
   only table is `[[hooks]]`, with the same fields as a source's; any other key
   is an error.
3. The item's frontmatter, in whatever file describes it (`SKILL.md`, `TOOL.md`,
   or an agent's or rule's `.md`): the scalar `install:`, `update:`, and
   `uninstall:` keys, one required hook each.

```toml
# skills/scanner/mind.toml
[[hooks]]
run = "./setup.sh"
name = "Set up"

[[hooks]]
run = "./migrate.sh"
event = "update"
```

```markdown
<!-- skills/scanner/SKILL.md -->
---
description: scanner
install: ./setup.sh
update: ./migrate.sh
---
```

An item hook runs in the item's store directory when the item is
directory-backed (a skill or a tool), so a relative `./setup.sh` resolves
against the item's own files. A single-file kind (an agent, a rule, or a
command) has no directory of its own: its hook runs in the shared kind
directory (`~/.mind/store/agent/`, `~/.mind/store/rule/`,
`~/.mind/store/command/`), and the item ships no scripts, so its hook must be
a command on `PATH` or an absolute path. A relative script there fails the
install with a `HookFailed` hard stop that rolls the install back.

An item manifest is part of the item's content: it is copied into the store and
hashed like any other file, so editing it upstream is drift `upgrade` picks up.
No agent harness reads it; only `mind` does. These keys are `mind`'s, not the
harness's. An agent harness reads the same frontmatter block, and `mind` does
not control how a given harness treats keys it does not define, so use the
scoped item `mind.toml` if you would rather keep the item's frontmatter to
what the harness itself defines.

`[source].install` is deprecated in favor of the `[[hooks]]` form. See
[The mind.toml file](mind-toml.md) for the schema and
[spec/install-hooks.md](https://github.com/jaemk/mind/blob/main/spec/install-hooks.md)
for the full behavior.
