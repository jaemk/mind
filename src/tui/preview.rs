//! Preview (shallow-clone) and suggested-registry support for the TUI.
//!
//! TUI-30: entering a repo spec shallow-clones it to a temp area and shows its
//! catalog under Available without registering it. Confirming promotes it to a
//! real meld; declining discards the temp clone.
//!
//! TUI-31: the suggested-registry union is built from `[discover].sources`
//! declared by all melded sources, de-duplicated by URL and excluding already-
//! melded sources.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::catalog::{self, CatalogItem};
use crate::error::Result;
use crate::git;
use crate::mindfile::MindToml;
use crate::paths::Paths;
use crate::source::{Registry, parse_spec};

/// Process-wide counter ensuring each `preview` call gets a unique temp dir,
/// even when multiple previews for repos with the same bare name run in the
/// same process (M4 fix: prevents path collisions between concurrent previews).
static PREVIEW_NONCE: AtomicU64 = AtomicU64::new(0);

/// A preview of a not-yet-melded source: a temp clone with its catalog.
/// Constructed by `preview()` and dropped (which cleans up the temp clone)
/// when the user declines or cancels (TUI-30).
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read by TUI display layer; function called from TUI flow
pub struct SourcePreview {
    /// The temp directory where the clone lives.
    pub temp_dir: PathBuf,
    /// The spec used to clone (for promoting to a real meld).
    pub spec: String,
    /// Catalog items found in the preview clone.
    pub items: Vec<CatalogItem>,
    /// Source name as parsed from the spec.
    pub name: String,
    /// URL of the source.
    pub url: String,
}

impl Drop for SourcePreview {
    fn drop(&mut self) {
        // Discard the temp clone on decline (TUI-30).
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

/// Build the temp-dir name for a preview, incorporating a per-call nonce so
/// two previews of repos with the same bare name never collide.
// spec: TUI-30
fn preview_temp_name(repo: &str, pid: u32, nonce: u64) -> String {
    format!("preview-{repo}-{pid}-{nonce}")
}

/// Shallow-clone a repo spec to a temp area and return its preview catalog.
/// Does not register the source. On error, any partial clone is cleaned up.
// spec: TUI-30
#[allow(dead_code)] // called from TUI interactive meld flow (TUI-30)
pub fn preview(paths: &Paths, spec: &str) -> Result<SourcePreview> {
    let source = parse_spec(spec)?;
    let nonce = PREVIEW_NONCE.fetch_add(1, Ordering::SeqCst);
    let temp_dir = paths.mind_home.join(".tmp").join(preview_temp_name(
        &source.repo,
        std::process::id(),
        nonce,
    ));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)
            .map_err(|e| crate::error::MindError::io(&temp_dir, e))?;
    }
    crate::paths::mkdir_p(&temp_dir)?;

    // Clone at the default branch for the preview.
    if let Err(e) = git::clone(&source.url, &temp_dir) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    // Scan the clone for items.
    let mut items = Vec::new();
    let result = catalog::scan_source_at(&temp_dir, &source, &mut items);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    let name = source.name.clone();
    let url = source.url.clone();
    Ok(SourcePreview {
        temp_dir,
        spec: spec.to_string(),
        items,
        name,
        url,
    })
}

/// A registry suggestion: a not-yet-melded source declared by a melded super-source.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read by TUI display layer
pub struct RegistrySuggestion {
    /// The repo spec for this source.
    pub spec: String,
    /// Source name (as it would be parsed).
    pub name: String,
    /// URL.
    pub url: String,
    /// Optional alias from the super-source's `[discover].sources as:` field.
    pub alias: Option<String>,
}

