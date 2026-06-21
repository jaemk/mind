# Install hooks

Status: planned. Some agent libraries ship tooling (binaries, scripts) that their
skills or agents depend on, which must be built or installed before the items
work. An install hook is a command a source declares (or a user supplies) that
`mind` runs to perform that setup. Because the command is arbitrary code from the
source, `mind` shows it and prompts before running it.

## Overview

An install hook is a shell command associated with a source. The maintainer
declares it in `mind.toml`, or a user supplies one on the command line (for a repo
that ships no `mind.toml`, or to override the declared one, in which case the
override is shown loudly). The hook runs in the source's clone during `meld`, and
re-runs during `evolve` when the source updates.

The hook is arbitrary code execution: it can run any command with the user's
privileges. So `mind` never runs a hook without first showing the user what will
run and where it came from (source identity, the pin and commit, the clone path,
and the exact command), with an explicit warning, and a prompt with three choices:
run the hook and continue, skip the hook but still install the source and its
items, or abort and install nothing. The default never runs the hook.
`--dangerously-skip-install-hook-check` bypasses the prompt for users who have
already vetted the source. The prompt is the trust boundary; the shown pin and
commit let the user inspect the repo at that exact point before approving.

The rest of this document states the rules normatively. Source identity is
`host/owner/repo` (see storage.md); a source's pin is its `Pin` (see cli.md,
CLI-17).

## Declaring a hook

- `HOOK-1` `[source].install` in a repo's `mind.toml` is a shell command string
  that declares the source's install hook (run to build or install the tooling
  the source's items rely on).
- `HOOK-2` `mind meld <repo> --install-hook <cmd>` supplies an install hook, both
  for a repo with no `mind.toml` and to override one a source declares. When it
  overrides a declared `[source].install`, the override is loud and obvious: the
  safety prompt (HOOK-20) shows the source's declared command and the overriding
  command, and states that the user-supplied command is what will run, so the user
  cannot miss that they replaced the maintainer's hook.
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
  exact command that will run, with a clear warning that this executes arbitrary
  code from the source. It then offers three choices: (1) run the hook and
  continue the install; (2) skip the hook but continue, installing the source and
  its items without building the tooling; or (3) abort, installing nothing. The
  default (a bare Enter) is choice 2, so `mind` never runs the hook without an
  explicit choice. When a `--install-hook` overrides the source's declared
  `[source].install` (HOOK-2), the prompt also shows the declared command and
  states plainly that the user-supplied command is replacing it.
- `HOOK-21` Skipping the hook (choice 2, the default) still installs the source
  and its items: only the declared tooling is not built. `mind` says so and notes
  the items may not work until the hook is run. Aborting (choice 3) installs
  nothing: the source is not registered, as for a declined meld.
- `HOOK-22` When stdin is not a TTY (a script, CI, or managed-policy auto-meld,
  POL-32), `mind` never runs a hook silently and never aborts: it takes the skip
  path (the source and its items install, the tooling is not built) and reports
  it, unless `--dangerously-skip-install-hook-check` is given.
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

## Managed-policy composition (research needed)

Install hooks are arbitrary code execution, which is exactly what an enterprise
managed policy (policy.md) exists to control, so the two compose. This section is
NOT yet specified: it records the design space and open questions to research
before any normative rule or stable ID is added. The default in the meantime is
the unmanaged behavior above (prompt, default No; non-TTY skips per HOOK-22).

Open questions to resolve:

- Stance. Should a policy forbid hooks outright (refuse to meld a source that
  declares one, or always skip hooks), allow them with the prompt unchanged, or
  pre-approve a specific set? A locked-down org likely wants "forbid" or
  "pre-approve", not "prompt".
- Pre-approval shape. If pre-approving, what is the unit: a source identity plus an
  expected exact command, plus a pinned commit, so a hook runs unattended only when
  it matches the approved (source, command, commit) triple and is refused otherwise?
- Bypass control. Should a policy be able to disallow
  `--dangerously-skip-install-hook-check` (force the prompt, or forbid running
  hooks at all), so a user cannot opt out of the policy's stance?
- Non-interactive provisioning. `auto_meld` (POL-32) runs during `sync` with no
  TTY, so a declared hook is skipped today (HOOK-22). A policy that provisions a
  source whose tooling is required needs a way to pre-approve that hook, or the
  provisioned source is left without its tooling.
- Audit. Whether a managed deployment should record each hook execution (source,
  command, commit, and how it was approved) for compliance.

The crux is the threat model: a hook is the most dangerous surface mind has, so the
policy's relationship to it should be deliberate. Resolve these before assigning
stable IDs and folding the rules into policy.md.
