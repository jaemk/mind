//! Unmanaged lobe items: skills/agents/rules present in a configured agent home
//! that `mind` did not install (spec/unmanaged.md). They are surfaced read-only
//! by `recall` and `probe`, and removable via `forget` with a distinct warning.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::{ItemKind, MindError, Result};
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::resolve::ItemRef;

/// A skill/agent/rule present in an agent home that `mind` did not install.
#[derive(Debug, Clone)]
pub struct UnmanagedItem {
    pub kind: ItemKind,
    /// The on-disk entry name: a skill directory name, or an agent/rule file
    /// stem (the `.md` suffix stripped).
    pub name: String,
    /// The lobe path(s) occupying this item, sorted, one per agent home.
    pub paths: Vec<PathBuf>,
}

impl UnmanagedItem {
    /// `kind:name`, matching the manifest key form so refs resolve uniformly.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }
}

/// Scan every configured agent home for unmanaged items (UNM-1): kind-dir entries
/// whose path is not a managed link recorded in the manifest. Deduplicated by
/// `(kind, name)` across lobes, each recording the lobe paths it occupies, sorted
/// by `(kind, name)`.
pub fn scan(paths: &Paths, manifest: &Manifest) -> Result<Vec<UnmanagedItem>> {
    // Every managed link path, for the "is this mind's own link?" test. Install
    // records links via the same `agent_homes` paths we walk here (STO-21), so a
    // direct path comparison matches.
    let managed: std::collections::HashSet<PathBuf> = manifest
        .items
        .values()
        .flat_map(|it| it.links.iter())
        .map(PathBuf::from)
        .collect();

    let mut found: BTreeMap<(ItemKind, String), Vec<PathBuf>> = BTreeMap::new();
    for home in paths.agent_homes()? {
        // Tools are never linked into an agent home (tooling.md TOOL-3), so only
        // the linkable kinds are scanned.
        for kind in ItemKind::LINKABLE {
            // A missing kind dir simply has no items.
            let Ok(rd) = std::fs::read_dir(home.join(kind.dir())) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if managed.contains(&path) {
                    continue; // mind's own link
                }
                let Some(name) = item_name(kind, &entry) else {
                    continue;
                };
                found.entry((kind, name)).or_default().push(path);
            }
        }
    }

    Ok(found
        .into_iter()
        .map(|((kind, name), mut paths)| {
            paths.sort();
            UnmanagedItem { kind, name, paths }
        })
        .collect())
}

/// The item name for a kind-dir entry, or `None` when the entry is not a
/// well-formed item of that kind. A skill is the directory `skills/<name>`; an
/// agent/rule is the file `<name>.md`.
fn item_name(kind: ItemKind, entry: &std::fs::DirEntry) -> Option<String> {
    let raw = entry.file_name();
    let name = raw.to_str()?;
    match kind {
        ItemKind::Skill => Some(name.to_string()),
        ItemKind::Agent | ItemKind::Rule => name.strip_suffix(".md").map(str::to_string),
        ItemKind::Tool => None,
    }
}

/// Select every unmanaged item matching the optional ref `r` (UNM-7).
///
/// - `None`  -> all items (the no-ref "remove everything" form).
/// - `Some(r)` with a source qualifier -> empty (unmanaged items have no source).
/// - `Some(r)` with a glob name -> every item whose name matches the pattern,
///   filtered by `r.kind` when given.
/// - `Some(r)` with an exact name -> items whose name equals `r.name`, filtered
///   by `r.kind`.
///
/// Managed items can never appear here because `scan` already excludes them.
// spec: UNM-7
pub fn select<'a>(items: &'a [UnmanagedItem], r: Option<&ItemRef>) -> Vec<&'a UnmanagedItem> {
    let Some(r) = r else {
        return items.iter().collect();
    };
    if r.source.is_some() {
        return vec![];
    }
    if crate::resolve::is_glob(&r.name) {
        let pattern = match glob::Pattern::new(&r.name) {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        items
            .iter()
            .filter(|it| r.kind.is_none_or(|k| it.kind == k) && pattern.matches(&it.name))
            .collect()
    } else {
        items
            .iter()
            .filter(|it| r.kind.is_none_or(|k| it.kind == k) && it.name == r.name)
            .collect()
    }
}

