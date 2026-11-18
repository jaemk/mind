//! Items new to a source since the last upgrade, for whole sources and profiles.

use super::*;

/// One profile addition an `upgrade` proposes (PROF-21), as the report and the
/// `--json` result's `profile_additions` carry it.
#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
pub(super) struct ProfileAddition {
    /// The effective `kind:name` key the item installs under.
    pub(super) key: String,
    pub(super) source: String,
    /// The enabled profiles the item is a member of.
    pub(super) profiles: Vec<String>,
}

/// One snapshot key that left its source's expansion (PROF-22), as the report
/// and the `--json` result's `profile_departures` carry it.
#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
pub(super) struct ProfileDeparture {
    /// The effective `kind:name` key when the item is installed or still
    /// offered, else the bare snapshot key.
    pub(super) key: String,
    pub(super) source: String,
    /// The profile(s) it left: the installed item's recorded reasons, else the
    /// source's enabled set before this pass.
    pub(super) profiles: Vec<String>,
    pub(super) installed: bool,
    /// `mind forget <key>`, when the item is installed for profiles only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) forget: Option<String>,
}

/// What the upgrade pass's profile tracking found (PROF-21..24): the
/// fresh additions still to install under the gate, per source, and what was
/// reported and already persisted.
#[derive(Default)]
pub(super) struct ProfileTracking {
    /// Per source: the plan of its fresh (not installed) additions.
    pub(super) plans: Vec<(String, ProfilePlan)>,
    pub(super) additions: Vec<ProfileAddition>,
    /// Closure items the additions pull in beyond the members themselves.
    extra_closure: usize,
    pub(super) departures: Vec<ProfileDeparture>,
    /// Fresh additions recorded as seen without installing (`--ignore-new`).
    pub(super) ignored: Vec<NewItem>,
    /// Fresh additions left out of this run (`--skip-new`).
    pub(super) skipped: usize,
}

impl ProfileTracking {
    pub(super) fn has_additions(&self) -> bool {
        !self.plans.is_empty()
    }
}

/// The installed entry of `source_name` whose stable identity is the bare
/// snapshot key `bare` (`kind:name`), if any.
fn installed_by_bare_key<'a>(
    manifest: &'a Manifest,
    source_name: &str,
    bare: &str,
) -> Option<&'a crate::manifest::InstalledItem> {
    let (kind_word, name) = bare.split_once(':')?;
    let kind = ItemKind::parse(kind_word)?;
    manifest
        .items
        .values()
        .find(|m| m.source == source_name && m.kind == kind && m.bare_name == name)
}

