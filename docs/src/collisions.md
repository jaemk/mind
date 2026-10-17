# Fix name collisions

Two sources can each ship an item with the same name, such as two skills called
`review`. Both would install to `~/.claude/skills/review`, so melding the
second fails with a name-collision error that lists the conflicting items and
suggests a namespace. Give one source a namespace and its items install as
`<namespace>:<name>` instead:

```
mind meld acme/skills --namespace acme      # its review installs as acme:review
```

Claude Code then offers `/acme:review` alongside `/review`.

## Namespacing a source

- `--namespace <prefix>` (short `-N`) at meld time sets it. It overrides any
  namespace the source declares for itself in its `mind.toml`.
- `--namespace ''` removes a namespace the source declares.

The namespace is part of the source's identity. Melding a repo you already have
under a different namespace registers a second, separate instance
(`github.com/acme/skills@acme`) with its own clone and items; it does not
rename the first. Address that instance by its full identity:

```
mind upgrade 'github.com/acme/skills@acme#*'
mind unmeld github.com/acme/skills@acme
```

To change the namespace of a source with nothing installed yet, use **Set
namespace** in the terminal UI's source details dialog (see
[Browse in the terminal UI](tui.md#details-dialog)). Once items are
installed, `unmeld` and meld again with the new namespace.

## Agents collide differently

The harness identifies an agent by the `name` in its frontmatter, not by its
file name, so a namespace cannot rename an agent. `mind` links agents under
their bare name even in a namespaced source. Two sources shipping an agent with
the same frontmatter `name` therefore still collide: `learn` fails with an
`AgentCollision` error naming the agent already installed, and `meld` warns
about it in advance.

To resolve it, `forget` the agent you do not want before installing the other.

## When a namespaced item breaks

Items that mention each other by name (a skill that says "hand off to the
`dev` agent") can break under a namespace, since `dev` is now `acme:dev`.
Authors guard against this with `{{ns:name}}` references, which `mind` rewrites
at install. When melding a namespaced source, `meld --verbose` warns about
items that name a sibling in plain text. See
[Namespaces and references](namespacing.md).

## Commands and skills with the same name

`commands/foo.md` and `skills/foo/SKILL.md` are different items
(`command:foo` and `skill:foo`) that link into different directories, so
`mind` does not treat them as a collision. Both offer `/foo` to Claude Code.
Install only one unless you want both.
