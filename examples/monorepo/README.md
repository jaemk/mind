# Monorepo example

A source repo whose items live under per-package subtrees rather than the repo
root, discovered by setting `[source].roots`.

## What it shows

By default convention discovery scans the repo root for `skills/<name>/SKILL.md`,
`agents/<name>.md`, `rules/<name>.md`, and `tools/<name>/`. In a monorepo the
items live under each package instead. Setting `[source].roots` relocates the
scan: mind looks for the same standard layout under each listed directory rather
than at the repo root.

Convention scanning stays on; `roots` only changes where it scans. An item found
under a root still installs by its bare name (`deploy`, not `web-deploy`), since
roots affects discovery, not the installed name. A prefix is the separate
mechanism for changing installed names.

## Layout

```
mind.toml                                roots = ["packages/web", "packages/cli"]
packages/web/skills/deploy/SKILL.md      skill, discovered under the web root
packages/cli/agents/release.md           agent, discovered under the cli root
```

Each root is scanned with the standard convention layout: `packages/web/skills/`,
`packages/cli/agents/`, and so on. The items are self-contained (no cross-
references) to keep the focus on discovery.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/monorepo /tmp/monorepo-demo
cd /tmp/monorepo-demo && git init -q && git add -A && git commit -qm init

mind meld /tmp/monorepo-demo
mind probe --no-tui            # both items appear: deploy (web), release (cli)

mind learn deploy              # install the web skill from its subtree
mind learn release             # install the cli agent from its subtree
```

Both items are discovered from their subtrees even though nothing sits at the
repo root.

For layouts that even `roots` cannot express (items scattered in non-standard
positions a per-root convention scan would miss), `[discover]` kind globs are the
alternative, e.g. `skills = { include = ["packages/*/skills/*/SKILL.md"] }`. A
skill glob must end at `SKILL.md` (the item is its parent directory); agent and
rule globs match the `.md` file directly (DSC-33).

## Teardown

```
mind forget deploy
mind forget release
mind unmeld /tmp/monorepo-demo
rm -rf /tmp/monorepo-demo
```

## Verified

`tests/cli.rs::example_monorepo_roots_discovery` melds this directory and asserts
both items are discovered from their subtrees, so the example stays correct as
the code changes.

## See also

`../../spec/discovery.md` - normative spec for `[source].roots` discovery and
`[discover]` kind globs (DSC-33, DSC-37).
