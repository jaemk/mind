//! `mind forget` and `mind prune`.

use super::*;

/// `mind forget <item>` — uninstall one item, or many via a glob.
///
/// When `unmanaged` is true, removal is scoped to unmanaged lobe items only
/// (UNM-7/UNM-8). `item_ref` may be `None` to remove every unmanaged item.
/// When `unmanaged` is false, behavior is unchanged: `item_ref` is required.
pub fn forget(
    paths: &Paths,
    item_ref: Option<&str>,
    unmanaged: bool,
    dropped: bool,
    yes: bool,
    force: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    if unmanaged {
        return forget_unmanaged_bulk(paths, item_ref, yes);
    }
    if dropped {
        // spec: CLI-245
        let manifest = Manifest::load(paths)?;
        let keys = dropped_item_keys(paths, &manifest, item_ref, None)?;
        let target = item_ref.unwrap_or("--dropped");
        return forget_dropped(paths, "forget", manifest, keys, target, yes, force, accept);
    }

    let mut manifest = Manifest::load(paths)?;
    // clap guarantees `item_ref` is Some without --unmanaged or --dropped.
    let item_ref = item_ref.expect("item_ref required without --unmanaged or --dropped");
    let parsed = parse_item_ref(item_ref)?;
    let keys = forget_ref_keys(paths, &manifest, &parsed, yes)?;
    if keys.is_empty() {
        // An exact ref that named an unmanaged item, already removed.
        return Ok(());
    }
    forget_keys(
        paths,
        &mut manifest,
        keys,
        item_ref,
        "forget",
        yes,
        force,
        accept,
    )
}

/// Preview the same managed selection as forget; an exact miss may select an
/// unmanaged item, just as the removal path does.
pub fn forget_preview(
    paths: &Paths,
    item_ref: Option<&str>,
    unmanaged: bool,
    dropped: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let parsed = item_ref.map(parse_item_ref).transpose()?;
    if unmanaged {
        let scanned = crate::unmanaged::scan(paths, &manifest)?;
        let items = crate::unmanaged::select(&scanned, parsed.as_ref());
        if items.is_empty() {
            return Err(MindError::NotInstalled {
                name: item_ref.unwrap_or("*").to_string(),
            });
        }
        return preview_unmanaged(&items);
    }
    if dropped {
        let keys = dropped_item_keys(paths, &manifest, item_ref, None)?;
        return preview_removals(paths, &manifest, keys, "forget", accept, None);
    }
    let parsed = parsed.expect("clap requires an item");
    let keys = if is_glob(&parsed.name) {
        let matches = select_installed(&manifest.items, &parsed);
        if matches.is_empty() {
            return Err(MindError::NotInstalled {
                name: parsed.name.clone(),
            });
        }
        matches.iter().map(|i| i.key().into()).collect()
    } else {
        match crate::resolve::resolve_installed(&manifest.items, &parsed) {
            Ok(item) => vec![item.key().into()],
            Err(MindError::NotInstalled { .. }) => {
                let scanned = crate::unmanaged::scan(paths, &manifest)?;
                let item = crate::unmanaged::resolve(&scanned, &parsed)?;
                return preview_unmanaged(&[item]);
            }
            Err(e) => return Err(e),
        }
    };
    preview_removals(paths, &manifest, keys, "forget", accept, None)
}

pub fn prune_preview(
    paths: &Paths,
    source: Option<&str>,
    accept: crate::hook::Accept,
) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let keys = prune_keys(paths, &manifest, source)?;
    preview_removals(paths, &manifest, keys, "prune", accept, None)
}

