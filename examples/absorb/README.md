# Absorb example

`absorb` claims an unmanaged lobe item into a managed, version-controlled source
(the inverse of `forget --unmanaged`).

This example is a walkthrough only: it ships no meldable source of its own (no
`mind.toml` or `items` here), because `absorb` operates on an item you already
have in a lobe plus a target git repo you create below.

## What it shows

- An unmanaged lobe item (a `SKILL.md` placed directly in `~/.claude` that `mind`
  did not install) is surfaced by `recall`.
- `absorb skill:notes --to <repo>` moves it into the target source at the
  convention path (`skills/notes/`), commits it, melds the source if needed, and
  installs it via `learn`.
- After absorb the item is an ordinary managed item: `recall` no longer lists it
  as unmanaged, the lobe path is a managed link, and the file lives under version
  control in the target repo.

## Try it

```
# 1. Starting point: an unmanaged skill, placed in the lobe by hand (not by mind).
mkdir -p ~/.claude/skills/notes
printf -- '---\ndescription: my personal notes skill\n---\n# notes\n' \
  > ~/.claude/skills/notes/SKILL.md

# recall (before): the skill shows up under the unmanaged group.
mind recall
#   * unmanaged: not installed by mind
#     !  skill:notes  ~/.claude/skills/notes

# 2. A target source the user owns: any git repo. Create a throwaway one.
mkdir -p /tmp/absorb-target
cd /tmp/absorb-target && git init -q && git commit -q --allow-empty -m init
cd -

# 3. Absorb: move the item into the target, commit, meld, and learn it.
mind absorb skill:notes --to /tmp/absorb-target --yes
#   learned skill:notes from local/tmp/absorb-target (<hash>)
#   + absorbed skill:notes -> managed as skill:notes

# recall (after): skill:notes is now a normal managed item, no longer unmanaged.
mind recall
#   * local/tmp/absorb-target  [<hash>]
#     +  skill:notes  installed @ <hash>

# The file now lives in the target repo, committed under version control.
ls /tmp/absorb-target/skills/notes/SKILL.md
git -C /tmp/absorb-target log --oneline    # an "absorb skill:notes" commit

# And the lobe path is a managed link, not the user's own file.
ls -l ~/.claude/skills/notes               # -> ~/.mind/store/skill/notes
```

## Destination

`absorb` resolves the destination source in precedence order (ABS-2):

1. `--to <path>` (the flag used above).
2. the `MIND_ABSORB_TO` environment variable.
3. the `absorb_to` key in `~/.mind/config.toml`.

The first one set wins. With none set, an interactive run prompts and offers the
built-in `~/.mind/personal` (git-init'd on demand) and can save your choice as
`absorb_to` (ABS-3/4); a global `--yes` with none configured uses and persists
`~/.mind/personal`; a non-TTY run with none configured errors and changes
nothing. The destination must be a git repository (ABS-5). A `kind:name`
collision at the destination errors unless you pass `--force` (`-f`) to overwrite
the destination path (ABS-6). Glob refs are rejected: `absorb` claims exactly one
item (ABS-1).

## Teardown

```
mind forget skill:notes --yes
mind unmeld local/tmp/absorb-target
rm -rf /tmp/absorb-target
```

Note that the absorbed file remains committed in the target repo's git history,
so even after teardown removing `/tmp/absorb-target` is what discards it; `forget`
and `unmeld` only undo the install and the meld.

## See also

- `../../spec/absorb.md` - normative spec for `absorb` (ABS-1 resolve-and-move,
  ABS-2/3/4/9 destination precedence and prompts, ABS-5 git-repo and commit,
  ABS-6 collision and `--force`, ABS-7 stray-copy removal, ABS-8 managed
  afterward, ABS-10 transactional).
- `../../spec/unmanaged.md` - unmanaged-item detection and `forget --unmanaged`,
  whose inverse `absorb` is (UNM-1 detection, UNM-7/8 `forget --unmanaged`).

## Verified

`tests/cli.rs::example_absorb_claims_unmanaged_item` seeds an unmanaged lobe item
and absorbs it into a temp git repo, asserting it becomes managed, so the example
stays correct as the code changes.
