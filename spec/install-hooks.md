# Install hooks

Status: done. Some agent libraries ship tooling (binaries, scripts) that their
skills or agents depend on, which must be built or installed before the items
work. An install hook is a command a source declares (or a user supplies) that
`mind` runs to perform that setup. Because the command is arbitrary code from the
source, `mind` shows it and prompts before running it.

## Overview

An install hook is a shell command associated with a source. The maintainer
declares it in `mind.toml`, or a user supplies one on the command line (for a repo
that ships no `mind.toml`, or to override the declared one, in which case the
override is shown loudly). The hook runs in the source's clone during `meld`, and
re-runs during `upgrade` when the source updates.

The hook is arbitrary code execution: it can run any command with the user's
privileges. So `mind` never runs a hook without first showing the user what will
run and where it came from (source identity, the pin and commit, the clone path,
and the exact command), with an explicit warning, framed by a header so the
disclosure is visibly distinct from the surrounding output. The interactive
prompt offers run (the default, a bare Enter), skip the hook but still install the
source and its items, or abort and install nothing. In a non-TTY context the hook
is skipped instead (HOOK-22); `--dangerously-skip-install-hook-check` runs it
unattended for users who have already vetted the source. The prompt is the trust
boundary; the shown pin and commit let the user inspect the repo at that exact
point before approving.

The rest of this document states the rules normatively. Source identity is
`host/owner/repo` (see storage.md); a source's pin is its `Pin` (see cli.md,
CLI-17).

## Declaring a hook

- `HOOK-1` `[source].install` in a repo's `mind.toml` is a shell command string
  that declares the source's install hook (run to build or install the tooling
  the source's items rely on). Deprecated in favor of a `[[hooks]]` install entry
  (HOOK-90); still parsed and folded into the hook set (HOOK-50) for back-compat.
- `HOOK-2` `mind meld <repo> --install-hook <cmd>` supplies an install hook, both
  for a repo with no `mind.toml` and to override one a source declares. When it
  overrides a declared `[source].install`, the override is loud and obvious: the
  safety prompt (HOOK-20) shows the source's declared command and the overriding
  command, and states that the user-supplied command is what will run, so the user
  cannot miss that they replaced the maintainer's hook.
- `HOOK-3` With no `[source].install` and no `--install-hook`, `meld` runs no hook
  (behavior is unchanged from a source without one). An empty or whitespace-only value
  for either field is treated the same as absent: no hook runs and nothing is recorded.

## When a hook runs

- `HOOK-10` An install hook runs during `meld`, once, in the source's clone
  directory, after the working tree is checked out at the resolved pin. It is a
  source-level step; `learn` does not run hooks.
- `HOOK-11` `upgrade` re-runs a source's hook (subject to the same prompt) when the
  source has advanced to a new commit and a hook is in effect, so the tooling
  tracks the source. `sync` alone (which only fetches and records the new commit)
  does not run the hook.

## The safety prompt

- `HOOK-20` Before running any hook, `mind` prints a disclosure framed by a
  `====== hook: <name> ======` header (so it is visibly distinct from the
  surrounding `melded <source>` output): the source identity, the resolved pin
  (the branch, tag, or ref) and the commit, the clone path, and the exact command
  that will run, with a clear warning that this executes arbitrary code from the
  source. It then offers `[Y/n/a]`: run the hook (`y`/`Y`/Enter), skip it but
  continue installing the source and its items without building the tooling
  (`n`/`N`), or abort and install nothing (`a`/`A`). The default (a bare Enter) is
  run; an unrecognized reply skips, so an unclear answer never runs the hook. When
  a `--install-hook` overrides the source's declared `[source].install` (HOOK-2),
  the prompt also shows the declared command and states plainly that the
  user-supplied command is replacing it.
- `HOOK-21` Skipping the hook (`n`) still installs the source and its items: only
  the declared tooling is not built. `mind` says so and notes the items may not
  work until the hook is run. Aborting (`a`) installs nothing: the source is not
  registered, as for a declined meld.
- `HOOK-22` When stdin is not a TTY (a script, CI, or managed-policy auto-meld,
  POL-32), `mind` never runs a hook silently and never aborts: regardless of the
  interactive default (HOOK-20), it takes the skip path (the source and its items
  install, the tooling is not built) and reports it, unless
  `--dangerously-skip-install-hook-check` is given.
- `HOOK-23` `--dangerously-skip-install-hook-check` runs the hook without
  prompting, and is what enables hooks in non-interactive use. The flag name is
  deliberately explicit about the risk.

