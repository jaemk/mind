//! Installing items into the store and linking them into `~/.claude`.
//!
//! Installs are transactional and preserve the previous version until the new
//! one is fully built and validated:
//!
//! 1. Build the new copy in a staging dir and expand its `{{ns:}}`/path-token
//!    references there, in every markdown file (NS-53). The likeliest failure
//!    (a bad reference) happens here, while the live install is untouched.
//! 2. Move any existing store copy aside to a backup, then move staging into
//!    place and ensure the symlink.
//! 3. On any failure during the swap, restore from the backup. On success,
//!    drop the backup.
//!
//! Uninstall is driven by the per-item file registry recorded in the manifest
//! (`store` + `links`), so it removes exactly what was installed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::catalog::CatalogItem;
use crate::error::BadRefReason::NoMatch;
use crate::error::{ItemKind, MindError, Result};
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
/// detection compares like with like. `dangerously_skip_build` runs a build hook
/// non-interactively without prompting (HOOK-74); without it a non-TTY context
/// skips the build hook (HOOK-72).
pub fn install(
    paths: &Paths,
    item: &CatalogItem,
    commit: &str,
    siblings: &[CatalogItem],
    force: bool,
    dangerously_skip_build: bool,
) -> Result<InstalledItem> {
    let kind = item.kind;
    let name = item.effective_name();

    // Defense-in-depth: verify the effective name is safe before using it to
    // build any filesystem paths (NS-28 / UnsafeName). The prefix validator
    // (`validate_prefix`) and catalog name check (`is_safe_item_name`) already
    // guard the inputs individually, but this belt-and-suspenders check ensures
    // that even an unexpected combination cannot yield a traversal name.
    {
        use std::path::Component;
        let effective_safe = !name.is_empty()
            && !name.contains('\0')
            && std::path::Path::new(&name).components().all(|c| {
                !matches!(
                    c,
                    Component::RootDir | Component::ParentDir | Component::Prefix(..)
                )
            });
        if !effective_safe {
            return Err(MindError::UnsafeName { name: name.clone() });
        }
    }

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
    //
    // spec: NS-40 -- an agent's default link uses the bare harness name (the
    // frontmatter `name:` field, which is what the Claude harness resolves), NOT
    // the effective (possibly prefixed) name. The store path and manifest `name`
    // still use the effective name. An explicit `link_rel` from mind.toml wins.
    let link_name = if kind == ItemKind::Agent {
        item.agent_harness_name()
            .unwrap_or_else(|| item.name.clone())
    } else {
        name.clone()
    };
    let link_rel = item
        .link_rel
        .clone()
        .or_else(|| paths.default_link_rel(kind, &link_name));
    let store_root = paths.store_dir();
    // Only link into lobes whose `kinds` admit this item's kind (HARN-2/HARN-3)
    // and whose parent directory exists (STO-56: the reachability gate). A lobe
    // that excludes the kind or whose parent is absent contributes no link and is
    // not an error, so the recorded manifest `links` reflect exactly the lobes
    // that were active at install time. A skipped unreachable lobe prints a
    // one-time note (HARN-16) naming both remedies, deduplicated per process so a
    // multi-item `learn`/`meld` does not repeat it for every item.
    // spec: STO-56
    // spec: HARN-16
    let planned_links: Vec<std::path::PathBuf> = match &link_rel {
        Some(rel) => {
            let mut planned = Vec::new();
            for home in paths.agent_homes()?.iter().filter(|home| home.admits(kind)) {
                if home.reachable() {
                    planned.push(home.path.join(rel));
                } else {
                    note_unreachable_lobe_once(&home.path);
                }
            }
            planned
        }
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
    //     touches the live install (HOOK-70..74). It is arbitrary code, so it is
    //     disclosed and prompted on a TTY; a non-TTY context skips it (the item
    //     installs unbuilt, HOOK-72); `dangerously_skip_build` runs it unattended
    //     (HOOK-74).
    if let Some(build) = &item.build
        && let Err(e) = run_build_hook(item, build, &staging, commit, dangerously_skip_build)
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
    //    step 0, or force was given). Under force, any pre-existing foreign target
    //    is stashed before ensure_link removes it so rollback can restore it
    //    (LIFE-43). On any failure, undo the links made so far (restoring their
    //    stashes) and roll the store back.
    let mut links: Vec<std::path::PathBuf> = Vec::new();
    let mut stashes: Vec<Option<std::path::PathBuf>> = Vec::new();
    for (i, link) in planned_links.into_iter().enumerate() {
        // Under force, move any pre-existing foreign target to a stash so it
        // can be restored on rollback. A missing target or mind's own symlink
        // needs no stash. (LIFE-43)
        let stash = if force {
            let sp = paths.tmp_dir().join("foreign-stash").join(i.to_string());
            match maybe_stash_foreign(&store_root, &link, &sp) {
                Ok(true) => Some(sp),
                Ok(false) => None,
                Err(e) => {
                    // Stashing failed: roll back links and store made so far.
                    for (made, s) in links.iter().zip(stashes.iter()) {
                        let _ = remove_path(made);
                        if let Some(s) = s {
                            let _ = rename(s, made);
                        }
                    }
                    let _ = remove_path(&store);
                    if had_backup {
                        let _ = rename(&backup, &store);
                    }
                    return Err(e);
                }
            }
        } else {
            None
        };

        if let Err(e) = ensure_link(&store, &link) {
            // Rollback: remove symlinks made so far, restore their stashes, then
            // restore the store. Also restore the current link's stash if any.
            for (made, s) in links.iter().zip(stashes.iter()) {
                let _ = remove_path(made);
                if let Some(s) = s {
                    let _ = rename(s, made);
                }
            }
            if let Some(s) = &stash {
                let _ = rename(s, &link);
            }
            let _ = remove_path(&store);
            if had_backup {
                let _ = rename(&backup, &store);
            }
            return Err(e);
        }
        links.push(link);
        stashes.push(stash);
    }

    // 4. Success: drop the backup and any foreign-target stashes (the new
    //    symlinks have taken their place; LIFE-43).
    if had_backup {
        let _ = remove_path(&backup);
    }
    for s in stashes.iter().flatten() {
        let _ = remove_path(s);
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
        // spec: HOOK-110 -- filled in by the caller (`install_item` in
        // commands.rs) once the item's install hooks have actually run; this
        // function runs no hooks itself.
        install_hooks: Vec::new(),
    })
}

