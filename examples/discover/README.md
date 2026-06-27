# Discover kind globs example

A source repo whose items sit in non-standard positions, discovered by
`[discover]` kind globs with explicit per-kind `include` and `exclude` lists.

## What it shows

`[discover]` kind globs find items in layouts that neither plain convention nor
`[source].roots` capture cleanly. Each kind (`skills`, `agents`, `rules`) is a
table with an `include` glob list and an optional `exclude` glob list, relative
to the repo root.

Declaring `[discover]` item globs makes `mind.toml` authoritative: convention
scanning is turned off and only what the globs match is offered.

A skill glob ends at `SKILL.md` and the item is its parent directory; agent and
rule globs match the `.md` file directly (DSC-33). Include globs match first,
then any path also matched by an exclude glob is dropped (DSC-37), so the skill
under `internal/` is not offered.

## Layout

```
mind.toml                              [discover] kind globs (authoritative; convention off)
packages/a/skills/alpha/SKILL.md       skill 'alpha', matched by skills include -> offered
packages/b/agents/beta.md              agent 'beta', matched by agents include -> offered
internal/skills/secret/SKILL.md        skill 'secret', dropped by exclude internal/** -> NOT offered
```

The globs in `mind.toml`:

```toml
[discover]
skills = { include = ["packages/*/skills/*/SKILL.md"], exclude = ["internal/**"] }
agents = { include = ["packages/*/agents/*.md"] }
```

`secret`'s `SKILL.md` matches the `skills` include glob, but `exclude =
["internal/**"]` drops every path under `internal/`, so it never reaches the
catalog.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/discover /tmp/discover-demo
cd /tmp/discover-demo && git init -q && git add -A && git commit -qm init

mind meld /tmp/discover-demo
mind probe --no-tui            # alpha (skill) and beta (agent) appear; secret does NOT
```

`probe` offers only `alpha` and `beta`; `secret` never appears because the
exclude glob drops it. Install the offered items:

```
mind learn alpha
mind learn beta
```

## Teardown

```
mind forget alpha
mind forget beta
mind unmeld /tmp/discover-demo
rm -rf /tmp/discover-demo
```

## Verified

`tests/cli.rs::example_discover_kind_globs` melds this directory and asserts the
glob-matched items are offered and the excluded one is not, so the example stays
correct as the code changes.

## See also

`../../spec/discovery.md` - normative spec for `[discover]` kind globs: the
include/exclude glob shape and skill-vs-agent matching (DSC-33), and the
include-then-exclude ordering (DSC-37).
