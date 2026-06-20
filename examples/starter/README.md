# Starter example

A minimal source repo in the plain convention layout: one skill, one agent, one
rule, and no `mind.toml`. Melding it shows zero-config discovery, since mind
finds items by directory convention with no manifest required.

## Layout

```
skills/greet/SKILL.md    skill, description in frontmatter
agents/scribe.md         agent, description in frontmatter
rules/tone.md            rule, description in frontmatter
```

Each item's `description` comes from its own YAML frontmatter. There is no
`mind.toml`: convention scanning is the default and needs no configuration. Add
a `mind.toml` only to set repo metadata, a prefix, or a non-standard layout (see
[../namespacing/](../namespacing/) for a repo that ships one).

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/starter /tmp/starter
cd /tmp/starter && git init -q && git add -A && git commit -qm init

mind meld /tmp/starter
mind probe --no-tui          # lists greet, scribe, tone with their descriptions
mind learn greet             # links skills/greet into each agent home
mind recall                  # shows greet as installed
```

`probe` matches descriptions too, so `mind probe --no-tui plain` finds `tone` by
its frontmatter text, not just its name.

## Verified

`tests/cli.rs::example_starter_convention_discovery` melds this directory and
asserts the three items are discovered with their descriptions, so the example
stays correct as the code changes.