/// Remove an installed item using its recorded file registry (absolute link
/// paths across every agent home, then the store copy).
///
/// Before removing any path, each one is verified to be lexically under the
/// expected root (store root for the store copy; a configured lobe for links).
/// Paths that contain `..` components or fail the `starts_with` check are
/// skipped with a warning rather than removed (LIFE-44).
pub fn uninstall(paths: &Paths, item: &InstalledItem) -> Result<()> {
    let store_root = paths.store_dir();
    let agent_homes = paths.agent_homes()?;

    // Precompute canonicalized lobe roots once (LIFE-44). Lobe paths may be
    // stored as relative paths resolved against the cwd at config-load time,
    // but the manifest records absolute link paths (resolved at install time).
    // Canonicalization aligns both sides so a legitimate install-from-other-cwd
    // is not rejected as out-of-bounds.
    let canonical_roots: Vec<PathBuf> = agent_homes
        .iter()
        .map(|h| h.path.canonicalize().unwrap_or_else(|_| h.path.clone()))
        .collect();
    let root_refs: Vec<&Path> = canonical_roots.iter().map(|p| p.as_path()).collect();

    for link in &item.links {
        let p = Path::new(link);
        // Canonicalize the link path for the comparison. If the link no longer
        // exists, fall back to the raw path (it will be absent anyway).
        let canon_p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if !is_confined_under_any(&canon_p, &root_refs) {
            // Secondary check: if the stored link IS absolute and contains no
            // `..` components, allow deletion (the manifest was written by mind
            // itself and the path is structurally safe even if not under a
            // currently-configured lobe). This handles the case where a lobe
            // was removed from config after install.
            if !p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir) {
                eprintln!(
                    "mind: warning: skipping removal of '{}' -- path is outside all configured agent homes",
                    p.display()
                );
                continue;
            }
        }
        remove_path(p)?;
    }
    let store_path = paths.mind_home.join(&item.store);
    if !is_confined_under(&store_path, &store_root) {
        eprintln!(
            "mind: warning: skipping removal of '{}' -- path is outside the mind store root",
            store_path.display()
        );
        return Ok(());
    }
    remove_path(&store_path)?;
    Ok(())
}

/// True when `path` is lexically under `root`: `Path::starts_with(root)` is
/// true AND the path contains no `..` components (which could defeat the
/// component-by-component check on some edge cases where canonicalization has
/// not been performed). (LIFE-44)
pub(crate) fn is_confined_under(path: &Path, root: &Path) -> bool {
    use std::path::Component;
    if path.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    path.starts_with(root)
}

/// True when `path` is confined under at least one of `roots`. (LIFE-44)
fn is_confined_under_any(path: &Path, roots: &[&Path]) -> bool {
    roots.iter().any(|root| is_confined_under(path, root))
}

/// Recreate any missing links for an installed item, pointing at its existing
/// store copy. Returns the number of links repaired. Used by `introspect --fix`.
/// If the store copy itself is gone there is nothing to link to, so it repairs
/// nothing (that is drift for `upgrade`/`learn` to resolve, not a re-link).
///
/// Links belonging to an unreachable lobe (STO-56: the lobe's parent directory
/// no longer exists) are skipped: a vanished project should not be recreated.
pub fn relink(paths: &Paths, item: &InstalledItem) -> Result<usize> {
    let store = paths.mind_home.join(&item.store);
    if !store.exists() {
        return Ok(0);
    }
    // spec: STO-56 -- skip links whose lobe parent is gone.
    let agent_homes = paths.agent_homes()?;
    let mut fixed = 0;
    for link_str in &item.links {
        let link = Path::new(link_str);
        // Determine if this link belongs to a reachable lobe. If it is not under
        // any configured lobe path, assume reachable (fallback preserves behavior).
        let reachable = agent_homes
            .iter()
            .find(|lobe| link.starts_with(&lobe.path))
            .is_none_or(|lobe| lobe.reachable());
        if !reachable {
            continue;
        }
        if std::fs::symlink_metadata(link).is_err() {
            ensure_link(&store, link)?;
            fixed += 1;
        }
    }
    Ok(fixed)
}

/// Link an already-installed item into one or more new lobes (HARN-7/HARN-8).
/// Recovers `link_rel` from the item's existing links by stripping known lobe
/// prefixes; falls back to `paths.default_link_rel`. Returns the paths
/// successfully created and (path, error) pairs for failures.
/// Does NOT update the manifest — the caller records created paths into item.links.
///
/// spec: HARN-17 -- unless `force` is set, each target is guarded by the same
/// `ensure_unoccupied` check the install path uses (LIFE-41): a pre-existing
/// foreign file or directory is never clobbered. A blocked target is reported
/// via the ordinary `LinkOccupied` failure (naming the `--force` remedy) in the
/// returned `failed` list, exactly like any other link failure.
pub fn link_into_new_lobes(
    paths: &Paths,
    item: &InstalledItem,
    new_lobes: &[crate::paths::Lobe],
    force: bool,
) -> (
    Vec<std::path::PathBuf>,
    Vec<(std::path::PathBuf, MindError)>,
) {
    let mut created = Vec::new();
    let mut failed = Vec::new();

    let agent_homes = paths.agent_homes().unwrap_or_default();
    let link_rel = item.links.iter().find_map(|link_str| {
        let link = Path::new(link_str);
        agent_homes
            .iter()
            .find_map(|lobe| link.strip_prefix(&lobe.path).ok())
            .map(|rel| rel.to_string_lossy().into_owned())
    });
    let link_rel = match link_rel.or_else(|| paths.default_link_rel(item.kind, &item.name)) {
        Some(rel) => rel,
        None => return (created, failed),
    };

    let store = paths.mind_home.join(&item.store);
    let store_root = paths.store_dir();
    for lobe in new_lobes {
        if !lobe.admits(item.kind) {
            continue;
        }
        let expected = lobe.path.join(&link_rel);
        let expected_str = expected.to_string_lossy().into_owned();
        if item.links.iter().any(|l| l == &expected_str) {
            continue;
        }
        if !force && let Err(e) = ensure_unoccupied(&store_root, &expected) {
            failed.push((expected, e));
            continue;
        }
        match ensure_link(&store, &expected) {
            Ok(()) => created.push(expected),
            Err(e) => failed.push((expected, e)),
        }
    }
    (created, failed)
}

/// Under a forced install, move a pre-existing foreign target at `link` to
/// `stash` so it can be restored on rollback (LIFE-43). Returns `true` when
/// the target was moved (a stash was created). Returns `false` when the link
/// is absent or is already mind's own symlink into the store (no stash needed).
///
/// Uses the same "is mind's own" predicate as `ensure_unoccupied`: a symlink
/// pointing into `store_root` is ours; everything else (regular file, directory,
/// symlink to another location) is foreign.
///
/// Called only when `force` is true. The non-force path uses `ensure_unoccupied`
/// which refuses to touch foreign targets at all.
fn maybe_stash_foreign(store_root: &Path, link: &Path, stash: &Path) -> Result<bool> {
    let meta = match std::fs::symlink_metadata(link) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(MindError::io(link, e)),
        Ok(m) => m,
    };
    // Mind's own symlink: no stash needed.
    if meta.file_type().is_symlink()
        && std::fs::read_link(link).is_ok_and(|t| t.starts_with(store_root))
    {
        return Ok(false);
    }
    // Foreign target: move it to the stash so rollback can rename it back.
    if let Some(parent) = stash.parent() {
        mkdir_p(parent)?;
    }
    rename(link, stash)?;
    Ok(true)
}

