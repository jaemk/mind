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
    /// The minimum `mind` version this repo expects. Enforced at scan/meld time:
    /// a source requiring a newer `mind` than the one running is rejected (see
    /// [`version_at_least`] and `catalog::scan`).
    #[serde(rename = "min-mind-version")]
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

/// Whether `running` satisfies `>= required`, comparing dotted numeric version
/// components (a missing component counts as 0, so `0.2` == `0.2.0`). A
/// non-numeric component compares as 0, so a prerelease/build suffix is ignored.
pub fn version_at_least(running: &str, required: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|c| c.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let r = parse(running);
    let req = parse(required);
    for i in 0..r.len().max(req.len()) {
        let a = r.get(i).copied().unwrap_or(0);
        let b = req.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
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
        !self.items.is_empty() || self.discover.as_ref().is_some_and(|d| d.has_item_globs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_orders_dotted_components() {
        // spec: DSC-40
        assert!(version_at_least("0.2.0", "0.2"));
        assert!(version_at_least("0.2", "0.2.0"));
        assert!(version_at_least("1.0.0", "0.9.9"));
        assert!(version_at_least("0.10.0", "0.9.0"));
        assert!(!version_at_least("0.1.0", "0.2"));
        assert!(!version_at_least("0.1.0", "0.1.1"));
        // Non-numeric / suffix components count as 0.
        assert!(version_at_least("0.2.0-rc1", "0.2"));
    }
}
