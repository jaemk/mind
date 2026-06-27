# Namespacing example

A small source repo showing how `{{ns:name}}` reference tokens let items in one
source reference each other and survive a namespace prefix.

## Why tokens

A prefix (`meld --as <prefix>`, or a repo's `[source].prefix`) installs every
item from a source under `<prefix>-<name>`, so two sources can both ship a
`review` without colliding. The Claude harness resolves agents and skills at
runtime by the name in the text, so a plain-prose reference like "delegate to
the dev agent" breaks once `dev` installs as `jk-dev`.

Authors write intra-source references as `{{ns:name}}` instead. On install, mind
expands each token to that sibling's effective name: the bare name when the
source is unprefixed, or `<prefix>-<name>` when prefixed. A token whose referent
is not a real sibling is a `BadReference` error at install time.

## Layout

```
agents/lead.md           references {{ns:dev}}, {{ns:review}}, {{ns:style}}
agents/dev.md            no references
skills/review/SKILL.md   references {{ns:style}} (skill -> rule, cross-kind)
rules/style.md           no references
mind.toml                [source] description only (convention scanning stays on)
```

`lead` references three siblings across all three kinds; `review` references a
rule from inside a skill directory, showing tokens expand in every file of an
item, not just at its top level.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/namespacing /tmp/ns-demo
cd /tmp/ns-demo && git init -q && git add -A && git commit -qm init

# Unprefixed: tokens expand to the bare names.
mind meld /tmp/ns-demo
mind learn lead --yes      # --yes confirms the dep-closure prompt
cat ~/.mind/store/agent/lead        # "the dev agent", "the review skill", ...

# Prefixed: tokens expand to jk-<name>.
# Re-melding an already-registered source with --as updates its prefix in place
# (CLI-12, CLI-13) and renames any already-installed items; no unmeld needed.
mind meld /tmp/ns-demo --as jk
mind learn jk-lead --yes
cat ~/.mind/store/agent/jk-lead     # "the jk-dev agent", "the jk-review skill", ...

# Browse items non-interactively.
mind probe --no-tui
```

Because every sibling reference here is a token, prefixing produces no
unguarded-reference warnings. Change one token to bare prose (e.g. write `dev`
instead of `{{ns:dev}}`) and re-meld with `--as` to see the warning that flags
references prefixing would break.

## Teardown

Re-melding with `--as jk` renames the previously installed `lead` to `jk-lead`
(CLI-13), and learning `jk-lead` pulls in its sibling dependencies. Run in
inverse order:

```
mind forget jk-lead
mind unmeld ns-demo
rm -rf /tmp/ns-demo
```

## Verified

`tests/cli.rs::example_namespacing_expands_references` melds this directory and
asserts the expansion, so the example stays correct as the code changes.

## See also

`../../spec/namespacing.md` - normative spec for prefix namespacing (NS-1, NS-2),
`{{ns:}}` reference tokens (NS-10, NS-11), and the unguarded-reference warning
(NS-20).
