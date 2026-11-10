# Mind homes

Status: done. A source can be melded into a mind home other than `~/.mind`,
so a source can live under a directory a sandbox allows writes to while the
rest stay in `~/.mind`. Every command sees the sources and items of all homes
as one set.

## Allowed homes

- `HOME-1` The default home is `$MIND_HOME` if set, else `~/.mind` (STO-1). It
  is always allowed, and it alone holds `config.toml`, the hook history and
  logs, the auto-update state, and the `absorb` personal source. When the
  default home cannot be written, a hook record or the auto-update state is
  dropped with a warning and the command carries on.
- `HOME-2` The extra homes are the entries of `$MIND_HOMES` (`:`-separated),
  then the `mind-homes` config key (a list of paths), in that order. A leading
  `~` is expanded; a relative `$MIND_HOMES` entry is resolved against the
  working directory and a relative `mind-homes` entry against the default
  home; duplicates and the default home are dropped. An empty entry is ignored.
  With both empty, the `source-homes` config key mind records (HOME-17) gives
  the extra homes. `config show` lists the resolved extra homes (`mind_homes`
  under `--json`).

## Layout

- `HOME-3` Each home is a full mind tree: its own `sources.json`,
  `manifest.json`, `sources/` clones, `store/`, `.tmp/`, and `.lock`. The
  registry and manifest a command works on are the union of every allowed
  home's files, the default home first. Saving writes each source and item back
  to its own home's file, and a home whose file would not change is not
  written, so a command that touches only one home's sources writes nothing in
  another. A source or item name present in two homes is read from the first
  and the later one is ignored with a warning; a save writes the ignored entry
  back to its own home's file unchanged, so it comes into view once the first
  is removed.
- `HOME-6` A source's clone, store copies, staging and backup space, and items
  hook document all live under its home. A home that is no longer allowed is
  not read, so its sources drop out of view until it is listed again.
- `HOME-14` Throwaway work goes where it can be written. `absorb` keeps its
  backup of the lobe item in the destination source's home. `review`'s clone
  and the `probe` TUI's preview clone, tied to no source, go in the `.tmp` of
  the first allowed home (the default first) that can be written.
- `HOME-7` A manifest entry's `store` is relative to the default home for an
  item there (unchanged), and the absolute store path for an item in an extra
  home, which is what routes the entry back to its home on save. An absolute
  entry under no allowed home is treated as the default home's: it is saved to
  the default manifest, and `forget` refuses to remove a store path outside the
  default store root (LIFE-44).

## Choosing a home

- `HOME-4` `meld --home <dir>` melds the named source, and every source it
  curates, into `<dir>`. `<dir>` must be an allowed home (compared after `~`
  expansion and making it absolute, HOME-17); any other value is refused
  before anything is cloned (`home-not-allowed`, exit 1), naming the reason.
