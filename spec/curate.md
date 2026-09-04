# Curate

`mind curate` reconciles the consumer's state with what the curators they have
melded now declare, and reports curated sources whose upstream has moved. It is
the one command to run when a curated registry changes: `sync` refreshes clones
and picks up newly listed sources register-only (DSC-57), and `upgrade` acts on
installed items, but neither applies a curator's own decisions after the meld
that first registered them.

A *curator* is a registered source that declares `[discover].sources` in its
`mind.toml` (DSC-38) or ships a `.claude-plugin/marketplace.json` (MKT-7). A
*curated source* is one registered from such a list.

## The plan

- `CUR-1` `mind curate` builds one plan from every registered curator and reports
  it as a list of proposed changes, each naming the curator that declares it, the
  curated source it acts on, and what applying it does. The change kinds are:
  - `register`: an entry the curator lists that is not registered (CUR-3),
  - `install`: items an entry's `install`/`install-items` declares that are not
    installed (CUR-4),
  - `repin`: a curated source whose recorded pin differs from the entry's pin
    directive (CUR-5),
  - `upgrade`: a curated source whose installed items are out of date (CUR-6),
  - `unlist`: a registered source no curator lists any more (CUR-7),
  - `namespace`: an entry whose declared namespace differs from the instance's
    (CUR-8; advisory, never applied),
  - `adopt`: a registered but unowned source a curator claims (CUR-16;
    advisory, applied only by `mind curate --adopt <identity>`).

  With no pending change, `curate` reports that the curated set is up to date.
  A consumer with no curators registered gets the same report, not an error.
- `CUR-2` Unless `--no-sync`, `curate` fetches every curator and every curated
  source before planning, so the lists it reads and the commits it compares are
  both current. A per-source fetch failure is reported and skipped (CLI-54); the
  plan is built from the sources that did refresh. `--no-sync` plans against the
  clones already on disk, which is what makes an offline `curate --check`
  meaningful.

## Change kinds

- `CUR-3` `register` applies exactly as the DSC-57 re-walk does: a register-only
  meld of the entry, cycle-safe by the DSC-38 guards and subject to managed
  policy (POL-11) like any meld. It then installs the entry's declared items
  (CUR-4) in the same run. That second half is the difference from `sync`, which
  registers a newly listed entry and stops, leaving an `install = true` entry
  registered but empty until the consumer notices. A `register` change's
  `detail` names the entry's resolved URL or filesystem path (from parsing its
  spec), not just the curator and the item refs, so a reader of the plan or
  `--json`'s `detail` field sees what is about to be cloned before it happens.
- `CUR-4` `install` covers a registered curated source whose entry declares items
  that are not installed, under the ordinary precedence (DSC-62 `install-items`
  over DSC-58 `install`). An entry that declares neither is register-only by the
  curator's choice, so it is never proposed for install; nor is an entry whose
  declared items are all installed already. A marketplace catalog's entries
  (MKT-7) declare no install directive, so they propose no install of their own:
  what a catalog contributes to the plan is membership (an entry still listed is
  not proposed for unlisting, CUR-7) and, subject to the same CUR-12 ownership
  rule as any other entry, its registered entries' place in the CUR-6 upgrade
  sweep. A catalog's in-repo plugins are items of the curator source itself
  (MKT-14), not separately registered sources, so they install with that source
  like any other item. "Not installed" is checked against the current
  manifest, not against install history: `forget`ting one item of an
  `install = true` (or `install-items`-listed) source makes `install` propose
  it again on the next `curate`, same as it would for any other declared-but-
  missing item. There is no per-item opt-out; the escapes are unmelding the
  curated source or asking the curator to drop the entry.
- `CUR-5` `repin` covers a curated source whose recorded pin (STO-18) differs
  from the pin directive its entry declares (DSC-59, authoritative by DSC-65).
  Applying it re-pins the source exactly as `meld --pin` does on a re-meld
  (CLI-209): resolve the pin, re-check-out the clone when the resolved commit
  differs, record both. An entry with no pin directive proposes nothing: the
  source keeps whatever pin it has, including a consumer's own. A marketplace
  entry carries no pin directive, so it never proposes a repin.
- `CUR-6` `upgrade` covers a curated source with at least one installed item that
  is out of date by the CLI-75 test (source content hash changed, or effective
  name changed). Applying it runs the ordinary upgrade pass (CLI-53) scoped to
  those sources, so `curate` never upgrades an item installed from a
  directly-melded source: that stays `mind upgrade`'s job.
