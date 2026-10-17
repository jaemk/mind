# Install profiles

An install profile is a named set of a source's items, declared by the source
maintainer in `mind.toml` and selected by the consumer at `meld` or `learn`. The
model is cargo's feature flags: profiles compose by including one another, a
`default` profile applies unless the consumer opts out, and naming a profile adds
to `default` rather than replacing it.

```toml
[profiles]
default = ["basic"]
basic   = ["skill:review", "skill:test", "rule:style"]
full    = ["basic", "agent:*", "skill:deploy"]
all     = ["*"]
```

```sh
mind meld owner/repo                        # installs basic (via default)
mind meld owner/repo --profile full         # default + full
mind meld owner/repo --no-default-profile --profile full
mind learn owner/repo --profile full        # enable full on an already-melded source
mind forget owner/repo --profile full       # disable it again
```

## Declaration

- `PROF-1` A source declares profiles in a `[profiles]` table in its root
  `mind.toml`. Each key is a profile name; each value is an array of members.
  `[profiles]` is metadata, like `[source]`: it does not make the `mind.toml`
  authoritative, and it composes with whatever discovery is in effect
  (convention, `[[items]]`, `[discover]`, `--add-root`, a plugin manifest's own
  items, MKT-15). A member can only reach an item the source offers, so a
  profile never widens the source's export control.
- `PROF-2` A profile name matches `[a-z0-9][a-z0-9_-]*` and is at most 64 bytes.
  Any other name, including one carrying `:`, is a `MindToml` error. The syntax
  rule is what makes a profile name safe to print unsanitized and lets PROF-3
  tell a profile member from an item member.
- `PROF-3` A member is one of:
  - an item pattern `kind:name`, where `kind` is an item kind word and `name` is
    a bare name or a glob over bare names (`agent:*`, `skill:pdf-*`),
  - `"*"`, every item the source offers,
  - a profile name (no `:`), which includes that profile's members.

  An item pattern splits at its first `:`, so `command:frontend:build` is the
  `command` named `frontend:build`. Item patterns are in source truth: a namespace in effect for the source is
  applied at install (as DSC-63 does for `install-items`), never written in the
  profile. A member with an unknown kind word, or a profile-name member that is
  not a key of the table, is a `MindToml` error.
- `PROF-4` Profile inclusion is acyclic. A cycle, including a profile that lists
  itself, is a `MindToml` error whose message names the cycle (`full -> basic ->
  full`).
- `PROF-5` A profile's expansion is the union of its item patterns' matches and
  the expansions of the profiles it includes. An exact item pattern (no glob
  metacharacters) naming an item the source does not offer is `BadReference`
  when the profile is expanded, at `meld`, `learn`, `upgrade`, or `curate`; it
  cannot be caught at parse time, since it needs the catalog. A glob pattern
  that matches nothing is not an error, so a source can keep `agent:*` before it
  ships an agent; `review` reports it (PROF-30).
- `PROF-6` Installing a profile installs each item of its expansion through the
  ordinary `learn` path, so the dependency closure (DEP-30/31) and the collision
  checks (CLI-33) apply unchanged. A closure item is not a profile member; it is
  installed on behalf of the member that pulled it in and carries that member's
  install reason (PROF-20).
- `PROF-7` The `default` key names the profile that is enabled when the consumer
  names none and does not opt out (PROF-10). Its value follows PROF-3. A source
  whose `[profiles]` has no `default` keeps the CLI-23 install-all offer, and its
  profiles are opt-in only.
- `PROF-8` Profiles are read from the root `mind.toml` only. A scoped item
  `mind.toml` (HOOK-131) declaring `[profiles]` is a `MindToml` error, and a
  plugin or marketplace manifest declares none. An item-link instance
  (spec/item-link.md) is a single item, so `--profile`, `--no-default-profile`,
  and `--all-profiles` on an item link are a usage error.

## Selection at meld and learn

- `PROF-9` `meld <repo> --profile <NAME>` enables the named profile in addition
  to `default`. The flag is repeatable and accepts a comma-separated list. The
  CLI-23 install offer then covers the expansion of the enabled set instead of
  the whole source, with the CLI-23 gate unchanged: one preview and one prompt on
  a TTY, `--yes` installs without prompting, a non-TTY run without `--yes`
  installs nothing and prints how to install later.
- `PROF-10` `--no-default-profile` removes `default` from the enabled set. With
  no `--profile`, nothing is offered for install: the source is registered with
  no profile enabled, and `meld` names `mind learn <source> --profile <name>` as
  the way to install later.
