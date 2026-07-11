//! `mind hooks run` and `mind hooks list` -- on-demand hook execution and
//! inspection, outside the meld/learn/forget/upgrade flows.
//!
//! spec: HOOK-100..104, CLI-194..196

use crate::catalog::CatalogItem;
use crate::cli::HookEventArg;
use crate::error::{MindError, Result};
use crate::manifest::{InstalledItem, Manifest};
use crate::mindfile::{HookEvent, MindToml, ResolvedHook};
use crate::paths::Paths;
use crate::resolve::{HookTarget, parse_hook_target, select_installed, source_matches_glob};
use crate::source::{Pin, RecordedHook, Registry, Source};

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// `mind hooks run <target>` -- run hooks on demand.
///
/// Reuses the same disclosure + consent + run machinery as the automatic flows,
/// so it is neither more nor less guarded (HOOK-100).
// spec: HOOK-100 HOOK-101 HOOK-102 HOOK-103 CLI-194 CLI-195
pub fn run(
    paths: &Paths,
    target: &str,
    event: HookEventArg,
    force: bool,
    dangerously_skip_install: bool,
    dangerously_skip_build: bool,
) -> Result<()> {
    match parse_hook_target(target)? {
        HookTarget::Source(selector) => {
            // spec: HOOK-103 CLI-195 - --event build is invalid for a source target.
            if event == HookEventArg::Build {
                return Err(MindError::BuildEventRequiresItemTarget);
            }
            run_source_hooks(paths, &selector, event, force, dangerously_skip_install)
        }
        HookTarget::Item(item_ref) => run_item_hooks(
            paths,
            &item_ref,
            event,
            dangerously_skip_install,
            dangerously_skip_build,
        ),
    }
}

/// `mind hooks list <target>` -- report declared hooks without running any.
///
/// For a source target, lists the source's hooks (with pending/last-ran info
/// for install hooks) and the hooks of its installed items. For an item ref,
/// lists only that item's hooks.
// spec: HOOK-104 CLI-196
pub fn list(paths: &Paths, target: &str) -> Result<()> {
    match parse_hook_target(target)? {
        HookTarget::Source(selector) => list_source_hooks(paths, &selector),
        HookTarget::Item(item_ref) => list_item_hooks(paths, &item_ref),
    }
}

// ---------------------------------------------------------------------------
// Source-level hook runner (HOOK-101)
// ---------------------------------------------------------------------------

