//! `mind hooks run` and `mind hooks list` -- on-demand hook execution and
//! inspection, outside the meld/learn/forget/upgrade flows.
//!
//! spec: HOOK-100..105, CLI-194..196

use serde::Serialize;

use crate::catalog::CatalogItem;
use crate::cli::HookEventArg;
use crate::error::{MindError, Result};
use crate::manifest::{InstalledItem, Manifest};
use crate::mindfile::{HookEvent, MindToml, ResolvedHook};
use crate::paths::Paths;
use crate::resolve::{
    HookTarget, parse_hook_target, parse_item_ref, select_installed, source_matches_glob,
};
use crate::sanitize::strip_ansi;
use crate::source::{Pin, RecordedEvent, RecordedHook, RecordedSourceHook, Registry, Source};

/// [`crate::commands`]'s `print_json`, mirrored here so `hooks_cmd` stays
/// self-contained (the same duplication rationale as `pin_description` below):
/// route through `json_stdout::record` when stdout is reserved (CLI-217), or
/// print directly otherwise.
fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    if crate::json_stdout::is_reserved() {
        let s =
            serde_json::to_string_pretty(value).map_err(|e| MindError::json("json output", e))?;
        crate::json_stdout::record(s);
        return Ok(());
    }
    crate::render::print_json(value)
}

/// `mind hooks run --json`'s result document (CLI-222): the HOOK-107/HOOK-108
/// tally for the invocation, so a script can tell "nothing to do" from "ran N
/// hooks" without scraping stderr notes.
#[derive(Serialize, Debug, PartialEq, Eq)]
struct HooksRunResult {
    schema: u8,
    action: &'static str,
    target: String,
    event: &'static str,
    existed: usize,
    ran: usize,
    skipped: usize,
}

/// `mind hooks list --json`'s result document (CLI-220). Exactly one of
/// `sources`/`items` is populated, depending on whether `<target>` resolved
/// to a source or to an item ref (HOOK-104's two branches).
#[derive(Serialize, Debug, PartialEq)]
struct HooksListResult {
    schema: u8,
    action: &'static str,
    target: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sources: Vec<SourceHooksJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    items: Vec<ItemHooksJson>,
}

/// One matched source's hooks and installed items, for `hooks list --json`'s
/// `sources` array.
#[derive(Serialize, Debug, PartialEq)]
struct SourceHooksJson {
    source: String,
    hooks: Vec<SourceHookJson>,
    items: Vec<ItemHooksJson>,
}

