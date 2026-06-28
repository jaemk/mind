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
- A `mind.toml` change is rejected as a parse error. The file is strict
  (`deny_unknown_fields`), so a misspelled key is a hard error, not silently
  ignored (DSC-30). Common near-misses: pin keys are hyphenated
  (`follow-branch`, `pin-tag`, `pin-ref`), not underscored; `min-mind-version`
  likewise uses a hyphen; and entry keys under `[discover].sources` are exact
  (DSC-31). Run `mind review <path>` to check the file before melding.
- A source's items are not discovered after setting `roots` in `mind.toml`.
  `roots = []` (an explicit empty list) is distinct from omitting the key: it
  scans zero roots and finds nothing. Omitting `roots` (or removing the key)
  keeps the default behavior of scanning the repo root (DSC-50). Set the actual
  subdirectory paths, or remove `roots` entirely.
- `learn` reports `LinkOccupied` and refuses to overwrite. The clobber guard
  will not replace a path that mind did not create (LIFE-41). Move the existing
  file aside, then `learn` again, or pass `learn --force` (CLI-35) to replace
  it unconditionally. Note: on non-unix platforms mind cannot always recognize
  its own copies (symlink ownership is not detectable), so a reinstall or
  `upgrade` may report `LinkOccupied` for items mind did install; this is a
  documented platform limitation (see the lifecycle.md platform note).
- An item shows as out of date in `recall`/`probe` without an upstream change.
  Editing a store or source file by hand changes its content hash; mind compares
  source-content hashes and reports the delta as drift (LIFE-33, CLI-75). Either
  re-`sync` and then `upgrade` the item, or restore the edited file to its
  original content. See [Configuration](configuration.md) for where store and
  source files live.
- `meld` or `sync` fails with "git executable not found". mind shells out to
  `git` for all clone and fetch operations; put `git` on your PATH first. See
  the Install page.
- A skill links to a Gemini or Codex lobe but a rule does not. Rules have no
  cross-harness directory equivalent and are Claude-only (HARN-3). Only skills
  and agents are linked into non-Claude lobes. If the lobe was added via a
  preset, this is expected; rules remain in `~/.claude` only.
- An agent's tool permissions don't work in Gemini or Codex after linking. mind
  links files verbatim and does not rewrite frontmatter. A skill or agent whose
  frontmatter uses Claude-specific keys (e.g. the `tools:` allow-list schema)
  will link correctly but those keys may be ignored or produce a warning in the
  target harness. Adapt the frontmatter for the target harness by hand (HARN-6).
  See [Configuration](configuration.md#frontmatter-portability).
