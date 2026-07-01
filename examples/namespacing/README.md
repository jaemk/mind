# Namespacing example

A small source repo showing how `{{ns:name}}` reference tokens let items in one
source reference each other and survive a namespace prefix.

## Why tokens

A prefix (`meld --namespace <prefix>`, or a repo's `[source].prefix`) installs every
skill, rule, and tool from a source under `<prefix>:<name>`, so two sources can
both ship a `review` without colliding. The Claude harness resolves a skill at
runtime by the name in the text, so a plain-prose reference like "run the review
skill" breaks once `review` installs as `jk:review`.

Authors write intra-source references as `{{ns:name}}` instead. On install, mind
expands each token to that sibling's effective name: the bare name when the
source is unprefixed, or `<prefix>:<name>` when prefixed. A token whose referent
is not a real sibling is a `BadReference` error at install time.

Agents are the exception. The harness keys an agent by its frontmatter `name`,
not its link path, so mind links agents under their bare name even under a prefix
and does not rewrite references to them. A `{{ns:}}` token naming a sibling agent
therefore expands to the bare name in both cases.

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
item, not just at its top level. Under a prefix the skill and rule tokens gain
the prefix (`jk:review`, `jk:style`) while the agent token stays bare (`dev`),
since agents are not namespaced.

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

# Prefixed: skill/rule tokens expand to jk:<name>; the agent token stays bare.
# To switch to a namespace, forget any installed items first (mind errors if items remain).
mind forget lead --yes
mind meld /tmp/ns-demo --namespace jk
mind learn jk:lead --yes
cat ~/.mind/store/agent/jk:lead     # "the dev agent", "the jk:review skill", "the jk:style rule"

# Browse items non-interactively.
mind probe --no-tui
```

Because every sibling reference here is a token, prefixing produces no
unguarded-reference warnings. Change a skill or rule token to bare prose (e.g.
write `review` instead of `{{ns:review}}`) and re-meld with `--namespace` to see
the warning that flags references prefixing would break. A bare agent reference
does not warn, since agents keep their bare name under a prefix.

## Teardown

After following Try it, `jk:lead` and its sibling dependencies are installed. Run in
inverse order:

```
mind forget jk:lead
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
