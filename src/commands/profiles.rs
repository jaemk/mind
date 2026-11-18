//! Source profiles: enabling, disabling, listing, and installing them (spec/profiles.md).

use super::*;

/// What [`install_source_profiles`] installs for one registered source
/// (PROF-9..15): the expansion of the profiles `selection` enables, unioned
/// with the `--learn` matches (PROF-15).
pub(crate) struct ProfileInstall<'a> {
    /// The registered source identity (STO-58).
    pub source_name: &'a str,
    /// The consumer's selection (flags, the PROF-13 picker, or a curator
    /// entry via [`crate::profile::Selection::from_nested`]).
    pub selection: &'a crate::profile::Selection,
    /// `meld --learn` patterns (CLI-236) whose matches join the batch.
    pub learn_patterns: &'a [String],
    /// Offer only expansion members missing from the source's recorded
    /// snapshot (PROF-21, PROF-23, PROF-27): `curate` sets this so a member
    /// the consumer forgot is not re-proposed.
    pub only_missing_from_snapshot: bool,
    /// Applying the install makes the selection's enabled set the source's
    /// whole recorded set instead of adding to it (PROF-40, PROF-41): a
    /// re-meld with profile flags, or a curator entry.
    pub replace: bool,
}

/// The result of [`install_source_profiles`] / its `--json` twin.
#[derive(Debug, Default)]
pub(crate) struct ProfileInstallOutcome {
    /// The enabled profile names, sorted (PROF-18).
    pub enabled: Vec<String>,
    /// Effective keys installed in this call, closure items included.
    pub installed_keys: Vec<String>,
    /// Whether the install was applied and recorded on the source (PROF-19):
    /// false for a declined prompt, a non-TTY run without `--yes`, a `--json`
    /// run without `--yes`, and an empty enabled set (PROF-10).
    pub applied: bool,
    /// Under `--json` without `--yes`: the items left to install, the union of
    /// the expansion, the `--learn` matches, and their closures, each once
    /// (PROF-18). Zero otherwise.
    pub pending: usize,
}

/// One item of a profile install batch.
pub(super) struct ProfileMember {
    pub(super) target: LearnTarget,
    /// The bare `kind:name` key in source truth (PROF-3), recorded in the
    /// snapshot. `None` for a `--learn`-only match outside the expansion.
    pub(super) bare_key: Option<String>,
    /// The enabled profiles this member came through; empty for a
    /// `--learn`-only match.
    pub(super) profiles: BTreeSet<String>,
    /// Matched a `--learn` pattern: named by the consumer (PROF-20).
    by_name: bool,
    /// Already installed before this call (tagged, not reinstalled).
    pub(super) installed: bool,
}

/// A resolved profile install, before any gate.
pub(super) struct ProfilePlan {
    /// The declared profile names, sorted (for the PROF-12 header).
    pub(super) declared: Vec<String>,
    pub(super) enabled: Vec<String>,
    pub(super) members: Vec<ProfileMember>,
    /// Effective key -> the profiles of every fresh member whose closure
    /// reaches it (PROF-6: a closure item carries its member's reasons).
    pub(super) reasons: std::collections::BTreeMap<String, BTreeSet<String>>,
    /// The union of the fresh members' closures, first-seen order (PROF-18).
    pub(super) pending: Vec<String>,
    /// Dependency trees of the fresh members that add dependencies (DEP-31).
    pub(super) trees: Vec<String>,
    /// The enabled set's whole expansion, snapshotted members included.
    pub(super) expansion: crate::profile::Expansion,
    /// [`ProfileInstall::replace`].
    pub(super) replace: bool,
}

impl ProfilePlan {
    pub(super) fn fresh(&self) -> impl Iterator<Item = &ProfileMember> {
        self.members.iter().filter(|m| !m.installed)
    }
}

/// The `[profiles]` table of a registered source's root `mind.toml` (PROF-1),
/// or `None` when it declares none. An item-link instance has none (PROF-8).
pub(crate) fn source_profile_table(
    paths: &Paths,
    source: &crate::source::Source,
) -> Result<Option<crate::profile::ProfileTable>> {
    if source.item_path.is_some() {
        return Ok(None);
    }
    Ok(MindToml::load(&source.clone_dir(paths))?.and_then(|m| m.profiles))
}

/// Whether a meld of `source_name` routes through the profile install
/// (PROF-9, PROF-12) rather than the CLI-23 install-all offer: a profile flag
/// was given, or the source declares a `default` profile. A source with no
/// `[profiles]`, or one whose table has no `default`, keeps install-all when
/// no flag is given (PROF-7).
pub(crate) fn meld_uses_profiles(
    paths: &Paths,
    source_name: &str,
    flags_given: bool,
) -> Result<bool> {
    if flags_given {
        return Ok(true);
    }
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(false);
    };
    Ok(source_profile_table(paths, source)?
        .is_some_and(|t| t.contains_key(crate::profile::DEFAULT)))
}

/// The PROF-13 profile picker: on an interactive meld with no profile flag,
/// no `--learn`, and no `--yes`, of a source declaring profiles but no
/// `default`, ask which profiles to enable. The answer is the enabled set,
/// exactly as `--profile <answer>` would give it; an empty answer returns
/// `None`, keeping the CLI-23 install-all offer. A source that declares
/// `default` is never asked (PROF-12).
///
/// `table` is the source's `[profiles]`, read from the fresh clone: `meld`
/// asks before the source's install hooks run, so they see the answer
/// (HOOK-142).
fn pick_profiles_from(
    source_name: &str,
    table: Option<&crate::profile::ProfileTable>,
    skip: bool,
) -> Result<Option<crate::profile::Selection>> {
    // spec: PROF-13
    if skip || json_mode() || !crate::hook::is_tty() {
        return Ok(None);
    }
    let Some(table) = table.filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if table.contains_key(crate::profile::DEFAULT) {
        return Ok(None);
    }
    let declared: Vec<&str> = table.keys().map(String::as_str).collect();
    println!(
        "{} declares profiles: {}",
        strip_ansi(source_name),
        declared.join(", ")
    );
    let answer =
        prompt_line("enable which profiles? (comma-separated; Enter offers every item): ")?;
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(None);
    }
    let named = crate::profile::parse_cli_names(&[answer.to_string()])?;
    Ok(Some(crate::profile::Selection {
        include_default: false,
        named,
        all: false,
    }))
}

