//! `mind unmeld`.

use super::*;

/// Every hook unmelding `source_name` would offer, in teardown order
/// (HOOK-87, HOOK-186): each installed item's uninstall hooks (unless the
/// items are kept), then the source's.
fn unmeld_plan(
    paths: &Paths,
    registry: &Registry,
    source_name: &str,
    keep_items: bool,
    uninstall_hook: Option<&str>,
    accept: crate::hook::Accept,
) -> Result<Vec<crate::consent::PlannedHook>> {
    let Some(source) = registry.find(source_name) else {
        return Ok(Vec::new());
    };
    let mut plan = Vec::new();
    if !keep_items {
        let manifest = Manifest::load(paths)?;
        let mut item_catalog: Vec<CatalogItem> = Vec::new();
        let _ = catalog::scan_source(paths, source, &mut item_catalog);
        let commit = source.commit.clone().unwrap_or_default();
        for item in manifest
            .items
            .values()
            .filter(|it| it.source == source_name)
        {
            let hooks: Vec<&crate::mindfile::ResolvedHook> =
                item_catalog_match(&item_catalog, item)
                    .map(|c| c.uninstall_hooks())
                    .unwrap_or_default();
            plan.extend(item_uninstall_plan(paths, item, &hooks, &commit, accept));
        }
        // spec: HOOK-198 HOOK-192 -- the items hooks run between the items'
        // uninstall hooks and the source's, with the emptied set.
        plan.extend(crate::items_hook::planned(
            paths,
            registry,
            [source_name],
            crate::items_hook::PlanWhen::Emptied,
            accept,
        ));
    }
    plan.extend(source_uninstall_plan(
        paths,
        source,
        uninstall_hook,
        accept,
    )?);
    Ok(plan)
}

/// `mind unmeld <name> [--unlink-only] [--yes] [--accept-hooks]`
/// — drop a source. `name` may be the full `owner/repo`, an unambiguous repo
/// basename, or a glob (`*`, `?`, `[`) matched against each source's identity and
/// its trailing-suffix forms (CLI-28); a glob removes every source it matches,
/// listing them and confirming first when it matches more than one. By default
/// every item installed from a matched source is uninstalled (via its file
/// registry) before the source is removed (CLI-21); `--unlink-only` keeps the
/// items and only removes the source (CLI-22). Runs the source's declared
/// uninstall hooks (HOOK-54) before removal; `accept.uninstall`
/// bypasses the prompt. `yes` skips the removal confirmations (CLI-42).
pub fn unmeld(
    paths: &Paths,
    name: &str,
    unlink_only: bool,
    yes: bool,
    accept: crate::hook::Accept,
    uninstall_hook: Option<String>,
) -> Result<()> {
    unmeld_inner(paths, name, unlink_only, yes, accept, uninstall_hook, false)
}

pub fn unmeld_preview(
    paths: &Paths,
    name: &str,
    keep_items: bool,
    accept: crate::hook::Accept,
    uninstall_hook: Option<String>,
) -> Result<()> {
    unmeld_inner(paths, name, keep_items, false, accept, uninstall_hook, true)
}

