//! Unmanaged lobe items: skills/agents/rules/commands/workflows present in a
//! configured agent home that `mind` did not install (spec/unmanaged.md). They
//! are surfaced read-only by `recall` and `probe`, and removable via `forget`
//! with a distinct warning.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::{ItemKind, MindError, Result};
use crate::manifest::Manifest;
use crate::paths::Paths;
use crate::resolve::ItemRef;
use crate::sanitize::ItemKey;

/// A skill/agent/rule/command/workflow present in an agent home that `mind`
/// did not install.
#[derive(Debug, Clone)]
pub struct UnmanagedItem {
    pub kind: ItemKind,
    /// The on-disk entry name: a skill directory name, an agent/rule/command
    /// file stem (the `.md` suffix stripped), or a workflow file stem (the
    /// `.js` suffix stripped).
    pub name: String,
    /// The lobe path(s) occupying this item, sorted, one per agent home.
    pub paths: Vec<PathBuf>,
}

impl UnmanagedItem {
    /// `kind:name`, matching the manifest key form so refs resolve uniformly.
    ///
    /// Returns an [`ItemKey`] (DSC-95): the name is a lobe filesystem
    /// component and can carry ANSI/control/bidi code points, so the raw and
    /// display readings are distinct types. `.as_str()` for identity (ref
    /// resolution); `.display()` at every human/`--json` print site --
    /// `ItemKey` has no `Display`, so passing it straight to
    /// `println!`/`format!` is a compile error.
    pub fn key(&self) -> ItemKey {
        ItemKey::new(format!("{}:{}", self.kind.as_str(), self.name))
    }

    /// Convenience for [`ItemKey::display`] on `self.key()` (DSC-95).
    pub fn display_key(&self) -> String {
        self.key().display()
    }
}