- `CUR-7` `unlist` covers a registered source whose entry recorded the curator
  that registered it (STO-82) and that no registered curator lists any more.
  Applying it uninstalls that source's items and drops the source, so it is never
  applied by `--yes` alone: it needs `--prune`, and without that flag it is
  reported and left in place. This keeps a curator's removal of an entry from
  silently uninstalling a consumer's items in an unattended run, while still
  surfacing it. A source registered without provenance (a direct meld, or a
  curated meld by a binary older than STO-82) is never proposed for unlisting:
  absent provenance reads as "not curated", never as "no longer listed". Two
  consequences of provenance being fixed at registration and never rewritten
  (CUR-12): `unmeld`ing a curator does not clear the `curated_by` its entries
  recorded, so on the next `curate --prune --yes` every source it registered
  is now unlisted (no curator lists it any more) and gets uninstalled in one
  run -- a large effect from unmelding one source, worth knowing before
  running `--prune` unattended. And if a DIFFERENT curator lists an identity
  whose `curated_by` still names the departed one, CUR-7 keeps it listed (any
  curator naming it is enough) while CUR-12 still refuses to let the new
  curator mutate it, since ownership did not transfer: the source is frozen
  (no install, no repin, no upgrade) until `--adopt` re-claims it -- adopt
  requires no owner today, so an orphaned source (a curator gone, no
  `curated_by` match) is not yet a valid `--adopt` target; only a genuinely
  unowned one is.
- `CUR-8` `namespace` covers an entry whose declared namespace (DSC-78) differs
  from the registered instance's identity alias (STO-58). It is reported and
  never applied: the alias is part of the instance's identity, so adopting a new
  one registers a different instance rather than renaming this one. The report
  names the two-step `mind unmeld <identity> && mind meld <url> --namespace <ns>`
  that adopts it, the same shape LNK-18's remedy uses.

## Applying

- `CUR-9` With no flags, `curate` prints the whole plan and asks once (`[Y/n]`,
  EOF counts as No) before applying it, mirroring `upgrade` (CLI-60) rather than
  prompting per change. `--yes` applies without asking. `--check` reports and
  applies nothing, and outranks `--yes` when both are given, so a `--check` run
  is always safe to paste. A non-TTY run without `--yes` reports the plan,
  applies nothing, and says how to apply it. `--check` outranks `--adopt`
  (CUR-16) too: `curate --check --adopt <identity>` runs every validation the
  claim must pass (CUR-20), so a stale, ambiguous, or mismatched identity still
  fails, reports the curator the identity would be adopted for, and writes
  nothing. "Safe to paste" cannot depend on which other flags accompany
  `--check`. The prompt counts the changes THIS
  run would apply, `--prune` included, so it never disagrees with what
  answering `y` does: a plan of nothing but `unlist` changes under `--prune`
  still prompts rather than silently applying nothing, and a plan that mixes
  them with ordinary changes names how many are destructive `unlist`s instead
  of reporting a bare total.
- `CUR-10` Changes apply in a fixed order: `register`, `install`, `repin`,
  `upgrade`, then `unlist`. A newly registered entry's items therefore install in
  the same run, a repin lands before the upgrade pass compares content, and a
  removal never precedes work on the sources that remain. A failure applying one
  change is reported against that change and stops the run; the changes already
  applied stay applied (each is an ordinary meld/learn/upgrade/unmeld, each
  individually transactional).
- `CUR-11` Registering and installing run the ordinary consent-gated lifecycle
  hooks (HOOK-20, HOOK-50); `--dangerously-skip-install-hook-check` and
  `--dangerously-skip-build-hook-check` pass through as they do on
  `sync --upgrade`. `curate` adds no hook path of its own.
- `CUR-12` `curate` proposes only what a curator declares. It never installs an
  item no entry names, never registers a source no list contains, and never
  changes a directly-melded source: the plan is a function of the curators'
  `mind.toml` (and marketplace manifests) plus the registry and manifest. This
  holds even when an entry's identity collides with a source registered some
  other way: `install` (CUR-4), `repin` (CUR-5), and the CUR-6 upgrade sweep
  act on a registered source only when its recorded curator (STO-82) is the
  one whose entry matched it. An entry naming an identity already owned by a
  direct meld or a different curator proposes nothing against it; it still
  counts toward CUR-7's "still listed" check (any curator listing an identity
  protects it from unlisting), just not toward mutating it.

## Resilience, adoption, and self-listing

- `CUR-15` A per-source or per-entry failure while planning is reported and
  skipped, never fatal to the run, mirroring CLI-54's per-source sync
  tolerance:
  - A curator whose clone is missing, or whose `mind.toml` or marketplace
    manifest fails to read, contributes nothing to the plan -- **not** an empty
    list. The distinction matters for CUR-7: an empty list would propose
    `unlist` for every source that curator owns; "could not be read" proposes
    nothing about them at all, applying the same absent-provenance reasoning
    CUR-7 already uses for a source no binary ever stamped.
  - A curated entry whose pin directive is invalid, or whose catalog scan
    fails (a renamed upstream file, an oversized manifest), skips only the
    `install`/`repin`/`upgrade` change that one failure would have produced;
    every other entry and curator in the run is still planned normally.
  - Every such failure appears in the `skipped` array (CUR-13) and, in text
    mode, as a `warning:` line naming the source and the error.