- `HOME-5` Without `--home`, a source's `[source] mind-home = "<dir>"` in its
  `mind.toml` chooses its home. It must be an allowed home (HOME-17);
  otherwise the meld is refused (`home-not-allowed`) and the clone removed.
  `--home` outranks it.
  The first clone, made before the `mind.toml` can be read, goes into the home
  the meld started in (the default home, or the curator's), and moves once the
  declared home is known.
  A curated source with no `mind-home` of its own goes into its curator's
  home, whether the curator's first meld, `sync`'s re-walk (DSC-57), or
  `curate` registers it.
- `HOME-17` A home named by `--home`, a `[source] mind-home`, or a curator
  entry's `mind-home` is allowed when it is the default home or an extra
  home. When the user has set no extra homes (`MIND_HOMES` and `mind-homes`
  both empty), any other absolute directory is allowed too, unless it lies
  under a mind home's `store`, `sources`, or `.tmp`, is a lobe or lies under
  one, or is a directory with contents but no `sources.json` or
  `manifest.json` (not a mind tree). Before
  a source's registry entry is first written in such a home, the home is
  appended to the `source-homes` config key (absolute paths) and called out
  in bold: `<source> is melded into <dir>, a new mind home, recorded under
  source-homes in <config>` (stdout, stderr under `--json`). While the user
  sets no extra homes, the `source-homes` entries are the extra homes
  (HOME-2), so later runs read, lock, and save them. Once `MIND_HOMES` or
  `mind-homes` is set, only that list is allowed and `source-homes` is not
  read, so a source in a recorded home drops out of view (HOME-6) until the
  list names its home; `source-homes` homes never restrict a lobe (LOBE-2).
  A recorded home stays recorded until removed from the config. `config
  show` lists `source-homes` with a note when it is not read, and `--json`
  gives `source_homes` and `homes_restricted`.
- `HOME-8` A source lives in one home. Re-melding it with `--home` naming a
  different home moves it there, outranking its own `[source] mind-home`:
  its clone moves (a rename, else a copy keeping symlinks; a linked local
  source's working tree stays put), its registry entry is written to the new
  home's `sources.json` and dropped from the old one's, and `moved <source> to
  mind home <dir>` is printed. Every source it curates that follows it (in
  its home, with no `mind-home` from its own `mind.toml` or the curator's
  entry, HOME-5, HOME-10) moves with it. Then each installed item of those
  sources whose store copy is in another home is reinstalled in the new home
  through the upgrade pass, without fetching: it is listed with a `home old
  -> new` line (under `N items to move:` when nothing else changed),
  confirmed as an upgrade is, its links replaced, and its old copy removed.
  An `upgrade` reinstalls such an item the same way, e.g. one kept by
  `unmeld --keep-items` and melded again elsewhere. Re-melding with the home
  the source is already in moves nothing, so the command is idempotent. A
  move into a home whose `sources.json` already lists a shadowed copy of the
  source (HOME-3) is refused (`source-in-other-home`, exit 1). A lobe symlink
  into any allowed home's store is mind's own (LIFE-41). A move that fails
  partway puts back the clones it moved, and the registry with them.
- `HOME-15` An `upgrade` pass that is not limited to items (unscoped,
  `--source`, `sync --upgrade`, `curate`, `auto-update run`) moves a source
  whose own `[source] mind-home`, at the commit its clone is now on, names an
  allowed home it is not in. It moves as HOME-8 describes, with the curated
  sources that follow it, and the same pass reinstalls its installed items
  there, linked into the lobe now in effect (LOBE-5). A source whose home the
  consumer chose (`meld --home`, `learn <url> --home`, or a curator entry's
  `mind-home`, recorded as `chosen_home` on its registry entry) is not moved.
  A declared home that is not allowed, or a move that fails (e.g. a home that
  cannot be written), leaves the source where it is, with a warning on
  stderr. `upgrade <item>` moves no source.
- `HOME-16` A move reinstalls each item from the source's clone, so an item
  with a build hook (HOOK-70) is built again in the new home. When that hook
  cannot run (no terminal to ask, and `--accept-hooks` does not name
  `build`, HOOK-72), the move does not happen rather than leave the item
  unbuilt: `meld --home` and `learn <url> --home` are refused
  (`move-needs-build`, exit 1) before anything moves, an `upgrade` (HOME-15)
  leaves the source where it is with a warning on stderr, and `auto-update
  run` holds the release that would move it (AUP-17).

- `HOME-10` A curator's `[[discover.sources]]` entry may name a `mind-home`
  for that source. It is checked like `meld --home` (an allowed home,
  HOME-17, before the clone) and outranks the source's own `[source] mind-home`. A nested
  source refused for its home, by either key, is skipped with a warning like
  an unreachable one (DSC-79), with the skip reason `home_not_allowed`. `dump` writes
  it for every source in an extra home, with a home-directory prefix written
  as `~`, so melding the dump puts each source back in its home; a source in
  the default home gets none.
- `HOME-12` `learn <url> --home <dir>` registers an item link in `<dir>` as
  `meld --home` does, and moves an instance already in another home there
  (HOME-8). For a plain item ref, `--home` is ignored with a note.

## Showing the home

- `HOME-11` `recall --sources` lists `home:<dir>` in a source's bracket when
  it is in an extra home, and `--json` gives every source's `mind_home` as an
  absolute path.
- `HOME-13` The `probe` TUI's meld input takes `--home` as `mind meld` does
  (TUI-77), and a source's details dialog shows `home:` for a source in an
  extra home.

## Locking

- `HOME-9` Every command locks each allowed home's `.lock`, the default home
  first and then the extra homes in order, so two commands cannot deadlock. A
  lock file that cannot be opened for writing is locked through a read-only
  handle. A home, the default one included, whose lock file does not exist and
  cannot be created because writes are denied there is skipped; an extra home
  whose lock fails to open for any other reason is skipped too.
