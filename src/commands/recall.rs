//! `mind recall`.

use super::*;

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
        let catalog = {
            let _step = crate::progress::step_if_idle(|| "scanning melded sources".to_string());
            let mut catalog = Vec::new();
            for source in &registry.sources {
                match catalog::scan_one(paths, source) {
                    Ok(items) => catalog.extend(items),
                    Err(e) => crate::render::scan_warn(strip_ansi(&format!(
                        "warning: could not scan source '{}': {e}",
                        source.name
                    ))),
                }
            }
            catalog
        };
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
        // The manifest still knows what is installed when an item is absent
        // from its source's current catalog.
        let missing_items = || {
            manifest
                .items
                .values()
                .filter(|item| !graph.contains_key(item.key().as_str()))
        };

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
                let mut roots = graph.forest_nodes();
                roots.extend(
                    missing_items()
                        .map(|item| crate::deps::DepNode::normal(item.display_key(), vec![])),
                );
                return print_json(&roots);
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
            let mut missing = missing_items().peekable();
            if forest.is_empty() && missing.peek().is_none() {
                println!("no installed items");
            } else {
                print!("{forest}");
                for item in missing {
                    println!("- {}", item.display_key());
                }
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
        let policy = policy_for_display(paths);
        if json {
            // spec: PROF-29 -- each source carries its declared profiles and
            // the enabled ones; a source without profiles is unchanged.
            let docs = registry
                .sources
                .iter()
                .map(|s| {
                    let mut v = serde_json::to_value(s)
                        .map_err(|e| MindError::json("recall sources", e))?;
                    if let Some(obj) = v.as_object_mut() {
                        insert_profile_json(paths, s, obj);
                        // spec: VER-91
                        if let Some(version) = version_json(&s.pin) {
                            obj.insert(
                                "version".into(),
                                serde_json::to_value(version)
                                    .map_err(|e| MindError::json("recall sources", e))?,
                            );
                        }
                        // spec: AUP-11 -- replaces the bare grant the
                        // registry serializes.
                        obj.insert(
                            "auto_update".into(),
                            serde_json::to_value(auto_update_json(
                                s,
                                auto_update_cap(policy.as_ref(), s),
                            ))
                            .map_err(|e| MindError::json("recall sources", e))?,
                        );
                        // spec: HOME-11 -- the mind home the source lives in.
                        obj.insert(
                            "mind_home".into(),
                            serde_json::Value::String(
                                crate::paths::normalize_home(&paths.home_of(&s.name))
                                    .display()
                                    .to_string(),
                            ),
                        );
                        // spec: LOBE-9 -- the lobe in effect, the consumer's or
                        // the source's own; `lobe` stays the consumer's as given.
                        obj.insert(
                            "lobe_path".into(),
                            match paths.effective_source_lobe(&s.name).ok().flatten() {
                                Some(root) => {
                                    serde_json::Value::String(crate::sanitize::display_path(&root))
                                }
                                None => serde_json::Value::Null,
                            },
                        );
                    }
                    Ok(v)
                })
                .collect::<Result<Vec<_>>>()?;
            // spec: CLI-167 - array wrapped in versioned envelope.
            return print_json_envelope(&docs);
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
                let hook = hook_token(s);
                // MKT-10: show the manifest origin so a native-plugin source is
                // distinguishable from a convention or mind.toml source.
                let origin = origin_label(s.origin);
                // spec: VER-90
                let release = version_token(&s.pin);
                // spec: AUP-11
                let auto = auto_update_token(s, auto_update_cap(policy.as_ref(), s));
                // spec: HOME-11 -- a source in an extra home names it.
                let home = match paths.source_home(&s.name) {
                    Some(h) => format!(" home:{}", h.display()),
                    None => String::new(),
                };
                // spec: LOBE-9 -- a source with a lobe in effect names it.
                let lobe = match paths.effective_source_lobe(&s.name).ok().flatten() {
                    Some(root) => format!(" lobe:{}", crate::sanitize::display_path(&root)),
                    None => String::new(),
                };
                vec![
                    out.bullet(),
                    s.name.clone(),
                    out.dim(&s.url),
                    out.dim(&format!(
                        "[{commit}{release}{auto}{ns}{hook}{origin}{home}{lobe}]"
                    )),
                    s.description.clone().unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>();
        out.print_rows(&rows);
        // spec: VER-82 VER-90
        for s in &registry.sources {
            for mark in version_marks(s) {
                println!("{mark}");
            }
            if let Some(hint) = version_pin_hint(paths, s) {
                println!("{}", out.dim(&hint));
            }
        }
        // spec: PROF-29
        print_profile_block(paths, &registry.sources.iter().collect::<Vec<_>>());
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
            let mut doc = serde_json::to_value(found.sanitized_for_display())
                .map_err(|e| MindError::json("recall item", e))?;
            // spec: WF-60 -- a workflow answers to its `meta.name`, which the
            // manifest does not record, so a `--json` consumer had no way to
            // learn of a WF-24 divergence or a WF-29 collision that the text
            // view printed right below. Same findings, same wording, one
            // serialized field each.
            if found.kind == ItemKind::Workflow
                && let Some(obj) = doc.as_object_mut()
            {
                let detail = workflow_detail(paths, &manifest, found);
                // spec: WF-62 -- `whenToUse` as its own key, beside the bare
                // `description` the manifest records; omitted when absent.
                if let Some(when) = &detail.when_to_use {
                    obj.insert(
                        "when_to_use".to_string(),
                        serde_json::Value::String(crate::sanitize::strip_ansi(when)),
                    );
                }
                obj.insert(
                    "harness_name".to_string(),
                    match &detail.harness_name {
                        Some(name) => serde_json::Value::String(crate::sanitize::strip_ansi(name)),
                        None => serde_json::Value::Null,
                    },
                );
                obj.insert(
                    "workflow_findings".to_string(),
                    serde_json::Value::Array(
                        detail
                            .findings
                            .iter()
                            .map(|f| serde_json::Value::String(crate::sanitize::strip_ansi(f)))
                            .collect(),
                    ),
                );
            }
            return print_json(&doc);
        }
        println!("{}", out.bold(&found.display_key()));
        if let Some(d) = &found.description {
            println!("  {}{d}", out.dim("desc    "));
        }
        println!("  {}{}", out.dim("source  "), found.source);
        // spec: PROF-29 -- the profiles the item was installed for.
        if !found.profiles.is_empty() {
            let also = if found.explicit {
                " (also learned by name)"
            } else {
                ""
            };
            println!(
                "  {}{}{also}",
                out.dim("profile "),
                found
                    .profiles
                    .iter()
                    .map(|p| strip_ansi(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
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
        // spec: WF-24 WF-29 -- a workflow answers to its `meta.name`, not to the
        // name printed above, so the two are shown together when they differ.
        // This is a property of the installed item, not a one-off install
        // message, so it belongs in the detail view the way LNK-19's dropped
        // requirement does. Deliberately absent from the `recall` LISTING and
        // from `introspect`: the divergence is legal and nothing mind can
        // repair, so it would be unactionable noise in the one command whose
        // output is meant to be acted on.
        if found.kind == ItemKind::Workflow {
            for msg in workflow_detail(paths, &manifest, found).findings {
                println!("  {}{}", out.dim("harness "), out.yellow(&msg));
            }
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
                        let mut row = serde_json::json!({
                            // DSC-95: the key embeds a source-controlled name.
                            "key": it.display_key(),
                            "installed": inst.is_some(),
                            "commit": inst.map(|m| m.commit.clone()),
                        });
                        // spec: WF-62 -- an installed workflow's `whenToUse`
                        // comes from the store copy (what the harness reads);
                        // an available one's from the catalog.
                        let when = match inst {
                            Some(m) => installed_when_to_use(paths, m),
                            None => it
                                .when_to_use
                                .as_deref()
                                .filter(|_| it.kind == ItemKind::Workflow)
                                .map(crate::sanitize::strip_ansi),
                        };
                        insert_when_to_use(&mut row, when);
                        row
                    })
                    .collect();
                for m in orphans_of(s) {
                    let mut row = serde_json::json!({
                        "key": m.display_key(),
                        "installed": true,
                        "commit": m.commit.clone(),
                        "orphaned": true,
                    });
                    // spec: DSC-123 -- still shipped, but its manifest won't parse.
                    if catalog::dropped_as_unreadable(&m.source, m.kind, &m.bare_name) {
                        row["unreadable"] = serde_json::Value::Bool(true);
                    }
                    insert_when_to_use(&mut row, installed_when_to_use(paths, m));
                    rows.push(row);
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

    let display_policy = policy_for_display(paths);
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
        let hook = hook_token(s);
        // MKT-10: show the manifest origin in the source header so the
        // provenance is visible in the default recall view too.
        let origin = origin_label(s.origin);
        // spec: VER-90
        let release = version_token(&s.pin);
        // spec: AUP-11
        let auto = auto_update_token(s, auto_update_cap(display_policy.as_ref(), s));
        println!(
            "{} {}  {}{}",
            out.bullet(),
            out.bold(&s.name),
            out.dim(&format!("[{commit}{release}{auto}{ns}{hook}{origin}]")),
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
                    // spec: CLI-261 -- point at the command that clears it, as
                    // the outdated marker points at `mind upgrade`. DSC-123: an
                    // item skipped for an unreadable manifest is not removed.
                    if catalog::dropped_as_unreadable(&m.source, m.kind, &m.bare_name) {
                        out.yellow("(its manifest upstream is unreadable)")
                    } else {
                        out.yellow("(removed upstream; run mind prune)")
                    }
                ),
            ]);
        }
        out.print_rows(&rows);
        // spec: VER-82 VER-90
        for mark in version_marks(s) {
            println!("  {mark}");
        }
        if let Some(hint) = version_pin_hint(paths, s) {
            println!("  {}", out.dim(&hint));
        }
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
