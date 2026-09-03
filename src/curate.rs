//! Implementation of `mind curate` (spec/curate.md, CUR-1..CUR-19).
//!
//! One pass over every registered curator: read what each declares now, compare
//! it against what is registered and installed, report the difference as a plan,
//! and (unless `--check`) offer to apply it.
//!
//! `curate` owns no lifecycle mechanics of its own. Every change it applies is
//! an ordinary verb's path -- the nested meld the DSC-57 re-walk uses, `learn`'s
//! install, `meld --pin`'s re-pin, the `upgrade` pass, `unmeld` -- so a curated
//! source ends in exactly the state the equivalent hand-run commands would
//! leave it in (CUR-3, CUR-5, CUR-6, CUR-7).

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::catalog;
use crate::commands::{self, InstallFlow, PinRequest, SkippedEntry};
use crate::config::Config;
use crate::error::{MindError, Result};
use crate::manifest::Manifest;
use crate::mindfile::{MindToml, NestedSource};
use crate::paths::Paths;
use crate::policy::Policy;
use crate::sanitize::strip_ansi;
use crate::source::{Pin, Registry, Source, parse_spec_quiet};

/// The flags `curate` reads, as one value (the CLI has six and they compose).
#[derive(Clone, Default)]
pub struct CurateFlags {
    /// Report the plan and apply nothing (CUR-9). Outranks `yes`.
    pub check: bool,
    /// Apply without the confirmation prompt (CUR-9).
    pub yes: bool,
    /// Also apply `unlist` changes, which uninstall and drop a source (CUR-7).
    pub prune: bool,
    /// Plan against the clones on disk instead of fetching first (CUR-2).
    pub no_sync: bool,
    /// CUR-16: stamp provenance on one unowned identity instead of planning.
    pub adopt: Option<String>,
    pub dangerously_skip_hook_check: bool,
    pub dangerously_skip_build_hook_check: bool,
}

/// One proposed change, in the shape `--json` emits (CUR-13). `action` carries
/// what applying it does and is not serialized. Every field that can carry
/// curator-controlled text is sanitized (`strip_ansi`) at construction (CUR-18):
/// the plan is the consent surface for the single `[Y/n]` prompt, so a curated
/// repo must not be able to spoof what another line says.
#[derive(Serialize)]
struct Change {
    kind: &'static str,
    curator: String,
    source: String,
    detail: String,
    #[serde(skip)]
    action: Action,
}

/// What applying a change does. Each variant maps to one existing verb path.
enum Action {
    /// Register a listed-but-unregistered entry (CUR-3), then install per CUR-4.
    Register {
        curator: String,
        toml_path: std::path::PathBuf,
        entry: Box<NestedSource>,
    },
    /// Install the entry's declared items into an already-registered source
    /// (CUR-4). `refs` is the DSC-62 subset, or `None` for DSC-58 "all".
    Install {
        source: String,
        refs: Option<Vec<String>>,
    },
    /// Re-pin a curated source to the entry's directive (CUR-5).
    Repin { source: String, pin: Pin },
    /// Upgrade a curated source's out-of-date items (CUR-6).
    Upgrade { source: String },
    /// Uninstall and drop a source no curator lists (CUR-7). `--prune` only.
    Unlist { source: String },
    /// Reported, never applied (CUR-8, CUR-16).
    Advisory,
}

#[derive(Serialize)]
struct CurateResult {
    schema: u8,
    action: &'static str,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    changes: Vec<Change>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<SkippedEntry>,
}

