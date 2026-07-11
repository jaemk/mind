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

/// A parsed hook target: either a source selector or an item ref.
///
/// The rule is simple: if the raw string contains `#`, it is an item ref
/// (`<source>#<item>`); otherwise it is a source selector. Source selectors may
/// carry glob metacharacters (`*`, `?`, `[..]`) to match multiple sources.
// spec: CLI-194
#[derive(Debug)]
pub enum HookTarget {
    /// A source selector (no `#` in the raw target string). May be a glob.
    Source(String),
    /// An item ref `<source>#<item>` (contains `#`). Parsed via
    /// [`parse_item_ref`]; the source part acts as a filter and may itself be a
    /// glob.
    Item(ItemRef),
}

/// Parse a hook target string, distinguishing a source selector (no `#`) from
/// an `<source>#<item>` item ref (contains `#`). Used by `mind hooks run` and
/// `mind hooks list` to decide what the target addresses.
// spec: CLI-194
pub fn parse_hook_target(target: &str) -> Result<HookTarget> {
    let target = target.trim();
    if target.contains('#') {
        Ok(HookTarget::Item(parse_item_ref(target)?))
    } else {
        Ok(HookTarget::Source(target.to_string()))
    }
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
    // A pre-colon token is read as a KIND only when it is a reserved kind word
    // (NS-26). Otherwise the WHOLE ref (colon and all) is the effective name with
    // kind=None, so a prefixed effective name like `jk:review` parses as a name and
    // resolves by effective-name match, while `skill:review` stays kind-qualified.
    if let Some((prefix, name)) = raw.split_once(':')
        && let Some(kind) = ItemKind::parse(prefix)
    {
        // Kind-qualified: the name after the kind word must be non-empty
        // (so `skill:` stays invalid).
        if name.is_empty() {
            return Err(invalid());
        }
        return Ok((Some(kind), name.to_string()));
    }
    if raw.is_empty() {
        return Err(invalid());
    }
    Ok((None, raw.to_string()))
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

/// Filter a catalog slice to the items named by a set of bare `kind:name` refs
/// (DSC-62). Each ref must carry an explicit kind prefix; a ref that parses
/// without a kind (bare name only) is not matched here. Items are matched by
/// their bare `name` (not effective/prefixed name) and kind.
///
/// Returns the matching subset in catalog order. Does NOT error on unknown refs:
/// the caller (`install_source_items_subset`) is responsible for DSC-63
/// validation before calling this.
pub fn select_by_bare_refs<'a>(
    items: &'a [CatalogItem],
    bare_refs: &[String],
) -> Vec<&'a CatalogItem> {
    // Parse each ref into (kind, bare_name); skip unparseable entries.
    let pairs: Vec<(crate::error::ItemKind, String)> = bare_refs
        .iter()
        .filter_map(|r| {
            let (kind, name) = split_kind(r).ok()?;
            kind.map(|k| (k, name))
        })
        .collect();

    items
        .iter()
        .filter(|it| pairs.iter().any(|(k, n)| it.kind == *k && it.name == *n))
        .collect()
}

/// Whether an installed item matches a parsed ref: its kind (when the ref names
/// one), its effective installed name, and the source qualifier (when given).
/// Used by `recall <item>` and `resolve_installed`, which require exact
/// effective-name matching against the manifest.
pub fn installed_matches(it: &InstalledItem, r: &ItemRef) -> bool {
    r.kind.is_none_or(|k| it.kind == k)
        && it.name == r.name
        && r.source
            .as_ref()
            .is_none_or(|s| source_matches(&it.source, s))
}

/// Like [`installed_matches`] but the name matches as a glob when it contains
/// glob metacharacters (`*`, `?`, `[`), else exactly. Used by `forget` and
/// `upgrade` for multi-item selection.
pub fn installed_matches_glob(it: &InstalledItem, r: &ItemRef) -> bool {
    r.kind.is_none_or(|k| it.kind == k)
        && r.source
            .as_ref()
            .is_none_or(|s| source_matches(&it.source, s))
        && if is_glob(&r.name) {
            glob::Pattern::new(&r.name).is_ok_and(|p| p.matches(&it.name))
        } else {
            it.name == r.name
        }
}

