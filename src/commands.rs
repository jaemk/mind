//! Command implementations, one public function per CLI verb.

use std::collections::HashSet;
use std::io::Write;

use serde::Serialize;

use crate::catalog::{self, CatalogItem};
use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::git;
use crate::hash::hash_path;
use crate::install;
use crate::manifest::Manifest;
use crate::mindfile::AuthFailureAction;
use crate::mindfile::HookEvent;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::policy::Policy;
use crate::resolve::{
    is_glob, parse_item_ref, resolve, select, select_by_bare_refs, select_installed,
    source_matches, source_matches_glob,
};
use crate::source::{Pin, Registry, parse_spec};

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
    flat_skills: bool,
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
    install_hook: Option<String>,
    dangerously_skip_install_hook_check: bool,
) -> Result<MeldSummary> {
    // Resolve the consumer-supplied pin flags into a single Pin. The flags are
    // independent at the clap layer, so more than one surfaces here as the
    // structured `ConflictingPin` error (CLI-17) rather than a clap usage string.
    let consumer_pin = resolve_pin_flags(follow_branch, pin_tag, pin_ref)?;

    paths.ensure_layout()?;
    // POL-3: the managed policy is authoritative over user intent. Load it once
    // (Err = invalid policy, fail closed via `?`; None = unmanaged, inert).
    let policy = Policy::load()?;
    // CLI-19: prefer SSH for remotes when the user's config asks for it.
    let prefer_ssh = Config::load(paths)?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut visited = HashSet::new();
    let source_name = parse_spec(repo)
        .map(|s| s.name)
        .unwrap_or_else(|_| repo.to_string());
    let mut meld_skipped: Vec<SkippedEntry> = Vec::new();
    let added = meld_recursive(
        paths,
        &mut registry,
        repo,
        alias,
        roots,
        flat_skills,
        consumer_pin,
        true,
        &mut visited,
        policy.as_ref(),
        install_hook,
        dangerously_skip_install_hook_check,
        prefer_ssh,
        None, // a top-level meld has no curator-supplied configuration
        &mut meld_skipped,
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
pub fn maybe_probe_hint(paths: &Paths, repo: &str) -> Result<()> {
    let out = crate::render::ctx();
    if out.json {
        return Ok(());
    }
    let Ok(name) = parse_spec(repo).map(|s| s.name) else {
        return Ok(());
    };
    let registry = Registry::load(paths)?;
    let Some(source) = registry.find(&name) else {
        return Ok(());
    };
    let curates = MindToml::load(&source.clone_dir(paths))?
        .and_then(|m| m.discover)
        .is_some_and(|d| !d.sources.is_empty());
    if curates {
        println!(
            "note: this source curates other sources; run `mind probe` to browse and search what is available"
        );
    }
    Ok(())
}

/// Parse the three optional pin CLI flags into a single `Option<Pin>`.
/// More than one set flag is a `ConflictingPin` error (CLI-17). The flags are
/// kept independent at the clap layer so this structured error is what the user
/// sees, rather than a clap usage string.
///
/// Each supplied value is validated with [`crate::git::validate_ref_value`] so
/// a leading-dash value like `--pin-tag=-x` is rejected with `InvalidRef`
/// before it can reach any git subprocess (DSC-66).
fn resolve_pin_flags(
    follow_branch: Option<String>,
    pin_tag: Option<String>,
    pin_ref: Option<String>,
) -> Result<Option<Pin>> {
    match (follow_branch, pin_tag, pin_ref) {
        (None, None, None) => Ok(None),
        (Some(b), None, None) => {
            crate::git::validate_ref_value(&b)?;
            Ok(Some(Pin::FollowBranch(b)))
        }
        (None, Some(t), None) => {
            crate::git::validate_ref_value(&t)?;
            Ok(Some(Pin::Tag(t)))
        }
        (None, None, Some(r)) => {
            crate::git::validate_ref_value(&r)?;
            Ok(Some(Pin::Ref(r)))
        }
        (b, t, r) => {
            // More than one is set; name the first two for the error.
            let mut names = Vec::new();
            if b.is_some() {
                names.push("--follow-branch");
            }
            if t.is_some() {
                names.push("--pin-tag");
            }
            if r.is_some() {
                names.push("--pin-ref");
            }
            Err(MindError::ConflictingPin {
                first: names[0].to_string(),
                second: names[1].to_string(),
            })
        }
    }
}

/// A short human description of a `Pin` for the hook disclosure (HOOK-20).
/// Shared by `meld_recursive` and `upgrade` so both render the pin the same way.
fn pin_description(pin: &Pin) -> String {
    match pin {
        Pin::DefaultBranch => "default branch".to_string(),
        Pin::FollowBranch(b) => format!("branch {b}"),
        Pin::Tag(t) => format!("tag {t}"),
        Pin::Ref(r) => format!("ref {r}"),
    }
}

/// Whether `upgrade` should re-offer a source's install hooks (HOOK-11, HOOK-55):
/// any recorded install hook whose `ran_at` differs from the source's current
/// commit (never ran, or the source advanced past the run commit).
fn hook_rerun_warranted(source: &crate::source::Source) -> bool {
    !source
        .pending_install_hooks(source.commit.as_deref())
        .is_empty()
}

/// Whether a recorded install hook has already run at `current` (a real commit).
fn hook_ran_at(source: &crate::source::Source, command: &str, current: Option<&str>) -> bool {
    current.is_some()
        && source
            .install_hooks
            .iter()
            .any(|r| r.command == command && r.ran_at.as_deref() == current)
}

/// Record (upsert) an install hook's run state on the source.
fn record_install_hook(source: &mut crate::source::Source, command: &str, ran_at: Option<String>) {
    if let Some(r) = source
        .install_hooks
        .iter_mut()
        .find(|r| r.command == command)
    {
        r.ran_at = ran_at;
    } else {
        source.install_hooks.push(crate::source::RecordedHook {
            command: command.to_string(),
            ran_at,
        });
    }
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
    // DSC-61: curator-supplied hooks (when applied) run after the source's own,
    // through the exact same override/disclosure/decide/run path below.
    resolved.extend(extra_hooks);
    let (hooks, replaced) = crate::hook::apply_install_override(resolved, install_override);

    let pin_desc = pin_description(&source.pin);
    let commit = source.commit.clone().unwrap_or_default();
    let current = source.commit.clone();
    let clone_path = clone_dir.display().to_string();
    let name = source.name.clone();

    for h in hooks.iter().filter(|h| h.event == HookEvent::Install) {
        // HOOK-60: by default re-offer only hooks not yet run at this commit;
        // `--force` (force_rerun) re-offers every install hook.
        if !force_rerun && hook_ran_at(source, &h.run, current.as_deref()) {
            continue;
        }

        // HOOK-56: show the loud override note on the hook the override produced.
        let declared_override: Option<String> = replaced.as_ref().and_then(|cmds| {
            if install_override.map(str::trim) == Some(h.run.as_str()) {
                Some(cmds.join("; "))
            } else {
                None
            }
        });
        let disclosure = crate::hook::hook_disclosure_text(
            h.label(),
            h.optional,
            &name,
            &pin_desc,
            &commit,
            &clone_path,
            &h.run,
            declared_override.as_deref(),
        );

        match crate::hook::decide(&disclosure, h.optional, dangerously_skip)? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook.
                println!("running install hook '{}' for {}", h.label(), name);
                // HOOK-53: a non-zero exit (optional or required) is a hard stop.
                crate::hook::run_hook(&h.run, clone_dir, &name, h.label())?;
                record_install_hook(source, &h.run, current.clone());
            }
            crate::hook::HookAct::Skip => {
                println!(
                    "note: skipped install hook '{}' for {}; its items may not work until it runs",
                    h.label(),
                    name
                );
                record_install_hook(source, &h.run, None);
            }
            crate::hook::HookAct::Abort => return Ok(HookOutcome::Abort),
        }
    }
    Ok(HookOutcome::Proceed)
}

/// Curator-supplied configuration for a nested source, lifted from a parent
/// super-source's `[discover].sources` entry (DSC-59). Resolved by the parent
/// before recursing; the pin is always applied (DSC-65, authoritative); roots
/// and hooks are gated (DSC-60) and applied only when the nested source has no
/// `mind.toml` of its own.
struct CuratedConfig {
    /// The curator pin directive (follow-branch, pin-tag, or pin-ref), if set.
    /// Authoritative: applied whether or not the nested source has a mind.toml
    /// (DSC-65). NOT included in the DSC-60 gating/warning.
    pin: Option<Pin>,
    /// The curator `roots`, if set (an explicit empty list is preserved).
    /// Gated by DSC-60: only applied when the nested source has no mind.toml.
    roots: Option<Vec<String>>,
    /// The curator `flat-skills` (DSC-77). Gated by DSC-60: only applied when the
    /// nested source has no mind.toml of its own.
    flat_skills: bool,
    /// The curator `[[discover.sources.hooks]]`, resolved in declaration order.
    /// Gated by DSC-60: only applied when the nested source has no mind.toml.
    hooks: Vec<crate::mindfile::ResolvedHook>,
}