/// Run `mind curate`.
pub fn run(paths: &Paths, flags: CurateFlags) -> Result<()> {
    paths.ensure_layout()?;

    // spec: CUR-16 -- `--adopt` is a distinct, narrow sub-mode: it stamps
    // provenance on one identity and does nothing else, so it never mutates
    // anything under an unattended `--yes` run that did not ask for it by name.
    if let Some(identity) = &flags.adopt {
        return run_adopt(paths, identity);
    }

    let out = crate::render::ctx();
    let mut registry = Registry::load(paths)?;

    // spec: CUR-19 -- scope the CUR-2 refresh to curators and curated sources,
    // not the whole registry: an unrelated directly-melded source has no
    // reason to be re-fetched (network calls, a rewritten `commit`) just
    // because the consumer also happens to run `curate`. Read from the clones
    // already on disk: membership (is this a curator? is that a curated
    // source?) changes slowly, so a pre-sync read is a fine basis for scoping
    // the sync that follows.
    if !flags.no_sync {
        let scope = sync_scope(paths, &registry);
        commands::sync_sources_for_upgrade_scoped(paths, &mut registry, None, Some(&scope), &out)?;
    }

    let (plan, mut skipped) = build_plan(paths, &registry)?;

    if plan.is_empty() {
        if out.json {
            return commands::print_json(&CurateResult {
                schema: 1,
                action: "curate",
                outcome: "clean",
                changes: Vec::new(),
                applied: Vec::new(),
                skipped,
            });
        }
        println!("{} curated sources are up to date", out.ok());
        report_skipped(&skipped);
        return Ok(());
    }

    report(&plan, flags.prune);

    // spec: CUR-9 -- `--check` reports and applies nothing, and outranks `--yes`.
    if flags.check {
        return finish(&plan, Vec::new(), skipped, flags);
    }
    if !should_apply(&plan, flags.yes)? {
        return finish(&plan, Vec::new(), skipped, flags);
    }

    let (applied, apply_skipped) = apply(paths, plan_in_apply_order(&plan), &flags)?;
    skipped.extend(apply_skipped);
    finish(&plan, applied, skipped, flags)
}

/// `mind curate --adopt <identity>` (CUR-16): stamp `curated_by` on a source
/// that is registered, unowned, and currently listed (unowned) by exactly the
/// named curator's entry -- the only state `--adopt` accepts, so a stale or
/// mistyped identity fails loudly rather than silently doing nothing.
fn run_adopt(paths: &Paths, identity: &str) -> Result<()> {
    let out = crate::render::ctx();
    let mut registry = Registry::load(paths)?;
    if registry.find(identity).is_none() {
        return Err(MindError::NotAnAdoptCandidate {
            name: identity.to_string(),
            reason: "no melded source has that identity".to_string(),
        });
    }
    if let Some(owner) = registry.find(identity).and_then(|s| s.curated_by.clone()) {
        return Err(MindError::NotAnAdoptCandidate {
            name: identity.to_string(),
            reason: format!("already curated by {owner}"),
        });
    }
    let (curators_list, _) = curators(paths, &registry)?;
    let claimant = curators_list.iter().find_map(|c| {
        entry_targets(c, identity).next().map(|_| c.source.name.clone())
    });
    let Some(curator_name) = claimant else {
        return Err(MindError::NotAnAdoptCandidate {
            name: identity.to_string(),
            reason: "no registered curator currently lists it".to_string(),
        });
    };
    if let Some(source) = registry.sources.iter_mut().find(|s| s.name == identity) {
        source.curated_by = Some(curator_name.clone());
    }
    registry.save(paths)?;
    if out.json {
        return commands::print_json(&serde_json::json!({
            "schema": 1,
            "action": "curate-adopt",
            "outcome": "applied",
            "source": identity,
            "curator": curator_name,
        }));
    }
    println!(
        "{} {} is now curated by {}",
        out.ok(),
        strip_ansi(identity),
        strip_ansi(&curator_name)
    );
    Ok(())
}

/// Every identity a curator's entries or marketplace catalog resolve `target`
/// to (there is at most one match; this returns 0 or 1 items so the caller can
/// use `.next()`), excluding a self-listed entry (CUR-17).
fn entry_targets<'a>(curator: &'a Curator, target: &'a str) -> impl Iterator<Item = ()> + 'a {
    let from_entries = curator.entries.iter().filter_map(move |entry| {
        let mut spec = parse_spec_quiet(&entry.source).ok()?;
        spec.apply_alias(entry.effective_alias());
        (spec.name == target && spec.name != curator.source.name).then_some(())
    });
    let from_market = curator
        .market
        .iter()
        .filter(move |spec| spec.name == target && spec.name != curator.source.name)
        .map(|_| ());
    from_entries.chain(from_market)
}

