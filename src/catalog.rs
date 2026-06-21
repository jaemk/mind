//! Scanning melded sources for installable items.
//!
//! By convention (mirrors the `agents` repo layout):
//! - `skills/<name>/SKILL.md`  -> skill `<name>`
//! - `agents/<name>.md`        -> agent `<name>`
//! - `rules/<name>.md`         -> rule  `<name>`
//!
//! A source may instead ship a `mind.toml` (see [`crate::mindfile`]) declaring
//! its inventory explicitly via `[[items]]` or `[discover]` globs; that takes
//! over discovery for the source. Either way, an item's `description` is read
//! from its frontmatter unless overridden.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{ItemKind, MindError, Result};
use crate::frontmatter;
use crate::mindfile::{Discover, ItemDecl, KindGlobs, MindToml};
use crate::namespace;
use crate::paths::Paths;
use crate::source::{Registry, Source};

/// One installable item discovered in a source.
///
/// The catalog is source truth: `name` is the item's *bare* name exactly as it
/// appears in the repo. The namespace prefix and `{{ns:}}` token expansion are
/// install-time transforms, applied by `install.rs`, not baked in here. The
/// stable identity of an item is therefore `(source, kind, name)`, which is what
/// `upgrade` matches on across a prefix change.
#[derive(Debug, Clone)]
pub struct CatalogItem {
    pub kind: ItemKind,
    /// Bare name as it appears in the source.
    pub name: String,
    /// The source `name` it belongs to.
    pub source: String,
    /// The source's effective namespace prefix, if any (applied at install).
    pub prefix: Option<String>,
    /// Path to the item root on disk (a dir for skills, a file for agents/rules).
    pub path: PathBuf,
    /// One-line description, from frontmatter or a `mind.toml` override.
    pub description: Option<String>,
    /// Optional link target relative to `~/.claude` (from `mind.toml`); `None`
    /// means use the default location for the kind.
    pub link_rel: Option<String>,
}

impl CatalogItem {
    /// The name this item installs under: bare, or `<prefix>-<bare>` if namespaced.
    pub fn effective_name(&self) -> String {
        namespace::apply(&self.name, &self.prefix)
    }

    /// User-facing key, using the effective (possibly prefixed) name.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.effective_name())
    }
}

/// True when `query` matches the item by effective name or description,
/// case-insensitively. An empty query matches everything. (spec: CLI-85)
pub(crate) fn matches_query(item: &CatalogItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if item.effective_name().to_lowercase().contains(&q) {
        return true;
    }
    item.description
        .as_deref()
        .is_some_and(|d| d.to_lowercase().contains(&q))
}

/// Scan every melded source for installable items.
pub fn scan(paths: &Paths, registry: &Registry) -> Result<Vec<CatalogItem>> {
    let mut items = Vec::new();
    for source in &registry.sources {
        scan_source(paths, source, &mut items)?;
    }
    Ok(items)
}

pub(crate) fn scan_source(
    paths: &Paths,
    source: &Source,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    let clone_root = source.clone_dir(paths);
    scan_source_at(clone_root, source, out)
}

