//! `mind probe` in plain-text mode.

use super::*;

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
        // spec: PROF-29 -- each row of a source that declares profiles
        // carries the source's `profiles` and `enabled_profiles`.
        let profile_docs: std::collections::BTreeMap<
            &str,
            serde_json::Map<String, serde_json::Value>,
        > = registry
            .sources
            .iter()
            .map(|s| {
                let mut obj = serde_json::Map::new();
                insert_profile_json(paths, s, &mut obj);
                (s.name.as_str(), obj)
            })
            .collect();
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
                    // spec: WF-62 -- the JSON carries the pair separately; the
                    // `<description> - <whenToUse>` join (WF-51) is human
                    // output only.
                    description: it.description.as_deref().map(crate::sanitize::strip_ansi),
                    when_to_use: it
                        .when_to_use
                        .as_deref()
                        .filter(|_| it.kind == ItemKind::Workflow)
                        .map(crate::sanitize::strip_ansi),
                    unmanaged: false,
                    dependencies,
                    source_profiles: profile_docs
                        .get(it.source.as_str())
                        .cloned()
                        .unwrap_or_default(),
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
                when_to_use: None,
                unmanaged: true,
                dependencies: Vec::new(),
                source_profiles: serde_json::Map::new(),
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
        // spec: WF-51
        let mut desc = summary(it.display_description().as_deref(), 60);
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
        if let Some(subtree) = catalog_graph.render_item_subtree(it) {
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
    // spec: PROF-29 -- the declared profiles of each source with a hit.
    let hit_sources: Vec<&crate::source::Source> = registry
        .sources
        .iter()
        .filter(|s| hits.iter().any(|(_, it)| it.source == s.name))
        .collect();
    print_profile_block(paths, &hit_sources);
    Ok(())
}

/// One `probe --json` row. `unmanaged` is omitted for managed (catalog) rows, so
/// the existing schema is unchanged; an unmanaged row sets it true with no
/// `hash` and an empty `source`. `dependencies` lists the direct dependency keys
/// for catalog (managed) rows (DEP-62); empty for unmanaged rows.
#[derive(Serialize)]
struct ProbeRow<'a> {
    pub(super) installed: bool,
    pub(super) kind: &'a str,
    pub(super) name: String,
    // spec: DSC-95 -- owned and sanitized (not `&'a str` borrowed
    // straight from the catalog item): the source name and description are
    // both source-controlled text.
    pub(super) source: String,
    pub(super) hash: Option<String>,
    /// The item's own description, never joined with `whenToUse` (WF-62).
    pub(super) description: Option<String>,
    /// A workflow's `meta.whenToUse` (WF-62). Omitted when absent, and for
    /// every other kind (the catalog carries it for workflows only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) when_to_use: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) unmanaged: bool,
    /// Direct dependency keys (DEP-62). Empty for unmanaged rows. Omitted when
    /// the vec is empty so existing consumers that do not need deps see no change.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) dependencies: Vec<String>,
    /// The row's source's declared `profiles` (name -> members) and
    /// `enabled_profiles` (PROF-29). Empty, so no keys, for a source that
    /// declares none and for unmanaged rows.
    #[serde(flatten)]
    source_profiles: serde_json::Map<String, serde_json::Value>,
}