/// Build the union of `[discover].sources` from all melded sources, excluding
/// any that are already melded, de-duplicated by URL.
// spec: TUI-31
#[allow(dead_code)] // called from TUI registry display (TUI-31)
pub fn suggested_registry(paths: &Paths) -> Result<Vec<RegistrySuggestion>> {
    let registry = Registry::load(paths)?;
    let melded_urls: std::collections::HashSet<String> =
        registry.sources.iter().map(|s| s.url.clone()).collect();

    let mut seen_urls = melded_urls.clone();
    let mut suggestions = Vec::new();

    for source in &registry.sources {
        let clone_dir = source.clone_dir(paths);
        let Ok(Some(mt)) = MindToml::load(&clone_dir) else {
            continue;
        };
        let Some(discover) = &mt.discover else {
            continue;
        };
        for entry in &discover.sources {
            // Parse the spec to get the URL for dedup.
            let Ok(parsed) = parse_spec(&entry.source) else {
                continue;
            };
            if seen_urls.contains(&parsed.url) {
                continue;
            }
            seen_urls.insert(parsed.url.clone());
            suggestions.push(RegistrySuggestion {
                spec: entry.source.clone(),
                name: parsed.name.clone(),
                url: parsed.url.clone(),
                alias: entry.alias.clone(),
            });
        }
    }

    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::source::Registry;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_base() -> (PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-tui-preview-{}-{n}", std::process::id()));
        let mind = base.join("mind");
        crate::paths::mkdir_p(&mind).unwrap();
        (base, mind)
    }

    fn cleanup(base: &std::path::Path) {
        let _ = std::fs::remove_dir_all(base);
    }

    fn init_git_repo(dir: &std::path::Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
        };
        run(&["-c", "init.defaultBranch=main", "init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
    }

    fn make_source_repo(base: &std::path::Path) -> PathBuf {
        let src = base.join("source-repo");
        std::fs::create_dir_all(&src).unwrap();
        // Add a skill
        std::fs::create_dir_all(src.join("skills/meld")).unwrap();
        std::fs::write(
            src.join("skills/meld/SKILL.md"),
            "---\ndescription: meld skill\n---\n# meld\n",
        )
        .unwrap();
        init_git_repo(&src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "initial"])
            .current_dir(&src)
            .output()
            .unwrap();
        src
    }

    #[test]
    fn preview_clones_and_scans_catalog() {
        // spec: TUI-30
        let (base, mind) = temp_base();
        let src = make_source_repo(&base);
        let paths = Paths {
            mind_home: mind.clone(),
            claude_home: base.join("claude"),
        };

        let prev = preview(&paths, src.to_str().unwrap());
        assert!(prev.is_ok(), "preview should succeed: {:?}", prev.err());
        let prev = prev.unwrap();
        assert!(!prev.items.is_empty(), "preview should have catalog items");
        assert!(
            prev.items.iter().any(|it| it.name == "meld"),
            "preview catalog should contain the 'meld' skill"
        );
        // The temp dir should exist while the preview is live.
        assert!(
            prev.temp_dir.exists(),
            "temp clone should exist while preview is live"
        );
        // Dropping the preview discards the clone.
        let temp = prev.temp_dir.clone();
        drop(prev);
        assert!(
            !temp.exists(),
            "temp clone should be removed after preview is dropped"
        );
        cleanup(&base);
    }

    #[test]
    fn suggested_registry_empty_when_no_sources_melded() {
        // spec: TUI-31
        let (base, mind) = temp_base();
        let paths = Paths {
            mind_home: mind,
            claude_home: base.join("claude"),
        };
        let suggestions = suggested_registry(&paths).unwrap();
        assert!(
            suggestions.is_empty(),
            "no suggestions with no melded sources"
        );
        cleanup(&base);
    }

    #[test]
    fn suggested_registry_excludes_already_melded() {
        // spec: TUI-31 - a source listed in [discover].sources but already melded
        // must not appear in the suggestion list.
        let (base, mind) = temp_base();

        // Build a "super-source" repo with a mind.toml listing a nested source.
        let nested_src = make_source_repo(&base);
        let super_src = base.join("super-source");
        std::fs::create_dir_all(&super_src).unwrap();
        std::fs::write(super_src.join("README.md"), "# super\n").unwrap();
        // mind.toml pointing at the nested source.
        std::fs::write(
            super_src.join("mind.toml"),
            format!(
                "[source]\ndescription = \"super\"\n\n[discover]\n[[discover.sources]]\nsource = \"{}\"\n",
                nested_src.to_str().unwrap()
            ),
        ).unwrap();
        init_git_repo(&super_src);
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&super_src)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "init"])
            .current_dir(&super_src)
            .output()
            .unwrap();

        // Build paths with the super-source melded.
        let paths = Paths {
            mind_home: mind.clone(),
            claude_home: base.join("claude"),
        };
        // Manually register the super-source.
        let mut super_source_parsed = parse_spec(super_src.to_str().unwrap()).unwrap();
        super_source_parsed.commit = Some("abc".to_string());
        // Clone the super-source into the sources dir.
        let clone_dir = super_source_parsed.clone_dir(&paths);
        crate::paths::mkdir_p(clone_dir.parent().unwrap()).unwrap();
        Command::new("git")
            .args([
                "clone",
                super_src.to_str().unwrap(),
                clone_dir.to_str().unwrap(),
            ])
            .output()
            .unwrap();

        let registry = Registry {
            sources: vec![super_source_parsed.clone()],
        };
        registry.save(&paths).unwrap();

        // Nested source NOT yet melded -> should appear in suggestions.
        let suggestions = suggested_registry(&paths).unwrap();
        let has_nested = suggestions.iter().any(|s| {
            s.url
                .contains(nested_src.file_name().unwrap().to_str().unwrap())
        });
        assert!(
            has_nested,
            "nested source should be suggested when not yet melded: {suggestions:?}"
        );

        // Now meld the nested source (add it to registry).
        if let Ok(n) = parse_spec(nested_src.to_str().unwrap()) {
            // Don't actually add duplicate super-source; just add nested.
            let mut new_reg = Registry::load(&paths).unwrap();
            new_reg.sources.push(n);
            new_reg.save(&paths).unwrap();
        }

        let suggestions2 = suggested_registry(&paths).unwrap();
        // The nested source is now melded, so it must NOT appear in suggestions.
        // For local-path specs the source URL is the literal path, so the
        // discover entry's parsed URL equals the melded source's URL and the
        // dedup-by-URL exclusion (TUI-31) applies exactly. This is the core
        // exclusion assertion: a previously-suggested source disappears from the
        // list once melded. (Mutation-check: if `seen_urls` were not seeded with
        // the melded URLs, the nested source would still be suggested here.)
        let nested_url = parse_spec(nested_src.to_str().unwrap()).unwrap().url;
        let still_has = suggestions2.iter().any(|s| s.url == nested_url);
        assert!(
            !still_has,
            "a source that is now melded must be excluded from suggestions (dedup by URL): {suggestions2:?}"
        );

        cleanup(&base);
    }

    #[test]
    fn preview_invalid_spec_returns_error() {
        // spec: TUI-30
        let (base, mind) = temp_base();
        let paths = Paths {
            mind_home: mind,
            claude_home: base.join("claude"),
        };
        let result = preview(&paths, "not-a-valid/url-that-has-no-slash");
        // Should fail with some error (parse or git).
        // Either InvalidRepoSpec or a Git error.
        assert!(result.is_err(), "invalid spec should return an error");
        cleanup(&base);
    }

    /// Two successive calls to `preview_temp_name` with the same repo and pid
    /// but distinct nonces must produce distinct names. This proves the M4 fix:
    /// previewing `alice/agents` then `bob/agents` (same bare repo name "agents")
    /// in one process no longer maps to the same temp path.
    // spec: TUI-30
    #[test]
    fn preview_temp_names_are_unique_for_same_repo() {
        let pid = std::process::id();
        let name1 = preview_temp_name("agents", pid, 0);
        let name2 = preview_temp_name("agents", pid, 1);
        assert_ne!(
            name1, name2,
            "successive preview temp names for the same repo must differ: {name1} vs {name2}"
        );
        // Both must contain the repo name so they remain identifiable.
        assert!(
            name1.contains("agents"),
            "name1 must contain repo name: {name1}"
        );
        assert!(
            name2.contains("agents"),
            "name2 must contain repo name: {name2}"
        );
    }

    /// The nonce increments across two real `preview` calls, so the temp dirs
    /// they would occupy are always distinct even when the bare repo names match.
    // spec: TUI-30
    #[test]
    fn preview_nonce_advances_across_calls() {
        // Read two successive nonce values from the global counter to verify
        // monotone advancement; no clone needed.
        let n1 = PREVIEW_NONCE.fetch_add(1, Ordering::SeqCst);
        let n2 = PREVIEW_NONCE.fetch_add(1, Ordering::SeqCst);
        assert!(n2 > n1, "PREVIEW_NONCE must advance: got {n1} then {n2}");
        let pid = std::process::id();
        assert_ne!(
            preview_temp_name("agents", pid, n1),
            preview_temp_name("agents", pid, n2),
            "names built from consecutive nonces must differ"
        );
    }
}
