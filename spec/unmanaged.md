# Unmanaged lobe items

Status: planned. An agent home (lobe) often holds skills, agents, and rules that
`mind` did not install: files a user wrote by hand, or items installed by other
means. `mind` surfaces these *unmanaged* items in `recall` and `probe` so the
listing reflects everything the agent actually sees, and lets `forget` remove one
after a distinct confirmation that names it as not managed by `mind`.

"item", "source", "store", and "link" are as in [README.md](README.md). The
manifest and its per-item `links` are defined in [storage.md](storage.md).

## Detection

- `UNM-1` An *unmanaged item* is an entry in a configured agent home's kind
  directory (`skills/<name>/`, `agents/<name>.md`, `rules/<name>.md`) whose path
  is not a managed link, i.e. not recorded in any manifest item's `links`
  (storage.md STO-21). Its kind is the directory's kind and its name is the entry
  name (the basename, with a trailing `.md` stripped for an agent or rule). A
  managed link occupies its path, so a given path is either managed or unmanaged,
  never both. Tools are never linked into an agent home (tooling.md TOOL-3), so
  unmanaged detection covers skills, agents, and rules only. `mind` scans every
  configured lobe (`Paths::agent_homes`, STO-14); an item present in more than one
  lobe is one logical item that records each occupied path.

## recall

- `UNM-2` `recall` (the no-argument status view, CLI-70) lists unmanaged items
  after the melded sources, in a clearly labeled group ("unmanaged: not installed
  by mind"), one row per item showing its `kind:name` and the lobe path(s) it
  occupies. They are shown by default (no flag needed); `--kind` filters them as
  it filters managed items, and `--source` excludes them (they have no source).
  `--json` (CLI-73) adds a top-level `unmanaged` array alongside the sources.

## probe

- `UNM-3` `probe` includes unmanaged items in its listing and its substring
  search (CLI-85 matches their name), in a synthetic group distinct from any
  source. The non-TUI and `--json` listings mark each row `unmanaged: true`; the
  interactive TUI (tui.md) shows them under an "unmanaged" group node that is
  browsable, searchable, and selectable like a source's items. `--kind` filters;
  `--source` excludes them.

## forget

- `UNM-4` `forget <ref>` whose ref resolves to an unmanaged item removes the lobe
  entry itself (the directory, file, or foreign link); there is no store copy or
  manifest entry to remove, and the manifest is left unchanged. An unmanaged item
  is matched only by an exact ref (its `kind:name`), never by a glob: a broad
  `forget '*'` removes managed items only and never deletes a user's own files.
  When a bare name matches both a managed and an unmanaged item, the managed
  `forget` ambiguity rules apply (CLI-40): a kind prefix disambiguates.
- `UNM-5` Removing an unmanaged item always prompts first, regardless of count,
  and the prompt states explicitly that the item is not managed by `mind` and that
  removal deletes the user's own file or directory (not just a symlink). `--yes`
  proceeds after that statement; a non-TTY run without `--yes` refuses with
  `ConfirmationRequired` and removes nothing. The clobber/`--force` flags
  (LIFE-41) do not apply, since nothing is being overwritten.
