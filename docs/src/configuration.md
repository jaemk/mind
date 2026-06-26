# Configuration

## Agent homes (lobes)

`learn` links items into every configured agent home (a *lobe*). Each item is
linked under its kind subdirectory: `skills/`, `agents/`, `rules/`. The default
lobe is `~/.claude`. Configure more in `~/.mind/config.toml`:

```toml
lobes = ["~/.claude", "~/.config/some-other-agent"]
```

The file is created with the default lobe (`~/.claude`) on first use. For a single
invocation, set `MIND_AGENT_HOMES` to a `:`-separated path list instead.

## SSH cloning

To authenticate with an SSH key instead of an https username/password, meld the
`git@host:owner/repo` form, or set `ssh = true` in `~/.mind/config.toml` so the
`owner/repo` shorthand clones over SSH. An https remote still prompts (or uses a
credential helper) as git normally does.

## Paths

- sources clone under `~/.mind`
- installed copies live in `~/.mind/store`
- the source registry is `~/.mind/sources.json`
- config is `~/.mind/config.toml`

Override the roots with `MIND_HOME` (the `~/.mind` tree) and `CLAUDE_HOME` (the
default lobe).
