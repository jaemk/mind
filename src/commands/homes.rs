//! Mind homes: where a source's clone lives and moving it between homes (HOME-*).

use super::*;

/// Resolve a source's clone directory (`Source::clone_dir`) and confine it to
/// the managed sources tree before any caller mutates it (clone, re-clone, or
/// `remove_dir_all`). STO-69: a stale or hand-tampered `sources.json` entry
/// (e.g. `host`/`owner`/`repo` parts containing `..`) must not let `mind`
/// write or delete outside `~/.mind/sources`. Mirrors the confinement check
/// `install.rs` already applies to a manifest's store/link paths (LIFE-44).
///
/// A linked local source (`Source::is_linked`) is exempt: its "clone dir" is
/// the user's own working tree by design (CLI-27), not a path under the
/// sources tree, so it is returned unchecked.
// spec: STO-69
pub(super) fn clone_dir_checked(
    paths: &Paths,
    source: &crate::source::Source,
) -> Result<std::path::PathBuf> {
    let dir = source.clone_dir(paths);
    if source.is_linked() {
        return Ok(dir);
    }
    if install::is_confined_under(&dir, &paths.home_of(&source.name).join("sources")) {
        Ok(dir)
    } else {
        Err(MindError::UnsafeClonePath {
            path: dir,
            identity: source.name.clone(),
        })
    }
}

/// Move a fresh clone at `from` to where `source` now resolves, after its
/// `[source] mind-home` changed its home (HOME-5). A rename when both are on
/// one filesystem, else a fresh clone at the new place. The clone at `from` is
/// gone afterwards, on success or failure.
pub(super) fn relocate_clone(
    paths: &Paths,
    source: &crate::source::Source,
    from: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let moved = (|| {
        let to = clone_dir_checked(paths, source)?;
        if to.exists() {
            std::fs::remove_dir_all(&to).map_err(|e| MindError::io(&to, e))?;
        }
        if let Some(parent) = to.parent() {
            crate::paths::mkdir_p(parent)?;
        }
        if std::fs::rename(from, &to).is_err()
            && let Err(e) = git::clone(&source.url, &to)
        {
            let _ = std::fs::remove_dir_all(&to);
            return Err(e);
        }
        Ok(to)
    })();
    let _ = std::fs::remove_dir_all(from);
    moved
}

/// Put melded source `name` in mind home `to` (`None`: the default home), for
/// `meld --home` and `learn <url> --home` on a source already melded (HOME-8).
/// The source moves with every source it curates that follows it (one in the
/// curator's home that names no home of its own, HOME-5): each clone moves (a
/// rename, else a copy) and each registry entry is written to `to`'s file.
/// Then every installed item of those sources whose store copy is in another
/// home is reinstalled in `to` through the upgrade pass, which replaces its
/// links and removes the old copy. Running it again changes nothing.
// spec: HOME-8
pub(crate) fn move_to_home(
    paths: &Paths,
    name: &str,
    to: Option<&std::path::Path>,
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    // spec: HOME-16 -- refuse a move that would leave a tool unbuilt.
    let registry = Registry::load(paths)?;
    if registry.find(name).is_some() {
        let family = home_family(paths, &registry, name);
        if let Some(item) = move_would_skip_build(paths, &registry, &family, to, accept)? {
            return Err(MindError::MoveNeedsBuild {
                name: name.to_string(),
                item,
            });
        }
    }
    let family = relocate_family(paths, name, to)?;
    if family.is_empty() {
        return Ok(());
    }
    // spec: HOME-15 -- the consumer chose this home; `upgrade` keeps it.
    let mut registry = Registry::load(paths)?;
    if let Some(entry) = registry.sources.iter_mut().find(|s| s.name == name)
        && !entry.chosen_home
    {
        entry.chosen_home = true;
        registry.save(paths)?;
    }
    reinstall_moved_items(paths, &family, yes, accept)
}

