# Quickstart

<script src="https://asciinema.org/a/qcAxP5PD7H6cuLTE.js" id="asciicast-qcAxP5PD7H6cuLTE" async="true"></script>

Meld a source and install its items:

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

Agent homes can be Claude Code, Gemini CLI, Codex CLI, or Antigravity -- not just
`~/.claude`. See [Configuration](configuration.md#cross-harness-lobes) for the
per-harness path table and preset commands.

For a self-contained first run with no remote, use the bundled starter source (a
plain convention layout, see
[examples/starter/](https://github.com/jaemk/mind/tree/main/examples/starter)):

```
cp -r examples/starter /tmp/starter
cd /tmp/starter && git init -q && git add -A && git commit -qm init
mind meld /tmp/starter   # prompts to install; confirm to install all three
mind recall
```

[Commands](commands.md) is the full verb reference. [Source layout](source-layout.md)
covers how a source repo exposes items.
