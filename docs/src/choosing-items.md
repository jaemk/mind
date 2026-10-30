# Choose what a source installs

By default `meld` offers every item a source ships. For a large repo you
usually want less. There are three ways to choose:

| you want | use |
|----------|-----|
| to decide later, item by item | `meld --register-only`, then `learn` |
| specific items by name or pattern | `meld --learn <pattern>` |
| a set the source's author defined | `meld --profile <name>` |

## Register now, install later

```
mind meld owner/repo --register-only
mind probe --source owner/repo        # browse what it offers
mind learn owner/repo#review
```

## Install items by name or pattern

`--learn` narrows the meld to the items you name, in one command:

```
mind meld owner/repo --learn review --yes
mind meld owner/repo --learn 'skill:*' --learn agent:dev --yes
mind meld owner/repo --learn 'pdf-*' --yes
```

- The pattern selects within the source being melded: an item name or glob,
  never a source-qualified ref. Quote globs.
- A name matches whether or not the source namespaces its items, so
  `--learn review` finds an item that installs as `team:review`.
- Each match brings the siblings it depends on (see
  [Find and install items](installing.md#items-that-depend-on-other-items)).
  The rest of the source stays registered for `learn` later.
- A pattern matching nothing is an error naming the source; the source stays
  melded.
- `*`, `?`, and `[` are always glob syntax here, so an item whose name contains
  one cannot be selected literally.
- `--learn` conflicts with `--register-only`.

Pass `--yes` in scripts. Like the install-all offer it replaces, `--learn`
prompts on a terminal and installs nothing without one, so a script without
`--yes` registers the source and exits 0 having installed nothing.

An item the source's inventory does not declare is not reachable this way. Add
`--add-root <dir>` at the meld that first registers the source to scan more of
the repo; an already-melded source must be unmelded first.

For a super-source, `--learn` applies to its own items. The sources it curates
are still registered, but nothing from them is installed.

## Install profiles

A source can define named sets of its items, called profiles, in its
`mind.toml` (for example `default`, `full`, `minimal`). If it defines a
`default` profile, a plain meld installs that set instead of everything.

```
mind meld owner/repo                         # the default profile
mind meld owner/repo --profile full          # default plus full
mind meld owner/repo --no-default-profile --profile full   # only full
mind meld owner/repo --all-profiles          # every profile
```

`--profile` takes a comma-separated list or repeats. An unknown name is an
error that lists the profiles the source declares, and leaves the source melded
with nothing installed.

A source that declares `default` goes straight to the offer for it, without
asking which profiles to enable. On a terminal, the first meld of a source that
declares profiles but no `default` asks: type the names you want, or press Enter
to be offered every item. `mind recall --sources` shows which profiles are
enabled for each source; the [terminal UI](tui.md#details-dialog) shows them in
the source's details dialog, with Enable and Disable actions.

### Change profiles later

```
mind learn owner/repo --profile          # list the profiles you can enable
mind recall --profiles owner/repo        # the same, with what each installs
mind learn owner/repo --profile full     # enable full and install its items
mind forget owner/repo --profile full    # disable full
```

`mind recall --profiles` with no source lists every melded source that declares
profiles.

Disabling a profile forgets the items that were installed only for it. It keeps
an item you installed by name, an item another enabled profile includes, and an
item another installed item depends on.

When the author adds an item to a profile you have enabled, `mind upgrade`
offers to install it. When an item leaves a profile, `upgrade` reports it and
leaves it installed; `forget` it if you no longer want it. An item you
`forget` yourself is not offered again while it stays in the profile.

### What profiles do not cover

- A source with no `default` profile keeps the install-everything offer on a
  plain `meld` unless you pick profiles at the prompt; its profiles are opt-in.
- `--all-profiles` enables every profile but does not install items that are
  in no profile. `learn owner/repo --all` installs the whole source.
- `--learn` and `--profile` compose: the meld offers the union.

To define profiles for your own source, see
[Offer install profiles](authoring-profiles.md).
