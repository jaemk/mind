//! Source and item hook runs for meld, learn, and forget, and their hook environments.

use super::*;

/// Whether a recorded hook has already run at `current` (a real commit) FOR
/// THIS EVENT.
///
/// The event is part of the key (HOOK-124): a source that declares the same
/// command for both `install` and `update` records two independent runs, so
/// running the install hook must not make the update hook look already-run
/// (and vice versa).
// spec: HOOK-124
pub(super) fn hook_ran_at(
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
pub(super) fn record_install_hook(
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
pub(super) fn record_source_hook_entry(
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
pub(super) enum HookOutcome {
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
///
/// `hook_env` is the profile environment the hooks run with: the selection the
/// meld is about to apply (HOOK-142).
///
/// `defer` records the update-hook baselines but offers no install hook: the
/// caller has stashed the call to make after the unified plan is answered
/// (HOOK-183), via [`run_deferred_meld_hooks`].
#[allow(clippy::too_many_arguments)]
pub(super) fn run_install_hooks(
    source: &mut crate::source::Source,
    clone_dir: &std::path::Path,
    mindfile: &Option<MindToml>,
    toml_path: &std::path::Path,
    install_override: Option<&str>,
    accept: crate::hook::Accept,
    force_rerun: bool,
    extra_hooks: Vec<crate::mindfile::ResolvedHook>,
    hook_env: &crate::hook::HookEnv,
    defer: bool,
) -> Result<HookOutcome> {
    // DSC-61, HOOK-127: curator-supplied hooks (when applied) run after the
    // source's own, through the exact same override/disclosure/decide/run path
    // below. They are remembered as curated so a later `upgrade` can still
    // offer them: they live in the PARENT's manifest, so the clone can never be
    // asked what it declares.
    let curated_keys: HashSet<(String, &'static str)> = extra_hooks
        .iter()
        .map(|h| (h.run.clone(), h.event.as_str()))
        .collect();
    let (hooks, replaced) = meld_hook_set(mindfile, toml_path, install_override, extra_hooks)?;
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

    for h in hooks
        .iter()
        .filter(|h| h.event == HookEvent::Install && !defer)
    {
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

        // spec: HOOK-160 HOOK-168 -- the override's record names what it
        // replaced, so `hooks list` finds it under the declared hooks.
        let replaced_here: &[String] = match &replaced {
            Some(cmds) if override_cmd == Some(h.run.as_str()) => cmds,
            _ => &[],
        };
        let site = crate::hook_log::Site {
            item: None,
            commit: current.as_deref(),
            replaced: replaced_here,
        };
        let key = crate::consent::HookKey::source_hook(&name, h.event.as_str(), &h.run);
        match crate::hook::decide(&key, &disclosure, h.optional, accept.event(h.event))? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook. A progress line, not an
                // aside: stdout in text mode, and unreachable from `--json`
                // stdout because the run's fd 1 points at stderr (main.rs's
                // `json_stdout`). spec: CLI-217
                println!("running install hook '{}' for {}", h.label(), name);
                // HOOK-53: a non-zero exit (optional or required) is a hard stop.
                crate::hook::run_hook(
                    &h.run,
                    clone_dir,
                    &name,
                    "install",
                    h.label(),
                    hook_env,
                    site,
                )?;
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
                crate::hook_log::not_run(&name, site, "install", h.label(), &h.run);
                record_install_hook(source, &h.run, RecordedEvent::Install, None);
                stamp_origin(source, h, RecordedEvent::Install, origin_of(h));
            }
            crate::hook::HookAct::Abort => {
                crate::hook_log::aborted(&name, site, "install", h.label(), &h.run);
                return Ok(HookOutcome::Abort);
            }
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

    // spec: HOOK-190 -- a curator's items hooks are recorded on the source, the
    // only place a later command can read them from (HOOK-127). They run at
    // the end of the command, with the installed set (HOOK-191).
    let curated_items: Vec<crate::mindfile::ResolvedHook> = hooks
        .iter()
        .filter(|h| h.event == HookEvent::Items && origin_of(h) == Some(HookOrigin::Curated))
        .cloned()
        .collect();
    crate::items_hook::record_curated(source, &curated_items);

    Ok(HookOutcome::Proceed)
}

/// The hooks a meld has in effect for a source (HOOK-50, HOOK-56, DSC-61): the
/// manifest's declared hooks, then any curator-supplied ones, with a consumer
/// `--install-hook` override applied. Returns the set and, when the override
/// replaced anything, the declared commands it replaced.
fn meld_hook_set(
    mindfile: &Option<MindToml>,
    toml_path: &std::path::Path,
    install_override: Option<&str>,
    extra_hooks: Vec<crate::mindfile::ResolvedHook>,
) -> Result<(Vec<crate::mindfile::ResolvedHook>, Option<Vec<String>>)> {
    let mut resolved = mindfile
        .as_ref()
        .map(|m| m.resolved_hooks(toml_path))
        .transpose()?
        .unwrap_or_default();
    resolved.extend(extra_hooks);
    Ok(crate::hook::apply_install_override(
        resolved,
        install_override,
    ))
}

/// A top-level meld's source install hooks, held back until the unified plan
/// is answered (HOOK-183): everything [`run_install_hooks`] needs to offer
/// them later.
pub(super) struct DeferredMeldHooks {
    pub(super) source: String,
    pub(super) clone_dir: std::path::PathBuf,
    pub(super) install_override: Option<String>,
    pub(super) accept: crate::hook::Accept,
    pub(super) force_rerun: bool,
    pub(super) extra_hooks: Vec<crate::mindfile::ResolvedHook>,
    pub(super) hook_env: crate::hook::HookEnv,
    /// A fresh meld registered the source only so the plan could be shown: an
    /// abort or a failed hook unregisters it again (HOOK-21, HOOK-53). A
    /// re-meld leaves the source melded.
    pub(super) fresh: bool,
}

thread_local! {
    /// Set by `meld_verb` for an interactive text-mode meld: the top-level
    /// source's install hooks are deferred to the plan (HOOK-183).
    static DEFER_MELD_HOOKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static DEFERRED_MELD_HOOKS: std::cell::RefCell<Option<DeferredMeldHooks>> =
        const { std::cell::RefCell::new(None) };
    /// Set when the deferred hooks were aborted, so `meld_verb` stops there.
    static MELD_ABORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The deferral state of one `meld` (HOOK-183), cleared again when dropped.
pub(super) struct MeldPlanScope {
    pub(super) _answers: crate::consent::Scope,
}

impl MeldPlanScope {
    /// Start a meld: defer the top-level install hooks when `interactive`.
    pub(super) fn enter(interactive: bool) -> MeldPlanScope {
        DEFER_MELD_HOOKS.with(|d| d.set(interactive));
        DEFERRED_MELD_HOOKS.with(|d| d.borrow_mut().take());
        MELD_ABORTED.with(|a| a.set(false));
        MeldPlanScope {
            _answers: crate::consent::scope(),
        }
    }
}

impl Drop for MeldPlanScope {
    fn drop(&mut self) {
        DEFER_MELD_HOOKS.with(|d| d.set(false));
        DEFERRED_MELD_HOOKS.with(|d| d.borrow_mut().take());
    }
}

/// Whether the deferred meld hooks were aborted (or failed) in this meld, so
/// the rest of the verb stops.
pub(super) fn meld_aborted() -> bool {
    MELD_ABORTED.with(std::cell::Cell::get)
}

/// Whether this meld's top-level install hooks wait for the plan: only for an
/// interactive meld whose install hooks would be prompted (HOOK-182: accepted
/// hooks are not deferred).
pub(super) fn defers_meld_hooks(accept: crate::hook::Accept) -> bool {
    DEFER_MELD_HOOKS.with(std::cell::Cell::get) && !accept.install
}

/// The plan entries for the deferred meld hooks (HOOK-180), in run order. Empty
/// when nothing is deferred.
pub(super) fn deferred_meld_plan(paths: &Paths) -> Result<Vec<crate::consent::PlannedHook>> {
    let Some((name, dir, install_override, force_rerun, extra)) = DEFERRED_MELD_HOOKS.with(|d| {
        d.borrow().as_ref().map(|d| {
            (
                d.source.clone(),
                d.clone_dir.clone(),
                d.install_override.clone(),
                d.force_rerun,
                d.extra_hooks.clone(),
            )
        })
    }) else {
        return Ok(Vec::new());
    };
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(&name) else {
        return Ok(Vec::new());
    };
    let mindfile = MindToml::load(&dir).unwrap_or_default();
    let (hooks, replaced) = meld_hook_set(
        &mindfile,
        &dir.join("mind.toml"),
        install_override.as_deref(),
        extra,
    )?;
    let override_cmd = install_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let commit = source.commit.clone().unwrap_or_default();
    Ok(hooks
        .iter()
        .filter(|h| h.event == HookEvent::Install)
        .filter(|h| {
            force_rerun
                || !hook_ran_at(
                    source,
                    &h.run,
                    RecordedEvent::Install,
                    source.commit.as_deref(),
                )
        })
        .map(|h| crate::consent::PlannedHook {
            key: crate::consent::HookKey::source_hook(&name, h.event.as_str(), &h.run),
            label: h.label().to_string(),
            optional: h.optional,
            commit: commit.clone(),
            run_dir: dir.display().to_string(),
            inspect: None,
            browse_url: source.browse_url(&commit),
            replaced: replaced
                .as_ref()
                .filter(|_| override_cmd == Some(h.run.as_str()))
                .map(|cmds| cmds.join("; ")),
            accepted: false,
        })
        .collect())
}

/// Offer the deferred meld hooks now, under whatever answer the plan recorded
/// (HOOK-183). A fresh meld whose hook is aborted or fails is unregistered, as
/// it would have been had the hook run before registration (HOOK-21, HOOK-53).
pub(super) fn run_deferred_meld_hooks(paths: &Paths) -> Result<HookOutcome> {
    let Some(d) = DEFERRED_MELD_HOOKS.with(|c| c.borrow_mut().take()) else {
        return Ok(HookOutcome::Proceed);
    };
    let mut registry = Registry::load(paths)?;
    let Some(idx) = registry.sources.iter().position(|s| s.name == d.source) else {
        return Ok(HookOutcome::Proceed);
    };
    let mindfile = MindToml::load(&d.clone_dir).unwrap_or_default();
    let outcome = run_install_hooks(
        &mut registry.sources[idx],
        &d.clone_dir,
        &mindfile,
        &d.clone_dir.join("mind.toml"),
        d.install_override.as_deref(),
        d.accept,
        d.force_rerun,
        d.extra_hooks,
        &d.hook_env,
        false,
    );
    let drop_source = |registry: &mut Registry| -> Result<()> {
        let source = registry.sources.remove(idx);
        // A linked working tree is the user's own; never remove it (CLI-27).
        if !source.is_linked() {
            let _ = std::fs::remove_dir_all(&d.clone_dir);
        }
        registry.save(paths)
    };
    match outcome {
        Ok(HookOutcome::Proceed) => {
            registry.save(paths)?;
            Ok(HookOutcome::Proceed)
        }
        Ok(HookOutcome::Abort) => {
            MELD_ABORTED.with(|a| a.set(true));
            if d.fresh {
                drop_source(&mut registry)?;
                println!("aborted; nothing installed");
            } else {
                registry.save(paths)?; // persist any hook that did run
                println!("aborted; source left in place");
            }
            Ok(HookOutcome::Abort)
        }
        Err(e) => {
            MELD_ABORTED.with(|a| a.set(true));
            if d.fresh {
                drop_source(&mut registry)?;
            } else {
                // spec: LIFE-48 -- keep any earlier hook's record.
                registry.save(paths)?;
            }
            Err(e)
        }
    }
}

/// Run the uninstall hooks declared by the source at `idx` in `registry`.
/// Extracted so both the `--unlink-only` and the default `unmeld` paths can call
/// it without duplicating the logic.
///
/// Returns `Ok(true)` if all hooks were handled (run or skipped) and the caller
/// should proceed with the unmeld. Returns `Ok(false)` if the user chose Abort
/// (the source should be left in place). Returns `Err` on a hook failure
/// (HOOK-53), which also leaves the source in place.
pub(super) fn run_uninstall_hooks(
    paths: &Paths,
    registry: &Registry,
    idx: usize,
    source_name: &str,
    uninstall_hook: Option<&str>,
    accept: crate::hook::Accept,
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
    let replaced_note = replaced.as_ref().map(|cmds| cmds.join("; "));

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

        let replaced_here: &[String] = match (&replaced, override_cmd) {
            (Some(cmds), Some(cmd)) if h.run == cmd => cmds,
            _ => &[],
        };
        let site = crate::hook_log::Site {
            item: None,
            commit: Some(commit.as_str()),
            replaced: replaced_here,
        };
        let key = crate::consent::HookKey::source_hook(source_name, h.event.as_str(), &h.run);
        match crate::hook::decide(&key, &disclosure, h.optional, accept.event(h.event))? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook. Same as the install-hook
                // line above. spec: CLI-217
                println!("running uninstall hook '{}' for {}", h.label(), source_name);
                // HOOK-53: any failure (optional or required) is a hard stop;
                // the unmeld stops and the source remains.
                // spec: HOOK-141 -- an unmeld sees the recorded set.
                let hook_env = source_hook_env(&registry.sources[idx]);
                crate::hook::run_hook(
                    &h.run,
                    &clone_dir,
                    source_name,
                    "uninstall",
                    h.label(),
                    &hook_env,
                    site,
                )?;
            }
            crate::hook::HookAct::Skip => {
                // spec: CLI-217 -- same for `unmeld --json`.
                crate::render::note(format!(
                    "note: skipped uninstall hook '{}' for {}",
                    h.label(),
                    source_name
                ));
                crate::hook_log::not_run(source_name, site, "uninstall", h.label(), &h.run);
            }
            crate::hook::HookAct::Abort => {
                crate::hook_log::aborted(source_name, site, "uninstall", h.label(), &h.run);
                println!("aborted; source left in place");
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// The plan entries for a source's uninstall hooks (HOOK-186): what
/// [`run_uninstall_hooks`] would offer, with the same `--uninstall-hook`
/// override applied (HOOK-59).
pub(super) fn source_uninstall_plan(
    paths: &Paths,
    source: &crate::source::Source,
    uninstall_hook: Option<&str>,
    accept: crate::hook::Accept,
) -> Result<Vec<crate::consent::PlannedHook>> {
    let clone_dir = source.clone_dir(paths);
    let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
    let resolved = mindfile
        .as_ref()
        .map(|m| m.resolved_hooks(&clone_dir.join("mind.toml")))
        .transpose()?
        .unwrap_or_default();
    let (resolved, replaced) =
        crate::hook::apply_hook_override(resolved, uninstall_hook, HookEvent::Uninstall);
    let override_cmd = uninstall_hook.map(str::trim).filter(|s| !s.is_empty());
    let commit = source.commit.clone().unwrap_or_default();
    Ok(resolved
        .iter()
        .filter(|h| h.event == HookEvent::Uninstall)
        .map(|h| crate::consent::PlannedHook {
            key: crate::consent::HookKey::source_hook(&source.name, "uninstall", &h.run),
            label: h.label().to_string(),
            optional: h.optional,
            commit: commit.clone(),
            run_dir: clone_dir.display().to_string(),
            inspect: None,
            browse_url: source.browse_url(&commit),
            replaced: replaced
                .as_ref()
                .filter(|_| override_cmd == Some(h.run.as_str()))
                .map(|cmds| cmds.join("; ")),
            accepted: accept.uninstall,
        })
        .collect())
}

/// The profile environment a meld's source install hooks run with (HOOK-142):
/// `recorded` plus the set `selection` resolves to against `table`. A name the
/// table does not declare is left out here; the profile install raises it
/// afterwards (PROF-14). `None` for `selection` means no profile install
/// follows, so only the recorded set applies.
pub(super) fn meld_hook_env(
    table: Option<&crate::profile::ProfileTable>,
    selection: Option<&crate::profile::Selection>,
    recorded: Option<&Vec<String>>,
) -> crate::hook::HookEnv {
    let mut profiles: BTreeSet<String> = recorded.into_iter().flatten().cloned().collect();
    if let Some(sel) = selection {
        let mut sel = sel.clone();
        sel.named
            .retain(|n| table.is_some_and(|t| t.contains_key(n)));
        profiles.extend(crate::profile::enabled_set(table, &sel, "").unwrap_or_default());
    }
    crate::hook::HookEnv {
        profiles: profiles.into_iter().collect(),
        item_profiles: Vec::new(),
        items_file: None,
    }
}

/// The `recall --sources` token for a source's recorded hooks (HOOK-58,
/// HOOK-196): none for 0, ` hook` for 1, ` hooks(N)` beyond. Items hooks count
/// alongside the install and update records.
pub(super) fn hook_token(s: &crate::source::Source) -> String {
    match s.install_hooks.len() + s.items_hooks.len() {
        0 => String::new(),
        1 => " hook".to_string(),
        n => format!(" hooks({n})"),
    }
}

/// The profile environment a source hook of `source` runs with (HOOK-140,
/// HOOK-141): an in-flight profile change's enabled set, else the recorded one.
pub(crate) fn source_hook_env(source: &crate::source::Source) -> crate::hook::HookEnv {
    crate::hook::HookEnv {
        profiles: crate::hook::scoped_profiles(&source.name)
            .unwrap_or_else(|| source.profiles.clone().unwrap_or_default()),
        item_profiles: Vec::new(),
        items_file: None,
    }
}

/// The profile environment an item hook runs with (HOOK-140, HOOK-141): the
/// source's enabled set as [`source_hook_env`] reads it, and `item_profiles`,
/// the item's recorded reasons.
pub(crate) fn item_hook_env(
    paths: &Paths,
    source_name: &str,
    item_profiles: Vec<String>,
) -> Result<crate::hook::HookEnv> {
    let registry = Registry::load(paths)?;
    let mut env = registry
        .find(source_name)
        .map(source_hook_env)
        .unwrap_or_default();
    env.item_profiles = item_profiles;
    Ok(env)
}

/// [`item_hook_env`] for a catalog item about to be installed: its reasons are
/// those the in-flight profile install gives it plus what an existing install
/// of the same stable identity recorded, so an upgrade (in place or a rename)
/// keeps them (PROF-20).
pub(super) fn catalog_item_hook_env(
    paths: &Paths,
    item: &CatalogItem,
) -> Result<crate::hook::HookEnv> {
    let manifest = Manifest::load(paths)?;
    let mut reasons = crate::hook::scoped_item_reasons(&item.source, item.key().as_str());
    if let Some(prev) = manifest
        .items
        .values()
        .find(|m| m.source == item.source && m.kind == item.kind && m.bare_name == item.name)
    {
        reasons.extend(prev.profiles.iter().cloned());
    }
    item_hook_env(paths, &item.source, reasons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    // spec: HOOK-58
    // The recall --sources hook token is count-aware: no token for 0 hooks,
    // ` hook` for 1, ` hooks(N)` for N > 1.
    #[test]
    fn hook_token_is_count_aware() {
        // 0 hooks -> empty string.
        let s0 = hook_source(Some("abc"), &[]);
        assert_eq!(hook_token(&s0), "", "no hooks => empty token");

        // 1 hook -> ` hook`.
        let s1 = hook_source(Some("abc"), &[("make install", Some("abc"))]);
        assert_eq!(hook_token(&s1), " hook", "1 hook => ' hook'");

        // 2 hooks -> ` hooks(2)`.
        let s2 = hook_source(
            Some("abc"),
            &[("make build", Some("abc")), ("make install", Some("abc"))],
        );
        assert_eq!(hook_token(&s2), " hooks(2)", "2 hooks => ' hooks(2)'");

        // spec: HOOK-196 -- a recorded items hook counts too.
        let mut s3 = hook_source(Some("abc"), &[("make install", Some("abc"))]);
        s3.items_hooks.push(crate::source::RecordedItemsHook {
            command: "make registry".into(),
            applied: None,
            name: None,
            optional: false,
            origin: None,
        });
        assert_eq!(hook_token(&s3), " hooks(2)");
    }
}
