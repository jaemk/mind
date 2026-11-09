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
  `~` is expanded, a relative entry is resolved against the working directory,
  duplicates and the default home are dropped. An empty entry is ignored.

## Layout

- `HOME-3` Each home is a full mind tree: its own `sources.json`,
  `manifest.json`, `sources/` clones, `store/`, `.tmp/`, and `.lock`. The
  registry and manifest a command works on are the union of every allowed
  home's files, the default home first. Saving writes each source and item back
  to its own home's file, and a home whose file would not change is not
  written, so a command that touches only one home's sources writes nothing in
  another. A source or item name present in two homes is read from the first
  and the later one is ignored with a warning.
- `HOME-6` A source's clone, store copies, staging and backup space, and items
  hook document all live under its home. A home that is no longer allowed is
  not read, so its sources drop out of view until it is listed again.
- `HOME-7` A manifest entry's `store` is relative to the default home for an
  item there (unchanged), and the absolute store path for an item in an extra
  home, which is what routes the entry back to its home on save.

## Choosing a home

- `HOME-4` `meld --home <dir>` melds the named source, and every source it
  curates, into `<dir>`. `<dir>` must be the default home or an extra home
  (compared after `~` expansion and making it absolute); any other value is
  refused before anything is cloned (`home-not-allowed`, exit 1), naming the
  allowed homes.
- `HOME-5` Without `--home`, a source's `[source] mind-home = "<dir>"` in its
  `mind.toml` chooses its home. It must be an allowed home; otherwise the meld
  is refused (`home-not-allowed`) and the clone removed. `--home` outranks it.
  A curated source with no `mind-home` of its own goes into its curator's
  home.
- `HOME-8` A source lives in one home. Re-melding it with `--home` naming a
  different home is refused (`source-in-other-home`, exit 1); moving it means
  `unmeld` and `meld` again.

## Locking

- `HOME-9` Every command locks each allowed home's `.lock`, the default home
  first and then the extra homes in order, so two commands cannot deadlock. A
  lock file that cannot be opened for writing is locked through a read-only
  handle; an extra home with no lock file that cannot be created is skipped.