- `PROF-11` `--all-profiles` enables every declared profile. An item in no
  profile is still not installed by it; `learn --all` (CLI-36) remains the
  whole-source form. `--profile` alongside `--all-profiles` is accepted and
  redundant, and `--all-profiles` enables `default` even alongside
  `--no-default-profile`.
- `PROF-12` When the source declares `default`, a plain `meld` (no profile flag)
  offers the `default` expansion instead of every item. The preview names the
  enabled profiles, lists the other declared profiles, and names `--profile` /
  `--all-profiles`, so the consumer sees what is not being installed.
- `PROF-13` An interactive first `meld` with no profile flag and no `--yes`, of
  a source that declares `default` and at least one other profile, asks which
  profiles to enable before the install preview, with `default` preselected.
  The answer is the complete list to enable: an empty answer keeps `default`,
  and typing names replaces the preselection (`full` alone enables only `full`,
  the same as `--no-default-profile --profile full`). An undeclared name is
  `ProfileNotFound`, not a re-prompt. A non-interactive `meld` takes `default`
  as-is (PROF-12).
- `PROF-14` `--profile` values are checked against PROF-2's syntax before the
  clone, so a malformed name (`InvalidProfileName`) does not leave a registered
  source behind. A well-formed name the source does not declare is
  `ProfileNotFound`, raised after the clone, whose message lists the declared
  profiles or says the source declares none. As with an unmatched `--learn`
  pattern (CLI-236), the source stays melded and nothing from it is installed.
- `PROF-15` `--profile` and `--all-profiles` compose with `meld --learn`
  (CLI-236): the install batch is the union of the profile expansion and the
  `--learn` matches, under one preview and one prompt. Both conflict with
  `--register-only`.
- `PROF-16` A re-meld (CLI-12) honors the profile flags, adding to the recorded
  set (PROF-19). On a profile-tracked source the selection is the recorded set
  (names still declared) plus the flags; `default` is not re-added unless it is
  recorded or named, so an earlier `--no-default-profile` holds. A re-meld never
  disables a profile; that is PROF-25.
- `PROF-17` `learn <source> --profile <NAME>` installs the named profile's
  expansion for an already-melded source and adds it to the recorded set.
  `<source>` is a source selector as in CLI-36. `learn <source> --all-profiles`
  does the same for every declared profile. `learn` enables only what it names:
  it never adds `default`, and `--no-default-profile` on `learn` changes nothing
  (it prints a note). `--all` conflicts with `--profile` and `--all-profiles`.
  The gate is `learn`'s, including the dependency-closure confirmation and
  `--dry-run`.
- `PROF-18` Under `--json`, the `meld` and `learn` result objects carry a
  `profiles` array of the enabled profile names. `pending_items` (CLI-156)
  counts the union of the expansion and its closures, each item once.

## Recording

- `PROF-19` A source installed through a profile is *profile-tracked*: the
  registry entry records the enabled profile names (`default` by name, so a
  later change to its members is tracked like any other) and a snapshot of the
  bare `kind:name` keys the enabled set expanded to when last applied. A source
  with neither field, including every source registered before this feature and
  one installed by the CLI-23 install-all offer, is not profile-tracked and none
  of PROF-21..24 applies to it. Both fields are optional additions to
  `sources.json` and need no schema bump (STO-50).

  The enabled set is recorded only when the batch is applied (`--yes`, a
  confirmed prompt, or nothing left to install, in which case it is recorded
  without a prompt). A declined or non-TTY `meld` records nothing and leaves the
  source untracked. Only keys that actually installed enter the snapshot, so a
  member that failed, or that managed policy skipped, is offered again later.
- `PROF-20` A manifest entry installed by a profile (as a member or as a closure
  item, PROF-6) records the profile names it was installed for. A `learn` of the
  item by an exact (non-glob, non-`--all`) ref marks it explicit, keeping the
  profile names, and so does enabling a profile over an item that was already
  installed by hand. The reasons survive a content upgrade and a rename. PROF-25
  reads these reasons to decide what disabling a profile removes.

## Tracking

PROF-21..24 run in a source-scoped upgrade pass: plain `upgrade`, `sync
[source] --upgrade`, and `curate`'s upgrade change (CUR-6). They do not run for
`upgrade <item>` or the TUI's confirmed-key apply, since an addition is never an
item the user named.

- `PROF-21` `upgrade` re-expands each profile-tracked source's enabled set
  against the current catalog. An item in the new expansion and not in the
  snapshot is pending: the upgrade report lists it as an addition naming its
  profile, and applying the upgrade installs it under `upgrade`'s gate. An
  addition the consumer declines stays out of the snapshot and is offered again
  next time.
