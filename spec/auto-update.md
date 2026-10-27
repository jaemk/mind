# Auto-update

Status: planned.

Auto-update runs `mind`'s release resolution on its own, so a version-pinned
source picks up new releases without the consumer running `sync` and `upgrade`.
The maintainer declares what their source may take automatically; the consumer
grants it per source; a managed policy can cap it. Compatible releases are the
ordinary case. A maintainer may also mark the source so that breaking releases
are taken automatically, which a consumer must grant explicitly.

```toml
[source]
release-tags = "v{version}"
auto-update = "breaking"     # "compatible" (default when declared) | "breaking"
pin-version = "^2"           # with "breaking": the newest major auto-update may move to
```

```sh
mind config auto-update on --every 1d   # install the trigger
mind meld owner/repo --auto-update      # grant what the source declares
mind auto-update run                    # what the trigger runs; also usable from cron
```

## Levels

- `AUP-1` A source's auto-update level is one of `off`, `compatible`, or
  `breaking`. `compatible` takes releases inside the source's range, as `sync`
  plus `upgrade --yes` would. `breaking` also takes a breaking release
  (VER-61) as `upgrade --allow-breaking --yes` would, subject to AUP-12.
- `AUP-2` Only a range version pin (VER-20) auto-updates. A source following a
  branch, pinned to a tag or commit, or held at an exact version (`=X.Y.Z`) is
  never auto-updated, whatever is declared or granted.

## Declaration

- `AUP-5` `[source].auto-update` declares the highest level the maintainer
  wants: `"compatible"` or `"breaking"`. `true` means `"compatible"`. Any other
  value is a `MindToml` error. It is read from the default-branch tip, like
  `release-tags` (VER-3), so a change applies without a new release.
- `AUP-6` A source that declares nothing can still be auto-updated at
  `compatible` when the consumer grants it (AUP-8). `breaking` requires the
  declaration: a consumer cannot grant a level the maintainer did not declare.

## Consent

- `AUP-8` The consumer's grant is recorded per source. `meld --auto-update`
  grants the declared level (or `compatible` when nothing is declared), and
  `--auto-update=<level>` grants an explicit one. `meld <source>
  --auto-update=off` on a melded source revokes it.
- `AUP-9` When a meld on a TTY, without `--yes`, finds a declared level and the
  consumer passed no `--auto-update`, it asks once:
  `<source> asks to update automatically (compatible releases) [Y/n]`, or for
  `breaking`, `<source> asks to update automatically, including breaking
  releases [y/N]`. Declining `breaking` offers `compatible` next. Without a
  TTY, with `--yes`, or under `--json`, nothing is granted.
- `AUP-10` The effective level is the lowest of the declaration (AUP-5, or
  `compatible` under AUP-6), the grant, and the policy cap (AUP-30). A
  maintainer lowering the declaration lowers the effective level at the next
  run. A maintainer raising it does not raise the grant: `recall` notes
  `<source> asks to update automatically, including breaking releases; allow
  it with mind meld <source> --auto-update=breaking`.
- `AUP-11` `recall` and `recall --sources` show the effective level in the
  source header, and `--json` adds `auto_update: {declared, granted,
  effective}` to each source.

## Breaking moves

- `AUP-12` At `breaking`, a run takes the breaking release only when the
  source's recommended range (VER-140) admits it, and moves the range to that
  recommendation rather than to the caret of the release (VER-62). With no
  recommendation, nothing breaking is taken. So the maintainer stages each
  major: tag `2.0.0`, then set `pin-version = "^2"` when consumers should move.
- `AUP-13` A breaking move taken by a run is reported like one taken by
  `upgrade --allow-breaking` (VER-91), and the release's tag message (VER-64)
  is kept for the run report (AUP-21).

## The run

- `AUP-15` `mind auto-update run` syncs and upgrades every source whose
  effective level is not `off`, at that level. It never prompts. Each step
  that would need consent the run cannot give (AUP-17) is skipped and reported.
- `AUP-16` A run honors everything `sync` does: `min-mind-version` (VER-41),
  yanked releases (VER-46), and moved tags (VER-44).
- `AUP-17` A run never grants hook consent (VER-70, HOOK-22). An item whose
  install or update hook would need consent is not upgraded; its source stays
  where it was, so the item and its source do not disagree. An item with local
  drift is not overwritten.
- `AUP-18` A run takes the state lock without waiting. If another `mind` holds
  it, the run exits 0 and does nothing.
- `AUP-19` `--if-stale <duration>` (`1h`, `1d`) skips the run when the last
  completed run is more recent. The last run's time is recorded under
  `~/.mind`.

## Trigger

- `AUP-20` `mind config auto-update on [--every <duration>]` installs a
  Claude Code `SessionStart` hook in the `settings.json` of each Claude lobe.
  The hook runs `mind auto-update run --if-stale <duration>` (default `1d`)
  detached, so a session never waits on it, and writes nothing to the hook's
  stdout, which the harness would add to the session. `off` removes exactly
  the hook entry `on` added. A non-Claude lobe gets no trigger; `config
  auto-update on` names it and suggests cron.
- `AUP-21` Each run appends a report under `~/.mind` (each source and item
  moved, each step skipped and why, each failure). The next interactive
  `recall` or `upgrade` prints a one-line summary of reports it has not shown,
  such as `auto-update: acme/skills 1.4.2 -> 1.5.0; 1 item held for hook
  consent (run mind upgrade)`.

## Policy

- `AUP-30` A managed policy's `[sources].auto-update` caps the effective level
  for every source: `"off"`, `"compatible"`, or `"breaking"`. `"off"` also
  makes `config auto-update on` an error.

## Not covered

- Auto-updating branch-following sources (AUP-2).
- Updating the `mind` binary itself, which `evolve` and POL-51 cover.