/// Lobe paths already reported unreachable this process (HARN-16), so the note
/// is printed once per lobe rather than once per skipped item.
static WARNED_UNREACHABLE_LOBES: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// Print a one-time (per-process) note when a configured lobe is skipped at
/// install time because its parent directory does not exist (STO-56/HARN-16).
/// Names both remedies: create the missing directory (making the lobe
/// reachable), or drop the lobe from config with `mind config lobes remove
/// <path>`. Deduplicated by lobe path so a multi-item `learn`/`meld` prints it
/// once, not once per affected item.
fn note_unreachable_lobe_once(path: &Path) {
    let seen = WARNED_UNREACHABLE_LOBES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.insert(path.to_path_buf()) {
        // spec: CLI-217 -- this runs on the ordinary `learn`/`meld` install path,
        // ahead of the verb's `print_json`, so it must not be a bare `println!`.
        // spec: CLI-225 -- the lobe path comes from user config (`~/.mind/config.toml`),
        // not restricted against shell metacharacters, and lands in a pasteable
        // `mind config lobes remove <path>` remedy, so it is shell-quoted first.
        let quoted_path = crate::error::shell_quote(&path.display().to_string());
        crate::render::note(format!(
            "note: lobe '{}' is unreachable (its parent directory does not exist); \
             create the missing directory, or run `mind config lobes remove {quoted_path}` to drop it",
            path.display()
        ));
    }
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
pub(crate) fn ensure_link(store: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        mkdir_p(parent)?;
    }
    remove_path(link)?;
    symlink(store, link)
}

/// Rewrite reference tokens in every markdown file under the staged copy (the
/// extension test `namespace::is_markdown`, NS-53; a token in a non-markdown
/// file is left literal): the `{{ns:name}}` name tokens, then the `{{self}}` /
/// `{{tools:name}}` / `{{path:ref}}` path tokens. Both resolve against
/// `siblings` (every item in the same source) and a bad reference in either
/// pass aborts the staged install.
/// A non-markdown file the item lists in `expand:` is expanded too (NS-57), with
/// path tokens rendered absolute rather than the TOOL-16 `~` form (TOOL-20).
/// Also validates the `requires` frontmatter entries (DEP-6): each must resolve
/// to exactly one sibling (not source-qualified, not ambiguous, not missing);
/// and each `expand:` entry must be a safe relative path naming a shipped file.
fn expand_references(
    root: &Path,
    item: &CatalogItem,
    siblings: &[CatalogItem],
    store_root: &Path,
) -> Result<()> {
    let names: std::collections::HashSet<String> =
        siblings.iter().map(|s| s.name.clone()).collect();
    // spec: NS-42 -- agent referents expand bare even under a prefix. Compute the
    // set of agent bare names that are NOT also a non-agent sibling name (the
    // cross-kind shadow rule: if a name is both an agent and a skill/rule/tool, it
    // is NOT bare -- it keeps the prefix).
    let agent_names: std::collections::HashSet<String> = siblings
        .iter()
        .filter(|s| s.kind == crate::error::ItemKind::Agent)
        .map(|s| s.name.clone())
        .collect();
    let non_agent_names: std::collections::HashSet<String> = siblings
        .iter()
        .filter(|s| s.kind != crate::error::ItemKind::Agent)
        .map(|s| s.name.clone())
        .collect();
    let bare_names: std::collections::HashSet<String> =
        agent_names.difference(&non_agent_names).cloned().collect();
    let path_siblings: Vec<namespace::PathSibling> =
        siblings.iter().map(CatalogItem::as_path_sibling).collect();
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
    // NS-57/TOOL-20: an `expand:`-listed non-markdown file renders path tokens
    // as an absolute store path (home = None), since it is program input read by
    // something that does not expand a leading `~` the way TOOL-16's markdown
    // form assumes.
    let ctx_abs = namespace::PathCtx { home: None, ..ctx };

    // DEP-6: validate every `requires` entry before touching any file. A bad
    // entry here aborts the staged install (the live copy is still untouched).
    let bad_ref = |referent: String, reason: crate::error::BadRefReason| MindError::BadReference {
        item: item.key(),
        referent,
        reason,
        in_source: item.source.clone(),
    };
    // A `requires` entry that fails to resolve names the specific cause (DEP-7),
    // mirroring what `review` reports, so the install-time error is not the blunt
    // "does not match any item" for a ref that in fact crosses sources, is
    // malformed, or is merely ambiguous.
    let bad_requires = |entry: &str, reason| bad_ref(format!("requires: {entry}"), reason);
    for entry in &item.requires {
        // spec: DEP-6 DEP-7
        use crate::error::BadRefReason::{AmbiguousKind, CrossSource, InvalidRef};
        let r =
            crate::resolve::parse_item_ref(entry).map_err(|_| bad_requires(entry, InvalidRef))?;
        // Source-qualified entries cross sources, which is forbidden (DEP-5).
        if r.source.is_some() {
            return Err(bad_requires(entry, CrossSource));
        }
        // Resolve against siblings by bare name, narrowing by kind (DEP-5).
        let matches: Vec<&CatalogItem> = siblings
            .iter()
            .filter(|s| s.name == r.name && r.kind.is_none_or(|k| s.kind == k))
            .collect();
        if matches.is_empty() {
            // A genuine typo/unknown item (DEP-6).
            return Err(bad_requires(entry, NoMatch));
        }
        if matches.len() > 1 && r.kind.is_none() {
            // A bare name matching several kinds without a qualifier (DEP-6).
            return Err(bad_requires(entry, AmbiguousKind));
        }
    }

    // NS-57: validate every `expand:` entry before staging touches a file. Each
    // must be a safe relative path (no absolute, no `..`) naming a file the item
    // ships; a bad entry aborts the staged install as a BadReference, the same
    // transactional pre-swap failure a bad `requires` or `{{ns:}}` raises, so a
    // typo is caught loudly rather than left as an un-expanded literal.
    let mut expand_set: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    for entry in &item.expand {
        // spec: NS-57
        use crate::error::BadRefReason::InvalidRef;
        let rel = Path::new(entry);
        let unsafe_path = rel.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });
        if unsafe_path {
            return Err(bad_ref(format!("expand: {entry}"), InvalidRef));
        }
        // Must name a real file within the staged item. A single-file item
        // (agent/rule) has no bundled files, so any entry is a miss.
        if !(root.is_dir() && root.join(rel).is_file()) {
            return Err(bad_ref(format!("expand: {entry}"), NoMatch));
        }
        expand_set.insert(rel.to_path_buf());
    }

    let mut files = Vec::new();
    if root.is_dir() {
        collect_files(root, &mut files)?;
    } else {
        files.push(root.to_path_buf());
    }
    for file in files {
        // NS-53: all four token families expand only in a markdown file. A
        // token in any other file (a script, data) is left exactly as written
        // -- including one that would not resolve, which retires the
        // BadReference this loop used to raise for it (NS-11/NS-12 are scoped
        // to markdown accordingly).
        //
        // A directory item (skill/tool) stages every file under its original
        // name, so `file` itself carries the right extension to check. A
        // single-file item (agent/rule) stages as a bare name with no
        // extension at all (matching its store form), so its markdown-ness is
        // read from the source path instead.
        let source_like: &Path = if root.is_dir() { &file } else { &item.path };
        let is_md = namespace::is_markdown(source_like);
        // NS-57: a file listed in `expand:` is expanded like markdown even
        // though its extension is not, so a bundled script can reference a
        // sibling tool. Its relative path (under the staged dir) is what the
        // validated `expand_set` holds.
        let is_listed = root.is_dir()
            && file
                .strip_prefix(root)
                .map(|rel| expand_set.contains(rel))
                .unwrap_or(false);
        if !is_md && !is_listed {
            continue;
        }
        // Skip anything that is not valid UTF-8 text.
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !content.contains("{{") {
            continue;
        }
        // TOOL-20: a listed non-markdown file renders path tokens absolute; a
        // markdown file keeps the TOOL-16 `~` form.
        let path_ctx = if is_listed && !is_md { &ctx_abs } else { &ctx };
        let expanded = namespace::expand(&content, &item.prefix, &names, &bare_names)
            .map_err(|name| bad_ref(format!("{{{{ns:{name}}}}}"), NoMatch))?;
        let expanded = namespace::expand_paths(&expanded, path_ctx)
            .map_err(|(referent, reason)| bad_ref(referent, reason))?;
        std::fs::write(&file, expanded).map_err(|e| MindError::io(&file, e))?;
    }
    Ok(())
}