/// Move each source `in_scope` admits whose own `[source] mind-home`, as its
/// clone now has it, names an allowed home it is not in, unless the consumer
/// chose its home (HOME-15). The curated sources that follow it move with it
/// (HOME-8); the upgrade pass then reinstalls their items there. A home that
/// is not allowed, or a move that fails, leaves the source where it is with a
/// warning. Returns whether any source moved. A `preview` (CLI-263) moves
/// nothing and returns the moves it would make instead.
// spec: HOME-15
pub(super) fn follow_declared_homes(
    paths: &Paths,
    registry: &Registry,
    in_scope: &dyn Fn(&crate::source::Source) -> bool,
    accept: crate::hook::Accept,
    preview: bool,
) -> Result<(bool, Vec<HomeMoveJson>)> {
    use crate::paths::normalize_home;
    let out = crate::render::ctx();
    let mut saved = false;
    let mut any = false;
    let mut predicted = Vec::new();
    let names: Vec<String> = registry.sources.iter().map(|s| s.name.clone()).collect();
    for name in names {
        // Re-read: an earlier move may have taken this source along.
        let current = Registry::load(paths)?;
        let Some(src) = current
            .find(&name)
            .filter(|s| !s.chosen_home && in_scope(s))
        else {
            continue;
        };
        let Some(declared) = MindToml::load(&src.clone_dir(paths))
            .ok()
            .flatten()
            .and_then(|m| m.source.mind_home)
        else {
            continue;
        };
        let shown = strip_ansi(&name);
        let origin = format!("the `mind-home` in {shown}'s mind.toml");
        let target = match paths.allowed_home(&declared, &origin) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "{} {shown} stays in {}: {}",
                    out.warn(),
                    paths.home_of(&name).display(),
                    strip_ansi(&e.to_string())
                );
                continue;
            }
        };
        let to = normalize_home(target.as_deref().unwrap_or(&paths.mind_home));
        if to == normalize_home(&paths.home_of(&name)) {
            continue;
        }
        // spec: HOME-16 -- a move that would leave a tool unbuilt waits for a
        // run that can build it.
        let family = home_family(paths, &current, &name);
        if let Some(item) =
            move_would_skip_build(paths, &current, &family, target.as_deref(), accept)?
        {
            eprintln!(
                "{} {shown} stays in {}: {}",
                out.warn(),
                paths.home_of(&name).display(),
                strip_ansi(
                    &MindError::MoveNeedsBuild {
                        name: name.clone(),
                        item
                    }
                    .to_string()
                )
            );
            continue;
        }
        if preview {
            // The sources that follow its home move with it (HOME-5).
            for member in &family {
                let from = normalize_home(&paths.home_of(member));
                let member_shown = strip_ansi(member);
                if from == to
                    || predicted
                        .iter()
                        .any(|m: &HomeMoveJson| m.source == member_shown)
                {
                    continue;
                }
                if !out.json {
                    let why = if *member == name {
                        "its mind.toml declares `mind-home`".to_string()
                    } else {
                        format!("it follows {shown}")
                    };
                    println!(
                        "{member_shown} would move from {} to {} ({why})",
                        from.display(),
                        to.display()
                    );
                }
                predicted.push(HomeMoveJson {
                    source: member_shown,
                    from: from.display().to_string(),
                    to: to.display().to_string(),
                });
            }
            continue;
        }
        if !saved {
            // What the pass changed so far (sync, breaking moves) is on disk
            // before a move reads the registry back.
            registry.save(paths)?;
            saved = true;
        }
        match relocate_family(paths, &name, target.as_deref()) {
            Ok(_) => any = true,
            Err(e) => eprintln!(
                "{} {shown} stays in {}: could not move it to {}: {}",
                out.warn(),
                paths.home_of(&name).display(),
                to.display(),
                strip_ansi(&e.to_string())
            ),
        }
    }
    Ok((any, predicted))
}

