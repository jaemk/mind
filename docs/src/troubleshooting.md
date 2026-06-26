# Troubleshooting

- An item didn't show up in `~/.claude`. Run `mind introspect`; it reports
  missing links and drift, and `mind introspect --fix` recreates missing
  symlinks.
- `learn` refused to overwrite a path. mind will not clobber a file or link it
  did not create (the clobber guard). Move the existing one aside, then `learn`
  again.
- Two sources ship an item with the same name. Namespace one with `mind meld
  <repo> --as <prefix>`, so its items install as `<prefix>-<name>`. See
  [examples/namespacing/](https://github.com/jaemk/mind/tree/main/examples/namespacing).
- Where things live: see [Configuration](configuration.md#paths). Override the
  roots with `MIND_HOME` and `CLAUDE_HOME`.
- Before publishing a source, run `mind review <path>` to check its `mind.toml`,
  item kinds, `{{ns:}}` references, and pin directive. See
  [Authoring a source](authoring.md).
- To authenticate with an SSH key, see [Configuration](configuration.md#ssh-cloning).
- Stuck in the TUI. Press `q` in the main view, or Ctrl-C twice from anywhere
  (search box, dialogs) to force exit.
