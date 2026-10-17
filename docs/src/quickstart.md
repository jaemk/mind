# Quickstart

This page installs a first skill, shows where it went, and points at what to
do next. It assumes `mind` and `git` are installed ([Install](install.md)).

<script src="https://asciinema.org/a/qcAxP5PD7H6cuLTE.js" id="asciicast-qcAxP5PD7H6cuLTE" async="true"></script>

## 1. Meld a source

```
mind meld jaemk/mind
```

`meld` clones the repo, lists the items it offers, and asks whether to install
them. Confirm to install all of them, or pick individually. For this repo that
is one example skill, `hello-mind`. It also registers two sources the repo
recommends, `anthropics/skills` and `ComposioHQ/awesome-claude-skills`, without
installing anything from them.

Any repo that ships agent tooling works the same way:

```
mind meld owner/repo
mind meld https://gitlab.example.com/team/agent-tools
mind meld ~/dev/my-skills
```

## 2. See what is installed

```
mind recall
```

`recall` lists each source with its items, marked installed or available. The
skill is now linked at `~/.claude/skills/hello-mind`, and Claude Code offers it
as `/hello-mind`.

## 3. Browse and install more

```
mind probe
```

`probe` opens an interactive browser over every melded source: search, read
descriptions, and install or remove items. See
[Browse in the terminal UI](tui.md). To install by name instead:

```
mind learn skill-creator          # one item
mind learn 'anthropics/skills#*'  # every item of one source
```

## 4. Keep it current

```
mind upgrade
```

`upgrade` fetches the sources your installed items came from, shows what
changed, and asks before applying anything.

## Where next

- Install into Gemini CLI, Codex CLI, or Windsurf too:
  [Use more than one agent](lobes.md). `mind config lobes detect` finds the
  harnesses on this machine.
- Install only part of a large repo:
  [Choose what a source installs](choosing-items.md).
- Install a single skill from a GitHub link:
  [Install one item from a link](item-links.md).
- Learn the vocabulary: [Core concepts](concepts.md).
- Try `mind` against a source you can edit: the
  [starter example](https://github.com/jaemk/mind/tree/main/examples/starter)
  and the other [example sources](examples.md).
