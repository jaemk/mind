# Install hooks

Status: planned. Some agent libraries ship tooling (binaries, scripts) that their
skills or agents depend on, which must be built or installed before the items
work. An install hook is a command a source declares (or a user supplies) that
`mind` runs to perform that setup. Because the command is arbitrary code from the
source, `mind` shows it and prompts before running it.

## Overview

An install hook is a shell command associated with a source. The maintainer
declares it in `mind.toml`, or a user supplies one on the command line for a repo
that ships no `mind.toml`. The hook runs in the source's clone during `meld`, and
re-runs during `evolve` when the source updates.

The hook is arbitrary code execution: it can run any command with the user's
privileges. So `mind` never runs a hook without first showing the user what will
run and where it came from (source identity, the pin and commit, the clone path,
and the exact command), with an explicit warning, and a prompt that defaults to
declining. `--dangerously-skip-install-hook-check` bypasses the prompt for users
who have already vetted the source. The prompt is the trust boundary; the shown
pin and commit let the user inspect the repo at that exact point before approving.

The rest of this document states the rules normatively. Source identity is
`host/owner/repo` (see storage.md); a source's pin is its `Pin` (see cli.md,
CLI-17).

## Declaring a hook

- `HOOK-1` `[source].install` in a repo's `mind.toml` is a shell command string
  that declares the source's install hook (run to build or install the tooling
  the source's items rely on).
- `HOOK-2` `mind meld <repo> --install-hook <cmd>` supplies an install hook for a
  source that does not declare one, so a user can run a build for a repo with no
  `mind.toml`. Supplying `--install-hook` for a source whose `mind.toml` already
  declares `[source].install` is a `ConflictingInstallHook` error: the user should
  review the declared hook (HOOK-40) rather than silently override it.
- `HOOK-3` With no `[source].install` and no `--install-hook`, `meld` runs no hook
  (behavior is unchanged from a source without one).

## When a hook runs

- `HOOK-10` An install hook runs during `meld`, once, in the source's clone
  directory, after the working tree is checked out at the resolved pin. It is a
  source-level step; `learn` does not run hooks.
- `HOOK-11` `evolve` re-runs a source's hook (subject to the same prompt) when the
  source has advanced to a new commit and a hook is in effect, so the tooling
  tracks the source. `sync` alone (which only fetches and records the new commit)
  does not run the hook.

## The safety prompt

- `HOOK-20` Before running any hook, `mind` prints the source identity, the
  resolved pin (the branch, tag, or ref) and the commit, the clone path, and the
  exact command, with a clear warning that this executes arbitrary code from the
  source, and then prompts `[y/N]` defaulting to No.
- `HOOK-21` Declining (the default) skips the hook and continues: the source and
  its items still install, with a notice that the declared tooling was not built
  and the items may not work until the hook is run.
- `HOOK-22` When stdin is not a TTY (a script, CI, or managed-policy auto-meld,
  POL-32), `mind` never runs a hook silently: the hook is skipped and reported
  unless `--dangerously-skip-install-hook-check` is given.
- `HOOK-23` `--dangerously-skip-install-hook-check` runs the hook without
  prompting, and is what enables hooks in non-interactive use. The flag name is
  deliberately explicit about the risk.

## Execution and recording

- `HOOK-30` A hook runs via the shell in the clone directory. A non-zero exit is a
  `HookFailed` error and fails the `meld`: the source is not left registered (the
  clone is removed, as for any failed meld). Side effects the hook already had on
  the system (an installed binary, a global package) are outside `mind`'s state
  and are not rolled back.
- `HOOK-31` `mind` records the in-effect hook command and the commit it ran at on
  the source registry entry, so `evolve` can detect a changed command or commit
  and re-prompt (HOOK-11), and `recall` / `introspect` can report that a source
  has an install hook.

## Validation (`mind review`)

- `HOOK-40` `mind review <target>` reports a source's declared `[source].install`
  hook as an advisory finding (showing the command), so a consumer can see, before
  melding, that the source will ask to run code, and a maintainer can confirm the
  hook is what they intend to publish (CLI-130).
