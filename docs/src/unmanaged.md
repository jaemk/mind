# Adopt items you placed by hand

Agent homes often hold skills, agents, and rules that `mind` did not install:
files you wrote yourself or another tool put there. `mind` lists these as
*unmanaged* items, can remove them, and can move one into a git repo you own so
it is versioned and managed from then on.

```
mind recall                         # unmanaged items are listed at the end
mind absorb skill:my-skill          # move it into a source you own, then manage it
mind forget skill:old-experiment    # or delete it
```

## What counts as unmanaged

Any entry in a lobe's `skills/`, `agents/`, `rules/`, `commands/`, or
`workflows/` directory that is not one of `mind`'s own links. A path is always
one or the other. An item present in several lobes is listed once with every
path it occupies. Tools and resources live only in the store, so they are never unmanaged.

## List them

- `mind recall` lists them after your sources, under "unmanaged: not installed
  by mind", with each item's `kind:name` and lobe paths. `--kind` filters them;
  `--source` hides them, since they have no source.
- `mind probe` includes them in its listing and search. In `--json` each
  carries `"unmanaged": true` and no `source` or `hash`. `recall --json` covers
  managed items only.
- The terminal UI shows an **Unmanaged** group.

## Move one into a source you own

`mind absorb <ref>` takes one unmanaged item, moves it into a git repo you own,
commits it there, and replaces the lobe entry with a managed link. From then on
it upgrades, syncs, and forgets like anything else.

```
mind absorb skill:my-skill
mind absorb agent:reviewer --to ~/dev/my-agent-library
```

The ref must name exactly one item; globs are refused. Add a kind when a name
exists in more than one.

### Where it goes

The first of these that is set wins:

1. `--to <path>`
2. the `MIND_ABSORB_TO` environment variable
3. `absorb-to` in `~/.mind/config.toml`

With none set, an interactive run asks, offering `~/.mind/personal` (created
and `git init`-ed on demand), and then offers to save your answer as
`absorb-to`. With `--yes` and no destination, it uses and saves
`~/.mind/personal`. A non-interactive run with no destination is an error.

The destination must already be a git repo, except `~/.mind/personal`. The item
lands at its conventional path (`skills/<name>/`, `agents/<name>.md`,
`rules/<name>.md`, `commands/<name>.md`, `workflows/<name>.js`) and is committed
as `absorb <kind>:<name>`. `mind` melds the destination first if it is not
already a source. If the path is taken in the destination, `absorb` stops;
`--force` overwrites it.

### What it asks first

If the item sits in several lobes, `absorb` takes the content from one and
deletes the other copies so the managed link can replace them. It lists what it
will move and delete and asks once, even for a single lobe, since it deletes
files you own. `--yes` skips the question; a non-interactive run without `--yes`
changes nothing.

A failure before the install finishes leaves the original file in place and
nothing recorded. `absorb` never loses the file.

After absorbing, the item's name follows the destination source's namespace, if
it has one.

## Remove unmanaged items

```
mind forget skill:old-experiment          # one item, by exact kind:name
mind forget --unmanaged 'skill:*'         # every unmanaged skill
mind forget --unmanaged                   # every unmanaged item in every lobe
```

`forget` deletes the file or directory itself, not a link. It always asks
first and says that the item is not managed by `mind`; `--yes` skips the
question, and a non-interactive run without `--yes` removes nothing.

An ordinary glob such as `forget '*'` matches managed items only, so it never
deletes your own files. Unmanaged items are reached by exact `kind:name`, or by
`--unmanaged`, which matches unmanaged items only and can never remove a
managed one.
