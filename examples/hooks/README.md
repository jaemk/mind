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

# Interactive: mind discloses each install hook and prompts before running it.
mind meld /tmp/hooks-demo

# Unattended (e.g. CI / non-TTY): run the install hooks without prompting.
mind meld /tmp/hooks-demo --dangerously-skip-install-hook-check
```

`mind unmeld hooks-demo` runs the uninstall hook, removing the marker files the
install hooks wrote.

## Verified

`tests/cli.rs::example_hooks_lists_declared_hooks` melds this directory and
asserts the declared hooks, so the example stays correct as the code changes.
