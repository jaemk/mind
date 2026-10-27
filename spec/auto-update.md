# Auto-update

Auto-update runs `mind`'s release resolution on its own, so a version-pinned
source picks up new releases without the consumer running `sync` and `upgrade`.
The maintainer declares what they would like their source to take
automatically; the consumer decides per source, and their choice wins; a
managed policy can cap it. Compatible releases are the ordinary case. A
maintainer may also ask for breaking releases to be taken automatically, which
a consumer must grant explicitly.

```toml
[source]
release-tags = "v{version}"
auto-update = "breaking"     # what to ask for: true / "compatible" | "breaking"
pin-version = "^2"           # the newest major a `breaking` grant may move to
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

- `AUP-5` `[source].auto-update` declares the level the maintainer asks for:
  `"compatible"` or `"breaking"`. `true` means `"compatible"`. Any other
  value is a `MindToml` error. It is read from the default-branch tip, like
  `release-tags` (VER-3), so a change applies without a new release.
- `AUP-6` The declaration is a request, not a limit. It sets what a bare
  `--auto-update` grants (AUP-8) and what a first meld asks for (AUP-9). A
  consumer may grant any level whatever the source declares, `breaking`
  included; a breaking move still waits for the maintainer's recommended range
  (AUP-12).

## Consent

- `AUP-8` The consumer's grant is recorded per source. `meld --auto-update`
  grants the declared level (or `compatible` when nothing is declared), and
  `--auto-update=<level>` grants an explicit one. On a melded source the flag
  replaces the grant, and `--auto-update=off` revokes it. A value that is not a
  level is an error before anything is cloned.
- `AUP-9` When the meld that registers a source with a range version pin runs
  on a TTY, without `--yes`, finds a declared level, and the consumer passed no
  `--auto-update`, it asks once:
  `<source> asks to update automatically (compatible releases) [Y/n]`, or for
  `breaking`, `<source> asks to update automatically, including breaking
  releases [y/N]`. Declining `breaking` offers `compatible` next. Without a
  TTY, with `--yes`, or under `--json`, nothing is granted. A re-meld never
  asks; the flag (AUP-8) is the only way to change a grant afterwards.
- `AUP-10` The effective level is the consumer's grant (`off` when there is
  none), capped by the policy (AUP-30). A change to the declaration never
  changes it. When the declaration is above a grant that is not `off`, `recall`
  notes
  `<source> asks to update automatically, including breaking releases; allow
  it with mind meld <source> --auto-update=breaking`.
- `AUP-11` `recall` and `recall --sources` show the effective level in the
  source header (` auto-update compatible`), and nothing when it is `off`.
  `recall --sources --json` adds `auto_update: {declared, granted, effective}`
  to each source: `declared` is `null` when the source declares nothing, and
  `granted` is `off` when nothing was granted.

## Breaking moves

- `AUP-12` At `breaking`, a run takes the breaking release only when the
  source's recommended range (VER-140) admits it, and moves the range to that
  recommendation rather than to the caret of the release (VER-62). With no
  recommendation, nothing breaking is taken. So the maintainer stages each
  major: tag `2.0.0`, then set `pin-version = "^2"` when consumers should move.
- `AUP-13` A breaking move taken by a run is reported like one taken by
  `upgrade --allow-breaking` (VER-91): `run --json` lists it in a `breaking`
  array of `{source, from, to, applied, recommended}`. The document is the
  mutating-verb result (CLI-153) with `action: "auto-update run"`, `target:
  "*"`, an `outcome` of `ok`, `partial` (a source failed), or `skipped` (not
  stale, AUP-19, or the lock was held, AUP-18), and `moved`, `held`, and
  `failed` lists of the lines AUP-21 logs.

## The run

- `AUP-15` `mind auto-update run` syncs and upgrades every source whose
  effective level is not `off`, at that level. It never prompts. Each step
  that would need consent the run cannot give (AUP-17) is skipped and reported.
  Under a locked allowlist, a source the allowlist no longer admits is not
  moved (POL-12). One source failing does not stop the others.
- `AUP-16` A run honors everything `sync` does: `min-mind-version` (VER-41),
  yanked releases (VER-46), and moved tags (VER-44).
- `AUP-17` A run never grants hook consent (VER-70, HOOK-22). A source whose
  move would run a hook is held where it was, clone included, so the source
  and its items do not disagree: a pending source-level hook (HOOK-122), or an
  installed item whose new content changed and declares an install, update,
  or build hook. `mind upgrade` takes the held move with its prompts.
- `AUP-18` A run takes the state lock without waiting. If another `mind` holds
  it, the run exits 0 and does nothing.
- `AUP-19` `--if-stale <duration>` skips the run when the last completed run
  is more recent. A duration is a positive whole number of minutes, hours, or
  days (`30m`, `12h`, `1d`); anything else is an error. The last run's time is
  recorded under `~/.mind`; a run that steps aside (AUP-18) records nothing.

## Trigger

- `AUP-20` `mind config auto-update on [--every <duration>]` installs a
  Claude Code `SessionStart` hook in the `settings.json` of each Claude lobe
  (a lobe with no kinds filter; a harness preset lobe is skill-only). The hook
  runs `mind auto-update run --if-stale <duration> --background` (default
  `1d`). `--background` checks the throttle, starts the run detached with no
  terminal and nothing to print to, and returns, so a session never waits on
  it and the harness adds nothing to the session. Running `on` again replaces
  the trigger; the rest of `settings.json` is kept as written, and a
  symlinked `settings.json` is written through with its mode kept. `off`
  removes exactly the hook `on` added (that command, with any valid duration),
  and any container that leaves empty. A non-Claude lobe gets no trigger;
  `config auto-update on` names it and suggests cron.
- `AUP-21` A run prints each source it moved, held, or failed
  (`auto-update: moved acme/skills 1.4.2 -> 1.5.0`), failures on stderr, or
  that everything is up to date. It also appends those lines to
  `~/.mind/auto-update.log` and keeps the report in `~/.mind/auto-update.json`
  until it is shown. The next `recall` or `upgrade` that is not `--json`
  prints a one-line summary of the reports not yet shown on stderr, such as
  `auto-update: acme/skills 1.4.2 -> 1.5.0; 1 source held for hook consent
  (run mind upgrade)`.

## Policy

- `AUP-30` A managed policy's `[sources].auto-update` caps the effective level
  for every source: `"off"`, `"compatible"`, or `"breaking"`. The cap applies
  to the run and to the level `recall` shows (AUP-11). `"off"` also makes
  `config auto-update on` an error. Any other value is an `InvalidPolicy`
  error naming `[sources].auto-update`.

## Not covered

- Auto-updating branch-following sources (AUP-2).
- Updating the `mind` binary itself, which `evolve` and POL-51 cover.