/// Scan every configured agent home for unmanaged items (UNM-1): kind-dir entries
/// whose path is not a managed link recorded in the manifest. Deduplicated by
/// `(kind, name)` across lobes, each recording the lobe paths it occupies, sorted
/// by `(kind, name)`. An entry whose derived name fails the safety check is
/// skipped with a warning (UNM-9), not a hard failure of the whole scan: this
/// is a passively discovered on-disk entry, the exact analog of the catalog
/// scan's own hostile-name handling (DSC-96/DSC-102), so the same single-entry
/// severity applies here.
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
    for lobe in paths.agent_homes()? {
        // Unmanaged detection is kind-agnostic: scan every lobe path regardless
        // of its `kinds` filter (a filtered lobe can still hold a hand-placed
        // item of an excluded kind).
        let home = &lobe.path;
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
                match item_name(kind, &entry) {
                    Ok(Some(name)) => {
                        found.entry((kind, name)).or_default().push(path);
                    }
                    Ok(None) => {}
                    // spec: UNM-9 -- skip this one entry and keep scanning, the
                    // same severity the catalog scan gives a hostile
                    // source-declared name (DSC-96/DSC-102): a single hostile
                    // lobe file must not take the rest of the listing with it.
                    Err(MindError::UnsafeName { name }) => {
                        crate::render::scan_warn(format!(
                            "warning: skipping unmanaged {} '{}': unsafe item name (a control \
                             character, a path separator, or a bidi/zero-width Unicode code \
                             point)",
                            kind.as_str(),
                            crate::sanitize::strip_ansi(&name),
                        ));
                    }
                    Err(e) => return Err(e),
                }
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

/// The item name for a kind-dir entry, `Ok(None)` when the entry is not a
/// well-formed item of that kind, or `Err(UnsafeName)` when a name derives but
/// fails the safety check (UNM-9). A skill is the directory `skills/<name>`; an
/// agent/rule/command is the file `<name>.md`; a workflow the file `<name>.js`
/// (WF-50). Only the immediate children of
/// the kind directory are scanned (CMD-8): a nested `commands/<group>/<name>.md`
/// (the grouped layout mirrored on the source side) is not walked into and so
/// is neither surfaced by `recall`/`probe` nor reachable by `absorb`.
///
/// A naive `strip_suffix` on the filename, with no validation of what it
/// leaves behind, would let a lobe file literally named `...js` (or `...md`)
/// derive the name `..` -- a valid path component that later gets joined onto
/// a kind directory to build a destination path (e.g. in `absorb`), a store
/// path (`Paths::store_rel`), and a staging/backup path, escaping every one of
/// them. So every derived name, regardless of kind, goes through the very same
/// predicate the catalog scan applies to a source-declared name,
/// [`crate::catalog::is_safe_item_name`] (empty, `.`, `..`, a path separator,
/// NUL, or a blocked Unicode code point -- DSC-96). One definition, not a
/// mirrored copy: a name that fails it is returned as `Err(UnsafeName)` rather
/// than surfaced as a resolvable, absorbable item; the caller (`scan`) turns
/// that into a skip-with-warning for this one entry rather than failing the
/// whole scan (UNM-9).
fn item_name(kind: ItemKind, entry: &std::fs::DirEntry) -> Result<Option<String>> {
    let raw = entry.file_name();
    let Some(name) = raw.to_str() else {
        return Ok(None);
    };
    let derived = match kind {
        ItemKind::Skill => Some(name.to_string()),
        ItemKind::Agent | ItemKind::Rule | ItemKind::Command => {
            name.strip_suffix(".md").map(str::to_string)
        }
        // spec: WF-50 -- a workflow is the file `<name>.js`, and only the
        // immediate children of `workflows/` are scanned, matching the flat
        // convention scan (WF-2).
        // A directory named `deploy.js` is not a workflow the harness loads, so
        // only a file (or a symlink resolving to one) derives a name.
        ItemKind::Workflow => name
            .strip_suffix(".js")
            .filter(|_| entry.path().is_file())
            .map(str::to_string),
        ItemKind::Tool => None,
    };
    let Some(derived) = derived else {
        return Ok(None);
    };
    if crate::catalog::is_safe_item_name(&derived) {
        Ok(Some(derived))
    } else {
        // spec: UNM-9
        Err(MindError::UnsafeName { name: derived })
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
            // spec: DSC-95 -- `MindError::AmbiguousItem`'s `#[error(...)]`
            // Display joins `candidates` verbatim, with no sanitizing step of
            // its own; a lobe filesystem entry name is not restricted against
            // ANSI/control/bidi code points the way a catalog item name is.
            candidates: many.iter().map(|it| it.key().display()).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::parse_item_ref;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

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
    /// only for agents/rules/commands.
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

    /// `resolve`'s `AmbiguousItem` candidates must reach the caller already
    /// sanitized (DSC-95). This is the ONE unrestricted sanitize site in the
    /// codebase: an unmanaged item's `name` is a lobe filesystem entry read
    /// straight off disk, not gated by `is_safe_item_name` the way a catalog
    /// name is (DSC-96 only guards catalog names). So an ESC byte can
    /// actually reach `it.key().display()` here in practice, unlike most
    /// other sanitize call sites which are defense-in-depth against an input
    /// that is already restricted upstream. Reverting `key().display()` to
    /// `key().as_str().to_string()` at the `candidates` call site would still
    /// compile and would still pass every OTHER test in this module -- only
    /// this one exercises the raw ESC byte actually surviving to the
    /// `candidates` list.
    // spec: DSC-95
    #[test]
    fn resolve_candidates_are_sanitized_for_a_hostile_unmanaged_name() {
        let items = vec![
            UnmanagedItem {
                kind: ItemKind::Skill,
                name: "evil\x1b[31mname".to_string(),
                paths: vec![],
            },
            UnmanagedItem {
                kind: ItemKind::Agent,
                name: "evil\x1b[31mname".to_string(),
                paths: vec![],
            },
        ];
        let r = parse_item_ref("evil\x1b[31mname").unwrap();
        let err = resolve(&items, &r).unwrap_err();
        match err {
            MindError::AmbiguousItem { candidates, .. } => {
                assert!(
                    candidates.iter().all(|c| !c.contains('\x1b')),
                    "candidates must not carry a raw ESC byte: {candidates:?}"
                );
                assert!(
                    candidates.iter().any(|c| c.contains("evil")),
                    "candidates must still name the item: {candidates:?}"
                );
            }
            other => panic!("expected AmbiguousItem, got {other:?}"),
        }
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

    /// A lobe file whose filename strips to an unsafe derived name (`...js` ->
    /// `..`) is skipped rather than silently surfacing as a
    /// scannable/absorbable item named `..`. The scan itself still succeeds
    /// (UNM-9's severity is a per-entry skip, not a whole-scan failure): a
    /// safe sibling in the same directory proves the fix does not
    /// blanket-reject the rest of the listing along with it.
    // spec: UNM-9
    #[test]
    fn scan_skips_a_workflow_whose_derived_name_is_unsafe() {
        // SAFETY: ENV_LOCK held for the duration of the env var mutation below.
        let _guard = crate::paths::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("mind-unm-unsafe-{}-{n}", std::process::id()));
        let workflows = home.join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("...js"), b"export const meta = {};").unwrap();
        std::fs::write(workflows.join("deploy.js"), b"export const meta = {};").unwrap();

        let saved_agent_homes = std::env::var_os("MIND_AGENT_HOMES");
        let saved_policy = std::env::var_os("MIND_POLICY_FILE");
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", home.to_str().unwrap());
            std::env::remove_var("MIND_POLICY_FILE");
        }
        let paths = Paths {
            mind_home: home.join("mind-home-unused"),
            claude_home: home.clone(),
        };
        let result = scan(&paths, &Manifest::default());
        unsafe {
            match saved_agent_homes {
                Some(v) => std::env::set_var("MIND_AGENT_HOMES", v),
                None => std::env::remove_var("MIND_AGENT_HOMES"),
            }
            if let Some(v) = saved_policy {
                std::env::set_var("MIND_POLICY_FILE", v);
            }
        }
        let _ = std::fs::remove_dir_all(&home);

        let items = result.expect("scan must succeed despite the one unsafe entry");
        assert_eq!(
            items.len(),
            1,
            "the unsafe entry must be skipped, not listed: {items:?}"
        );
        assert_eq!(items[0].kind, ItemKind::Workflow);
        assert_eq!(
            items[0].name, "deploy",
            "the well-formed sibling must still be listed: {items:?}"
        );
    }

    /// The `DirEntry` in `dir` whose file name is exactly `name`.
    fn entry_named(dir: &std::path::Path, name: &str) -> std::fs::DirEntry {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_str() == Some(name))
            .unwrap_or_else(|| panic!("no dir entry named {name:?} in {dir:?}"))
    }

    /// `item_name` validates the derived name for EVERY suffix-stripped kind,
    /// not just workflow's, and each of the three unsafe derivations a bare
    /// suffix strip can produce is refused:
    ///
    /// | filename | derives | as a path component |
    /// |----------|---------|---------------------|
    /// | `...md`  | `..`    | the kind dir's PARENT (`store/agent/..` -> `store`) |
    /// | `..md`   | `.`     | the kind dir ITSELF (`store/agent/.` -> `store/agent`) |
    /// | `.md`    | `` (empty) | the kind dir itself, with a trailing separator |
    ///
    /// All three would make `Paths::store_rel`/`staging_path`/`backup_path`
    /// name a directory that holds OTHER items, so an install swap or an
    /// uninstall keyed on the recorded path would clobber them. A well-formed
    /// sibling in the same directory still derives normally, and an entry that
    /// is not an item of the kind at all stays `Ok(None)` rather than becoming
    /// an error.
    // spec: UNM-9
    #[test]
    fn item_name_refuses_every_unsafe_derivation_for_every_kind() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-unm-derive-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "...md",
            "..md",
            ".md",
            "...js",
            "..js",
            ".js",
            "ok.md",
            "ok.js",
            "plain.txt",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        // A skill is directory-shaped, so its name is the entry name verbatim:
        // a `.`/`..` name is unrepresentable on disk, but a blocked Unicode
        // code point (here a RIGHT-TO-LEFT OVERRIDE) is not.
        std::fs::create_dir(dir.join("okdir")).unwrap();
        std::fs::create_dir(dir.join("ha\u{202E}ck")).unwrap();

        let unsafe_name = |kind: ItemKind, file: &str, want: &str| {
            let entry = entry_named(&dir, file);
            match item_name(kind, &entry) {
                Err(MindError::UnsafeName { name }) => assert_eq!(
                    name, want,
                    "{kind:?} {file:?} must be refused with the derived name {want:?}"
                ),
                other => panic!("{kind:?} {file:?}: expected Err(UnsafeName), got {other:?}"),
            }
        };

        // Every `.md` kind shares one arm; assert each so a future per-kind
        // split cannot drop the check from one of them silently.
        for kind in [ItemKind::Agent, ItemKind::Rule, ItemKind::Command] {
            unsafe_name(kind, "...md", "..");
            unsafe_name(kind, "..md", ".");
            unsafe_name(kind, ".md", "");
            assert_eq!(
                item_name(kind, &entry_named(&dir, "ok.md")).unwrap(),
                Some("ok".to_string()),
                "{kind:?}: a well-formed sibling must still derive its name"
            );
            // Not an item of this kind at all: skipped, not an error.
            assert_eq!(
                item_name(kind, &entry_named(&dir, "plain.txt")).unwrap(),
                None,
                "{kind:?}: a non-matching suffix must be skipped, not refused"
            );
        }

        // spec: WF-50 -- the `.js` strip has the identical three-way failure.
        unsafe_name(ItemKind::Workflow, "...js", "..");
        unsafe_name(ItemKind::Workflow, "..js", ".");
        unsafe_name(ItemKind::Workflow, ".js", "");
        assert_eq!(
            item_name(ItemKind::Workflow, &entry_named(&dir, "ok.js")).unwrap(),
            Some("ok".to_string())
        );
        // A directory named `x.js` is not a workflow file: skipped.
        std::fs::create_dir(dir.join("dirflow.js")).unwrap();
        assert_eq!(
            item_name(ItemKind::Workflow, &entry_named(&dir, "dirflow.js")).unwrap(),
            None
        );
        // A `.md` file is not a workflow, and a `.js` file is not an agent:
        // the wrong-suffix entry is skipped by both, never cross-derived.
        assert_eq!(
            item_name(ItemKind::Workflow, &entry_named(&dir, "...md")).unwrap(),
            None
        );
        assert_eq!(
            item_name(ItemKind::Agent, &entry_named(&dir, "...js")).unwrap(),
            None
        );

        // Skill: the verbatim entry name, checked by the same predicate.
        assert_eq!(
            item_name(ItemKind::Skill, &entry_named(&dir, "okdir")).unwrap(),
            Some("okdir".to_string())
        );
        unsafe_name(ItemKind::Skill, "ha\u{202E}ck", "ha\u{202E}ck");
        // A tool is never linked into a lobe (TOOL-3), so it never derives a
        // name -- and so never errors either, for any entry.
        assert_eq!(
            item_name(ItemKind::Tool, &entry_named(&dir, "...md")).unwrap(),
            None
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
