# Publish your first source

A source is a git repo. `mind` melds any repo with no configuration, so
publishing one is: lay the items out by convention and push.

## A minimal source

```
myrepo/
  skills/greet/SKILL.md      # a skill (the whole directory is the item)
  agents/reviewer.md         # optional: an agent
  rules/style.md             # optional: a rule
```

`skills/greet/SKILL.md`:

```markdown
---
name: greet
description: Greet the user by name and summarize the current directory.
---

When asked to greet someone, ...
```

The `description` in each item's frontmatter is what `mind probe` and
`mind recall` show, so write it for someone deciding whether to install.

Push it, and anyone can install from it:

```
mind meld owner/myrepo       # finds the items by convention, offers to install
```

Check what a meld will find before you push, from inside the repo:

```
mind review                  # validate the source in the current directory
mind meld . --register-only  # register it locally and browse with mind probe
```

See [examples/starter](https://github.com/jaemk/mind/tree/main/examples/starter)
for a minimal repo of this shape, and
[Source layout and item kinds](source-layout.md) for every kind and path.

## Skills at the repo root

If your skill directories sit at the root with no `skills/` container
(`<repo>/greet/SKILL.md`), declare a flat layout:

```toml
# mind.toml
[source]
flat-skills = true
```

It applies to skills only; agents, rules, and tools keep their directories. A
consumer can force it for a repo that has not declared it with
`mind meld owner/repo --flat-skills`.

## Already a Claude plugin?

A repo with `.claude-plugin/plugin.json` or `.claude-plugin/marketplace.json`
is meldable as it is, with no `mind.toml`. See
[Claude plugins and marketplaces](marketplace.md).

## What to add when you need it

Everything below is optional. A `mind.toml` at the repo root enables it; see
[The mind.toml file](mind-toml.md) for the full schema.

| you want to | see |
|-------------|-----|
| describe the source and set a default namespace | [`[source]`](mind-toml.md#source---repo-metadata) |
| let consumers pick a basic or full set | [Offer install profiles](authoring-profiles.md) |
| have items reference each other safely | [Namespaces and references](namespacing.md) |
| share a script between several skills | [Share helper scripts](tooling.md) |
| ship a data file or template items read | [Share data files](resources.md) |
| build or install tooling when the source is melded | [Run setup with hooks](install-hooks.md) |
| make one item pull in another | [Dependencies between items](dependencies.md) |
| ship only some files, or use a non-standard layout | [`[[items]]` and `[discover]`](mind-toml.md#items---explicit-inventory-authoritative) |
| list other people's repos as a collection | [Curate other repos](curating.md) |
| require a minimum `mind` version | `min-mind-version` in [`[source]`](mind-toml.md#source---repo-metadata) |

Before publishing, run [`mind review`](validate.md) to catch broken references,
malformed configuration, and fragile paths.