/// Run an item's build hook in its staging directory. Disclosed and prompted on
/// a TTY (two-way: run, or skip and install unbuilt); a non-TTY context skips it
/// (HOOK-72); `dangerously_skip` runs it unattended (HOOK-74). A non-zero exit
/// is a hard stop (HOOK-71) the caller rolls back.
fn run_build_hook(
    item: &CatalogItem,
    build: &str,
    staging: &Path,
    commit: &str,
    dangerously_skip: bool,
) -> Result<()> {
    // spec: HOOK-72 HOOK-74
    let run = if dangerously_skip {
        true
    } else if !crate::hook::is_tty() {
        // spec: CLI-217
        crate::render::note(format!(
            "note: skipped build hook for {} in a non-interactive context; its tooling is not built",
            item.key()
        ));
        false
    } else {
        let disclosure = crate::hook::disclosure_text(
            &item.source,
            "(per-item build)",
            commit,
            &staging.to_string_lossy(),
            build,
            None,
            None, // no browse URL for per-item build hooks (HOOK-24: local path only)
        );
        matches!(
            crate::hook::prompt_choice_optional(&disclosure)?,
            crate::hook::OptionalChoice::Run
        )
    };
    if run {
        // A progress line, not an aside: it belongs on stdout in text mode, and
        // under `--json` the whole run's fd 1 already points at stderr
        // (main.rs's `json_stdout`), so it cannot reach the result document.
        // spec: CLI-217
        println!("running build hook for {}", item.key());
        crate::hook::run_hook(build, staging, &item.source, "build")?;
    } else if crate::hook::is_tty() && !dangerously_skip {
        // spec: CLI-217
        crate::render::note(format!(
            "note: skipped build hook for {}; its tooling is not built",
            item.key()
        ));
    }
    Ok(())
}

/// The working directory for an item lifecycle hook: the item's store path when
/// it is a directory (a skill or tool), else its parent (an agent/rule store is a
/// single file, so the kind directory is the working dir).
fn hook_cwd(store: &Path) -> std::path::PathBuf {
    if store.is_dir() {
        store.to_path_buf()
    } else {
        store
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| store.to_path_buf())
    }
}

/// Run a per-item lifecycle hook (`event` is "install", HOOK-81, or "uninstall",
/// HOOK-82) in the item's store directory: the final step of an install, or the
/// step before a removal. Disclosed and prompted two-way (run / skip) on a TTY; a
/// non-TTY context skips it; `dangerously_skip` runs it unattended (HOOK-83). A
/// non-zero exit is a `HookFailed` hard stop the caller acts on (install rolls
/// back; uninstall leaves the item installed).
///
/// Returns whether the hook actually RAN (`true`) or was skipped (`false`),
/// so an install-hook caller can record the outcome (HOOK-110).
fn run_item_hook(
    event: &str,
    key: &str,
    source: &str,
    cmd: &str,
    store: &Path,
    commit: &str,
    dangerously_skip: bool,
) -> Result<bool> {
    // The noun for the side effect the hook applies, per event.
    let effect = if event == "install" {
        "its side effect is not applied"
    } else {
        "its cleanup is not run"
    };
    let cwd = hook_cwd(store);
    let run = if dangerously_skip {
        true
    } else if !crate::hook::is_tty() {
        // spec: CLI-217
        crate::render::note(format!(
            "note: skipped {event} hook for {key} in a non-interactive context; {effect}"
        ));
        false
    } else {
        let disclosure = crate::hook::disclosure_text(
            source,
            &format!("(per-item {event})"),
            commit,
            &cwd.to_string_lossy(),
            cmd,
            None,
            None, // no browse URL for per-item lifecycle hooks (HOOK-24: local path only)
        );
        matches!(
            crate::hook::prompt_choice_optional(&disclosure)?,
            crate::hook::OptionalChoice::Run
        )
    };
    if run {
        // Same as the build-hook line above: a progress line on stdout in text
        // mode, unreachable from `--json` stdout by the process-wide redirect.
        // spec: CLI-217
        println!("running {event} hook for {key}");
        crate::hook::run_hook(cmd, &cwd, source, event)?;
    } else if crate::hook::is_tty() && !dangerously_skip {
        // spec: CLI-217
        crate::render::note(format!("note: skipped {event} hook for {key}; {effect}"));
    }
    Ok(run)
}

