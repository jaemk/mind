# Quickstart

<script src="https://asciinema.org/a/qcAxP5PD7H6cuLTE.js" id="asciicast-qcAxP5PD7H6cuLTE" async="true"></script>

Meld a source and install its items:

```
mind meld jaemk/mind   # clone this repo, prompt to install its items
mind recall            # list what's installed
```

`mind meld jaemk/mind` registers the `hello-mind` example skill plus two
curated sources it points at, `anthropics/skills` and
`ComposioHQ/awesome-claude-skills`, without installing their items. Melding
any other repo works the same way:

```
mind meld owner/repo   # clone and prompt to install items
mind recall            # list what's installed
```

`meld` presents available items and prompts to install. Confirm to install all, or
select individually. To register without installing and choose items later:

```
mind meld owner/repo --register-only   # register only, skip install prompt
mind probe                              # browse available items (interactive TUI)
mind learn <item>                       # install a specific item
mind recall                             # list what's installed
```

Agent homes can be Claude Code, Gemini CLI, Codex CLI, Antigravity, or Windsurf
-- not just `~/.claude`. Run `mind config lobes detect` to find which of these
are installed and add matching lobes (Windsurf is project-scoped, so it gets a
`mind link-project` hint instead of an auto-added lobe). See
[Configuration](configuration.md#cross-harness-lobes) for the per-harness path
table and preset commands.

To try mind's install flow against a source you can inspect and edit freely,
clone the repo and meld the bundled starter example (a plain convention
layout, see
[examples/starter/](https://github.com/jaemk/mind/tree/main/examples/starter)):

```
git clone --depth 1 https://github.com/jaemk/mind /tmp/mind-repo
cp -r /tmp/mind-repo/examples/starter /tmp/starter
cd /tmp/starter && git init -q && git add -A && git commit -qm init
mind meld /tmp/starter   # prompts to install; confirm to install all three
mind recall
```

[Commands](commands.md) is the full verb reference. [Source layout](source-layout.md)
covers how a source repo exposes items.
