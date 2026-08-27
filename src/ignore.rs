//! Ignored files (spec/ignore.md, IGN-1..21): which paths under an item are
//! excluded from the store copy and from the content hash.
//!
//! An item's path names a directory whose whole tree is copied at install and
//! walked to compute the drift hash (LIFE-15). That is fine while the directory
//! holds only the item, and wrong the moment an item's path IS the repo root,
//! which a source declares to ship a top-level `SKILL.md`: `.git/` lands in the
//! store and the item reads as drifted after every commit in the source clone.
//!
//! The rule this module enforces is one rule shared by both walks: what is not
//! installed is not hashed (IGN-10). Hashing a file that is not installed makes
//! `upgrade` offer changes the user cannot see; installing a file that is not
//! hashed makes a change to it invisible to drift detection.
//!
//! Ignores are source truth (declared in `mind.toml`), never local config, so
//! an item's hash is a property of the source rather than of the machine that
//! computed it.

use std::path::Path;

use crate::error::{MindError, Result};

/// Version-control metadata directories, excluded from every item's copy and
/// hash whether or not the source declares anything (IGN-2).
///
/// This is the whole of the built-in set. Build and dependency output
/// (`target/`, `node_modules/`, `__pycache__/`) is deliberately NOT implied:
/// mind cannot tell a build directory from a directory a skill ships on
/// purpose, and guessing would silently drop a file the author meant to
/// install. A source that wants those excluded lists them.
pub const BUILTIN_IGNORES: [&str; 4] = [".git", ".hg", ".svn", ".bzr"];

/// One compiled ignore entry.
#[derive(Debug, Clone)]
struct Rule {
    pattern: glob::Pattern,
    /// A trailing `/` in the entry: matches a directory (and its subtree) only,
    /// never a file of the same name (IGN-3).
    dir_only: bool,
}

/// The compiled ignore set for one item: the built-ins plus whatever the source
/// declared for it (IGN-1).
#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    rules: Vec<Rule>,
}

impl IgnoreSet {
    /// Compile `entries` (already resolved to this item's effective list) into a
    /// matcher, validating each as a safe relative pattern (IGN-4).
    ///
    /// `item` names the item the entries belong to, for the error message.
    pub fn new(entries: &[String], item: &str) -> Result<Self> {
        let mut rules = Vec::new();
        for raw in entries {
            let entry = raw.trim();
            // spec: IGN-4 -- a pattern is a safe relative path (the DSC-71..73
            // rule): an absolute or `~`-rooted entry, a `..` component, or a NUL
            // is a hard error rather than a silently inert entry, since an entry
            // that never matches reads at a glance like one that does.
            if entry.is_empty() {
                return Err(bad_ignore(item, raw, "it is empty"));
            }
            if entry.contains('\0') {
                return Err(bad_ignore(item, raw, "it contains a NUL byte"));
            }
            if entry.starts_with('/') || entry.starts_with('~') {
                return Err(bad_ignore(
                    item,
                    raw,
                    "it is not relative; an ignore pattern is matched against paths \
                     inside the item, so it must not start with '/' or '~'",
                ));
            }
            let dir_only = entry.ends_with('/');
            let body = entry.trim_end_matches('/');
            if body.split('/').any(|c| c == "..") {
                return Err(bad_ignore(
                    item,
                    raw,
                    "it has a '..' component, which would reach outside the item",
                ));
            }
            let pattern = glob::Pattern::new(body)
                .map_err(|e| bad_ignore(item, raw, &format!("it is not a valid glob ({e})")))?;
            rules.push(Rule { pattern, dir_only });
        }
        // The built-ins are appended last and are never user-supplied, so they
        // cannot fail to compile.
        for name in BUILTIN_IGNORES {
            rules.push(Rule {
                pattern: glob::Pattern::new(name).expect("built-in ignore is a valid glob"),
                dir_only: true,
            });
        }
        Ok(IgnoreSet { rules })
    }

    /// The built-ins alone: the set every item has with no declaration (IGN-2).
    pub fn builtin() -> Self {
        IgnoreSet::new(&[], "").expect("built-in-only set always compiles")
    }

    /// Whether `rel` (a path relative to the item root, `/`-separated) is
    /// excluded. `is_dir` selects whether a directory-only rule can match.
    ///
    /// A matching directory is not descended into by the callers, so its whole
    /// subtree is skipped without being walked (IGN-3).
    pub fn is_ignored(&self, rel: &Path, is_dir: bool) -> bool {
        // Compare on a `/`-joined rendering so a pattern reads the same on every
        // platform, matching how the patterns are written in `mind.toml`.
        let rel: String = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if rel.is_empty() {
            return false;
        }
        // spec: IGN-3 -- `require_literal_separator` makes `*` stop at a `/`
        // and `**` cross it, which is what a path pattern is expected to mean.
        // The crate's DEFAULT is the opposite (a bare `*` spans separators), so
        // this is not the same matching the item selectors use: those match a
        // flat item name, where the question never arises.
        let opts = glob::MatchOptions {
            require_literal_separator: true,
            ..glob::MatchOptions::new()
        };
        self.rules.iter().any(|r| {
            if r.dir_only && !is_dir {
                return false;
            }
            r.pattern.matches_with(&rel, opts)
        })
    }

