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
/// `evolve` matches on across a prefix change.
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

fn scan_source(paths: &Paths, source: &Source, out: &mut Vec<CatalogItem>) -> Result<()> {
    let root = source.clone_dir(paths);
    let mindfile = MindToml::load(&root)?;

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
            for decl in &mt.items {
                out.push(from_decl(&root, source, &prefix, decl)?);
            }
            if let Some(discover) = &mt.discover {
                scan_globs(&root, source, &prefix, discover, out)?;
            }
            Ok(())
        }
        _ => scan_convention(&root, source, &prefix, out),
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
    use std::path::PathBuf;

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