/// Resolve a profile install for `req` (PROF-5, PROF-6, PROF-15): the enabled
/// set, the expansion's members plus the `--learn` matches, which of them are
/// already installed, and each fresh member's dependency closure.
pub(super) fn plan_profile_install(paths: &Paths, req: &ProfileInstall<'_>) -> Result<ProfilePlan> {
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(req.source_name) else {
        return Err(MindError::SourceNotFound {
            name: req.source_name.to_string(),
        });
    };
    let table = source_profile_table(paths, source)?;
    // spec: PROF-14 -- an undeclared name is ProfileNotFound, after the clone.
    let enabled = crate::profile::enabled_set(table.as_ref(), req.selection, req.source_name)?;
    let items = scan_one(paths, source)?;
    // spec: PROF-5 -- an exact member naming no item is BadReference here.
    let expansion = match &table {
        Some(t) if !enabled.is_empty() => {
            crate::profile::expand(t, &enabled, &items, req.source_name)?
        }
        _ => crate::profile::Expansion::default(),
    };
    let snapshot: BTreeSet<String> = if req.only_missing_from_snapshot {
        source
            .profile_snapshot
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        BTreeSet::new()
    };
    let bare = |it: &CatalogItem| format!("{}:{}", it.kind.as_str(), it.name);
    let manifest = Manifest::load(paths)?;
    let mut members: Vec<ProfileMember> = Vec::new();
    for (key, profiles) in &expansion.keys {
        if snapshot.contains(key) {
            continue;
        }
        let Some(it) = items.iter().find(|it| bare(it) == *key) else {
            continue;
        };
        let target = LearnTarget {
            source: req.source_name.to_string(),
            key: it.key().as_str().to_string(),
        };
        members.push(ProfileMember {
            installed: manifest.items.contains_key(&target.key),
            target,
            bare_key: Some(key.clone()),
            profiles: profiles.clone(),
            by_name: false,
        });
    }
    // spec: PROF-15 -- the batch is the union of the expansion and the
    // `--learn` matches; a pattern matching nothing is still an error
    // (CLI-236).
    if !req.learn_patterns.is_empty() {
        let (matched, _fresh) = learn_targets(paths, req.source_name, req.learn_patterns)?;
        for t in matched {
            if let Some(m) = members.iter_mut().find(|m| m.target == t) {
                m.by_name = true;
                continue;
            }
            members.push(ProfileMember {
                installed: manifest.items.contains_key(&t.key),
                target: t,
                bare_key: None,
                profiles: BTreeSet::new(),
                by_name: true,
            });
        }
    }
    let mut reasons: std::collections::BTreeMap<String, BTreeSet<String>> =
        std::collections::BTreeMap::new();
    let mut pending: Vec<String> = Vec::new();
    let mut trees: Vec<String> = Vec::new();
    let fresh: Vec<&ProfileMember> = members.iter().filter(|m| !m.installed).collect();
    // spec: PROF-6 -- each member resolves through the ordinary learn path,
    // so its closure is DEP-30's. spec: PROF-35 -- one catalog scan serves
    // every member.
    let targets: Vec<&LearnTarget> = fresh.iter().map(|m| &m.target).collect();
    let plans = learn_plans(paths, &targets)?;
    for (m, plan) in fresh.into_iter().zip(plans) {
        for key in &plan.keys {
            reasons
                .entry(key.clone())
                .or_default()
                .extend(m.profiles.iter().cloned());
            if !pending.contains(key) {
                pending.push(key.clone());
            }
        }
        if plan.adds_dependencies {
            trees.push(plan.tree);
        }
    }
    Ok(ProfilePlan {
        declared: table
            .as_ref()
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default(),
        enabled,
        members,
        reasons,
        pending,
        trees,
        expansion,
        replace: req.replace,
    })
}

/// Install a plan's fresh members, then record the outcome (PROF-19, PROF-20):
/// tag every installed key (closure items too) with its member's profiles,
/// tag each already-installed member (marking a by-name one explicit first),
/// mark `--learn` matches explicit, and union the enabled set and the applied
/// members' bare keys into the source's registry entry.
///
/// `collect` installs silently (the `--json` path); otherwise each member goes
/// through `learn`, which narrates. Either way `flow.yes` must already be set
/// by the caller's gate. A failure part-way still records what installed.
pub(super) fn apply_profile_plan(
    paths: &Paths,
    source_name: &str,
    plan: &ProfilePlan,
    flow: InstallFlow,
    collect: bool,
) -> Result<Vec<String>> {
    let before: HashSet<String> = Manifest::load(paths)?.items.keys().cloned().collect();
    // spec: HOOK-141 -- hooks run by these installs see the enabled set this
    // install records (the recorded set plus the profiles being enabled) and
    // each item's profiles, before either is written.
    let recorded = Registry::load(paths)?
        .find(source_name)
        .and_then(|s| s.profiles.clone())
        .unwrap_or_default();
    let enabled: BTreeSet<String> = if plan.replace {
        plan.enabled.iter().cloned().collect()
    } else {
        recorded.into_iter().chain(plan.enabled.clone()).collect()
    };
    let _scope = crate::hook::scope_profiles(crate::hook::ProfileScope {
        source: source_name.to_string(),
        enabled: enabled.into_iter().collect(),
        reasons: plan.reasons.clone(),
    });
    // spec: PROF-35 -- every fresh member installs in one learn pass over
    // their combined closure, rather than one full learn per member.
    let targets: Vec<LearnTarget> = plan.fresh().map(|m| m.target.clone()).collect();
    let failure: Option<MindError> = if targets.is_empty() {
        None
    } else if collect {
        learn_collecting_selected(paths, Selection::ExactMany(&targets), flow).err()
    } else {
        learn_selected(paths, Selection::ExactMany(&targets), false, flow).err()
    };
    let mut manifest = Manifest::load(paths)?;
    let installed_keys: Vec<String> = plan
        .pending
        .iter()
        .filter(|k| !before.contains(*k) && manifest.items.contains_key(*k))
        .cloned()
        .collect();
    // spec: PROF-20 -- a closure item carries the profiles of the member that
    // pulled it in (PROF-6).
    for key in &installed_keys {
        if let (Some(entry), Some(reasons)) = (manifest.items.get_mut(key), plan.reasons.get(key)) {
            crate::profile::tag_reasons(entry, reasons, false);
        }
    }
    let mut applied_keys: Vec<String> = Vec::new();
    for m in &plan.members {
        let Some(entry) = manifest.items.get_mut(&m.target.key) else {
            continue; // not installed (failed, or skipped by policy)
        };
        if m.installed {
            crate::profile::tag_reasons(entry, &m.profiles, true);
        }
        if m.by_name {
            crate::profile::mark_explicit(entry);
        }
        if let Some(bare) = &m.bare_key {
            applied_keys.push(bare.clone());
        }
    }
    if plan.replace {
        // spec: PROF-40 PROF-41 -- the replaced set is the source's whole
        // enabled set. Every installed member of its expansion (a snapshotted
        // one `curate` did not offer too) is tagged with the profiles it is
        // in now; then names no longer enabled leave the reasons of items
        // that keep another, and the rest are left as leftovers (PROF-37).
        // A closure item carries the profiles of each member that reaches it
        // (PROF-6), so it is retagged along with its members.
        // Best effort: the installs already ran, so a scan error here must not
        // stop them from being recorded.
        let closure_reasons =
            replaced_closure_reasons(paths, source_name, &plan.expansion).unwrap_or_default();
        for entry in manifest.items.values_mut() {
            if entry.source != source_name {
                continue;
            }
            let bare = format!("{}:{}", entry.kind.as_str(), entry.bare_name);
            let pre_existing = !installed_keys.iter().any(|k| *k == entry.key().as_str());
            if let Some(profiles) = plan.expansion.keys.get(&bare) {
                crate::profile::tag_reasons(entry, profiles, pre_existing);
            }
            if let Some(profiles) = closure_reasons.get(&bare) {
                crate::profile::tag_reasons(entry, profiles, pre_existing);
            }
            crate::profile::strip_disabled_reasons(entry, &plan.enabled);
        }
        manifest.save(paths)?;
        let mut registry = Registry::load(paths)?;
        if let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) {
            crate::profile::record_replaced(
                source,
                &plan.enabled,
                &plan.expansion.key_list(),
                &applied_keys,
            );
            registry.save(paths)?;
        }
        return match failure {
            Some(e) => Err(e),
            None => Ok(installed_keys),
        };
    }
    manifest.save(paths)?;
    if !plan.enabled.is_empty() {
        // spec: PROF-19 -- only keys actually present are snapshotted, so a
        // member that did not install is offered again later (PROF-21).
        let mut registry = Registry::load(paths)?;
        if let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) {
            crate::profile::record_applied(source, &plan.enabled, &applied_keys);
            registry.save(paths)?;
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(installed_keys),
    }
}

/// For a replacing install (PROF-40): each item of `source_name` that a
/// member of `expansion` reaches through its dependencies (DEP-1, DEP-4),
/// keyed by bare `kind:name`, with the profiles of every member reaching it.
fn replaced_closure_reasons(
    paths: &Paths,
    source_name: &str,
    expansion: &crate::profile::Expansion,
) -> Result<std::collections::BTreeMap<String, BTreeSet<String>>> {
    let mut out: std::collections::BTreeMap<String, BTreeSet<String>> =
        std::collections::BTreeMap::new();
    if expansion.keys.is_empty() {
        return Ok(out);
    }
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(out);
    };
    let catalog = catalog::scan(paths, &single(source))?;
    let bare = |c: &CatalogItem| format!("{}:{}", c.kind.as_str(), c.name);
    for (i, member) in catalog.iter().enumerate() {
        let Some(profiles) = expansion.keys.get(&bare(member)) else {
            continue;
        };
        for j in crate::deps::reachable(&catalog, &[i], read_item_text) {
            out.entry(bare(&catalog[j]))
                .or_default()
                .extend(profiles.iter().cloned());
        }
    }
    Ok(out)
}

