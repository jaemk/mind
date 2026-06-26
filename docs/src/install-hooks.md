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

## Visibility

`recall --sources` marks a source that carries hooks with a count-aware token in
its status bracket (e.g. `1 hook` or `3 hooks`). `mind review <repo>` lists every
declared hook (install and uninstall), showing each hook's command, event, and
whether it is required or optional.

`[source].install` is deprecated in favor of the `[[hooks]]` form. See
[The mind.toml file](mind-toml.md) for the schema and
[spec/install-hooks.md](https://github.com/jaemk/mind/blob/main/spec/install-hooks.md)
for the full behavior.
