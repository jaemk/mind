//! `mind sync`.

use super::*;

/// `mind sync [--upgrade] [--accept-hooks[=EVENTS]]` — fetch every source and
/// refresh its recorded commit. With `--upgrade`, an `upgrade` pass runs after
/// the refresh (reporting pending upgrades and prompting before applying,
/// exactly like `mind upgrade`), so one command both fetches upstream and
/// applies pending upgrades. `accept` is forwarded to the `upgrade` pass so
/// hooks can run unattended in CI; it is unused when `--upgrade` is absent.
///
/// spec: LIFE-49 -- M1: `yes` is the same global `--yes` the CLI already
/// threads into `mind upgrade`/`mind forget`; it is forwarded to the
/// `--upgrade` pass's `upgrade_pass` call below instead of being hardcoded to
/// `false`, so `sync --upgrade --yes` (and, since B1/LIFE-45, `sync --upgrade
/// --json --yes`) actually skips the confirmation instead of silently
/// cancelling or refusing.
/// `mind sync` over ALL sources: the historical entry point, kept for callers
/// that never scope the sync (e.g. the TUI's sync action). Delegates to
/// [`sync_with_selector`] with no selector.
pub fn sync(
    paths: &Paths,
    then_upgrade: Option<NewItems>,
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    sync_with_selector(paths, None, then_upgrade, yes, accept)
}

/// Resolve a `[source]` filter (CLI-231) to the names of every melded source it
/// matches: exactly, by trailing suffix, or by glob. A filter that matches
/// nothing is a `SourceNotFound`, so a typo is reported rather than silently
/// selecting nothing. Shared by `sync <source>` and `upgrade --source` (CLI-244).
pub(super) fn select_sources(registry: &Registry, sel: &str) -> Result<Vec<String>> {
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
    Ok(names)
}

