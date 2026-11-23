//! warnings

use super::*;

/// Warn when an unprefixed source spelled a namespace into a bare item name
/// (DSC-99): `commands/acme:deploy.md` from a source with no prefix installs
/// `acme:deploy`, the exact spelling a source prefixed `acme` produces for its
/// `deploy`, and no later surface can tell the two apart by name alone.
///
/// Emitted by the verbs where the user is deciding to trust a source (`meld`,
/// `learn`) and NOT from the scan itself: `catalog::scan` runs on every
/// read-only surface, including the `probe` TUI's ~1s poll tick, where writing
/// to stderr both repeats forever and corrupts the alternate screen.
///
/// spec: DSC-99 DSC-101
pub(super) fn warn_forged_namespaces<'a>(items: impl IntoIterator<Item = &'a CatalogItem>) {
    for item in items {
        if !item.forges_namespace() {
            continue;
        }
        // spec: DSC-95 -- both the name and the source are source-controlled, so
        // each field is sanitized before the line is composed.
        eprintln!(
            "warning: {} '{}' in source '{}' has no namespace prefix, but its bare name contains \
             ':', so it installs under a name a PREFIXED source would produce; check the source \
             column in `mind recall` before trusting it",
            item.kind.as_str(),
            item.display_name(),
            crate::sanitize::strip_ansi(&item.source)
        );
    }
}