/// The PROF-12 preview header: the enabled profiles, the other declared ones,
/// and how to enable more.
fn print_profile_header(source_name: &str, plan: &ProfilePlan) {
    if plan.declared.is_empty() {
        return;
    }
    let enabled = if plan.enabled.is_empty() {
        "none".to_string()
    } else {
        plan.enabled.join(", ")
    };
    println!("profiles of {}: enabled {enabled}", strip_ansi(source_name));
    let others: Vec<&str> = plan
        .declared
        .iter()
        .filter(|d| !plan.enabled.contains(d))
        .map(String::as_str)
        .collect();
    if !others.is_empty() {
        // spec: PROF-34 -- name commands that still work after the meld.
        let quoted = crate::error::shell_quote(&strip_ansi(source_name));
        println!(
            "  not enabled: {}; list their items with `mind recall --profiles {quoted}`, \
             enable one with `mind learn {quoted} --profile <name>` (or --all-profiles)",
            others.join(", ")
        );
    }
}

/// The PROF-35 progress line printed before a profile install resolves, so a
/// large source does not look stuck. Not printed under `--json`.
fn print_profile_progress(paths: &Paths, source_name: &str, selection: &crate::profile::Selection) {
    if json_mode() {
        return;
    }
    // Only the enabled set's names; an error here (an undeclared name) is
    // raised by the resolve that follows, so it is not reported twice.
    let Ok(registry) = Registry::load(paths) else {
        return;
    };
    let Some(source) = registry.find(source_name) else {
        return;
    };
    let Ok(table) = source_profile_table(paths, source) else {
        return;
    };
    let Ok(enabled) = crate::profile::enabled_set(table.as_ref(), selection, source_name) else {
        return;
    };
    if enabled.is_empty() {
        return;
    }
    println!(
        "{} resolving profile(s) {} of {} ...",
        crate::render::ctx().bullet(),
        enabled.join(", "),
        strip_ansi(source_name)
    );
}

/// The pasteable command that installs `plan` later (PROF-9, PROF-10).
fn profile_remedy(source_name: &str, plan: &ProfilePlan) -> String {
    let quoted_source = crate::error::shell_quote(&strip_ansi(source_name));
    let mut steps: Vec<String> = Vec::new();
    if plan.enabled.is_empty() {
        if plan.members.is_empty() {
            steps.push(format!("mind learn {quoted_source} --profile <name>"));
        }
    } else {
        steps.push(format!(
            "mind learn {quoted_source} --profile {}",
            plan.enabled.join(",")
        ));
    }
    for m in plan.fresh().filter(|m| m.profiles.is_empty()) {
        steps.push(format!(
            "mind learn {}",
            crate::error::shell_quote(&m.target.display())
        ));
    }
    steps.join(" && ")
}

/// Print the batch preview (DEP-31 trees, then the union list) for a plan.
fn print_profile_preview(paths: &Paths, plan: &ProfilePlan) -> Result<()> {
    let out = crate::render::ctx();
    for tree in &plan.trees {
        print!("{tree}");
    }
    println!("would learn {} item(s):", plan.pending.len());
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let rows = plan
        .pending
        .iter()
        .map(|k| {
            let src = items
                .iter()
                .find(|i| i.key().as_str() == k)
                .map(|i| strip_ansi(&i.source))
                .unwrap_or_default();
            vec![strip_ansi(k), src]
        })
        .collect::<Vec<_>>();
    out.print_rows(&rows);
    Ok(())
}

/// Install a source's enabled profiles (PROF-9..15) under the CLI-23 meld
/// gate, in place of the install-all offer: one preview and one prompt on a
/// TTY, `--yes` installs unprompted, a non-TTY run without `--yes` installs
/// nothing and names `mind learn <source> --profile <name>`. Each member goes
/// through the ordinary `learn` path (PROF-6); an already-installed member is
/// tagged, not reinstalled. The enabled set and snapshot are recorded only
/// when the install is applied (PROF-19).
// spec: PROF-9 PROF-10 PROF-12 PROF-15
pub(crate) fn install_source_profiles(
    paths: &Paths,
    req: &ProfileInstall<'_>,
    flow: InstallFlow,
) -> Result<ProfileInstallOutcome> {
    let outcome = install_source_profiles_once(paths, req, flow)?;
    // spec: PROF-40 PROF-41 -- a replaced set can leave items behind; name
    // the command that removes them.
    if req.replace && outcome.applied && !json_mode() {
        note_profile_leftovers(paths, req.source_name);
    }
    Ok(outcome)
}

/// The profile leftovers (PROF-37) `source_name` has now, and whether it
/// declares `prune`. Empty when they cannot be computed: the caller only
/// reports them.
pub(crate) fn source_profile_leftovers(paths: &Paths, source_name: &str) -> (Vec<String>, bool) {
    (|| -> Result<(Vec<String>, bool)> {
        let registry = Registry::load(paths)?;
        let Some(source) = registry.find(source_name) else {
            return Ok((Vec::new(), false));
        };
        let manifest = Manifest::load(paths)?;
        let catalog = catalog::scan(paths, &registry)?;
        let graph = manifest_graph(&catalog, &manifest);
        Ok((
            profile_leftovers(paths, &catalog, &manifest, &graph, source)?,
            declares_prune(paths, source),
        ))
    })()
    .unwrap_or_default()
}

/// Print how many profile leftovers (PROF-37) `source_name` has and the
/// `mind prune <source>` that removes them. Silent when there are none or
/// they cannot be computed.
pub(crate) fn note_profile_leftovers(paths: &Paths, source_name: &str) {
    let (found, prunes) = source_profile_leftovers(paths, source_name);
    if found.is_empty() {
        return;
    }
    let safe = strip_ansi(source_name);
    // spec: PROF-39 -- a pruning source's next upgrade removes them too.
    let also = if prunes {
        " (or `mind upgrade`, since the source declares `prune`)"
    } else {
        ""
    };
    println!(
        "note: {} item(s) of {safe} are no longer in its enabled profiles: {}; run `mind prune {}` \
         to remove them{also}",
        found.len(),
        found
            .iter()
            .map(|k| strip_ansi(k))
            .collect::<Vec<_>>()
            .join(", "),
        crate::error::shell_quote(&safe)
    );
}

