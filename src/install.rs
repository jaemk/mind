//! Installing items into the store and linking them into `~/.claude`.
//!
//! Installs are transactional and preserve the previous version until the new
//! one is fully built and validated:
//!
//! 1. Build the new copy in a staging dir and expand its `{{ns:}}` references
//!    there. The likeliest failure (a bad reference) happens here, while the
//!    live install is untouched.
//! 2. Move any existing store copy aside to a backup, then move staging into
//!    place and ensure the symlink.
//! 3. On any failure during the swap, restore from the backup. On success,
//!    drop the backup.
//!
//! Uninstall is driven by the per-item file registry recorded in the manifest
//! (`store` + `links`), so it removes exactly what was installed.

use std::path::Path;

use crate::catalog::CatalogItem;
use crate::error::{MindError, Result};
use crate::hash::hash_path;
use crate::manifest::InstalledItem;
use crate::namespace;
use crate::paths::{Paths, mkdir_p};

/// Install (or upgrade in place) one catalog item, returning its manifest record.
///
/// `commit` is the source's current commit; `siblings` is every catalog item in
/// the same source (including `item` itself), used to validate `{{ns:}}` name
/// tokens and to resolve the `{{self}}` / `{{tools:}}` / `{{path:}}` path tokens
/// to store paths. The recorded hash is of the *source* content so drift
/// detection compares like with like.
pub fn install(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &[CatalogItem],
    force: bool,
) -> Result<InstalledItem> {
    let kind = item.kind;
    let name = item.effective_name();
    let store = paths.store_item(kind, &name);
    let staging = paths.staging_path(kind, &name);
    let backup = paths.backup_path(kind, &name);

    // 0. Resolve where this item will link, and refuse up front to overwrite any
    //    target that is not mind's own symlink (a user's file/dir/foreign link).
    //    This runs before staging, so a clobber aborts touching nothing. With
    //    `force`, the guard is skipped and step 3 overwrites the conflicting
    //    target (LIFE-41).
    // A tool with no explicit `link` is store-only: no link target, so no agent
    // home symlink (the harness does not discover it; items reach it by token).
    let link_rel = item
        .link_rel
        .clone()
        .or_else(|| paths.default_link_rel(kind, &name));
    let store_root = paths.store_dir();
    let planned_links: Vec<std::path::PathBuf> = match &link_rel {
        Some(rel) => paths
            .agent_homes()?
            .iter()
            .map(|home| home.join(rel))
            .collect(),
        None => Vec::new(),
    };
    if !force {
        for link in &planned_links {
            ensure_unoccupied(&store_root, link)?;
        }
    }

    // 1. Stage and validate the new copy. Live install is untouched until step 2.
    remove_path(&staging)?;
    if let Some(parent) = staging.parent() {
        mkdir_p(parent)?;
    }
    copy_recursive(&item.path, &staging)?;
    if let Err(e) = expand_references(&staging, item, siblings, &store_root) {
        let _ = remove_path(&staging);
        return Err(e);
    }

    // 1b. Per-item build hook: build the item's tooling inside staging, before
    //     the swap, so a failed build is rolled back with staging and never
    //     touches the live install (HOOK-70..73). It is arbitrary code, so it is
    //     disclosed and prompted on a TTY; a non-TTY context skips it (the item
    //     installs unbuilt, HOOK-72).
    if let Some(build) = &item.build
        && let Err(e) = run_build_hook(item, build, &staging, commit)
    {
        let _ = remove_path(&staging);
        return Err(e);
    }

    // 2. Swap staging into the store, holding any prior copy in backup.
    if let Some(parent) = store.parent() {
        mkdir_p(parent)?;
    }
    let had_backup = store.exists();
    if had_backup {
        remove_path(&backup)?;
        if let Some(parent) = backup.parent() {
            mkdir_p(parent)?;
        }
        rename(&store, &backup)?;
    }
    if let Err(e) = rename(&staging, &store) {
        if had_backup {
            let _ = rename(&backup, &store); // restore previous version
        }
        return Err(e);
    }

    // 3. Link the store copy into every agent home (targets were checked free in
    //    step 0). On any failure, undo the links made so far and roll the store
    //    back.
    let mut links: Vec<std::path::PathBuf> = Vec::new();
    for link in planned_links {
        if let Err(e) = ensure_link(&store, &link) {
            for made in &links {
                let _ = remove_path(made);
            }
            let _ = remove_path(&store);
            if had_backup {
                let _ = rename(&backup, &store);
            }
            return Err(e);
        }
        links.push(link);
    }

    // 4. Success: drop the backup.
    if had_backup {
        let _ = remove_path(&backup);
    }

    Ok(InstalledItem {
        kind,
        name,
        bare_name: item.name.clone(),
        source: item.source.clone(),
        commit: commit.to_string(),
        hash: hash_path(&item.path)?,
        store: paths.store_rel(kind, &item.effective_name()),
        links: links
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        description: item.description.clone(),
    })
}