/// `mind sync [source]` — refresh melded source clones and catalogs. With a
/// `[source]` selector, only matching sources are fetched (CLI-231); with none,
/// every source is (CLI-50). `then_upgrade` is `--upgrade`, carrying its
/// `--skip-new` / `--ignore-new` mode (LIFE-55).
pub fn sync_with_selector(
    paths: &Paths,
    source_selector: Option<&str>,
    then_upgrade: Option<NewItems>,
    yes: bool,
    accept: crate::hook::Accept,
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
    // spec: LIFE-53 -- before the fetch, as `upgrade` does.
    record_whole_sources(paths, None)?;
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
                    // spec: VER-101
                    if same_declared_pin(&old_pin, &am.pin) {
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
                // Not top-level: skip (not error) if a same-URL entry is already
                // present. No install hook, curator, or item link; provisioned by
                // policy, not by a curator (STO-82).
                match meld_recursive(
                    &mut MeldState {
                        paths,
                        registry: &mut registry,
                        visited: &mut visited,
                        policy: Some(policy),
                        // auto-meld is non-TTY, so its hooks take the HOOK-22 skip path
                        accept: crate::hook::Accept::default(),
                        prefer_ssh,
                        skipped: &mut sync_skipped,
                        accept_newer_mind: false, // spec: DSC-116 -- no one to ask; refuse
                    },
                    MeldRequest {
                        consumer_pin: PinRequest::Follow(am.pin.clone()), // POL-55: apply the policy pin verbatim
                        yes: true, // auto-meld is non-interactive (no collision prompt)
                        ..MeldRequest::new(&am.repo, &mut MeldProfiles::None)
                    },
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
    let selected: Option<Vec<String>> = source_selector
        .map(|sel| select_sources(&registry, sel))
        .transpose()?;
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
    // POL-12: with the allowlist locked, a source whose identity is no longer
    // allowed is not synced.
    let blocked = |source: &crate::source::Source| {
        policy
            .as_ref()
            .is_some_and(|p| p.lock() && !p.allow_matches(&source.base_identity()))
    };
    // spec: CLI-248 -- fetch every source in scope in parallel up front; the
    // loop below reports and records the results in registry order.
    let to_sync: Vec<&crate::source::Source> = registry
        .sources
        .iter()
        .filter(|s| in_scope(&s.name) && !blocked(s))
        .collect();
    let mut results = refresh_sources(paths, &to_sync).into_iter();
    for source in &mut registry.sources {
        // spec: CLI-231 -- skip sources outside a `[source]` selector's scope.
        if !in_scope(&source.name) {
            continue;
        }
        // POL-12: report a blocked source and skip it (the rest still sync).
        if blocked(source) {
            if !out.json {
                println!(
                    "{} skipping {}: source not permitted by the managed policy's allowlist",
                    out.warn(),
                    source.name
                );
            }
            continue;
        }
        let Some(fetched) = results.next() else {
            break;
        };
        if !out.json {
            print!("{} syncing {} ... ", out.bullet(), source.name);
            let _ = std::io::stdout().flush();
        }
        let refreshed = fetched.and_then(|(new_commit, pin)| {
            let changed = source.commit.as_deref() != Some(new_commit.as_str());
            // spec: DSC-94 -- B4: sanitize at the sync-time re-read too.
            let desc = MindToml::load(&source.clone_dir(paths))?
                .and_then(|mt| mt.source.description)
                .map(|d| strip_ansi(&d));
            Ok((new_commit, changed, desc, pin))
        });
        match refreshed {
            Ok((new_commit, changed, desc, pin)) => {
                source.pin = pin;
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
    // spec: VER-91
    let version_rows = source_versions(&registry, &in_scope);

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
                // spec: DSC-88 -- the same lift a fresh meld applies, so
                // `add-roots` is gated by DSC-60 exactly like `roots`.
                let curated = curated_config_for(&ns, &toml_path)?;
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
                    curated: CuratedConfig::default(),
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
            // No consumer roots, add-roots (DSC-88: gated, carried via
            // todo.curated), flat-skills, pin, or install hook on a re-walk.
            match meld_recursive(
                &mut MeldState {
                    paths,
                    registry: &mut registry,
                    visited: &mut visited,
                    policy: policy.as_ref(),
                    // sync is non-TTY: its hooks take the HOOK-22 skip path
                    accept: crate::hook::Accept::default(),
                    prefer_ssh,
                    skipped: &mut sync_skipped,
                    accept_newer_mind: false, // spec: DSC-116 -- no one to ask; refuse
                },
                MeldRequest {
                    alias: todo.alias,
                    yes: true, // sync re-walk is non-interactive (no collision prompt)
                    curated: Some(todo.curated),
                    item_kind: todo_kind,           // spec: DSC-100
                    curated_by: Some(todo_curator), // spec: STO-82
                    ..MeldRequest::new(&todo.spec, &mut MeldProfiles::None)
                },
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
                // spec: HOOK-172 -- a hook stop is not a clone failure.
                Err(e) if e.is_hook_stop() => return Err(e),
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
                        reason: nested_skip_reason(&e),
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
    let mut pruned: Vec<String> = Vec::new();
    let mut version_rows = version_rows;
    let mut breaking_moves: Vec<BreakingMoveJson> = Vec::new();
    if let Some(new_items) = then_upgrade {
        // spec: HOOK-11, HOOK-23 - sync already done above; run the pass with
        // no_sync to avoid a redundant fetch. Deprecated: prefer `mind upgrade`
        // (CLI-169).
        // spec: CLI-217 -- `upgrade_pass` (not the `upgrade` verb) so the
        // pass does not emit its own document: the caller
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
        let pass = upgrade_pass(
            paths,
            UpgradeOpts {
                no_sync: true,
                sources: selected.as_deref().map_or(SourceScope::All, |names| {
                    SourceScope::Names(names, source_selector)
                }),
                // spec: LIFE-55
                new_items,
                ..UpgradeOpts::new(yes, accept)
            },
        )?;
        if let Some(result) = pass {
            upgraded = result.installed;
            // spec: LIFE-58
            pruned = result.removed;
            // spec: VER-91 -- the pass may have settled a breaking release, so
            // its release state is the current one, and its breaking moves are
            // this run's.
            version_rows = result.sources;
            breaking_moves = result.breaking;
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
        // sanitized (`upgrade_pass` pushes `display_key()`).
        result.installed = provisioned_keys.iter().map(|k| strip_ansi(k)).collect();
        result.installed.extend(upgraded);
        result.removed = pruned;
        result.sources = version_rows;
        result.breaking = breaking_moves;
        print_json(&result)?;
    }
    Ok(())
}

/// Sync sources relevant to the upgrade scope. Per-source failures are reported
/// and skipped (CLI-54 resilience); the upgrade pass uses whatever was refreshed.
/// `item_ref` limits the fetch to sources with a matching installed item, and
/// `names` to an explicit set (`upgrade --source`, CLI-244; curate.rs's CUR-2
/// refresh, which scopes the pre-plan fetch to curators and curated sources).
/// Both filters must agree a source is in scope; `None` for either one imposes
/// no restriction from it.
// spec: CLI-169
pub(crate) fn sync_sources_for_upgrade_scoped(
    paths: &Paths,
    registry: &mut Registry,
    item_ref: Option<&str>,
    names: Option<&HashSet<String>>,
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

    let should_sync = |name: &str| {
        in_scope.as_ref().is_none_or(|s| s.contains(name)) && names.is_none_or(|s| s.contains(name))
    };

    // spec: CLI-248 -- the fetches run in parallel; results apply in order.
    let to_sync: Vec<&crate::source::Source> = registry
        .sources
        .iter()
        .filter(|s| should_sync(&s.name))
        .collect();
    let mut results = refresh_sources(paths, &to_sync).into_iter();
    for source in &mut registry.sources {
        if !should_sync(&source.name) {
            continue;
        }
        let Some(refreshed) = results.next() else {
            break;
        };
        match refreshed {
            Ok((new_commit, pin)) => {
                source.pin = pin;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_parallel_fetches_parses_a_positive_integer_else_defaults() {
        // spec: CLI-249
        assert_eq!(parse_max_parallel_fetches(None), 8);
        assert_eq!(parse_max_parallel_fetches(Some("")), 8);
        assert_eq!(parse_max_parallel_fetches(Some("0")), 8);
        assert_eq!(parse_max_parallel_fetches(Some("-2")), 8);
        assert_eq!(parse_max_parallel_fetches(Some("many")), 8);
        assert_eq!(parse_max_parallel_fetches(Some("1")), 1);
        assert_eq!(parse_max_parallel_fetches(Some(" 32 ")), 32);
    }
}