/// [`install_source_profiles`] without the leftovers note, for a caller that
/// prints it later (`curate`, after its upgrade pass may have pruned them).
///
/// Under `--json` it prints nothing and never prompts: `--yes` installs, and
/// without it the outcome carries the pending count (CLI-156, PROF-18).
/// Nothing is recorded without `--yes`.
// spec: PROF-18 CLI-156
pub(crate) fn install_source_profiles_once(
    paths: &Paths,
    req: &ProfileInstall<'_>,
    flow: InstallFlow,
) -> Result<ProfileInstallOutcome> {
    // spec: PROF-35
    print_profile_progress(paths, req.source_name, req.selection);
    let plan = plan_profile_install(paths, req)?;
    let mut outcome = ProfileInstallOutcome {
        enabled: plan.enabled.clone(),
        ..ProfileInstallOutcome::default()
    };
    let json = json_mode();
    if !json {
        print_profile_header(req.source_name, &plan);
    }
    if plan.members.is_empty() {
        if plan.enabled.is_empty() {
            // spec: PROF-40 -- a replacing selection that enables nothing
            // still clears the recorded set.
            if plan.replace {
                apply_profile_plan(paths, req.source_name, &plan, flow, json)?;
                outcome.applied = true;
            }
            // spec: PROF-10 -- nothing enabled, nothing offered.
            if !json {
                println!(
                    "note: no profile enabled; nothing installed from {}; run `{}` to install a \
                     profile later",
                    strip_ansi(req.source_name),
                    profile_remedy(req.source_name, &plan)
                );
            }
        } else {
            // Every member was in the snapshot (curate): nothing to propose.
            // Applying still records the enabled set.
            apply_profile_plan(paths, req.source_name, &plan, flow, json)?;
            outcome.applied = true;
        }
        return Ok(outcome);
    }
    if plan.pending.is_empty() {
        // Every member is installed already (CLI-157): tag and record.
        if !json {
            println!(
                "already installed; nothing to do ({} item(s))",
                plan.members.len()
            );
        }
        apply_profile_plan(paths, req.source_name, &plan, flow, json)?;
        outcome.applied = true;
        return Ok(outcome);
    }
    if flow.yes {
        outcome.installed_keys = apply_profile_plan(paths, req.source_name, &plan, flow, json)?;
        outcome.applied = true;
        return Ok(outcome);
    }
    let remedy = profile_remedy(req.source_name, &plan);
    if json {
        outcome.pending = plan.pending.len();
        return Ok(outcome);
    }
    if !crate::hook::is_tty() {
        println!(
            "note: registered only, nothing installed (not a TTY); {} has {} item(s) to \
             install; run `{remedy}` (or re-meld with --yes)",
            strip_ansi(req.source_name),
            plan.pending.len()
        );
        return Ok(outcome);
    }
    print_profile_preview(paths, &plan)?;
    let question = format!("install these {} item(s) now?", plan.pending.len());
    // spec: HOOK-180
    match ask_plan(
        paths,
        Some(PlanItems {
            question: &question,
            always: true,
            default_yes: true,
            removal: false,
            listing: Vec::new(),
        }),
        source_keys_plan(paths, req.source_name, &plan.pending, flow.accept)?,
    )? {
        PlanOutcome::Proceed => {
            outcome.installed_keys = apply_profile_plan(
                paths,
                req.source_name,
                &plan,
                InstallFlow { yes: true, ..flow },
                false,
            )?;
            outcome.applied = true;
        }
        PlanOutcome::Cancel => println!("skipped; run `{remedy}` to install later"),
        PlanOutcome::Abort => {}
    }
    Ok(outcome)
}

/// The install hooks of `source_name`'s items named by `keys` (effective
/// `kind:name`), in `keys` order (HOOK-180). A key the catalog no longer
/// offers contributes nothing.
fn source_keys_plan(
    paths: &Paths,
    source_name: &str,
    keys: &[String],
    accept: crate::hook::Accept,
) -> Result<Vec<crate::consent::PlannedHook>> {
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(Vec::new());
    };
    let items = catalog::scan(paths, &single(source))?;
    let mut plan: Vec<crate::consent::PlannedHook> = keys
        .iter()
        .filter_map(|k| items.iter().find(|it| it.key().as_str() == k))
        .flat_map(|it| item_install_plan(paths, &registry, it, false, accept))
        .collect();
    // spec: HOOK-198
    plan.extend(crate::items_hook::planned(
        paths,
        &registry,
        [source_name],
        crate::items_hook::PlanWhen::Touched,
        accept,
    ));
    Ok(plan)
}

/// Resolve a CLI-36 source selector (no `#` part) to exactly one registered
/// source identity, for `learn`/`forget <source> --profile` (PROF-17,
/// PROF-25).
fn resolve_profile_source(paths: &Paths, source_ref: &str) -> Result<String> {
    let source_ref = source_ref.trim();
    if source_ref.contains('#') {
        return Err(MindError::InvalidItemRef {
            name: source_ref.to_string(),
        });
    }
    let registry = Registry::load(paths)?;
    let matching: Vec<String> = registry
        .sources
        .iter()
        .filter(|s| source_matches(&s.name, source_ref))
        .map(|s| s.name.clone())
        .collect();
    match matching.as_slice() {
        [] => Err(MindError::SourceNotFound {
            name: source_ref.to_string(),
        }),
        [only] => Ok(only.clone()),
        many => Err(MindError::AmbiguousSource {
            query: source_ref.to_string(),
            candidates: many.iter().map(|n| strip_ansi(n)).collect(),
        }),
    }
}