/// The sources the CUR-2 refresh should fetch (CUR-19): every source any
/// curator currently owns (STO-82, so its content is fresh for the CUR-4/5/6
/// comparisons), plus every source that CARRIES a `mind.toml` or marketplace
/// manifest file, whether or not it currently declares anything -- checked by
/// file presence, not by parsing (a source whose `[discover].sources` reads
/// empty right now is exactly the case that needs a fresh fetch to stop
/// reading empty). A source curated before this consumer's binary knew
/// `curated_by` existed reads back unowned (STO-82) and so falls outside the
/// "already owned" half of this scope until `--adopt` claims it, but is still
/// covered by the file-presence half if it is itself a curator.
///
/// The one gap this cannot close: a source that adds its very first
/// `mind.toml` (or marketplace manifest) and populates `[discover].sources` in
/// that same push is invisible to a file-presence check made before the
/// fetch. It surfaces on the next `curate` (or an explicit `mind sync`),
/// exactly like a plain `sync` needing a second run to notice a source that
/// only just started existing.
fn sync_scope(paths: &Paths, registry: &Registry) -> HashSet<String> {
    let mut scope = HashSet::new();
    for source in &registry.sources {
        // spec: LNK-8 -- an item-link instance curates nothing and owns nothing.
        if source.item_path.is_some() {
            continue;
        }
        if let Some(curator) = &source.curated_by {
            scope.insert(source.name.clone());
            scope.insert(curator.clone());
            continue;
        }
        let clone = source.clone_dir(paths);
        if clone.join("mind.toml").is_file()
            || crate::plugin_manifest::find_marketplace_manifest(&clone).is_some()
        {
            scope.insert(source.name.clone());
        }
    }
    scope
}

fn report_skipped(skipped: &[SkippedEntry]) {
    let out = crate::render::ctx();
    if out.json || skipped.is_empty() {
        return;
    }
    for s in skipped {
        println!("  {} {}", out.dim("skipped"), s.describe());
    }
}

/// Emit the result document (`--json`) or the closing text line.
fn finish(
    plan: &[Change],
    applied: Vec<String>,
    skipped: Vec<SkippedEntry>,
    flags: CurateFlags,
) -> Result<()> {
    let out = crate::render::ctx();
    if out.json {
        // spec: CUR-13 -- `changes` is always the whole plan, so a caller sees
        // what was proposed as well as what ran.
        let changes = plan
            .iter()
            .map(|c| Change {
                kind: c.kind,
                curator: c.curator.clone(),
                source: c.source.clone(),
                detail: c.detail.clone(),
                action: Action::Advisory,
            })
            .collect();
        return commands::print_json(&CurateResult {
            schema: 1,
            action: "curate",
            outcome: if applied.is_empty() {
                "pending"
            } else {
                "applied"
            },
            changes,
            applied,
            skipped,
        });
    }
    report_skipped(&skipped);
    let applicable = plan.iter().filter(|c| applies(c, flags.prune)).count();
    if applied.is_empty() && !flags.check && applicable > 0 {
        println!(
            "note: nothing applied; run `mind curate --yes` to apply {applicable} change(s)"
        );
    }
    Ok(())
}

/// Print the plan, one line per change (CUR-1).
fn report(plan: &[Change], prune: bool) {
    let out = crate::render::ctx();
    if out.json {
        return;
    }
    println!("{} curated changes pending:", out.bullet());
    for c in plan {
        println!(
            "  {:<10} {}  {}",
            out.dim(c.kind),
            strip_ansi(&c.source),
            c.detail
        );
    }
    // spec: CUR-7 / CUR-8 -- say plainly which listed changes an apply will not
    // touch, rather than letting a reported-but-never-applied line read as one
    // the run is about to handle.
    if plan.iter().any(|c| c.kind == "unlist") && !prune {
        println!(
            "note: `unlist` changes uninstall a source's items; pass --prune to apply them too"
        );
    }
    if plan.iter().any(|c| c.kind == "namespace") {
        println!("note: `namespace` changes are advisory; adopt one with the command shown above");
    }
    if plan.iter().any(|c| c.kind == "adopt") {
        println!(
            "note: `adopt` changes are advisory; run the `mind curate --adopt` command shown above to apply one"
        );
    }
}

