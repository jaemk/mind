//! Parsing and resolving item refs against a catalog.

use crate::catalog::CatalogItem;
use crate::error::{ItemKind, MindError, Result};
use crate::manifest::InstalledItem;

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
            "tool" => ItemKind::Tool,
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

/// The full source identity plus each of its trailing-suffix forms at a component
/// boundary (e.g. `host/owner/repo`, `owner/repo`, `repo`). The candidates a
/// selector is matched against in both [`source_matches`] and
/// [`source_matches_glob`].
fn source_suffix_forms(full_name: &str) -> Vec<&str> {
    let mut forms = vec![full_name];
    let mut rest = full_name;
    while let Some(idx) = rest.find('/') {
        rest = &rest[idx + 1..];
        forms.push(rest);
    }
    forms
}

/// Whether a source selector matches a full `host/owner/repo` source name,
/// permitting a glob. When `selector` carries glob metacharacters (CLI-28,
/// CLI-86), it is compiled as a [`glob::Pattern`] and matched as a plain string
/// against the full identity or any trailing-suffix form, so `*` spans any run
/// including `/` (`*agents` matches `github.com/jaemk/agents`). Otherwise it
/// falls back to the exact/unambiguous-suffix semantics of [`source_matches`].
pub fn source_matches_glob(full_name: &str, selector: &str) -> bool {
    if is_glob(selector) {
        match glob::Pattern::new(selector) {
            Ok(pattern) => source_suffix_forms(full_name)
                .iter()
                .any(|form| pattern.matches(form)),
            Err(_) => false,
        }
    } else {
        source_matches(full_name, selector)
    }
}

/// Validate a source selector that may carry glob metacharacters (CLI-28). When
/// `selector` is a glob, compile it once and surface a malformed pattern as
/// [`MindError::InvalidPattern`] so a typo like `[bad` reports a clear
/// invalid-pattern error rather than silently matching nothing (which would
/// surface downstream as `SourceNotFound`). A non-glob selector is always valid.
pub fn validate_source_selector(selector: &str) -> Result<()> {
    if is_glob(selector) {
        glob::Pattern::new(selector).map_err(|source| MindError::InvalidPattern {
            pattern: selector.to_string(),
            source,
        })?;
    }
    Ok(())
}

/// Whether a ref name is a glob pattern (selects many) rather than an exact name.
pub fn is_glob(name: &str) -> bool {
    name.contains(['*', '?', '['])
}

/// Apply the `learn --all` flag (CLI-36): append the `#*` selector so the
/// positional ref is read as a source qualifier selecting every item of that
/// source. Errors `InvalidItemRef` when the ref already carries a `#` selector,
/// since the selector would be doubled.
pub fn all_selector(item: &str) -> Result<String> {
    let item = item.trim();
    if item.contains('#') {
        return Err(MindError::InvalidItemRef {
            name: item.to_string(),
        });
    }
    Ok(format!("{item}#*"))
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

/// Whether an installed item matches a parsed ref: its kind (when the ref names
/// one), its effective installed name, and the source qualifier (when given).
/// Used by `forget`, `recall <item>`, and `upgrade [item]`, which match against
/// the manifest rather than the catalog.
pub fn installed_matches(it: &InstalledItem, r: &ItemRef) -> bool {
    r.kind.is_none_or(|k| it.kind == k)
        && it.name == r.name
        && r.source
            .as_ref()
            .is_none_or(|s| source_matches(&it.source, s))
}

/// Select every installed item matching `r`: the name as a glob when it contains
/// glob metacharacters, else by exact effective name, honoring the kind and
/// source qualifier as in [`installed_matches`]. Used for multi-item `forget`.
pub fn select_installed<'a>(
    items: &'a std::collections::BTreeMap<String, InstalledItem>,
    r: &ItemRef,
) -> Vec<&'a InstalledItem> {
    let pattern = glob::Pattern::new(&r.name).ok();
    items
        .values()
        .filter(|it| {
            r.kind.is_none_or(|k| it.kind == k)
                && r.source
                    .as_ref()
                    .is_none_or(|s| source_matches(&it.source, s))
                && match &pattern {
                    Some(p) => p.matches(&it.name),
                    None => it.name == r.name,
                }
        })
        .collect()
}

