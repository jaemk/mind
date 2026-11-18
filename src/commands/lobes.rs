//! `mind config lobes` and `mind link-project`.

use super::*;

/// Backfill all installed items into `new_lobes`. Called after a lobe is added
/// so existing items also land in the new home.
///
/// spec: HARN-17 -- for a lobe registered in fan-out mode (today's only mode),
/// this runs unconditionally in every invocation mode (`--yes`, an interactive
/// TTY, a non-interactive non-TTY context, and `--json`): it never prompts and
/// never defers to `introspect --fix`. A pre-existing foreign file at a backfill
/// target is never clobbered unless `force` is set: `link_into_new_lobes` guards
/// each target with the same `ensure_unoccupied` check used at install time
/// (LIFE-41), and a blocked target surfaces as an ordinary failure (naming the
/// `--force` remedy) in the `errs` list printed below as a warning, rather than
/// overwriting it.
fn backfill_new_lobes(paths: &Paths, new_lobes: &[crate::paths::Lobe], force: bool) -> Result<()> {
    let mut manifest = Manifest::load(paths)?;
    if manifest.items.is_empty() {
        return Ok(());
    }

    for item in manifest.items.values_mut() {
        let (created, errs) = install::link_into_new_lobes(paths, item, new_lobes, force);
        item.links.extend(
            created
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned()),
        );
        for (p, e) in errs {
            // spec: CLI-217 -- HARN-17 runs this backfill under `--json` too, and
            // the caller prints its JSON result AFTER this loop, so a bare
            // `println!` here would land ahead of the envelope on stdout.
            crate::render::warn(format!("could not link {}: {e}", p.display()));
        }
    }
    manifest.save(paths)?;
    Ok(())
}