#[allow(clippy::too_many_arguments)]
fn unmeld_inner(
    paths: &Paths,
    name: &str,
    unlink_only: bool,
    yes: bool,
    accept: crate::hook::Accept,
    uninstall_hook: Option<String>,
    preview: bool,
) -> Result<()> {
    // spec: HOOK-186 -- the plan's answer lasts for this verb only.
    let _answers = crate::consent::scope();
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

    if preview {
        let manifest = Manifest::load(paths)?;
        let names: Vec<_> = matched.iter().map(|i| &registry.sources[*i]).collect();
        let mut hooks = Vec::new();
        let mut rows = Vec::new();
        for source in names {
            hooks.extend(unmeld_plan(
                paths,
                &registry,
                &source.name,
                unlink_only,
                uninstall_hook.as_deref(),
                accept,
            )?);
            let items: Vec<_> = if unlink_only {
                Vec::new()
            } else {
                manifest.items.values().filter(|i| i.source == source.name).map(|i| serde_json::json!({
                    "key": i.display_key(), "store": display_path(&paths.mind_home.join(&i.store)),
                    "links": i.links.iter().map(|p| strip_ansi(p)).collect::<Vec<_>>()
                })).collect()
            };
            let dir = clone_dir_checked(paths, source)?;
            let clone = (!source.is_linked() && dir.exists()).then(|| display_path(&dir));
            rows.push(serde_json::json!({"source": strip_ansi(&source.name), "items":items, "clone":clone}));
        }
        if out.json {
            let hook_rows: Vec<_> = hooks.iter().map(|h| serde_json::json!({"source":strip_ansi(&h.key.source),"item":h.key.item.as_ref().map(|v| strip_ansi(v)),"event":strip_ansi(&h.key.event),"command":strip_ansi(&h.label)})).collect();
            print_json(
                &serde_json::json!({"schema":1,"action":"unmeld","outcome":"preview","sources":rows,"hooks":hook_rows,"keep_items":unlink_only}),
            )?;
        } else {
            println!("Preview: unmeld would remove {} source(s):", rows.len());
            for row in &rows {
                println!("  {}", row["source"].as_str().unwrap_or_default());
                if let Some(clone) = row["clone"].as_str() {
                    println!("    clone: {clone}");
                }
                for item in row["items"].as_array().unwrap() {
                    println!(
                        "    {} (store: {})",
                        item["key"].as_str().unwrap_or_default(),
                        item["store"].as_str().unwrap_or_default()
                    );
                    for link in item["links"].as_array().unwrap() {
                        println!("      link: {}", link.as_str().unwrap_or_default());
                    }
                }
            }
            if unlink_only {
                println!("Installed items will be kept.");
            }
            if !hooks.is_empty() {
                print!("{}", crate::consent::render(&hooks));
            }
        }
        return Ok(());
    }

    // CLI-28: when a glob matches more than one source, list the matched sources
    // and confirm once before removing them (CLI-260, at source granularity).
    // `--yes` skips it; a non-TTY run without `--yes` refuses rather than
    // removing silently.
    if matched.len() > 1 && !yes {
        // spec: LIFE-45 -- `--json` is always non-interactive (regardless of
        // whether stdin is a real TTY), so it never reaches the plan prompt
        // below: the check right after this block returns
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
    }
    if matched.len() > 1 {
        // spec: HOOK-186 -- one plan for every matched source's hooks: with the
        // removal confirmation, or hooks-only under `--yes` (a non-interactive
        // `--yes` run asks nothing here).
        let mut hooks = Vec::new();
        for i in &matched {
            hooks.extend(unmeld_plan(
                paths,
                &registry,
                &registry.sources[*i].name,
                unlink_only,
                uninstall_hook.as_deref(),
                accept,
            )?);
        }
        let items = (!yes).then_some(PlanItems {
            question: "remove these source(s)?",
            always: true,
            default_yes: false,
            removal: true,
            listing: Vec::new(),
        });
        if ask_plan(paths, items, hooks)? != PlanOutcome::Proceed {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // Removing a source mutates `registry.sources` indices, so resolve each
    // matched source by its name up front and unmeld them one at a time. When the
    // glob matched several sources we already confirmed above at source
    // granularity, so the per-source confirmation is suppressed (`yes`); a
    // single match gets its own (CLI-260).
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
            accept,
            uninstall_hook.as_deref(),
        )?;
    }
    Ok(())
}