/// A source move `upgrade --dry-run` predicts (HOME-15, CLI-263).
#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct HomeMoveJson {
    pub(super) source: String,
    pub(super) from: String,
    pub(super) to: String,
}

/// Reinstall, through the upgrade pass and without fetching, every installed
/// item of `family` whose store copy is in another home than its source's
/// (HOME-8).
fn reinstall_moved_items(
    paths: &Paths,
    family: &[String],
    yes: bool,
    accept: crate::hook::Accept,
) -> Result<()> {
    let keys: Vec<String> = Manifest::load(paths)?
        .items
        .values()
        .filter(|it| family.contains(&it.source) && item_in_other_home(paths, it))
        .map(|it| it.key().as_str().to_string())
        .collect();
    if !keys.is_empty() {
        // The meld's own result is the one `--json` document (CLI-217).
        upgrade_inner_scoped(
            paths,
            yes,
            None,
            true,
            SourceScope::Names(family, None),
            Some(&keys),
            accept,
        )?;
    }
    Ok(())
}

/// Move melded source `name`, with the curated sources that follow it, into
/// home `to` (`None`: the default home), HOME-8: each clone moves (a rename,
/// else a copy) and each registry entry is written to `to`'s file. Returns
/// the family (empty when `name` is not melded); its items are left for the
/// upgrade pass to reinstall. Refuses before anything moves when `to` lists a
/// shadowed copy of a member, and puts the clones back when the registry
/// cannot be saved.
fn relocate_family(paths: &Paths, name: &str, to: Option<&std::path::Path>) -> Result<Vec<String>> {
    use crate::paths::{normalize_home, set_source_home, source_home};
    let out = crate::render::ctx();
    let home_key = |h: Option<&std::path::Path>| normalize_home(h.unwrap_or(&paths.mind_home));
    let target = home_key(to);
    let registry = Registry::load(paths)?;
    if registry.find(name).is_none() {
        return Ok(Vec::new());
    }
    let family = home_family(paths, &registry, name);
    // Refuse before anything moves, so a refused follower does not leave the
    // family split across homes.
    for member in &family {
        if home_key(source_home(member).as_deref()) != target
            && Registry::home_lists(&target, member)
        {
            return Err(MindError::SourceInOtherHome {
                name: member.clone(),
                home: target.display().to_string(),
            });
        }
    }
    // Each member moved so far: its name, old home, and (old, new) clone dirs
    // when its clone moved.
    type Moved = (
        String,
        Option<std::path::PathBuf>,
        Option<(std::path::PathBuf, std::path::PathBuf)>,
    );
    let mut moved: Vec<Moved> = Vec::new();
    // Put every moved member back, so a failure leaves no one split off.
    let undo = |moved: &[Moved]| {
        for (member, from, dirs) in moved.iter().rev() {
            if let Some((old, new)) = dirs {
                if old.exists() {
                    let _ = install::remove_path(new);
                } else {
                    let _ = move_tree(new, old);
                }
            }
            set_source_home(member, from.as_deref());
        }
    };
    for member in &family {
        let from = source_home(member);
        if home_key(from.as_deref()) == target {
            continue;
        }
        let Some(src) = registry.find(member).cloned() else {
            continue;
        };
        let relocated = (|| {
            let old_dir = clone_dir_checked(paths, &src)?;
            set_source_home(member, to);
            if src.is_linked() || !old_dir.exists() {
                return Ok(None);
            }
            let new_dir = clone_dir_checked(paths, &src)?;
            move_tree(&old_dir, &new_dir)?;
            Ok(Some((old_dir, new_dir)))
        })();
        match relocated {
            Ok(dirs) => moved.push((member.clone(), from, dirs)),
            Err(e) => {
                set_source_home(member, from.as_deref());
                undo(&moved);
                return Err(e);
            }
        }
    }
    if moved.is_empty() {
        return Ok(family);
    }
    if let Err(e) = registry.save(paths) {
        undo(&moved);
        // A save that wrote some homes before failing is written back.
        let _ = registry.save(paths);
        return Err(e);
    }
    if !out.json {
        for (member, _, _) in &moved {
            println!(
                "{} moved {} to mind home {}",
                out.ok(),
                strip_ansi(member),
                target.display()
            );
        }
    }
    Ok(family)
}