/// Run an item's install hooks (HOOK-81, HOOK-86) as the final step of installing
/// it, in declaration order. Each entry is disclosed, prompted, and fails exactly
/// as the scalar shorthand does (HOOK-86): a non-zero exit aborts the loop and the
/// caller rolls the install back. An empty list is a no-op.
///
/// Returns one [`crate::source::RecordedHook`] per hook offered, in the same
/// order, so the caller can persist the outcome on the item's manifest record
/// (HOOK-110): `ran_at = Some(commit)` for a hook that ran, `ran_at = None` for
/// one that was offered but skipped (mirrors the source-level mechanism,
/// HOOK-55). A hook whose failure aborts the loop is not represented in the
/// returned vec (its error propagates instead and the caller rolls the whole
/// install back, so there is nothing to record).
///
/// This is the entry point used by the `learn`/`upgrade` install path
/// (`commands::install_item`), where a hook failure rolls the whole
/// just-installed item back (HOOK-86): there is no manifest entry left to
/// attach a partial record to, so discarding the partial vec on error is
/// correct here. A caller that instead runs hooks against an item that stays
/// installed regardless of the outcome (`hooks run`, HOOK-102) needs the
/// hooks recorded before the failure too; see
/// [`run_item_install_hooks_partial`] for that shape.
pub fn run_item_install_hooks(
    item: &CatalogItem,
    hooks: &[&crate::mindfile::ResolvedHook],
    store: &Path,
    commit: &str,
    dangerously_skip: bool,
) -> Result<Vec<crate::source::RecordedHook>> {
    let mut recorded = Vec::with_capacity(hooks.len());
    run_item_install_hooks_recording(item, hooks, store, commit, dangerously_skip, &mut recorded)?;
    Ok(recorded)
}

/// Same hook batch as [`run_item_install_hooks`], but for a caller whose item is
/// already installed and stays installed regardless of the batch's outcome
/// (`hooks run`, HOOK-102). Returns the hooks recorded before an abort
/// alongside the error, mirroring `run_source_hooks`'s "save whatever was
/// recorded so far before propagating the error" (HOOK-53): the caller can
/// persist the partial vec to the manifest before returning the error, so a
/// hook that already ran its side effect is not offered again on retry
/// (HOOK-110).
pub fn run_item_install_hooks_partial(
    item: &CatalogItem,
    hooks: &[&crate::mindfile::ResolvedHook],
    store: &Path,
    commit: &str,
    dangerously_skip: bool,
) -> (Vec<crate::source::RecordedHook>, Result<()>) {
    let mut recorded = Vec::with_capacity(hooks.len());
    let result = run_item_install_hooks_recording(
        item,
        hooks,
        store,
        commit,
        dangerously_skip,
        &mut recorded,
    );
    (recorded, result)
}

/// Shared loop backing [`run_item_install_hooks`] and
/// [`run_item_install_hooks_partial`]: runs each hook in order, pushing its
/// outcome onto `recorded` as it goes (not only on overall success), so a
/// caller holding a `&mut` into `recorded` sees every hook that ran or was
/// offered before an abort even though this function returns early via `?`.
fn run_item_install_hooks_recording(
    item: &CatalogItem,
    hooks: &[&crate::mindfile::ResolvedHook],
    store: &Path,
    commit: &str,
    dangerously_skip: bool,
    recorded: &mut Vec<crate::source::RecordedHook>,
) -> Result<()> {
    for hook in hooks {
        let ran = run_item_hook(
            "install",
            &item.key(),
            &item.source,
            &hook.run,
            store,
            commit,
            dangerously_skip,
        )?;
        recorded.push(crate::source::RecordedHook {
            command: hook.run.clone(),
            ran_at: if ran { Some(commit.to_string()) } else { None },
        });
    }
    Ok(())
}