/// Scan a source whose clone root is known directly (e.g. for `review`, where
/// the directory may not live under the standard sources tree).
pub(crate) fn scan_source_at(
    clone_root: impl AsRef<std::path::Path>,
    source: &Source,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    let clone_root = clone_root.as_ref();
    let mindfile = MindToml::load(clone_root)?;

    // Reject a source that requires a newer `mind` than the one running, rather
    // than scanning it against a format this version may predate (DSC-40).
    if let Some(required) = mindfile
        .as_ref()
        .and_then(|m| m.source.min_mind_version.as_deref())
        && !crate::mindfile::version_at_least(env!("CARGO_PKG_VERSION"), required)
    {
        return Err(MindError::IncompatibleVersion {
            source_name: source.name.clone(),
            required: required.to_string(),
            running: env!("CARGO_PKG_VERSION").to_string(),
        });
    }

    // Effective prefix: consumer alias wins over the repo's own declaration.
    let prefix = source
        .alias
        .clone()
        .or_else(|| mindfile.as_ref().and_then(|m| m.source.prefix.clone()));

    match mindfile {
        Some(mt) if mt.is_authoritative() => {
            // spec: DSC-52 — authoritative mind.toml ignores scan roots entirely;
            // its paths are always repo-root-relative.
            for decl in &mt.items {
                out.push(from_decl(clone_root, source, &prefix, decl)?);
            }
            if let Some(discover) = &mt.discover {
                scan_globs(clone_root, source, &prefix, discover, out)?;
            }
            Ok(())
        }
        ref mt => {
            // spec: DSC-50 / DSC-51 — resolve the effective scan roots:
            //   source.roots (--root override) wins; else mindfile [source].roots;
            //   else implicit single root of the repo root.
            let effective_roots: Vec<String> = source
                .roots
                .clone()
                .or_else(|| mt.as_ref().and_then(|m| m.source.roots.clone()))
                .unwrap_or_else(|| vec![".".to_string()]);

            // Validate each root: must exist as a directory inside the clone and
            // must not be absolute or escape the clone via `..`.
            for r in &effective_roots {
                if std::path::Path::new(r).is_absolute() {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
                let full = clone_root.join(r);
                // Reject paths that try to escape via `..`.
                if !full
                    .canonicalize()
                    .unwrap_or_else(|_| full.clone())
                    .starts_with(
                        clone_root
                            .canonicalize()
                            .unwrap_or_else(|_| clone_root.to_path_buf()),
                    )
                {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
                if !full.is_dir() {
                    return Err(MindError::InvalidRoot {
                        source_name: source.name.clone(),
                        root: r.clone(),
                    });
                }
            }

            // spec: DSC-53 — scan each root and union the results. Detect a
            // (kind, bare_name) collision within this source.
            let pre_scan_len = out.len();
            for r in &effective_roots {
                let scan_root = clone_root.join(r);
                scan_convention(&scan_root, source, &prefix, out)?;
            }
            // Check for duplicates among items contributed by this source.
            let new_items = &out[pre_scan_len..];
            let mut seen: std::collections::HashSet<(crate::error::ItemKind, String)> =
                std::collections::HashSet::new();
            for item in new_items {
                let key = (item.kind, item.name.clone());
                if !seen.insert(key.clone()) {
                    return Err(MindError::DuplicateItem {
                        source_name: source.name.clone(),
                        kind: key.0,
                        name: key.1,
                    });
                }
            }
            Ok(())
        }
    }
}

/// Build a catalog item from an explicit `[[items]]` declaration.
fn from_decl(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    decl: &ItemDecl,
) -> Result<CatalogItem> {
    let kind = ItemKind::parse(&decl.kind).ok_or_else(|| MindError::MindToml {
        path: root.join("mind.toml"),
        msg: format!("unknown item kind '{}' for '{}'", decl.kind, decl.name),
    })?;
    let path = root.join(&decl.path);
    let description = decl
        .description
        .clone()
        .or_else(|| frontmatter::description(&meta_file(kind, &path)));
    Ok(CatalogItem {
        kind,
        name: decl.name.clone(),
        source: source.name.clone(),
        prefix: prefix.clone(),
        path,
        description,
        link_rel: decl.link.clone(),
    })
}

/// Discover items by glob, relative to the repo root. Nested `sources` are
/// handled at meld time, not here.
fn scan_globs(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    discover: &Discover,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    for skill_md in resolve_globs(root, &discover.skills)? {
        // The glob points at the SKILL.md; the item is its parent dir.
        if let Some(dir) = skill_md.parent() {
            out.push(make_item(
                source,
                prefix,
                ItemKind::Skill,
                dir.to_path_buf(),
                &skill_md,
            ));
        }
    }
    for (kind, globs) in [
        (ItemKind::Agent, &discover.agents),
        (ItemKind::Rule, &discover.rules),
    ] {
        for md in resolve_globs(root, globs)? {
            out.push(make_item(source, prefix, kind, md.clone(), &md));
        }
    }
    Ok(())
}

/// Expand a kind's include globs, then drop anything its exclude globs match.
fn resolve_globs(root: &Path, globs: &KindGlobs) -> Result<Vec<PathBuf>> {
    let mut included = BTreeSet::new();
    for pattern in &globs.include {
        included.extend(glob_paths(root, pattern)?);
    }
    let mut excluded = BTreeSet::new();
    for pattern in &globs.exclude {
        excluded.extend(glob_paths(root, pattern)?);
    }
    Ok(included.difference(&excluded).cloned().collect())
}

/// Convention scan: fixed `skills/`, `agents/`, `rules/` directories.
fn scan_convention(
    root: &Path,
    source: &Source,
    prefix: &Option<String>,
    out: &mut Vec<CatalogItem>,
) -> Result<()> {
    let skills_dir = root.join("skills");
    for entry in read_dir_opt(&skills_dir)? {
        let skill_md = entry.join("SKILL.md");
        if entry.is_dir() && skill_md.is_file() {
            out.push(make_item(source, prefix, ItemKind::Skill, entry, &skill_md));
        }
    }

    for (kind, dir) in [(ItemKind::Agent, "agents"), (ItemKind::Rule, "rules")] {
        let kind_dir = root.join(dir);
        for entry in read_dir_opt(&kind_dir)? {
            if entry.is_file() && entry.extension().is_some_and(|e| e == "md") {
                out.push(make_item(source, prefix, kind, entry.clone(), &entry));
            }
        }
    }
    Ok(())
}

/// Build a [`CatalogItem`], deriving its bare name from the path and its
/// description from `meta_file`'s frontmatter, then applying the prefix.
fn make_item(
    source: &Source,
    prefix: &Option<String>,
    kind: ItemKind,
    path: PathBuf,
    meta: &Path,
) -> CatalogItem {
    let bare = match kind {
        ItemKind::Skill => file_name(&path),
        _ => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    CatalogItem {
        kind,
        name: bare,
        source: source.name.clone(),
        prefix: prefix.clone(),
        path,
        description: frontmatter::description(meta),
        link_rel: None,
    }
}

/// The file whose frontmatter describes an item (SKILL.md for skills).
fn meta_file(kind: ItemKind, path: &Path) -> PathBuf {
    match kind {
        ItemKind::Skill => path.join("SKILL.md"),
        _ => path.to_path_buf(),
    }
}

/// Expand a glob pattern rooted at `root`, returning sorted matches.
fn glob_paths(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let joined = root.join(pattern);
    let full = joined.to_string_lossy();
    let paths = glob::glob(&full).map_err(|e| MindError::MindToml {
        path: root.join("mind.toml"),
        msg: format!("bad discover glob '{pattern}': {e}"),
    })?;
    let mut out = Vec::new();
    for entry in paths {
        match entry {
            Ok(p) => out.push(p),
            Err(e) => {
                let path = e.path().to_path_buf();
                return Err(MindError::io(path, e.into_error()));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Read a directory's entries, treating "not found" as empty.
fn read_dir_opt(dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut paths = Vec::new();
            for entry in rd {
                let entry = entry.map_err(|e| MindError::io(dir, e))?;
                paths.push(entry.path());
            }
            paths.sort();
            Ok(paths)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(MindError::io(dir, e)),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ItemKind;
    use crate::paths::Paths;
    use crate::source::{Pin, Source};
    use std::path::PathBuf;

    // ---- scan roots unit tests (DSC-50, DSC-51, DSC-52, DSC-53) -------

    use std::sync::atomic::{AtomicU32, Ordering};
    static UNIT_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Allocate a unique temp dir for a unit test and return a guard that
    /// removes it on drop (via a wrapper struct).
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let n = UNIT_COUNTER.fetch_add(1, Ordering::SeqCst);
            let p =
                std::env::temp_dir().join(format!("mind-catalog-unit-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Create a minimal `Source` for a local path fixture.
    fn make_source_for(clone: &std::path::Path) -> Source {
        Source {
            name: "local/test/repo".to_string(),
            url: clone.to_string_lossy().into_owned(),
            host: "local".to_string(),
            owner: "test".to_string(),
            repo: "repo".to_string(),
            commit: None,
            description: None,
            alias: None,
            pin: Pin::default(),
            roots: None,
            install_hook: None,
            install_hook_commit: None,
        }
    }

    /// Create a `Paths` whose sources dir is `base/sources`, so that
    /// the clone of `local/test/repo` lives at `base/sources/local/test/repo`.
    fn paths_for(base: &std::path::Path) -> Paths {
        Paths {
            mind_home: base.to_path_buf(),
            claude_home: base.join("claude"),
        }
    }

    fn write_file(path: &std::path::Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn convention_discovery_under_single_explicit_root() {
        // spec: DSC-50 DSC-53
        // When [source].roots = ["tools"], items in tools/skills/ etc. are found.
        let tmp = TmpDir::new();
        let base = tmp.path();

        // The clone lands at base/sources/local/test/repo.
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/meld/SKILL.md"),
            "---\ndescription: meld skill\n---\n# meld\n",
        );
        write_file(
            &clone.join("tools/agents/do.md"),
            "---\ndescription: do agent\n---\n# do\n",
        );
        // Write a mind.toml with roots = ["tools"].
        write_file(&clone.join("mind.toml"), "[source]\nroots = [\"tools\"]\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);
        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"meld"), "expected 'meld': {names:?}");
        assert!(names.contains(&"do"), "expected 'do': {names:?}");
        // No items from the repo root (no skills/ at root).
        assert!(!names.contains(&"review"), "unexpected 'review': {names:?}");
    }

    #[test]
    fn source_roots_override_beats_mindfile_roots() {
        // spec: DSC-51 STO-17
        // Source.roots (--root override) takes precedence over [source].roots in mind.toml.
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // Items only under "a/".
        write_file(
            &clone.join("a/skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );
        // Items only under "b/".
        write_file(
            &clone.join("b/skills/beta/SKILL.md"),
            "---\ndescription: beta\n---\n# beta\n",
        );
        // mind.toml says roots = ["b"], but the source override says ["a"].
        write_file(&clone.join("mind.toml"), "[source]\nroots = [\"b\"]\n");

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // Consumer --root override points at "a".
        source.roots = Some(vec!["a".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"alpha"),
            "override root 'a' expected: {names:?}"
        );
        assert!(
            !names.contains(&"beta"),
            "toml root 'b' should be ignored: {names:?}"
        );
    }

    #[test]
    fn two_roots_are_unioned() {
        // spec: DSC-53
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("a/skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\n# alpha\n",
        );
        write_file(
            &clone.join("b/skills/beta/SKILL.md"),
            "---\ndescription: beta\n---\n# beta\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "expected alpha: {names:?}");
        assert!(names.contains(&"beta"), "expected beta: {names:?}");
    }

    #[test]
    fn duplicate_item_across_roots_is_an_error() {
        // spec: DSC-53
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // "review" skill under both "a/" and "b/".
        write_file(
            &clone.join("a/skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        write_file(
            &clone.join("b/skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "review"),
            "expected DuplicateItem: {err}"
        );
    }

    #[test]
    fn non_directory_root_is_invalid_root_error() {
        // spec: DSC-52
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["nonexistent".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "nonexistent"),
            "expected InvalidRoot: {err}"
        );
    }

    #[test]
    fn authoritative_mind_toml_ignores_roots() {
        // spec: DSC-52
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone = base.join("sources/local/test/repo");
        // A rule declared explicitly in mind.toml.
        write_file(
            &clone.join("guidelines/style.md"),
            "---\ndescription: style rule\n---\n# style\n",
        );
        // Convention items under "sub/".
        write_file(
            &clone.join("sub/skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );
        write_file(
            &clone.join("mind.toml"),
            concat!(
                "[[items]]\n",
                "kind = \"rule\"\n",
                "name = \"style\"\n",
                "path = \"guidelines/style.md\"\n",
            ),
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // Consumer root pointing at "sub/" -- should be ignored for authoritative source.
        source.roots = Some(vec!["sub".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();

        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        // Only the explicitly declared item; the convention root is ignored.
        assert!(names.contains(&"style"), "expected 'style': {names:?}");
        assert!(
            !names.contains(&"review"),
            "convention scan should be ignored: {names:?}"
        );
    }

    #[test]
    fn absolute_root_pointing_inside_the_clone_is_still_invalid() {
        // spec: DSC-52 CLI-16
        // A root must be repo-root-relative. An ABSOLUTE path is rejected even
        // when it names a real directory INSIDE the clone -- so only the
        // is_absolute() guard can catch it (the escape and is_dir checks would
        // both pass). This isolates the absolute-path guard from the escape guard.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/build/SKILL.md"),
            "---\ndescription: build\n---\n# build\n",
        );
        // The clone path must canonicalize stably (no symlinks in temp here).
        let abs_inside = clone.join("tools").canonicalize().unwrap();
        assert!(abs_inside.is_absolute());

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec![abs_inside.to_string_lossy().into_owned()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { .. }),
            "an absolute root, even inside the clone, must be InvalidRoot: {err}"
        );
        assert!(items.is_empty(), "absolute root must contribute nothing");
    }

    #[test]
    fn absolute_root_outside_the_clone_is_invalid() {
        // spec: DSC-52 CLI-16
        // The plain case: an absolute path outside the clone is rejected.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["/tmp".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "/tmp"),
            "absolute root outside the clone must be InvalidRoot: {err}"
        );
    }

    #[test]
    fn parent_escaping_root_to_existing_sibling_is_invalid_root() {
        // spec: DSC-52 CLI-16
        // The escape guard must reject a `..` root that resolves to a real
        // directory OUTSIDE the clone. This is the adversarial case the is_dir()
        // check alone cannot catch (the sibling exists), so only the
        // canonicalize/starts_with guard stands between it and a read outside the
        // clone.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        // A sibling clone dir that exists and even has scannable items.
        let sibling = base.join("sources/local/test/other");
        write_file(
            &sibling.join("skills/leak/SKILL.md"),
            "---\ndescription: leaked\n---\n# leak\n",
        );
        std::fs::create_dir_all(&clone).unwrap();

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        // ../other escapes the clone but points at an existing directory.
        source.roots = Some(vec!["../other".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRoot { ref root, .. } if root == "../other"),
            "escaping root must be InvalidRoot, not a silent read outside the clone: {err}"
        );
        assert!(
            items.is_empty(),
            "no items should leak from outside the clone"
        );
    }

    #[test]
    fn in_clone_dotdot_root_is_allowed() {
        // spec: DSC-50 DSC-52
        // A `..` segment that stays inside the clone (`tools/../tools`) is a
        // legitimate in-clone path and must be accepted, distinguishing it from a
        // genuinely escaping `../x`. Mirror test of the escape rejection: this
        // pins that the guard is not over-broad (rejecting all `..`).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("tools/skills/build/SKILL.md"),
            "---\ndescription: build\n---\n# build\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.roots = Some(vec!["tools/../tools".to_string()]);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"build"),
            "in-clone .. should resolve: {names:?}"
        );
    }

    #[test]
    fn duplicate_item_check_is_scoped_to_one_source() {
        // spec: DSC-53
        // The (kind, bare_name) duplicate check is per-source: a `review` skill in
        // source A and a `review` skill in source B is NOT a DuplicateItem -- only
        // a collision WITHIN one source's roots is. Regression guard: if the dedup
        // scanned `out` from index 0 instead of this source's slice, this would
        // wrongly error.
        let tmp = TmpDir::new();
        let base = tmp.path();

        let clone_a = base.join("sources/local/test/repo");
        write_file(
            &clone_a.join("skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        let clone_b = base.join("sources/local/other/repo");
        write_file(
            &clone_b.join("skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source_a = make_source_for(&clone_a);
        let mut source_b = make_source_for(&clone_b);
        source_b.name = "local/other/repo".to_string();
        source_b.owner = "other".to_string();

        let mut items = Vec::new();
        scan_source(&paths, &source_a, &mut items).unwrap();
        // Scanning B into the same `out` that already holds A's `review` must not
        // be seen as a duplicate.
        scan_source(&paths, &source_b, &mut items)
            .expect("same name in a different source is not a DuplicateItem");
        let reviews = items.iter().filter(|i| i.name == "review").count();
        assert_eq!(reviews, 2, "both sources' review items should be present");
    }

    #[test]
    fn duplicate_across_roots_collides_on_bare_name_under_a_prefix() {
        // spec: DSC-53
        // The duplicate check is on the BARE name, independent of any namespace
        // prefix: two roots each contributing a bare `review` collide even when
        // the source has a prefix/alias (which would prefix both identically).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("a/skills/review/SKILL.md"),
            "---\ndescription: review a\n---\n# review\n",
        );
        write_file(
            &clone.join("b/skills/review/SKILL.md"),
            "---\ndescription: review b\n---\n# review\n",
        );

        let paths = paths_for(base);
        let mut source = make_source_for(&clone);
        source.alias = Some("jk".to_string()); // a namespace prefix is in effect
        source.roots = Some(vec!["a".to_string(), "b".to_string()]);

        let mut items = Vec::new();
        let err = scan_source(&paths, &source, &mut items).unwrap_err();
        assert!(
            matches!(err, MindError::DuplicateItem { ref name, .. } if name == "review"),
            "bare-name collision must error regardless of prefix: {err}"
        );
    }

    #[test]
    fn explicit_empty_roots_list_discovers_nothing() {
        // spec: DSC-50
        // DSC-50 says an UNSET `roots` means a single implicit repo root. An
        // explicitly empty list (`roots = []`) is distinct: it is honored as
        // "scan zero roots", so nothing is discovered. This pins the
        // unset-vs-explicit-empty fork rather than letting [] silently fall back
        // to the repo root. See certification note (spec ambiguity).
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        // A conventional item at the repo root: it WOULD be found by the implicit
        // root, so if [] fell back to the repo root this item would appear.
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );
        write_file(&clone.join("mind.toml"), "[source]\nroots = []\n");

        let paths = paths_for(base);
        let source = make_source_for(&clone);

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        assert!(
            items.is_empty(),
            "an explicit empty roots list scans zero roots: {:?}",
            items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unset_roots_falls_back_to_implicit_repo_root() {
        // spec: DSC-50
        // The counterpart to the empty-list case: with no roots configured at all,
        // discovery scans the repo root (the DSC-10..13 behavior). This is the
        // mutation guard distinguishing `None` (implicit ["."]) from `Some([])`.
        let tmp = TmpDir::new();
        let base = tmp.path();
        let clone = base.join("sources/local/test/repo");
        write_file(
            &clone.join("skills/review/SKILL.md"),
            "---\ndescription: review\n---\n# review\n",
        );

        let paths = paths_for(base);
        let source = make_source_for(&clone); // roots: None, no mind.toml

        let mut items = Vec::new();
        scan_source(&paths, &source, &mut items).unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(
            names.contains(&"review"),
            "unset roots scans the repo root: {names:?}"
        );
    }

    fn make_test_item(name: &str, description: Option<&str>) -> CatalogItem {
        CatalogItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            source: "test-source".to_string(),
            prefix: None,
            path: PathBuf::from("/tmp/fake"),
            description: description.map(|s| s.to_string()),
            link_rel: None,
        }
    }

    #[test]
    fn empty_query_matches_all() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(matches_query(&item, ""));
    }

    #[test]
    fn matches_by_effective_name() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(matches_query(&item, "review"));
    }

    #[test]
    fn matches_by_description_when_name_does_not_contain_query() {
        // spec: CLI-85
        // "bugs" is only in the description, not the name
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(!item.effective_name().contains("bugs"));
        assert!(matches_query(&item, "bugs"));
    }

    #[test]
    fn match_is_case_insensitive_on_name() {
        // spec: CLI-85
        let item = make_test_item("Review", None);
        assert!(matches_query(&item, "REVIEW"));
        assert!(matches_query(&item, "review"));
        assert!(matches_query(&item, "ReViEw"));
    }

    #[test]
    fn match_is_case_insensitive_on_description() {
        // spec: CLI-85
        let item = make_test_item("x", Some("Implements a Spec with Tests"));
        assert!(matches_query(&item, "SPEC"));
        assert!(matches_query(&item, "spec"));
    }

    #[test]
    fn no_match_when_query_absent_from_both_name_and_description() {
        // spec: CLI-85
        let item = make_test_item("review", Some("Review the diff for bugs"));
        assert!(!matches_query(&item, "python"));
    }

    #[test]
    fn no_match_when_description_is_none_and_name_does_not_match() {
        // spec: CLI-85
        let item = make_test_item("review", None);
        assert!(!matches_query(&item, "bugs"));
    }

    #[test]
    fn empty_description_does_not_match_a_nonempty_query() {
        // spec: CLI-85
        // Some("") is distinct from None: an empty description string must not
        // spuriously match a non-empty query (it would if `contains` were
        // reasoned about backwards). The empty *query* still matches (all),
        // but a non-empty query against an empty description must not.
        let item = make_test_item("x", Some(""));
        assert!(matches_query(&item, ""));
        assert!(!matches_query(&item, "anything"));
    }

    #[test]
    fn whitespace_query_matches_a_description_that_contains_whitespace() {
        // spec: CLI-85
        // A non-empty query is a raw substring; it is not trimmed. A query of a
        // single space matches a description containing a space but a name that
        // has none.
        let item = make_test_item("review", Some("Review the diff"));
        assert!(!item.effective_name().contains(' '));
        assert!(matches_query(&item, " "));
    }

    #[test]
    fn substring_in_middle_of_word_matches() {
        // spec: CLI-85
        // Matching is substring, not word-boundary: a fragment inside a longer
        // word matches both in the name and in the description.
        let by_name = make_test_item("refactor", None);
        assert!(matches_query(&by_name, "factor"));
        let by_desc = make_test_item("x", Some("Performs refactoring"));
        assert!(matches_query(&by_desc, "factor"));
    }

    #[test]
    fn prefix_is_used_in_effective_name_match() {
        // spec: CLI-85
        let mut item = make_test_item("review", None);
        item.prefix = Some("jk".to_string());
        // effective_name() is "jk-review"
        assert!(matches_query(&item, "jk-review"));
        assert!(matches_query(&item, "jk"));
        // "review" is a substring of "jk-review", so it also matches
        assert!(matches_query(&item, "review"));
    }
}