/// `name` and, transitively, the curated sources that follow its home (HOME-8).
pub(super) fn home_family(paths: &Paths, registry: &Registry, name: &str) -> Vec<String> {
    let mut family = vec![name.to_string()];
    let mut i = 0;
    while i < family.len() {
        let curator = family[i].clone();
        for child in home_followers(paths, registry, &curator) {
            if !family.contains(&child) {
                family.push(child);
            }
        }
        i += 1;
    }
    family
}

/// An installed item of `family`, not yet in home `to`, whose build hook a
/// move there would have to run again (the move reinstalls it from the clone),
/// when that hook cannot run here: no terminal to ask and `--accept-hooks`
/// does not name `build` (HOOK-72). `None` when the move rebuilds nothing it
/// cannot. A source that cannot be scanned is left to the move to report.
// spec: HOME-16
pub(super) fn move_would_skip_build(
    paths: &Paths,
    registry: &Registry,
    family: &[String],
    to: Option<&std::path::Path>,
    accept: crate::hook::Accept,
) -> Result<Option<String>> {
    use crate::paths::normalize_home;
    if accept.build || crate::hook::is_tty() {
        return Ok(None);
    }
    let target = normalize_home(to.unwrap_or(&paths.mind_home));
    let manifest = Manifest::load(paths)?;
    for member in family {
        let Some(src) = registry.find(member) else {
            continue;
        };
        let mut catalog: Vec<CatalogItem> = Vec::new();
        if catalog::scan_source(paths, src, &mut catalog).is_err() {
            continue;
        }
        for it in manifest.items.values().filter(|i| i.source == *member) {
            if normalize_home(&paths.home_of_item_store(&it.store)) == target {
                continue;
            }
            if catalog.iter().any(|c| {
                c.kind == it.kind
                    && c.name == it.bare_name
                    && c.source == it.source
                    && c.build.is_some()
            }) {
                return Ok(Some(it.display_key()));
            }
        }
    }
    Ok(None)
}

/// Whether an installed item's store copy is in another home than its
/// source's (HOME-8): the upgrade pass reinstalls it there.
pub(super) fn item_in_other_home(paths: &Paths, item: &crate::manifest::InstalledItem) -> bool {
    use crate::paths::normalize_home;
    normalize_home(&paths.home_of_item_store(&item.store))
        != normalize_home(&paths.home_of(&item.source))
}

/// The sources `curator` curates that follow its home (HOME-5): in the same
/// home, with no `mind-home` of their own in their `mind.toml` or in the
/// curator's entry for them (HOME-10).
fn home_followers(paths: &Paths, registry: &Registry, curator: &str) -> Vec<String> {
    use crate::paths::source_home;
    let Some(cur) = registry.find(curator) else {
        return Vec::new();
    };
    let entries = MindToml::load(&cur.clone_dir(paths))
        .ok()
        .flatten()
        .and_then(|m| m.discover)
        .map(|d| d.sources)
        .unwrap_or_default();
    registry
        .sources
        .iter()
        .filter(|s| s.curated_by.as_deref() == Some(curator))
        .filter(|s| source_home(&s.name) == source_home(curator))
        .filter(|s| {
            !entries.iter().any(|e| {
                e.mind_home.is_some()
                    && instance_name(&e.source, e.effective_alias().as_deref())
                        .is_ok_and(|n| n == s.name)
            })
        })
        .filter(|s| {
            !MindToml::load(&s.clone_dir(paths))
                .ok()
                .flatten()
                .is_some_and(|m| m.source.mind_home.is_some())
        })
        .map(|s| s.name.clone())
        .collect()
}

