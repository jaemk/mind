# Quickstart

Meld a source, install an item, and see it linked into `~/.claude`:

```
mind meld owner/repo        # clone and register a source repo
mind probe                  # browse and search available items (interactive)
mind learn <item>           # install one into each agent home
mind recall                 # list what's installed
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
mind meld /tmp/starter
mind learn greet
```

[Commands](commands.md) is the full verb reference. [Source layout](source-layout.md)
covers how a source repo exposes items.