/// `forget <source> --profile <NAME>` (PROF-25): disable one enabled profile
/// of a source. NAME leaves the recorded set; each installed item whose
/// recorded reasons are now only disabled profiles, and that is not explicit,
/// is forgotten, except one another installed item depends on, which is kept
/// and named (DEP-60). Kept items lose the disabled name from their reasons,
/// and the snapshot is recomputed from the remaining enabled set (both source
/// fields are cleared when none remains). Removing more than one item goes
/// through `forget`'s glob gate (CLI-42, LIFE-45). A NAME that is not enabled
/// changes nothing.
// spec: PROF-25
pub fn forget_source_profile(
    paths: &Paths,
    source_ref: &str,
    profile: &str,
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let out = crate::render::ctx();
    let names = crate::profile::parse_cli_names(&[profile.to_string()])?;
    let [name] = names.as_slice() else {
        return Err(MindError::InvalidProfileName {
            name: profile.trim().to_string(),
            reason: "forget --profile takes one profile name".to_string(),
        });
    };
    let source_name = resolve_profile_source(paths, source_ref)?;
    let safe_source = strip_ansi(&source_name);
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(&source_name) else {
        return Err(MindError::SourceNotFound { name: source_name });
    };
    let recorded: Vec<String> = source.profiles.clone().unwrap_or_default();
    if !recorded.iter().any(|p| p == name) {
        if out.json {
            let mut r = MutationResult::new("forget", source_ref, "no-op");
            r.profiles = Some(recorded);
            return print_json(&r);
        }
        let enabled = if recorded.is_empty() {
            "none".to_string()
        } else {
            recorded.join(", ")
        };
        println!(
            "profile '{name}' is not enabled for {safe_source} (enabled: {enabled}); nothing changed"
        );
        return Ok(());
    }
    let ProfileDisable {
        plan,
        remove,
        kept,
        mut manifest,
        catalog,
    } = resolve_profile_disable(paths, &registry, source, name, true)?;

    if remove.len() > 1 && !yes {
        if !out.json {
            println!(
                "disabling profile '{name}' of {safe_source} would remove {} item(s):",
                remove.len()
            );
            for key in &remove {
                println!("  {} {}", out.warn(), strip_ansi(key));
            }
        }
        // spec: LIFE-45 -- `--json` is non-interactive.
        if !crate::hook::is_tty() || out.json {
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!(
                        "disabling profile '{name}' of '{safe_source}' (removes {} items)",
                        remove.len()
                    ),
                    out.json,
                ),
            });
        }
        if !confirm("remove these item(s)?")? {
            println!("cancelled; nothing changed");
            return Ok(());
        }
    }

    // Strip the disabled name from every kept item's reasons; an item kept
    // only for a dependent keeps its reasons, so it stays removable later.
    for (key, reasons) in &plan.strip {
        if let Some(entry) = manifest.items.get_mut(key) {
            entry.profiles = reasons.clone();
        }
    }
    // spec: HOOK-141 -- uninstall hooks see the set that stays enabled.
    let _scope = crate::hook::scope_profiles(crate::hook::ProfileScope {
        source: source_name.to_string(),
        enabled: plan.remaining_enabled.clone().unwrap_or_default(),
        reasons: Default::default(),
    });
    let mut removed: Vec<String> = Vec::new();
    for key in &remove {
        let Some(item) = manifest.items.remove(key) else {
            continue;
        };
        let uninstall_hooks: Vec<&crate::mindfile::ResolvedHook> =
            item_catalog_match(&catalog, &item)
                .map(|c| c.uninstall_hooks())
                .unwrap_or_default();
        let commit = source.commit.clone().unwrap_or_default();
        if let Err(e) = uninstall_item(paths, &item, &uninstall_hooks, &commit, accept) {
            manifest.items.insert(key.clone(), item);
            // spec: LIFE-48 -- a save failure here must not mask `e`.
            if let Err(se) = manifest.save(paths) {
                warn_manifest_save_also_failed(&se);
            }
            return Err(e);
        }
        let key_display = strip_ansi(key);
        if !out.json {
            println!("{} forgot {key_display}", out.ok());
        }
        removed.push(key_display);
    }
    manifest.save(paths)?;
    let mut registry = Registry::load(paths)?;
    if let Some(s) = registry.sources.iter_mut().find(|s| s.name == source_name) {
        s.profiles = plan.remaining_enabled.clone();
        s.profile_snapshot = plan.new_snapshot.clone();
    }
    registry.save(paths)?;

    let kept_lines: Vec<String> = kept
        .iter()
        .map(|(k, deps)| {
            format!(
                "{} (needed by {})",
                strip_ansi(k),
                deps.iter()
                    .map(|d| strip_ansi(d))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    if out.json {
        let mut r = MutationResult::new("forget", source_ref, "disabled");
        r.removed = removed;
        r.kept = kept_lines;
        r.profiles = Some(plan.remaining_enabled.unwrap_or_default());
        return print_json(&r);
    }
    for line in &kept_lines {
        println!("{} kept {line}", out.warn());
    }
    let left = match &plan.remaining_enabled {
        Some(v) if !v.is_empty() => format!("enabled: {}", v.join(", ")),
        _ => "no profile enabled".to_string(),
    };
    println!(
        "{} disabled profile '{name}' of {safe_source} ({left})",
        out.ok()
    );
    Ok(())
}

pub fn forget_source_profile_preview(
    paths: &Paths,
    source_ref: &str,
    profile: &str,
    accept: crate::hook::Accept,
) -> Result<()> {
    let names = crate::profile::parse_cli_names(&[profile.to_string()])?;
    let [name] = names.as_slice() else {
        return Err(MindError::InvalidProfileName {
            name: profile.trim().to_string(),
            reason: "forget --profile takes one profile name".to_string(),
        });
    };
    let source_name = resolve_profile_source(paths, source_ref)?;
    let registry = Registry::load(paths)?;
    let source = registry
        .find(&source_name)
        .ok_or_else(|| MindError::SourceNotFound {
            name: source_name.clone(),
        })?;
    let enabled = source.profiles.clone().unwrap_or_default();
    if !enabled.iter().any(|p| p == name) {
        let manifest = Manifest::load(paths)?;
        return preview_removals(
            paths,
            &manifest,
            Vec::new(),
            "forget",
            accept,
            Some(
                serde_json::json!({"source":strip_ansi(&source_name),"name":strip_ansi(name),"enabled":false,"remaining":enabled,"kept":[]}),
            ),
        );
    }
    let ProfileDisable {
        plan,
        remove,
        kept,
        manifest,
        ..
    } = resolve_profile_disable(paths, &registry, source, name, true)?;
    let kept: Vec<_> = kept.iter().map(|(key, deps)| serde_json::json!({"key":strip_ansi(key),"dependents":deps.iter().map(|d|strip_ansi(d)).collect::<Vec<_>>()})).collect();
    let extra = serde_json::json!({"source":strip_ansi(&source_name),"name":strip_ansi(name),"enabled":true,"remaining":plan.remaining_enabled.unwrap_or_default(),"kept":kept});
    preview_removals(paths, &manifest, remove, "forget", accept, Some(extra))
}

/// What disabling one profile of a source resolves to (PROF-25), before any
/// gate: the plan, the keys to forget, and the `(key, dependents)` pairs kept
/// because another installed item depends on them (DEP-60).
struct ProfileDisable {
    pub(super) plan: crate::profile::DisablePlan,
    pub(super) remove: Vec<String>,
    pub(super) kept: Vec<(String, Vec<String>)>,
    pub(super) manifest: Manifest,
    pub(super) catalog: Vec<CatalogItem>,
}

/// Resolve disabling `name` on `source` (PROF-25). A broken remaining table
/// (an exact member naming no item, PROF-5) keeps the old snapshot, warning
/// when `warn` is set, so disabling always works.
fn resolve_profile_disable(
    paths: &Paths,
    registry: &Registry,
    source: &crate::source::Source,
    name: &str,
    warn: bool,
) -> Result<ProfileDisable> {
    let source_name = &source.name;
    let table = source_profile_table(paths, source)?.unwrap_or_default();
    let remaining: Vec<String> = source
        .profiles
        .iter()
        .flatten()
        .filter(|p| *p != name)
        .cloned()
        .collect();
    let items = scan_one(paths, source)?;
    let (expansion, expand_failed) =
        match crate::profile::expand(&table, &remaining, &items, source_name) {
            Ok(e) => (e, false),
            Err(e) => {
                if warn {
                    crate::render::warn(format!(
                        "could not expand the remaining profiles of {}: {}; keeping its \
                         snapshot unchanged",
                        strip_ansi(source_name),
                        strip_ansi(&e.to_string())
                    ));
                }
                (crate::profile::Expansion::default(), true)
            }
        };
    let manifest = Manifest::load(paths)?;
    let mut plan = {
        let of_source: Vec<&crate::manifest::InstalledItem> = manifest
            .items
            .values()
            .filter(|m| m.source == *source_name)
            .collect();
        crate::profile::disable_plan(source, &of_source, name, &expansion)
    };
    if expand_failed && plan.remaining_enabled.is_some() {
        plan.new_snapshot = source.profile_snapshot.clone();
    }

    // spec: DEP-60 -- an item another installed item (not itself being
    // removed) depends on is kept and named.
    let catalog = catalog::scan(paths, registry).unwrap_or_default();
    let graph = crate::deps::installed_graph(
        &catalog,
        |it| {
            manifest
                .items
                .get(it.key().as_str())
                .is_some_and(|m| m.source == it.source)
        },
        read_item_text,
    );
    let (remove, kept) = keep_needed(&graph, plan.removable.clone());
    Ok(ProfileDisable {
        plan,
        remove,
        kept,
        manifest,
        catalog,
    })
}

/// Split `remove` into the keys that can go and the `(key, dependents)` pairs
/// kept because an installed item outside the removed set depends on them
/// (DEP-60), repeating until no removed key is still needed.
fn keep_needed(
    graph: &crate::deps::InstalledGraph,
    mut remove: Vec<String>,
) -> (Vec<String>, Vec<(String, Vec<String>)>) {
    let mut kept: Vec<(String, Vec<String>)> = Vec::new();
    loop {
        let blocked = remove.iter().find_map(|k| {
            let deps: Vec<String> = graph
                .dependents(k)
                .into_iter()
                .filter(|d| !remove.contains(d))
                .collect();
            (!deps.is_empty()).then(|| (k.clone(), deps))
        });
        match blocked {
            Some((k, deps)) => {
                remove.retain(|r| *r != k);
                kept.push((k, deps));
            }
            None => break,
        }
    }
    (remove, kept)
}

/// The installed dependency graph over `manifest` (DEP-60), matching each
/// catalog item by stable identity (source as well as key).
pub(super) fn manifest_graph(
    catalog: &[CatalogItem],
    manifest: &Manifest,
) -> crate::deps::InstalledGraph {
    crate::deps::installed_graph(
        catalog,
        |it| {
            manifest
                .items
                .get(it.key().as_str())
                .is_some_and(|m| m.source == it.source)
        },
        read_item_text,
    )
}

/// The profile leftovers of `source` (PROF-37): its installed items that
/// were installed for profiles, are not explicit, are still offered, and are
/// outside the expansion of its enabled set (the recorded names the source
/// still declares; none for a source that is not profile-tracked), minus any
/// another installed item still depends on (DEP-60). Returns manifest keys,
/// sorted. A source that declares no profiles has none. An expansion that
/// fails (PROF-5) is the error: what the enabled set wants is unknown, so
/// nothing can be called a leftover.
// spec: PROF-37
pub(super) fn profile_leftovers(
    paths: &Paths,
    catalog: &[CatalogItem],
    manifest: &Manifest,
    graph: &crate::deps::InstalledGraph,
    source: &crate::source::Source,
) -> Result<Vec<String>> {
    let of_source: Vec<&crate::manifest::InstalledItem> = manifest
        .items
        .values()
        .filter(|m| m.source == source.name && !m.profiles.is_empty() && !m.is_explicit())
        .collect();
    if of_source.is_empty() {
        return Ok(Vec::new());
    }
    // A source that stopped declaring profiles altogether says nothing about
    // which items its consumers want, so it has no leftovers.
    let Some(table) = source_profile_table(paths, source)?.filter(|t| !t.is_empty()) else {
        return Ok(Vec::new());
    };
    let enabled: Vec<String> = source
        .profiles
        .iter()
        .flatten()
        .filter(|p| table.contains_key(*p))
        .cloned()
        .collect();
    let expansion = crate::profile::expand(&table, &enabled, catalog, &source.name)?;
    let offered: BTreeSet<String> = catalog
        .iter()
        .filter(|c| c.source == source.name)
        .map(|c| format!("{}:{}", c.kind.as_str(), c.name))
        .collect();
    let mut candidates = crate::profile::leftover_candidates(&of_source, &offered, &expansion);
    // spec: PROF-37 -- the enabled set also wants what its members require,
    // including a member not installed yet (an addition this pass or a later
    // upgrade installs): pruning such an item would only have that install
    // bring it back. A member the consumer forgot (in the snapshot, not
    // installed, PROF-23) wants nothing.
    let snapshot: BTreeSet<&String> = source.profile_snapshot.iter().flatten().collect();
    let roots: Vec<usize> = catalog
        .iter()
        .enumerate()
        .filter(|(_, c)| c.source == source.name)
        .filter(|(_, c)| {
            let bare = format!("{}:{}", c.kind.as_str(), c.name);
            expansion.keys.contains_key(&bare)
                && (!snapshot.contains(&bare)
                    || manifest
                        .items
                        .get(c.key().as_str())
                        .is_some_and(|m| m.source == source.name))
        })
        .map(|(i, _)| i)
        .collect();
    let required: BTreeSet<String> = crate::deps::reachable(catalog, &roots, read_item_text)
        .into_iter()
        .map(|i| &catalog[i])
        .filter(|c| c.source == source.name)
        .map(|c| format!("{}:{}", c.kind.as_str(), c.name))
        .collect();
    candidates.retain(|k| {
        manifest
            .items
            .get(k.as_str())
            .is_none_or(|m| !required.contains(&format!("{}:{}", m.kind.as_str(), m.bare_name)))
    });
    let (mut remove, _kept) = keep_needed(graph, candidates);
    remove.sort();
    Ok(remove)
}

/// What changing one profile of a melded source would do, for the probe
/// TUI's confirm modal (TUI-78): the effective keys enabling `profile` would
/// install, or the keys disabling it would forget plus the keys kept for a
/// dependent (DEP-60). Read-only; the change itself runs through
/// [`learn_source_profiles`] / [`forget_source_profile`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ProfileChangePreview {
    pub install: Vec<String>,
    pub remove: Vec<String>,
    pub kept: Vec<String>,
}

/// The effective keys `learn <source> --profile <each of profiles>` would
/// install (PROF-17): for the TUI's Enable and Install-enabled confirms
/// (TUI-78, TUI-79). Read-only.
pub(crate) fn profiles_install_preview(
    paths: &Paths,
    source_name: &str,
    profiles: &[String],
) -> Result<ProfileChangePreview> {
    let selection = crate::profile::Selection {
        include_default: false,
        named: profiles.to_vec(),
        all: false,
    };
    let req = ProfileInstall {
        source_name,
        selection: &selection,
        learn_patterns: &[],
        only_missing_from_snapshot: false,
        replace: false,
    };
    let plan = plan_profile_install(paths, &req)?;
    Ok(ProfileChangePreview {
        install: plan.pending.iter().map(|k| strip_ansi(k)).collect(),
        ..ProfileChangePreview::default()
    })
}

/// Preview enabling (`enable`) or disabling `profile` on `source_name`
/// (TUI-78, PROF-17, PROF-25). Disabling a profile that is not enabled
/// previews nothing.
pub(crate) fn profile_change_preview(
    paths: &Paths,
    source_name: &str,
    profile: &str,
    enable: bool,
) -> Result<ProfileChangePreview> {
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Err(MindError::SourceNotFound {
            name: source_name.to_string(),
        });
    };
    if enable {
        return profiles_install_preview(paths, source_name, &[profile.to_string()]);
    }
    if !source.profiles.iter().flatten().any(|p| p == profile) {
        return Ok(ProfileChangePreview::default());
    }
    let d = resolve_profile_disable(paths, &registry, source, profile, false)?;
    Ok(ProfileChangePreview {
        install: Vec::new(),
        remove: d.remove.iter().map(|k| strip_ansi(k)).collect(),
        kept: d.kept.iter().map(|(k, _)| strip_ansi(k)).collect(),
    })
}