- `CUR-16` `adopt` covers a registered source that is unowned
  (`curated_by` absent, STO-82) and that a curator's entry or marketplace
  membership currently names. It is reported like any other change but is
  **never** applied by `curate` itself, `--yes` included: only
  `mind curate --adopt <identity>` claims it, by stamping `curated_by` on that
  one source and changing nothing else. A later `curate` run then plans
  `install`/`repin`/`upgrade` for it normally, through the ordinary
  confirmation gate. This is the one way a source curated before this
  consumer's binary recorded provenance -- which is to say every source a
  curator's list currently names but does not yet own -- becomes visible to
  `install`/`repin`/`upgrade`; until adopted, it is reported (`adopt`) rather
  than silently treated as up to date. `--adopt` needs no `--yes`: naming the
  identity is the confirmation, and which curator the claim resolves to is
  CUR-20's rule. `--check` still outranks it (CUR-9): under both flags the
  claim is resolved and reported, and nothing is written.
- `CUR-17` An entry (or marketplace plugin) whose resolved identity is the
  curator's OWN registered identity contributes nothing: not a `register`, not
  `install`/`repin`/`upgrade`, and not membership toward CUR-7's "still
  listed" check. Without this, a source could list itself in its own
  `mind.toml` (or marketplace manifest) and stay immune to `unlist` forever,
  even after the curator that actually registered it drops it -- a
  self-shield CUR-7's "any curator listing an identity protects it" would
  otherwise permit.
- `CUR-20` A claim on an identity is a curator's `[discover].sources` entry
  (DSC-38) or its marketplace catalog membership (MKT-7) resolving to that
  identity; both count equally, and every claim on a registered but unowned
  source is reported as an `adopt` line (CUR-16), one line per curator and
  identity, so a curator claiming a source through both mechanisms reports
  once. `--adopt` applies a claim only when:
  - exactly one registered curator claims the identity. Two curators claiming
    it is an ambiguity `--adopt` refuses, naming the claimants, rather than
    handing ownership to whichever the registry happens to list first; and
  - the claim resolves to the same upstream as the source is registered from.
    The comparison is on the identity the URL derives (`host`/`owner`/`repo`,
    plus an item-link `#path`, plus the absolute path for a local source),
    never on the URL text, so the ssh and https forms of one repo, a `.git`
    suffix, and a trailing slash are all the same upstream, and a consumer's
    `ssh = true` rewrite (DSC-66) changes nothing. A claim that resolves
    elsewhere (another repo, another item-link path, another local directory
    that happens to derive the same `local/<parent>/<dir>` identity) is
    refused, naming both, so a curator cannot capture a source by listing
    something that only shares its name.
- `CUR-18` Every field of a `Change` that can carry curator-controlled text --
  `curator` (the curator's identity), `source` (the acted-on identity), and
  `detail` (an `install-items` list, a pin directive's value, a namespace) --
  is sanitized (`strip_ansi`) at construction, before it is reported. The plan
  is the text a consumer reads before answering the single `[Y/n]` prompt
  (CUR-9); a curated repo must not be able to use escape sequences to make one
  line's report read as a different line's, or to spoof the curator/source
  identity shown for a change. The same holds for the `source` of a CUR-13
  `skipped` entry, which names an identity assembled from repo parts that are
  screened for control characters but not for the wider blocked set (a bidi
  override, a zero-width character): it is sanitized where it is captured
  rather than only where text mode prints it.
- `CUR-19` Unless `--no-sync`, the CUR-2 refresh scopes its fetch to curators
  and the sources they own, not the whole registry: a source with no
  `curated_by` and no `mind.toml`/marketplace manifest file in its clone is
  not a curator and is not curated, so `curate` has nothing to compare it
  against and does not re-fetch it. The scope is read from the clones already
  on disk before the fetch runs (file presence, not parsed content, so a
  curator whose `[discover].sources` currently reads empty is still in
  scope). The one gap this leaves: a source that adds its very first
  `mind.toml` (or marketplace manifest) and populates it in the same push is
  invisible to a file-presence check made before the fetch, and surfaces on
  the following `curate` or an explicit `mind sync`.

## Reporting

- `CUR-13` `--json` emits one document:

  ```json
  {
    "schema": 1,
    "action": "curate",
    "outcome": "clean|pending|applied",
    "changes": [{"kind": "<kind>", "curator": "<identity>", "source": "<identity>", "detail": "<text>"}],
    "applied": ["<kind>:<identity>"],
    "skipped": [{"source": "<identity>", "reason": "<slug>"}]
  }
  ```

  `outcome` is `clean` when the plan is empty, `pending` when a plan exists and
  nothing was applied (no `--yes`, a non-TTY run, or `--check`), and `applied`
  when at least one change was applied. `changes` always lists the whole plan,
  including the changes an apply skipped (`unlist` without `--prune`, `namespace`
  always), so a caller sees what was proposed as well as what ran. `changes`,
  `applied`, and `skipped` are always present, each as `[]` when there is
  nothing to report, never omitted: a caller can always index all three keys
  without first checking they exist.
- `CUR-14` `curate` exits 0 whenever it produced a plan, pending changes
  included, matching `evolve --check` (CLI-141): pending curation is a state to
  report, not a failure. Only a genuine error (an unreadable registry, a failed
  apply) exits non-zero.