## Execution and recording

- `HOOK-30` A hook runs via the shell in the clone directory with its stdin
  closed (it cannot consume `mind`'s input). Its stdout and stderr are captured
  and mirrored to `mind`'s output under labeled separators -- the captured stdout
  under `====== (hook-stdout: <name>) ======` and the captured stderr under
  `====== (hook-stderr: <name>) ======`, each block omitted when that stream is
  empty, with a closing `====== (end hook: <name>) ======` divider when any output
  was shown -- so the hook's output is clearly demarcated from `mind`'s own and
  from whatever it prints next (e.g. the install preview). A
  non-zero exit is a `HookFailed` error and fails the `meld`: the source is not
  left registered (the clone is removed, as for any failed meld), and the error
  points to the framed output already shown rather than repeating it. Side effects
  the hook already had on the system (an installed binary, a global package) are
  outside `mind`'s state and are not rolled back.
- `HOOK-31` `mind` records the in-effect hook command and the commit it ran at on
  the source registry entry, so `upgrade` can detect a changed command or commit
  and re-prompt (HOOK-11), and `recall` / `introspect` can report that a source
  has an install hook.

## Validation (`mind review`)

- `HOOK-40` `mind review <target>` reports a source's declared `[source].install`
  hook as an advisory finding (showing the command), so a consumer can see, before
  melding, that the source will ask to run code, and a maintainer can confirm the
  hook is what they intend to publish (CLI-130).

## Multiple hooks and lifecycle events

A source may declare more than one hook, for two lifecycle events (install at
`meld`, uninstall at `unmeld`), and mark a hook optional so the user can skip
that step. `[[hooks]]` is the canonical form; the single `[source].install`
string (HOOK-1) is a deprecated shorthand for one required install hook (HOOK-90),
still parsed and folded in (HOOK-50).

- `HOOK-50` A source declares hooks via a `[[hooks]]` array-of-tables in
  `mind.toml`. Each hook runs, in declaration order, in the source's clone
  directory at its lifecycle event. The legacy `[source].install` (HOOK-1) is
  exactly equivalent to one `[[hooks]]` with that command, `optional = false`,
  `event = "install"`, folded in ahead of any declared `[[hooks]]`. An empty or
  whitespace-only `run` is treated as absent (HOOK-3) and contributes no hook.
- `HOOK-51` A `[[hooks]]` entry's fields are: `run` (the shell command,
  required), `name` (an optional label shown in the disclosure; defaults to the
  command), `optional` (bool, default `false` = required), and `event`
  (`"install"` or `"uninstall"`, default `"install"`). An unknown `event` value
  is a `mind.toml` schema error naming the bad value and the legal set.
- `HOOK-52` An optional hook (`optional = true`) is disclosed like any hook but
  prompted with a two-way `[Y/n]` choice: run it (`y`/`Y`/Enter, the default), or
  skip it (`n`/`N`). `optional` means the user may decline to run the step (skip
  it); it offers no abort because skipping is the graceful decline. A required hook
  keeps the three-way `[Y/n/a]` prompt (HOOK-20). The interactive default is run
  for both; in a non-TTY context every hook is skipped instead (HOOK-22), and
  `--dangerously-skip-install-hook-check` runs every hook (HOOK-23), optional and
  required alike. `optional` does NOT make the hook's failure tolerable (HOOK-53).
- `HOOK-53` Any hook's non-zero exit is a hard stop, whether the hook is optional
  or required: at `meld` the clone is removed and nothing is registered
  (HOOK-30); at `unmeld` the unmeld stops and the source remains. `optional`
  governs only whether the user may decline to run the hook (HOOK-52), never
  whether it may fail.
- `HOOK-60` When a hook runs (a chosen run, or an unattended run under
  `--dangerously-skip-install-hook-check`), `mind` prints a line naming the hook
  before running it, so the user sees which step is executing. Re-melding an
  already-melded source (CLI-12) re-offers its install hooks that have not run at
  the source's current commit (a hook skipped at an earlier meld, or added since),
  with the same disclosure and prompt as a fresh meld, before the install step.
  So `mind meld` in an already-melded project still prompts for a pending optional
  or required hook. `meld --force` re-offers ALL of the source's install hooks on
  a re-meld, even those already run at the current commit (alongside forcing the
  clobber overwrite, CLI-35).