/// The upgrade pass's profile tracking (PROF-5, PROF-21..24), run for each
/// profile-tracked source in `sources` before the gate.
///
/// For each source: an enabled profile the source no longer declares is
/// dropped from the recorded set with a warning (PROF-24); the remaining set
/// is expanded against `catalog` (an exact-member miss is `BadReference`,
/// PROF-5) and diffed against the snapshot. A departed key is reported once
/// and removed from the snapshot, never uninstalled (PROF-22); both of those
/// are persisted now, whatever the gate later decides. An addition that is
/// already installed is snapshotted and tagged now; the rest are returned for
/// the caller to install under its gate, and stay out of the snapshot until
/// they do (PROF-21). A member the consumer forgot is still in the snapshot,
/// so it is not an addition (PROF-23).
///
/// `mode` (LIFE-55) decides what happens to the fresh additions: returned for
/// the gate, left out (`Skip`), or snapshotted without installing (`Ignore`).
/// A departure whose manifest key is in `leftovers` is pruned by the pass
/// (PROF-39), so its line says so instead of naming `mind forget`.
// spec: PROF-5 PROF-21 PROF-22 PROF-23 PROF-24 LIFE-55
pub(super) fn track_profiles_for_upgrade(
    paths: &Paths,
    sources: &[String],
    catalog: &[CatalogItem],
    mode: NewItems,
    leftovers: &[String],
    preview: bool,
) -> Result<ProfileTracking> {
    let mut tracking = ProfileTracking::default();
    for name in sources {
        let mut registry = Registry::load(paths)?;
        let Some(source) = registry.find(name) else {
            continue;
        };
        if !source.is_profile_tracked() {
            continue;
        }
        let table = source_profile_table(paths, source)?.unwrap_or_default();
        let recorded: Vec<String> = source.profiles.clone().unwrap_or_default();
        let (kept, dropped): (Vec<String>, Vec<String>) = recorded
            .iter()
            .cloned()
            .partition(|p| table.contains_key(p));
        let safe_source = strip_ansi(name);
        for p in &dropped {
            // spec: PROF-24
            let action = if preview {
                "would be removed from"
            } else {
                "removed from"
            };
            crate::render::warn(format!(
                "profile '{}' is no longer declared by {safe_source}; {action} its enabled profiles",
                strip_ansi(p)
            ));
        }
        let expansion = crate::profile::expand(&table, &kept, catalog, name)?;
        let snapshot: Vec<String> = source.profile_snapshot.clone().unwrap_or_default();
        let diff = crate::profile::diff(&snapshot, &expansion);

        let manifest = Manifest::load(paths)?;
        for bare in &diff.departures {
            // spec: PROF-22 -- reported once, then dropped from the snapshot.
            let entry = installed_by_bare_key(&manifest, name, bare);
            let key = match entry {
                Some(e) => e.key().as_str().to_string(),
                None => catalog
                    .iter()
                    .find(|c| {
                        c.source == *name && format!("{}:{}", c.kind.as_str(), c.name) == *bare
                    })
                    .map(|c| c.key().as_str().to_string())
                    .unwrap_or_else(|| bare.clone()),
            };
            let safe_key = strip_ansi(&key);
            let profiles = match entry {
                Some(e) if !e.profiles.is_empty() => e.profiles.clone(),
                _ => recorded.clone(),
            };
            // spec: PROF-39 -- a leftover of a pruning source goes with this pass.
            let to_prune = entry.is_some_and(|e| leftovers.iter().any(|k| *k == e.key().as_str()));
            let forget = entry
                .filter(|e| !e.is_explicit() && !to_prune)
                .map(|_| format!("mind forget {}", crate::error::shell_quote(&safe_key)));
            let mut line = format!(
                "{safe_key} left profile {} of {safe_source}",
                profiles.join(", ")
            );
            if to_prune {
                line.push_str("; to prune (the source declares `prune`)");
            } else if entry.is_some() {
                line.push_str("; it stays installed");
            }
            if let Some(cmd) = &forget {
                line.push_str(&format!("; run `{cmd}` to remove it"));
            }
            crate::render::note(format!("note: {line}"));
            tracking.departures.push(ProfileDeparture {
                key: safe_key,
                source: safe_source.clone(),
                profiles,
                installed: entry.is_some(),
                forget,
            });
        }
        if !dropped.is_empty() || !diff.departures.is_empty() {
            if let Some(s) = registry.sources.iter_mut().find(|s| s.name == *name) {
                s.profiles = Some(kept.clone());
                s.profile_snapshot = Some(
                    snapshot
                        .iter()
                        .filter(|k| !diff.departures.contains(k))
                        .cloned()
                        .collect(),
                );
            }
            if !preview {
                registry.save(paths)?;
            }
        }

        if diff.additions.is_empty() || kept.is_empty() {
            continue;
        }
        let selection = crate::profile::Selection {
            include_default: false,
            named: kept.clone(),
            all: false,
        };
        let req = ProfileInstall {
            source_name: name,
            selection: &selection,
            learn_patterns: &[],
            only_missing_from_snapshot: true,
            replace: false,
        };
        let plan = plan_profile_install(paths, &req)?;
        let ProfilePlan {
            declared,
            enabled,
            members,
            reasons,
            pending,
            trees,
            expansion,
            replace,
        } = plan;
        let (present, fresh): (Vec<ProfileMember>, Vec<ProfileMember>) =
            members.into_iter().partition(|m| m.installed);
        if !present.is_empty() && !preview {
            // An addition already installed is snapshotted and tagged, not
            // proposed (PROF-21).
            let now = ProfilePlan {
                declared: declared.clone(),
                enabled: enabled.clone(),
                members: present,
                reasons: std::collections::BTreeMap::new(),
                pending: Vec::new(),
                trees: Vec::new(),
                expansion: expansion.clone(),
                replace,
            };
            apply_profile_plan(
                paths,
                name,
                &now,
                InstallFlow {
                    yes: true,
                    clobber: Clobber::Error,
                    accept: crate::hook::Accept::default(),
                },
                true,
            )?;
        }
        if fresh.is_empty() {
            continue;
        }
        match mode {
            NewItems::Install => {}
            NewItems::Skip => {
                tracking.skipped += fresh.len();
                continue;
            }
            NewItems::Ignore => {
                let keys: Vec<String> = fresh.iter().filter_map(|m| m.bare_key.clone()).collect();
                let mut registry = Registry::load(paths)?;
                if let Some(s) = registry.sources.iter_mut().find(|s| s.name == *name) {
                    crate::profile::record_applied(s, &[], &keys);
                    if !preview {
                        registry.save(paths)?;
                    }
                }
                tracking.ignored.extend(fresh.iter().map(|m| NewItem {
                    key: strip_ansi(&m.target.key),
                    source: safe_source.clone(),
                }));
                continue;
            }
        }
        for m in &fresh {
            tracking.additions.push(ProfileAddition {
                key: strip_ansi(&m.target.key),
                source: safe_source.clone(),
                profiles: m.profiles.iter().cloned().collect(),
            });
        }
        tracking.extra_closure += pending.len().saturating_sub(fresh.len());
        tracking.plans.push((
            name.clone(),
            ProfilePlan {
                declared,
                enabled,
                members: fresh,
                reasons,
                pending,
                trees,
                expansion,
                replace,
            },
        ));
    }
    Ok(tracking)
}

