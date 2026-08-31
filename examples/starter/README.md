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
commands/ship.md         command, description in frontmatter
```

Each item's `description` comes from its own YAML frontmatter. There is no
`mind.toml`: convention scanning is the default and needs no configuration. Add
a `mind.toml` only to set repo metadata, a namespace, or a non-standard layout (see
[../namespacing/](../namespacing/) for a repo that ships one).

`ship` is a `command` item (CMD-1): a Claude Code slash command found by
convention at `commands/<name>.md`, the same shape as an agent or a rule. It
installs into the agent home's `commands/` directory and the harness offers it
as `/ship` (CMD-5).

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
```

The default flow: `meld` clones and prompts to install available items. Confirm
to install all four (greet, scribe, tone, ship):

```
mind meld /tmp/starter       # prompts to install; confirm to install all four
mind probe --no-tui          # lists greet, scribe, tone, ship with their descriptions
mind recall                  # shows all four as installed
```

`probe` matches descriptions too, so `mind probe --no-tui plain` finds `tone` by
its frontmatter text, not just its name. Note: bare `mind probe` launches the TUI;
pass `--no-tui` for non-interactive output.

To register without installing and choose items individually, use `--register-only`:

```
mind meld /tmp/starter --register-only   # register only, skip install prompt
mind probe --no-tui                      # browse available items
mind learn greet                         # install one item
```

### Teardown

```
mind unmeld starter    # uninstalls items and drops the source
rm -rf /tmp/starter
```

## See also

`../../spec/discovery.md` - convention-discovery feature IDs demonstrated here:
DSC-1 (zero-config default, no manifest required), DSC-36 (repo with no
`mind.toml` uses pure convention scanning).

`../../spec/commands.md` - the `command` kind `ship` demonstrates: CMD-1
(convention path, name from the file stem), CMD-5 (store path and link
target).

## Verified

`tests/cli.rs::example_starter_convention_discovery` melds this directory and
asserts the items are discovered with their descriptions, so the example stays
correct as the code changes. `tests/cli_examples_commands.rs` melds it and
asserts the `ship` command installs and links at `commands/ship.md`.
