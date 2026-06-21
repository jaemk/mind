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
use crate::source::Pin;

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
    /// An install hook (HOOK-1): a shell command the maintainer declares to build
    /// or install the tooling this source's items rely on. Run on `meld` after
    /// checkout, gated by a safety prompt (see spec/install-hooks.md). `None` when
    /// the source declares no hook.
    pub install: Option<String>,
    /// Namespace prefix applied to every item from this source (see
    /// [`crate::namespace`]). A consumer `meld --as` overrides it.
    pub prefix: Option<String>,
    /// The minimum `mind` version this repo expects. Enforced at scan/meld time:
    /// a source requiring a newer `mind` than the one running is rejected (see
    /// [`version_at_least`] and `catalog::scan`).
    #[serde(rename = "min-mind-version")]
    pub min_mind_version: Option<String>,
    /// Pin directive: track a named branch (DSC-41).
    #[serde(rename = "follow-branch")]
    pub follow_branch: Option<String>,
    /// Pin directive: fix to a tag (DSC-41).
    #[serde(rename = "pin-tag")]
    pub pin_tag: Option<String>,
    /// Pin directive: fix to a specific commit (DSC-41).
    #[serde(rename = "pin-ref")]
    pub pin_ref: Option<String>,
    /// Convention scan roots (DSC-50). When set, convention discovery scans
    /// under each listed repo-root-relative directory instead of the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
}

impl SourceMeta {
    /// Return the single pin declared by this `[source]` section, or `None`
    /// if none is declared. More than one declared pin is a `MindToml` error
    /// (DSC-41).
    pub fn pin_directive(&self, toml_path: &Path) -> Result<Option<Pin>> {
        // Collect which directives are set so we can detect conflicts and give
        // a useful error message naming both conflicting keys.
        let mut set: Vec<(&str, Pin)> = Vec::new();
        if let Some(b) = &self.follow_branch {
            set.push(("follow-branch", Pin::FollowBranch(b.clone())));
        }
        if let Some(t) = &self.pin_tag {
            set.push(("pin-tag", Pin::Tag(t.clone())));
        }
        if let Some(r) = &self.pin_ref {
            set.push(("pin-ref", Pin::Ref(r.clone())));
        }
        match set.len() {
            0 => Ok(None),
            1 => Ok(Some(set.remove(0).1)),
            _ => {
                let names: Vec<&str> = set.iter().map(|(k, _)| *k).collect();
                Err(MindError::MindToml {
                    path: toml_path.to_path_buf(),
                    msg: format!(
                        "conflicting pin directives: {}; declare at most one of follow-branch, pin-tag, pin-ref",
                        names.join(", ")
                    ),
                })
            }
        }
    }
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
    use std::path::Path;

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

    #[test]
    fn source_install_hook_parses() {
        // spec: HOOK-1
        let toml = r#"
            [source]
            description = "tools"
            install = "make build && make install"
        "#;
        let parsed: MindToml = toml::from_str(toml).expect("parse");
        assert_eq!(
            parsed.source.install.as_deref(),
            Some("make build && make install")
        );

        // Absent install => None.
        let none: MindToml = toml::from_str("[source]\ndescription = \"x\"\n").unwrap();
        assert_eq!(none.source.install, None);
    }

    #[test]
    fn pin_directive_none_when_no_fields_set() {
        // spec: DSC-41
        let meta = SourceMeta::default();
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("should not error");
        assert!(pin.is_none(), "no directive set => None");
    }

    #[test]
    fn pin_directive_follow_branch() {
        // spec: DSC-41
        let meta = SourceMeta {
            follow_branch: Some("develop".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::FollowBranch("develop".into())));
    }

    #[test]
    fn pin_directive_tag() {
        // spec: DSC-41
        let meta = SourceMeta {
            pin_tag: Some("v2.0".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::Tag("v2.0".into())));
    }

    #[test]
    fn pin_directive_ref() {
        // spec: DSC-41
        let meta = SourceMeta {
            pin_ref: Some("abc123".into()),
            ..Default::default()
        };
        let pin = meta
            .pin_directive(Path::new("mind.toml"))
            .expect("no error");
        assert_eq!(pin, Some(Pin::Ref("abc123".into())));
    }

    #[test]
    fn pin_directive_conflict_is_an_error() {
        // spec: DSC-41 — more than one directive is a MindToml error
        let meta = SourceMeta {
            follow_branch: Some("main".into()),
            pin_tag: Some("v1.0".into()),
            ..Default::default()
        };
        let result = meta.pin_directive(Path::new("/repo/mind.toml"));
        assert!(result.is_err(), "conflicting directives must error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("conflicting pin"),
            "expected 'conflicting pin' in: {err_msg}"
        );
    }
}
