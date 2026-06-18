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

use std::collections::HashSet;
use std::path::Path;

use crate::catalog::CatalogItem;
use crate::error::{MindError, Result};
use crate::hash::hash_path;
use crate::manifest::InstalledItem;
use crate::namespace;
use crate::paths::{Paths, mkdir_p};

/// Install (or upgrade in place) one catalog item, returning its manifest record.
///
/// `commit` is the source's current commit; `siblings` is the set of bare item
/// names in the same source, used to validate `{{ns:}}` reference tokens. The
/// recorded hash is of the *source* content so drift detection compares like
/// with like.
pub fn install(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &HashSet<String>,
) -> Result<InstalledItem> {
    let kind = item.kind;
    let name = item.effective_name();
    let store = paths.store_item(kind, &name);
    let staging = paths.staging_path(kind, &name);
    let backup = paths.backup_path(kind, &name);

    // 1. Stage and validate the new copy. Live install is untouched until step 2.
    remove_path(&staging)?;
    if let Some(parent) = staging.parent() {
        mkdir_p(parent)?;
    }
    copy_recursive(&item.path, &staging)?;
    if let Err(e) = expand_references(&staging, item, siblings) {
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

    // 3. Link the store copy into every agent home. On any failure, undo the
    //    links made so far and roll the store back.
    let link_rel = item
        .link_rel
        .clone()
        .unwrap_or_else(|| paths.default_link_rel(kind, &name));
    let mut links: Vec<std::path::PathBuf> = Vec::new();
    for home in paths.agent_homes()? {
        let link = home.join(&link_rel);
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

/// Create (or refresh) a symlink at `link` pointing to `store`.
fn ensure_link(store: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        mkdir_p(parent)?;
    }
    remove_path(link)?;
    symlink(store, link)
}

/// Rewrite `{{ns:name}}` tokens in every text file under the staged copy.
fn expand_references(root: &Path, item: &CatalogItem, siblings: &HashSet<String>) -> Result<()> {
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
        if !content.contains("{{ns:") {
            continue;
        }
        let expanded = namespace::expand(&content, &item.prefix, siblings).map_err(|referent| {
            MindError::BadReference {
                item: item.key(),
                referent,
                in_source: item.source.clone(),
            }
        })?;
        std::fs::write(&file, expanded).map_err(|e| MindError::io(&file, e))?;
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
    // On non-unix, fall back to a copy so the layout still works.
    copy_recursive(target, link)
}
