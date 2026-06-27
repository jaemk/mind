# Drift example

Drift is when a source's content moves on after you installed an item, so your
local copy is out of date; mind surfaces it in `recall` and `introspect` and
resolves it with `mind upgrade`.

## Layout

```
skills/audit/SKILL.md    skill, description in frontmatter, body line edited to create drift
```

One item is enough to show the cycle: install it, change it upstream, then
detect and upgrade. `mind upgrade` is the item-upgrade verb (it reports pending
hash/commit deltas and prompts before changing anything). Do not confuse it with
`mind evolve`, which is the binary self-update verb.

## Try it

This directory is part of the `mind` repo, not its own git repo, so copy it out
and init a repo before melding:

```
cp -r examples/drift /tmp/drift
cd /tmp/drift && git init -q && git add -A && git commit -qm init

mind meld /tmp/drift
mind learn audit             # links skills/audit into each agent home
mind recall                  # shows audit as installed and current
```

Now simulate an upstream change. Edit the source's skill body and commit, then
`mind sync` to advance the recorded commit so the installed copy is out of date:

```
# edit /tmp/drift/skills/audit/SKILL.md (change the body), then:
cd /tmp/drift && git commit -aqm "edit audit"

mind sync                    # refreshes the recorded commit for the source
```

`recall` now marks the item out of date. The left-edge marker changes from the
installed glyph (`+` in ASCII) to the stale glyph (`^` in ASCII), and the line
gains a trailing `(outdated; run mind upgrade)`:

```
mind recall
# * local/tmp/drift  [00cc9eff]
#   ^  skill:audit  installed @ 7f8e50eb  (outdated; run mind upgrade)
```

`introspect` reports the same drift:

```
mind introspect
# ! skill:audit: upstream changed; run `mind upgrade`
#
# x 1 issue(s) found
```

`mind upgrade audit` reports the hash and commit deltas, then re-links the item
to the new version (bare `mind upgrade` covers every pending item). It prompts
before changing anything; pass `--yes` to skip the prompt:

```
mind upgrade audit
# 1 item(s) have upstream changes:
#
#   ! skill:audit [local/tmp/drift]
#     hash    54b6fc6e -> 06f63331
#     commit  7f8e50eb -> 00cc9eff
#
# apply these upgrades? [Y/n]
# + upgraded skill:audit
```

Afterward `recall` shows the marker gone (back to `+`, installed and current)
and `introspect` is clean:

```
mind recall
# * local/tmp/drift  [00cc9eff]
#   +  skill:audit  installed @ 00cc9eff
```

### Teardown

Undo the demo in reverse order:

```
mind forget audit
mind unmeld local/tmp/drift
rm -rf /tmp/drift
```

## See also

`../../spec/lifecycle.md` - the lifecycle IDs demonstrated here: LIFE-11 (an
item is pending when its source-content hash changed), LIFE-13 (applying a
content-only upgrade reinstalls under the same effective name), LIFE-15 (the hash
compared is of the source content, so detection compares source with source),
and LIFE-33 (introspect reports an item whose source-content hash drifted).
`../../spec/cli.md` - CLI-75 and CLI-155 (the `recall` stale marker: the `^`
glyph and `(outdated)` text for an out-of-date item) and CLI-90 (introspect's
`drifted` finding).

## Verified

`tests/cli.rs::example_drift_upgrade` melds this directory, edits and syncs the
source, and asserts the stale marker on `recall`, the `introspect` drift report,
and that `mind upgrade` clears both, so the example stays correct as the code
changes.
