# Install hooks

A source can declare install hooks in `mind.toml`: shell commands that build or
install the tooling its items rely on. A user can supply or override them with
`meld --install-hook <cmd>`.

The full form is a `[[hooks]]` array-of-tables. Each entry has a required `run`
field (the shell command), an optional `name` label shown in the disclosure, an
`optional` bool (default `false`), and an `event` field (`"install"` for `meld`,
`"uninstall"` for `unmeld`; default `"install"`). The legacy `[source].install`
string is shorthand for one required install hook:

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

## Uninstall hooks

Uninstall hooks (`event = "uninstall"`) run at `unmeld`, in the source's clone,
before the clone is removed. They use the same safety-prompt model as install
hooks: required hooks prompt run / skip / abort-the-unmeld; optional hooks prompt
run / skip; a non-TTY `unmeld` skips them and notes it. `unmeld --uninstall-hook
<cmd>` supplies or overrides the source's declared uninstall hooks. `unmeld
--dangerously-skip-install-hook-check` runs them unattended (the flag name is
reused deliberately). A required uninstall hook that fails or is aborted leaves
the source melded.

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

`<target>` is a source selector (the source's own `[[hooks]]`) or an
`owner/repo#item` ref (that item's hooks); a ref that matches several sources or
items runs each in turn. `--event` selects the lifecycle event (`install`,
`uninstall`, or `build`); `build` is valid only for an item target.

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
a recorded source install hook whether it is pending and the commit it last ran at
-- without running any. It is the read-only companion to `hooks run`.

## Visibility

`recall --sources` marks a source that carries hooks with a count-aware token in
its status bracket (e.g. `1 hook` or `3 hooks`). `mind review <repo>` lists every
declared hook (install and uninstall), showing each hook's command, event, and
whether it is required or optional. `mind hooks list <target>` shows the same
detail plus the pending/last-ran state of recorded install hooks.

`[source].install` is deprecated in favor of the `[[hooks]]` form. See
[The mind.toml file](mind-toml.md) for the schema and
[spec/install-hooks.md](https://github.com/jaemk/mind/blob/main/spec/install-hooks.md)
for the full behavior.
