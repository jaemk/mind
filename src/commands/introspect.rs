//! `mind introspect`.

use super::*;

/// One diagnostic finding from `introspect`. `kind` is a stable machine tag;
/// `message` is the human line.
#[derive(Serialize)]
struct Issue {
    pub(super) kind: &'static str,
    pub(super) target: String,
    pub(super) message: String,
}

/// `mind introspect [--fix] [--json]` — report drift and breakage. With `--fix`,
/// repair what can be fixed without changing versions: recreate missing symlinks
/// from each item's file registry. Drifted or renamed items are left to
/// `upgrade`. `--json` emits the findings as JSON on stdout.
pub fn introspect(paths: &Paths, fix: bool, json: bool) -> Result<()> {
    let registry = Registry::load(paths)?;
    let mut manifest = Manifest::load(paths)?;
    let mut issues: Vec<Issue> = Vec::new();
    // spec: CLI-210 -- a source whose clone is missing or otherwise unscannable
    // (e.g. an item-link instance whose linked path vanished) must not abort
    // the whole run: scan each source independently, report the failure as a
    // "source-scan-failed" issue, and keep going with a partial catalog built
    // from the sources that DID scan successfully.
    let mut catalog: Vec<CatalogItem> = Vec::new();
    for s in &registry.sources {
        if let Err(e) = catalog::scan_source(paths, s, &mut catalog) {
            issues.push(Issue {
                kind: "source-scan-failed",
                target: s.name.clone(),
                message: format!("source '{}' could not be scanned: {e}", s.name),
            });
        }
    }
    let mut repaired: Vec<String> = Vec::new();
    // HARN-8: `--fix` may create new lobe links; record whether we mutated the
    // manifest so it is saved once after the loop.
    let mut manifest_dirty = false;

    // HARN-13: check for vanished lobes (configured lobes whose parent dir is
    // gone) before computing all_lobes, so --fix can prune them from config and
    // the subsequent relink loop sees only live lobes.
    //
    // spec: HARN-13
    // spec: HARN-18 -- a vanished-lobe finding that this same `--fix` run repairs
    // (prunes from config) must not also be counted as an outstanding issue: track
    // the targets actually pruned and drop their findings below, so the exit
    // summary counts only what remains.
    let mut repaired_vanished_lobes: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    {
        let cfg_result = paths.settings().config();
        if let Ok(mut cfg) = cfg_result
            && !cfg.lobes.is_empty()
        {
            let mut pruned = false;
            for entry in &cfg.lobes {
                let lobe = crate::paths::Lobe {
                    path: std::path::PathBuf::from(entry.path()),
                    kinds: entry.kinds().map(|ks| ks.to_vec()),
                };
                if !lobe.reachable() {
                    issues.push(Issue {
                        kind: "vanished-lobe",
                        target: entry.path().to_string(),
                        message: format!(
                            "lobe '{}' parent dir is gone; \
                                 run `mind introspect --fix` to prune it",
                            entry.path()
                        ),
                    });
                    if fix {
                        // Strip manifest links confined under this lobe.
                        let lobe_pb = std::path::PathBuf::from(entry.path());
                        for item in manifest.items.values_mut() {
                            let before = item.links.len();
                            item.links
                                .retain(|l| !std::path::Path::new(l).starts_with(&lobe_pb));
                            if item.links.len() != before {
                                manifest_dirty = true;
                            }
                        }
                        pruned = true;
                        repaired_vanished_lobes.insert(entry.path().to_string());
                    }
                }
            }
            if fix && pruned {
                cfg.lobes.retain(|e| {
                    let lobe = crate::paths::Lobe {
                        path: std::path::PathBuf::from(e.path()),
                        kinds: e.kinds().map(|ks| ks.to_vec()),
                    };
                    lobe.reachable()
                });
                paths.settings().save_config(&cfg)?;
                repaired.push("pruned vanished lobe(s) from config".to_string());
            }
        }
    }
    // spec: HARN-18 -- drop each vanished-lobe finding this run repaired, so it
    // is not both reported as fixed (above) and counted as an outstanding issue.
    if !repaired_vanished_lobes.is_empty() {
        issues.retain(|i| {
            !(i.kind == "vanished-lobe" && repaired_vanished_lobes.contains(&i.target))
        });
    }

    // spec: HARN-26 -- a legacy `<project>/.windsurf` lobe still works (Devin
    // reads it as a fallback), so it is advice, not an issue: it never counts
    // toward the summary and `--fix` leaves it alone.
    let mut notes: Vec<Issue> = Vec::new();
    if let Ok(cfg) = paths.settings().config() {
        for entry in &cfg.lobes {
            let path = std::path::Path::new(entry.path());
            if path.file_name().is_some_and(|n| n == ".windsurf")
                && let Some(project) = path.parent()
            {
                notes.push(Issue {
                    kind: "legacy-windsurf-lobe",
                    target: entry.path().to_string(),
                    // The two commands are meant to be pasted, so each path
                    // is shell-quoted (a project path may hold spaces).
                    message: format!(
                        "lobe '{}' uses the legacy Windsurf skills dir; Devin reads \
                         '.devin/skills' first. To move: `mind config lobes remove {}` \
                         then `mind link-project {}`",
                        entry.path(),
                        crate::error::shell_quote(entry.path()),
                        crate::error::shell_quote(&project.to_string_lossy())
                    ),
                });
            }
        }
    }

    // For HARN-8 checks, only consider lobes whose parent dir still exists (skip
    // vanished lobes so they don't generate spurious missing-lobe-link findings).
    // spec: LOBE-3 -- per source: one with its own `lobe` is checked against
    // that lobe in place of the default one.
    // An item kept after its source was unmelded (`--keep-items`) is checked
    // against the lobe it recorded, unless a policy lobe lock rules (LOBE-4).
    let lobes_locked = paths.settings().policy()?.is_some_and(|p| p.lobes_lock());
    let live_lobes_for = |it: &crate::manifest::InstalledItem| -> Result<Vec<crate::paths::Lobe>> {
        let (own, lobes) = match (registry.find(&it.source), &it.lobe) {
            (None, Some(recorded)) if !lobes_locked => {
                let root = std::path::PathBuf::from(recorded);
                let lobes = paths.agent_homes_with_lobe(paths.agent_homes()?, root.clone())?;
                (Some(root), lobes)
            }
            _ => (
                paths.effective_source_lobe(&it.source)?,
                paths.agent_homes_for(&it.source)?,
            ),
        };
        Ok(lobes
            .into_iter()
            .filter(|l| l.reachable() || own.as_ref() == Some(&l.path))
            .collect())
    };

    for s in &registry.sources {
        let dir = s.clone_dir(paths);
        let git_metadata = dir.join(".git");
        // Git worktrees use a .git file pointing to their metadata directory.
        let has_clone = git_metadata.is_dir() || (git_metadata.is_file() && git::is_repo(&dir));
        if !has_clone {
            issues.push(Issue {
                kind: "no-clone",
                target: s.name.clone(),
                message: format!("source '{}' has no clone on disk; run `mind sync`", s.name),
            });
        } else if s.commit.is_none() {
            issues.push(Issue {
                kind: "never-synced",
                target: s.name.clone(),
                message: format!("source '{}' was never synced; run `mind sync`", s.name),
            });
        }
    }

    for it in manifest.items.values_mut() {
        let missing: Vec<String> = it
            .links
            .iter()
            .filter(|link| std::fs::symlink_metadata(link).is_err())
            .cloned()
            .collect();
        if !missing.is_empty() {
            // With --fix, re-link from the store copy; report only what cannot
            // be repaired (e.g. the store copy itself is gone).
            let n = if fix { install::relink(paths, it)? } else { 0 };
            if n > 0 {
                repaired.push(format!(
                    "{}: relinked {n} missing symlink(s)",
                    it.key().as_str()
                ));
            }
            for link in &missing {
                if std::fs::symlink_metadata(link).is_err() {
                    issues.push(Issue {
                        kind: "missing-link",
                        target: it.key().into(),
                        message: format!("{}: symlink missing at {link}", it.key().as_str()),
                    });
                }
            }
        }
        // HARN-8: check that the item is linked into every configured lobe that
        // admits its kind. A link that is in the manifest but not on disk is
        // already handled above (missing-link). This checks for lobes that were
        // added after the item was installed -- the link would be absent from
        // both the manifest and disk. Use live_lobes so vanished lobes don't
        // generate spurious missing-lobe-link findings (HARN-13).
        // spec: LOBE-2 -- a refused lobe is reported, once per source, and
        // leaves the source's items unchecked against the lobes rather than
        // failing the whole report.
        let live_lobes = match live_lobes_for(it) {
            Ok(lobes) => lobes,
            Err(e @ MindError::LobeNotAllowed { .. }) => {
                if !issues
                    .iter()
                    .any(|i| i.kind == "lobe-not-allowed" && i.target == it.source)
                {
                    issues.push(Issue {
                        kind: "lobe-not-allowed",
                        target: it.source.clone(),
                        message: strip_ansi(&e.to_string()),
                    });
                }
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let link_rel = it
            .links
            .iter()
            .find_map(|link_str| {
                let link = std::path::Path::new(link_str);
                live_lobes
                    .iter()
                    .find_map(|lobe| link.strip_prefix(&lobe.path).ok())
                    .map(|rel| rel.to_string_lossy().into_owned())
            })
            .or_else(|| paths.default_link_rel(it.kind, &it.name));
        if let Some(rel) = link_rel {
            for lobe in &live_lobes {
                if !lobe.admits(it.kind) {
                    continue;
                }
                let expected = lobe.path.join(&rel);
                let expected_str = expected.to_string_lossy().into_owned();
                let in_manifest = it.links.iter().any(|l| l == &expected_str);
                let on_disk = std::fs::symlink_metadata(&expected).is_ok();
                if in_manifest && on_disk {
                    continue;
                }
                if fix {
                    // HARN-17: `introspect --fix` has no `--force` flag, so a
                    // foreign file at the target is reported (below), never
                    // clobbered.
                    let (created, errs) =
                        install::link_into_new_lobes(paths, it, std::slice::from_ref(lobe), false);
                    if !created.is_empty() {
                        it.links.extend(
                            created
                                .into_iter()
                                .map(|p| p.to_string_lossy().into_owned()),
                        );
                        manifest_dirty = true;
                        repaired.push(format!(
                            "{}: linked into new lobe {}",
                            it.key().as_str(),
                            lobe.path.display()
                        ));
                    }
                    for (p, e) in errs {
                        issues.push(Issue {
                            kind: "missing-lobe-link",
                            target: it.key().into(),
                            message: format!("{}: {e}", p.display()),
                        });
                    }
                } else {
                    issues.push(Issue {
                        kind: "missing-lobe-link",
                        target: it.key().into(),
                        message: format!(
                            "{}: not linked into lobe {}; run `mind introspect --fix`",
                            it.key().as_str(),
                            lobe.path.display()
                        ),
                    });
                }
            }
        }
        // Match on stable identity (source, kind, bare_name).
        match catalog
            .iter()
            .find(|c| c.kind == it.kind && c.name == it.bare_name && c.source == it.source)
        {
            // spec: DSC-123 -- skipped by the scan for an unreadable manifest,
            // so still shipped: not a removal.
            None if catalog::dropped_as_unreadable(&it.source, it.kind, &it.bare_name) => issues
                .push(Issue {
                    kind: "unreadable-upstream",
                    target: it.key().into(),
                    message: format!(
                        "{}: its own mind.toml in source '{}' does not parse; it stays \
                         installed until the source fixes it",
                        it.key().as_str(),
                        it.source
                    ),
                }),
            None => issues.push(Issue {
                kind: "removed-upstream",
                target: it.key().into(),
                message: format!(
                    "{}: no longer present in source '{}'",
                    it.key().as_str(),
                    it.source
                ),
            }),
            Some(cat) => {
                if cat.effective_name() != it.name {
                    issues.push(Issue {
                        kind: "namespace-changed",
                        target: it.key().into(),
                        message: format!(
                            "{}: namespace changed to '{}'; run `mind upgrade`",
                            it.key().as_str(),
                            cat.effective_name()
                        ),
                    });
                } else if let Ok(h) = cat.content_hash()
                    && h != it.hash
                {
                    issues.push(Issue {
                        kind: "drifted",
                        target: it.key().into(),
                        message: format!(
                            "{}: upstream changed; run `mind upgrade`",
                            it.key().as_str()
                        ),
                    });
                }
            }
        }
    }

    // spec: LNK-19 -- an item installed with a dropped requirement (LNK-18) is
    // degraded, not broken, so it is reported as an issue `--fix` cannot repair:
    // the only fix is to replace the link with the whole repo, which is the
    // user's call (it changes which sources are registered). Reported after the
    // structural checks so it never displaces a broken link or a missing store.
    for it in manifest.items.values() {
        if it.dropped_requires.is_empty() {
            continue;
        }
        // spec: DSC-95 -- sanitize each source-controlled field BEFORE it is
        // composed into the message. The batch sanitize further down runs on
        // the already-joined string, which is too late: an identity or a
        // `requires` entry ending in a dangling escape introducer would swallow
        // the text that follows it.
        let source = strip_ansi(&it.source);
        let listed: String = it
            .dropped_requires
            .iter()
            .map(|e| strip_ansi(e))
            .collect::<Vec<_>>()
            .join(", ");
        issues.push(Issue {
            kind: "dropped-requires",
            target: it.key().into(),
            // No pasteable command carries the source identity: `recall`'s
            // positional resolves an ITEM against the manifest, so
            // `mind recall <source>` is not a command at all (H2), and
            // `mind recall --sources` needs no interpolation to be correct.
            message: format!(
                "{}: installed from a single-item link (source '{source}'), so its \
                 `requires {listed}` was dropped and the item it names is not installed; \
                 meld the whole repo to get both (`mind recall --sources` lists the melded \
                 sources)",
                it.display_key(),
            ),
        });
    }

    if fix && manifest_dirty {
        manifest.save(paths)?;
    }

    // spec: DSC-95 -- every issue's `target`/`message`, and every
    // `repaired` note, can embed a source-controlled name (an item key, a
    // source name, a lobe path): sanitize once here, covering both the
    // `--json` `Report` below and the human `println!` loop further down,
    // rather than at each of this function's dozen build sites above.
    for issue in issues.iter_mut().chain(notes.iter_mut()) {
        issue.target = strip_ansi(&issue.target);
        issue.message = strip_ansi(&issue.message);
    }
    for note in &mut repaired {
        *note = strip_ansi(note);
    }

    if json {
        #[derive(Serialize)]
        struct Report<'a> {
            // spec: CLI-189 - schema version; always 1.
            schema: u8,
            issues: &'a [Issue],
            // spec: HARN-26 -- advice that is not an issue; omitted when empty.
            #[serde(skip_serializing_if = "<[Issue]>::is_empty")]
            notes: &'a [Issue],
            sources: usize,
            items: usize,
        }
        return print_json(&Report {
            schema: 1,
            issues: &issues,
            notes: &notes,
            sources: registry.sources.len(),
            items: manifest.items.len(),
        });
    }

    let out = crate::render::ctx();
    for note in &repaired {
        println!("{} {note}", out.ok());
    }
    for issue in &issues {
        println!("{} {}", out.warn(), issue.message);
    }
    for note in &notes {
        println!("note: {}", note.message);
    }
    if issues.is_empty() {
        println!(
            "{} all good: {} source(s), {} item(s) installed",
            out.ok(),
            registry.sources.len(),
            manifest.items.len()
        );
    } else {
        println!("\n{} {} issue(s) found", out.err(), issues.len());
    }
    Ok(())
}
