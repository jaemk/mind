//! Parsing and resolving item refs against a catalog.

use crate::catalog::CatalogItem;
use crate::error::{ItemKind, MindError, Result};

/// A parsed item ref, before it is matched against a catalog.
#[derive(Debug, Clone)]
pub struct ItemRef {
    /// `None` means "any kind".
    pub kind: Option<ItemKind>,
    pub name: String,
    /// `Some(source_name)` for the `owner/repo#name` form (uses repo as source name).
    pub source: Option<String>,
}

/// Parse one of: `name`, `skill:name`, `agent:name`, `rule:name`, `owner/repo#name`.
pub fn parse_item_ref(raw: &str) -> Result<ItemRef> {
    let raw = raw.trim();
    let invalid = || MindError::InvalidItemRef {
        name: raw.to_string(),
    };

    // Source-qualified: owner/repo#name (or repo#name). The selector is kept
    // verbatim and matched in `resolve` against the full name or the basename.
    if let Some((repo_part, name_part)) = raw.split_once('#') {
        let selector = repo_part.trim();
        if selector.is_empty() {
            return Err(invalid());
        }
        let (kind, name) = split_kind(name_part)?;
        return Ok(ItemRef {
            kind,
            name,
            source: Some(selector.to_string()),
        });
    }

    let (kind, name) = split_kind(raw)?;
    Ok(ItemRef {
        kind,
        name,
        source: None,
    })
}

fn split_kind(raw: &str) -> Result<(Option<ItemKind>, String)> {
    let invalid = || MindError::InvalidItemRef {
        name: raw.to_string(),
    };
    if let Some((prefix, name)) = raw.split_once(':') {
        let kind = match prefix {
            "skill" => ItemKind::Skill,
            "agent" => ItemKind::Agent,
            "rule" => ItemKind::Rule,
            _ => return Err(invalid()),
        };
        if name.is_empty() {
            return Err(invalid());
        }
        Ok((Some(kind), name.to_string()))
    } else {
        if raw.is_empty() {
            return Err(invalid());
        }
        Ok((None, raw.to_string()))
    }
}

/// Whether a source selector matches a full `host/owner/repo` source name. It
/// matches the full name or any trailing path suffix at a component boundary, so
/// `repo`, `owner/repo`, and `host/owner/repo` all select it. A selector resolves
/// uniquely only when one source carries that suffix.
pub fn source_matches(full_name: &str, selector: &str) -> bool {
    full_name == selector || full_name.ends_with(&format!("/{selector}"))
}

/// Whether a ref name is a glob pattern (selects many) rather than an exact name.
pub fn is_glob(name: &str) -> bool {
    name.contains(['*', '?', '['])
}

/// Select every catalog item matching `r`: the name as a glob when it contains
/// glob metacharacters, else by exact effective name, with the kind and source
/// qualifier filtering as in [`resolve`]. Used for multi-item `learn`.
pub fn select<'a>(items: &'a [CatalogItem], r: &ItemRef) -> Vec<&'a CatalogItem> {
    let pattern = glob::Pattern::new(&r.name).ok();
    items
        .iter()
        .filter(|it| {
            r.kind.is_none_or(|k| it.kind == k)
                && r.source
                    .as_ref()
                    .is_none_or(|s| source_matches(&it.source, s))
                && match &pattern {
                    Some(p) => p.matches(&it.effective_name()),
                    None => it.effective_name() == r.name,
                }
        })
        .collect()
}