/// Select every installed item matching `r`: the name as a glob when it contains
/// glob metacharacters, else by exact effective name, honoring the kind and
/// source qualifier as in [`installed_matches`]. Used for multi-item `forget`
/// and `upgrade`.
pub fn select_installed<'a>(
    items: &'a std::collections::BTreeMap<String, InstalledItem>,
    r: &ItemRef,
) -> Vec<&'a InstalledItem> {
    items
        .values()
        .filter(|it| installed_matches_glob(it, r))
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

    // ---- parse_hook_target ----

    // spec: CLI-194
    #[test]
    fn parse_hook_target_no_hash_is_source() {
        // A bare name, an owner/repo pair, and a full host/owner/repo are all
        // source selectors when they contain no `#`.
        let t = parse_hook_target("agents").unwrap();
        assert!(matches!(t, HookTarget::Source(s) if s == "agents"));

        let t = parse_hook_target("owner/repo").unwrap();
        assert!(matches!(t, HookTarget::Source(s) if s == "owner/repo"));

        let t = parse_hook_target("github.com/owner/repo").unwrap();
        assert!(matches!(t, HookTarget::Source(s) if s == "github.com/owner/repo"));
    }

    // spec: CLI-194
    #[test]
    fn parse_hook_target_glob_source() {
        // Glob metacharacters with no `#` remain a source selector.
        let t = parse_hook_target("*").unwrap();
        assert!(matches!(t, HookTarget::Source(s) if s == "*"));

        let t = parse_hook_target("owner/*").unwrap();
        assert!(matches!(t, HookTarget::Source(s) if s == "owner/*"));
    }

    // spec: CLI-194
    #[test]
    fn parse_hook_target_with_hash_is_item() {
        // Bare item ref: source="agents", name="scan", kind=None.
        let t = parse_hook_target("agents#scan").unwrap();
        match t {
            HookTarget::Item(r) => {
                assert_eq!(r.source.as_deref(), Some("agents"));
                assert_eq!(r.name, "scan");
                assert_eq!(r.kind, None);
            }
            HookTarget::Source(_) => panic!("expected Item"),
        }

        // Kind-qualified item ref.
        let t = parse_hook_target("owner/repo#skill:scan").unwrap();
        match t {
            HookTarget::Item(r) => {
                assert_eq!(r.source.as_deref(), Some("owner/repo"));
                assert_eq!(r.kind, Some(ItemKind::Skill));
                assert_eq!(r.name, "scan");
            }
            HookTarget::Source(_) => panic!("expected Item"),
        }

        // Glob item ref.
        let t = parse_hook_target("agents#*").unwrap();
        match t {
            HookTarget::Item(r) => {
                assert_eq!(r.source.as_deref(), Some("agents"));
                assert_eq!(r.name, "*");
            }
            HookTarget::Source(_) => panic!("expected Item"),
        }
    }

    // spec: CLI-194
    #[test]
    fn parse_hook_target_invalid_item_ref_errors() {
        // A hash with an empty repo part is invalid.
        assert!(parse_hook_target("#item").is_err());
        // A hash with a bad kind (empty name after kind) is invalid.
        assert!(parse_hook_target("repo#skill:").is_err());
    }

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

    // spec: NS-26
    #[test]
    fn colon_token_is_kind_only_when_reserved_word() {
        // A non-kind pre-colon token: the WHOLE ref is the effective name, kind=None.
        // This lets a prefixed effective name like `jk:review` be used as a ref.
        let r = parse_item_ref("jk:review").unwrap();
        assert_eq!(r.kind, None);
        assert_eq!(r.name, "jk:review");
        assert_eq!(r.source, None);

        // A reserved kind word stays kind-qualified.
        let s = parse_item_ref("skill:review").unwrap();
        assert_eq!(s.kind, Some(ItemKind::Skill));
        assert_eq!(s.name, "review");

        // Resolve by effective name: an item whose effective name is `jk:review`
        // (built with prefix=None so this is independent of the separator constant)
        // is found by the kindless effective-name ref.
        let items = vec![cat(ItemKind::Skill, "jk:review", "agents")];
        let found = resolve(&items, &parse_item_ref("jk:review").unwrap(), 1).unwrap();
        assert_eq!(found.effective_name(), "jk:review");

        // Source-qualified, kindless: the post-`#` token still parses as a name.
        let q = parse_item_ref("owner/repo#jk:review").unwrap();
        assert_eq!(q.source.as_deref(), Some("owner/repo"));
        assert_eq!(q.kind, None);
        assert_eq!(q.name, "jk:review");
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
        for bad in ["", "skill:"] {
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

    // spec: CLI-65
    #[test]
    fn installed_matches_glob_bare_star_matches_all() {
        let items = [
            inst(ItemKind::Skill, "review", "github.com/james/agents"),
            inst(ItemKind::Agent, "dev", "github.com/james/agents"),
            inst(ItemKind::Rule, "style", "github.com/bob/agents"),
        ];
        let r = parse_item_ref("*").unwrap();
        assert!(items.iter().all(|it| installed_matches_glob(it, &r)));
    }

    // spec: CLI-65
    #[test]
    fn installed_matches_glob_kind_prefix_narrows() {
        let skill = inst(ItemKind::Skill, "review", "github.com/james/agents");
        let agent = inst(ItemKind::Agent, "dev", "github.com/james/agents");
        let r = parse_item_ref("skill:*").unwrap();
        assert!(installed_matches_glob(&skill, &r));
        assert!(!installed_matches_glob(&agent, &r));
    }

    // spec: CLI-65
    #[test]
    fn installed_matches_glob_source_qualifier_narrows() {
        let james = inst(ItemKind::Skill, "review", "github.com/james/agents");
        let bob = inst(ItemKind::Skill, "audit", "github.com/bob/agents");
        let r = parse_item_ref("james/agents#*").unwrap();
        assert!(installed_matches_glob(&james, &r));
        assert!(!installed_matches_glob(&bob, &r));
    }

    // spec: CLI-65
    #[test]
    fn installed_matches_glob_exact_name_matches_only_that_item() {
        let review = inst(ItemKind::Skill, "review", "github.com/james/agents");
        let release = inst(ItemKind::Skill, "release", "github.com/james/agents");
        let r = parse_item_ref("review").unwrap();
        assert!(installed_matches_glob(&review, &r));
        assert!(!installed_matches_glob(&release, &r));
    }

    // spec: CLI-65
    #[test]
    fn installed_matches_glob_non_matching_glob_is_false() {
        let review = inst(ItemKind::Skill, "review", "github.com/james/agents");
        let r = parse_item_ref("xyz*").unwrap();
        assert!(!installed_matches_glob(&review, &r));
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

    // ----- DSC-62 / DSC-63: select_by_bare_refs subset filtering -----

    #[test]
    fn select_by_bare_refs_matches_kind_and_bare_name() {
        // spec: DSC-62 — a bare kind:name ref selects exactly the item of that
        // kind and bare name; an unlisted item is excluded.
        let items = vec![
            cat(ItemKind::Skill, "review", "a"),
            cat(ItemKind::Agent, "dev", "a"),
            cat(ItemKind::Rule, "style", "a"),
        ];
        let refs = vec!["skill:review".to_string(), "agent:dev".to_string()];
        let picked = select_by_bare_refs(&items, &refs);
        assert_eq!(picked.len(), 2);
        assert!(
            picked
                .iter()
                .any(|it| it.kind == ItemKind::Skill && it.name == "review")
        );
        assert!(
            picked
                .iter()
                .any(|it| it.kind == ItemKind::Agent && it.name == "dev")
        );
        assert!(
            !picked.iter().any(|it| it.name == "style"),
            "an unlisted item must not be selected"
        );
    }

    #[test]
    fn select_by_bare_refs_distinguishes_kind_for_same_bare_name() {
        // spec: DSC-63 — refs carry an explicit kind, so two items sharing a bare
        // name across kinds are not conflated: skill:x selects only the skill.
        let items = vec![
            cat(ItemKind::Skill, "x", "a"),
            cat(ItemKind::Agent, "x", "a"),
        ];
        let only_skill = select_by_bare_refs(&items, &["skill:x".to_string()]);
        assert_eq!(only_skill.len(), 1);
        assert_eq!(only_skill[0].kind, ItemKind::Skill);

        let only_agent = select_by_bare_refs(&items, &["agent:x".to_string()]);
        assert_eq!(only_agent.len(), 1);
        assert_eq!(only_agent[0].kind, ItemKind::Agent);

        // Both refs select both items.
        let both = select_by_bare_refs(&items, &["skill:x".to_string(), "agent:x".to_string()]);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn select_by_bare_refs_matches_by_bare_not_effective_name() {
        // spec: DSC-63 — matching is against the BARE name, so a prefixed item is
        // still selected by its bare ref (the prefix is an install-time transform).
        let mut item = cat(ItemKind::Skill, "review", "a");
        item.prefix = Some("pfx".to_string());
        assert_eq!(item.effective_name(), "pfx:review");
        let items = vec![item];

        // The bare ref selects it.
        let by_bare = select_by_bare_refs(&items, &["skill:review".to_string()]);
        assert_eq!(by_bare.len(), 1, "bare ref must select the prefixed item");

        // A ref written with the prefix does NOT match (refs are bare in source truth).
        let by_prefixed = select_by_bare_refs(&items, &["skill:pfx:review".to_string()]);
        assert!(
            by_prefixed.is_empty(),
            "a prefixed-name ref must not match; refs are bare names"
        );
    }

    #[test]
    fn select_by_bare_refs_skips_kindless_and_malformed_refs() {
        // spec: DSC-62 — a ref with no explicit kind (bare name only) is not
        // matched here, and a malformed ref is skipped rather than panicking.
        let items = vec![
            cat(ItemKind::Skill, "review", "a"),
            cat(ItemKind::Agent, "dev", "a"),
        ];
        // Bare name (no kind) matches nothing.
        assert!(
            select_by_bare_refs(&items, &["review".to_string()]).is_empty(),
            "a kindless ref must not select anything"
        );
        // A non-kind-word prefix is kindless (parses to kind=None), so the
        // kindless filter drops it; an empty name after a kind word is malformed.
        // Both are skipped.
        assert!(select_by_bare_refs(&items, &["bogus:review".to_string()]).is_empty());
        assert!(select_by_bare_refs(&items, &["skill:".to_string()]).is_empty());
        assert!(select_by_bare_refs(&items, &["".to_string()]).is_empty());
    }

    #[test]
    fn select_by_bare_refs_ref_matching_nothing_yields_empty() {
        // spec: DSC-62 — a well-formed ref naming an item that is not present
        // selects nothing (validation of unknown refs is the caller's job).
        let items = vec![cat(ItemKind::Skill, "review", "a")];
        assert!(
            select_by_bare_refs(&items, &["skill:absent".to_string()]).is_empty(),
            "a ref matching no item yields an empty subset"
        );
        // An empty ref list selects nothing.
        assert!(select_by_bare_refs(&items, &[]).is_empty());
    }

    #[test]
    fn select_by_bare_refs_duplicate_refs_do_not_duplicate_items() {
        // spec: DSC-62 — the result is the matching subset in catalog order; a
        // duplicate ref does not yield the same item twice (iteration is over the
        // catalog, filtered by the ref set).
        let items = vec![
            cat(ItemKind::Skill, "review", "a"),
            cat(ItemKind::Agent, "dev", "a"),
        ];
        let refs = vec![
            "skill:review".to_string(),
            "skill:review".to_string(),
            "skill:review".to_string(),
        ];
        let picked = select_by_bare_refs(&items, &refs);
        assert_eq!(
            picked.len(),
            1,
            "a duplicated ref must not duplicate the item"
        );
        assert_eq!(picked[0].name, "review");
    }

    #[test]
    fn select_by_bare_refs_preserves_catalog_order() {
        // spec: DSC-62 — the subset is returned in catalog order, independent of
        // the order refs are listed in install-items.
        let items = vec![
            cat(ItemKind::Skill, "review", "a"),
            cat(ItemKind::Agent, "dev", "a"),
            cat(ItemKind::Rule, "style", "a"),
        ];
        // Refs in reverse order; result follows catalog order.
        let refs = vec![
            "rule:style".to_string(),
            "agent:dev".to_string(),
            "skill:review".to_string(),
        ];
        let picked = select_by_bare_refs(&items, &refs);
        assert_eq!(picked.len(), 3);
        assert_eq!(picked[0].name, "review");
        assert_eq!(picked[1].name, "dev");
        assert_eq!(picked[2].name, "style");
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
