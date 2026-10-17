# Introduction

`mind` installs agent tooling (skills, agents, rules, commands, workflows, and
tools) from git repos into your agent homes, and keeps it up to date.

Point it at any repo that ships agent tooling and it finds the items by their
layout, with no manifest required. It copies the items you choose into a store
it manages and links them into `~/.claude` (or Gemini CLI, Codex CLI,
Windsurf, or any directory you name). Later it tells you what changed upstream
and upgrades on your say-so.

```
mind meld anthropics/skills     # register a repo and pick items to install
mind probe                      # browse everything your repos offer
mind upgrade                    # pull upstream changes into installed items
```

## Who it is for

- **People using agent tooling.** Install skills from several repos without
  copying files around, see what is installed and where it came from, update or
  roll back, and keep more than one agent harness in step.
- **Teams.** Share one curated set of sources, reproduce a setup on a new
  machine with one command, and (for an organization) restrict which sources
  may be installed.
- **People publishing agent tooling.** Any repo is installable as it is. An
  optional `mind.toml` adds a namespace, install profiles, hooks, or an
  explicit inventory.

## Where to go next

- New here: [Install](install.md), then the [Quickstart](quickstart.md).
- The words `mind` uses (meld, learn, lobe, store): [Core concepts](concepts.md).
- A specific task: the **Using mind** section in the sidebar, one page per goal.
- Publishing a repo: [Publish your first source](authoring.md).
- Every verb and flag: the [Commands](commands.md) reference.

The normative behavior is the
[spec](https://github.com/jaemk/mind/tree/main/spec); these pages describe the
same behavior for people using it.