/// The single confirmation gate (CUR-9).
fn should_apply(plan: &[Change], yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    // spec: CUR-9 -- json mode and a non-TTY run apply nothing without `--yes`;
    // there is no prompt to answer.
    if crate::render::ctx().json || !crate::hook::is_tty() {
        return Ok(false);
    }
    let applicable = plan.iter().filter(|c| applies(c, false)).count();
    if applicable == 0 {
        return Ok(false);
    }
    commands::confirm_default_yes(&format!("apply these {applicable} change(s) now?"))
}

/// Whether a change is one this run would apply, as opposed to one it only
/// reports (CUR-7's `unlist` without `--prune`, CUR-8/CUR-16's advisories).
fn applies(change: &Change, prune: bool) -> bool {
    match change.action {
        Action::Advisory => false,
        Action::Unlist { .. } => prune,
        _ => true,
    }
}

/// The plan in application order (CUR-10).
fn plan_in_apply_order(plan: &[Change]) -> Vec<&Change> {
    let rank = |kind: &str| match kind {
        "register" => 0,
        "install" => 1,
        "repin" => 2,
        "upgrade" => 3,
        "unlist" => 4,
        _ => 5,
    };
    let mut ordered: Vec<&Change> = plan.iter().collect();
    ordered.sort_by_key(|c| rank(c.kind));
    ordered
}

/// Apply the plan in order, returning the applied change ids and anything the
/// registration path skipped.
fn apply(
    paths: &Paths,
    ordered: Vec<&Change>,
    flags: &CurateFlags,
) -> Result<(Vec<String>, Vec<SkippedEntry>)> {
    let out = crate::render::ctx();
    let mut applied: Vec<String> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();
    let flow = InstallFlow {
        yes: true,
        clobber: commands::Clobber::Prompt,
        dangerously_skip: flags.dangerously_skip_hook_check,
        dangerously_skip_build: flags.dangerously_skip_build_hook_check,
    };
    // spec: CUR-6 -- the upgrade pass runs once over every source that needs it,
    // not once per source: one report, one hook re-run pass.
    let mut upgrade_scope: Vec<String> = Vec::new();

    for change in ordered {
        if !applies(change, flags.prune) {
            continue;
        }
        match &change.action {
            Action::Register {
                curator,
                toml_path,
                entry,
            } => {
                let policy = Policy::load()?;
                let prefer_ssh = Config::load(paths)?.ssh;
                let mut registry = Registry::load(paths)?;
                let added = commands::meld_curated_entry(
                    paths,
                    &mut registry,
                    curator,
                    toml_path,
                    entry,
                    policy.as_ref(),
                    prefer_ssh,
                    flags.dangerously_skip_hook_check,
                    &mut skipped,
                )?;
                registry.save(paths)?;
                if added == 0 {
                    continue;
                }
                // spec: CUR-3 -- and install what the entry declares, in the
                // same run. This is the half `sync`'s re-walk does not do.
                install_declared(paths, &change.source, entry, flow)?;
                applied.push(format!("register:{}", change.source));
            }
            Action::Install { source, refs } => {
                match refs {
                    Some(list) => commands::install_source_items_subset(paths, source, list, flow)?,
                    None => commands::install_source_items(paths, source, flow)?,
                }
                applied.push(format!("install:{source}"));
            }
            Action::Repin { source, pin } => {
                commands::repin_source(paths, source, PinRequest::Follow(pin.clone()))?;
                applied.push(format!("repin:{source}"));
            }
            Action::Upgrade { source } => {
                upgrade_scope.push(source.clone());
                applied.push(format!("upgrade:{source}"));
            }
            Action::Unlist { source } => {
                // spec: CUR-7 -- `--prune` only (guarded by `applies`), and the
                // ordinary uninstall path: hooks, links, and store copies all
                // go through `unmeld`.
                commands::unmeld(
                    paths,
                    source,
                    false,
                    true,
                    flags.dangerously_skip_hook_check,
                    None,
                )?;
                applied.push(format!("unlist:{source}"));
            }
            Action::Advisory => {}
        }
    }

    if !upgrade_scope.is_empty() {
        // Already fetched by CUR-2 (or deliberately not, under --no-sync), so
        // the pass never re-fetches here.
        commands::upgrade_sources_no_sync(
            paths,
            true,
            &upgrade_scope,
            flags.dangerously_skip_hook_check,
            flags.dangerously_skip_build_hook_check,
        )?;
    }
    if !out.json && !applied.is_empty() {
        println!("{} applied {} change(s)", out.ok(), applied.len());
    }
    Ok((applied, skipped))
}