    /// Whether any ancestor directory of `rel` is ignored, which makes `rel`
    /// itself ignored.
    ///
    /// The tree walks skip an ignored directory outright and never look inside
    /// it, so they do not need this. It is for the callers that hold a file path
    /// directly (the `expand:` conflict check, IGN-12, and the file-list filters
    /// of IGN-11) and must reach the same verdict the walk would.
    pub fn is_under_ignored(&self, rel: &Path) -> bool {
        let mut prefix = std::path::PathBuf::new();
        let mut comps: Vec<_> = rel.components().collect();
        // The final component is the file itself; ancestors are everything before.
        comps.pop();
        for c in comps {
            prefix.push(c);
            if self.is_ignored(&prefix, true) {
                return true;
            }
        }
        self.is_ignored(rel, false)
    }
}

fn bad_ignore(item: &str, entry: &str, reason: &str) -> MindError {
    // spec: DSC-95 -- the entry is raw `mind.toml` text, so it is sanitized
    // before being composed into the message.
    MindError::BadIgnorePattern {
        item: crate::sanitize::strip_ansi(item),
        entry: crate::sanitize::strip_ansi(entry),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn set(entries: &[&str]) -> IgnoreSet {
        IgnoreSet::new(
            &entries.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "skill:t",
        )
        .expect("compiles")
    }

    /// The built-in set applies with no declaration at all, and only to
    /// directories: a FILE named `.git` is an ordinary file.
    // spec: IGN-2
    #[test]
    fn builtin_vcs_dirs_are_ignored_without_any_declaration() {
        let s = IgnoreSet::builtin();
        for name in BUILTIN_IGNORES {
            assert!(
                s.is_ignored(&PathBuf::from(name), true),
                "{name} must be ignored as a directory"
            );
            assert!(
                !s.is_ignored(&PathBuf::from(name), false),
                "{name} as a FILE is ordinary content"
            );
        }
        // Build output is deliberately not implied.
        assert!(!s.is_ignored(&PathBuf::from("target"), true));
        assert!(!s.is_ignored(&PathBuf::from("node_modules"), true));
    }

    /// A trailing `/` restricts a rule to directories; without it either matches.
    // spec: IGN-3
    #[test]
    fn trailing_slash_marks_a_directory_only_rule() {
        let dir_only = set(&["scratch/"]);
        assert!(dir_only.is_ignored(&PathBuf::from("scratch"), true));
        assert!(
            !dir_only.is_ignored(&PathBuf::from("scratch"), false),
            "a file named `scratch` is not what `scratch/` names"
        );
        let either = set(&["scratch"]);
        assert!(either.is_ignored(&PathBuf::from("scratch"), true));
        assert!(either.is_ignored(&PathBuf::from("scratch"), false));
    }

    /// `*` does not cross a separator; `**` does.
    // spec: IGN-3
    #[test]
    fn glob_semantics_match_the_item_selectors() {
        let s = set(&["*.tmp", "docs/**"]);
        assert!(s.is_ignored(&PathBuf::from("a.tmp"), false));
        assert!(
            !s.is_ignored(&PathBuf::from("sub/a.tmp"), false),
            "`*` must not cross a `/`"
        );
        assert!(s.is_ignored(&PathBuf::from("docs/guide/x.md"), false));
    }

    /// An ancestor match makes a nested file ignored, which is how the
    /// file-list filters reach the same verdict the tree walk does.
    // spec: IGN-11
    #[test]
    fn a_file_under_an_ignored_directory_is_ignored() {
        let s = set(&["scratch/"]);
        assert!(s.is_under_ignored(&PathBuf::from("scratch/notes/a.md")));
        assert!(!s.is_under_ignored(&PathBuf::from("keep/a.md")));
        // The built-ins participate too.
        assert!(IgnoreSet::builtin().is_under_ignored(&PathBuf::from(".git/config")));
    }

    /// Unsafe and malformed patterns are refused at compile, not left inert.
    // spec: IGN-4
    #[test]
    fn unsafe_or_malformed_patterns_are_refused() {
        for bad in [
            "",
            "  ",
            "/etc/passwd",
            "~/secrets",
            "../outside",
            "a/../b",
            "[bad",
        ] {
            let err = IgnoreSet::new(&[bad.to_string()], "skill:t")
                .expect_err(&format!("{bad:?} must be refused"));
            assert!(
                matches!(err, MindError::BadIgnorePattern { .. }),
                "{bad:?} must be a BadIgnorePattern, got {err}"
            );
        }
        // A `..` that is only part of a NAME is fine.
        assert!(IgnoreSet::new(&["a..b".to_string()], "skill:t").is_ok());
    }

    /// The root itself is never ignored, whatever the rules say: an item is not
    /// its own exclusion.
    // spec: IGN-5
    #[test]
    fn the_item_root_is_never_ignored() {
        let s = set(&["*"]);
        assert!(!s.is_ignored(&PathBuf::from(""), true));
    }
}