/// A catalog item's bare `kind:name` key, the form the profile and whole
/// snapshots record.
pub(super) fn bare_key(item: &CatalogItem) -> String {
    format!("{}:{}", item.kind.as_str(), item.name)
}

/// Mark whole-tracked (LIFE-53) every source that is not yet tracked (by
/// profile or whole) and whose catalog items are all installed, recording
/// their bare keys. `catalog` is the full catalog when the caller has one;
/// `None` scans each candidate source, skipping one that fails to scan.
// spec: LIFE-53
pub(super) fn record_whole_sources(paths: &Paths, catalog: Option<&[CatalogItem]>) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<(&str, ItemKind, &str)> = manifest
        .items
        .values()
        .map(|m| (m.source.as_str(), m.kind, m.bare_name.as_str()))
        .collect();
    let mut registry = Registry::load(paths)?;
    let mut changed = false;
    for source in &mut registry.sources {
        if source.is_profile_tracked()
            || source.whole_snapshot.is_some()
            || !installed.iter().any(|(s, _, _)| *s == source.name)
        {
            continue;
        }
        let scanned: Vec<CatalogItem>;
        let items: Vec<&CatalogItem> = match catalog {
            Some(c) => c.iter().filter(|c| c.source == source.name).collect(),
            None => {
                // Quiet: the verb's own scan reports a source that fails.
                let Ok(v) = catalog::scan_one_strict(paths, source) else {
                    continue;
                };
                scanned = v;
                scanned.iter().collect()
            }
        };
        if items.is_empty()
            || !items
                .iter()
                .all(|c| installed.contains(&(c.source.as_str(), c.kind, c.name.as_str())))
        {
            continue;
        }
        let keys: BTreeSet<String> = items.iter().map(|c| bare_key(c)).collect();
        source.whole_snapshot = Some(keys.into_iter().collect());
        changed = true;
    }
    if changed {
        registry.save(paths)?;
    }
    Ok(())
}