/// `learn <source> --profile <NAME>` / `--all-profiles` (PROF-17): install the
/// selected profiles' expansion for an already-melded source and add them to
/// its recorded set. `source_ref` is a CLI-36 source selector. The gate is
/// `learn`'s: `--dry-run` previews, and a closure that adds dependencies is
/// shown and confirmed (default no) unless `--yes`.
// spec: PROF-17 PROF-18
pub fn learn_source_profiles(
    paths: &Paths,
    source_ref: &str,
    selection: &crate::profile::Selection,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    let source_name = resolve_profile_source(paths, source_ref)?;
    let req = ProfileInstall {
        source_name: &source_name,
        selection,
        learn_patterns: &[],
        only_missing_from_snapshot: false,
        replace: false,
    };
    // spec: PROF-35
    print_profile_progress(paths, &source_name, selection);
    let plan = plan_profile_install(paths, &req)?;
    let out = crate::render::ctx();
    let result = |outcome: &'static str, installed: Vec<String>| {
        let mut r = MutationResult::new("learn", &source_name, outcome);
        r.installed = installed.iter().map(|k| strip_ansi(k)).collect();
        r.profiles = Some(plan.enabled.clone());
        r
    };
    if !out.json {
        print_profile_header(&source_name, &plan);
    }
    if plan.enabled.is_empty() {
        // `--all-profiles` on a source that declares none.
        if out.json {
            return print_json(&result("up-to-date", Vec::new()));
        }
        println!(
            "note: {} declares no profiles; nothing to do",
            strip_ansi(&source_name)
        );
        return Ok(());
    }
    if dry_run {
        if out.json {
            return print_json(&result("dry-run", plan.pending.clone()));
        }
        return print_profile_preview(paths, &plan);
    }
    if plan.pending.is_empty() {
        apply_profile_plan(paths, &source_name, &plan, flow, out.json)?;
        if out.json {
            return print_json(&result("up-to-date", Vec::new()));
        }
        println!("already installed; nothing to do");
        return Ok(());
    }
    // spec: DEP-31 -- the dependency-closure confirmation, as for any learn.
    if !plan.trees.is_empty() && !flow.yes {
        if out.json {
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!(
                        "installing the dependency closure of profile(s) {} of '{}'",
                        plan.enabled.join(", "),
                        strip_ansi(&source_name)
                    ),
                    true,
                ),
            });
        }
        for tree in &plan.trees {
            print!("{tree}");
        }
        if !confirm("install this dependency closure?")? {
            println!("cancelled; nothing installed");
            return Ok(());
        }
    }
    let installed = apply_profile_plan(
        paths,
        &source_name,
        &plan,
        InstallFlow { yes: true, ..flow },
        out.json,
    )?;
    if out.json {
        return print_json(&result("installed", installed));
    }
    Ok(())
}

/// The selection a re-meld applies (PROF-16, PROF-40), or `None` to keep the
/// CLI-12 install-all path, paired with whether applying it replaces the
/// recorded set. A profile flag, a profile-tracked source, or a source
/// declaring `default` routes through profiles. On a tracked source, profile
/// flags replace the recorded set, computed as a first meld computes it
/// (`default` unless `--no-default-profile`, plus `--profile`); with no flag
/// the recorded set is re-enabled as it is, `default` only when recorded, and
/// recorded names the source no longer declares are left to `upgrade`
/// (PROF-24).
pub(super) fn remeld_profile_selection(
    paths: &Paths,
    source_name: &str,
    flags: Option<&crate::profile::Selection>,
) -> Result<Option<(crate::profile::Selection, bool)>> {
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(None);
    };
    let table = source_profile_table(paths, source)?;
    let tracked = source.is_profile_tracked();
    let has_default = table
        .as_ref()
        .is_some_and(|t| t.contains_key(crate::profile::DEFAULT));
    if flags.is_none() && !tracked && !has_default {
        return Ok(None);
    }
    // spec: PROF-40 -- profile flags on a tracked source name its whole set.
    if let Some(sel) = flags
        && tracked
    {
        return Ok(Some((sel.clone(), true)));
    }
    let mut sel = flags.cloned().unwrap_or(crate::profile::Selection {
        include_default: true,
        named: Vec::new(),
        all: false,
    });
    if tracked {
        sel.include_default = false;
        for p in source.profiles.iter().flatten() {
            let declared = table.as_ref().is_some_and(|t| t.contains_key(p));
            if declared && !sel.named.contains(p) {
                sel.named.push(p.clone());
            }
        }
    }
    Ok(Some((sel, false)))
}

