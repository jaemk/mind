# Hooks example

A source repo that declares `[[hooks]]` in `mind.toml`, so `mind` builds the
tooling its skill relies on at meld and tears it down at unmeld.

## What it shows

* Source-level lifecycle hooks: `event = "install"` hooks run on `meld`,
  `event = "uninstall"` hooks run on `unmeld`.
* The disclosure-and-prompt safety model: before any hook runs, `mind` discloses
  the command and asks the user to approve it. A source can run arbitrary code,
  so nothing executes without consent.
* Optional vs required: a required hook prompts run/skip/abort (declining can
  abort the meld); an optional hook (`optional = true`) prompts run/skip only, so
  declining never blocks the meld.
* Non-TTY behavior: with no terminal to prompt on, hooks are skipped rather than
  run blind, unless you pass `--dangerously-skip-install-hook-check` to run them
  unattended.

Convention discovery stays on: there is no `[[items]]` or `[discover]`, so the
skill is found by convention at `skills/probe/SKILL.md`.

## Layout

```
mind.toml                 [source] description + three [[hooks]] (convention scanning stays on)
bin/build.sh              install hook: writes marker files (the helper tooling)
bin/clean.sh              uninstall hook: removes those marker files
skills/probe/SKILL.md     convention skill that relies on the tooling the install hook builds
```

`mind.toml` declares a required install hook (`bash ./bin/build.sh`), an optional
install hook (`bash ./bin/build.sh --cache`), and an uninstall hook
(`bash ./bin/clean.sh`). Each `run` points at a real shipped script, so an
approving user succeeds.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/hooks /tmp/hooks-demo
cd /tmp/hooks-demo && git init -q && git add -A && git commit -qm init
```

Run `mind probe --no-tui hooks-demo` to browse the catalog entry before melding.

When you meld interactively, `mind` discloses each hook and prompts before
running it. For each required hook you will see:

```
====== hook: build helper tooling ======
Source:  localhost/hooks-demo
Pin:     main @ <commit>
Clone:   ~/.mind/sources/localhost/hooks-demo
Command: bash ./bin/build.sh

WARNING: this will execute arbitrary code from the source above.
Run the hook? [Y/n/a] (y=run, n=skip, a=abort):
```

An optional hook (`warm an optional cache`) shows `[Y/n]` with no abort option,
since skipping it never blocks the meld.

```
# Interactive: prompts for each hook in turn.
mind meld /tmp/hooks-demo

# Unattended (e.g. CI / non-TTY): run the install hooks without prompting.
mind meld /tmp/hooks-demo --dangerously-skip-install-hook-check
```

After the install hooks run, confirm they executed by checking the marker files:

```
ls /tmp/hooks-demo/bin/.built
ls /tmp/hooks-demo/bin/.cache
```

Both files are created by `bin/build.sh` (`.cache` only when the optional hook
ran). If a marker is absent, that hook was skipped.

To tear down:

```
mind forget skill:probe
mind unmeld hooks-demo
rm -rf /tmp/hooks-demo
```

`mind unmeld hooks-demo` runs the uninstall hook (`bash ./bin/clean.sh`), which
removes both marker files before the clone and registry entry are dropped.

## Verified

`tests/cli.rs::example_hooks_lists_declared_hooks` melds this directory and
asserts the declared hooks, so the example stays correct as the code changes.

## See also

`../../spec/install-hooks.md` - normative rules for the hook feature. IDs
demonstrated here: HOOK-20 (disclosure header and prompt), HOOK-21 (skip
behavior), HOOK-22 (non-TTY skips hooks), HOOK-23
(`--dangerously-skip-install-hook-check`), HOOK-50 (`[[hooks]]` array),
HOOK-51 (hook fields: `run`, `name`, `optional`, `event`), HOOK-52 (optional
two-way `[Y/n]` prompt), HOOK-54 (uninstall hooks at `unmeld`).
