# Starter example

This is the most common way to use mind: meld an arbitrary existing repo that
you did not author and did not modify. Convention discovery (DSC-1) finds items
by directory layout with no `mind.toml` required and no changes to the source
repo. Any repo that follows the convention can be melded as-is.

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

In real use you skip the local copy entirely and run:

```
mind meld owner/repo
```

against any existing GitHub repo that follows the convention. The `/tmp` copy in
"Try it" below is only necessary because this example lives inside the mind repo
and must be its own git repo to meld.

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
its frontmatter text, not just its name. Note: bare `mind probe` launches the TUI;
pass `--no-tui` for non-interactive output.

### Teardown

Undo the demo in reverse order:

```
mind forget greet
mind unmeld local/tmp/starter
rm -rf /tmp/starter
```

## See also

`../../spec/discovery.md` - convention-discovery feature IDs demonstrated here:
DSC-1 (zero-config default, no manifest required), DSC-36 (repo with no
`mind.toml` uses pure convention scanning).

## Verified

`tests/cli.rs::example_starter_convention_discovery` melds this directory and
asserts the three items are discovered with their descriptions, so the example
stays correct as the code changes.
