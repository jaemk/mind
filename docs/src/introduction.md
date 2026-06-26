# Introduction

`mind` is a manager for agent tooling: skills, agents, rules, and tools. It melds
arbitrary git repos and links the items they offer into one or more agent homes
(default `~/.claude`).

- A *source* is a melded git repo (`mind meld`). It offers *items*: skills,
  agents, rules, and tools, found by convention or declared in a `mind.toml`.
- `mind learn <item>` copies an item into the *store* (`~/.mind/store`) and
  symlinks it into each *lobe* (agent home). A *tool* is the exception: store-only
  helper tooling reached by reference, never linked into a lobe.
- `mind recall` and `mind probe` inspect what is installed and what is available;
  `mind review` and `mind init-source` validate and scaffold a source for
  publishing.

This site is the reference for installing, using, and authoring `mind`. Start
with [Install](install.md) and the [Quickstart](quickstart.md); [Commands](commands.md)
is the full verb reference. For authoring a source, see [Source layout](source-layout.md)
and [Authoring a source](authoring.md). The normative behavior is the
[spec](https://github.com/jaemk/mind/tree/main/spec).