- `PROF-22` An item in the snapshot that is no longer in the expansion is
  reported once as having left its profile and removed from the snapshot. It is
  never uninstalled by `upgrade`; the report names `mind forget <key>`
  (shell-quoted, CLI-225) when the item was installed only for profiles. The
  departure is persisted even when the upgrade itself is declined.
- `PROF-23` An item the consumer `forget`s stays in the snapshot, so `upgrade`
  does not offer it again while it remains a member. Only a member the
  maintainer adds after the snapshot was taken is offered. This differs on
  purpose from a curator's `install` (CUR-4), which re-proposes any declared
  item that is not installed.
- `PROF-24` An enabled profile the source no longer declares is removed from the
  recorded set with a warning naming it, and its former members are reported
  per PROF-22.

## Disabling

- `PROF-25` `forget <source> --profile <NAME>` removes NAME from the recorded set
  and forgets each installed item whose recorded reasons (PROF-20) are only
  profiles that are no longer enabled and that is not marked explicit. An item
  another installed item depends on is kept and named (DEP-60), and keeps its
  disabled reason so it is not later mistaken for a hand-installed item. An
  explicit item that loses all its profile reasons stays installed as explicit.
  The preview and confirmation are `forget`'s glob gate (CLI-42), which prompts
  only when more than one item would be removed. The snapshot is recomputed from
  the remaining enabled set; if that expansion fails (a maintainer's broken
  table), `forget` warns, keeps the old snapshot, and still disables the
  profile. Disabling the last enabled profile clears both recorded fields.
  Disabling a profile that is not enabled changes nothing. Disabling `default`
  is `--profile default`.

## Curators and dump

- `PROF-26` A `[discover].sources` entry may set `profiles = ["<name>", ...]` and
  `default-profile = false` as the install directive for the nested source,
  applied by the install flow exactly as PROF-9 and PROF-10 apply the flags.
  `profiles` with `install = true` is a `MindToml` error (as DSC-64 is for
  `install-items`); `profiles` with `install-items` installs the union. An
  `install = true` entry whose nested source declares `default` offers the
  `default` expansion, the same as a plain `meld` of it (PROF-12). An entry with
  `install-items` and no `profiles` keeps the DSC-62 path even when the nested
  source declares `default`. A profile name the nested source does not declare
  is `BadReference` at meld.
- `PROF-27` `curate`'s `install` change (CUR-4) covers an entry's profiles: it
  proposes the members missing from the source's snapshot (PROF-21, PROF-23),
  together with any of the entry's `install-items` that are not installed, as
  one change, and applying it records the entry's profiles on the source. An
  entry whose members are all installed but whose profiles are not all recorded
  proposes a change that only records them. A newly listed entry routed to
  profiles installs through them at registration (CUR-3). An entry whose
  profile selection is invalid (an undeclared name, or an exact member naming
  no item) is skipped with reason `profile_selection_invalid` (CUR-15) rather
  than failing the run.
- `PROF-28` `dump` emits each profile-tracked source's enabled profiles as
  `profiles = [...]`, with `default-profile = false` when `default` is not
  enabled, plus `install-items` (DSC-62) for the source's installed items that
  are marked explicit and outside the recorded snapshot. A source with only
  `default` enabled emits `profiles = []`. `dump` reads the recorded state only
  and never re-expands against the catalog. `--whole-sources` (DUMP-3) is
  unchanged and emits no profile keys.

## Display and review

- `PROF-29` `probe --no-tui` and `recall --sources` list each source's declared
  profiles and mark the enabled ones; a source with no profiles adds nothing.
  `--json` carries `profiles` as an object of profile name to its declared
  members, plus an `enabled_profiles` array: on each source object for `recall
  --sources` (where `profiles` therefore means the declared table, not the
  recorded enabled list), and on each item row for `probe`, whose rows of a
  source without profiles carry neither key. `recall <item>` names the profiles
  an installed item was installed for. The `probe` TUI shows the same and
  toggles profiles from the source details dialog (TUI-78).
- `PROF-30` `review` reports PROF-2..4 violations (`invalid-profile`) and an
  exact member naming an item the source does not offer (`bad-profile-member`,
  PROF-5) as hard findings, and a glob or `*` member matching nothing
  (`empty-profile-member`) or a profile whose expansion is empty
  (`empty-profile`) as advisories.

## Out of scope

- Per-profile hooks or profile-conditional item content. A profile selects
  items; it does not change them.