impl CuratedConfig {
    /// Whether any DSC-60-GATED values (roots or hooks) are present, so the
    /// "ignored" warning is warranted when the nested source has its own
    /// `mind.toml`. The pin is NOT gated (DSC-65) and does NOT participate.
    fn has_gated_values(&self) -> bool {
        self.roots.is_some() || self.flat_skills || !self.hooks.is_empty()
    }
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
#[allow(clippy::too_many_arguments)]
fn meld_recursive(
    paths: &Paths,
    registry: &mut Registry,
    repo: &str,
    alias: Option<String>,
    roots: Vec<String>,
    flat_skills: bool,
    consumer_pin: Option<Pin>,
    top_level: bool,
    visited: &mut HashSet<String>,
    policy: Option<&Policy>,
    install_hook: Option<String>,
    dangerously_skip_hook_check: bool,
    prefer_ssh: bool,
    curated: Option<CuratedConfig>,
    skipped: &mut Vec<SkippedEntry>,
) -> Result<usize> {
    let out = crate::render::ctx();
    let mut source = parse_spec(repo)?;
    // NS-25: reject a reserved-kind-word prefix at the chokepoint where any alias
    // (top-level `--as`, or a nested source's `as =`) is applied. An empty alias
    // ("no prefix") is accepted by validate_prefix.
    if let Some(a) = &alias {
        crate::namespace::validate_prefix(a)?;
    }
    source.alias = alias;
    // CLI-19: rewrite an https remote to its SSH form when SSH is preferred, so
    // the clone uses the user's key (no https username/password prompt). Done
    // before the cycle guard so the recorded URL is the one we actually clone.
    source.prefer_ssh(prefer_ssh);

    // Cycle guard: don't process the same URL twice in one meld run.
    if !visited.insert(source.url.clone()) {
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
    let mut dir = source.clone_dir(paths);
    let is_local = source.is_local();

    if !out.json {
        println!(
            "{} melding {} from {}",
            out.bullet(),
            source.name,
            out.dim(&source.url)
        );
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
        git::clone(&source.url, &dir)?;
    }
    let mut mindfile = MindToml::load(&dir)?;
    source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());

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
        flat_skills: false,
        hooks: Vec::new(),
    });
    let apply_curated = mindfile.is_none();
    // The curator pin is always extracted (DSC-65: authoritative, not gated).
    let curated_pin = curated.pin.clone();
    if !apply_curated && curated.has_gated_values() {
        // spec: DSC-60 — warn only when gated fields (roots/hooks) are present
        // and suppressed. A pin-only entry must NOT trigger this warning.
        eprintln!(
            "warning: {} ships its own mind.toml; curator-supplied roots/flat-skills/hooks are ignored",
            source.name
        );
    }
    let curated_hooks = if apply_curated {
        curated.hooks.clone()
    } else {
        Vec::new()
    };

    // Step 2: resolve the effective pin (CLI-17, DSC-41, DSC-65):
    //   consumer flag > curator pin (DSC-65, authoritative) >
    //   [source] directive > DefaultBranch.
    let toml_path = dir.join("mind.toml");
    let directive_pin = mindfile
        .as_ref()
        .map(|m| m.source.pin_directive(&toml_path))
        .transpose()?
        .flatten();
    let effective_pin = consumer_pin
        .or(curated_pin)
        .or(directive_pin)
        .unwrap_or(Pin::DefaultBranch);

    // Managed-policy enforcement (POL-3 authoritative). The identity is known and
    // the effective pin is resolved, but nothing is registered yet and the only
    // thing on disk is the throwaway default-branch clone (removed on refusal), so
    // a refusal here leaves nothing behind.
    if let Some(policy) = policy {
        let identity = source.name.clone();
        let allowed = policy.allow_matches(&identity);
        if policy.lock() && !allowed {
            // POL-11: locked allowlist refuses a non-matching source outright.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::SourceNotAllowed { identity });
        }
        if !policy.lock() && !allowed {
            // POL-13: with lock off, allow is advisory; warn but proceed.
            eprintln!(
                "warning: source '{identity}' is not in the managed policy's allowlist (advisory; not enforced because [sources].lock is false)"
            );
        }
        if policy.pinned() && matches!(effective_pin, Pin::DefaultBranch | Pin::FollowBranch(_)) {
            // POL-20: pinned policy forbids a floating branch (default branch or
            // --follow-branch); only a tag/ref pin is permitted.
            if !is_local {
                let _ = std::fs::remove_dir_all(&dir);
            }
            return Err(MindError::UnpinnedSourceForbidden { identity });
        }
    }

    // Step 3: if the effective pin is not DefaultBranch, the source is a clone at
    // that point -- including a *pinned local* source, which is snapshotted into
    // the sources tree (so pinning still works) WITHOUT touching the working tree.
    // A pin that does not resolve is a `Git` error that must leave nothing behind
    // (CLI-18); the clone target is always under the sources tree, never the user's
    // working tree.
    if effective_pin != Pin::DefaultBranch {
        // Setting the pin makes `clone_dir` resolve to the sources-tree path even
        // for a local source.
        source.pin = effective_pin.clone();
        let target = source.clone_dir(paths);
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| MindError::io(&target, e))?;
        }
        if let Some(parent) = target.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if let Err(e) = git::clone_at(&source.url, &target, &effective_pin) {
            let _ = std::fs::remove_dir_all(&target);
            return Err(e);
        }
        dir = target;
        // The pin may land on a different mind.toml than the working tree / default
        // branch. Reload it so downstream in-memory reads see the pinned content.
        mindfile = MindToml::load(&dir)?;
        source.description = mindfile.as_ref().and_then(|m| m.source.description.clone());
    }

    source.pin = effective_pin;
    // A linked (no-pin) local source records its working-tree HEAD (best-effort; a
    // non-git local dir simply records no commit). Everything else records the
    // cloned commit.
    source.commit = if source.is_linked() {
        git::head_commit(&source.url, &dir).ok()
    } else {
        Some(git::head_commit(&source.url, &dir)?)
    };

    // Persist the consumer's --root override (STO-17, DSC-51).
    // DSC-52: if --root is given for an authoritative source, print a note.
    let is_authoritative = mindfile.as_ref().is_some_and(|m| m.is_authoritative());
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
            let names: Vec<String> = items.iter().map(|it| it.key()).collect();
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

    warn_unguarded_references(&items);
    if !out.json {
        println!(
            "{} melded {} ({} item(s))",
            out.ok(),
            source.name,
            items.len()
        );
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

    // Capture the super-source name before moving `source` into the registry,
    // so DSC-63 error messages can reference it.
    let super_source_name = source.name.clone();
    registry.sources.push(source);

    let mut added = 1;
    if let Some(nested) = mindfile
        .as_ref()
        .and_then(|m| m.discover.as_ref())
        .map(|d| &d.sources)
    {
        for entry in nested {
            // DSC-64: install = true and a non-empty install-items list are
            // mutually exclusive; error before we register anything.
            entry.validate(&toml_path)?;

            // DSC-59 DSC-65: lift this entry's curator-supplied configuration.
            // The pin directive is authoritative (DSC-65). Hooks and roots are
            // gated (DSC-60). All are resolved here against the super-source's
            // mind.toml path; the gate lives in the recursive call.
            let curated = CuratedConfig {
                pin: entry.pin_directive(&toml_path)?,
                roots: entry.roots.clone(),
                flat_skills: entry.flat_skills,
                hooks: entry.resolved_hooks(&toml_path)?,
            };
            // Nested sources from a curated super-source get no consumer pin or
            // root override; the curator config (when applied) supplies them.
            match meld_recursive(
                paths,
                registry,
                &entry.source,
                entry.alias.clone(),
                vec![], // no consumer roots for nested sources
                false,  // no consumer --flat-skills for nested sources (curator config supplies it)
                None,   // no consumer pin for nested sources
                false,
                visited,
                policy,
                None, // no consumer install hook for nested sources
                dangerously_skip_hook_check,
                prefer_ssh, // nested sources inherit the SSH preference
                Some(curated),
                skipped,
            ) {
                Ok(n) => added += n,
                // DSC-68/DSC-69: an auth failure is governed by on-auth-failure
                // when present; without it, it stays a generic git error.
                Err(e) if git::is_auth_failure(&e) => {
                    let entry_name = parse_spec(&entry.source)
                        .map(|s| s.name)
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
                Err(e) => return Err(e),
            }

            // DSC-63: validate each install_items ref against the nested source's
            // offered bare names. A ref that names a non-existent item is an error
            // at meld, not a silent skip.
            if let Some(refs) = &entry.install_items
                && !refs.is_empty()
                && let Ok(spec) = parse_spec(&entry.source)
                && let Some(nested_src) = registry.find(&spec.name)
            {
                let nested_items = catalog::scan(paths, &single(nested_src))?;
                for item_ref in refs {
                    // Each ref must be a bare kind:name (DSC-63).
                    let parsed = crate::resolve::parse_item_ref(item_ref).map_err(|_| {
                        MindError::BadReference {
                            item: format!("install-items in '{super_source_name}'"),
                            referent: item_ref.clone(),
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
                            in_source: spec.name.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(added)
}

/// Strip ANSI CSI escape sequences and control characters from `s`.
/// CSI sequences (`ESC [` params final-byte) are removed in full; C0
/// Strip ANSI/VT escape sequences and terminal-dangerous Unicode from `s`.
/// `strip-ansi-escapes` handles the full escape grammar (CSI, OSC, DCS, etc.).
/// A second pass drops C0/DEL/C1 controls and Unicode bidi-override/separator
/// code points that are not escape sequences but can still corrupt terminal output.
/// Printable non-ASCII (U+00A0 and above, minus the blocked ranges) is preserved
/// so non-English curator messages are not corrupted.
fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    // Input is valid UTF-8, so output is too; lossy conversion is a no-op in practice.
    String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|&c| {
            (('\x20'..='\x7e').contains(&c) || c > '\u{009f}')
                && !matches!(
                    c,
                    // Bidi-override code points: phishing/spoofing vectors.
                    '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                    // Line separator and paragraph separator.
                    | '\u{2028}' | '\u{2029}'
                )
        })
        .collect()
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

/// Warn when a namespaced source references siblings in bare prose, which
/// prefixing will break unless rewritten as `{{ns:name}}` tokens. Scans every
/// text file of each item (the whole skill directory, or the agent/rule file),
/// matching the breadth of install-time `{{ns:}}` expansion.
fn warn_unguarded_references(items: &[CatalogItem]) {
    // Only meaningful once a prefix is in effect.
    if !items.iter().any(|it| it.prefix.is_some()) {
        return;
    }
    let siblings: std::collections::HashSet<String> =
        items.iter().map(|it| it.name.clone()).collect();
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
            eprintln!(
                "warning: {} references sibling(s) in prose: {}; prefixing may break them at runtime (use {{{{ns:name}}}})",
                item.key(),
                refs.join(", ")
            );
        }
    }
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

/// `mind init-source [path] [--template]` — maintainer scaffolding. Discovers the
/// repo's items, reports the intra-source reference graph, scaffolds a `mind.toml`
/// if absent, and (with `--template`) rewrites bare sibling references into
/// `{{ns:}}` tokens. Operates only on the target directory: no store, no agent
/// home, no network (INIT-6).
// spec: INIT-1 INIT-2 INIT-3 INIT-4 INIT-6 INIT-9
pub fn init_source(dir: Option<&str>, template: bool) -> Result<()> {
    let dir = dir.unwrap_or(".");
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        return Err(MindError::NotADirectory {
            path: dir.to_string(),
        });
    }
    let root = path.canonicalize().map_err(|e| MindError::io(path, e))?;

    // Discover items exactly as melding would (INIT-2): build a local Source for
    // the directory and scan it (honors convention + mind.toml + min-mind-version).
    let source = parse_spec(&root.to_string_lossy())?;
    let mut items: Vec<CatalogItem> = Vec::new();
    catalog::scan_source_at(&root, &source, &mut items)?;

    println!("init-source: {}", root.display());
    if items.is_empty() {
        println!("  no items found (skills/<name>/SKILL.md, agents/<name>.md, rules/<name>.md)");
    } else {
        println!("  {} item(s):", items.len());
        for it in &items {
            println!("    {} {}", it.kind, it.name);
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
                    it.key(),
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
        println!(
            "run `mind init-source {dir} --template` to wrap the bare references as {{{{ns:name}}}}"
        );
    }

    // mind.toml scaffold (INIT-3): create only when absent; never overwrite.
    let toml_path = root.join("mind.toml");
    if toml_path.exists() {
        println!("  mind.toml already exists; left unchanged");
    } else {
        let scaffold = concat!(
            "[source]\n",
            "description = \"\"   # what this source offers\n",
            "# prefix = \"prefix\"   # namespace items as prefix:<name>\n",
            "\n",
            "# Declare hooks that run when a consumer melds or unmelds this source.\n",
            "# Remove the leading `# ` to enable a hook.\n",
            "#\n",
            "# [[hooks]]\n",
            "# run = \"make install\"         # shell command to run\n",
            "# name = \"Build\"               # optional label shown in the prompt\n",
            "# event = \"install\"             # \"install\" (default) or \"uninstall\"\n",
            "# optional = false              # false = required (default). optional only lets the\n",
            "#                               # user decline running it; a failure always aborts.\n",
            "#\n",
            "# [[hooks]]\n",
            "# run = \"make clean\"            # cleanup hook run at unmeld time\n",
            "# name = \"Cleanup\"\n",
            "# event = \"uninstall\"\n",
            "# optional = true               # the user may decline this step (its failure still aborts)\n",
        );
        std::fs::write(&toml_path, scaffold).map_err(|e| MindError::io(&toml_path, e))?;
        println!("  wrote mind.toml");
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
                if file.extension().and_then(|e| e.to_str()) != Some("md") {
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

/// Read all of an item's text files into one buffer, for reference detection.
fn read_item_text(item: &CatalogItem) -> String {
    let mut buf = String::new();
    for file in crate::review::item_files(item) {
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
        let disclosure = crate::hook::hook_disclosure_text(
            h.label(),
            h.optional,
            source_name,
            &pin_desc,
            &commit,
            &clone_path,
            &h.run,
            declared_override,
        );

        match crate::hook::decide(&disclosure, h.optional, dangerously_skip_hook_check)? {
            crate::hook::HookAct::Run => {
                // HOOK-60: indicate the running hook.
                println!("running uninstall hook '{}' for {}", h.label(), source_name);
                // HOOK-53: any failure (optional or required) is a hard stop;
                // the unmeld stops and the source remains.
                crate::hook::run_hook(&h.run, &clone_dir, source_name, h.label())?;
            }
            crate::hook::HookAct::Skip => {
                println!(
                    "note: skipped uninstall hook '{}' for {}",
                    h.label(),
                    source_name
                );
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
        // `--json` is treated as non-interactive: both the human-readable listing
        // and the `confirm(...)` prompt below are gated on `!out.json`, so a JSON
        // run never blocks on a prompt. A non-TTY JSON run without `--yes` still
        // returns ConfirmationRequired (the is_tty check below applies regardless
        // of json), so nothing is removed without an explicit `--yes`; the prompt
        // is intentionally skipped only when an interactive terminal is also JSON.
        if !out.json {
            println!("unmeld would remove {} source(s):", matched.len());
            for i in &matched {
                println!("  {} {}", out.warn(), registry.sources[*i].name);
            }
        }
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!("unmelding {} sources", matched.len()),
            });
        }
        if !out.json && !confirm("remove these source(s)?")? {
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
        .map(|it| it.key())
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
        let dir = source.clone_dir(paths);
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
                println!("  {} {k}", out.bullet());
            }
            println!("run `mind forget '{source_name}#*'` to remove them");
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
                println!("  {} {k}", out.warn());
            }
        }
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!(
                    "unmelding {source_name} (removing {} items)",
                    item_keys.len()
                ),
            });
        }
        if !out.json && !confirm("remove these item(s) and unmeld the source?")? {
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
    let dir = source.clone_dir(paths);
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
}

/// Resolve a `learn` selection (loading the registry, scanning the catalog, and
/// running the dependency closure) without installing anything. Returns the
/// catalog, the registry, and the [`crate::deps::Resolution`] so both `learn`
/// and `learn_preview` share one computation.
fn resolve_learn(
    paths: &Paths,
    item_ref: &str,
) -> Result<(Registry, Vec<CatalogItem>, crate::deps::Resolution)> {
    let registry = Registry::load(paths)?;
    let items = catalog::scan(paths, &registry)?;
    let parsed = parse_item_ref(item_ref)?;

    // A glob selects every match; an exact ref must resolve to exactly one.
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

    // Map the explicitly selected items back to indices into `items` (by
    // identity: a CatalogItem is a unique (source, kind, name)).
    let selected_idx: Vec<usize> = targets
        .iter()
        .filter_map(|t| {
            items
                .iter()
                .position(|c| c.kind == t.kind && c.name == t.name && c.source == t.source)
        })
        .collect();

    // What is already installed (manifest keys are `CatalogItem::key()` form).
    let manifest = Manifest::load(paths)?;
    let installed: HashSet<String> = manifest.items.keys().cloned().collect();

    // The `read` closure feeds each item's concatenated UTF-8 text to the
    // resolver so it can scan for `{{ns:}}` tokens (DEP-1).
    let read = |item: &CatalogItem| -> String {
        let mut parts: Vec<String> = Vec::new();
        for file in crate::review::item_files(item) {
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
    let (_registry, items, resolution) = resolve_learn(paths, item_ref)?;
    Ok(LearnPlan {
        tree: resolution.render_tree(&items),
        adds_dependencies: resolution.adds_dependencies(),
        install_count: resolution.install_order().len(),
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
/// (`clobber`), and whether to run install hooks unattended (`dangerously_skip`).
#[derive(Clone, Copy)]
pub struct InstallFlow {
    pub yes: bool,
    pub clobber: Clobber,
    pub dangerously_skip: bool,
}

pub fn learn(paths: &Paths, item_ref: &str, dry_run: bool, flow: InstallFlow) -> Result<()> {
    let InstallFlow {
        yes,
        clobber,
        dangerously_skip,
    } = flow;
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let out = crate::render::ctx();
    let (registry, items, resolution) = resolve_learn(paths, item_ref)?;

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
            result.installed = closure.iter().map(|t| t.key()).collect();
            return print_json(&result);
        }
        if resolution.adds_dependencies() {
            print!("{}", resolution.render_tree(&items));
        }
        println!("would learn {} item(s):", closure.len());
        let rows = closure
            .iter()
            .map(|t| vec![t.key(), t.source.clone()])
            .collect::<Vec<_>>();
        out.print_rows(&rows);
        return Ok(());
    }

    // DEP-31: when the closure adds items beyond the explicit selection, show the
    // tree and prompt; proceed only on a yes (or `--yes`). When it adds nothing,
    // install directly with no prompt and no tree (CLI-30 behavior unchanged).
    if resolution.adds_dependencies() && !yes && !out.json {
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
            && !policy.allow_matches(&target.source)
        {
            if !out.json {
                println!(
                    "{} skipping {} from {}: source not permitted by the managed policy's allowlist",
                    out.warn(),
                    target.key(),
                    target.source
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
        let siblings = siblings_of(&items, &target.source);
        let force = clobber == Clobber::Force;
        let mut result = install_item(paths, target, &commit, &siblings, force, dangerously_skip);
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
            result = if confirm(&format!(
                "{path} exists and is not managed by mind; overwrite it?"
            ))? {
                install_item(paths, target, &commit, &siblings, true, dangerously_skip)
            } else {
                Err(MindError::LinkOccupied { path })
            };
        }
        match result {
            Ok(installed) => {
                installed_keys.push(installed.key());
                if !out.json {
                    // Keep the line starting with "learned <key>" (tests assert the
                    // prefix); the commit is greened (no-op when color is off).
                    println!(
                        "learned {} from {} ({})",
                        installed.key(),
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
                result.installed = installed_keys;
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
    let InstallFlow {
        clobber,
        dangerously_skip,
        ..
    } = flow;
    let policy = Policy::load()?;
    let (registry, items, resolution) = resolve_learn(paths, item_ref)?;

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
            && !policy.allow_matches(&target.source)
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
        let siblings = siblings_of(&items, &target.source);
        let force = clobber == Clobber::Force;
        let result = install_item(paths, target, &commit, &siblings, force, dangerously_skip);
        match result {
            Ok(installed) => {
                installed_keys.push(installed.key());
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

/// After a `meld`, offer to install the newly melded source's items (CLI-23).
/// This is the interactive form of `learn '<source>#*'`: it previews the items
/// that would be installed and prompts, then installs the whole source on a yes.
/// `--link-only` skips this entirely (the caller does not invoke it). A source
/// with no installable items (e.g. a curated super-source) is a no-op.
///
/// With `yes`, it installs without prompting (and in a non-TTY context). Without
/// `yes` in a non-TTY context it installs nothing and prints how to install
/// later, mirroring the install-hook non-TTY behavior (HOOK-22): a scripted meld
/// stays register-only unless `--yes` is given.
// spec: CLI-23
pub fn install_melded_source(paths: &Paths, repo: &str, flow: InstallFlow) -> Result<()> {
    install_source_items(paths, &parse_spec(repo)?.name, flow)
}

/// Run the post-meld auto-install flow (CLI-23) for one registered source by its
/// name: preview and prompt to install its items (`<source>#*`), or install
/// directly under `--yes`. Used for the top-level source and, via
/// `install_curated_sources`, for nested sources installed with `--recursive`
/// (DSC-55) or a curator `install = true` (DSC-58).
pub fn install_source_items(paths: &Paths, source_name: &str, flow: InstallFlow) -> Result<()> {
    let item_ref = format!("{source_name}#*");

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
                "note: {source_name} has {} item(s) to install; run `mind learn '{item_ref}'` (or re-meld with --yes)",
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
        println!("skipped; run `mind learn '{item_ref}'` to install later");
        Ok(())
    }
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
        .filter(|it| !installed_keys.contains(&it.key()))
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
        .map(|it| format!("{source_name}#{}", it.key()))
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
            println!(
                "note: {source_name} has {count} item(s) to install; run `mind learn '{}'` (or re-meld with --yes)",
                if refs.len() == 1 {
                    refs[0].clone()
                } else {
                    format!("{source_name}#*")
                }
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
        println!("skipped; run `mind learn '{source_name}#*'` to install later");
    }
    Ok(())
}

/// True when the repo spec resolves to an already-registered source.
pub fn is_melded(paths: &Paths, repo: &str) -> Result<bool> {
    let name = parse_spec(repo)?.name;
    Ok(Registry::load(paths)?.find(&name).is_some())
}

/// Re-melding an already-melded source (CLI-12). It does not re-clone or
/// re-register: it ensures the source's items are installed, installing any that
/// are missing just as a fresh meld does (CLI-23), and otherwise (or with
/// `--link-only`) prints a status of the source's items and the commit each is
/// installed at.
// spec: CLI-12
pub fn remeld(
    paths: &Paths,
    repo: &str,
    alias: Option<String>,
    link_only: bool,
    flow: InstallFlow,
    recursive: bool,
) -> Result<()> {
    // `yes` rides inside `flow` for the install calls; remeld itself only needs
    // the clobber (hook force-rerun) and the dangerously-skip flag.
    let InstallFlow {
        clobber,
        dangerously_skip: dangerously_skip_hook_check,
        ..
    } = flow;
    let out = crate::render::ctx();
    let source_name = parse_spec(repo)?.name;
    if !out.json {
        println!("{} {source_name} is already melded", out.bullet());
    }

    // CLI-13: an explicit `--as` on a re-meld changes the source's prefix. Update
    // the recorded alias and rename its installed items to the new effective
    // names (`<prefix>:<bare>`), so re-melding with a prefix actually re-namespaces
    // an already-melded source. `--as ''` removes the prefix.
    if let Some(new_alias) = alias {
        // NS-25: a `--as` prefix change on a re-meld is validated too.
        crate::namespace::validate_prefix(&new_alias)?;
        let mut registry = Registry::load(paths)?;
        if let Some(source) = registry.sources.iter_mut().find(|s| s.name == source_name) {
            let current = source.alias.clone().unwrap_or_default();
            if current != new_alias {
                source.alias = Some(new_alias);
                registry.save(paths)?;
                let renamed = reprefix_source(paths, &registry, &source_name)?;
                if renamed == 0 && !out.json {
                    println!("prefix updated; no installed items to rename");
                }
            }
        }
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
                Err(e) => return Err(e), // a hook failed; leave the source melded
            }
        }
    }

    if !link_only {
        let item_ref = format!("{source_name}#*");
        let to_install = match learn_preview(paths, &item_ref) {
            Ok(plan) => plan.install_count,
            Err(MindError::ItemNotFound { .. }) => 0,
            Err(e) => return Err(e),
        };
        if to_install > 0 {
            install_melded_source(paths, repo, flow)?;
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
    if out.json {
        return print_json(&MutationResult::new("meld", &source_name, "already-melded"));
    }
    source_status(paths, &source_name)
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
        let nested = MindToml::load(&source.clone_dir(paths))?
            .and_then(|m| m.discover)
            .map(|d| d.sources)
            .unwrap_or_default();
        for ns in nested {
            let Ok(spec) = parse_spec(&ns.source) else {
                continue;
            };
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
    }
    Ok(())
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
        let nested = MindToml::load(&source.clone_dir(paths))?
            .and_then(|m| m.discover)
            .map(|d| d.sources)
            .unwrap_or_default();
        for ns in nested {
            let Ok(spec) = parse_spec(&ns.source) else {
                continue;
            };
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
    result.installed = installed;
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
    let items = catalog::scan(paths, &single(source))?;
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
                let hash_lag = hash_path(&it.path).map_or(true, |h| h != m.hash);
                let rename_lag = it.effective_name() != m.name;
                let stale = hash_lag || rename_lag;
                let lag = if stale {
                    out.yellow(" (outdated; run `mind upgrade`)")
                } else {
                    String::new()
                };
                // Stale installs use the ↑ marker, distinct from ✓ for current.
                let marker = if stale { out.stale() } else { out.ok() };
                println!(
                    "  {} {}  installed @ {}{}",
                    marker,
                    it.key(),
                    out.green(&short(&m.commit)),
                    lag
                );
            }
            None => println!(
                "  {} {}  not installed (run `mind learn '{}'`)",
                out.available(),
                it.key(),
                it.key()
            ),
        }
    }
    Ok(())
}

/// Rename a source's installed items to their current effective names after its
/// prefix changed (CLI-13). The registry must already carry the new alias.
/// Matches the manifest by stable identity (source, kind, bare name) and reuses
/// the upgrade rename step: install the new name, then drop the old item by its
/// file registry and re-key the manifest. Returns the number renamed.
fn reprefix_source(paths: &Paths, registry: &Registry, source_name: &str) -> Result<usize> {
    let catalog = catalog::scan(paths, registry)?;
    let mut manifest = Manifest::load(paths)?;
    let installed: Vec<crate::manifest::InstalledItem> = manifest
        .items
        .values()
        .filter(|it| it.source == source_name)
        .cloned()
        .collect();

    let mut count = 0;
    for old in installed {
        let Some(cat) = catalog
            .iter()
            .find(|c| c.kind == old.kind && c.name == old.bare_name && c.source == old.source)
        else {
            continue; // item removed upstream; introspect reports it
        };
        if cat.effective_name() == old.name {
            continue; // prefix unchanged for this item
        }
        let siblings = siblings_of(&catalog, &old.source);
        // HOOK-81/82: the prefix change reinstalls under the new name and removes
        // the old item, so both lifecycle hooks fire (run/skip via the same TTY
        // prompt; no dangerous-skip flag on this interactive path).
        let new = install_item(paths, cat, &old.commit, &siblings, false, false)?;
        uninstall_item(paths, &old, &cat.uninstall_hooks(), &old.commit, false)?;
        manifest.items.remove(&old.key());
        if !json_mode() {
            let out = crate::render::ctx();
            println!(
                "{} renamed {} -> {}",
                out.ok(),
                old.key(),
                out.green(&new.key())
            );
        }
        manifest.insert(new);
        count += 1;
    }
    manifest.save(paths)?;
    Ok(count)
}

/// If two selected items would install under the same `kind:name`, return that
/// key and the sources that collide on it.
fn colliding_install(targets: &[&CatalogItem]) -> Option<(String, Vec<String>)> {
    let mut by_key: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in targets {
        by_key.entry(t.key()).or_default().push(t.source.clone());
    }
    by_key.into_iter().find(|(_, sources)| sources.len() > 1)
}

/// Install one item, then run its install hook (HOOK-81) as the final step. On a
/// hook failure, roll the just-installed item back (remove its links and store
/// copy via the file registry) so it is left not installed, then propagate the
/// error. `dangerously_skip` runs the hook unattended (HOOK-83).
fn install_item(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &[CatalogItem],
    force: bool,
    dangerously_skip: bool,
) -> Result<crate::manifest::InstalledItem> {
    let installed = install::install(paths, item, commit, siblings, force)?;
    // HOOK-86: run every resolved install hook in declaration order (the scalar
    // shorthand is folded in as the first required hook). On a hook failure, roll
    // the just-installed item back.
    let install_hooks = item.install_hooks();
    if !install_hooks.is_empty() {
        let store = paths.mind_home.join(&installed.store);
        if let Err(e) =
            install::run_item_install_hooks(item, &install_hooks, &store, commit, dangerously_skip)
        {
            let _ = install::uninstall(paths, &installed);
            return Err(e);
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
/// - skill  -> `skills/<name>/`  (returns a directory path)
/// - agent  -> `agents/<name>.md`
/// - rule   -> `rules/<name>.md`
fn convention_path_in_root(
    root: &std::path::Path,
    kind: ItemKind,
    name: &str,
) -> std::path::PathBuf {
    match kind {
        ItemKind::Skill => root.join("skills").join(name),
        ItemKind::Agent => root.join("agents").join(format!("{name}.md")),
        ItemKind::Rule => root.join("rules").join(format!("{name}.md")),
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
        .ok_or_else(|| MindError::NotInstalled { name: item.key() })?
        .clone();
    let stray_paths: Vec<&std::path::PathBuf> = item.paths.iter().skip(1).collect();

    if !yes {
        // Print what we will do.
        if !out.json {
            println!("absorb will:");
            println!(
                "  move  {} -> {}",
                source_lobe_path.display(),
                dest_item_path.display()
            );
            for stray in &stray_paths {
                println!("  delete (stray copy) {}", stray.display());
            }
        }
        // C3 / ABS-7: json mode is non-interactive; treat it like non-TTY for
        // the destructive confirmation. A missing --yes refuses with
        // ConfirmationRequired regardless of whether a real TTY is attached.
        if !crate::hook::is_tty() || out.json {
            return Err(MindError::ConfirmationRequired {
                action: format!("absorbing {}", item.key()),
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
    let git_err = (|| {
        crate::git::add_all(&dest_path)?;
        let commit_msg = format!("absorb {}:{}", item.kind.as_str(), item.name);
        crate::git::commit(&dest_path, &commit_msg)
    })();
    if let Err(e) = git_err {
        // Restore: remove the dest copy we just made; source lobe is still intact.
        let _ = crate::install::remove_path(&dest_item_path);
        return Err(e);
    }

    // 3. ABS-1: meld the destination if not yet registered.
    let dest_spec = dest_path.to_string_lossy().into_owned();
    if !is_melded(paths, &dest_spec)? {
        let meld_err = meld(
            paths,
            &dest_spec,
            None,
            vec![],
            false,
            None,
            None,
            None,
            None,
            false,
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
    let effective_key = format!("{}:{}", item.kind.as_str(), effective_name);

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
        item.key()
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
        matches.iter().map(|it| it.key()).collect()
    } else {
        match crate::resolve::resolve_installed(&manifest.items, &parsed) {
            Ok(it) => vec![it.key()],
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
                println!("  {} {key}", out.warn());
            }
        }
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!("removing {} items", keys.len()),
            });
        }
        if !out.json && !confirm("remove these item(s)?")? {
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
        let installed_keys: HashSet<String> = manifest.items.keys().cloned().collect();
        let graph = crate::deps::installed_graph(&catalog, &installed_keys, read_item_text);
        let dependents = graph.dependents(removed_key);
        if !dependents.is_empty() && !yes && !force {
            if !out.json {
                println!(
                    "{} removing {removed_key} will break the following installed items that depend on it:",
                    out.warn()
                );
                for dep in &dependents {
                    println!("  {dep}");
                }
            }
            // C3 / DEP-60: json mode is non-interactive; treat it like non-TTY
            // for this destructive confirmation. A missing --yes/--force refuses
            // with ConfirmationRequired regardless of whether a real TTY is attached.
            if !crate::hook::is_tty() || out.json {
                return Err(MindError::ConfirmationRequired {
                    action: format!(
                        "removing {removed_key} (has {} dependent(s))",
                        dependents.len()
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
            manifest.save(paths)?;
            return Err(e);
        }
        removed.push(key.clone());
        if !out.json {
            println!("{} forgot {key}", out.ok());
        }
    }
    manifest.save(paths)?;
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
    let where_ = item
        .paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    // UNM-5: always state explicitly that the item is not mind-managed and that
    // removal deletes the user's own file or directory.
    if !out.json {
        println!(
            "{} {} is not managed by mind: it is your own file or directory at {where_}, not a mind install. Removing it deletes it.",
            out.warn(),
            item.key()
        );
    }
    if !yes {
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!("removing unmanaged {}", item.key()),
            });
        }
        if !out.json && !confirm("remove this unmanaged item?")? {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }
    for p in &item.paths {
        crate::install::remove_path(p)?;
    }
    if out.json {
        let mut result = MutationResult::new("forget", &item.key(), "removed");
        result.removed = vec![item.key()];
        return print_json(&result);
    }
    println!("{} forgot {} (unmanaged)", out.ok(), item.key());
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
            println!("  {} {}", out.warn(), item.key());
        }
        println!(
            "{} these items are NOT managed by mind: removing them deletes your own files or directories, not symlinks.",
            out.warn()
        );
    }
    if !yes {
        if !crate::hook::is_tty() {
            return Err(MindError::ConfirmationRequired {
                action: format!("removing {} unmanaged items", matched.len()),
            });
        }
        if !out.json && !confirm("remove these unmanaged items?")? {
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
        removed.push(item.key());
        if !out.json {
            println!("{} forgot {} (unmanaged)", out.ok(), item.key());
        }
    }

    if out.json {
        let mut result = MutationResult::new("forget", sentinel, "removed");
        result.removed = removed;
        return print_json(&result);
    }
    Ok(())
}

/// `mind sync [--upgrade] [--dangerously-skip-install-hook-check]` — fetch every
/// source and refresh its recorded commit. With `--upgrade`, an `upgrade` pass
/// runs after the refresh (reporting pending upgrades and prompting before
/// applying, exactly like `mind upgrade`), so one command both fetches upstream
/// and applies pending upgrades. `dangerously_skip_hook_check` is forwarded to
/// the `upgrade` pass so install-hook re-runs can run unattended in CI (HOOK-11,
/// HOOK-23); it is unused when `--upgrade` is absent.
pub fn sync(paths: &Paths, then_upgrade: bool, dangerously_skip_hook_check: bool) -> Result<()> {
    let out = crate::render::ctx();
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    // CLI-19: auto-meld honors the user's SSH preference too.
    let prefer_ssh = Config::load(paths)?.ssh;
    let mut registry = Registry::load(paths)?;
    let mut sync_skipped: Vec<SkippedEntry> = Vec::new();

    // POL-32: provision the policy's auto-meld base set before syncing. Each entry
    // not already in the registry is melded at its declared pin; an entry already
    // present is left unchanged (idempotent). Reuses the meld path, so auto-meld
    // entries discover nested sources just like a user meld. Entries satisfy
    // allow/pinned by policy validation (POL-21/POL-31), so they pass the meld
    // enforcement above.
    if let Some(policy) = policy.as_ref() {
        paths.ensure_layout()?;
        let mut visited = HashSet::new();
        let mut provisioned = 0usize;
        for am in policy.auto_meld() {
            // Idempotency: derive the entry's identity and skip if already melded.
            // (meld_recursive also skips a same-URL duplicate, but checking here
            // avoids the clone attempt and the "melding ..." chatter.)
            if let Ok(spec) = parse_spec(&am.repo)
                && registry.find(&spec.name).is_some()
            {
                continue;
            }
            provisioned += meld_recursive(
                paths,
                &mut registry,
                &am.repo,
                None,
                vec![],
                false, // no consumer --flat-skills for an auto-melded source
                Some(am.pin.clone()),
                false, // skip (not error) if a same-URL entry is already present
                &mut visited,
                Some(policy),
                None,  // auto-meld supplies no install hook
                false, // auto-meld is non-TTY, so its hooks take the HOOK-22 skip path
                prefer_ssh,
                None, // auto-meld has no curator-supplied configuration
                &mut sync_skipped,
            )?;
        }
        if provisioned > 0 {
            registry.save(paths)?;
        }
    }

    if registry.sources.is_empty() {
        if out.json {
            return print_json(&MutationResult::new("sync", "", "no-op"));
        }
        println!("no sources melded; run `mind meld <owner/repo>`");
        return Ok(());
    }
    // A per-source failure (e.g. a network error on one remote) must not abort
    // the whole run: refresh each source independently, persist whatever
    // progress was made, then report the failures and exit non-zero.
    let total = registry.sources.len();
    let mut failures: Vec<String> = Vec::new();
    let mut synced = 0usize;
    for source in &mut registry.sources {
        // POL-12: with the allowlist locked, do not sync a source whose identity
        // is no longer allowed; report and skip it (the rest still sync).
        if let Some(policy) = policy.as_ref()
            && policy.lock()
            && !policy.allow_matches(&source.name)
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
        let dir = source.clone_dir(paths);
        if !out.json {
            print!("{} syncing {} ... ", out.bullet(), source.name);
            let _ = std::io::stdout().flush();
        }
        let refreshed = (|| -> Result<(String, bool, Option<String>)> {
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
                let desc = MindToml::load(&dir)?.and_then(|mt| mt.source.description);
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
            let desc = MindToml::load(&dir)?.and_then(|mt| mt.source.description);
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
                    eprintln!("  {} {}: {e}", out.err(), source.name);
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
    {
        // Collect the nested specs now, before mutably borrowing the registry.
        // DSC-61: carry each entry's curator-supplied configuration so a newly
        // discovered nested source is melded with the same gate/apply behavior as
        // a fresh meld. Hooks resolve against the super-source's mind.toml path.
        struct NestedTodo {
            spec: String,
            alias: Option<String>,
            curated: CuratedConfig,
            /// Auth-failure policy for this entry (DSC-68). Carried so the re-walk
            /// loop can handle auth failures the same way as meld (DSC-68 requires
            /// the same behavior applies during sync).
            on_auth_failure: Option<crate::mindfile::OnAuthFailure>,
        }
        let mut nested: Vec<NestedTodo> = Vec::new();
        for s in &registry.sources {
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
                    flat_skills: ns.flat_skills,
                    hooks: ns.resolved_hooks(&toml_path)?,
                };
                nested.push(NestedTodo {
                    spec: ns.source,
                    alias: ns.alias,
                    curated,
                    on_auth_failure: ns.on_auth_failure,
                });
            }
        }
        // Seed the cycle guard with every registered URL so an existing source is
        // skipped without a clone attempt.
        let mut visited: HashSet<String> = registry.sources.iter().map(|s| s.url.clone()).collect();
        let mut discovered = 0usize;
        for todo in nested {
            if let Ok(s) = parse_spec(&todo.spec)
                && registry.find(&s.name).is_some()
            {
                continue;
            }
            // spec: DSC-68 -- auth failures during the sync re-walk honor
            // on-auth-failure the same way as the nested-source loop in
            // meld_recursive. Without on-auth-failure, the error propagates
            // as a generic git error (hard failure).
            match meld_recursive(
                paths,
                &mut registry,
                &todo.spec,
                todo.alias,
                vec![],
                false, // no consumer --flat-skills on a sync re-walk (curator config supplies it)
                None,
                false,
                &mut visited,
                policy.as_ref(),
                None,  // a re-walked nested source supplies no install hook
                false, // sync is non-TTY: its hooks take the HOOK-22 skip path
                prefer_ssh,
                Some(todo.curated),
                &mut sync_skipped,
            ) {
                Ok(n) => discovered += n,
                Err(e) if git::is_auth_failure(&e) => {
                    let entry_name = parse_spec(&todo.spec)
                        .map(|s| s.name)
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
                Err(e) => return Err(e),
            }
        }
        if discovered > 0 {
            registry.save(paths)?;
        }
    }

    if !failures.is_empty() {
        return Err(MindError::SyncFailed {
            failed: failures.len(),
            total,
        });
    }
    if out.json {
        let mut result = MutationResult::new("sync", "", "synced");
        result.count = Some(synced);
        result.skipped = sync_skipped;
        print_json(&result)?;
    }
    if then_upgrade {
        // spec: HOOK-11, HOOK-23
        upgrade(paths, false, None, dangerously_skip_hook_check)?;
    }
    Ok(())
}

/// `mind upgrade [--yes] [item]` — report and optionally apply upgrades.
pub fn upgrade(
    paths: &Paths,
    yes: bool,
    item_ref: Option<&str>,
    dangerously_skip_hook_check: bool,
) -> Result<()> {
    let out = crate::render::ctx();
    // POL-3: load the managed policy once (fail closed on Err; None = inert).
    let policy = Policy::load()?;
    let mut registry = Registry::load(paths)?;
    let manifest = Manifest::load(paths)?;

    let filter = item_ref.map(parse_item_ref).transpose()?;

    // HOOK-11 scope: a scoped `upgrade <item>` must not re-run install hooks
    // (arbitrary code) for sources unrelated to the targeted item. When a filter
    // is present, restrict the hook re-run to sources that have at least one
    // INSTALLED item matching the filter (the same scoping the per-item loop uses
    // via `installed_matches`). With no filter, `None` means every source is in
    // scope, leaving the unscoped behavior unchanged.
    let hook_scope: Option<HashSet<String>> = filter.as_ref().map(|f| {
        manifest
            .items
            .values()
            .filter(|it| crate::resolve::installed_matches(it, f))
            .map(|it| it.source.clone())
            .collect()
    });

    // HOOK-11: re-run a source's install hook when its commit has advanced past
    // the commit the hook last ran at (or it was recorded but never run). This is
    // a source-level pass, separate from the per-item upgrade loop below.
    rerun_source_hooks(
        paths,
        &mut registry,
        dangerously_skip_hook_check,
        hook_scope.as_ref(),
        policy.as_ref(),
    )?;

    let catalog = catalog::scan(paths, &registry)?;
    let mut pending: Vec<Upgrade> = Vec::new();

    for installed in manifest.items.values() {
        match upgrade_item_disposition(installed, filter.as_ref(), policy.as_ref()) {
            // Out of the scoped selection: silent skip, no output.
            UpgradeDisposition::OutOfScope => continue,
            // POL-12: in-scope but the source is no longer allowed by the locked
            // allowlist; report and skip. The item-ref filter is checked first,
            // so a scoped upgrade never emits skip lines for out-of-scope sources
            // the user never selected.
            UpgradeDisposition::PolicyBlocked => {
                if !out.json {
                    println!(
                        "{} skipping {} from {}: source not permitted by the managed policy's allowlist",
                        out.warn(),
                        installed.key(),
                        installed.source
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
        let new_hash = hash_path(&cat.path)?;
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

    let target = item_ref.unwrap_or("all");

    if pending.is_empty() {
        if out.json {
            return print_json(&MutationResult::new("upgrade", target, "up-to-date"));
        }
        println!("everything is up to date");
        return Ok(());
    }

    if !out.json {
        print_upgrade_report(&registry, &pending);
    }

    if !yes && !out.json && !confirm_default_yes("apply these upgrades?")? {
        println!("aborted; nothing changed");
        return Ok(());
    }

    let mut manifest = manifest;
    let mut applied: Vec<String> = Vec::new();
    let mut renamed = false;
    for up in &pending {
        let siblings = siblings_of(&catalog, &up.cat.source);
        // Build the new version first; the old copy is preserved until this
        // succeeds (transactional install). An upgrade never force-overwrites a
        // foreign target; that is for an explicit `learn --force`.
        let installed = install_item(
            paths,
            &up.cat,
            &up.new_commit,
            &siblings,
            false,
            dangerously_skip_hook_check,
        )?;
        if up.new_name != up.old.name {
            // Rename: drop the old item (by its file registry) and re-key. The OLD
            // item is removed, so its uninstall hook fires (HOOK-82); the new
            // item's install hook already ran in install_item above.
            uninstall_item(
                paths,
                &up.old,
                &up.cat.uninstall_hooks(),
                &up.old.commit,
                dangerously_skip_hook_check,
            )?;
            manifest.items.remove(&up.old.key());
            renamed = true;
            if !out.json {
                println!(
                    "{} upgraded {} -> {}",
                    out.ok(),
                    up.old.key(),
                    out.green(&installed.key())
                );
            }
        } else if !out.json {
            println!("{} upgraded {}", out.ok(), out.green(&installed.key()));
        }
        applied.push(installed.key());
        manifest.insert(installed);
    }
    manifest.save(paths)?;
    if out.json {
        let outcome = if renamed { "renamed" } else { "upgraded" };
        let mut result = MutationResult::new("upgrade", target, outcome);
        result.installed = applied;
        return print_json(&result);
    }
    Ok(())
}

/// HOOK-11, HOOK-55: re-run each source's install hooks when warranted (any
/// recorded hook whose `ran_at` differs from the source's current commit). Same
/// trust boundary as `meld`: prompt and disclose, unless
/// `--dangerously-skip-install-hook-check` is set or there is no TTY. In
/// `upgrade`, Abort is treated as Skip: the source is already registered, so
/// declining the re-run just leaves the existing install in place. Persists the
/// registry only if a hook was run and its recorded commit advanced.
fn rerun_source_hooks(
    paths: &Paths,
    registry: &mut Registry,
    dangerously_skip_hook_check: bool,
    in_scope: Option<&HashSet<String>>,
    policy: Option<&Policy>,
) -> Result<()> {
    let mut changed = false;
    for source in &mut registry.sources {
        if !hook_rerun_warranted(source) {
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
            && !policy.allow_matches(&source.name)
        {
            if !json_mode() {
                println!(
                    "skipping install hook for {}: source not permitted by the managed policy's allowlist",
                    source.name
                );
            }
            continue;
        }

        let dir = source.clone_dir(paths);
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let clone_path = dir.display().to_string();

        // Collect indices of pending hooks so we can mutate by index below.
        // A hook is pending when it has never run (ran_at=None) OR its run-commit
        // differs from the source's current commit. Treating ran_at=None as always
        // pending ensures hooks skipped on a commitless linked source are re-offered.
        let pending_indices: Vec<usize> = source
            .install_hooks
            .iter()
            .enumerate()
            .filter(|(_, h)| h.ran_at.is_none() || h.ran_at.as_deref() != source.commit.as_deref())
            .map(|(i, _)| i)
            .collect();

        for idx in pending_indices {
            let cmd = source.install_hooks[idx].command.clone();

            let run = if dangerously_skip_hook_check {
                // HOOK-23: re-run without prompting.
                if !json_mode() {
                    println!(
                        "note: re-running install hook for {} without the safety prompt (--dangerously-skip-install-hook-check)",
                        source.name
                    );
                }
                true
            } else if !crate::hook::is_tty() {
                // HOOK-22: no TTY; never run silently. Skip the re-run.
                if !json_mode() {
                    println!(
                        "note: skipped re-running the install hook for {} (no TTY); its tooling may be out of date until the hook is re-run",
                        source.name
                    );
                }
                false
            } else {
                let disclosure = crate::hook::disclosure_text(
                    &source.name,
                    &pin_desc,
                    &commit,
                    &clone_path,
                    &cmd,
                    None,
                );
                // Abort is treated as Skip here (the source is already registered,
                // per the HOOK-11 note).
                matches!(
                    crate::hook::prompt_choice(&disclosure)?,
                    crate::hook::HookChoice::RunAndContinue
                )
            };

            if run {
                // HOOK-30: a non-zero exit is a hard error. The source stays
                // registered; just propagate the failure.
                crate::hook::run_hook(&cmd, &dir, &source.name, &cmd)?;
                // HOOK-55: record the commit the hook ran at.
                source.install_hooks[idx].ran_at = source.commit.clone();
                changed = true;
                if !json_mode() {
                    println!("re-ran install hook for {}", source.name);
                }
            }
        }
    }
    if changed {
        registry.save(paths)?;
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
    /// In scope and permitted: consider it for an upgrade.
    Consider,
}

/// Decide an installed item's `upgrade` disposition. The item-ref filter is
/// applied first (POL-12 ordering fix): a scoped `upgrade <item>` must not emit
/// policy-skip lines for sources the user never selected. The policy block is
/// only ever reported for items that passed the filter.
fn upgrade_item_disposition(
    installed: &crate::manifest::InstalledItem,
    filter: Option<&crate::resolve::ItemRef>,
    policy: Option<&Policy>,
) -> UpgradeDisposition {
    if let Some(f) = filter
        && !crate::resolve::installed_matches(installed, f)
    {
        return UpgradeDisposition::OutOfScope;
    }
    if let Some(policy) = policy
        && policy.lock()
        && !policy.allow_matches(&installed.source)
    {
        return UpgradeDisposition::PolicyBlocked;
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
    println!("{} item(s) have upstream changes:\n", pending.len());
    for up in pending {
        if up.new_name != up.old.name {
            println!(
                "  {} {} {} {}  rename {} -> {}",
                out.warn(),
                up.cat.kind,
                up.cat.name,
                out.dim(&format!("[{}]", up.cat.source)),
                up.old.name,
                out.green(&up.new_name)
            );
        } else {
            println!(
                "  {} {} {}",
                out.warn(),
                up.cat.key(),
                out.dim(&format!("[{}]", up.cat.source))
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
        let installed_keys: HashSet<String> = manifest.items.keys().cloned().collect();
        let graph = crate::deps::installed_graph(&catalog, &installed_keys, read_item_text);

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
                let node = graph
                    .subtree_node(&key)
                    .unwrap_or_else(|| crate::deps::DepNode::normal(key, vec![]));
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
            match graph.render_subtree(&key) {
                Some(subtree) => print!("{subtree}"),
                None => println!("{key}"),
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
            return print_json(&registry.sources);
        }
        if registry.sources.is_empty() {
            println!("no sources melded");
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
                    Some(a) => format!(" as:{a}"),
                    None => String::new(),
                };
                // HOOK-58: surface that a source carries install hooks with a
                // count-aware token.
                let hook = match s.install_hooks.len() {
                    0 => String::new(),
                    1 => " hook".to_string(),
                    n => format!(" hooks({n})"),
                };
                vec![
                    out.bullet(),
                    s.name.clone(),
                    out.dim(&s.url),
                    out.dim(&format!("[{commit}{ns}{hook}]")),
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
            return print_json(found);
        }
        println!("{}", out.bold(&found.key()));
        if let Some(d) = &found.description {
            println!("  {}{d}", out.dim("desc    "));
        }
        println!("  {}{}", out.dim("source  "), found.source);
        println!("  {}{}", out.dim("commit  "), short(&found.commit));
        println!("  {}{}", out.dim("hash    "), short(&found.hash));
        println!(
            "  {}{}",
            out.dim("store   "),
            paths.mind_home.join(&found.store).display()
        );
        for link in &found.links {
            println!("  {}{link}", out.dim("link    "));
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
                let hash_lag = hash_path(&cat.path).map_or(true, |h| h != found.hash);
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
                            "key": it.key(),
                            "installed": inst.is_some(),
                            "commit": inst.map(|m| m.commit.clone()),
                        })
                    })
                    .collect();
                for m in orphans_of(s) {
                    rows.push(serde_json::json!({
                        "key": m.key(),
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
        return print_json(&out);
    }

    if registry.sources.is_empty() {
        println!("no sources melded; `mind meld <repo>` to add one");
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
            Some(a) => format!(" as:{a}"),
            None => String::new(),
        };
        let hook = match s.install_hooks.len() {
            0 => String::new(),
            1 => " hook".to_string(),
            n => format!(" hooks({n})"),
        };
        println!(
            "{} {}  {}{}",
            out.bullet(),
            out.bold(&s.name),
            out.dim(&format!("[{commit}{ns}{hook}]")),
            s.description
                .as_deref()
                .map(|d| format!("  {d}"))
                .unwrap_or_default()
        );
        // Each item is a status-marked row; print_rows aligns the key column even
        // with the leading glyph (its visible width ignores any ANSI codes).
        let mut rows: Vec<Vec<String>> = Vec::new();
        for it in items {
            let key = it.key();
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
                    let hash_lag = hash_path(&it.path).map_or(true, |h| h != m.hash);
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
            rows.push(vec![
                format!("  {}", out.warn()),
                m.key(),
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
                    let where_ = u
                        .paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    vec![format!("  {}", out.warn()), u.key(), out.dim(&where_)]
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
    let mut hits: Vec<&CatalogItem> = items
        .iter()
        .filter(|it| {
            catalog::matches_query(it, q) // spec: CLI-85
                && kind.is_none_or(|k| it.kind == k)
                // CLI-86: `--source` accepts a glob over source identities.
                && source.is_none_or(|s| source_matches_glob(&it.source, s))
        })
        .collect();
    hits.sort_by_key(|a| a.key());

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
        let mut rows: Vec<ProbeRow> = hits
            .iter()
            .map(|it| {
                // DEP-62: add direct dependency keys to each catalog row.
                let dependencies = crate::deps::direct_dependency_keys(it, &items, &read_item_text);
                ProbeRow {
                    installed: installed(it),
                    kind: it.kind.as_str(),
                    name: it.effective_name(),
                    source: &it.source,
                    hash: hash_path(&it.path).ok(),
                    description: it.description.as_deref(),
                    unmanaged: false,
                    dependencies,
                }
            })
            .collect();
        for u in &unmanaged {
            rows.push(ProbeRow {
                installed: false,
                kind: u.kind.as_str(),
                name: u.name.clone(),
                source: "",
                hash: None,
                description: None,
                unmanaged: true,
                dependencies: Vec::new(),
            });
        }
        return print_json(&rows);
    }

    if hits.is_empty() && unmanaged.is_empty() {
        if registry.sources.is_empty() {
            println!("no sources melded; run `mind meld <owner/repo>`");
        } else {
            println!("no items match '{q}'");
        }
        return Ok(());
    }

    // spec: DEP-62
    // Human listing: nest each hit's transitive dependencies beneath it. Build a
    // graph over all catalog items (passing every catalog key as the "installed"
    // set makes every item a node), then render_subtree for each hit.
    let all_catalog_keys: HashSet<String> = items.iter().map(|it| it.key()).collect();
    let catalog_graph = crate::deps::installed_graph(&items, &all_catalog_keys, read_item_text);

    let mut rows = Vec::new();
    for it in &hits {
        let cur = hash_path(&it.path).ok();
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
            it.key(),
            out.dim(&it.source),
            out.dim(&hash),
            desc,
        ]);

        // DEP-62: nest transitive dependencies beneath each hit. Use the
        // catalog graph to render a subtree; each dependency line is indented
        // with two leading spaces so it reads as a child of the hit above.
        // Cycle back-edges are marked (cycle) by render_subtree/render_forest.
        if let Some(subtree) = catalog_graph.render_subtree(&it.key()) {
            // The subtree includes the root (it.key()) at depth 0; skip it
            // and emit only the nested child lines (depth >= 1).
            for line in subtree.lines().skip(1) {
                rows.push(vec![
                    String::new(),
                    line.to_string(),
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
        let where_ = u
            .paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(vec![
            String::new(),
            u.key(),
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
    source: &'a str,
    hash: Option<String>,
    description: Option<&'a str>,
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
    let catalog = catalog::scan(paths, &registry)?;
    let manifest = Manifest::load(paths)?;
    let mut issues: Vec<Issue> = Vec::new();
    let mut repaired: Vec<String> = Vec::new();

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

    for it in manifest.items.values() {
        let missing: Vec<&String> = it
            .links
            .iter()
            .filter(|link| std::fs::symlink_metadata(link).is_err())
            .collect();
        if !missing.is_empty() {
            // With --fix, re-link from the store copy; report only what cannot
            // be repaired (e.g. the store copy itself is gone).
            let n = if fix { install::relink(paths, it)? } else { 0 };
            if n > 0 {
                repaired.push(format!("{}: relinked {n} missing symlink(s)", it.key()));
            }
            for link in &missing {
                if std::fs::symlink_metadata(link).is_err() {
                    issues.push(Issue {
                        kind: "missing-link",
                        target: it.key(),
                        message: format!("{}: symlink missing at {link}", it.key()),
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
                target: it.key(),
                message: format!("{}: no longer present in source '{}'", it.key(), it.source),
            }),
            Some(cat) => {
                if cat.effective_name() != it.name {
                    issues.push(Issue {
                        kind: "namespace-changed",
                        target: it.key(),
                        message: format!(
                            "{}: namespace changed to '{}'; run `mind upgrade`",
                            it.key(),
                            cat.effective_name()
                        ),
                    });
                } else if let Ok(h) = hash_path(&cat.path)
                    && h != it.hash
                {
                    issues.push(Issue {
                        kind: "drifted",
                        target: it.key(),
                        message: format!("{}: upstream changed; run `mind upgrade`", it.key()),
                    });
                }
            }
        }
    }

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            issues: &'a [Issue],
            sources: usize,
            items: usize,
        }
        return print_json(&Report {
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

    // Print hard then advisory findings in the shared format.
    crate::review::print_findings(&result.hard, &result.advisory);

    let out = crate::render::ctx();
    // Report any files `--fix` rewrote.
    for f in &result.fixed {
        println!("{} fixed {f}", out.ok());
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

/// `mind config lobes add <path>` — add an agent home by path.
pub fn lobe_add(paths: &Paths, path: &str) -> Result<()> {
    let out = crate::render::ctx();
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing. Load the policy first so the refusal precedes any config write.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("add"));
    }
    paths.ensure_config()?;
    let mut cfg = Config::load(paths)?;
    if cfg.lobes.iter().any(|e| e.path() == path) {
        if out.json {
            return print_json(&MutationResult::new("lobe-add", path, "no-op"));
        }
        println!("{} lobe already configured: {path}", out.available());
        return Ok(());
    }
    cfg.lobes.push(crate::config::LobeEntry::bare(path));
    cfg.save(paths)?;
    if out.json {
        return print_json(&MutationResult::new("lobe-add", path, "added"));
    }
    println!("{} added lobe {path}", out.ok());
    Ok(())
}

/// `mind config lobes add --preset <name>` — add a known harness preset's lobe
/// (its parent path and `kinds` filter) (HARN-4).
pub fn lobe_add_preset(paths: &Paths, name: &str) -> Result<()> {
    let out = crate::render::ctx();
    // POL-40: refuse under a lobe lock before validating or writing anything.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("add"));
    }
    // Resolve (and validate) the preset before touching config.
    let lobe = Paths::preset_lobe(name)?;
    let path = lobe.path.to_string_lossy().into_owned();
    let entry = crate::config::LobeEntry {
        path: path.clone(),
        kinds: lobe.kinds.clone(),
    };
    paths.ensure_config()?;
    let mut cfg = Config::load(paths)?;
    if cfg.lobes.iter().any(|e| e.path() == path) {
        if out.json {
            return print_json(&MutationResult::new("lobe-add", &path, "no-op"));
        }
        println!("{} lobe already configured: {path}", out.available());
        return Ok(());
    }
    cfg.lobes.push(entry.clone());
    cfg.save(paths)?;
    if out.json {
        return print_json(&MutationResult::new("lobe-add", &path, "added"));
    }
    println!("{} added {name} lobe {}", out.ok(), format_lobe(&entry));
    Ok(())
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

/// `mind config lobes remove <path>` — drop an agent home.
pub fn lobe_remove(paths: &Paths, path: &str) -> Result<()> {
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
    cfg.save(paths)?;
    if out.json {
        return print_json(&MutationResult::new("lobe-remove", path, "removed"));
    }
    println!("{} removed lobe {path}", out.ok());
    Ok(())
}

/// `mind config lobes detect` — detect installed harness homes and offer to add
/// their presets (HARN-5). Detection itself never mutates config: it adds the
/// detected lobes only with `--yes` (or, on a TTY, after a confirm prompt).
/// Without a TTY and without `--yes`, it reports only. Honors the POL-40 lobe
/// lock and dedups against the already-configured lobes.
pub fn lobe_detect(paths: &Paths, yes: bool) -> Result<()> {
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

    // Decide whether to mutate. With --yes, add unconditionally. Without it, a
    // TTY gets a confirm prompt; a non-TTY reports only (HARN-5).
    let do_add = if candidates.is_empty() {
        false
    } else if yes {
        true
    } else if crate::hook::is_tty() {
        let names: Vec<String> = candidates
            .iter()
            .map(|(n, e)| format!("{n} ({})", format_lobe(e)))
            .collect();
        confirm(&format!("add detected lobe(s): {}?", names.join(", ")))?
    } else {
        false
    };

    if out.json {
        let detected_json: Vec<serde_json::Value> = candidates
            .iter()
            .map(|(name, entry)| {
                serde_json::json!({
                    "preset": name,
                    "path": entry.path(),
                    "kinds": entry.kinds().map(|ks| {
                        ks.iter().map(|k| k.as_str()).collect::<Vec<_>>()
                    }),
                })
            })
            .collect();
        if do_add {
            for (_, entry) in &candidates {
                cfg.lobes.push(entry.clone());
            }
            cfg.save(paths)?;
        }
        return print_json(&serde_json::json!({
            "action": "lobe-detect",
            "detected": detected_json,
            "added": do_add,
        }));
    }

    if candidates.is_empty() {
        println!("{} no new harness homes detected", out.bullet());
        return Ok(());
    }

    if do_add {
        for (name, entry) in &candidates {
            cfg.lobes.push(entry.clone());
            println!("{} added {name} lobe {}", out.ok(), format_lobe(entry));
        }
        cfg.save(paths)?;
    } else {
        println!("{} detected harness home(s):", out.bullet());
        for (name, entry) in &candidates {
            println!("  {} {name}: {}", out.dim("·"), format_lobe(entry));
        }
        println!("re-run with --yes to add them");
    }
    Ok(())
}

/// `mind completions <shell>` — write a shell completion script to stdout.
pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "mind", &mut std::io::stdout());
}

/// `mind man` — write the roff man page to stdout.
pub fn man() -> Result<()> {
    use clap::CommandFactory;
    let mut out = Vec::new();
    clap_mangen::Man::new(crate::cli::Cli::command())
        .render(&mut out)
        .map_err(|e| MindError::io("<man>", e))?;
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

use crate::render::print_json;

/// A throwaway registry holding just one source, for catalog scans during meld.
fn single(source: &crate::source::Source) -> Registry {
    Registry {
        sources: vec![source.clone()],
    }
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
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

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
        // U+206A sits just above the U+2066-2069 isolate block.
        assert_eq!(
            strip_ansi("a\u{206A}b"),
            "a\u{206A}b",
            "U+206A must pass through (above the isolate block)"
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

    /// Serialize every test that mutates process-global env vars
    /// (`MIND_POLICY_FILE`, `MIND_AGENT_HOMES`). Env is process-wide, so these
    /// tests cannot run concurrently.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

        // Out-of-scope item: silently skipped, never reported as PolicyBlocked.
        assert_eq!(
            upgrade_item_disposition(&other, Some(&filter), Some(&policy)),
            UpgradeDisposition::OutOfScope,
            "POL-12: an unselected item must not be policy-skipped (no skip line)"
        );
        // The selected, allowed item is considered for upgrade.
        assert_eq!(
            upgrade_item_disposition(&selected, Some(&filter), Some(&policy)),
            UpgradeDisposition::Consider,
        );

        // Sanity: with no scope filter, the disallowed item IS policy-blocked
        // (the existing unscoped behavior is preserved).
        assert_eq!(
            upgrade_item_disposition(&other, None, Some(&policy)),
            UpgradeDisposition::PolicyBlocked,
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

        assert_eq!(
            upgrade_item_disposition(&selected, Some(&filter), Some(&policy)),
            UpgradeDisposition::PolicyBlocked,
            "a selected item from a disallowed source is still reported"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Build a bare `Source` for the `hook_rerun_warranted` truth table. Uses
    /// `parse_spec` so the identity fields are filled the same way `meld` fills
    /// them; the test only manipulates the commit and install_hooks fields.
    ///
    /// `hooks` is a list of `(command, ran_at)` pairs to populate
    /// `Source.install_hooks`.
    fn hook_source(commit: Option<&str>, hooks: &[(&str, Option<&str>)]) -> crate::source::Source {
        use crate::source::RecordedHook;
        let mut s = crate::source::parse_spec("acme/tools").expect("spec parses");
        s.commit = commit.map(str::to_string);
        s.install_hooks = hooks
            .iter()
            .map(|(cmd, ran_at)| RecordedHook {
                command: cmd.to_string(),
                ran_at: ran_at.map(str::to_string),
            })
            .collect();
        s
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

    // spec: HOOK-57
    // The init-source scaffold must include commented [[hooks]] examples for both
    // install and uninstall events, at least one marked optional = true.
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
        init_source(Some(tmp.to_str().unwrap()), false).expect("init_source should succeed");

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
            contents.contains("# prefix = \"prefix\""),
            "scaffold must still have commented prefix: {contents}"
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
}