/// Run a source's hooks for the given event (`install` or `uninstall`).
// spec: HOOK-101
fn run_source_hooks(
    paths: &Paths,
    selector: &str,
    event: HookEventArg,
    force: bool,
    dangerously_skip: bool,
) -> Result<()> {
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
            HookEventArg::Uninstall => HookEvent::Uninstall,
            HookEventArg::Build => unreachable!("caller guards build for source targets"),
        };
        let event_name = event_label(event);

        let hooks: Vec<&ResolvedHook> = resolved.iter().filter(|h| h.event == hook_event).collect();

        if hooks.is_empty() {
            println!(
                "note: no {event_name} hooks declared for source {}",
                source.name
            );
            continue;
        }

        let source_name = source.name.clone();
        let pin_desc = pin_description(&source.pin);
        let commit = source.commit.clone().unwrap_or_default();
        let current = source.commit.clone();
        let clone_path = clone_dir.display().to_string();
        let browse_url = source.browse_url(&commit);

        for h in &hooks {
            // spec: HOOK-101 -- for install event, skip hooks already at current
            // commit unless --force overrides.
            if hook_event == HookEvent::Install
                && !force
                && hook_already_ran(&registry.sources[idx], &h.run, current.as_deref())
            {
                continue;
            }

            let disclosure = crate::hook::hook_disclosure_text(
                h.label(),
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
                    println!(
                        "running {event_name} hook '{}' for {}",
                        h.label(),
                        source_name
                    );
                    // spec: HOOK-53 -- a non-zero exit is a hard stop; propagate
                    // the error after saving whatever was recorded so far.
                    if let Err(e) =
                        crate::hook::run_hook(&h.run, &clone_dir, &source_name, h.label())
                    {
                        if registry_dirty {
                            let _ = registry.save(paths);
                        }
                        return Err(e);
                    }
                    // spec: HOOK-101 -- record the run-commit for install hooks.
                    if hook_event == HookEvent::Install {
                        record_hook_run(&mut registry.sources[idx], &h.run, current.clone());
                        registry_dirty = true;
                    }
                }
                crate::hook::HookAct::Skip => {
                    println!(
                        "note: skipped {event_name} hook '{}' for {}",
                        h.label(),
                        source_name
                    );
                    // spec: HOOK-101 -- even a skipped install hook is recorded
                    // (with ran_at = None) so repeat runs know it was offered.
                    if hook_event == HookEvent::Install {
                        record_hook_run(&mut registry.sources[idx], &h.run, None);
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
    }

    if registry_dirty {
        registry.save(paths)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Item-level hook runner (HOOK-102 / HOOK-103)
// ---------------------------------------------------------------------------

/// Run an item's hooks in place (install/uninstall) or re-install it (build).
// spec: HOOK-102 HOOK-103
fn run_item_hooks(
    paths: &Paths,
    item_ref: &crate::resolve::ItemRef,
    event: HookEventArg,
    dangerously_skip_install: bool,
    dangerously_skip_build: bool,
) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let matches = select_installed(&manifest.items, item_ref);

    if matches.is_empty() {
        // spec: HOOK-102 -- not-installed item is an error naming the item.
        return Err(MindError::NotInstalled {
            name: item_ref.name.clone(),
        });
    }

    // spec: CLI-194 -- a ref matching several items runs each in turn.
    for installed in matches {
        match event {
            HookEventArg::Build => {
                run_item_build(paths, installed, dangerously_skip_build)?;
            }
            HookEventArg::Install | HookEventArg::Uninstall => {
                run_item_lifecycle_hooks(paths, installed, event, dangerously_skip_install)?;
            }
        }
    }
    Ok(())
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
    println!("rebuilding {} via transactional reinstall", installed.key());
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

/// Run an item's install or uninstall hooks in place at its store location.
// spec: HOOK-102
fn run_item_lifecycle_hooks(
    paths: &Paths,
    installed: &InstalledItem,
    event: HookEventArg,
    dangerously_skip: bool,
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
        HookEventArg::Install => {
            let hooks = catalog_item.install_hooks();
            if hooks.is_empty() {
                println!("note: no install hooks declared for {}", installed.key());
                return Ok(());
            }
            crate::install::run_item_install_hooks(
                catalog_item,
                &hooks,
                &store,
                commit,
                dangerously_skip,
            )?;
        }
        HookEventArg::Uninstall => {
            let hooks = catalog_item.uninstall_hooks();
            if hooks.is_empty() {
                println!("note: no uninstall hooks declared for {}", installed.key());
                return Ok(());
            }
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
// spec: HOOK-104 CLI-196
fn list_source_hooks(paths: &Paths, selector: &str) -> Result<()> {
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

    for source in matching {
        let clone_dir = source.clone_dir(paths);
        let toml_path = clone_dir.join("mind.toml");
        let mindfile = MindToml::load(&clone_dir).unwrap_or_default();
        let resolved = mindfile
            .as_ref()
            .map(|m| m.resolved_hooks(&toml_path))
            .transpose()?
            .unwrap_or_default();

        println!("source: {}", source.name);
        let current = source.commit.as_deref();

        if resolved.is_empty() {
            println!("  (no source-level hooks declared)");
        } else {
            for h in &resolved {
                let kind_str = if h.optional { "optional" } else { "required" };
                let event_str = match h.event {
                    HookEvent::Install => "install",
                    HookEvent::Uninstall => "uninstall",
                };
                let status = if h.event == HookEvent::Install {
                    install_hook_status(source, &h.run, current)
                } else {
                    "(not recorded)".to_string()
                };
                println!("  [{event_str}] {kind_str}  {:?}  {}", h.run, status);
            }
        }

        // Also list the installed items of this source and their hooks.
        let source_items: Vec<&InstalledItem> = manifest
            .items
            .values()
            .filter(|it| it.source == source.name)
            .collect();

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
                    println!("  item: {}", installed.key());
                    for h in item_hooks {
                        let kind_str = if h.optional { "optional" } else { "required" };
                        let event_str = match h.event {
                            HookEvent::Install => "install",
                            HookEvent::Uninstall => "uninstall",
                        };
                        println!("    [{event_str}] {kind_str}  {:?}", h.run);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Item-level hook lister (HOOK-104)
// ---------------------------------------------------------------------------

/// List hooks for matching installed items without running any.
// spec: HOOK-104 CLI-196
fn list_item_hooks(paths: &Paths, item_ref: &crate::resolve::ItemRef) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let matches = select_installed(&manifest.items, item_ref);

    if matches.is_empty() {
        return Err(MindError::NotInstalled {
            name: item_ref.name.clone(),
        });
    }

    let registry = Registry::load(paths)?;

    for installed in matches {
        println!("item: {} (source {})", installed.key(), installed.source);

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

        if hooks.is_empty() {
            println!("  (no hooks declared)");
        } else {
            for h in &hooks {
                let kind_str = if h.optional { "optional" } else { "required" };
                let event_str = match h.event {
                    HookEvent::Install => "install",
                    HookEvent::Uninstall => "uninstall",
                };
                println!("  [{event_str}] {kind_str}  {:?}", h.run);
            }
        }
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

/// Whether an install hook has already run at `current` (mirrors the private
/// `hook_ran_at` in commands.rs).
fn hook_already_ran(source: &Source, command: &str, current: Option<&str>) -> bool {
    current.is_some()
        && source
            .install_hooks
            .iter()
            .any(|r| r.command == command && r.ran_at.as_deref() == current)
}

/// Upsert a hook's run state in `source.install_hooks` (mirrors the private
/// `record_install_hook` in commands.rs).
fn record_hook_run(source: &mut Source, command: &str, ran_at: Option<String>) {
    if let Some(r) = source
        .install_hooks
        .iter_mut()
        .find(|r| r.command == command)
    {
        r.ran_at = ran_at;
    } else {
        source.install_hooks.push(RecordedHook {
            command: command.to_string(),
            ran_at,
        });
    }
}

/// A human label for a hook event arg.
fn event_label(event: HookEventArg) -> &'static str {
    match event {
        HookEventArg::Install => "install",
        HookEventArg::Uninstall => "uninstall",
        HookEventArg::Build => "build",
    }
}

/// Status string for a recorded source install hook shown by `hooks list`.
/// Returns "pending (never ran)", "pending (last ran at <commit>)", or
/// "ran at <commit>".
fn install_hook_status(source: &Source, command: &str, current: Option<&str>) -> String {
    match source.install_hooks.iter().find(|r| r.command == command) {
        None => "pending (never ran)".to_string(),
        Some(RecordedHook { ran_at: None, .. }) => "pending (never ran)".to_string(),
        Some(RecordedHook {
            ran_at: Some(ran), ..
        }) => {
            if current.is_some_and(|c| c == ran) {
                format!("ran at {ran}")
            } else {
                format!("pending (last ran at {ran})")
            }
        }
    }
}
