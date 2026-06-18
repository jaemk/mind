//! The optional `mind.toml` a source repo may place at its root to declare its
//! inventory explicitly, instead of relying on convention scanning.
//!
//! Everything is optional. A repo with no `mind.toml` is scanned by convention.
//! A repo with only `[source]` metadata still gets convention scanning. A repo
//! that declares `[[items]]` or `[discover]` opts out of convention scanning and
//! becomes authoritative for its own inventory.

use std::path::Path;

use serde::Deserialize;

use crate::error::{MindError, Result};

/// The parsed `mind.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MindToml {
    #[serde(default)]
    pub source: SourceMeta,
    /// Explicit inventory; authoritative when non-empty.
    #[serde(default)]
    pub items: Vec<ItemDecl>,
    /// Glob-based discovery; authoritative when present.
    pub discover: Option<Discover>,
}

/// Repo-level metadata.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMeta {
    pub description: Option<String>,
    /// Namespace prefix applied to every item from this source (see
    /// [`crate::namespace`]). A consumer `meld --as` overrides it.
    pub prefix: Option<String>,
    /// Reserved: the minimum `mind` version this repo expects. Parsed so a repo
    /// can declare it now; version gating is not yet enforced.
    #[serde(rename = "min-mind-version")]
    #[allow(dead_code)]
    pub min_mind_version: Option<String>,
}

/// One explicitly declared item.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDecl {
    /// `skill`, `agent`, or `rule`.
    pub kind: String,
    pub name: String,
    /// Path to the item, relative to the repo root (a dir for skills).
    pub path: String,
    /// Optional override for where to link it under `~/.claude`
    /// (relative to the claude home, e.g. `rules/style.md`).
    pub link: Option<String>,
    /// Optional description override (else taken from frontmatter).
    pub description: Option<String>,
}

/// Glob-based discovery: per-kind include/exclude, plus nested sources.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discover {
    #[serde(default)]
    pub skills: KindGlobs,
    #[serde(default)]
    pub agents: KindGlobs,
    #[serde(default)]
    pub rules: KindGlobs,
    /// Other sources this repo curates. Melding this repo recursively melds each
    /// (see commands::meld), so a `mind.toml` can act as a registry / super-source.
    #[serde(default)]
    pub sources: Vec<NestedSource>,
}

/// Include/exclude glob patterns for one kind, relative to the repo root. An
/// item matched by `include` is kept unless it is also matched by `exclude`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindGlobs {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// A source referenced by a curated super-source.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NestedSource {
    /// A repo spec, parsed exactly like a `meld` argument.
    pub source: String,
    /// Optional namespace to impose on the nested source (like `meld --as`).
    #[serde(rename = "as", default)]
    pub alias: Option<String>,
}

impl Discover {
    /// Whether this section declares item globs (as opposed to only nested
    /// sources). Item globs turn off convention discovery; a bare `sources` list
    /// does not.
    pub fn has_item_globs(&self) -> bool {
        !self.skills.include.is_empty()
            || !self.agents.include.is_empty()
            || !self.rules.include.is_empty()
    }
}

impl MindToml {
    /// Load `mind.toml` from a repo root, returning `None` if absent.
    pub fn load(root: &Path) -> Result<Option<MindToml>> {
        let file = root.join("mind.toml");
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let parsed = toml::from_str(&text).map_err(|e| MindError::Toml {
                    path: file.clone(),
                    source: e,
                })?;
                Ok(Some(parsed))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    /// Whether this file takes over item discovery (vs. leaving it to
    /// convention). Nested `[discover].sources` alone does not.
    pub fn is_authoritative(&self) -> bool {
        !self.items.is_empty()
            || self.discover.as_ref().is_some_and(|d| d.has_item_globs())
    }
}
