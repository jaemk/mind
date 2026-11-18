//! `mind absorb`: claim an unmanaged lobe item into a managed source.

use super::*;

/// The catalog item matching an installed item by stable identity (source, kind,
/// bare name), used to read its live uninstall hooks (HOOK-82/86; nothing is
/// recorded for them, HOOK-84). `None` when the item is gone from the catalog.
pub(super) fn item_catalog_match<'a>(
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
/// - skill   -> `skills/<name>/`  (returns a directory path)
/// - agent   -> `agents/<name>.md`
/// - rule    -> `rules/<name>.md`
/// - command -> `commands/<name>.md` (CMD-8)
/// - workflow -> `workflows/<name>.js` (WF-50)
///
/// A tool or resource is never unmanaged (each is store-only, so it is never
/// linked into a lobe for `absorb` to find), and panics.
fn convention_path_in_root(
    root: &std::path::Path,
    kind: ItemKind,
    name: &str,
) -> std::path::PathBuf {
    match kind {
        ItemKind::Skill => root.join("skills").join(name),
        ItemKind::Agent => root.join("agents").join(format!("{name}.md")),
        ItemKind::Rule => root.join("rules").join(format!("{name}.md")),
        // spec: CMD-8
        ItemKind::Command => root.join("commands").join(format!("{name}.md")),
        // spec: WF-50
        ItemKind::Workflow => root.join("workflows").join(format!("{name}.js")),
        ItemKind::Tool => panic!("tools are never unmanaged; absorb should not reach this"),
        // spec: RES-8
        ItemKind::Resource => {
            panic!("resources are never unmanaged; absorb should not reach this")
        }
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

/// Build `absorb`'s reported `kind:name` key (the human "absorbed ... -> managed
/// as {key}" line and the `--json` `result.key` field) from the item's kind and
/// its destination-prefixed effective name.
///
/// spec: DSC-95 -- M7: `effective_name` is derived (via `crate::namespace::apply`)
/// from the unmanaged item's on-disk NAME, which comes off the lobe filesystem
/// and is NOT DSC-96-gated (that gate only runs at catalog-scan time for a
/// managed source's items, not for an unmanaged lobe entry `absorb` claims).
/// This used to compose the raw name straight into the printed/JSON key, right
/// alongside the ALREADY-sanitized `item.key().display()` used two lines away
/// in the same message -- one field sanitized, the other not, in the same
/// line. Sanitize here, at the one composition site, mirroring `item.key()
/// .display()`.
fn absorb_effective_key(kind: ItemKind, effective_name: &str) -> String {
    format!("{}:{}", kind.as_str(), strip_ansi(effective_name))
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
        .ok_or_else(|| MindError::NotInstalled {
            name: item.key().into(),
        })?
        .clone();
    let stray_paths: Vec<&std::path::PathBuf> = item.paths.iter().skip(1).collect();

    if !yes {
        // Print what we will do.
        if !out.json {
            println!("absorb will:");
            // spec: DSC-95
            println!(
                "  move  {} -> {}",
                display_path(&source_lobe_path),
                display_path(&dest_item_path)
            );
            for stray in &stray_paths {
                println!("  delete (stray copy) {}", display_path(stray));
            }
        }
        // C3 / ABS-7: json mode is non-interactive; treat it like non-TTY for
        // the destructive confirmation. A missing --yes refuses with
        // ConfirmationRequired regardless of whether a real TTY is attached.
        if !crate::hook::is_tty() || out.json {
            // spec: CLI-232
            return Err(MindError::ConfirmationRequired {
                action: json_confirmation_action(
                    format!("absorbing {}", item.display_key()),
                    out.json,
                ),
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
    let commit_msg = format!("absorb {}:{}", item.kind.as_str(), item.name);
    let git_err = (|| {
        crate::git::add_all(&dest_path)?;
        crate::git::commit(&dest_path, &commit_msg)
    })();
    if let Err(e) = git_err {
        // Restore: remove the dest copy we just made; source lobe is still intact.
        let _ = crate::install::remove_path(&dest_item_path);
        return Err(e);
    }

    // 3. ABS-1: meld the destination if not yet registered. Remember whether
    //    THIS call registered it, so a later learn failure can unwind the
    //    registration (ABS-12) without touching a pre-existing source.
    let dest_spec = dest_path.to_string_lossy().into_owned();
    let freshly_melded = !is_melded(paths, &dest_spec, None)?;
    if freshly_melded {
        let meld_err = meld(
            paths,
            &dest_spec,
            None,
            vec![],
            vec![],
            false,
            PinRequest::None,
            None,
            crate::hook::Accept::default(),
            None, // absorb melds a directory, never an item link
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
    //    Use a tmp path under the destination source's mind home (HOME-14),
    //    where its install writes anyway, so it survives an in-repo rename.
    let dest_source_name = parse_spec(&dest_spec)
        .map(|s| s.name)
        .unwrap_or_else(|_| dest_spec.clone());
    let backup = paths
        .home_of(&dest_source_name)
        .join(".tmp")
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
    let effective_key = absorb_effective_key(item.kind, &effective_name);

    // ABS-1 / ABS-8: learn the item under the destination source.
    // When `--json` is in effect use `learn_collecting` (no JSON emitted by
    // learn itself) so that absorb can emit its own single result (ABS-11).
    // In human mode the regular `learn()` path prints the "learned ..." line.
    let learn_ref = format!("{}:{}", item.kind.as_str(), effective_name);
    let qualified_ref = format!("{dest_source_name}#{learn_ref}");
    let learn_flow = InstallFlow {
        yes: true,               // already confirmed above
        clobber: Clobber::Force, // stray lobe copies handled by Force
        accept: crate::hook::Accept::default(),
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
        // spec: ABS-12 -- unwind a registration this call made (best-effort;
        // nothing was installed from it, learn failed) and name the dest
        // commit, which is left in place because the dest repo is the user's.
        if freshly_melded {
            let _ = unmeld_one(
                paths,
                &dest_source_name,
                false,
                true,
                crate::hook::Accept::default(),
                None,
            );
        }
        crate::render::warn(format!(
            "absorb left commit '{}' in {}; remove it with git if unwanted",
            strip_ansi(&commit_msg),
            display_path(&dest_path)
        ));
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
        item.key().display()
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
    // spec: CLI-257 CLI-258
    if confirm(&format!(
        "Save '{}' as absorb_to in config.toml?",
        dest.display()
    ))? {
        let mut config = Config::load(paths)?;
        config.absorb_to = Some(dest.to_string_lossy().into_owned());
        paths.ensure_layout()?;
        config.save(paths)?;
        println!("Saved absorb_to = '{}'", dest.display());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

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
        // spec: CMD-8 -- a command absorbs to the same shape as an agent/rule.
        assert_eq!(
            convention_path_in_root(root, ItemKind::Command, "ship"),
            PathBuf::from("/repo/commands/ship.md"),
            "command convention path is commands/<name>.md"
        );
        // spec: WF-50 -- a workflow is a one-file kind like a command, but its
        // extension is `.js`, not `.md`.
        assert_eq!(
            convention_path_in_root(root, ItemKind::Workflow, "deploy"),
            PathBuf::from("/repo/workflows/deploy.js"),
            "workflow convention path is workflows/<name>.js"
        );
    }

    /// M7: `absorb_effective_key` sanitizes the on-disk unmanaged-item name
    /// before composing it into the reported `kind:name` key. A hostile name
    /// (a control byte, an ANSI escape, or a bidi override) is NOT
    /// DSC-96-gated at this point -- that gate only runs at catalog-scan time
    /// for a managed source's items, not for an unmanaged lobe entry `absorb`
    /// claims -- so an end-to-end CLI drive of `absorb` on a hostile-named
    /// unmanaged item cannot exercise this: `absorb`'s internal `learn()`
    /// call re-scans the destination source's catalog, and DSC-96 skips the
    /// hostile-named item there before `absorb` ever reaches the success path
    /// this key is built for. This unit test exercises the composition
    /// directly instead, pinning the fix independent of that (unrelated,
    /// upstream) gate.
    // spec: DSC-95
    #[test]
    fn absorb_effective_key_sanitizes_a_hostile_name() {
        let hostile_name = format!("hand{}ma\u{202E}de", '\x07');
        let key = absorb_effective_key(ItemKind::Skill, &hostile_name);
        assert!(
            !key.contains('\x07') && !key.contains('\u{202E}'),
            "the composed key must not carry a raw control byte or bidi override: {key:?}"
        );
        assert_eq!(
            key, "skill:handmade",
            "the sanitized key must still read as the intended name"
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

    // ----- NS-30/CLI-161: set_source_namespace mutability lock -----

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
        let paths = Paths::new(base.join("mind"), base.join("claude"));
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

    // ---- CLI-212/CLI-213: a single-source scan does not degrade ----
}