/// One item new to a whole-tracked source (LIFE-54), as the upgrade report
/// and the `--json` result's `new_items` / `ignored_new_items` carry it.
#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
pub(super) struct NewItem {
    /// The effective `kind:name` key the item installs under.
    pub(super) key: String,
    pub(super) source: String,
}

/// What the upgrade pass's whole-source tracking found (LIFE-54, LIFE-55).
#[derive(Default)]
pub(super) struct WholeTracking {
    /// Per source: the fresh additions still to install under the gate.
    pub(super) targets: Vec<(String, Vec<LearnTarget>)>,
    pub(super) additions: Vec<NewItem>,
    /// Recorded as seen without installing (`--ignore-new`).
    pub(super) ignored: Vec<NewItem>,
    /// Left out of this run (`--skip-new`).
    pub(super) skipped: usize,
}

/// The upgrade pass's whole-source tracking (LIFE-54, LIFE-55), run for each
/// whole-tracked source in `sources` before the gate.
///
/// A catalog key not in the snapshot is new. One already installed (learned by
/// hand) joins the snapshot now; the rest are returned to install under the
/// gate, recorded as seen (`Ignore`), or left alone (`Skip`). A snapshot key no
/// longer in the catalog is dropped. Snapshot changes are persisted now,
/// whatever the gate later decides; a fresh addition joins the snapshot only
/// once it installs.
// spec: LIFE-54 LIFE-55
pub(super) fn track_whole_for_upgrade(
    paths: &Paths,
    sources: &[String],
    catalog: &[CatalogItem],
    mode: NewItems,
    preview: bool,
) -> Result<WholeTracking> {
    let mut tracking = WholeTracking::default();
    if sources.is_empty() {
        return Ok(tracking);
    }
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<(&str, ItemKind, &str)> = manifest
        .items
        .values()
        .map(|m| (m.source.as_str(), m.kind, m.bare_name.as_str()))
        .collect();
    let mut registry = Registry::load(paths)?;
    let mut changed = false;
    for name in sources {
        let Some(source) = registry.sources.iter_mut().find(|s| s.name == *name) else {
            continue;
        };
        let Some(snapshot) = source.whole_snapshot.clone() else {
            continue;
        };
        // Every item forgotten: the consumer no longer installs this source.
        if !installed.iter().any(|(s, _, _)| *s == name.as_str()) {
            source.whole_snapshot = None;
            changed = true;
            continue;
        }
        let items: Vec<&CatalogItem> = catalog.iter().filter(|c| c.source == *name).collect();
        let current: HashSet<String> = items.iter().map(|c| bare_key(c)).collect();
        let mut next: BTreeSet<String> = snapshot
            .iter()
            .filter(|k| current.contains(*k))
            .cloned()
            .collect();
        let safe_source = strip_ansi(name);
        let mut fresh: Vec<LearnTarget> = Vec::new();
        for c in items {
            let bare = bare_key(c);
            if next.contains(&bare) {
                continue;
            }
            let is_installed = installed.contains(&(c.source.as_str(), c.kind, c.name.as_str()));
            let item = NewItem {
                key: c.display_key(),
                source: safe_source.clone(),
            };
            match (is_installed, mode) {
                (true, _) => {
                    next.insert(bare);
                }
                (false, NewItems::Install) => {
                    fresh.push(LearnTarget {
                        source: name.clone(),
                        key: c.key().as_str().to_string(),
                    });
                    tracking.additions.push(item);
                }
                (false, NewItems::Ignore) => {
                    next.insert(bare);
                    tracking.ignored.push(item);
                }
                (false, NewItems::Skip) => tracking.skipped += 1,
            }
        }
        let next: Vec<String> = next.into_iter().collect();
        if next != snapshot {
            source.whole_snapshot = Some(next);
            changed = true;
        }
        if !fresh.is_empty() {
            tracking.targets.push((name.clone(), fresh));
        }
    }
    if changed && !preview {
        registry.save(paths)?;
    }
    Ok(tracking)
}