/// Move a directory tree to `to`: a rename, else (another filesystem) a copy
/// that keeps symlinks, then removal of the original. A leftover at `to` is
/// replaced. Once the copy is complete the move has happened: an original
/// that cannot be removed is left with a warning, not reported as a failure
/// that would send the source back to a partly removed tree.
fn move_tree(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    install::remove_path(to)?;
    if let Some(parent) = to.parent() {
        crate::paths::mkdir_p(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if let Err(e) = install::copy_tree_keeping_links(from, to) {
        let _ = install::remove_path(to);
        return Err(e);
    }
    if let Err(e) = install::remove_path(from) {
        let msg = format!(
            "warning: moved {} to {} but could not remove the original: {}",
            crate::sanitize::display_path(from),
            crate::sanitize::display_path(to),
            strip_ansi(&e.to_string())
        );
        crate::progress::suspend(|| eprintln!("{msg}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::*;

    /// Build a `Source` directly (bypassing `parse_spec`'s own validation) so a
    /// traversing `host`/`owner`/`repo` part -- the shape a stale or
    /// hand-tampered `sources.json` entry could carry -- reaches
    /// `clone_dir_checked` exactly as it would from `Registry::load`.
    fn raw_source(host: &str, owner: &str, repo: &str) -> crate::source::Source {
        crate::source::Source {
            name: format!("{host}/{owner}/{repo}"),
            url: "https://example.com/evil".into(),
            host: host.into(),
            owner: owner.into(),
            repo: repo.into(),
            commit: None,
            description: None,
            alias: None,
            as_alias: None,
            pin: Pin::DefaultBranch,
            roots: None,
            flat_skills: false,
            full_history: false,
            add_roots: None,
            item_path: None,
            item_kind: None,
            curated_by: None,
            origin: None,
            plugin_version: None,
            install_hooks: Vec::new(),
            items_hooks: Vec::new(),
            install_hook: None,
            install_hook_commit: None,
            profiles: None,
            profile_snapshot: None,
            whole_snapshot: None,
            auto_update: None,
            lobe: None,
            chosen_home: false,
        }
    }

    #[test]
    fn clone_dir_checked_rejects_a_traversing_host_part() {
        // spec: STO-69 -- a `..` host segment resolves `clone_dir` outside the
        // sources tree; `clone_dir_checked` must refuse it with
        // `UnsafeClonePath` before any caller can clone into or delete it.
        let (paths, base) = ns_paths();
        let source = raw_source("..", "..", "victim");
        let err = clone_dir_checked(&paths, &source).expect_err("traversing host must be refused");
        match err {
            MindError::UnsafeClonePath { identity, .. } => {
                assert_eq!(identity, source.name);
            }
            other => panic!("expected UnsafeClonePath, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clone_dir_checked_rejects_a_traversing_repo_part() {
        // spec: STO-69 -- same guard, the `..` on the `repo` segment instead
        // (host/owner both structurally safe, single-component values).
        let (paths, base) = ns_paths();
        let source = raw_source("github.com", "acme", "../../victim");
        let err = clone_dir_checked(&paths, &source).expect_err("traversing repo must be refused");
        assert_eq!(err.kind(), "unsafe-clone-path");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clone_dir_checked_accepts_a_well_formed_source() {
        // spec: STO-69 -- an ordinary source's clone dir stays confined under
        // the sources tree and must be accepted.
        let (paths, base) = ns_paths();
        let source = raw_source("github.com", "acme", "agents");
        let dir = clone_dir_checked(&paths, &source).expect("well-formed source must be accepted");
        assert!(
            dir.starts_with(paths.sources_dir()),
            "accepted clone dir must be under the sources tree: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ----- CLI-233/CLI-234/CLI-235: naming an upgrade pass's matched sources
    // before consent -----
}
