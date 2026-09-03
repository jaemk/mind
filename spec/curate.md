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
    (CUR-8; advisory, never applied).

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
  registered but empty until the consumer notices.
- `CUR-4` `install` covers a registered curated source whose entry declares items
  that are not installed, under the ordinary precedence (DSC-62 `install-items`
  over DSC-58 `install`). An entry that declares neither is register-only by the
  curator's choice, so it is never proposed for install; nor is an entry whose
  declared items are all installed already. A marketplace entry's in-repo plugins
  count as declared (MKT-7), its external entries do not.
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
  absent provenance reads as "not curated", never as "no longer listed".
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
  applies nothing, and says how to apply it.
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
  `mind.toml` (and marketplace manifests) plus the registry and manifest.

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
  always), so a caller sees what was proposed as well as what ran.
- `CUR-14` `curate` exits 0 whenever it produced a plan, pending changes
  included, matching `evolve --check` (CLI-141): pending curation is a state to
  report, not a failure. Only a genuine error (an unreadable registry, a failed
  apply) exits non-zero.