/// One source-level hook entry (CLI-220).
#[derive(Serialize, Debug, PartialEq)]
struct SourceHookJson {
    event: &'static str,
    required: bool,
    command: String,
    /// The CLI-196 pending/last-ran status; present only for a recorded
    /// install- or update-event hook (HOOK-124; an uninstall hook carries no
    /// recorded run state).
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

/// One item's hooks -- either nested under a `SourceHooksJson` (its `source`
/// is the enclosing object, so omitted) or a top-level entry for an item-ref
/// target (its `source` is set). A list of objects, not bare strings, so a
/// later addition (e.g. an item-level install/uninstall disclosure record)
/// has somewhere to go.
#[derive(Serialize, Debug, PartialEq)]
struct ItemHooksJson {
    item: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    hooks: Vec<ItemHookJson>,
}

/// One item-level hook entry (CLI-220).
#[derive(Serialize, Debug, PartialEq)]
struct ItemHookJson {
    event: &'static str,
    required: bool,
    command: String,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// `mind hooks run <target>` -- run hooks on demand.
///
/// Reuses the same disclosure + consent + run machinery as the automatic flows,
/// so it is neither more nor less guarded (HOOK-100).
// spec: HOOK-100 HOOK-101 HOOK-102 HOOK-103 CLI-194 CLI-195 HOOK-105 CLI-222
pub fn run(
    paths: &Paths,
    target: &str,
    event: HookEventArg,
    force: bool,
    dangerously_skip_install: bool,
    dangerously_skip_build: bool,
) -> Result<()> {
    let tally = match resolve_hook_target(paths, target)? {
        HookTarget::Source(selector) => {
            // spec: HOOK-103 CLI-195 - --event build is invalid for a source target.
            if event == HookEventArg::Build {
                return Err(MindError::BuildEventRequiresItemTarget);
            }
            run_source_hooks(paths, &selector, event, force, dangerously_skip_install)?
        }
        HookTarget::Item(item_ref) => run_item_hooks(
            paths,
            &item_ref,
            target,
            event,
            force,
            dangerously_skip_install,
            dangerously_skip_build,
        )?,
    };

    // spec: CLI-222 -- a successful run answers `--json` with the HOOK-107/108
    // tally rather than nothing, so a script sees "ran 2, skipped 0" instead
    // of an empty stdout.
    if crate::render::ctx().json {
        emit_json(&HooksRunResult {
            schema: 1,
            action: "hooks-run",
            target: target.to_string(),
            event: event_label(event),
            existed: tally.existed,
            ran: tally.ran,
            skipped: tally.skipped_for_consent,
        })?;
    }
    Ok(())
}

/// `mind hooks list <target>` -- report declared hooks without running any.
///
/// For a source target, lists the source's hooks (with pending/last-ran info
/// for the recorded events, install and update) and the hooks of its installed
/// items. For an item ref, lists only that item's hooks. Every event is
/// reported: `hooks list` takes no `--event` filter (CLI-196, HOOK-126).
// spec: HOOK-104 CLI-196 HOOK-105 CLI-220 HOOK-126
pub fn list(paths: &Paths, target: &str) -> Result<()> {
    match resolve_hook_target(paths, target)? {
        HookTarget::Source(selector) => list_source_hooks(paths, target, &selector),
        HookTarget::Item(item_ref) => list_item_hooks(paths, target, &item_ref),
    }
}

/// Resolve a `hooks run`/`hooks list` target string, preferring an exact match
/// against a registered source identity over [`parse_hook_target`]'s `#`-split
/// heuristic (C11).
///
/// An item-link instance's own source identity carries a `#<path>` suffix
/// (LNK-4), so the exact identity string contains `#` and would otherwise
/// parse as an item ref that matches nothing (`NotInstalled`), leaving the
/// instance's source-level hooks unreachable by name. When `target` (trimmed)
/// equals some registered source's `name` exactly, it is read as a source
/// target regardless of the `#` it carries -- UNLESS that same string, read as
/// an ordinary `<source>#<item>` ref, also names an installed item (e.g. a
/// link instance whose skill sits at a single top-level path segment, so its
/// identity `host/owner/repo#foo` is spelled identically to item `foo` in
/// source `host/owner/repo`). That is a genuine ambiguity -- which reading is
/// meant depends on registry state the caller cannot see -- so it is reported
/// as [`MindError::AmbiguousHookTarget`] rather than silently picking one
/// side. Two escapes disambiguate explicitly and always take priority over
/// both the exact-match check and the ambiguity check:
/// - A leading `source:` prefix always forces the source reading (e.g.
///   `source:host/owner/repo#foo`), stripped before the rest of resolution.
/// - A kind-qualified item ref (`<source>#<kind>:<name>`, e.g.
///   `host/owner/repo#skill:foo`) never equals a registered source's exact
///   identity (identities carry no `kind:` segment), so it always falls
///   through to [`parse_hook_target`] and resolves as that item.
///
/// Any other string -- including a plain `source#item` that does not name a
/// registered source, and the triple-`#` `<link-identity>#<item>` form for an
/// item inside a link instance -- falls back to [`parse_hook_target`] unchanged.
// spec: HOOK-105
fn resolve_hook_target(paths: &Paths, target: &str) -> Result<HookTarget> {
    let trimmed = target.trim();

    // spec: HOOK-105 -- the explicit source-targeting escape: skip both the
    // exact-match and ambiguity checks entirely.
    if let Some(rest) = trimmed.strip_prefix("source:") {
        return Ok(HookTarget::Source(rest.trim().to_string()));
    }

    let registry = Registry::load(paths)?;
    let source_match = registry.sources.iter().any(|s| s.name == trimmed);

    // spec: HOOK-105 -- only a `#`-carrying string can collide with the
    // item-ref reading; a target with no `#` is unambiguously a source
    // selector under the CLI-194 heuristic and is never read as an item here.
    if source_match
        && trimmed.contains('#')
        && let Ok(item_ref) = parse_item_ref(trimmed)
    {
        let manifest = Manifest::load(paths)?;
        let matches = select_installed(&manifest.items, &item_ref);
        if !matches.is_empty() {
            // spec: DSC-95 -- `it.source`/`it.name` are attacker-controlled (a
            // source name or a `[[items]] name`); sanitize each field BEFORE
            // composing, never the composed string (an unterminated OSC in one
            // field would otherwise swallow the fields appended after it).
            let item_forms: Vec<String> = matches
                .iter()
                .map(|it| {
                    format!(
                        "{}#{}:{}",
                        strip_ansi(&it.source),
                        it.kind.as_str(),
                        strip_ansi(&it.name)
                    )
                })
                .collect();
            return Err(MindError::AmbiguousHookTarget {
                target: trimmed.to_string(),
                item_forms,
            });
        }
    }

    if source_match {
        return Ok(HookTarget::Source(trimmed.to_string()));
    }
    parse_hook_target(target)
}

// ---------------------------------------------------------------------------
// Source-level hook runner (HOOK-101)
// ---------------------------------------------------------------------------

/// Run a source's hooks for the given event (`install`, `update`, or
/// `uninstall`; `build` is an item-only event and is refused by the caller).
/// Returns the HOOK-107 tally on success (CLI-222).
// spec: HOOK-101 HOOK-126
fn run_source_hooks(
    paths: &Paths,
    selector: &str,
    event: HookEventArg,
    force: bool,
    dangerously_skip: bool,
) -> Result<HookTally> {
    let mut registry = Registry::load(paths)?;

    let indices: Vec<usize> = registry
        .sources
        .iter()
        .enumerate()
        .filter(|(_, s)| source_matches_glob(&s.name, selector))
        .map(|(i, _)| i)
        .collect();

    if indices.is_empty() {
        return Err(MindError::SourceNotFound {
            name: selector.to_string(),
        });
    }

    let mut registry_dirty = false;

    // spec: HOOK-107 -- count hooks that existed (were actually considered: not
    // already-up-to-date and not "no hooks declared"), how many of those ran,
    // and how many were skipped specifically for want of consent (a non-TTY
    // run, not an interactive decline). A `hooks run` that had work to do but
    // could not get consent for any of it is reported as an error rather than
    // a silent exit 0 (U43).
    let mut existed: usize = 0;
    let mut ran: usize = 0;
    let mut skipped_for_consent: usize = 0;
    // spec: HOOK-106 HOOK-107 -- every source that contributed at least one
    // considered hook, for the HooksNotRun remedy (paste-able only when this
    // holds exactly one entry; see `hooks_not_run_message`).
    let mut contributors: Vec<String> = Vec::new();

    for idx in indices {
        let source = &registry.sources[idx];
        let clone_dir = source.clone_dir(paths);
        let toml_path = clone_dir.join("mind.toml");

        let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
        let resolved = mindfile
            .as_ref()
            .map(|m| m.resolved_hooks(&toml_path))
            .transpose()?
            .unwrap_or_default();

        let hook_event = match event {
            HookEventArg::Install => HookEvent::Install,
            // spec: HOOK-126 -- an update run selects the update hooks and is
            // otherwise identical to an install run (pending filter, recording).
            HookEventArg::Update => HookEvent::Update,
            HookEventArg::Uninstall => HookEvent::Uninstall,
            HookEventArg::Build => unreachable!("caller guards build for source targets"),
        };
        let event_name = event_label(event);

        let hooks: Vec<&ResolvedHook> = resolved.iter().filter(|h| h.event == hook_event).collect();

        if hooks.is_empty() {
            // spec: CLI-217, DSC-95 -- `source.name` is source-influenced.
            crate::render::note(format!(
                "note: no {event_name} hooks declared for source {}",
                strip_ansi(&source.name)
            ));
            continue;
        }

        let source_name = source.name.clone();
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let current = source.commit.clone();
        let clone_path = clone_dir.display().to_string();
        let browse_url = source.browse_url(&commit);
        let mut source_contributed = false;
        // spec: HOOK-126 -- hooks the pending filter held back (each of which
        // really ran at this commit; a HOOK-121 baseline does not filter here),
        // so a run that considered nothing can say WHY instead of exiting 0 in
        // silence.
        let mut settled: Vec<String> = Vec::new();

        for h in &hooks {
            // spec: HOOK-101 -- for a recorded event (install or update), skip
            // hooks already run at the current commit unless --force overrides.
            if let Some(rec_event) = recorded_event(hook_event)
                && !force
                && hook_already_ran(
                    &registry.sources[idx],
                    &h.run,
                    rec_event,
                    current.as_deref(),
                )
            {
                settled.push(strip_ansi(h.label()));
                continue;
            }
            existed += 1;
            source_contributed = true;

            let disclosure = crate::hook::hook_disclosure_text(
                h.label(),
                h.event.as_str(),
                h.optional,
                &source_name,
                &pin_desc,
                &commit,
                &clone_path,
                &h.run,
                None,
                browse_url.as_deref(),
            );

            match crate::hook::decide(&disclosure, h.optional, dangerously_skip)? {
                crate::hook::HookAct::Run => {
                    ran += 1;
                    // Unlike the `note:`-prefixed lines around it this is a
                    // progress announcement, so it stays a `println!`: stdout in
                    // text mode, unreachable from `--json` stdout because the
                    // run's fd 1 points at stderr (main.rs's `json_stdout`).
                    // Same for install.rs's `running build hook for ...` and
                    // commands.rs's `running install hook '...' for ...`. See
                    // `tests/cli_hooks.rs::hooks_run_running_hook_note_is_one_document`.
                    // spec: CLI-217
                    println!(
                        "running {event_name} hook '{}' for {}",
                        h.label(),
                        source_name
                    );
                    // spec: HOOK-53 -- a non-zero exit is a hard stop; propagate
                    // the error after saving whatever was recorded so far.
                    if let Err(e) = crate::hook::run_hook(
                        &h.run,
                        &clone_dir,
                        &source_name,
                        event_name,
                        h.label(),
                    ) {
                        if registry_dirty {
                            let _ = registry.save(paths);
                        }
                        return Err(e);
                    }
                    // spec: HOOK-101 HOOK-124 -- record the run-commit for
                    // install and update hooks, under this event's key.
                    if let Some(rec_event) = recorded_event(hook_event) {
                        record_hook_run(
                            &mut registry.sources[idx],
                            &h.run,
                            rec_event,
                            current.clone(),
                        );
                        registry_dirty = true;
                    }
                }
                crate::hook::HookAct::Skip => {
                    // spec: HOOK-106 -- when the skip was for want of consent (a
                    // non-TTY run), name the cause and print the exact,
                    // copy-pasteable command to re-run it unattended, instead of
                    // a bare note that says nothing about why or what to do.
                    if !crate::hook::is_tty() {
                        skipped_for_consent += 1;
                        // The remedy re-selects the event that was run:
                        // `--event` defaults to `install`, so an uninstall
                        // skip whose remedy omitted it would silently suggest
                        // running different code.
                        // spec: CLI-217 HOOK-106
                        crate::render::note(source_skip_note(event_name, h.label(), &source_name));
                    } else {
                        // spec: CLI-217, DSC-95 -- the label and the identity are
                        // both source-controlled and `render::note` does not
                        // sanitize, so strip each field before composing.
                        crate::render::note(format!(
                            "note: skipped {event_name} hook '{}' for {}",
                            strip_ansi(h.label()),
                            strip_ansi(&source_name)
                        ));
                    }
                    // spec: HOOK-101 -- even a skipped install (or update)
                    // hook is recorded (with ran_at = None) so repeat runs know
                    // it was offered.
                    if let Some(rec_event) = recorded_event(hook_event) {
                        record_hook_run(&mut registry.sources[idx], &h.run, rec_event, None);
                        registry_dirty = true;
                    }
                }
                crate::hook::HookAct::Abort => {
                    // spec: HOOK-100 -- a required hook's abort is a non-zero exit.
                    if registry_dirty {
                        let _ = registry.save(paths);
                    }
                    return Err(MindError::HookAborted {
                        label: h.label().to_string(),
                    });
                }
            }
        }
        if source_contributed {
            contributors.push(source_name.clone());
        } else if !settled.is_empty() {
            // spec: HOOK-126 CLI-217 -- every declared hook was held back by the
            // pending filter. Without this the run exits 0 having done nothing
            // and said nothing, which reads exactly like "this source has no
            // hooks". Name the commit they are settled at and the way to run
            // them anyway.
            crate::render::note(nothing_pending_note(
                event_name,
                &settled,
                &source_name,
                current.as_deref(),
            ));
        }
    }

    if registry_dirty {
        registry.save(paths)?;
    }

    // spec: HOOK-107 -- there was work to do, and none of it could get consent
    // (every hook that existed was skipped for want of a terminal). A run with
    // nothing to do (no hooks declared, or every install hook already ran at
    // the current commit) stays exit 0; only this specific "had work, no
    // consent available" case is an error.
    if existed > 0 && ran == 0 && skipped_for_consent > 0 {
        return Err(MindError::HooksNotRun {
            target: selector.to_string(),
            event: event_label(event).to_string(),
            skipped: skipped_for_consent,
            resolved: contributors,
        });
    }

    Ok(HookTally {
        existed,
        ran,
        skipped_for_consent,
    })
}

// ---------------------------------------------------------------------------
// Item-level hook runner (HOOK-102 / HOOK-103)
// ---------------------------------------------------------------------------

/// The HOOK-107/HOOK-108 accounting for one `hooks run` invocation: how many
/// hooks for the selected event were actually considered, how many of those
/// ran, and how many were skipped specifically for want of consent (a non-TTY
/// run, not an interactive decline).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HookTally {
    existed: usize,
    ran: usize,
    skipped_for_consent: usize,
}

impl HookTally {
    /// Record `count` item hooks offered for the selected event, resolving how
    /// they will be decided (HOOK-108).
    ///
    /// Every item lifecycle hook goes through the same ladder as a source hook
    /// (`install.rs::run_item_hook`), mirrored here in the same order: the
    /// bypass flag runs it unattended (HOOK-83), a non-TTY run skips it
    /// outright (HOOK-22), and only a terminal prompts. So the outcome for the
    /// whole batch is known before the batch runs. An interactive run is the
    /// user's own decision per hook and is counted as neither a run nor a
    /// consent failure, which is what keeps an interactive decline at exit 0.
    fn offered(&mut self, count: usize, dangerously_skip: bool) {
        self.existed += count;
        if dangerously_skip {
            self.ran += count;
        } else if !crate::hook::is_tty() {
            self.skipped_for_consent += count;
        }
    }