/// Remove an installed item using its recorded file registry (absolute link
/// paths across every agent home, then the store copy).
pub fn uninstall(paths: &Paths, item: &InstalledItem) -> Result<()> {
    for link in &item.links {
        remove_path(Path::new(link))?;
    }
    remove_path(&paths.mind_home.join(&item.store))?;
    Ok(())
}

/// Recreate any missing links for an installed item, pointing at its existing
/// store copy. Returns the number of links repaired. Used by `introspect --fix`.
/// If the store copy itself is gone there is nothing to link to, so it repairs
/// nothing (that is drift for `upgrade`/`learn` to resolve, not a re-link).
pub fn relink(paths: &Paths, item: &InstalledItem) -> Result<usize> {
    let store = paths.mind_home.join(&item.store);
    if !store.exists() {
        return Ok(0);
    }
    let mut fixed = 0;
    for link in &item.links {
        let link = Path::new(link);
        if std::fs::symlink_metadata(link).is_err() {
            ensure_link(&store, link)?;
            fixed += 1;
        }
    }
    Ok(fixed)
}

/// Refuse to install over a link target that mind does not own. A target is
/// "ours" only if it is a symlink pointing into the store root; a regular file,
/// a directory, or a symlink elsewhere is the user's and is left untouched
/// (`LinkOccupied`). A missing target is free.
fn ensure_unoccupied(store_root: &Path, link: &Path) -> Result<()> {
    match std::fs::symlink_metadata(link) {
        Ok(meta) => {
            let ours = meta.file_type().is_symlink()
                && std::fs::read_link(link).is_ok_and(|t| t.starts_with(store_root));
            if ours {
                Ok(())
            } else {
                Err(MindError::LinkOccupied {
                    path: link.display().to_string(),
                })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(MindError::io(link, e)),
    }
}

/// Create (or refresh) a symlink at `link` pointing to `store`.
fn ensure_link(store: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        mkdir_p(parent)?;
    }
    remove_path(link)?;
    symlink(store, link)
}

/// Rewrite reference tokens in every text file under the staged copy: the
/// `{{ns:name}}` name tokens, then the `{{self}}` / `{{tools:name}}` /
/// `{{path:ref}}` path tokens. Both resolve against `siblings` (every item in the
/// same source) and a bad reference in either pass aborts the staged install.
fn expand_references(
    root: &Path,
    item: &CatalogItem,
    siblings: &[CatalogItem],
    store_root: &Path,
) -> Result<()> {
    let names: std::collections::HashSet<String> =
        siblings.iter().map(|s| s.name.clone()).collect();
    let path_siblings: Vec<namespace::PathSibling> = siblings
        .iter()
        .map(|s| namespace::PathSibling {
            kind: s.kind,
            name: s.name.clone(),
            bin: s.resolved_bin(),
        })
        .collect();
    // TOOL-16: render store paths with a leading `~` when the store is under
    // home, so a token expands to a value a Claude permission glob can match.
    let home = dirs::home_dir();
    let ctx = namespace::PathCtx {
        store_root,
        home: home.as_deref(),
        prefix: &item.prefix,
        self_kind: item.kind,
        self_name: &item.name,
        siblings: &path_siblings,
    };

    let mut files = Vec::new();
    if root.is_dir() {
        collect_files(root, &mut files)?;
    } else {
        files.push(root.to_path_buf());
    }
    for file in files {
        // Skip anything that is not valid UTF-8 text.
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !content.contains("{{") {
            continue;
        }
        let bad_ref = |referent: String| MindError::BadReference {
            item: item.key(),
            referent,
            in_source: item.source.clone(),
        };
        let expanded = namespace::expand(&content, &item.prefix, &names)
            .map_err(|name| bad_ref(format!("{{{{ns:{name}}}}}")))?;
        let expanded = namespace::expand_paths(&expanded, &ctx).map_err(bad_ref)?;
        std::fs::write(&file, expanded).map_err(|e| MindError::io(&file, e))?;
    }
    Ok(())
}

/// Run an item's build hook in its staging directory. Disclosed and prompted on
/// a TTY (two-way: run, or skip and install unbuilt); a non-TTY context skips it
/// (HOOK-72). A non-zero exit is a hard stop (HOOK-71) the caller rolls back.
fn run_build_hook(item: &CatalogItem, build: &str, staging: &Path, commit: &str) -> Result<()> {
    let run = if !crate::hook::is_tty() {
        println!(
            "note: skipped build hook for {} in a non-interactive context; its tooling is not built",
            item.key()
        );
        false
    } else {
        let disclosure = crate::hook::disclosure_text(
            &item.source,
            "(per-item build)",
            commit,
            &staging.to_string_lossy(),
            build,
            None,
        );
        matches!(
            crate::hook::prompt_choice_optional(&disclosure)?,
            crate::hook::OptionalChoice::Run
        )
    };
    if run {
        println!("running build hook for {}", item.key());
        crate::hook::run_hook(build, staging, &item.source, "build")?;
    } else if crate::hook::is_tty() {
        println!(
            "note: skipped build hook for {}; its tooling is not built",
            item.key()
        );
    }
    Ok(())
}

fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| MindError::io(dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| MindError::io(dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).map_err(|e| MindError::io(to, e))
}

/// Best-effort removal of a file, dir, or symlink; "not found" is success.
fn remove_path(path: &Path) -> Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(MindError::io(path, e)),
    };
    let res = if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    res.map_err(|e| MindError::io(path, e))
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        mkdir_p(dst)?;
        let rd = std::fs::read_dir(src).map_err(|e| MindError::io(src, e))?;
        for entry in rd {
            let entry = entry.map_err(|e| MindError::io(src, e))?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            copy_recursive(&from, &to)?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            mkdir_p(parent)?;
        }
        std::fs::copy(src, dst).map_err(|e| MindError::io(dst, e))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| MindError::io(link, e))
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    // On non-unix, fall back to a copy so the layout still works. Known
    // limitation: a copied link is not recognized as mind's own by the clobber
    // guard (`ensure_unoccupied` keys on "symlink into the store"), so reinstall
    // / upgrade over it reports `LinkOccupied`. mind is unix-first; see the
    // platform-limitation note in spec/lifecycle.md.
    copy_recursive(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tool_item(build: &str, path: std::path::PathBuf) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Tool,
            name: "t".to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path,
            description: None,
            link_rel: None,
            bin: None,
            build: Some(build.to_string()),
        }
    }

    #[test]
    fn build_hook_is_skipped_without_a_tty() {
        // spec: HOOK-72
        // The test harness has no TTY, so the build hook must be skipped (the item
        // installs unbuilt): the command must not run, and the call still succeeds.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-build-skip-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        let marker = staging.join("built");
        let item = tool_item(
            &format!("touch {}", marker.display()),
            std::path::PathBuf::from("/src/tools/t"),
        );

        run_build_hook(&item, item.build.as_deref().unwrap(), &staging, "abc123").unwrap();
        assert!(
            !marker.exists(),
            "a non-TTY context must skip the build hook (HOOK-72)"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }
}