- `HOOK-54` Uninstall hooks (`event = "uninstall"`) run at `unmeld`, in the
  source's clone, before the clone and registry entry are removed (so cleanup can
  use the working tree). On the default unmeld path (CLI-21), the multi-item
  removal confirmation (CLI-42) runs first; uninstall hooks only run after the
  user confirms (or `--yes` skips the confirm). A user who declines the confirm
  does not trigger any hook. On the `--unlink-only` path (CLI-22), which has no
  multi-item confirm, hooks run before the source is removed, as before. They use
  the same prompt model as install hooks: required = run / skip / abort-the-unmeld;
  optional = run / skip; a non-TTY `unmeld` skips them and notes it; `mind unmeld
  --dangerously-skip-install-hook-check` runs them unattended. A required
  uninstall hook that fails or is aborted leaves the source melded.
- `HOOK-55` Install hooks are recorded as a set on the source's registry entry
  (`install_hooks`: each an effective command plus the commit it last ran at, or
  null when skipped), superseding the single `[source].install`/commit pair
  (HOOK-31), which is migrated into the set when an older `sources.json` is
  loaded. `upgrade` re-offers each install hook that is pending: a hook is pending
  when its recorded run-commit is null (never ran or was skipped), or when it
  differs from the source's current commit (the source advanced). A null run-commit
  is always treated as pending regardless of whether the source's commit is also
  null (a commitless linked source). Uninstall hooks are not recorded, since they
  only fire at `unmeld`.
- `HOOK-56` `meld --install-hook <cmd>` (HOOK-2) replaces all of a source's
  declared install hooks with one required install hook running `<cmd>`; the loud
  override disclosure (HOOK-2) shows the declared command(s) it replaced. Declared
  uninstall hooks are unaffected by `--install-hook`.
- `HOOK-57` `init-source` scaffolds commented `[[hooks]]` examples in the
  `mind.toml` it writes: at least one install hook and one uninstall hook, with
  one marked `optional = true`, each showing `run`, `name`, and `event`, all
  commented out so they are inert until the maintainer fills them in. The
  `optional` example's comment states that optional lets the user decline running
  the hook (it does not mean the hook may fail).
- `HOOK-58` `recall --sources` marks a source carrying install hooks with a
  count-aware token; `mind review <target>` lists every declared hook (install and
  uninstall), showing each hook's command, event, and whether it is required or
  optional (extending HOOK-40).
- `HOOK-59` `unmeld --uninstall-hook <cmd>` supplies or overrides a source's
  uninstall hook: it replaces all the source's declared uninstall hooks with one
  required uninstall hook running `<cmd>`, shown loudly in the disclosure (the
  uninstall-event counterpart to `meld --install-hook`, HOOK-56). Declared install
  hooks are unaffected.

## Deprecating `[source].install`

- `HOOK-90` `[source].install` (HOOK-1) is deprecated in favor of a `[[hooks]]`
  install entry (HOOK-50/51), which is strictly more expressive (it can be named,
  optional, an uninstall hook, or one of several). The field stays parsed and
  folded into the hook set (HOOK-50), so existing sources keep working unchanged;
  only new authoring is steered away. `mind review <target>` reports a source that
  declares `[source].install` as an advisory `deprecated-field` finding, naming the
  equivalent `[[hooks]]` entry (`run = <cmd>`, `event = "install"`), in addition to
  disclosing the hook itself (HOOK-40). `init-source` never writes `[source].install`:
  its scaffold offers only commented `[[hooks]]` examples (HOOK-57).

## Item build hooks

Source-level hooks (HOOK-50) run once in the clone at `meld`/`unmeld`. Tooling
that an individual item ships and must build before use (a compiled binary, a
vendored dependency) instead uses an item-level build hook: a command tied to one
item that runs when that item is installed, in the item's staging copy, so its
output is captured into the store transactionally. Build hooks back the `tool`
kind and `{{tools:name}}` (tooling.md, TOOL-12), but any kind may declare one.

- `HOOK-70` An item declares a build hook with `build`, a shell command:
  `[[items]].build` in `mind.toml`, or `build:` in a tool's `TOOL.md`
  frontmatter. It is distinct from a source's install/uninstall hooks (HOOK-50):
  it is item-scoped and runs per item at install, not once per source at meld. An
  empty or whitespace-only `build` is treated as absent (HOOK-3).