    /// The HOOK-107 predicate: there was work to do, none of it ran, and at
    /// least one hook was skipped for want of consent.
    fn nothing_could_consent(self) -> bool {
        self.existed > 0 && self.ran == 0 && self.skipped_for_consent > 0
    }
}

/// Run an item's hooks in place (install/uninstall) or re-install it (build).
///
/// `target` is the raw target string the user typed; it names the run in the
/// HOOK-108 error so the printed remedy is the command they actually invoked.
/// `force` mirrors `run_source_hooks`'s `--force`: for the install event, it
/// re-runs a hook already recorded as run at the item's current commit
/// (HOOK-110) instead of filtering it out as pending-only. Returns the
/// HOOK-107/108 tally on success (CLI-222); always zero for `--event build`,
/// which has no hook-by-hook count of its own (and does not consult `force`:
/// a build hook carries no HOOK-110 recorded-run state to override).
// spec: HOOK-102 HOOK-103 HOOK-108 HOOK-110
fn run_item_hooks(
    paths: &Paths,
    item_ref: &crate::resolve::ItemRef,
    target: &str,
    event: HookEventArg,
    force: bool,
    dangerously_skip_install: bool,
    dangerously_skip_build: bool,
) -> Result<HookTally> {
    let manifest = Manifest::load(paths)?;
    let matches = select_installed(&manifest.items, item_ref);

    if matches.is_empty() {
        // spec: HOOK-102 -- not-installed item is an error naming the item.
        return Err(MindError::NotInstalled {
            name: item_ref.name.clone(),
        });
    }

    // spec: HOOK-108 -- the HOOK-107 accounting applies to item targets too,
    // accumulated across every item the ref matched.
    let mut tally = HookTally::default();
    // spec: HOOK-106 HOOK-107 HOOK-108 -- every item that contributed at least
    // one considered hook, for the HooksNotRun remedy (see `run_source_hooks`'s
    // `contributors` for the same reasoning on the source side).
    let mut contributors: Vec<String> = Vec::new();

    // spec: CLI-194 -- a ref matching several items runs each in turn.
    for installed in matches {
        match event {
            HookEventArg::Build => {
                run_item_build(paths, installed, dangerously_skip_build)?;
            }
            HookEventArg::Install | HookEventArg::Update | HookEventArg::Uninstall => {
                run_item_lifecycle_hooks(
                    paths,
                    installed,
                    event,
                    force,
                    dangerously_skip_install,
                    &mut tally,
                    &mut contributors,
                )?;
            }
        }
    }

    // spec: HOOK-108 -- an item target that had hooks to offer but no way to
    // consent to any of them is an error, exactly as for a source target
    // (HOOK-107), instead of a silent exit 0.
    if tally.nothing_could_consent() {
        // spec: HOOK-106 HOOK-107 -- when the ref matched exactly one item AND
        // the typed target carries no glob metacharacter, the target as typed
        // already resolves back to that same item deterministically (whatever
        // form it was spelled in -- bare, a kind-qualified escape, or an
        // abbreviated source selector), so it is kept verbatim rather than
        // rewritten into a normalized form (that would risk re-entering the
        // HOOK-105 source/item ambiguity a kind-qualified escape was written
        // specifically to avoid). A target that DOES carry a glob metacharacter
        // (e.g. `agents#skill:sc*`) is echoed back only when it matched
        // SEVERAL items; when it happens to match exactly one, the glob text
        // itself is still not safe to paste into a shell (it would expand
        // against the caller's cwd, not necessarily naming the item at all),
        // so it is replaced with the resolved concrete identity instead --
        // `contributors` already holds exactly that (`item_hook_target`,
        // pushed by `run_item_lifecycle_hooks` for the one item that
        // contributed), which is what makes this branch able to reuse it
        // rather than re-deriving it from a separately captured `InstalledItem`.
        let resolved = if contributors.len() == 1 {
            if crate::resolve::is_glob(target) {
                contributors
            } else {
                vec![target.to_string()]
            }
        } else {
            contributors
        };
        return Err(MindError::HooksNotRun {
            target: target.to_string(),
            event: event_label(event).to_string(),
            skipped: tally.skipped_for_consent,
            resolved,
        });
    }
    Ok(tally)
}

/// Re-install an item through the transactional path so a failed build leaves
/// the existing copy untouched (HOOK-103 / LIFE-1 / LIFE-4).
// spec: HOOK-103
fn run_item_build(
    paths: &Paths,
    installed: &InstalledItem,
    dangerously_skip_build: bool,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let source = registry
        .sources
        .iter()
        .find(|s| s.name == installed.source)
        .ok_or_else(|| MindError::SourceNotFound {
            name: installed.source.clone(),
        })?;

    let mut catalog_items = Vec::new();
    crate::catalog::scan_source(paths, source, &mut catalog_items)?;

    let catalog_item = catalog_items
        .iter()
        .find(|c| c.kind == installed.kind && c.name == installed.bare_name)
        .ok_or_else(|| MindError::NotInstalled {
            name: installed.name.clone(),
        })?;

    let commit = installed.commit.clone();
    // A progress line, not an aside, so it stays a `println!`: stdout in text
    // mode, and unreachable from `--json` stdout because the run's fd 1 points
    // at stderr (main.rs's `json_stdout`). See
    // `tests/cli_hooks.rs::hooks_run_build_rebuilding_note_is_one_document`.
    // spec: CLI-217
    println!(
        "rebuilding {} via transactional reinstall",
        installed.display_key()
    );
    let new_installed = crate::install::install(
        paths,
        catalog_item,
        &commit,
        &catalog_items,
        false,
        dangerously_skip_build,
    )?;
    // Persist the updated InstalledItem (hash may change after a build hook).
    let mut manifest = Manifest::load(paths)?;
    manifest.insert(new_installed);
    manifest.save(paths)?;
    Ok(())
}

/// Run an item's install or uninstall hooks in place at its store location,
/// adding what it considered to `tally` (HOOK-108) and, if it contributed at
/// least one considered hook, its key to `contributors` (HOOK-106/107, for the
/// `HooksNotRun` remedy). `force` (HOOK-110) re-offers an install hook already
/// recorded as run at the item's current commit, mirroring `run_source_hooks`'s
/// `!force` guard on the source side; it has no effect on the uninstall event,
/// which carries no recorded run-state to override.
// spec: HOOK-102 HOOK-108 HOOK-110
fn run_item_lifecycle_hooks(
    paths: &Paths,
    installed: &InstalledItem,
    event: HookEventArg,
    force: bool,
    dangerously_skip: bool,
    tally: &mut HookTally,
    contributors: &mut Vec<String>,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let source = registry
        .sources
        .iter()
        .find(|s| s.name == installed.source)
        .ok_or_else(|| MindError::SourceNotFound {
            name: installed.source.clone(),
        })?;

    let mut catalog_items: Vec<CatalogItem> = Vec::new();
    crate::catalog::scan_source(paths, source, &mut catalog_items)?;

    let catalog_item = catalog_items
        .iter()
        .find(|c| c.kind == installed.kind && c.name == installed.bare_name)
        .ok_or_else(|| MindError::NotInstalled {
            name: installed.name.clone(),
        })?;

    let store = paths.mind_home.join(&installed.store);
    let commit = &installed.commit;

    match event {
        // spec: HOOK-126 -- an update run selects the item's update hooks and is
        // otherwise identical to an install run: same pending filter (HOOK-110),
        // same recording, same in-place execution against the store copy.
        HookEventArg::Install | HookEventArg::Update => {
            let event_name = event_label(event);
            let all_hooks = if event == HookEventArg::Update {
                catalog_item.update_hooks()
            } else {
                catalog_item.install_hooks()
            };
            if all_hooks.is_empty() {
                // spec: CLI-217
                crate::render::note(format!(
                    "note: no {event_name} hooks declared for {}",
                    installed.display_key()
                ));
                return Ok(());
            }
            // spec: HOOK-110 -- a hook already recorded as run at the item's
            // current commit is filtered out before the HOOK-108 tally, exactly
            // as `hook_already_ran` filters an already-ran source install hook
            // (HOOK-101). This is what makes a repeat `hooks run` after the hook
            // ran see `existed == 0` and settle to exit 0 instead of
            // `HooksNotRun`. Filtered-out hooks print nothing, mirroring the
            // silent `continue` in `run_source_hooks`. `--force` skips this
            // filter entirely (offering every declared install hook regardless
            // of its recorded commit), mirroring `run_source_hooks`'s
            // `!force && hook_already_ran(...)` guard on the source side.
            let current = Some(commit.as_str());
            let hooks: Vec<&crate::mindfile::ResolvedHook> = all_hooks
                .into_iter()
                .filter(|h| force || !item_hook_already_ran(installed, &h.run, current))
                .collect();
            if hooks.is_empty() {
                return Ok(());
            }
            // spec: HOOK-108
            tally.offered(hooks.len(), dangerously_skip);
            contributors.push(item_hook_target(installed));
            let (recorded, outcome) = crate::install::run_item_install_hooks_partial(
                catalog_item,
                &hooks,
                &store,
                commit,
                dangerously_skip,
            );
            // spec: HOOK-110 -- persist whatever ran or was offered before an
            // abort too, exactly as `run_source_hooks` saves the registry
            // before propagating a hook's error (HOOK-53): unlike the
            // `learn`/`upgrade` path, this item stays installed regardless of
            // the outcome, so a later hook's failure must not discard the
            // record of an earlier hook that already ran its side effect (it
            // would otherwise be offered, and its side effect re-applied, on
            // retry).
            record_item_hooks_run(paths, installed.key().as_str(), &recorded)?;
            outcome?;
        }
        HookEventArg::Uninstall => {
            let hooks = catalog_item.uninstall_hooks();
            if hooks.is_empty() {
                // spec: CLI-217
                crate::render::note(format!(
                    "note: no uninstall hooks declared for {}",
                    installed.display_key()
                ));
                return Ok(());
            }
            // spec: HOOK-108
            tally.offered(hooks.len(), dangerously_skip);
            contributors.push(item_hook_target(installed));
            crate::install::run_item_uninstall_hooks(
                installed,
                &hooks,
                &store,
                commit,
                dangerously_skip,
            )?;
        }
        HookEventArg::Build => unreachable!("build handled by run_item_build"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Source-level hook lister (HOOK-104)
// ---------------------------------------------------------------------------

/// List hooks for matching sources (and their installed items) without running.
// spec: HOOK-104 CLI-196 CLI-220
fn list_source_hooks(paths: &Paths, target: &str, selector: &str) -> Result<()> {
    let registry = Registry::load(paths)?;
    let manifest = Manifest::load(paths)?;

    let matching: Vec<&Source> = registry
        .sources
        .iter()
        .filter(|s| source_matches_glob(&s.name, selector))
        .collect();

    if matching.is_empty() {
        return Err(MindError::SourceNotFound {
            name: selector.to_string(),
        });
    }

    let mut sources_json: Vec<SourceHooksJson> = Vec::new();

    for source in matching {
        let clone_dir = source.clone_dir(paths);
        let toml_path = clone_dir.join("mind.toml");
        let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
        let resolved = mindfile
            .as_ref()
            .map(|m| m.resolved_hooks(&toml_path))
            .transpose()?
            .unwrap_or_default();

        // spec: DSC-95 -- `source.name` is attacker-controlled (a local-path
        // meld derives it from directory names).
        println!("source: {}", strip_ansi(&source.name));
        let current = source.commit.as_deref();

        let mut hooks_json: Vec<SourceHookJson> = Vec::new();
        if resolved.is_empty() {
            println!("  (no source-level hooks declared)");
        } else {
            for h in &resolved {
                let kind_str = if h.optional { "optional" } else { "required" };
                let event_str = h.event.as_str();
                // spec: HOOK-124 -- an update hook is recorded in the same
                // set as an install hook, so it carries the same
                // pending/last-ran status; an uninstall hook records nothing.
                let status = recorded_event(h.event)
                    .map(|rec_event| install_hook_status(source, &h.run, rec_event, current));
                println!(
                    "  [{event_str}] {kind_str}  {:?}  {}",
                    h.run,
                    status.as_deref().unwrap_or("(not recorded)")
                );
                hooks_json.push(SourceHookJson {
                    event: event_str,
                    required: !h.optional,
                    command: h.run.clone(),
                    status,
                });
            }
        }

        // Also list the installed items of this source and their hooks.
        let source_items: Vec<&InstalledItem> = manifest
            .items
            .values()
            .filter(|it| it.source == source.name)
            .collect();

        let mut items_json: Vec<ItemHooksJson> = Vec::new();
        if !source_items.is_empty() {
            let mut catalog_items = Vec::new();
            let _ = crate::catalog::scan_source(paths, source, &mut catalog_items);

            for installed in source_items {
                let item_hooks: Vec<_> = if let Some(c) = catalog_items
                    .iter()
                    .find(|c| c.kind == installed.kind && c.name == installed.bare_name)
                {
                    c.hooks.iter().collect()
                } else {
                    vec![]
                };

                if !item_hooks.is_empty() {
                    println!("  item: {}", installed.display_key());
                    let mut item_hooks_json: Vec<ItemHookJson> = Vec::new();
                    for h in item_hooks {
                        let kind_str = if h.optional { "optional" } else { "required" };
                        let event_str = h.event.as_str();
                        println!("    [{event_str}] {kind_str}  {:?}", h.run);
                        item_hooks_json.push(ItemHookJson {
                            event: event_str,
                            required: !h.optional,
                            command: h.run.clone(),
                        });
                    }
                    items_json.push(ItemHooksJson {
                        // spec: DSC-95 -- `--json` field.
                        item: installed.display_key(),
                        source: None,
                        hooks: item_hooks_json,
                    });
                }
            }
        }

        sources_json.push(SourceHooksJson {
            // spec: DSC-95 -- `--json` field.
            source: strip_ansi(&source.name),
            hooks: hooks_json,
            items: items_json,
        });
    }

    if crate::render::ctx().json {
        emit_json(&HooksListResult {
            schema: 1,
            action: "hooks-list",
            target: target.to_string(),
            sources: sources_json,
            items: Vec::new(),
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Item-level hook lister (HOOK-104)
// ---------------------------------------------------------------------------

/// List hooks for matching installed items without running any.
// spec: HOOK-104 CLI-196 CLI-220
fn list_item_hooks(paths: &Paths, target: &str, item_ref: &crate::resolve::ItemRef) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let matches = select_installed(&manifest.items, item_ref);

    if matches.is_empty() {
        return Err(MindError::NotInstalled {
            name: item_ref.name.clone(),
        });
    }

    let registry = Registry::load(paths)?;

    let mut items_json: Vec<ItemHooksJson> = Vec::new();

    for installed in matches {
        // spec: DSC-95 -- `installed.source` is attacker-controlled.
        println!(
            "item: {} (source {})",
            installed.display_key(),
            strip_ansi(&installed.source)
        );

        let source = registry.sources.iter().find(|s| s.name == installed.source);
        let hooks: Vec<ResolvedHook> = if let Some(src) = source {
            let mut catalog_items = Vec::new();
            let _ = crate::catalog::scan_source(paths, src, &mut catalog_items);
            catalog_items
                .iter()
                .find(|c| c.kind == installed.kind && c.name == installed.bare_name)
                .map(|c| c.hooks.clone())
                .unwrap_or_default()
        } else {
            vec![]
        };

        let mut hooks_json: Vec<ItemHookJson> = Vec::new();
        if hooks.is_empty() {
            println!("  (no hooks declared)");
        } else {
            for h in &hooks {
                let kind_str = if h.optional { "optional" } else { "required" };
                let event_str = h.event.as_str();
                println!("  [{event_str}] {kind_str}  {:?}", h.run);
                hooks_json.push(ItemHookJson {
                    event: event_str,
                    required: !h.optional,
                    command: h.run.clone(),
                });
            }
        }

        items_json.push(ItemHooksJson {
            // spec: DSC-95 -- `--json` field.
            item: installed.display_key(),
            source: Some(strip_ansi(&installed.source)),
            hooks: hooks_json,
        });
    }

    if crate::render::ctx().json {
        emit_json(&HooksListResult {
            schema: 1,
            action: "hooks-list",
            target: target.to_string(),
            sources: Vec::new(),
            items: items_json,
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// A short description of a `Pin` for the hook disclosure (mirrors the private
/// `pin_description` in commands.rs; duplicated here so hooks_cmd is self-contained).
fn pin_description(pin: &Pin) -> String {
    match pin {
        Pin::DefaultBranch => "default branch".to_string(),
        Pin::FollowBranch(b) => format!("branch {b}"),
        Pin::Tag(t) => format!("tag {t}"),
        Pin::Ref(r) => format!("ref {r}"),
    }
}

/// The `<source>#<kind>:<name>` form that re-resolves `installed` as a
/// `hooks run` item target (HOOK-106/107/108): unlike `InstalledItem::key()`
/// (the manifest key, `kind:name`, which `hooks run` does not accept as a
/// target at all), this is what a `HooksNotRun` remedy must name for the
/// command it prints to actually be runnable. Mirrors the `item_forms`
/// construction in `resolve_hook_target`'s `AmbiguousHookTarget` path.
fn item_hook_target(installed: &InstalledItem) -> String {
    // spec: DSC-95 -- `installed.source`/`installed.name` are attacker-controlled;
    // sanitize each field BEFORE composing (never the composed string), same
    // reasoning as `resolve_hook_target`'s `item_forms`.
    format!(
        "{}#{}:{}",
        strip_ansi(&installed.source),
        installed.kind.as_str(),
        strip_ansi(&installed.name)
    )
}

/// Whether a source hook has already RUN at `current` for this event (mirrors
/// the private `hook_ran_at` in commands.rs).
///
/// The event is half the key (HOOK-124). Without it, a source declaring the
/// same command for `install` and `update` has its update hook considered
/// already-run the moment the install hook ran, and `hooks run <src> --event
/// update` exits 0 having run nothing.
///
/// A HOOK-121 meld baseline is NOT a run: it holds back the automatic `upgrade`
/// pass on an unmoved source, not an on-demand `hooks run` where the user named
/// the target and the event themselves (HOOK-126).
// spec: HOOK-121 HOOK-124
fn hook_already_ran(
    source: &Source,
    command: &str,
    event: RecordedEvent,
    current: Option<&str>,
) -> bool {
    current.is_some()
        && source
            .install_hooks
            .iter()
            .any(|r| r.is(command, event) && !r.baseline && r.ran_at.as_deref() == current)
}

/// Whether an item's install hook has already run at `current` (HOOK-110).
/// Mirrors `hook_already_ran` above, but reads the per-item record
/// (`InstalledItem::install_hooks`) instead of the source's.
fn item_hook_already_ran(installed: &InstalledItem, command: &str, current: Option<&str>) -> bool {
    current.is_some()
        && installed
            .install_hooks
            .iter()
            .any(|r| r.command == command && r.ran_at.as_deref() == current)
}

/// Upsert `recorded`'s outcomes into the manifest entry keyed `key`'s
/// `install_hooks` (HOOK-110), mirroring `record_hook_run`'s upsert-by-command
/// for the source-level record. A no-op if the item vanished from the manifest
/// between load and this call (nothing to persist against).
fn record_item_hooks_run(paths: &Paths, key: &str, recorded: &[RecordedHook]) -> Result<()> {
    if recorded.is_empty() {
        return Ok(());
    }
    let mut manifest = Manifest::load(paths)?;
    if let Some(item) = manifest.items.get_mut(key) {
        for r in recorded {
            if let Some(existing) = item
                .install_hooks
                .iter_mut()
                .find(|e| e.command == r.command)
            {
                existing.ran_at = r.ran_at.clone();
            } else {
                item.install_hooks.push(r.clone());
            }
        }
        manifest.save(paths)?;
    }
    Ok(())
}

/// Upsert a hook's run state in `source.install_hooks`, keyed by `(command,
/// event)` (mirrors the private `record_install_hook` in commands.rs). A run
/// clears the HOOK-121 baseline flag: the record now describes a real run.
// spec: HOOK-124
fn record_hook_run(
    source: &mut Source,
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
        let mut rec = RecordedSourceHook::install(command, ran_at);
        rec.event = Some(event);
        source.install_hooks.push(rec);
    }
}

/// The HOOK-106 skip NOTE for one source hook skipped for want of consent in a
/// non-TTY run: it names the cause and prints the exact, copy-pasteable
/// `mind hooks run <id> ...` command to re-run the hook unattended. This is the
/// note counterpart of the [`MindError::HooksNotRun`] error's remedy, and it
/// carries the SAME injection surface: `source_name` is attacker-influenced (a
/// source name, marketplace alias, or item-link path segment, none restricted
/// against shell metacharacters), so the identity that lands inside the runnable
/// command is passed through [`crate::error::shell_quote`] (HOOK-106) exactly as
/// the error's remedy is. The runnable command is printed on its own indented
/// line with no surrounding shell-quote character, exactly like
/// `error::hooks_not_run_message`'s single-match arm: a DOUBLE-quote
/// presentation frame around the (already single-quoted) identity is copyable
/// in a way that re-exposes `$`/backtick inside it when the frame's own quotes
/// are pasted along with the command, so this note avoids a shell-quote
/// character as its frame entirely. `source_name` also appears once as bare
/// prose ("for <name>"), which is not a shell command and needs no quoting.
fn source_skip_note(event_name: &str, label: &str, source_name: &str) -> String {
    // spec: DSC-95 -- `render::note` does not sanitize, and both the label and
    // the identity are source-controlled: a label of cursor-control escapes can
    // scroll away or overwrite the region where the NEXT hook's consent
    // disclosure is about to be drawn. Strip each field before composing.
    let label = strip_ansi(label);
    let source_name = strip_ansi(source_name);
    let quoted = crate::error::shell_quote(&source_name);
    format!(
        "note: skipped {event_name} hook '{label}' for {source_name} (not a terminal); \
         re-run unattended with:\n  mind hooks run {quoted} --event {event_name} \
         --dangerously-skip-install-hook-check"
    )
}

/// The HOOK-126 "nothing pending" note: `hooks run` found hooks for the event
/// but the pending filter held every one of them back, so the run had nothing
/// to consider.
///
/// `labels` and `source_name` are both source-controlled and land in
/// `render::note`, which does not sanitize, so each is stripped before it is
/// composed in (DSC-95). The identity also appears inside a runnable command,
/// where it is shell-quoted for the same reason `source_skip_note` quotes it.
fn nothing_pending_note(
    event_name: &str,
    settled: &[String],
    source_name: &str,
    current: Option<&str>,
) -> String {
    let safe_name = strip_ansi(source_name);
    let quoted = crate::error::shell_quote(&safe_name);
    let at = match current {
        Some(c) => format!("already ran at {}", strip_ansi(c)),
        None => "already ran".to_string(),
    };
    let which: Vec<String> = settled.iter().map(|l| format!("'{l}'")).collect();
    format!(
        "note: nothing pending for {safe_name}: {event_name} hook(s) {} {at}; \
         use --force to run them anyway:\n  mind hooks run {quoted} --event {event_name} --force",
        which.join(", ")
    )
}

/// The recorded counterpart of a lifecycle event (HOOK-55, HOOK-124), or `None`
/// for an event whose runs are not recorded and so does not participate in the
/// pending filter: install and update are recorded, uninstall is not (it only
/// ever fires at `unmeld` or on demand).
// spec: HOOK-124
fn recorded_event(event: HookEvent) -> Option<RecordedEvent> {
    RecordedEvent::of(event)
}

/// The label for a `--event` value, as it appears in notes and disclosures.
fn event_label(event: HookEventArg) -> &'static str {
    match event {
        HookEventArg::Install => "install",
        HookEventArg::Update => "update",
        HookEventArg::Uninstall => "uninstall",
        HookEventArg::Build => "build",
    }
}

/// Status string for a recorded source hook of `event` shown by `hooks list`.
/// Returns "pending (never ran)", "pending (last ran at <commit>)", "ran at
/// <commit>", or, for a HOOK-121 baseline, "not pending (recorded at meld
/// <commit>)".
///
/// The event is part of the lookup key (HOOK-124): a command declared for both
/// events has two records, and reporting one event's status under the other
/// would tell the user a hook had run when it had not.
// spec: HOOK-124
fn install_hook_status(
    source: &Source,
    command: &str,
    event: RecordedEvent,
    current: Option<&str>,
) -> String {
    match source.install_hooks.iter().find(|r| r.is(command, event)) {
        None => "pending (never ran)".to_string(),
        Some(rec) => match &rec.ran_at {
            None => "pending (never ran)".to_string(),
            Some(ran) if rec.baseline => {
                // HOOK-121: recorded at the meld commit without running, so the
                // hook is not pending, but it never ran either. Saying "ran at"
                // here would be a plain untruth on an informational surface.
                if current.is_some_and(|c| c == ran) {
                    format!("not pending (recorded at meld {ran})")
                } else {
                    format!("pending (never ran; melded at {ran})")
                }
            }
            Some(ran) => {
                if current.is_some_and(|c| c == ran) {
                    format!("ran at {ran}")
                } else {
                    format!("pending (last ran at {ran})")
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HOOK-107 predicate, one axis at a time. Each of the three clauses
    /// has to be able to veto on its own, or the error fires on a run that
    /// either had nothing to do or actually did something.
    // spec: HOOK-107 HOOK-108
    #[test]
    fn nothing_could_consent_needs_work_no_runs_and_a_consent_failure() {
        let t = |existed, ran, skipped_for_consent| {
            HookTally {
                existed,
                ran,
                skipped_for_consent,
            }
            .nothing_could_consent()
        };
        // The one true case: there was work, none of it ran, consent failed.
        assert!(t(1, 0, 1));
        assert!(t(5, 0, 5));
        // Nothing existed: a run with nothing to do is never an error, even if
        // the other counters were somehow non-zero.
        assert!(!t(0, 0, 0));
        assert!(!t(0, 0, 1));
        // Something ran: partial progress is not "the run did nothing".
        assert!(!t(2, 1, 1));
        // Nothing was skipped for want of consent: an interactive decline (or
        // an already-satisfied run) leaves this at zero and stays exit 0.
        assert!(!t(1, 0, 0));
    }

    /// `offered` records HOOKS, not calls: a single call for an item with three
    /// hooks contributes three, and repeated calls accumulate across the items
    /// an item glob matched (CLI-194).
    // spec: HOOK-108
    #[test]
    fn offered_accumulates_hook_counts_across_items() {
        let mut tally = HookTally::default();
        tally.offered(3, true);
        tally.offered(2, true);
        assert_eq!(
            tally,
            HookTally {
                existed: 5,
                ran: 5,
                skipped_for_consent: 0,
            }
        );
        assert!(!tally.nothing_could_consent());
    }

    /// A zero-hook offer moves nothing. The callers guard `hooks.is_empty()`
    /// before calling, so this pins that the guard is belt-and-braces rather
    /// than the only thing keeping an empty batch out of the accounting.
    // spec: HOOK-108
    #[test]
    fn offered_zero_hooks_is_a_no_op() {
        let mut tally = HookTally::default();
        tally.offered(0, true);
        tally.offered(0, false);
        assert_eq!(tally, HookTally::default());
        assert!(!tally.nothing_could_consent());
    }

    /// The bypass arm of `offered` short-circuits ahead of the TTY test, so it
    /// predicts "ran" without consulting the environment at all. That is what
    /// makes `--dangerously-skip-install-hook-check` unable to produce a
    /// `HooksNotRun` no matter how the process was launched.
    // spec: HOOK-108 HOOK-83
    #[test]
    fn offered_under_the_bypass_predicts_ran_regardless_of_environment() {
        let mut tally = HookTally::default();
        tally.offered(4, true);
        assert_eq!(tally.ran, 4);
        assert_eq!(tally.skipped_for_consent, 0);
        assert!(!tally.nothing_could_consent());
    }

    /// The event labels are the literal `--event` values (CLI-195), because the
    /// HOOK-106 remedy interpolates one straight into the command it tells the
    /// user to run. A label that is not a valid flag value would print a
    /// command that fails to parse.
    // spec: HOOK-106 CLI-195
    #[test]
    fn event_label_matches_the_cli_event_values() {
        assert_eq!(event_label(HookEventArg::Install), "install");
        assert_eq!(event_label(HookEventArg::Uninstall), "uninstall");
        assert_eq!(event_label(HookEventArg::Build), "build");
        for arg in [
            HookEventArg::Install,
            HookEventArg::Uninstall,
            HookEventArg::Build,
        ] {
            let label = event_label(arg);
            assert!(
                !label.is_empty() && label.chars().all(|c| c.is_ascii_lowercase()),
                "{label:?} must be a bare lowercase flag value"
            );
        }
    }

    /// HOOK-106 (P1 injection): the source skip NOTE's runnable command must
    /// shell-quote the resolved identity, exactly as the `HooksNotRun` error's
    /// remedy does. A source name is not restricted against shell metacharacters
    /// (`validate_prefix`/`is_safe_manifest_path` allow them), and the note
    /// interpolates it into a "copy this and run it" `mind hooks run <id> ...`
    /// command. On the UNFIXED note (`'mind hooks run {source_name} ...'`, framed
    /// in single quotes with the raw name inside) an identity carrying a single
    /// quote breaks out of the frame; this pins that it no longer can.
    // spec: HOOK-106
    #[test]
    fn source_skip_note_shell_quotes_the_identity_in_the_runnable_command() {
        let evil = "x'; touch /tmp/mind-skip-note-pwned; echo '";
        let note = source_skip_note("install", "setup", evil);
        // The runnable command carries the shell-quoted identity, never the bare
        // injected text spliced straight into the command.
        assert!(
            note.contains(&crate::error::shell_quote(evil)),
            "the note's command must carry the shell-quoted identity: {note}"
        );
        assert!(
            !note.contains("mind hooks run x'; touch"),
            "the note must never splice the raw identity into the command: {note}"
        );
    }

    /// HOOK-106 (P1 injection), proved by execution rather than string match:
    /// extract the `mind hooks run ...` command the note prints and run it
    /// through a real `sh -c`. The injected `touch` must never fire, and the
    /// shell must see the identity back as a single literal argument. This is
    /// the note-side counterpart of `error::tests::
    /// shell_quote_round_trips_a_malicious_identity_through_a_real_shell`.
    // spec: HOOK-106
    #[test]
    fn source_skip_note_command_is_inert_when_pasted_into_a_real_shell() {
        use std::process::Command;
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            // No `sh` on PATH (mirrors selfupdate.rs's skip): nothing to prove.
            return;
        }
        let sentinel = std::path::Path::new("/tmp/mind-skip-note-rt-pwned");
        let _ = std::fs::remove_file(sentinel);
        // No balancing quote in the payload, so on the UNFIXED note (bare
        // identity spliced into the command) the `;` terminates `mind hooks run`
        // and `touch` fires -- making the sentinel check below load-bearing, not
        // merely the stdout comparison.
        let evil = "x; touch /tmp/mind-skip-note-rt-pwned; echo hi";
        let note = source_skip_note("install", "setup", evil);

        // Pull out the runnable command from its own indented line (no
        // surrounding shell-quote character, HOOK-106 Fix 2).
        let lead = "re-run unattended with:\n";
        let start = note.find(lead).expect("note has a runnable command") + lead.len();
        let command = note[start..].lines().next().unwrap().trim();
        assert!(
            command.starts_with("mind hooks run "),
            "extracted command must be the mind invocation: {command:?}"
        );

        // Replace the `mind` binary name with `printf '%s\n'` so the arguments
        // the shell would hand `mind` are instead echoed: an injection would run
        // `touch` (and drop the sentinel) rather than being passed as data.
        let probe = command.replacen("mind hooks run", "printf '%s\\n'", 1);
        let out = Command::new("sh")
            .arg("-c")
            .arg(&probe)
            .output()
            .expect("sh -c must run");
        assert!(
            !sentinel.exists(),
            "the injected 'touch' must not have executed via the note's command: {probe:?}"
        );
        let _ = std::fs::remove_file(sentinel);
        // The identity must survive as one literal argument, unexecuted.
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(evil),
            "the shell must see the identity back literally, proving the quote \
             was not interpreted: stdout {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// HOOK-106 Fix 2: pasting the WHOLE framed remedy -- the "re-run
    /// unattended with:" line and the indented command line beneath it, frame
    /// included -- into a real shell must not execute an embedded `$(...)`
    /// command substitution. On the OLD double-quote presentation frame
    /// (`re-run with "mind hooks run '...' ..."`), copying the surrounding `"`
    /// characters along with the already-single-quoted identity re-exposed
    /// `$`/backtick inside it, because a double-quoted context keeps `$` and a
    /// backtick special even around an inner single-quoted segment. The new
    /// frame carries no shell-quote character at all, so a verbatim paste of
    /// the frame is inert regardless of what a reader includes.
    // spec: HOOK-106
    #[test]
    fn source_skip_note_whole_framed_remedy_is_inert_when_pasted_with_the_frame() {
        use std::process::Command;
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        let sentinel = std::path::Path::new("/tmp/mind-skip-note-whole-frame-pwned");
        let _ = std::fs::remove_file(sentinel);
        let evil = "$(touch /tmp/mind-skip-note-whole-frame-pwned)`touch /tmp/mind-skip-note-whole-frame-pwned`";
        let note = source_skip_note("install", "setup", evil);

        // Take the remedy exactly as a reader would copy it: from "re-run
        // unattended with:" through the end of the note, frame and all.
        let lead_at = note
            .find("re-run unattended with:")
            .expect("note has a remedy frame");
        let framed_remedy = &note[lead_at..];

        let _ = Command::new("sh").arg("-c").arg(framed_remedy).output();
        assert!(
            !sentinel.exists(),
            "pasting the whole framed remedy (frame included) must not execute \
             the embedded command substitution: {framed_remedy:?}"
        );
        let _ = std::fs::remove_file(sentinel);
    }
}