/// Convert detected-lobe candidates (name + config entry) into resolved
/// [`Lobe`]s for backfill (HARN-7). Each entry's path is used verbatim (the
/// detection base produces absolute paths) with its declared `kinds` filter.
fn candidates_to_lobes(
    candidates: &[(&'static str, crate::config::LobeEntry)],
) -> Vec<crate::paths::Lobe> {
    candidates
        .iter()
        .map(|(_, entry)| crate::paths::Lobe {
            path: std::path::PathBuf::from(entry.path()),
            kinds: entry.kinds().map(|ks| ks.to_vec()),
        })
        .collect()
}

/// Unified add path for `config lobes add` and `link-project`.
///
/// Calls `resolve_lobe(base, preset, subdir)` to get `(lobe, preset_opt)`.
///
/// - snapshot=true: materialize frozen real-file copies of admitted items into
///   the lobe path; do NOT register a config entry (HARN-12).
/// - snapshot=false (default): register the lobe in config, preserve HARN-9
///   claude_home, run the HARN-7 backfill offer, print gitignore guidance for
///   a project lobe (HARN-11).
// spec: HARN-10 HARN-11 HARN-12
pub fn lobe_add_resolved(
    paths: &Paths,
    base: Option<&str>,
    preset: Option<&str>,
    subdir: Option<&str>,
    snapshot: bool,
    force: bool,
    // Retained for call-site compatibility (main.rs dispatches positionally);
    // no longer read here. The HARN-7 backfill is unconditional in every mode
    // (HARN-17), so a confirmation flag no longer gates it.
    _yes: bool,
) -> Result<()> {
    use crate::paths::{Scope, resolve_lobe};

    let out = crate::render::ctx();
    // POL-40: a lobe lock pins the effective agent homes; refuse and change
    // nothing. Load the policy first so the refusal precedes any config write.
    if let Some(policy) = Policy::load()?
        && policy.lobes_lock()
    {
        return Err(lobes_locked_error("add"));
    }

    // When a preset name is given, validate it up-front via `preset_lobe`
    // (fast path for unknown-preset errors before any config write). The full
    // `resolve_lobe` call below re-uses the same lookup and handles `base`.
    if let Some(name) = preset {
        Paths::preset_lobe(name)?; // validate; result discarded, resolve_lobe handles base
    }
    let (lobe, preset_opt) = resolve_lobe(base, preset, subdir)?;
    let path_str = lobe.path.to_string_lossy().into_owned();

    if snapshot {
        // HARN-12: snapshot mode -- write frozen real-file copies, no config entry.
        let manifest = Manifest::load(paths)?;
        let agent_homes = paths.agent_homes()?;
        let mut copied = 0usize;
        let mut frozen_keys: Vec<String> = Vec::new();
        for item in manifest.items.values() {
            // Respect the lobe's kinds filter.
            if !lobe.admits(item.kind) {
                continue;
            }
            // Determine the relative link path (mirrors link_into_new_lobes).
            let link_rel = item
                .links
                .iter()
                .find_map(|link_str| {
                    let link = std::path::Path::new(link_str);
                    agent_homes
                        .iter()
                        .find_map(|h| link.strip_prefix(&h.path).ok())
                        .map(|rel| rel.to_string_lossy().into_owned())
                })
                .or_else(|| paths.default_link_rel(item.kind, &item.name));
            let link_rel = match link_rel {
                Some(r) => r,
                None => continue,
            };
            let dst = lobe.path.join(&link_rel);
            let store_src = paths.mind_home.join(&item.store);
            // Collision check (mirrors LIFE-41).
            if std::fs::symlink_metadata(&dst).is_ok() {
                if !force {
                    return Err(MindError::LinkOccupied {
                        path: dst.to_string_lossy().into_owned(),
                    });
                }
                // Force: remove the existing target before copying.
                if dst.is_dir() {
                    std::fs::remove_dir_all(&dst).map_err(|e| MindError::io(&dst, e))?;
                } else {
                    std::fs::remove_file(&dst).map_err(|e| MindError::io(&dst, e))?;
                }
            }
            install::copy_recursive(&store_src, &dst)?;
            copied += 1;
            // spec: DSC-95 -- `result.installed` below rides straight into the
            // `--json` envelope with no sanitizing step of its own (mirrors the
            // `learn()`/`sync` pattern of sanitizing before assigning to a
            // `--json` field), so sanitize here at collection.
            frozen_keys.push(item.key().display());
        }
        // spec: HARN-14 -- machine-readable snapshot result under --json, in place
        // of the prose lines. `outcome` is `snapshot` when anything was frozen,
        // else `no-op`; `count` and `installed` (sorted keys) report what was written.
        if out.json {
            frozen_keys.sort();
            let mut result = MutationResult::new(
                "lobe-add",
                &path_str,
                if copied == 0 { "no-op" } else { "snapshot" },
            );
            result.count = Some(copied);
            result.installed = frozen_keys;
            return print_json(&result);
        }
        if copied == 0 {
            println!("note: no installed items to snapshot into {path_str}");
        } else {
            println!("wrote {copied} frozen skill(s) to {path_str}");
            // Advisory: if the target is not inside a git repo, suggest committing.
            let is_git = lobe.path.join(".git").exists()
                || lobe.path.parent().is_some_and(|p| p.join(".git").exists());
            if !is_git {
                println!(
                    "note: {path_str} does not appear to be a git repo; \
                     commit the frozen copies to version-control them"
                );
            }
        }
        return Ok(());
    }

    // Managed (non-snapshot) path: register in config, backfill, gitignore note.
    //
    // spec: HARN-15 -- create the resolved lobe path on disk immediately, before
    // it is recorded in config. A preset base (e.g. `--preset gemini` before
    // Gemini CLI is installed) may not exist yet; the whole point of registering
    // a lobe ahead of its harness is that the lobe becomes reachable (STO-56) the
    // moment it is registered, so the HARN-17 backfill lands, the HARN-11
    // gitignore note names a real directory, and `introspect --fix` does not
    // treat it as vanished (HARN-13) and prune it on the next run. This does NOT
    // relax `resolve_lobe`'s existing `LobeBaseMissing` guard on an EXPLICIT base
    // (HARN-10): a base directory the user mistyped is still refused above,
    // before this point is ever reached.
    crate::paths::mkdir_p(&lobe.path)?;
    paths.ensure_config()?;
    let entry = crate::config::LobeEntry {
        path: path_str.clone(),
        kinds: lobe.kinds.clone(),
    };
    let mut cfg = Config::load(paths)?;
    if cfg.lobes.iter().any(|e| e.path() == path_str) {
        if out.json {
            return print_json(&MutationResult::new("lobe-add", &path_str, "no-op"));
        }
        println!("{} lobe already configured: {path_str}", out.available());
        return Ok(());
    }
    // spec: HARN-9 -- preserve the implicit claude_home default when this is
    // the first explicit lobe. Without this the default is silently replaced
    // by the configured list and new installs no longer reach ~/.claude.
    if cfg.lobes.is_empty() {
        let ch = paths.claude_home.to_string_lossy().into_owned();
        if ch.as_str() != path_str {
            cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
        }
    }
    cfg.lobes.push(entry.clone());
    cfg.save(paths)?;

    // Determine if this is a project-scoped lobe for gitignore guidance.
    let is_project_scope =
        preset_opt.is_some_and(|p| p.scope == Scope::Project) || subdir.is_some();

    if out.json {
        backfill_new_lobes(paths, std::slice::from_ref(&lobe), force)?;
        return print_json(&MutationResult::new("lobe-add", &path_str, "added"));
    }
    // Human output: print the added lobe, then gitignore guidance for project lobes.
    match preset_opt {
        Some(p) => {
            println!("{} added {} lobe {}", out.ok(), p.name, format_lobe(&entry));
        }
        None => {
            println!("{} added lobe {}", out.ok(), format_lobe(&entry));
        }
    }
    if is_project_scope {
        let skills_dir = lobe.path.join("skills");
        println!(
            "note: {}/  contains symlinks into ~/.mind/store; \
             add it to .gitignore so the symlinks are not committed",
            skills_dir.display()
        );
    }
    backfill_new_lobes(paths, std::slice::from_ref(&lobe), force)?;
    Ok(())
}

/// `mind config lobes add <path>` — add an agent home by path.
pub fn lobe_add(paths: &Paths, path: &str, yes: bool) -> Result<()> {
    lobe_add_resolved(paths, Some(path), None, None, false, false, yes)
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

/// `mind config lobes remove <path> [--snapshot]` — drop an agent home.
///
/// With `--snapshot` (HARN-12): for each manifest item whose recorded link is
/// confined under the lobe path, replace the symlink with a frozen real-file copy
/// of the store content, strip the link from the manifest, then drop the config
/// entry.
pub fn lobe_remove(paths: &Paths, path: &str, snapshot: bool) -> Result<()> {
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

    let mut frozen_count: Option<usize> = None;
    if snapshot {
        // HARN-12: freeze symlinks confined under the lobe before dropping the entry.
        let lobe_path = std::path::Path::new(path);
        let mut manifest = Manifest::load(paths)?;
        let mut frozen = 0usize;
        for item in manifest.items.values_mut() {
            let mut new_links = Vec::new();
            for link_str in &item.links {
                let link = std::path::Path::new(link_str);
                // Only process links confined under this lobe.
                if !link.starts_with(lobe_path) {
                    new_links.push(link_str.clone());
                    continue;
                }
                // Replace the symlink with a frozen real-file copy.
                let store_src = paths.mind_home.join(&item.store);
                // Remove the existing symlink (or whatever is there).
                if std::fs::symlink_metadata(link).is_ok() {
                    if link.is_dir() && !link.is_symlink() {
                        std::fs::remove_dir_all(link).map_err(|e| MindError::io(link, e))?;
                    } else {
                        std::fs::remove_file(link).map_err(|e| MindError::io(link, e))?;
                    }
                }
                install::copy_recursive(&store_src, link)?;
                frozen += 1;
                // The link path stays the same (now a real file/dir); keep it
                // in the manifest as a record of the on-disk location, but it
                // is no longer a mind-managed symlink. We strip it so a later
                // `forget` does not try to remove it (the user owns the copy).
                // (HARN-12: strip from manifest.)
            }
            item.links = new_links;
        }
        manifest.save(paths)?;
        frozen_count = Some(frozen);
        if !out.json {
            println!("frozen {frozen} link(s) in {path} to real files");
        }
    }

    cfg.save(paths)?;
    if out.json {
        // spec: HARN-14 -- a plain remove is `removed`; a `--snapshot` detach is
        // `detached` with `count` = the number of links frozen.
        let mut result = MutationResult::new(
            "lobe-remove",
            path,
            if frozen_count.is_some() {
                "detached"
            } else {
                "removed"
            },
        );
        result.count = frozen_count;
        return print_json(&result);
    }
    println!("{} removed lobe {path}", out.ok());
    Ok(())
}

/// `mind config lobes detect` — detect installed agent homes (lobes) and offer
/// to add their presets (HARN-5). Only global presets (gemini, codex,
/// universal, devin-cli) carry markers, and each is offered for auto-add with
/// confirmation or `--yes`; a project preset is never offered.
/// Honors the POL-40 lobe lock and dedups against the already-configured lobes.
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
    // spec: HARN-5 -- only global presets carry markers, so every candidate
    // is a global lobe.
    let mut global_candidates: Vec<(&'static str, crate::config::LobeEntry)> = Vec::new();
    for (name, lobe) in detected {
        let path = lobe.path.to_string_lossy().into_owned();
        if configured.contains(&path) || !seen.insert(lobe.path.clone()) {
            continue;
        }
        global_candidates.push((
            name,
            crate::config::LobeEntry {
                path,
                kinds: lobe.kinds.clone(),
            },
        ));
    }

    // Decide whether to mutate global candidates. With --yes, add
    // unconditionally. Without it, a TTY gets a confirm prompt; a non-TTY
    // reports only (HARN-5).
    let do_add = if global_candidates.is_empty() {
        false
    } else if yes {
        true
    } else if crate::hook::is_tty() {
        let names: Vec<String> = global_candidates
            .iter()
            .map(|(n, e)| format!("{n} ({})", format_lobe(e)))
            .collect();
        confirm(&format!("add detected lobe(s): {}?", names.join(", ")))?
    } else {
        false
    };

    if out.json {
        let detected_json: Vec<serde_json::Value> = global_candidates
            .iter()
            .map(|(name, entry)| {
                serde_json::json!({
                    "preset": name,
                    "path": entry.path(),
                    "kinds": entry.kinds().map(|ks| {
                        ks.iter().map(|k| k.as_str()).collect::<Vec<_>>()
                    }),
                    "scope": "global",
                })
            })
            .collect();
        // spec: HARN-5 -- kept, always empty, for scripts that read it.
        let guidance: Vec<String> = Vec::new();
        if do_add {
            // spec: HARN-9
            if cfg.lobes.is_empty() {
                let ch = paths.claude_home.to_string_lossy().into_owned();
                if !global_candidates
                    .iter()
                    .any(|(_, e)| e.path() == ch.as_str())
                {
                    cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
                }
            }
            for (_, entry) in &global_candidates {
                cfg.lobes.push(entry.clone());
            }
            cfg.save(paths)?;
            let new_lobes = candidates_to_lobes(&global_candidates);
            // HARN-17: unconditional backfill; no --force plumbing for `detect`,
            // so a foreign target is reported (never clobbered).
            backfill_new_lobes(paths, &new_lobes, false)?;
        }
        return print_json(&serde_json::json!({
            "action": "lobe-detect",
            "detected": detected_json,
            "added": do_add,
            "guidance": guidance,
        }));
    }

    if global_candidates.is_empty() {
        println!("{} no new agent homes (lobes) detected", out.bullet());
        return Ok(());
    }

    if do_add {
        // spec: HARN-9
        if cfg.lobes.is_empty() {
            let ch = paths.claude_home.to_string_lossy().into_owned();
            if !global_candidates
                .iter()
                .any(|(_, e)| e.path() == ch.as_str())
            {
                cfg.lobes.push(crate::config::LobeEntry::bare(&ch));
            }
        }
        for (name, entry) in &global_candidates {
            cfg.lobes.push(entry.clone());
            println!("{} added {name} lobe {}", out.ok(), format_lobe(entry));
        }
        cfg.save(paths)?;
        let new_lobes = candidates_to_lobes(&global_candidates);
        // HARN-17: unconditional backfill; no --force plumbing for `detect`, so a
        // foreign target is reported (never clobbered).
        backfill_new_lobes(paths, &new_lobes, false)?;
    } else if !global_candidates.is_empty() {
        println!("{} detected agent home(s) (lobes):", out.bullet());
        for (name, entry) in &global_candidates {
            println!("  {} {name}: {}", out.dim("·"), format_lobe(entry));
        }
        println!("re-run with --yes to add them");
    }
    Ok(())
}

/// Build the refusal error for a locked `config lobes <action>` (POL-40). The
/// effective agent homes are pinned by `[lobes].lock`, so the action changes
/// nothing.
fn lobes_locked_error(action: &str) -> MindError {
    MindError::LobesLocked {
        action: action.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

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
}