- `HOOK-71` A build hook runs in the item's staging directory
  (`~/.mind/.tmp/staging/<kind>/<name>/`) as the working directory, after
  reference/token expansion (NS-11, TOOL-13) and before the store swap (LIFE-1),
  so its output lands in the store atomically on success. A non-zero exit is a
  hard stop (HOOK-53): the staging copy is discarded and the live install is left
  untouched (LIFE-4), as for any failed install.
- `HOOK-72` A build hook is arbitrary code, so it is disclosed before running and
  its output is framed (HOOK-30). On a TTY it is prompted two-way: run it, or skip
  it and install the item with its tooling unbuilt (`mind` says so, HOOK-21, and a
  `{{tools:ref}}` then points at an unbuilt path until the build runs). A non-TTY
  context skips it (the item installs unbuilt). Skipping is the graceful decline;
  a build hook offers no abort, so a single item's build never aborts a batch
  install.
- `HOOK-73` A build hook re-runs whenever its item is (re)installed or upgraded,
  since the store copy is rebuilt from staging each time. `learn`/`evolve`/
  `upgrade` disclose and prompt for it as part of installing the item; nothing
  beyond the item's content hash is recorded for it.

## Item install and uninstall hooks

Status: done. A `build` hook (HOOK-70) produces an item's store content in
staging. Separately, an item may need a side-effect command on the host when it
is installed (set up a venv, register a helper) and a matching cleanup command
when it is removed. These are the item-level analog of a source's install and
uninstall hooks (HOOK-50), scoped to one item and tied to its install/removal
rather than to `meld`/`unmeld`. They are distinct from `build`: `build` runs in
staging before the store swap and produces content; an install hook runs after
the item is in place and is for host side effects.

- `HOOK-80` An item declares lifecycle hooks with `install` and `uninstall` shell
  commands: `[[items]].install` / `[[items]].uninstall` in `mind.toml`, or
  `install:` / `uninstall:` in a tool's `TOOL.md` frontmatter. They are valid on
  any kind and are distinct from `build` (HOOK-70). An empty or whitespace-only
  value is treated as absent (HOOK-3).
- `HOOK-81` An item's install hook runs as the final step of installing the item:
  after its store copy is swapped in and its links are created (LIFE-1), in the
  installed store directory (`~/.mind/store/<kind>/<name>/`) as the working
  directory, and after the item's `build` hook (which runs earlier, in staging).
  It runs at `learn` and re-runs whenever the item is reinstalled or upgraded
  (`upgrade`), so the side effect tracks the item's content, mirroring the build
  hook's re-run rule (HOOK-73). A non-zero exit is a `HookFailed` hard stop that
  rolls back that item's install (its store copy and links are removed, leaving it
  not installed); host side effects the hook already performed are outside `mind`'s
  state and are not rolled back (HOOK-30).
- `HOOK-82` An item's uninstall hook runs when the item is removed: at `forget`,
  at `unmeld` (which removes the source's items), and for the OLD item when an
  `upgrade` renames it (a namespace change removes the old item via its registry).
  It runs in the item's store directory before the store copy and links are
  removed, so cleanup can use the installed files. It does NOT run on an in-place
  upgrade (same effective name, content swapped), since the item is not removed,
  only its install hook re-runs (HOOK-81). A non-zero exit is a hard stop
  (HOOK-53): the removal stops and the item is left installed, mirroring a failed
  source uninstall hook leaving the source melded (HOOK-54).
- `HOOK-83` Item install and uninstall hooks are arbitrary code, so each is
  disclosed and its output framed exactly as other hooks (HOOK-20, HOOK-30). On a
  TTY each is prompted two-way (run / skip), like the build hook (HOOK-72): for an
  install hook, skipping installs the item without the side effect; for an
  uninstall hook, skipping removes the item without running cleanup. There is no
  abort, so one item's hook never aborts a multi-item `learn`/`forget`. A non-TTY
  context skips them, and `--dangerously-skip-install-hook-check` runs them
  unattended (HOOK-22, HOOK-23).
- `HOOK-84` Nothing beyond the item's content hash is recorded for an item's
  install/uninstall hooks (as for `build`, HOOK-73): the install hook re-runs on
  every (re)install or upgrade of the item, and the uninstall hook fires on every
  removal. Together with the source-level rule (a source's install hooks re-run
  when the source advances, HOOK-11/55), an install hook re-runs whenever its
  source OR its item is upgraded.
- `HOOK-85` `mind review <target>` lists an item's declared install and uninstall
  hooks as advisory findings, showing each command and event, so a consumer sees
  before installing that an item will run code (the item-level counterpart of the
  source-hook disclosure, HOOK-40/58).

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
