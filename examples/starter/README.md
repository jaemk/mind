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
workflows/hello.js       workflow, description in its `export const meta` object
```

Each item's `description` comes from its own YAML frontmatter, or a workflow's
`meta` object. There is no
`mind.toml`: convention scanning is the default and needs no configuration. Add
a `mind.toml` only to set repo metadata, a namespace, or a non-standard layout (see
[../namespacing/](../namespacing/) for a repo that ships one).

`ship` is a `command` item (CMD-1): a Claude Code slash command found by
convention at `commands/<name>.md`, the same shape as an agent or a rule. It
installs into the agent home's `commands/` directory and the harness offers it
as `/ship` (CMD-5).

`hello` is a `workflow` item (WF-1): a `.js` file found by convention at
`workflows/<name>.js`, installed into the agent home's `workflows/` directory
(WF-10). It is the one kind with no YAML frontmatter, so its description and
`whenToUse` are read from the `export const meta` object the harness already
requires (WF-4, WF-51). Its `meta.name` is the `{{ns:hello}}` token, which
expands to `hello` when unprefixed and `<prefix>:hello` under a namespace, so the
harness name stays in step with the installed name (WF-23, WF-24).

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
to install all five (greet, scribe, tone, ship, hello):

```
mind meld /tmp/starter       # prompts to install; confirm to install all five
mind probe --no-tui          # lists greet, scribe, tone, ship, hello with their descriptions
mind recall                  # shows all five as installed
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

`../../spec/workflows.md` - the `workflow` kind `hello` demonstrates: WF-1
(convention path), WF-4/WF-51 (description and `whenToUse` from `meta`), WF-10
(store path and link target), WF-23/WF-24 (the `meta.name` token tracking the
prefix).

## Verified

`tests/cli.rs::example_starter_convention_discovery` melds this directory and
asserts the items are discovered with their descriptions, so the example stays
correct as the code changes. `tests/cli_examples_commands.rs` melds it and
asserts the `ship` command installs and links at `commands/ship.md`, that
`hello` installs and links at `workflows/hello.js`, that its `meta` object is
really read, and that its `meta.name` token expands to `hello` unprefixed and
`jk:hello` under a namespace prefix.