/// Install the whole-tracked additions (LIFE-54) through the learn path, so
/// their dependency closure comes with them, and add each one that installed
/// to its source's snapshot. A failure part-way still records what installed.
pub(super) fn apply_whole_additions(
    paths: &Paths,
    tracking: &WholeTracking,
    flow: InstallFlow,
    collect: bool,
) -> Result<Vec<String>> {
    let mut installed_keys = Vec::new();
    let mut failure = None;
    for (source_name, targets) in &tracking.targets {
        let before: HashSet<String> = Manifest::load(paths)?.items.keys().cloned().collect();
        let result = if collect {
            learn_collecting_selected(paths, Selection::ExactMany(targets), flow).map(drop)
        } else {
            learn_selected(paths, Selection::ExactMany(targets), false, flow)
        };
        let manifest = Manifest::load(paths)?;
        installed_keys.extend(
            manifest
                .items
                .keys()
                .filter(|k| !before.contains(*k))
                .cloned(),
        );
        let landed: Vec<String> = targets
            .iter()
            .filter_map(|t| manifest.items.get(&t.key))
            .map(|m| format!("{}:{}", m.kind.as_str(), m.bare_name))
            .collect();
        let mut registry = Registry::load(paths)?;
        if let Some(source) = registry.sources.iter_mut().find(|s| s.name == *source_name) {
            let mut snap: BTreeSet<String> = source
                .whole_snapshot
                .take()
                .unwrap_or_default()
                .into_iter()
                .collect();
            snap.extend(landed);
            source.whole_snapshot = Some(snap.into_iter().collect());
            registry.save(paths)?;
        }
        if let Err(e) = result {
            failure = Some(e);
            break;
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(installed_keys),
    }
}

/// The new-items sections of the upgrade report (LIFE-54, LIFE-55).
pub(super) fn print_whole_additions(tracking: &WholeTracking) {
    let out = crate::render::ctx();
    if !tracking.additions.is_empty() {
        let n = tracking.additions.len();
        println!(
            "{n} new {} in sources installed whole:\n",
            if n == 1 { "item" } else { "items" }
        );
        for a in &tracking.additions {
            println!(
                "  {} {} {}",
                out.warn(),
                a.key,
                out.dim(&format!("[{}]", a.source))
            );
        }
        println!();
    }
}

/// The notes for new items an upgrade did not propose (LIFE-55): skipped ones
/// come back next time, ignored ones do not.
pub(super) fn note_unproposed_new_items(skipped: usize, ignored: &[NewItem], preview: bool) {
    if skipped > 0 {
        let action = if preview { "would skip" } else { "skipped" };
        crate::render::note(format!(
            "note: {action} {skipped} new item(s) (--skip-new); the next upgrade offers them again"
        ));
    }
    for i in ignored {
        let action = if preview { "would ignore" } else { "ignored" };
        // Source-qualified, so it cannot be ambiguous with a same-named item
        // in another melded source.
        crate::render::note(format!(
            "note: {action} new item {} from {} (--ignore-new); run `mind learn {}` to install it",
            i.key,
            i.source,
            crate::error::shell_quote(&format!("{}#{}", i.source, i.key))
        ));
    }
}

/// The profile-additions section of the upgrade report (PROF-21).
pub(super) fn print_profile_additions(tracking: &ProfileTracking) {
    if tracking.additions.is_empty() {
        return;
    }
    let out = crate::render::ctx();
    let n = tracking.additions.len();
    println!(
        "{n} {} newly in an enabled profile:\n",
        if n == 1 { "item is" } else { "items are" }
    );
    for a in &tracking.additions {
        println!(
            "  {} {} {}  (profile {})",
            out.warn(),
            a.key,
            out.dim(&format!("[{}]", a.source)),
            a.profiles.join(", ")
        );
    }
    if tracking.extra_closure > 0 {
        println!(
            "  plus {} dependency item(s) they require",
            tracking.extra_closure
        );
    }
    println!();
}