/// The curator-entry form of the profile flags (PROF-26), or `None` to keep
/// the DSC-58/62 install path: an entry naming `profiles`, or an
/// `install = true` entry whose nested source declares `default` (or that
/// opts out with `default-profile = false`). A profile name the nested source
/// does not declare is `BadReference` naming the curator.
pub(super) fn curated_profile_selection(
    paths: &Paths,
    registry: &Registry,
    entry: &crate::mindfile::NestedSource,
    nested_name: &str,
    curator: &str,
) -> Result<Option<crate::profile::Selection>> {
    let Some(source) = registry.find(nested_name) else {
        return Ok(None);
    };
    Ok(curated_profile_route(paths, source, entry, curator)?.map(|(sel, _)| sel))
}

/// The routing rule behind [`curated_profile_selection`], for a registered
/// `source` (PROF-26, PROF-27): `None` keeps the DSC-58/62 path; otherwise the
/// entry's selection and the enabled set it resolves to. Shared with `curate`
/// so meld and curate route an entry identically. An undeclared profile name
/// is `BadReference` naming the curator.
pub(crate) fn curated_profile_route(
    paths: &Paths,
    source: &crate::source::Source,
    entry: &crate::mindfile::NestedSource,
    curator: &str,
) -> Result<Option<(crate::profile::Selection, Vec<String>)>> {
    let table = source_profile_table(paths, source)?;
    let Some(sel) = curated_route_selection(table.as_ref(), entry) else {
        return Ok(None);
    };
    // spec: PROF-26 PROF-27 -- an undeclared name is BadReference.
    let enabled =
        crate::profile::enabled_set_for_curator(table.as_ref(), &sel, &source.name, curator)?;
    Ok(Some((sel, enabled)))
}

/// The PROF-26 routing rule against the nested source's `table`: the entry's
/// selection when it routes through profiles (it names `profiles`, or it is an
/// `install = true` entry whose source declares `default` or that opts out
/// with `default-profile = false`), else `None`.
fn curated_route_selection(
    table: Option<&crate::profile::ProfileTable>,
    entry: &crate::mindfile::NestedSource,
) -> Option<crate::profile::Selection> {
    let names_profiles = entry.profiles.as_ref().is_some_and(|p| !p.is_empty());
    let has_default = table.is_some_and(|t| t.contains_key(crate::profile::DEFAULT));
    let via_install = entry.install && (has_default || entry.default_profile == Some(false));
    (names_profiles || via_install).then(|| crate::profile::Selection::from_nested(entry))
}

/// The profile install a meld is followed by, so the source's install hooks
/// see the selection it applies (HOOK-142).
pub(crate) enum MeldProfiles<'a> {
    /// No profile install follows (`--register-only`, absorb, a policy
    /// auto-meld, a marketplace entry, a sync re-walk).
    None,
    /// The consumer's own meld: the flags, plus the PROF-13 picker when they
    /// are absent and the source declares no `default`. `picked` receives the
    /// selection when the meld routes through profiles (PROF-9, PROF-12,
    /// PROF-13), so the caller does not ask twice.
    Consumer {
        selection: crate::profile::Selection,
        flags_given: bool,
        learning: bool,
        yes: bool,
        picked: Option<crate::profile::Selection>,
    },
    /// A curator's `[discover].sources` entry (PROF-26).
    Curated(&'a crate::mindfile::NestedSource),
}

impl MeldProfiles<'_> {
    /// Resolve the source hook environment for `source_name` against its
    /// `table` (HOOK-142), running the PROF-13 picker for a consumer meld.
    pub(super) fn hook_env(
        &mut self,
        source_name: &str,
        table: Option<&crate::profile::ProfileTable>,
    ) -> Result<crate::hook::HookEnv> {
        let selection = match self {
            MeldProfiles::None => None,
            MeldProfiles::Consumer {
                selection,
                flags_given,
                learning,
                yes,
                picked,
            } => {
                let routes =
                    *flags_given || table.is_some_and(|t| t.contains_key(crate::profile::DEFAULT));
                *picked = if routes {
                    Some(selection.clone())
                } else {
                    pick_profiles_from(source_name, table, *learning || *yes)?
                };
                picked.clone()
            }
            MeldProfiles::Curated(entry) => curated_route_selection(table, entry),
        };
        Ok(meld_hook_env(table, selection.as_ref(), None))
    }
}

/// A curator entry's `install-items` that name a real item of the nested
/// source, as bare `kind:name` patterns, so they union with the entry's
/// profiles (PROF-26) under the same silent-skip rule DSC-62 applies.
pub(super) fn curated_install_item_patterns(
    paths: &Paths,
    registry: &Registry,
    nested_name: &str,
    refs: Option<&Vec<String>>,
) -> Result<Vec<String>> {
    let Some(refs) = refs.filter(|r| !r.is_empty()) else {
        return Ok(Vec::new());
    };
    let Some(source) = registry.find(nested_name) else {
        return Ok(Vec::new());
    };
    let items = catalog::scan(paths, &single(source))?;
    Ok(select_by_bare_refs(&items, refs)
        .into_iter()
        .map(|it| format!("{}:{}", it.kind.as_str(), it.name))
        .collect())
}

/// A by-name `learn <item>` marks an installed item explicit (PROF-20), so
/// disabling its profiles later keeps it. A glob or `--all` selection is not
/// by name. Resolution misses are ignored: `learn` itself already reported.
pub fn mark_learned_explicit(paths: &Paths, item_ref: &str) -> Result<()> {
    // spec: PROF-20
    let Ok(parsed) = parse_item_ref(item_ref) else {
        return Ok(());
    };
    if is_glob(&parsed.name) {
        return Ok(());
    }
    // Nothing carries a profile reason: nothing to mark, so skip the scan.
    if Manifest::load(paths)?
        .items
        .values()
        .all(|i| i.profiles.is_empty())
    {
        return Ok(());
    }
    let registry = Registry::load(paths)?;
    let Ok(items) = catalog::scan(paths, &registry) else {
        return Ok(());
    };
    let Ok(item) = resolve(&items, &parsed, registry.sources.len()) else {
        return Ok(());
    };
    let mut manifest = Manifest::load(paths)?;
    if let Some(entry) = manifest.items.get_mut(item.key().as_str()) {
        let was = entry.explicit;
        crate::profile::mark_explicit(entry);
        if entry.explicit != was {
            manifest.save(paths)?;
        }
    }
    Ok(())
}

/// A source's declared profiles for display (PROF-29), or `None` when it
/// declares none (or its `mind.toml` cannot be read: display never fails on
/// it).
fn display_profile_table(
    paths: &Paths,
    source: &crate::source::Source,
) -> Option<crate::profile::ProfileTable> {
    source_profile_table(paths, source)
        .ok()
        .flatten()
        .filter(|t| !t.is_empty())
}