fn preview_unmanaged(items: &[&crate::unmanaged::UnmanagedItem]) -> Result<()> {
    let rows: Vec<_> = items.iter().map(|item| serde_json::json!({
        "key": item.display_key(), "paths": item.paths.iter().map(|p| display_path(p)).collect::<Vec<_>>(),
        "managed": false,
    })).collect();
    if crate::render::ctx().json {
        print_json(
            &serde_json::json!({"schema":1,"action":"forget","outcome":"preview","items":rows,"hooks":[]}),
        )
    } else {
        println!("Preview: these unmanaged files or directories would be removed:");
        for row in &rows {
            println!(
                "  {}: {}",
                row["key"].as_str().unwrap_or_default(),
                row["paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }
}

// spec: CLI-264 -- use the same selected keys and catalog as the removal,
// but only render the plan; neither hooks nor deletion are invoked.
pub(super) fn preview_removals(
    paths: &Paths,
    manifest: &Manifest,
    keys: Vec<String>,
    action: &'static str,
    accept: crate::hook::Accept,
    profile: Option<serde_json::Value>,
) -> Result<()> {
    let registry = Registry::load(paths)?;
    let scan = catalog::scan(paths, &registry);
    let catalog_complete = scan.is_ok();
    let catalog = scan.unwrap_or_default();
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
    let mut hooks = Vec::new();
    let mut sources = BTreeSet::new();
    let mut rows = Vec::new();
    for key in &keys {
        let item = &manifest.items[key];
        let resolved = item_catalog_match(&catalog, item)
            .map(|c| c.uninstall_hooks())
            .unwrap_or_default();
        let commit = registry
            .find(&item.source)
            .and_then(|s| s.commit.clone())
            .unwrap_or_default();
        hooks.extend(item_uninstall_plan(paths, item, &resolved, &commit, accept));
        sources.insert(item.source.clone());
        rows.push(serde_json::json!({"key": item.display_key(), "source": strip_ansi(&item.source),
            "store": display_path(&paths.mind_home.join(&item.store)),
            "links": item.links.iter().map(|p| strip_ansi(p)).collect::<Vec<_>>(),
            "dependents": graph.dependents(key).iter().map(|d| strip_ansi(d)).collect::<Vec<_>>(), "managed": true}));
    }
    for source in &sources {
        hooks.extend(crate::items_hook::planned(
            paths,
            &registry,
            [source.as_str()],
            crate::items_hook::PlanWhen::Touched,
            accept,
        ));
    }
    if crate::render::ctx().json {
        let hook_rows: Vec<_> = hooks.iter().map(|h| serde_json::json!({"source":strip_ansi(&h.key.source),"item":h.key.item.as_ref().map(|v| strip_ansi(v)),"event":strip_ansi(&h.key.event),"command":strip_ansi(&h.label)})).collect();
        print_json(
            &serde_json::json!({"schema":1,"action":action,"outcome":"preview","items":rows,"hooks":hook_rows,"catalog_complete":catalog_complete,"profile":profile}),
        )
    } else {
        println!("Preview: {action} would remove {} item(s):", rows.len());
        if let Some(profile) = &profile {
            println!(
                "  profile {} of {} (enabled: {}, remaining: {})",
                profile["name"].as_str().unwrap_or_default(),
                profile["source"].as_str().unwrap_or_default(),
                profile["enabled"],
                profile["remaining"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for kept in profile["kept"].as_array().unwrap() {
                println!(
                    "  kept {} (needed by {})",
                    kept["key"].as_str().unwrap_or_default(),
                    kept["dependents"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        if !catalog_complete {
            println!("Source scan failed; dependents and hooks may be incomplete.");
        }
        for row in &rows {
            println!(
                "  {} (store: {})",
                row["key"].as_str().unwrap_or_default(),
                row["store"].as_str().unwrap_or_default()
            );
            for link in row["links"].as_array().unwrap() {
                println!("    link: {}", link.as_str().unwrap_or_default());
            }
            for dep in row["dependents"].as_array().unwrap() {
                println!("    dependent: {}", dep.as_str().unwrap_or_default());
            }
        }
        if !hooks.is_empty() {
            print!("{}", crate::consent::render(&hooks));
        }
        Ok(())
    }
}

/// `mind prune [<source>]` -- remove every installed item that is no longer
/// part of a melded source: the `forget --dropped` set (CLI-245) plus each
/// source's profile leftovers (PROF-37, PROF-38), narrowed to the items whose
/// recorded source matches the `source` filter. A dropped item's uninstall
/// hooks and dependents are read from its source's catalog, which no longer
/// lists it, so none run and no dependents gate applies. A leftover is still
/// offered: its uninstall hooks run under `accept`, and one another installed
/// item depends on is never in the set.
// spec: CLI-261 PROF-38
pub fn prune(
    paths: &Paths,
    source: Option<&str>,
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let manifest = Manifest::load(paths)?;
    let keys = prune_keys(paths, &manifest, source)?;
    let target = source.unwrap_or("*");
    forget_dropped(paths, "prune", manifest, keys, target, yes, false, accept)
}

fn prune_keys(paths: &Paths, manifest: &Manifest, source: Option<&str>) -> Result<Vec<String>> {
    if let Some(sel) = source {
        crate::resolve::validate_source_selector(sel)?;
        // The filter matches the source an item was installed from, so an
        // unmelded source's items still match; it must name something.
        let registry = Registry::load(paths)?;
        let matches = |name: &str| crate::resolve::source_matches_glob(name, sel);
        if !manifest.items.values().any(|it| matches(&it.source))
            && !registry.sources.iter().any(|s| matches(&s.name))
        {
            return Err(MindError::SourceNotFound {
                name: sel.to_string(),
            });
        }
    }
    let mut keys = dropped_item_keys(paths, manifest, None, source)?;
    for key in all_profile_leftovers(paths, manifest, source)? {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys.sort();
    Ok(keys)
}

/// The profile leftovers (PROF-37) of every melded source with a clone on
/// disk, or only of those matching the CLI-231 `filter`, for `prune`
/// (PROF-38). A source whose enabled set does not expand (PROF-5) is warned
/// about and contributes nothing.
fn all_profile_leftovers(
    paths: &Paths,
    manifest: &Manifest,
    filter: Option<&str>,
) -> Result<Vec<String>> {
    let registry = Registry::load(paths)?;
    let sources: Vec<&crate::source::Source> = registry
        .sources
        .iter()
        .filter(|s| filter.is_none_or(|f| crate::resolve::source_matches_glob(&s.name, f)))
        .filter(|s| {
            manifest
                .items
                .values()
                .any(|m| m.source == s.name && !m.profiles.is_empty())
        })
        .filter(|s| clone_dir_checked(paths, s).is_ok_and(|d| d.is_dir()))
        .collect();
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let catalog = catalog::scan(paths, &registry)?;
    let graph = manifest_graph(&catalog, manifest);
    let mut keys: Vec<String> = Vec::new();
    for s in sources {
        match profile_leftovers(paths, &catalog, manifest, &graph, s) {
            Ok(found) => keys.extend(found),
            Err(e) => crate::render::warn(format!(
                "could not expand the enabled profiles of {}: {}; keeping its items",
                strip_ansi(&s.name),
                strip_ansi(&e.to_string())
            )),
        }
    }
    Ok(keys)
}

/// Remove `keys`, reporting under `action` and `target`: the CLI-245 dropped
/// set for `forget --dropped`, plus the profile leftovers (PROF-38) for
/// `prune`.
#[allow(clippy::too_many_arguments)]
fn forget_dropped(
    paths: &Paths,
    action: &'static str,
    mut manifest: Manifest,
    keys: Vec<String>,
    target: &str,
    yes: bool,
    force: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let out = crate::render::ctx();
    if keys.is_empty() {
        if out.json {
            return print_json(&MutationResult::new(action, target, "no-op"));
        }
        if action == "prune" {
            println!("no dropped items or profile leftovers; nothing to prune");
        } else {
            println!("no dropped items; nothing to {action}");
        }
        return Ok(());
    }
    forget_keys(
        paths,
        &mut manifest,
        keys,
        target,
        action,
        yes,
        force,
        accept,
    )
}

/// The installed items `forget --dropped` removes (CLI-245): those whose
/// source is no longer melded, or whose melded source's catalog has no item
/// with the same stable identity. An item of a melded source with no clone on
/// disk is kept. `item_ref` narrows the set like a `forget` glob, and
/// `source` (a CLI-231 filter) to items whose recorded source it matches.
fn dropped_item_keys(
    paths: &Paths,
    manifest: &Manifest,
    item_ref: Option<&str>,
    source: Option<&str>,
) -> Result<Vec<String>> {
    let registry = Registry::load(paths)?;
    // A scan error must not make every item look dropped.
    let catalog = catalog::scan(paths, &registry)?;
    let candidates: Vec<&crate::manifest::InstalledItem> = match item_ref {
        Some(r) => select_installed(&manifest.items, &parse_item_ref(r)?),
        None => manifest.items.values().collect(),
    };
    let mut keys: Vec<String> = candidates
        .into_iter()
        .filter(|m| source.is_none_or(|sel| crate::resolve::source_matches_glob(&m.source, sel)))
        .filter(|m| match registry.find(&m.source) {
            None => true,
            // spec: DSC-123 -- an item skipped for an unreadable manifest is
            // still shipped, not dropped.
            Some(s) => {
                clone_dir_checked(paths, s).is_ok_and(|d| d.is_dir())
                    && !catalog.iter().any(|it| {
                        it.source == m.source && it.kind == m.kind && it.name == m.bare_name
                    })
                    && !catalog::dropped_as_unreadable(&m.source, m.kind, &m.bare_name)
            }
        })
        .map(|m| m.key().into())
        .collect();
    keys.sort();
    Ok(keys)
}

/// The manifest keys an explicit `forget <ref>` names. A glob uninstalls every
/// installed match (mirroring `learn`'s selection); an exact ref honors the
/// kind prefix and source qualifier and errors on an ambiguous bare name (e.g.
/// one shared by a skill and an agent). An exact ref naming only an unmanaged
/// lobe item removes that item here and returns no keys.
fn forget_ref_keys(
    paths: &Paths,
    manifest: &Manifest,
    parsed: &crate::resolve::ItemRef,
    yes: bool,
) -> Result<Vec<String>> {
    Ok(if is_glob(&parsed.name) {
        let matches = select_installed(&manifest.items, parsed);
        if matches.is_empty() {
            return Err(MindError::NotInstalled {
                name: parsed.name.clone(),
            });
        }
        matches.iter().map(|it| it.key().into()).collect()
    } else {
        match crate::resolve::resolve_installed(&manifest.items, parsed) {
            Ok(it) => vec![it.key().into()],
            // UNM-4: an exact ref that names no managed item may name an
            // unmanaged lobe item; a glob never sweeps unmanaged entries.
            Err(MindError::NotInstalled { .. }) => {
                let unmanaged_items = crate::unmanaged::scan(paths, manifest)?;
                let item = crate::unmanaged::resolve(&unmanaged_items, parsed)?;
                forget_unmanaged_single(item, yes)?;
                Vec::new()
            }
            Err(e) => return Err(e),
        }
    })
}

/// Remove the installed items `keys` (CLI-40..42): confirm a multi-item
/// removal, warn about dependents of a single one (DEP-60), run each item's
/// uninstall hooks, and save the manifest. `action` names the verb in the
/// output (`forget` or `prune`).
#[allow(clippy::too_many_arguments)]
fn forget_keys(
    paths: &Paths,
    manifest: &mut Manifest,
    keys: Vec<String>,
    item_ref: &str,
    action: &'static str,
    yes: bool,
    force: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let out = crate::render::ctx();
    // CLI-42: removing more than one item (typically a glob that matched more
    // broadly than intended) lists the matches and confirms first. `--yes` skips;
    // a non-TTY run without `--yes` refuses rather than removing silently.
    if keys.len() > 1 && !yes {
        if !out.json {
            println!("{action} would remove {} item(s):", keys.len());
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
        if let Err(e) = uninstall_item(paths, &item, &uninstall_hooks, &commit, accept) {
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
            let verb = if action == "prune" {
                "pruned"
            } else {
                "forgot"
            };
            println!("{} {verb} {key_display}", out.ok());
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
        let mut result = MutationResult::new(action, item_ref, "removed");
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