/// Install what a curator entry declares for an already-registered source
/// (CUR-4): the DSC-62 subset when it names one, else everything when DSC-58
/// `install = true`, else nothing.
fn install_declared(
    paths: &Paths,
    source: &str,
    entry: &NestedSource,
    flow: InstallFlow,
) -> Result<()> {
    match declared_installs(entry) {
        Some(Some(refs)) => commands::install_source_items_subset(paths, source, &refs, flow),
        Some(None) => commands::install_source_items(paths, source, flow),
        None => Ok(()),
    }
}

/// What an entry declares for install: `Some(Some(refs))` for a DSC-62 subset,
/// `Some(None)` for DSC-58 "all", `None` for a register-only entry.
fn declared_installs(entry: &NestedSource) -> Option<Option<Vec<String>>> {
    match &entry.install_items {
        Some(refs) if !refs.is_empty() => Some(Some(refs.clone())),
        Some(_) => None, // an empty list is "offer nothing" (DSC-62)
        None if entry.install => Some(None),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// A registered curator and what it declares now (see [`curators`]).
struct Curator {
    /// The curator source itself.
    source: Source,
    /// Its `mind.toml` path, for the DSC-59 directives its entries carry.
    toml_path: std::path::PathBuf,
    /// Its `[discover].sources` entries (DSC-38).
    entries: Vec<NestedSource>,
    /// The sub-sources its marketplace catalog names (MKT-7), resolved to the
    /// identities they register under.
    market: Vec<Source>,
}

/// Every registered curator, paired with what it declares now: its
/// `[discover].sources` entries (DSC-38) and the sub-sources of its marketplace
/// catalog, if it ships one (MKT-7).
///
/// A source with neither is not a curator and contributes nothing to the plan.
/// A source that curates ONLY through a marketplace manifest is still a curator:
/// missing that is what would make its entries look unlisted (CUR-7).
///
/// spec: CUR-15 -- a source whose clone is missing, or whose `mind.toml` /
/// marketplace manifest fails to read, is NOT the same as a source with no
/// curator list: the former is reported in the second return value rather than
/// silently read as "curates nothing", which is what would make CUR-7 propose
/// `unlist` for everything it legitimately registered.
fn curators(paths: &Paths, registry: &Registry) -> Result<(Vec<Curator>, HashSet<String>)> {
    let mut out = Vec::new();
    let mut unreadable = HashSet::new();
    for source in &registry.sources {
        // spec: LNK-8 -- an item-link instance curates nothing.
        if source.item_path.is_some() {
            continue;
        }
        let clone = source.clone_dir(paths);
        if !clone.is_dir() {
            // A registered source's clone should exist; if it does not, we
            // cannot tell "never a curator" from "was one, unreadable now".
            // Treat as the latter so CUR-7 does not sweep what it owns.
            unreadable.insert(source.name.clone());
            continue;
        }
        let entries = match MindToml::load(&clone) {
            Ok(Some(m)) => m.discover.map(|d| d.sources).unwrap_or_default(),
            Ok(None) => Vec::new(),
            Err(_) => {
                unreadable.insert(source.name.clone());
                continue;
            }
        };
        // `marketplace_subsources` already applies the MKT-2 suppression (an
        // authoritative mind.toml wins) and each entry's MKT-8 alias.
        let market: Vec<Source> = match commands::marketplace_subsources(paths, source) {
            Ok(list) => list.into_iter().map(|(spec, _in_repo)| spec).collect(),
            Err(_) => {
                unreadable.insert(source.name.clone());
                continue;
            }
        };
        if entries.is_empty() && market.is_empty() {
            continue;
        }
        out.push(Curator {
            source: source.clone(),
            toml_path: clone.join("mind.toml"),
            entries,
            market,
        });
    }
    Ok((out, unreadable))
}

/// Build the plan (CUR-1). The second element is every per-source or
/// per-entry failure that CUR-15 kept from aborting the whole run.
fn build_plan(paths: &Paths, registry: &Registry) -> Result<(Vec<Change>, Vec<SkippedEntry>)> {
    let manifest = Manifest::load(paths)?;
    let mut plan: Vec<Change> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();
    // Every identity any curator lists, for the CUR-7 unlist pass.
    let mut listed: HashSet<String> = HashSet::new();
    // Curated sources reached this run, for the CUR-6 upgrade pass.
    let mut curated: Vec<String> = Vec::new();

    let (curators_list, unreadable) = curators(paths, registry)?;
    for name in &unreadable {
        skipped.push(SkippedEntry::new(name.clone(), "curator_unreadable"));
    }

    for Curator {
        source: curator,
        toml_path,
        entries,
        market,
    } in curators_list
    {
        for entry in entries {
            // spec: CLI-216 -- resolving an entry to the identity it registers
            // under is a question, not a decision to clone: quiet.
            let Ok(mut spec) = parse_spec_quiet(&entry.source) else {
                continue;
            };
            // spec: STO-58/DSC-78 -- an entry registers under its effective
            // alias, so compare against that identity.
            spec.apply_alias(entry.effective_alias());

            // spec: CUR-17 -- an entry naming the curator's OWN identity
            // contributes nothing, not even to `listed`: otherwise a source
            // could list itself and stay immune to CUR-7 unlisting forever,
            // even after its actual curator drops it.
            if spec.name == curator.name {
                continue;
            }
            listed.insert(spec.name.clone());

            let Some(registered) = registry.find(&spec.name) else {
                // spec: CUR-8 -- an entry whose identity is not registered but
                // whose REPO is, under this same curator with a different
                // alias, is a namespace change rather than a new entry. The
                // alias is part of identity (STO-58), so applying it would
                // register a second instance and install a second copy of every
                // item beside the first; report it and let the user decide.
                if let Some(existing) = registry.sources.iter().find(|s| {
                    s.base_identity() == spec.base_identity()
                        && s.item_path == spec.item_path
                        && s.curated_by.as_deref() == Some(curator.name.as_str())
                }) {
                    // Still listed, just under a different alias: not an unlist.
                    listed.insert(existing.name.clone());
                    curated.push(existing.name.clone());
                    plan.push(Change {
                        kind: "namespace",
                        curator: curator.name.clone(),
                        source: existing.name.clone(),
                        detail: format!(
                            "{} now declares namespace {}; adopt it with `mind unmeld {} --yes && mind meld {} {}--yes`",
                            strip_ansi(&curator.name),
                            entry
                                .effective_alias()
                                .map(|a| format!("'{}'", strip_ansi(&a)))
                                .unwrap_or_else(|| "none".to_string()),
                            crate::error::shell_quote(&strip_ansi(&existing.name)),
                            crate::error::shell_quote(&strip_ansi(&existing.url)),
                            entry
                                .effective_alias()
                                .map(|a| format!(
                                    "--namespace {} ",
                                    crate::error::shell_quote(&strip_ansi(&a))
                                ))
                                .unwrap_or_default(),
                        ),
                        action: Action::Advisory,
                    });
                    continue;
                }
                let detail = match declared_installs(&entry) {
                    Some(Some(refs)) => format!(
                        "listed by {}; register and install {}",
                        strip_ansi(&curator.name),
                        strip_ansi(&refs.join(", "))
                    ),
                    Some(None) => format!(
                        "listed by {}; register and install its items",
                        strip_ansi(&curator.name)
                    ),
                    None => format!("listed by {}; register", strip_ansi(&curator.name)),
                };
                plan.push(Change {
                    kind: "register",
                    curator: curator.name.clone(),
                    source: spec.name.clone(),
                    detail,
                    action: Action::Register {
                        curator: curator.name.clone(),
                        toml_path: toml_path.clone(),
                        entry: Box::new(entry),
                    },
                });
                continue;
            };
            // spec: CUR-12 -- an entry naming an identity already registered
            // by someone else (a direct meld, or a different curator) must not
            // let THIS entry mutate it: only the curator that actually owns it
            // (STO-82) may propose install/repin, and it joins the CUR-6 sweep
            // only through its owner. The `listed` insert above still stands
            // regardless, so any curator naming it still protects it from
            // CUR-7 unlisting.
            if registered.curated_by.as_deref() != Some(curator.name.as_str()) {
                // spec: CUR-16 -- an identity no one owns yet is an adopt
                // candidate: report it, but only `mind curate --adopt` (never
                // this run, even under --yes) may claim it.
                if registered.curated_by.is_none() {
                    plan.push(Change {
                        kind: "adopt",
                        curator: curator.name.clone(),
                        source: registered.name.clone(),
                        detail: format!(
                            "{} lists this source but does not own it; run `mind curate --adopt {}` to let it manage it",
                            strip_ansi(&curator.name),
                            crate::error::shell_quote(&strip_ansi(&registered.name))
                        ),
                        action: Action::Advisory,
                    });
                }
                continue;
            }
            curated.push(registered.name.clone());

            // spec: CUR-4 -- declared items that are not installed.
            if let Some(declared) = declared_installs(&entry) {
                match pending_installs(paths, &manifest, registered, &declared) {
                    Ok(pending) if !pending.is_empty() => {
                        plan.push(Change {
                            kind: "install",
                            curator: curator.name.clone(),
                            source: registered.name.clone(),
                            detail: format!(
                                "{} declares {} not installed: {}",
                                strip_ansi(&curator.name),
                                pending.len(),
                                strip_ansi(&pending.join(", "))
                            ),
                            action: Action::Install {
                                source: registered.name.clone(),
                                refs: declared,
                            },
                        });
                    }
                    Ok(_) => {}
                    // spec: CUR-15 -- one curated source's scan failing (a
                    // renamed upstream file, an oversized manifest) reports
                    // and skips rather than aborting the whole plan.
                    Err(e) => {
                        warn_skip(&registered.name, "could not check declared items", &e);
                        skipped.push(SkippedEntry::new(
                            registered.name.clone(),
                            "declared_items_check_failed",
                        ));
                    }
                }
            }

            // spec: CUR-5 -- the entry's pin directive against the recorded pin.
            match entry.pin_directive(&toml_path) {
                Ok(Some(pin)) if pin != registered.pin => {
                    plan.push(Change {
                        kind: "repin",
                        curator: curator.name.clone(),
                        source: registered.name.clone(),
                        detail: format!(
                            "{} declares {}; registered as {}",
                            strip_ansi(&curator.name),
                            strip_ansi(&commands::pin_description(&pin)),
                            strip_ansi(&commands::pin_description(&registered.pin))
                        ),
                        action: Action::Repin {
                            source: registered.name.clone(),
                            pin,
                        },
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    warn_skip(&registered.name, "could not read the pin directive", &e);
                    skipped.push(SkippedEntry::new(
                        registered.name.clone(),
                        "pin_directive_invalid",
                    ));
                }
            }
        }

        // spec: CUR-4 MKT-7 -- a marketplace catalog curates too. Its entries
        // declare no install directive of their own, so they propose no
        // install: what they contribute is membership. An external entry still
        // in the manifest is still listed (so it is not proposed for unlisting,
        // CUR-7), and a registered one joins the CUR-6 upgrade sweep, subject
        // to the same CUR-12 ownership check as any other entry. In-repo
        // plugins are items of the curator source itself (MKT-14), never
        // separately registered sources, so they never appear here.
        for spec in market {
            // spec: CUR-17 -- same self-listing exclusion as the entry loop.
            if spec.name == curator.name {
                continue;
            }
            listed.insert(spec.name.clone());
            if let Some(registered) = registry.find(&spec.name)
                && registered.curated_by.as_deref() == Some(curator.name.as_str())
            {
                curated.push(registered.name.clone());
            }
        }
    }

    // spec: CUR-6 -- curated sources whose installed items are out of date.
    for source in dedup(curated) {
        match outdated_count(paths, registry, &manifest, &source) {
            Ok(Some(count)) if count > 0 => {
                let curator = registry
                    .find(&source)
                    .and_then(|s| s.curated_by.clone())
                    .unwrap_or_default();
                plan.push(Change {
                    kind: "upgrade",
                    curator,
                    source: source.clone(),
                    detail: format!("{count} installed item(s) out of date"),
                    action: Action::Upgrade { source },
                });
            }
            Ok(_) => {}
            // spec: CUR-15 -- likewise for the CUR-6 drift check.
            Err(e) => {
                warn_skip(&source, "could not check for drift", &e);
                skipped.push(SkippedEntry::new(source.clone(), "drift_check_failed"));
            }
        }
    }

    // spec: CUR-7 -- a source whose recorded curator no longer lists it.
    for source in &registry.sources {
        let Some(curator) = source.curated_by.as_deref() else {
            continue; // no provenance: never proposed for unlisting
        };
        if listed.contains(&source.name) {
            continue;
        }
        // spec: CUR-15 -- a curator this run could not read contributes
        // nothing, including no `unlist`: an unreadable list must not read as
        // an empty one.
        if unreadable.contains(curator) {
            continue;
        }
        // A curator that is itself gone takes its entries' provenance with it:
        // report against the recorded name either way, so the line still says
        // where the source came from.
        plan.push(Change {
            kind: "unlist",
            curator: curator.to_string(),
            source: source.name.clone(),
            detail: format!(
                "no longer listed by {}; --prune uninstalls its items and drops it",
                strip_ansi(curator)
            ),
            action: Action::Unlist {
                source: source.name.clone(),
            },
        });
    }

    Ok((plan, skipped))
}

/// A per-source or per-entry failure CUR-15 kept from aborting the plan:
/// printed as a warning in text mode (mirroring `sync_sources_for_upgrade`'s
/// own per-source failure warning), with the full error detail. The `--json`
/// `skipped` slug carries no free text (CUR-13), so this is the only place a
/// human reads why.
fn warn_skip(source: &str, doing: &str, e: &MindError) {
    if !crate::render::ctx().json {
        eprintln!("  warning: {doing} for {}: {e}", strip_ansi(source));
    }
}

fn dedup(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

/// The effective names a curator entry declares that are not installed (CUR-4).
fn pending_installs(
    paths: &Paths,
    manifest: &Manifest,
    source: &Source,
    declared: &Option<Vec<String>>,
) -> Result<Vec<String>> {
    let items = catalog::scan(paths, &commands::single(source))?;
    // spec: DSC-62 -- a subset entry declares bare `kind:name` refs, resolved by
    // the same matcher the subset install uses, so the plan and the apply agree
    // on what the entry names.
    let selected: Vec<&catalog::CatalogItem> = match declared {
        Some(refs) => crate::resolve::select_by_bare_refs(&items, refs),
        None => items.iter().collect(),
    };
    let mut pending: Vec<String> = selected
        .into_iter()
        .filter(|it| !manifest.items.contains_key(it.key().as_str()))
        .map(|it| it.display_key())
        .collect();
    pending.sort();
    Ok(pending)
}

/// How many of a source's installed items are out of date, by the CLI-75 test
/// `recall` and `upgrade` share: the source content hash moved, or the
/// effective name did (a rename).
fn outdated_count(
    paths: &Paths,
    registry: &Registry,
    manifest: &Manifest,
    source_name: &str,
) -> Result<Option<usize>> {
    let Some(source) = registry.find(source_name) else {
        return Ok(None);
    };
    let items = catalog::scan(paths, &commands::single(source))?;
    let by_identity: HashMap<(crate::error::ItemKind, &str), &catalog::CatalogItem> = items
        .iter()
        .map(|it| ((it.kind, it.name.as_str()), it))
        .collect();
    let mut count = 0usize;
    for installed in manifest.items.values() {
        if installed.source != source_name {
            continue;
        }
        let Some(cat) = by_identity.get(&(installed.kind, installed.bare_name.as_str())) else {
            continue; // gone upstream: `upgrade` reports it, `curate` does not
        };
        // spec: CLI-75 -- a hash error counts as drift, erring toward flagging.
        let hash_lag = cat.content_hash().map_or(true, |h| h != installed.hash);
        let rename_lag = cat.effective_name() != installed.name;
        if hash_lag || rename_lag {
            count += 1;
        }
    }
    Ok(Some(count))
}