/// Find the installed items matching `r`. Errors `NotInstalled` on no match and
/// `AmbiguousItem` on more than one (e.g. a bare name shared across kinds).
pub fn resolve_installed<'a>(
    items: &'a std::collections::BTreeMap<String, InstalledItem>,
    r: &ItemRef,
) -> Result<&'a InstalledItem> {
    let matches: Vec<&InstalledItem> = items
        .values()
        .filter(|it| installed_matches(it, r))
        .collect();
    match matches.as_slice() {
        [] => Err(MindError::NotInstalled {
            name: r.name.clone(),
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
            bin: None,
            build: None,
            install: None,
            uninstall: None,
            requires: Vec::new(),
            hooks: Vec::new(),
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

    // spec: CLI-28, CLI-86
    #[test]
    fn source_glob_matches_full_id_and_suffix_forms() {
        let full = "github.com/jaemk/agents";
        // `*` spans `/`, matching against the full identity as a plain string.
        assert!(source_matches_glob(full, "*agents"));
        assert!(source_matches_glob(full, "github.com/*/agents"));
        assert!(source_matches_glob(full, "*"));
        // Matched against a trailing-suffix form (`agents`).
        assert!(source_matches_glob(full, "ag*"));
        assert!(source_matches_glob(full, "jaemk/*"));
        // `?` and `[..]` metacharacters are honored.
        assert!(source_matches_glob(full, "agent?"));
        assert!(source_matches_glob(full, "[ab]gents"));
    }

    // spec: CLI-28, CLI-86
    #[test]
    fn source_glob_matching_nothing_is_false() {
        let full = "github.com/jaemk/agents";
        assert!(!source_matches_glob(full, "*foo"));
        assert!(!source_matches_glob(full, "skills*"));
    }

    // spec: CLI-28, CLI-86
    #[test]
    fn source_glob_non_glob_falls_back_to_exact_or_suffix() {
        let full = "github.com/jaemk/agents";
        // No glob metacharacters: exact/suffix semantics of `source_matches`.
        assert!(source_matches_glob(full, "agents"));
        assert!(source_matches_glob(full, "jaemk/agents"));
        assert!(source_matches_glob(full, "github.com/jaemk/agents"));
        // A non-component-boundary substring still does not match without a glob.
        assert!(!source_matches_glob(full, "jaemk"));
        assert!(!source_matches_glob(full, "ts"));
    }

    // spec: CLI-28, CLI-86
    #[test]
    fn validate_source_selector_accepts_valid_glob() {
        // A well-formed glob compiles, so validation passes.
        assert!(validate_source_selector("*agents").is_ok());
        assert!(validate_source_selector("github.com/*/agents").is_ok());
        assert!(validate_source_selector("[ab]gents").is_ok());
        assert!(validate_source_selector("agent?").is_ok());
    }

    // spec: CLI-28, CLI-86
    #[test]
    fn validate_source_selector_rejects_malformed_glob() {
        // `[bad` opens a character class that is never closed -- glob compilation
        // fails, and validation must surface InvalidPattern (not silently pass and
        // later read as SourceNotFound).
        let err = validate_source_selector("[bad").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPattern { ref pattern, .. } if pattern == "[bad"),
            "expected InvalidPattern carrying the offending pattern, got {err:?}"
        );
        // The user-facing message names the failure mode.
        assert!(
            err.to_string().contains("not a valid glob selector"),
            "message should explain the invalid glob: {err}"
        );
    }

    // spec: CLI-86
    #[test]
    fn validate_source_selector_passes_non_glob() {
        // No glob metacharacters: nothing to compile, always Ok even for a name
        // that would carry an unbalanced bracket meaning only as a literal.
        assert!(validate_source_selector("agents").is_ok());
        assert!(validate_source_selector("github.com/jaemk/agents").is_ok());
        assert!(validate_source_selector("").is_ok());
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

    fn inst(kind: ItemKind, name: &str, source: &str) -> InstalledItem {
        InstalledItem {
            kind,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: source.to_string(),
            commit: String::new(),
            hash: String::new(),
            store: String::new(),
            links: Vec::new(),
            description: None,
        }
    }

    fn manifest(items: Vec<InstalledItem>) -> std::collections::BTreeMap<String, InstalledItem> {
        items.into_iter().map(|it| (it.key(), it)).collect()
    }

    #[test]
    fn installed_lookup_honors_kind_and_source_qualifier() {
        // spec: CLI-40, CLI-63, CLI-71
        let m = manifest(vec![
            inst(ItemKind::Skill, "review", "github.com/james/agents"),
            inst(ItemKind::Agent, "review", "github.com/james/agents"),
        ]);
        // A bare name shared across kinds is ambiguous.
        let bare = parse_item_ref("review").unwrap();
        assert!(matches!(
            resolve_installed(&m, &bare),
            Err(MindError::AmbiguousItem { .. })
        ));
        // A kind prefix disambiguates.
        let skill = parse_item_ref("skill:review").unwrap();
        assert_eq!(resolve_installed(&m, &skill).unwrap().kind, ItemKind::Skill);
        // A source qualifier that does not match yields NotInstalled.
        let wrong = parse_item_ref("other/repo#skill:review").unwrap();
        assert!(matches!(
            resolve_installed(&m, &wrong),
            Err(MindError::NotInstalled { .. })
        ));
        // A matching source qualifier resolves.
        let right = parse_item_ref("james/agents#skill:review").unwrap();
        assert_eq!(resolve_installed(&m, &right).unwrap().kind, ItemKind::Skill);
    }

    #[test]
    fn select_installed_matches_glob_kind_and_source() {
        // spec: CLI-41
        // The manifest is keyed by `kind:effective_name`, so names are distinct.
        let m = manifest(vec![
            inst(ItemKind::Skill, "review", "github.com/james/agents"),
            inst(ItemKind::Skill, "release", "github.com/james/agents"),
            inst(ItemKind::Agent, "dev", "github.com/james/agents"),
            inst(ItemKind::Skill, "audit", "github.com/bob/agents"),
        ]);
        // Glob over all skills (across both sources).
        assert_eq!(
            select_installed(&m, &parse_item_ref("skill:*").unwrap()).len(),
            3
        );
        // Prefix glob.
        assert_eq!(
            select_installed(&m, &parse_item_ref("rele*").unwrap()).len(),
            1
        );
        // Everything.
        assert_eq!(select_installed(&m, &parse_item_ref("*").unwrap()).len(), 4);
        // Source-scoped glob.
        assert_eq!(
            select_installed(&m, &parse_item_ref("bob/agents#*").unwrap()).len(),
            1
        );
        // Exact name (no glob) still matches by equality.
        assert_eq!(
            select_installed(&m, &parse_item_ref("review").unwrap()).len(),
            1
        );
    }

    #[test]
    fn all_selector_appends_glob_and_rejects_hash() {
        // spec: CLI-36
        assert_eq!(
            all_selector("local/dev/agents").unwrap(),
            "local/dev/agents#*"
        );
        assert_eq!(all_selector("agents").unwrap(), "agents#*");
        // Whitespace is trimmed before the suffix is appended.
        assert_eq!(all_selector("  agents  ").unwrap(), "agents#*");
        // A ref that already names an item (carries `#`) is rejected.
        assert!(matches!(
            all_selector("agents#review"),
            Err(MindError::InvalidItemRef { .. })
        ));
        assert!(matches!(
            all_selector("agents#*"),
            Err(MindError::InvalidItemRef { .. })
        ));
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