/// Find the single catalog item matching `r`, erroring on none or ambiguity.
pub fn resolve<'a>(
    items: &'a [CatalogItem],
    r: &ItemRef,
    sources: usize,
) -> Result<&'a CatalogItem> {
    let matches: Vec<&CatalogItem> = items
        .iter()
        .filter(|it| {
            r.kind.is_none_or(|k| it.kind == k)
                && it.effective_name() == r.name
                && r.source
                    .as_ref()
                    .is_none_or(|s| source_matches(&it.source, s))
        })
        .collect();

    match matches.as_slice() {
        [] => Err(MindError::ItemNotFound {
            query: r.name.clone(),
            sources,
        }),
        [only] => Ok(only),
        many => Err(MindError::AmbiguousItem {
            query: r.name.clone(),
            candidates: many
                .iter()
                .map(|it| format!("{}#{}", it.source, it.key()))
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    // spec: CLI-1, CLI-2, CLI-3, CLI-4, CLI-5, CLI-31 (item ref parsing, resolution, selection)
    use super::*;
    use std::path::PathBuf;

    fn cat(kind: ItemKind, name: &str, source: &str) -> CatalogItem {
        CatalogItem {
            kind,
            name: name.to_string(),
            source: source.to_string(),
            prefix: None,
            path: PathBuf::new(),
            description: None,
            link_rel: None,
        }
    }

    #[test]
    fn parses_bare_name_as_any_kind() {
        let r = parse_item_ref("review").unwrap();
        assert_eq!(r.kind, None);
        assert_eq!(r.name, "review");
        assert_eq!(r.source, None);
    }

    #[test]
    fn parses_kind_prefix() {
        let r = parse_item_ref("skill:review").unwrap();
        assert_eq!(r.kind, Some(ItemKind::Skill));
        assert_eq!(r.name, "review");
    }

    #[test]
    fn parses_source_qualified() {
        let r = parse_item_ref("james/agents#agent:dev").unwrap();
        assert_eq!(r.source.as_deref(), Some("james/agents"));
        assert_eq!(r.kind, Some(ItemKind::Agent));
        assert_eq!(r.name, "dev");
    }

    #[test]
    fn source_selector_matches_full_name_or_trailing_suffix() {
        let full = "github.com/james/agents";
        assert!(source_matches(full, "github.com/james/agents"));
        assert!(source_matches(full, "james/agents"));
        assert!(source_matches(full, "agents"));
        // Not a component-boundary suffix.
        assert!(!source_matches(full, "james"));
        assert!(!source_matches(full, "ts"));
        assert!(!source_matches(full, "bob/agents"));
    }

    #[test]
    fn rejects_bad_refs() {
        for bad in ["", "skill:", "bogus:name"] {
            assert!(parse_item_ref(bad).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn resolves_unique_match() {
        let items = vec![cat(ItemKind::Skill, "review", "agents")];
        let r = parse_item_ref("review").unwrap();
        assert_eq!(resolve(&items, &r, 1).unwrap().name, "review");
    }

    #[test]
    fn errors_on_no_match() {
        let items = vec![cat(ItemKind::Skill, "review", "agents")];
        let r = parse_item_ref("nope").unwrap();
        assert!(matches!(
            resolve(&items, &r, 1),
            Err(MindError::ItemNotFound { .. })
        ));
    }

    #[test]
    fn errors_on_ambiguous_match() {
        let items = vec![
            cat(ItemKind::Skill, "review", "agents"),
            cat(ItemKind::Skill, "review", "other"),
        ];
        let r = parse_item_ref("review").unwrap();
        assert!(matches!(
            resolve(&items, &r, 2),
            Err(MindError::AmbiguousItem { .. })
        ));
    }

    #[test]
    fn kind_prefix_disambiguates() {
        let items = vec![
            cat(ItemKind::Skill, "x", "a"),
            cat(ItemKind::Agent, "x", "a"),
        ];
        let r = parse_item_ref("agent:x").unwrap();
        assert_eq!(resolve(&items, &r, 1).unwrap().kind, ItemKind::Agent);
    }

    #[test]
    fn detects_glob_patterns() {
        assert!(is_glob("*"));
        assert!(is_glob("review*"));
        assert!(is_glob("skill:*"));
        assert!(!is_glob("review"));
    }

    #[test]
    fn select_matches_glob_kind_and_source() {
        let items = vec![
            cat(ItemKind::Skill, "review", "a"),
            cat(ItemKind::Skill, "release", "a"),
            cat(ItemKind::Agent, "dev", "a"),
            cat(ItemKind::Skill, "review", "b"),
        ];
        // Glob over all skills.
        assert_eq!(select(&items, &parse_item_ref("skill:*").unwrap()).len(), 3);
        // Prefix glob.
        assert_eq!(select(&items, &parse_item_ref("rele*").unwrap()).len(), 1);
        // Everything.
        assert_eq!(select(&items, &parse_item_ref("*").unwrap()).len(), 4);
        // Source-scoped glob.
        assert_eq!(select(&items, &parse_item_ref("a#*").unwrap()).len(), 3);
        // Source + kind + glob compose: skills of source a.
        assert_eq!(
            select(&items, &parse_item_ref("a#skill:*").unwrap()).len(),
            2
        );
        // Exact name (no glob) still matches by equality.
        assert_eq!(select(&items, &parse_item_ref("dev").unwrap()).len(), 1);
    }
}