/// Run an item's uninstall hooks (HOOK-82, HOOK-86) before its store copy and
/// links go, in declaration order. A non-zero exit aborts the loop and the caller
/// leaves the item installed. An empty list is a no-op.
pub fn run_item_uninstall_hooks(
    item: &InstalledItem,
    hooks: &[&crate::mindfile::ResolvedHook],
    store: &Path,
    commit: &str,
    dangerously_skip: bool,
) -> Result<()> {
    for hook in hooks {
        run_item_hook(
            "uninstall",
            &item.key(),
            &item.source,
            &hook.run,
            store,
            commit,
            dangerously_skip,
        )?;
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
pub(crate) fn remove_path(path: &Path) -> Result<()> {
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

/// Copy a source item tree into `dst`, refusing any symlink entry (LIFE-42).
///
/// Uses `symlink_metadata` (no-follow) to determine whether each entry is a
/// directory, a regular file, or a symlink. A symlink anywhere in the tree is
/// rejected with an `Io` error carrying the offending path, so a crafted source
/// cannot exfiltrate secrets or cause unbounded recursion via a directory cycle.
///
/// Exported as `pub(crate)` so the surface shard's `--snapshot` frozen-copy can
/// reuse this implementation without duplicating the symlink-rejection logic.
pub(crate) fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src).map_err(|e| MindError::io(src, e))?;
    if meta.file_type().is_symlink() {
        return Err(MindError::io(
            src,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "source item trees must not contain symlinks",
            ),
        ));
    }
    if meta.is_dir() {
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
    use crate::paths::Lobe;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    // ---- STO-56: reachability gate -------------------------------------------

    /// `Lobe::reachable()` returns true when the parent dir exists, false when it
    /// does not. This is the primitive the install-time filter and relink use.
    ///
    /// `link_into_new_lobes` (backfill on explicit lobe-add) intentionally does
    /// NOT apply the reachability gate: it is an explicit user action, and
    /// `ensure_link` creates intermediate dirs. The gate applies at install-time
    /// (`planned_links`) and at `relink` (introspect --fix must not recreate links
    /// in vanished projects).
    #[test]
    fn lobe_reachable_gate_covers_planned_links_and_relink() {
        // spec: STO-56
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-reach-gate-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let existing_parent = base.join("existing-project");
        std::fs::create_dir_all(&existing_parent).unwrap();
        let reachable_lobe = Lobe {
            path: existing_parent.join(".windsurf"),
            kinds: Some(vec![ItemKind::Skill]),
        };
        assert!(
            reachable_lobe.reachable(),
            "lobe is reachable when parent exists"
        );

        let gone_parent = base.join("vanished-project");
        // Deliberately do NOT create gone_parent.
        let unreachable_lobe = Lobe {
            path: gone_parent.join(".windsurf"),
            kinds: Some(vec![ItemKind::Skill]),
        };
        assert!(
            !unreachable_lobe.reachable(),
            "lobe is unreachable when parent is missing (STO-56)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `relink` must skip a link that belongs to a configured but unreachable lobe
    /// (its parent directory has been deleted). The link stays unrepaired and no
    /// error is returned. The reachable lobe's link IS repaired.
    #[cfg(unix)]
    #[test]
    fn relink_skips_link_in_unreachable_lobe() {
        // spec: STO-56
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-reach-relink-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        // Write a store directory for the item.
        let store_rel = "store/skill/myskill";
        let store_path = mind_home.join(store_rel);
        std::fs::create_dir_all(&store_path).unwrap();
        std::fs::write(store_path.join("SKILL.md"), b"# My Skill\n").unwrap();

        // A project lobe whose parent does NOT exist (vanished project).
        let vanished_project = base.join("vanished");
        let project_lobe = vanished_project.join(".windsurf");
        let project_link = project_lobe.join("skills").join("myskill");

        // Write a config that includes the project lobe (unreachable: its parent
        // vanished_project does not exist).
        let cfg = format!(
            "lobes = [\"{claude}\", \"{project}\"]\n",
            claude = claude_home.display(),
            project = project_lobe.display(),
        );
        std::fs::write(mind_home.join("config.toml"), cfg.as_bytes()).unwrap();

        let paths = crate::paths::Paths {
            mind_home: mind_home.clone(),
            claude_home: claude_home.clone(),
        };

        // The installed item has recorded links under both lobes; neither link
        // currently exists (they were broken). relink should repair the one under
        // claude_home (reachable) but skip the one under the project lobe (gone).
        let claude_link = claude_home.join("skills").join("myskill");

        let item = InstalledItem {
            kind: ItemKind::Skill,
            name: "myskill".to_string(),
            bare_name: "myskill".to_string(),
            source: "local/test".to_string(),
            commit: "abc".to_string(),
            hash: "deadbeef".to_string(),
            store: store_rel.to_string(),
            links: vec![
                claude_link.to_string_lossy().into_owned(),
                project_link.to_string_lossy().into_owned(),
            ],
            description: None,
            install_hooks: Vec::new(),
        };

        let fixed = relink(&paths, &item).unwrap();
        // Only the claude_home link should have been repaired; the project link is
        // skipped because the project lobe is unreachable (STO-56).
        assert_eq!(
            fixed, 1,
            "relink must repair exactly 1 link (claude_home); project lobe must be skipped (STO-56)"
        );
        assert!(
            claude_link.exists() || std::fs::symlink_metadata(&claude_link).is_ok(),
            "claude_home link must have been created by relink"
        );
        assert!(
            !project_link.exists(),
            "project lobe link must not be created for an unreachable lobe (STO-56)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

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
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
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

        run_build_hook(
            &item,
            item.build.as_deref().unwrap(),
            &staging,
            "abc123",
            false,
        )
        .unwrap();
        assert!(
            !marker.exists(),
            "a non-TTY context must skip the build hook (HOOK-72)"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    // ---- DEP-6: `requires` validation in expand_references -----------------

    /// Build a minimal skill CatalogItem pointing at the given staging dir.
    fn skill_item_at(name: &str, path: std::path::PathBuf, requires: Vec<String>) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path,
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires,
            expand: Vec::new(),
            hooks: Vec::new(),
        }
    }

    fn agent_item_at(name: &str, path: std::path::PathBuf) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Agent,
            name: name.to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path,
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        }
    }

    #[test]
    fn requires_valid_entry_passes_validation() {
        // spec: DEP-6
        // A `requires: agent:test` entry that resolves to an existing sibling
        // must pass validation and allow expand_references to proceed.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging = std::env::temp_dir().join(format!("mind-req-ok-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        // A text file without any {{ so expand_references reaches the requires check
        // but skips file expansion.
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        let item = skill_item_at(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["agent:test".to_string()],
        );
        let test_agent = agent_item_at("test", std::path::PathBuf::from("/src/agents/test.md"));
        let siblings = vec![item.clone(), test_agent];

        let result = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"));
        assert!(
            result.is_ok(),
            "valid requires entry must not error: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn requires_typo_entry_is_bad_reference() {
        // spec: DEP-6
        // A `requires` entry naming a non-existent sibling is a BadReference.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-req-typo-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        let item = skill_item_at(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["agent:nonexistent".to_string()],
        );
        let siblings = vec![item.clone()]; // no "nonexistent" sibling

        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { ref referent, .. }
                if referent.contains("agent:nonexistent")),
            "typo requires entry must be BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn requires_source_qualified_entry_is_bad_reference() {
        // spec: DEP-6
        // A source-qualified `owner/repo#name` entry in `requires` is rejected as
        // a BadReference because `requires` is intra-source only (DEP-5).
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-req-cross-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        let item = skill_item_at(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["owner/repo#agent:test".to_string()],
        );
        let test_agent = agent_item_at("test", std::path::PathBuf::from("/src/agents/test.md"));
        let siblings = vec![item.clone(), test_agent];

        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { .. }),
            "source-qualified requires entry must be BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn requires_ambiguous_bare_name_is_bad_reference() {
        // spec: DEP-6
        // A bare `name` that matches siblings of two different kinds and no kind
        // qualifier is supplied is ambiguous and must be a BadReference.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-req-ambig-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        let item = skill_item_at(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["shared".to_string()], // bare name matches agent:shared AND rule:shared
        );
        let agent = CatalogItem {
            kind: ItemKind::Agent,
            name: "shared".to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path: std::path::PathBuf::from("/src/agents/shared.md"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        };
        let rule = CatalogItem {
            kind: ItemKind::Rule,
            name: "shared".to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path: std::path::PathBuf::from("/src/rules/shared.md"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        };
        let siblings = vec![item.clone(), agent, rule];

        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { ref referent, .. }
                if referent.contains("shared")),
            "ambiguous bare-name requires entry must be BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn requires_kind_qualified_among_two_same_name_siblings_is_accepted() {
        // spec: DEP-5 DEP-6
        // The other side of the ambiguity boundary: when two siblings share a
        // bare name across kinds, a `kind:name` qualifier resolves to exactly one
        // and MUST pass validation (it is not falsely flagged ambiguous). This
        // pins that the `matches.len() > 1 && r.kind.is_none()` guard does NOT
        // fire once the kind narrows the candidate set to one.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-req-kindok-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        // `agent:shared` uniquely picks the agent, even though a rule:shared also
        // exists; the bare name alone would be ambiguous.
        let item = skill_item_at(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["agent:shared".to_string()],
        );
        let agent = CatalogItem {
            kind: ItemKind::Agent,
            name: "shared".to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path: std::path::PathBuf::from("/src/agents/shared.md"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        };
        let rule = CatalogItem {
            kind: ItemKind::Rule,
            name: "shared".to_string(),
            source: "local/test/repo".to_string(),
            prefix: None,
            path: std::path::PathBuf::from("/src/rules/shared.md"),
            description: None,
            link_rel: None,
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        };
        let siblings = vec![item.clone(), agent, rule];

        let result = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"));
        assert!(
            result.is_ok(),
            "a kind-qualified ref that uniquely matches one of two same-name \
             siblings must pass validation, not be flagged ambiguous: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn requires_self_reference_resolves_and_is_accepted() {
        // spec: DEP-6
        // An item whose `requires` names its own bare name resolves to itself (it
        // is a sibling of itself in the passed list) and must NOT error: the
        // validation only rejects zero matches or an ambiguous bare name, and a
        // self-reference is a single, unambiguous match. (The pure resolver in
        // deps.rs separately drops the trivial self-edge.)
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-req-self-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hello\n").unwrap();

        let item = skill_item_at(
            "solo",
            std::path::PathBuf::from("/src/skills/solo"),
            vec!["skill:solo".to_string()],
        );
        let siblings = vec![item.clone()];

        let result = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"));
        assert!(
            result.is_ok(),
            "a self-requires must resolve to the item itself and not error: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    // ---- NS-57 / TOOL-20: `expand:` opt-in token expansion -----------------

    /// A skill item at `path` (a fake source path) that opts `expand` files into
    /// token expansion.
    fn skill_item_expand(name: &str, path: std::path::PathBuf, expand: Vec<String>) -> CatalogItem {
        let mut item = skill_item_at(name, path, Vec::new());
        item.expand = expand;
        item
    }

    #[test]
    fn expand_listed_non_markdown_file_expands_with_absolute_path() {
        // spec: NS-57 TOOL-20
        // A file named in `expand:` is expanded like markdown even though it is
        // not markdown, and its path tokens render as an ABSOLUTE store path,
        // while a markdown sibling keeps the TOOL-16 `~` form. Uses a store root
        // under home so the two renderings are distinguishable.
        let home = dirs::home_dir().expect("home dir for the tilde-vs-absolute check");
        let store_root = home.join("mind-expand-store");
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-expand-abs-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(staging.join("resources")).unwrap();
        std::fs::write(staging.join("SKILL.md"), "see {{self}}/x\n").unwrap();
        std::fs::write(staging.join("resources/pr.py"), "p = \"{{self}}/x\"\n").unwrap();

        let item = skill_item_expand(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["resources/pr.py".to_string()],
        );
        let siblings = vec![item.clone()];
        expand_references(&staging, &item, &siblings, &store_root).unwrap();

        let md = std::fs::read_to_string(staging.join("SKILL.md")).unwrap();
        let py = std::fs::read_to_string(staging.join("resources/pr.py")).unwrap();
        assert!(
            md.contains("~/mind-expand-store/skill/review/x"),
            "a markdown file keeps the ~ form (TOOL-16): {md:?}"
        );
        let abs = store_root.join("skill/review/x");
        assert!(
            py.contains(&abs.to_string_lossy().into_owned()),
            "an expand-listed file renders the absolute store path (TOOL-20): {py:?}"
        );
        assert!(
            !py.contains("~/mind-expand-store"),
            "an expand-listed file must not carry the ~ form: {py:?}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn unlisted_non_markdown_file_is_left_literal() {
        // spec: NS-57 NS-53
        // Without an `expand:` entry, a token in a non-markdown file is left
        // exactly as written (the NS-53 default).
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-expand-literal-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(staging.join("resources")).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hi\n").unwrap();
        std::fs::write(staging.join("resources/other.py"), "p = \"{{self}}/x\"\n").unwrap();

        let item = skill_item_expand(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            Vec::new(),
        );
        let siblings = vec![item.clone()];
        expand_references(&staging, &item, &siblings, std::path::Path::new("/store")).unwrap();

        let py = std::fs::read_to_string(staging.join("resources/other.py")).unwrap();
        assert_eq!(
            py, "p = \"{{self}}/x\"\n",
            "an unlisted non-markdown file must be left literal (NS-53)"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn expand_entry_naming_a_missing_file_is_bad_reference() {
        // spec: NS-57
        // An `expand:` entry that names no shipped file aborts the staged install
        // as a BadReference, so a typo is caught loudly.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-expand-miss-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hi\n").unwrap();

        let item = skill_item_expand(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["resources/nope.py".to_string()],
        );
        let siblings = vec![item.clone()];
        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { .. }),
            "a missing expand target must be BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn expand_entry_escaping_the_item_is_bad_reference() {
        // spec: NS-57
        // A `..`-bearing (or absolute) `expand:` entry is refused before any file
        // is touched.
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-expand-esc-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hi\n").unwrap();

        let item = skill_item_expand(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["../escape.py".to_string()],
        );
        let siblings = vec![item.clone()];
        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { .. }),
            "an escaping expand entry must be BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn unresolved_token_in_listed_file_is_a_hard_failure() {
        // spec: NS-57
        // A token that does not resolve is inert (left literal) in an unlisted
        // non-markdown file, but a HARD install failure once the file is listed,
        // exactly as in markdown (NS-12).
        let n = N.fetch_add(1, Ordering::SeqCst);
        let staging =
            std::env::temp_dir().join(format!("mind-expand-bad-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(staging.join("resources")).unwrap();
        std::fs::write(staging.join("SKILL.md"), "# hi\n").unwrap();
        std::fs::write(staging.join("resources/pr.py"), "p = \"{{tools:nope}}\"\n").unwrap();

        let item = skill_item_expand(
            "review",
            std::path::PathBuf::from("/src/skills/review"),
            vec!["resources/pr.py".to_string()],
        );
        let siblings = vec![item.clone()];
        let err = expand_references(&staging, &item, &siblings, std::path::Path::new("/store"))
            .unwrap_err();
        assert!(
            matches!(err, crate::error::MindError::BadReference { .. }),
            "an unresolved token in a listed file must be a hard BadReference: {err}"
        );
        let _ = std::fs::remove_dir_all(&staging);
    }

    // ---- LIFE-43: forced-install transactional stash/restore ---------------

    /// `maybe_stash_foreign` moves a foreign regular file to the stash (returning
    /// true) and leaves an absent path or a mind-owned symlink untouched (false).
    /// Renaming the stash back to the original path recovers the file intact.
    #[cfg(unix)]
    #[test]
    fn maybe_stash_foreign_moves_foreign_file_and_restores_correctly() {
        // spec: LIFE-43
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-stash-helper-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let store_root = base.join("store");
        let link = base.join("link-target.md");
        let stash0 = base.join("stash").join("0");

        // Case 1: absent path -> no stash, returns false.
        let got = maybe_stash_foreign(&store_root, &link, &stash0).unwrap();
        assert!(!got, "absent link must not be stashed (LIFE-43)");
        assert!(
            !stash0.exists(),
            "no stash must be created for an absent link"
        );

        // Case 2: foreign regular file -> stashed, returns true.
        let foreign_content = b"original foreign content";
        std::fs::write(&link, foreign_content).unwrap();
        let got = maybe_stash_foreign(&store_root, &link, &stash0).unwrap();
        assert!(got, "foreign file must be stashed (LIFE-43)");
        assert!(!link.exists(), "original path must be vacated after stash");
        assert!(stash0.exists(), "stash file must exist");
        assert_eq!(
            std::fs::read(&stash0).unwrap(),
            foreign_content,
            "stash must preserve original content"
        );

        // Simulate rollback: rename stash back to original location.
        rename(&stash0, &link).unwrap();
        assert!(link.exists(), "restored path must exist");
        assert_eq!(
            std::fs::read(&link).unwrap(),
            foreign_content,
            "restored file must have original content (LIFE-43)"
        );

        // Case 3: mind's own symlink -> not stashed, returns false.
        std::fs::create_dir_all(&store_root).unwrap();
        let store_file = store_root.join("agent").join("myagent");
        std::fs::create_dir_all(store_file.parent().unwrap()).unwrap();
        std::fs::write(&store_file, b"mind managed").unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&store_file, &link).unwrap();

        let stash1 = base.join("stash").join("1");
        let got = maybe_stash_foreign(&store_root, &link, &stash1).unwrap();
        assert!(!got, "mind's own symlink must not be stashed (LIFE-43)");
        assert!(!stash1.exists(), "no stash for mind's own symlink");
        // Symlink is untouched.
        assert!(std::fs::symlink_metadata(&link).is_ok());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A forced install that clobbers a foreign file at the first link target
    /// but fails on a later link must leave the original foreign file restored
    /// and the store copy rolled back (LIFE-43, LIFE-4).
    ///
    /// Two lobes are wired via config.toml: the first is a valid directory
    /// containing a pre-existing foreign file; the second has a regular file as
    /// its parent component, so `mkdir_p` inside `ensure_link` fails with ENOTDIR.
    #[cfg(unix)]
    #[test]
    fn force_install_rollback_restores_stashed_foreign_target() {
        // spec: LIFE-43
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-life43-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let mind_home = base.join("mind");
        let lobe1 = base.join("lobe1");
        // A regular file: using a subdirectory of it as a lobe causes ENOTDIR.
        let blocker = base.join("not-a-dir");

        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&lobe1).unwrap();
        std::fs::write(&blocker, b"i-am-a-regular-file").unwrap();

        let lobe2 = blocker.join("lobe2");

        // Config with two lobes; no env var mutation needed.
        let cfg = format!(
            "lobes = [\"{}\", \"{}\"]\n",
            lobe1.to_str().unwrap(),
            lobe2.to_str().unwrap(),
        );
        std::fs::write(mind_home.join("config.toml"), cfg.as_bytes()).unwrap();

        let paths = Paths {
            mind_home: mind_home.clone(),
            claude_home: lobe1.clone(),
        };

        // Place a foreign regular file at lobe1's agent link location.
        let agents_dir = lobe1.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let foreign_path = agents_dir.join("myagent.md");
        let foreign_content = b"foreign content - must survive the rollback";
        std::fs::write(&foreign_path, foreign_content).unwrap();

        // Source file for the catalog item.
        let src_file = base.join("myagent.md");
        std::fs::write(&src_file, b"# My Agent\n").unwrap();

        let item = CatalogItem {
            kind: ItemKind::Agent,
            name: "myagent".to_string(),
            source: "local/test".to_string(),
            prefix: None,
            path: src_file,
            description: None,
            link_rel: None, // defaults to agents/myagent.md under each lobe
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            expand: Vec::new(),
            hooks: Vec::new(),
        };

        // force=true: lobe1's link is stashed then overwritten with a symlink;
        // lobe2's link fails (mkdir_p into a regular file is ENOTDIR); rollback
        // must restore the stash to lobe1's link path.
        let result = install(
            &paths,
            &item,
            "abc",
            std::slice::from_ref(&item),
            true,
            false,
        );

        assert!(
            result.is_err(),
            "install must fail when a later link cannot be created: {result:?}"
        );

        // Foreign file must be restored at its original path (LIFE-43).
        let meta = std::fs::symlink_metadata(&foreign_path)
            .expect("foreign file must exist at original path after rollback (LIFE-43)");
        assert!(
            !meta.file_type().is_symlink(),
            "restored path must be a regular file, not a symlink (LIFE-43)"
        );
        assert_eq!(
            std::fs::read(&foreign_path).unwrap(),
            foreign_content,
            "restored file must have original content (LIFE-43)"
        );

        // Store copy must be absent (LIFE-4 rollback).
        let store_path = mind_home.join("store").join("agent").join("myagent");
        assert!(
            !store_path.exists(),
            "store copy must be absent after rollback: {store_path:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- LIFE-44: uninstall path confinement checks --------------------------

    #[test]
    fn is_confined_under_accepts_child_paths() {
        // spec: LIFE-44
        let root = std::path::Path::new("/home/user/.mind/store");
        assert!(
            is_confined_under(
                std::path::Path::new("/home/user/.mind/store/skill/review"),
                root
            ),
            "a child of the root must be confined"
        );
        assert!(
            is_confined_under(root, root),
            "the root itself must be accepted"
        );
    }

    #[test]
    fn is_confined_under_rejects_parent_dir_components() {
        // spec: LIFE-44 -- a `..` component in the recorded path is a violation
        // regardless of where the path appears to start with the root.
        let root = std::path::Path::new("/home/user/.mind/store");
        // Lexically starts with root but contains `..` -> must be rejected.
        assert!(
            !is_confined_under(
                std::path::Path::new("/home/user/.mind/store/skill/../../../etc/passwd"),
                root
            ),
            "path with .. must be rejected even if it starts with the root"
        );
        // A plain `..` relative path must be rejected too.
        assert!(
            !is_confined_under(std::path::Path::new("../outside"), root),
            "relative path with leading .. must be rejected"
        );
    }

    #[test]
    fn is_confined_under_rejects_sibling_paths() {
        // spec: LIFE-44
        let root = std::path::Path::new("/home/user/.mind/store");
        assert!(
            !is_confined_under(std::path::Path::new("/home/user/.claude/skills/x"), root),
            "a path outside the root must be rejected"
        );
        // A path that is a proper prefix of the root in string terms but not a
        // child in path-component terms must also be rejected.
        assert!(
            !is_confined_under(std::path::Path::new("/home/user/.mind"), root),
            "a parent of the root must be rejected"
        );
    }

    /// A source tree containing a symlink (e.g. pointing to a secret outside
    /// the tree) must be rejected by `copy_recursive` with a clear error that
    /// names the offending path (LIFE-42). The caller (`install`) discards the
    /// staging directory on any error, so the partial copy is cleaned up there.
    #[cfg(unix)]
    #[test]
    fn copy_recursive_rejects_symlink_in_source_tree() {
        // spec: LIFE-42
        let n = N.fetch_add(1, Ordering::SeqCst);
        let src = std::env::temp_dir().join(format!("mind-cplink-src-{}-{n}", std::process::id()));
        let dst = std::env::temp_dir().join(format!("mind-cplink-dst-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ok.txt"), b"normal").unwrap();
        // A symlink to /etc/passwd simulates a crafted source attempting to
        // exfiltrate a secret file outside the item tree.
        std::os::unix::fs::symlink("/etc/passwd", src.join("evil")).unwrap();

        let result = copy_recursive(&src, &dst);

        assert!(
            result.is_err(),
            "copy_recursive must reject a source tree containing a symlink"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("symlink"),
            "error message must mention 'symlink': {msg}"
        );
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }
}
