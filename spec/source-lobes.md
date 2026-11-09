# Source lobes

Status: done. A source, or the consumer melding it, can name a lobe its
items link into in place of the default lobe (`~/.claude`). This lets a
source that keeps itself current with `mind upgrade` from inside a sandbox,
where `~/.claude` cannot be written, link its items somewhere the sandbox
allows. The consumer then points their agent at that directory (e.g. from
their `CLAUDE.md`).

## Declaring it

- `LOBE-1` `[source] lobe = "<dir>"` in a source's `mind.toml` names its lobe.
  It is read from the commit the source's clone is on, like `prune`
  (LIFE-57), so a new commit can add, change, or drop it.
- `LOBE-2` A leading `~` is expanded and a relative `<dir>` is resolved
  against the source's mind home (HOME-6), so `lobe = "claude"` is
  `<home>/claude` and moves with the source when it changes home (HOME-8).
  The result must not be a mind home or lie under one's `store`, `sources`,
  or `.tmp`. When extra mind homes are set (`MIND_HOMES` or the `mind-homes`
  config key, HOME-2), it must also lie under an allowed home; with none set,
  any directory is allowed. Otherwise the lobe is refused (`lobe-not-allowed`,
  exit 1): a `--lobe` before anything is cloned, a source's own `lobe` at
  `meld` before the source is registered (the clone is removed), and one from
  a later commit when an item of the source installs.

- `LOBE-7` `meld --lobe <dir>` (and `learn <url> --lobe <dir>` for an item
  link) names the consumer's lobe for the source and outranks its `[source]
  lobe`; a source's own `lobe` is then not checked. It is recorded on the
  source's registry entry as given, before any item installs. On a source
  already melded it replaces the recorded one and relinks the installed
  items as LOBE-5 does; `--lobe ''` clears it. Giving the lobe already in
  effect changes nothing. It applies to the named source only, not the
  sources it curates. For a plain item ref, `learn --lobe` is ignored with a
  note.
- `LOBE-8` Before a source's items install, a lobe that takes the default
  lobe's place is called out once per source per run, in bold, with the
  install preview ahead of the confirmation (or before the install under
  `--yes`): `<source> links its items into <dir> in place of <default lobe>`,
  ending `, a directory outside every mind home` when it lies under none.
  `upgrade`'s `lobe` line (LOBE-5) marks such a lobe the same way.

## Linking

- `LOBE-3` The lobes an item of a source with a lobe links into are the
  configured lobes (`Paths::agent_homes`) with the default lobe (`~/.claude`,
  or the `MIND_DEFAULT_LOBE` / `CLAUDE_HOME` override) replaced by the source's
  lobe (the consumer's, LOBE-7, else its own). Every other lobe, a project lobe included, still gets its link. The
  source's lobe admits every kind and is created on first link, so it is
  never skipped as unreachable (STO-56). `introspect` checks and `--fix`
  repairs an item's links against the same set, and `forget` confines its
  removals to it (LIFE-44).
- `LOBE-4` A `--local` install (HARN-20) and a managed policy's lobe lock
  (POL-40) are unchanged by a source's `lobe`: they link into exactly the
  lobes they name.
- `LOBE-5` `upgrade` reinstalls an item whose links are not where its
  source's lobe puts them, with or without a content change: a source with a
  lobe whose linkable item has no link under it, or has one under the
  default lobe; or a source with none whose item has a link under no
  configured lobe nor the default lobe (left from a dropped lobe). The report
  shows `lobe    -> <dir>` (or `-> the configured lobes`), and a pass that
  only moves links or homes is headed `N items to move:` when any item
  changes home (HOME-8), else `N items to relink:`. The old links are
  removed.

## Unwritable lobes

- `LOBE-6` For every source, a lobe that cannot be written (permission denied,
  a read-only filesystem) does not fail an install, upgrade, or removal. A
  link already pointing at the item's store copy is left as it is and stays
  recorded, so an in-place upgrade writes nothing in the lobe. A link that
  cannot be made is skipped and not recorded; a stale link that cannot be
  removed is left and stays recorded, so a later run that can write there
  removes it. Each such directory is named once per run in a note (stdout,
  stderr under `--json`, CLI-217).