/// Tear down a single melded source by its full identity, the per-source body
/// shared by `unmeld` (CLI-21/CLI-22). It removes the source's installed items
/// (each via its file registry) before the source itself, preserving the HOOK-87
/// order (item uninstall hooks before the source's). It asks the removal
/// confirmation itself (CLI-260) unless `yes`, which the caller also sets after
/// confirming a multi-source glob once.
pub(super) fn unmeld_one(
    paths: &Paths,
    source_name: &str,
    unlink_only: bool,
    yes: bool,
    accept: crate::hook::Accept,
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

    // spec: CLI-260 HOOK-186 -- every unmeld confirms before anything is
    // removed or any uninstall hook runs, defaulting to no; `--yes` answers it
    // (leaving a hooks-only plan), and a non-TTY or `--json` run without `--yes`
    // refuses (CLI-259, LIFE-45).
    let hooks = unmeld_plan(
        paths,
        &registry,
        &source_name,
        unlink_only,
        uninstall_hook,
        accept,
    )?;
    if yes {
        ask_plan(paths, None, hooks)?;
    } else {
        let removing = !unlink_only && !item_keys.is_empty();
        if !out.json {
            if removing {
                println!(
                    "unmelding {source_name} will remove {} installed item(s):",
                    item_keys.len()
                );
                for k in &item_keys {
                    // spec: DSC-95
                    println!("  {} {}", out.warn(), strip_ansi(k));
                }
            } else {
                println!("unmelding {source_name}");
            }
        }
        // spec: LIFE-45 -- B1: `--json` is non-interactive (mirrors DEP-60).
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            let action = if removing {
                format!(
                    "unmelding {source_name} (removing {} items)",
                    item_keys.len()
                )
            } else {
                format!("unmelding {source_name}")
            };
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(action, out.json),
            });
        }
        let question = if removing {
            "remove these item(s) and unmeld the source?"
        } else if unlink_only && !item_keys.is_empty() {
            "unmeld the source and keep its items installed?"
        } else {
            "unmeld this source?"
        };
        let items = PlanItems {
            question,
            always: true,
            default_yes: false,
            removal: true,
            listing: Vec::new(),
        };
        if ask_plan(paths, Some(items), hooks)? != PlanOutcome::Proceed {
            println!("cancelled; nothing removed");
            return Ok(());
        }
    }

    // CLI-22: `--unlink-only` removes only the source, leaving its items in place,
    // and lists them with the command to remove them later. Uninstall hooks still
    // run on this path (before the source is removed).
    if unlink_only {
        let proceed =
            run_uninstall_hooks(paths, &registry, idx, &source_name, uninstall_hook, accept)?;
        if !proceed {
            // User aborted the unmeld via the hook prompt; source stays.
            // spec: CLI-153 -- still one result document under `--json`.
            if out.json {
                return print_json(&MutationResult::new("unmeld", &source_name, "aborted"));
            }
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

    // CLI-21: default -- uninstall every item from this source, then remove it,
    // confirmed above before any uninstall hook runs (HOOK-54).

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
            if let Err(e) = uninstall_item(paths, &item, &uninstall_hooks, &commit, accept) {
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

    // spec: HOOK-192 -- the source's items hooks see the emptied set after the
    // items' own uninstall hooks and before the source's. Offered even when
    // this unmeld removed nothing: an earlier skipped run's removals are
    // still pending, and this is the hook's last chance to see them.
    crate::items_hook::offer(paths, &source_name, "unmeld", accept, false)?;
    // `offer` saved its records through its own registry load; carry them
    // into this one, which the abort path below saves.
    if let Some(saved) = Registry::load(paths)?.find(&source_name) {
        registry.sources[idx].items_hooks = saved.items_hooks.clone();
    }

    // HOOK-54/87: the source's uninstall hooks run AFTER every item has been
    // removed, still in the clone, before the clone and registry entry are
    // dropped. Non-TTY: skip with a note; `accept.uninstall` runs them
    // unattended. An abort or required-hook failure leaves the source melded.
    let proceed = run_uninstall_hooks(paths, &registry, idx, &source_name, uninstall_hook, accept)?;
    if !proceed {
        // User aborted the source uninstall hook; source stays (items already
        // removed are kept removed, mirroring a partial teardown).
        registry.save(paths)?;
        if out.json {
            // spec: CLI-153
            let mut result = MutationResult::new("unmeld", &source_name, "aborted");
            result.count = Some(forgotten);
            return print_json(&result);
        }
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