/// The declared profiles as one line, each enabled one marked (PROF-29):
/// `basic, default (enabled), full`.
fn profile_summary(table: &crate::profile::ProfileTable, enabled: &[String]) -> String {
    table
        .keys()
        .map(|p| {
            if enabled.contains(p) {
                format!("{p} (enabled)")
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print the `profiles:` block of `recall --sources` / `probe --no-tui`
/// (PROF-29): one line per source that declares profiles. Nothing at all
/// when none does.
pub(super) fn print_profile_block(paths: &Paths, sources: &[&crate::source::Source]) {
    let lines: Vec<String> = sources
        .iter()
        .filter_map(|s| {
            let table = display_profile_table(paths, s)?;
            let enabled = s.profiles.clone().unwrap_or_default();
            Some(format!(
                "  {}: {}",
                strip_ansi(&s.name),
                profile_summary(&table, &enabled)
            ))
        })
        .collect();
    if lines.is_empty() {
        return;
    }
    println!("profiles:");
    for l in lines {
        println!("{l}");
    }
}

/// One profile of a `recall --profiles` listing (PROF-31, PROF-32): either its
/// expansion (`items`, `installed`, `total`) or the `error` expanding it gave.
#[derive(Serialize, Debug)]
struct ProfileListing {
    pub(super) name: String,
    pub(super) enabled: bool,
    pub(super) members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) items: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) installed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

/// One source of a `recall --profiles` listing (PROF-31, PROF-32).
#[derive(Serialize, Debug)]
struct SourceProfiles {
    pub(super) source: String,
    pub(super) profiles: Vec<ProfileListing>,
}

/// The PROF-31 listing of `source`'s declared profiles, or `None` when it
/// declares none. Each profile is expanded on its own against the source's
/// current catalog; an expansion error (or a scan failure) is reported on the
/// row instead of failing the listing.
fn source_profile_listing(
    paths: &Paths,
    source: &crate::source::Source,
    manifest: &Manifest,
) -> Option<SourceProfiles> {
    let table = display_profile_table(paths, source)?;
    let enabled = source.profiles.clone().unwrap_or_default();
    let items = scan_one(paths, source);
    let profiles = table
        .iter()
        .map(|(name, members)| {
            let mut row = ProfileListing {
                name: name.clone(),
                enabled: enabled.contains(name),
                members: members.iter().map(|m| strip_ansi(m)).collect(),
                items: None,
                installed: None,
                total: None,
                error: None,
            };
            let expansion = items.as_ref().map_err(|e| e.to_string()).and_then(|items| {
                crate::profile::expand(&table, std::slice::from_ref(name), items, &source.name)
                    .map(|x| (x, items))
                    .map_err(|e| e.to_string())
            });
            match expansion {
                Ok((x, items)) => {
                    let keys: Vec<String> = x
                        .keys
                        .keys()
                        .filter_map(|bare| {
                            items
                                .iter()
                                .find(|it| format!("{}:{}", it.kind.as_str(), it.name) == *bare)
                                .map(|it| it.key().as_str().to_string())
                        })
                        .collect();
                    let installed = keys
                        .iter()
                        .filter(|k| {
                            manifest
                                .items
                                .get(*k)
                                .is_some_and(|m| m.source == source.name)
                        })
                        .count();
                    row.total = Some(keys.len());
                    row.installed = Some(installed);
                    row.items = Some(keys.iter().map(|k| strip_ansi(k)).collect());
                }
                Err(e) => row.error = Some(strip_ansi(&e)),
            }
            row
        })
        .collect();
    Some(SourceProfiles {
        source: source.name.clone(),
        profiles,
    })
}

/// `recall --profiles [<source>]` (PROF-31, PROF-32), and `learn <source>
/// --profile` with no value (PROF-33): each listed source's declared profiles
/// with their members, enabled state, and installed count, then the `learn`
/// command that enables each profile that is not enabled.
// spec: PROF-31 PROF-32 PROF-33
pub fn list_profiles(paths: &Paths, source_ref: Option<&str>, json: bool) -> Result<()> {
    let registry = Registry::load(paths)?;
    let selected: Vec<&crate::source::Source> = match source_ref {
        Some(r) => {
            let name = resolve_profile_source(paths, r)?;
            registry.sources.iter().filter(|s| s.name == name).collect()
        }
        None => registry.sources.iter().collect(),
    };
    let manifest = Manifest::load(paths)?;
    let listings: Vec<SourceProfiles> = selected
        .iter()
        .filter_map(|s| source_profile_listing(paths, s, &manifest))
        .collect();
    if json {
        return print_json_envelope(&listings);
    }
    if listings.is_empty() {
        match selected.as_slice() {
            [only] if source_ref.is_some() => {
                println!("{} declares no profiles", strip_ansi(&only.name))
            }
            _ => println!("no melded source declares profiles"),
        }
        return Ok(());
    }
    let out = crate::render::ctx();
    let width = crate::render::term_width();
    for (i, listing) in listings.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("profiles of {}:", strip_ansi(&listing.source));
        let quoted = crate::error::shell_quote(&strip_ansi(&listing.source));
        let names: Vec<String> = listing
            .profiles
            .iter()
            .map(|p| {
                let name = strip_ansi(&p.name);
                if p.enabled {
                    format!("{name} (enabled)")
                } else {
                    name
                }
            })
            .collect();
        let name_width = names
            .iter()
            .map(|n| crate::render::visible_width(n))
            .max()
            .unwrap_or(0);
        for (p, name) in listing.profiles.iter().zip(&names) {
            print_profile_listing_block(&out, p, name, name_width, &quoted, width);
        }
    }
    Ok(())
}

/// One profile of the PROF-36 listing: a heading line (name, enabled marker,
/// installed count), then its members and any expansion error wrapped to
/// `width` under a hanging indent, then the `learn` command that enables it,
/// kept on one line so it copies whole.
fn print_profile_listing_block(
    out: &crate::render::OutputCtx,
    p: &ProfileListing,
    name: &str,
    name_width: usize,
    quoted_source: &str,
    width: usize,
) {
    const INDENT: &str = "      ";
    const HANG: &str = "               ";
    let count = match (p.installed, p.total) {
        (Some(n), Some(m)) => format!("{n}/{m} installed"),
        _ => String::new(),
    };
    let pad = name_width.saturating_sub(crate::render::visible_width(name));
    let heading = format!(
        "  {} {name}{}  {}",
        out.bullet(),
        " ".repeat(pad),
        out.dim(&count)
    );
    println!("{}", heading.trim_end());

    let mut members: Vec<String> = p.members.clone();
    if members.is_empty() {
        members.push("(none)".into());
    }
    let last = members.len() - 1;
    for m in &mut members[..last] {
        m.push(',');
    }
    let first = format!("{INDENT}members: ");
    for line in crate::render::wrap_tokens(&members, &first, HANG, width) {
        println!("{line}");
    }
    if let Some(e) = &p.error {
        let words: Vec<&str> = e.split_whitespace().collect();
        let first = format!("{INDENT}error:   ");
        for line in crate::render::wrap_tokens(&words, &first, HANG, width) {
            println!("{}", out.red(&line));
        }
    }
    if !p.enabled {
        println!(
            "{INDENT}enable:  mind learn {quoted_source} --profile {}",
            strip_ansi(&p.name)
        );
    }
}

/// Insert PROF-29's `profiles` (profile name -> declared members, members
/// sanitized) and `enabled_profiles` into a source's `--json` object. A source
/// that declares no profiles and has none enabled is left unchanged; the
/// registry's own `profiles` field (the enabled set) is replaced either way so
/// the key has one meaning in the document.
pub(super) fn insert_profile_json(
    paths: &Paths,
    source: &crate::source::Source,
    obj: &mut serde_json::Map<String, serde_json::Value>,
) {
    obj.remove("profiles");
    let table = display_profile_table(paths, source);
    let enabled = source.profiles.clone().unwrap_or_default();
    if table.is_none() && enabled.is_empty() {
        return;
    }
    let declared: serde_json::Map<String, serde_json::Value> = table
        .unwrap_or_default()
        .into_iter()
        .map(|(name, members)| {
            (
                name,
                serde_json::Value::Array(
                    members
                        .iter()
                        .map(|m| serde_json::Value::String(strip_ansi(m)))
                        .collect(),
                ),
            )
        })
        .collect();
    obj.insert("profiles".into(), serde_json::Value::Object(declared));
    obj.insert(
        "enabled_profiles".into(),
        serde_json::Value::Array(enabled.into_iter().map(serde_json::Value::String).collect()),
    );
}