/// Find the single unmanaged item matching `r` (UNM-4). A source-qualified ref
/// never matches (unmanaged items have no source). Errors `NotInstalled` on no
/// match and `AmbiguousItem` on more than one (a bare name shared across kinds).
pub fn resolve<'a>(items: &'a [UnmanagedItem], r: &ItemRef) -> Result<&'a UnmanagedItem> {
    if r.source.is_some() {
        return Err(MindError::NotInstalled {
            name: r.name.clone(),
        });
    }
    let matches: Vec<&UnmanagedItem> = items
        .iter()
        .filter(|it| it.name == r.name && r.kind.is_none_or(|k| it.kind == k))
        .collect();
    match matches.as_slice() {
        [] => Err(MindError::NotInstalled {
            name: r.name.clone(),
        }),
        [only] => Ok(only),
        many => Err(MindError::AmbiguousItem {
            query: r.name.clone(),
            candidates: many.iter().map(|it| it.key()).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::parse_item_ref;

    fn make_items() -> Vec<UnmanagedItem> {
        vec![
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "review".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "style".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Agent,
                name: "dev".to_string(),
                paths: vec![],
            },
        ]
    }

    /// select(None) returns all items.
    // spec: UNM-7
    #[test]
    fn select_none_returns_all() {
        let items = make_items();
        let result = select(&items, None);
        assert_eq!(result.len(), 3);
    }

    /// select with a glob `*` matches all items.
    // spec: UNM-7
    #[test]
    fn select_glob_star_matches_all() {
        let items = make_items();
        let r = parse_item_ref("*").unwrap();
        let result = select(&items, Some(&r));
        assert_eq!(result.len(), 3);
    }

    /// select with a kind-qualified glob `skill:*` matches only skills.
    // spec: UNM-7
    #[test]
    fn select_kind_glob_filters_by_kind() {
        let items = make_items();
        let r = parse_item_ref("skill:*").unwrap();
        let result = select(&items, Some(&r));
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|it| it.kind == ItemKind::Skill));
    }

    /// select with an exact name matches only that item.
    // spec: UNM-7
    #[test]
    fn select_exact_name() {
        let items = make_items();
        let r = parse_item_ref("review").unwrap();
        let result = select(&items, Some(&r));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "review");
    }

    /// select with a kind-qualified exact name matches only that item.
    // spec: UNM-7
    #[test]
    fn select_kind_exact_name() {
        let items = make_items();
        let r = parse_item_ref("agent:dev").unwrap();
        let result = select(&items, Some(&r));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, ItemKind::Agent);
        assert_eq!(result[0].name, "dev");
    }

    /// select with a source-qualified ref always returns empty (unmanaged items
    /// have no source).
    // spec: UNM-7
    #[test]
    fn select_source_qualified_returns_empty() {
        let items = make_items();
        let r = parse_item_ref("owner/repo#skill:review").unwrap();
        let result = select(&items, Some(&r));
        assert!(result.is_empty());
    }

    /// select with a ref that matches nothing returns empty.
    // spec: UNM-7
    #[test]
    fn select_no_match_returns_empty() {
        let items = make_items();
        let r = parse_item_ref("nope").unwrap();
        let result = select(&items, Some(&r));
        assert!(result.is_empty());
    }

    /// A bare exact name shared across kinds matches EVERY kind with that name
    /// (the bulk `select` path treats it uniformly), unlike the single-item
    /// `resolve` path which errors `AmbiguousItem`. Both are removed.
    // spec: UNM-7
    #[test]
    fn select_bare_name_matches_all_kinds() {
        let items = vec![
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "shared".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Agent,
                name: "shared".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Rule,
                name: "other".to_string(),
                paths: vec![],
            },
        ];
        let r = parse_item_ref("shared").unwrap();
        let result = select(&items, Some(&r));
        assert_eq!(result.len(), 2, "both `shared` items must match");
        assert!(result.iter().all(|it| it.name == "shared"));
    }

    /// A glob with a kind filter that matches no item of that kind returns empty
    /// even when the bare name pattern would match a different kind.
    // spec: UNM-7
    #[test]
    fn select_kind_glob_excludes_other_kinds() {
        let items = make_items(); // review/style skills, dev agent
        let r = parse_item_ref("agent:*e*").unwrap();
        let result = select(&items, Some(&r));
        // Only the agent `dev` matches `*e*` among agents.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, ItemKind::Agent);
        assert_eq!(result[0].name, "dev");
    }

    /// select with a glob that matches nothing returns empty.
    // spec: UNM-7
    #[test]
    fn select_glob_no_match_returns_empty() {
        let items = make_items();
        let r = parse_item_ref("nope*").unwrap();
        let result = select(&items, Some(&r));
        assert!(result.is_empty());
    }

    /// `key()` is the `kind:name` manifest form, and `item_name` strips `.md`
    /// only for agents/rules.
    /// spec: UNM-1
    #[test]
    fn key_and_name_forms() {
        let u = UnmanagedItem {
            kind: ItemKind::Agent,
            name: "dev".to_string(),
            paths: vec![],
        };
        assert_eq!(u.key(), "agent:dev");
        assert_eq!(
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "review".to_string(),
                paths: vec![]
            }
            .key(),
            "skill:review"
        );
    }

    /// resolve matches by name (kind-qualified disambiguates), rejects a
    /// source-qualified ref, and errors on ambiguity.
    /// spec: UNM-4
    #[test]
    fn resolve_matches_kind_and_rejects_source() {
        let items = vec![
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "x".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Agent,
                name: "x".to_string(),
                paths: vec![],
            },
        ];
        // A bare name shared across kinds is ambiguous.
        assert!(matches!(
            resolve(&items, &parse_item_ref("x").unwrap()),
            Err(MindError::AmbiguousItem { .. })
        ));
        // A kind prefix disambiguates.
        assert_eq!(
            resolve(&items, &parse_item_ref("agent:x").unwrap())
                .unwrap()
                .kind,
            ItemKind::Agent
        );
        // A source-qualified ref never matches an unmanaged item.
        assert!(matches!(
            resolve(&items, &parse_item_ref("owner/repo#skill:x").unwrap()),
            Err(MindError::NotInstalled { .. })
        ));
        // A miss is NotInstalled.
        assert!(matches!(
            resolve(&items, &parse_item_ref("nope").unwrap()),
            Err(MindError::NotInstalled { .. })
        ));
    }
}