/// Warn when a namespaced source references siblings in bare prose, which
/// prefixing will break unless rewritten as `{{ns:name}}` tokens. Scans every
/// text file of each item (the whole skill directory, or the agent/rule file),
/// matching the breadth of install-time `{{ns:}}` expansion.
pub(super) fn warn_unguarded_references(items: &[CatalogItem]) {
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
    // would be a false positive. The cross-kind shadow rule (a name that is both
    // an agent AND a non-agent sibling stays in the scan) lives in the one shared
    // definition, `namespace::bare_agent_names`.
    let bare =
        crate::namespace::bare_agent_names(items.iter().map(|it| (it.kind, it.name.as_str())));
    let siblings: std::collections::HashSet<String> = items
        .iter()
        .map(|it| it.name.clone())
        .filter(|name| !bare.contains(name))
        .collect();
    for item in items {
        let mut refs: Vec<String> = Vec::new();
        for file in crate::review::item_files(item) {
            // spec: DSC-90 DSC-108 DSC-125 -- the file's full content (the same
            // file install copies) up to the size cap, behind the no-follow
            // regular-file guard.
            if !is_regular_file_nofollow(&file) {
                continue;
            }
            let Ok(content) = crate::error::read_text_capped(&file) else {
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
pub(super) fn warn_agent_collisions(paths: &Paths, items: &[CatalogItem]) {
    // spec: NS-41
    let manifest = match Manifest::load(paths) {
        Ok(m) => m,
        Err(_) => return, // manifest unavailable; skip advisory rather than propagate
    };
    for item in items {
        if item.kind != ItemKind::Agent {
            continue;
        }
        // spec: LOBE-3 -- the lobes this item would actually link into.
        let Ok(agent_homes) = paths.agent_homes_for(&item.source) else {
            return;
        };
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
pub(super) fn agent_collision(
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
    // spec: LOBE-3
    let planned: Vec<std::path::PathBuf> = paths
        .agent_homes_for(&target.source)?
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

/// Warn about each freshly installed workflow the harness would skip (WF-30/31),
/// answer to under a name mind does not report (WF-24), or share a name with
/// (WF-29). Installs nothing back and fails nothing: every one of these is
/// advisory by construction (see `workflow_check`), so an unreadable manifest or
/// store file drops the warning rather than propagating.
///
/// Runs AFTER the install, against the STORE copies, which is what makes it
/// exact: `{{ns:}}` in a `meta.name` (WF-23) is already expanded there, so the
/// comparison is against the literal string the harness will read, with no
/// second expansion to keep in step with `install.rs`. It also means the WF-29
/// comparison set is the whole installed set, freshly written, including the
/// other items of this same closure.
///
/// Returns immediately when nothing in `installed_keys` is a workflow, which is
/// the common case: every warning below is keyed off one, so without one there
/// is nothing to report, and the scan of the whole manifest that builds the
/// WF-29 comparison set would be pure cost (it reads and parses every installed
/// workflow's store copy). Behavior is otherwise unchanged.
///
/// spec: WF-24 WF-29 WF-30 WF-31 WF-56 WF-59
pub(super) fn warn_workflows(paths: &Paths, manifest: &Manifest, installed_keys: &[String]) {
    let touches_workflow = installed_keys.iter().any(|key| {
        manifest
            .items
            .get(key.as_str())
            .is_some_and(|entry| entry.kind == ItemKind::Workflow)
    });
    if !touches_workflow {
        return;
    }

    // Every installed workflow, read ONCE: the tokens in a store copy are
    // already expanded, so the harness-facing name is a trim of `meta.name`.
    // One pass, one read per file -- the touched items' warnings below index
    // into this map rather than re-reading and re-parsing their store copies.
    let mut reads: std::collections::HashMap<&str, crate::workflow_check::WorkflowRead> =
        std::collections::HashMap::new();
    let mut claimed: Vec<(String, String)> = Vec::new();
    for entry in manifest.items.values() {
        if entry.kind != ItemKind::Workflow {
            continue;
        }
        let read = crate::workflow_check::read(&paths.mind_home.join(&entry.store));
        if let Some(name) = crate::workflow_check::harness_name(&read.meta) {
            claimed.push((name, entry.key().as_str().to_string()));
        }
        reads.insert(entry.name.as_str(), read);
    }
    // spec: WF-61 -- an unmanaged lobe workflow claims its name too: the
    // harness loads it like any other. Never a subject below (its key is never
    // in `touched`), only ever one of the others named.
    claimed.extend(unmanaged_workflow_claims(paths, manifest));
    // spec: WF-59 -- one group per colliding name, built once for the whole
    // installed set rather than rebuilt per claimant.
    let groups = crate::workflow_check::claim_groups(&claimed);

    let touched: std::collections::HashSet<&str> =
        installed_keys.iter().map(String::as_str).collect();
    for key in installed_keys {
        let Some(entry) = manifest.items.get(key.as_str()) else {
            continue;
        };
        if entry.kind != ItemKind::Workflow {
            continue;
        }
        let Some(read) = reads.get(entry.name.as_str()) else {
            continue;
        };
        // spec: WF-56 -- mind's own metadata cap is reported as mind's, not as
        // a verdict about the harness mind never read enough of the file to
        // reach.
        if let Some(notice) = crate::workflow_check::cap_notice(read) {
            eprintln!(
                "warning: {}: {notice} Installed anyway.",
                entry.display_key()
            );
        }
        // spec: WF-31 -- warn and keep the install. The item is already on disk
        // by the time this runs, which is the point: mind's reader disagreeing
        // with the harness's must not be able to decide an install.
        for reason in crate::workflow_check::skip_reasons(read) {
            eprintln!(
                "warning: {}: the harness will not load this workflow: {reason}; installed anyway",
                entry.display_key()
            );
        }
        // spec: WF-24 -- the remedy token in the message names the BARE name
        // (`entry.bare_name`), the only spelling `{{ns:}}` resolves.
        let harness = crate::workflow_check::harness_name(&read.meta);
        if let Some(msg) =
            crate::workflow_check::divergence(&entry.name, &entry.bare_name, harness.as_deref())
        {
            eprintln!("warning: {}: {msg}", entry.display_key());
        }
    }
    // spec: WF-29 WF-59 -- one warning per colliding NAME, not one per
    // claimant, and only for a name this run touched: an untouched pair was
    // reported by the run that installed it.
    for (name, claimants) in groups {
        // The subject is an item this run touched: the warning belongs to the
        // install that caused it to be printed.
        let Some(subject) = claimants.iter().find(|key| touched.contains(key.as_str())) else {
            continue;
        };
        let others: Vec<String> = claimants
            .iter()
            .filter(|key| *key != subject)
            .map(|key| crate::sanitize::strip_ansi(key))
            .collect();
        if others.is_empty() {
            continue;
        }
        eprintln!(
            "warning: {}: {}",
            crate::sanitize::strip_ansi(subject),
            crate::workflow_check::collision(&name, &others)
        );
    }
}

/// What `recall <item>` has to say about an installed workflow: the name the
/// harness answers to, and every finding about it.
///
/// spec: WF-60 -- one producer for both renderings. The text view prints each
/// finding on a `harness` line and the `--json` document serializes the same
/// strings, so a scripted consumer learns of a WF-24 divergence or a WF-29
/// collision on the same terms a reader does. The manifest records neither the
/// `meta.name` nor the resolved harness name, so without this the JSON document
/// was the one surface where the divergence was invisible.
pub(super) struct WorkflowDetail {
    /// The name the harness resolves this workflow by (WF-20), or `None` when
    /// mind read no usable one (absent, empty, unread, or WF-58-unusable).
    pub(super) harness_name: Option<String>,
    /// The findings, in the order the detail view prints them.
    pub(super) findings: Vec<String>,
    /// The store copy's `meta.whenToUse` (WF-62), for the `--json` document.
    pub(super) when_to_use: Option<String>,
}

pub(super) fn workflow_detail(
    paths: &Paths,
    manifest: &Manifest,
    found: &crate::manifest::InstalledItem,
) -> WorkflowDetail {
    let store = paths.mind_home.join(&found.store);
    let read = crate::workflow_check::read(&store);
    let harness_name = crate::workflow_check::harness_name(&read.meta);
    let mut findings: Vec<String> = Vec::new();
    // spec: WF-56 -- mind's own cap, reported as mind's own.
    if let Some(notice) = crate::workflow_check::cap_notice(&read) {
        findings.push(notice);
    }
    // spec: WF-30 WF-60 -- why the harness would not load it, the same reasons
    // `learn` and `upgrade` warn with. Only for a store copy mind actually
    // read: one it could not read at all (gone, a directory, not UTF-8) says
    // nothing about what the file declares, so it yields no finding rather
    // than a "no `meta`" verdict about bytes mind never saw.
    if store_copy_was_read(&store, &read) {
        for reason in crate::workflow_check::skip_reasons(&read) {
            findings.push(format!("the harness will not load this workflow: {reason}"));
        }
    }
    // spec: WF-24 -- the remedy names the bare name, the only spelling a
    // `{{ns:}}` token resolves (NS-11).
    if let Some(msg) =
        crate::workflow_check::divergence(&found.name, &found.bare_name, harness_name.as_deref())
    {
        findings.push(msg);
    }
    // spec: WF-29 WF-59 -- one report for the name, naming the other
    // claimants, capped in `workflow_check` rather than here.
    if let Some(harness) = &harness_name {
        let mut others: Vec<String> = manifest
            .items
            .values()
            .filter(|e| e.kind == ItemKind::Workflow && e.name != found.name)
            .filter(|e| {
                let other = crate::workflow_check::read(&paths.mind_home.join(&e.store));
                crate::workflow_check::harness_name(&other.meta).as_deref()
                    == Some(harness.as_str())
            })
            .map(|e| e.display_key())
            .collect();
        // spec: WF-61 -- unmanaged lobe workflows claim names too.
        others.extend(
            unmanaged_workflow_claims(paths, manifest)
                .into_iter()
                .filter(|(name, _)| name == harness)
                .map(|(_, key)| key),
        );
        if !others.is_empty() {
            findings.push(crate::workflow_check::collision(harness, &others));
        }
    }
    WorkflowDetail {
        harness_name,
        findings,
        when_to_use: read.meta.when_to_use.clone(),
    }
}

/// Whether mind actually read `store`'s text: the capped read either parsed
/// it, or refused it for size (WF-56, which [`crate::workflow_check::cap_notice`]
/// reports and `skip_reasons` already handles). A file that is gone, is not a
/// regular file, or is not UTF-8 was not read, so nothing is known about what
/// it declares.
///
/// A non-empty `meta` proves a read; only the empty case re-checks, so the
/// ordinary path costs no second read.
fn store_copy_was_read(
    store: &std::path::Path,
    read: &crate::workflow_check::WorkflowRead,
) -> bool {
    if read.over_mind_cap.is_some() || !read.meta.is_empty() {
        return true;
    }
    // `file_meta` folds a read failure into an empty `meta` (WF-5), so the
    // question "could mind read it" is asked of the capped reader directly.
    is_regular_file_nofollow(store) && crate::error::read_capped_metadata(store).is_ok()
}

/// The WF-29 claims of every UNMANAGED lobe workflow (WF-61): `(harness name,
/// "workflow:<name> (unmanaged)")` for each one whose `meta.name` mind can
/// read through the capped reader (WF-55) and use (WF-58).
///
/// Best-effort like every workflow check: a scan failure yields no claims.
/// The scan's own UNM-9 skip warnings are muted here, since the verbs that
/// list unmanaged items (`recall`, `probe`) already print them and a `learn`
/// warning block is the wrong place for them. Only a regular file (after
/// following a hand-placed symlink) is read, so a fifo in a lobe cannot stall.
fn unmanaged_workflow_claims(paths: &Paths, manifest: &Manifest) -> Vec<(String, String)> {
    let was_quiet = crate::render::scan_quiet();
    crate::render::set_scan_quiet(true);
    let scanned = crate::unmanaged::scan(paths, manifest);
    crate::render::set_scan_quiet(was_quiet);
    let Ok(items) = scanned else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|u| u.kind == ItemKind::Workflow)
        .filter_map(|u| {
            let file = u.paths.first()?;
            if !std::fs::metadata(file).is_ok_and(|m| m.is_file()) {
                return None;
            }
            let read = crate::workflow_check::read(file);
            let name = crate::workflow_check::harness_name(&read.meta)?;
            Some((name, format!("{} (unmanaged)", u.display_key())))
        })
        .collect()
}

/// An installed workflow's `meta.whenToUse`, read from its store copy (WF-62),
/// sanitized for `--json`. `None` for every other kind and when absent.
pub(super) fn installed_when_to_use(
    paths: &Paths,
    m: &crate::manifest::InstalledItem,
) -> Option<String> {
    if m.kind != ItemKind::Workflow {
        return None;
    }
    crate::workflow_check::read(&paths.mind_home.join(&m.store))
        .meta
        .when_to_use
        .as_deref()
        .map(crate::sanitize::strip_ansi)
}

/// Add `when_to_use` to a `recall --json` row object, only when present (WF-62).
pub(super) fn insert_when_to_use(row: &mut serde_json::Value, when: Option<String>) {
    if let (Some(when), Some(obj)) = (when, row.as_object_mut()) {
        obj.insert("when_to_use".to_string(), serde_json::Value::String(when));
    }
}

/// `upgrade`'s failure-path tail: persist what the batch applied, then warn
/// about the workflows among them, exactly as the success path does.
///
/// spec: LIFE-48 -- the save must not mask the root-cause error, so a save
/// failure is warned about rather than propagated; the caller returns the
/// original error.
///
/// spec: WF-24 WF-29 WF-30 WF-31 -- the warnings run here too. An item that
/// upgraded before a later one failed is LIVE on disk and recorded, and the
/// next `upgrade` finds it current and never looks at it again, so skipping
/// the warning would mean no run ever reports its defect. `learn` already
/// warns on its own failure path for this reason.
pub(super) fn save_and_warn_workflows(paths: &Paths, manifest: &Manifest, applied_keys: &[String]) {
    if let Err(se) = manifest.save(paths) {
        warn_manifest_save_also_failed(&se);
    }
    warn_workflows(paths, manifest, applied_keys);
}
