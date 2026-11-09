# Mind homes

Status: done. A source can be melded into a mind home other than `~/.mind`,
so a source can live under a directory a sandbox allows writes to while the
rest stay in `~/.mind`. Every command sees the sources and items of all homes
as one set.

## Allowed homes

- `HOME-1` The default home is `$MIND_HOME` if set, else `~/.mind` (STO-1). It
  is always allowed, and it alone holds `config.toml`, the hook history and
  logs, the auto-update state, and the `absorb` personal source.
- `HOME-2` The extra homes are the entries of `$MIND_HOMES` (`:`-separated),
  then the `mind-homes` config key (a list of paths), in that order. A leading
  `~` is expanded; a relative `$MIND_HOMES` entry is resolved against the
  working directory and a relative `mind-homes` entry against the default
  home; duplicates and the default home are dropped. An empty entry is ignored.
  `config show` lists the resolved extra homes (`mind_homes` under `--json`).

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
- `HOME-7` A manifest entry's `store` is relative to the default home for an
  item there (unchanged), and the absolute store path for an item in an extra
  home, which is what routes the entry back to its home on save. An absolute
  entry under no allowed home is treated as the default home's: it is saved to
  the default manifest, and `forget` refuses to remove a store path outside the
  default store root (LIFE-44).

## Choosing a home

- `HOME-4` `meld --home <dir>` melds the named source, and every source it
  curates, into `<dir>`. `<dir>` must be the default home or an extra home
  (compared after `~` expansion and making it absolute); any other value is
  refused before anything is cloned (`home-not-allowed`, exit 1), naming the
  allowed homes.
- `HOME-5` Without `--home`, a source's `[source] mind-home = "<dir>"` in its
  `mind.toml` chooses its home. It must be an allowed home; otherwise the meld
  is refused (`home-not-allowed`) and the clone removed. `--home` outranks it.
  The first clone, made before the `mind.toml` can be read, goes into the home
  the meld started in (the default home, or the curator's), and moves once the
  declared home is known.
  A curated source with no `mind-home` of its own goes into its curator's
  home, whether the curator's first meld, `sync`'s re-walk (DSC-57), or
  `curate` registers it.
- `HOME-8` A source lives in one home. Re-melding it with `--home` naming a
  different home is refused (`source-in-other-home`, exit 1); moving it means
  `unmeld` and `meld` again. An item kept by `unmeld --keep-items` stays in
  the old home's store until an upgrade reinstalls it in the source's new
  home, which removes the old copy. A lobe symlink into any allowed home's
  store is mind's own (LIFE-41).

- `HOME-10` A curator's `[[discover.sources]]` entry may name a `mind-home`
  for that source. It is checked like `meld --home` (an allowed home, before
  the clone) and outranks the source's own `[source] mind-home`. A nested
  source refused for its home, by either key, is skipped with a warning like
  an unreachable one (DSC-79), with the skip reason `home_not_allowed`. `dump` writes
  it for every source in an extra home, with a home-directory prefix written
  as `~`, so melding the dump puts each source back in its home; a source in
  the default home gets none.
- `HOME-12` `learn <url> --home <dir>` registers an item link in `<dir>` as
  `meld --home` does, including the HOME-8 refusal for an instance already in
  another home. For a plain item ref, `--home` is ignored with a note.

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
