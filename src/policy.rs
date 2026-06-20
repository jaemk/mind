//! Enterprise managed policy (POL-1..50).
//!
//! An organization distributes a policy file to a fixed per-OS system path. When
//! present, it is authoritative over user configuration: it restricts melding to
//! a trusted allowlist, can require pinned sources, declares an auto-meld base
//! set, and can lock the agent homes ("lobes"). This module is the parsing and
//! validation barrier the enforcement shards build on; it is pure (no enforcement
//! side effects) and fail-closed (a malformed or invalid policy is a hard error).
//!
//! The locate seam ([`locate_with`]) is factored out so the system-path/env
//! precedence (POL-1, POL-2) is unit-testable without touching `/etc`.
//!
//! The enforcement shards (`meld`/`sync`/`evolve`/`config lobes`/`review`) consume
//! the public API here; until they wire it up the module is exercised only by its
//! own tests, so the API trips dead-code warnings. Scope the allow to this file
//! (mirroring how `src/deps.rs` was bootstrapped).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{MindError, Result};
use crate::source::Pin;

/// The fixed per-OS system path the managed policy is read from (POL-1). Not
/// relocatable by `MIND_HOME` or other user environment (POL-2). On Windows the
/// base lives under `%PROGRAMDATA%`, so it is resolved at runtime by
/// [`system_path`] rather than a compile-time constant.
#[cfg(target_os = "macos")]
const SYSTEM_PATH_FIXED: &str = "/Library/Application Support/mind/policy.toml";
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const SYSTEM_PATH_FIXED: &str = "/etc/mind/policy.toml";

/// The fixed per-OS system policy path (POL-1).
fn system_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        return base.join("mind").join("policy.toml");
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from(SYSTEM_PATH_FIXED)
    }
}

/// The environment variable honored only when no system file exists (POL-2).
const ENV_VAR: &str = "MIND_POLICY_FILE";

/// One `[[sources.auto_meld]]` entry: an org-provisioned source plus its pin
/// (POL-30). The pin is normalized to the shared [`Pin`] enum so enforcement
/// reuses the same provisioning path as a user `meld`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoMeld {
    /// The source identity or repo spec to provision.
    pub repo: String,
    /// The declared pin (tag, ref, follow-branch, or the default branch).
    pub pin: Pin,
}

/// A parsed and validated managed policy (POL-3 authoritative when in effect).
#[derive(Debug, Clone)]
pub struct Policy {
    /// Trusted-source allowlist patterns (POL-10), raw as written.
    allow: Vec<String>,
    /// `[sources].lock`: the enforcement switch for `allow` (POL-11/13).
    lock: bool,
    /// `[sources].pinned`: require every meld to resolve to a tag/ref (POL-20).
    pinned: bool,
    /// `[[sources.auto_meld]]` base set (POL-30).
    auto_meld: Vec<AutoMeld>,
    /// `[lobes].lock`: pin the effective agent homes to `lobes_targets` (POL-40).
    lobes_lock: bool,
    /// `[lobes].targets`: policy-provided agent homes (POL-40/41).
    lobes_targets: Vec<String>,
}

// --- TOML wire shape --------------------------------------------------------
//
// Deserialized with `deny_unknown_fields` (mirroring src/config.rs) so an
// unknown key is a hard error (POL-5). The structs below are the raw file shape;
// `Policy` is the validated public form.

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    #[serde(default)]
    sources: RawSources,
    #[serde(default)]
    lobes: RawLobes,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSources {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    lock: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    auto_meld: Vec<RawAutoMeld>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLobes {
    #[serde(default)]
    lock: bool,
    #[serde(default)]
    targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAutoMeld {
    repo: String,
    #[serde(default)]
    tag: Option<String>,
    // `ref` is a Rust keyword; rename the wire key onto a legal field name.
    #[serde(default, rename = "ref")]
    ref_: Option<String>,
    #[serde(default)]
    follow_branch: Option<String>,
}

impl RawAutoMeld {
    /// Normalize the at-most-one-pin TOML shape into the shared [`Pin`] enum.
    /// More than one of `tag`/`ref`/`follow_branch` is invalid (POL-5).
    fn into_auto_meld(self, path: &Path) -> Result<AutoMeld> {
        let pins = self.tag.is_some() as u8
            + self.ref_.is_some() as u8
            + self.follow_branch.is_some() as u8;
        if pins > 1 {
            return Err(MindError::InvalidPolicy {
                path: path.display().to_string(),
                reason: format!(
                    "auto_meld entry '{}' declares more than one of tag/ref/follow_branch; supply at most one",
                    self.repo
                ),
            });
        }
        let pin = if let Some(tag) = self.tag {
            Pin::Tag(tag)
        } else if let Some(r) = self.ref_ {
            Pin::Ref(r)
        } else if let Some(branch) = self.follow_branch {
            Pin::FollowBranch(branch)
        } else {
            Pin::DefaultBranch
        };
        Ok(AutoMeld {
            repo: self.repo,
            pin,
        })
    }
}

impl Policy {
    /// Load the managed policy, or `None` when none is configured (POL-4 inert).
    ///
    /// Reads the fixed per-OS system path; falls back to `$MIND_POLICY_FILE` only
    /// when no system file exists (POL-1, POL-2). A parse error, unknown key, or
    /// failed [`validate`](Policy::validate) is `Err` (POL-5 fail closed).
    pub fn load() -> Result<Option<Policy>> {
        let system = system_path();
        let env = std::env::var_os(ENV_VAR).map(PathBuf::from);
        let system_present = system.exists();
        match locate_with(
            if system_present { Some(&system) } else { None },
            env.as_deref(),
        ) {
            Some(path) => Ok(Some(load_file(&path)?)),
            None => Ok(None),
        }
    }

    /// Does `identity` (a `host/owner/repo` string) match an `allow` pattern?
    /// `*` matches within a single path segment (POL-10).
    pub fn allow_matches(&self, identity: &str) -> bool {
        self.allow.iter().any(|p| glob_match(p, identity))
    }

    /// Enforce the internal invariants. Every `auto_meld` entry has a tag/ref when
    /// `pinned` (POL-21); every `auto_meld` entry satisfies `allow` when `lock`
    /// (POL-31). Returns `Err(MindError::InvalidPolicy)` naming the problem.
    /// Called by [`load`](Policy::load) (fail closed) and reused by
    /// `mind review --policy`.
    pub fn validate(&self) -> Result<()> {
        self.validate_at(&system_path())
    }

    /// Validate, attributing any error to `path` (so file-based callers report the
    /// real file rather than the system constant).
    fn validate_at(&self, path: &Path) -> Result<()> {
        if self.pinned {
            for am in &self.auto_meld {
                let is_pinned = matches!(am.pin, Pin::Tag(_) | Pin::Ref(_));
                if !is_pinned {
                    return Err(MindError::InvalidPolicy {
                        path: path.display().to_string(),
                        reason: format!(
                            "auto_meld entry '{}' must declare a tag or ref because [sources].pinned is true",
                            am.repo
                        ),
                    });
                }
            }
        }
        if self.lock {
            for am in &self.auto_meld {
                // Match the parsed `host/owner/repo` identity against `allow`, the
                // same form runtime meld enforcement uses (POL-11), so a shorthand
                // spec (e.g. `acme/baseline`) validates against a host-qualified
                // pattern (e.g. `github.com/acme/*`). A repo that does not parse is
                // itself invalid.
                let identity = crate::source::parse_spec(&am.repo)
                    .map(|s| s.name)
                    .map_err(|_| MindError::InvalidPolicy {
                        path: path.display().to_string(),
                        reason: format!("auto_meld entry '{}' is not a valid repo spec", am.repo),
                    })?;
                if !self.allow_matches(&identity) {
                    return Err(MindError::InvalidPolicy {
                        path: path.display().to_string(),
                        reason: format!(
                            "auto_meld entry '{}' (identity '{identity}') is outside [sources].allow but [sources].lock is true",
                            am.repo
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// `[sources].lock` (POL-11/13).
    pub fn lock(&self) -> bool {
        self.lock
    }

    /// `[sources].pinned` (POL-20).
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    /// The `[[sources.auto_meld]]` base set (POL-30).
    pub fn auto_meld(&self) -> &[AutoMeld] {
        &self.auto_meld
    }

    /// `[lobes].lock` (POL-40).
    pub fn lobes_lock(&self) -> bool {
        self.lobes_lock
    }

    /// `[lobes].targets` (POL-40/41).
    pub fn lobes_targets(&self) -> &[String] {
        &self.lobes_targets
    }

    /// The raw `allow` patterns, for review reporting.
    pub fn allow(&self) -> &[String] {
        &self.allow
    }
}

/// Resolve the policy file path from the system path and env var, applying the
/// POL-1/POL-2 precedence: the system file is authoritative when present and the
/// env var is honored only when no system file exists. Pure so the precedence is
/// unit-testable without touching the real system path.
fn locate_with(system: Option<&Path>, env: Option<&Path>) -> Option<PathBuf> {
    if let Some(s) = system {
        return Some(s.to_path_buf());
    }
    env.map(|e| e.to_path_buf())
}

/// Parse + validate a policy file at an explicit path WITHOUT consulting the
/// system path or env (for `mind review --policy <path>`). Returns the parsed
/// [`Policy`] on success; parse/unknown-key errors and validation failures
/// surface as [`MindError`] so review can render them.
pub fn load_file(path: &Path) -> Result<Policy> {
    let text = std::fs::read_to_string(path).map_err(|e| MindError::io(path, e))?;
    parse_str(&text, path)
}

/// Parse policy TOML text and validate it, attributing errors to `path`. Factored
/// out so tests can feed strings without temp files.
fn parse_str(text: &str, path: &Path) -> Result<Policy> {
    let raw: RawPolicy = toml::from_str(text).map_err(|e| MindError::InvalidPolicy {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let auto_meld = raw
        .sources
        .auto_meld
        .into_iter()
        .map(|am| am.into_auto_meld(path))
        .collect::<Result<Vec<_>>>()?;
    let policy = Policy {
        allow: raw.sources.allow,
        lock: raw.sources.lock,
        pinned: raw.sources.pinned,
        auto_meld,
        lobes_lock: raw.lobes.lock,
        lobes_targets: raw.lobes.targets,
    };
    policy.validate_at(path)?;
    Ok(policy)
}

/// Match `identity` against an `allow` pattern where `*` matches within a single
/// `/`-separated segment and does not cross a `/` (POL-10). Segments must align
/// one-to-one; each pattern segment is matched against the corresponding identity
/// segment with `*` as a per-segment wildcard.
fn glob_match(pattern: &str, identity: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let id: Vec<&str> = identity.split('/').collect();
    if pat.len() != id.len() {
        return false;
    }
    pat.iter().zip(id.iter()).all(|(p, s)| segment_match(p, s))
}

/// Match one segment with `*` as a wildcard for any run of characters within the
/// segment (it cannot match a `/`, since segments are already split on `/`).
fn segment_match(pattern: &str, segment: &str) -> bool {
    // Standard two-pointer glob with backtracking, supporting only `*`.
    let pat: Vec<char> = pattern.chars().collect();
    let seg: Vec<char> = segment.chars().collect();
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while si < seg.len() {
        if pi < pat.len() && pat[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if pi < pat.len() && pat[pi] == seg[si] {
            pi += 1;
            si += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Policy> {
        parse_str(text, Path::new("test-policy.toml"))
    }

    // POL-1/POL-2: the system path is authoritative when present (env ignored);
    // the env var is honored only when no system file exists; neither => None.
    // spec: POL-1
    // spec: POL-2
    #[test]
    fn locate_with_precedence() {
        let system = Path::new("/etc/mind/policy.toml");
        let env = Path::new("/tmp/custom-policy.toml");

        // System present => authoritative, env ignored.
        assert_eq!(
            locate_with(Some(system), Some(env)),
            Some(system.to_path_buf()),
            "system file is authoritative; env must be ignored"
        );
        // System absent + env set => env used.
        assert_eq!(
            locate_with(None, Some(env)),
            Some(env.to_path_buf()),
            "env is honored only when no system file exists"
        );
        // System present, no env => system.
        assert_eq!(locate_with(Some(system), None), Some(system.to_path_buf()));
        // Neither => None.
        assert_eq!(locate_with(None, None), None);
    }

    // POL-4: with no policy configured, the feature is inert. Through the locate
    // seam, "no system file and no env" yields None, which load() maps to
    // Ok(None) (unmanaged).
    // spec: POL-4
    #[test]
    fn no_policy_is_inert() {
        assert_eq!(locate_with(None, None), None);
    }

    // POL-5: a TOML with an unknown key is a hard error (fail closed).
    // spec: POL-5
    #[test]
    fn unknown_key_is_error() {
        let err = parse("[sources]\nallowed = []\n").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "got {err:?}"
        );
    }

    // POL-5: a malformed table is a hard error.
    // spec: POL-5
    #[test]
    fn malformed_toml_is_error() {
        let err = parse("[sources\nallow = [").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "got {err:?}"
        );
    }

    // POL-5: an auto_meld entry with two pins is invalid.
    // spec: POL-5
    #[test]
    fn two_pins_is_error() {
        let text = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "v1"
ref = "abc123"
"#;
        let err = parse(text).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(reason.contains("more than one"), "reason: {reason}");
                assert!(
                    reason.contains("github.com/acme/a"),
                    "names entry: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    // POL-10: `*` globs within a segment but does not cross `/`; exact matches and
    // non-matches behave as expected.
    // spec: POL-10
    #[test]
    fn allow_matches_segment_globbing() {
        let text = r#"
[sources]
allow = ["github.com/acme/*", "github.example.com/platform/agents"]
"#;
        let p = parse(text).unwrap();

        // `*` matches any repo under acme.
        assert!(p.allow_matches("github.com/acme/repo"));
        assert!(p.allow_matches("github.com/acme/agent-baseline"));
        // Exact pattern matches exactly.
        assert!(p.allow_matches("github.example.com/platform/agents"));

        // `*` does not cross `/`: an extra segment is not covered.
        assert!(!p.allow_matches("github.com/acme/group/repo"));
        // Different owner is not covered.
        assert!(!p.allow_matches("github.com/other/repo"));
        // Different host is not covered.
        assert!(!p.allow_matches("gitlab.com/acme/repo"));
        // The exact pattern does not match a sibling repo.
        assert!(!p.allow_matches("github.example.com/platform/other"));
    }

    // POL-10: a partial-segment `*` matches within the segment only.
    // spec: POL-10
    #[test]
    fn allow_matches_partial_segment() {
        let text = r#"
[sources]
allow = ["github.com/acme/agent-*"]
"#;
        let p = parse(text).unwrap();
        assert!(p.allow_matches("github.com/acme/agent-baseline"));
        assert!(p.allow_matches("github.com/acme/agent-"));
        assert!(!p.allow_matches("github.com/acme/baseline"));
        // The `*` cannot swallow a `/` to reach into another segment.
        assert!(!p.allow_matches("github.com/acme/agent-x/y"));
    }

    // POL-21: with pinned = true, every auto_meld entry must declare a tag or ref;
    // an unpinned or follow_branch entry makes the policy invalid.
    // spec: POL-21
    #[test]
    fn pinned_requires_tag_or_ref() {
        // Unpinned (default branch) entry is rejected.
        let unpinned = r#"
[sources]
pinned = true
[[sources.auto_meld]]
repo = "github.com/acme/a"
"#;
        let err = parse(unpinned).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(reason.contains("github.com/acme/a"), "reason: {reason}");
                assert!(reason.contains("tag or ref"), "reason: {reason}");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // follow_branch is also rejected under pinned.
        let branch = r#"
[sources]
pinned = true
[[sources.auto_meld]]
repo = "github.com/acme/a"
follow_branch = "main"
"#;
        assert!(matches!(
            parse(branch).unwrap_err(),
            MindError::InvalidPolicy { .. }
        ));

        // All entries pinned (tag/ref) => accepted.
        let ok = r#"
[sources]
pinned = true
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "v1.0.0"
[[sources.auto_meld]]
repo = "github.com/acme/b"
ref = "9f3a1c2e"
"#;
        assert!(parse(ok).is_ok());
    }

    // POL-30: auto_meld parses repo plus each pin variant into AutoMeld correctly.
    // spec: POL-30
    #[test]
    fn auto_meld_parses_each_pin_variant() {
        let text = r#"
[[sources.auto_meld]]
repo = "github.com/acme/tagged"
tag = "v1.4.0"

[[sources.auto_meld]]
repo = "github.com/acme/reffed"
ref = "9f3a1c2e"

[[sources.auto_meld]]
repo = "github.com/acme/branched"
follow_branch = "release"

[[sources.auto_meld]]
repo = "github.com/acme/floating"
"#;
        let p = parse(text).unwrap();
        let am = p.auto_meld();
        assert_eq!(am.len(), 4);
        assert_eq!(
            am[0],
            AutoMeld {
                repo: "github.com/acme/tagged".into(),
                pin: Pin::Tag("v1.4.0".into()),
            }
        );
        assert_eq!(
            am[1],
            AutoMeld {
                repo: "github.com/acme/reffed".into(),
                pin: Pin::Ref("9f3a1c2e".into()),
            }
        );
        assert_eq!(
            am[2],
            AutoMeld {
                repo: "github.com/acme/branched".into(),
                pin: Pin::FollowBranch("release".into()),
            }
        );
        assert_eq!(
            am[3],
            AutoMeld {
                repo: "github.com/acme/floating".into(),
                pin: Pin::DefaultBranch,
            }
        );
    }

    // POL-31: with lock = true, every auto_meld repo must satisfy allow, matched
    // on its parsed `host/owner/repo` identity (so a shorthand spec validates
    // against a host-qualified pattern). One outside is invalid; an all-matching
    // set is accepted.
    // spec: POL-31
    #[test]
    fn lock_requires_auto_meld_in_allow() {
        let outside = r#"
[sources]
lock = true
allow = ["github.com/acme/*"]
[[sources.auto_meld]]
repo = "other/x"
"#;
        let err = parse(outside).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(reason.contains("other/x"), "reason: {reason}");
                assert!(reason.contains("allow"), "reason: {reason}");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // A shorthand spec (`acme/baseline`) parses to identity
        // `github.com/acme/baseline`, which matches the host-qualified pattern.
        let inside = r#"
[sources]
lock = true
allow = ["github.com/acme/*"]
[[sources.auto_meld]]
repo = "acme/baseline"
"#;
        assert!(parse(inside).is_ok());
    }

    // POL-4: an empty/minimal policy parses, validates, and reports all controls
    // off by default.
    // spec: POL-4
    #[test]
    fn empty_policy_has_controls_off() {
        let p = parse("").unwrap();
        assert!(!p.lock());
        assert!(!p.pinned());
        assert!(!p.lobes_lock());
        assert!(p.auto_meld().is_empty());
        assert!(p.lobes_targets().is_empty());
        assert!(p.allow().is_empty());
    }

    // load_file with a real temp file round-trips through the filesystem seam.
    // spec: POL-30
    #[test]
    fn load_file_reads_a_real_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mind-policy-test-{}.toml", std::process::id()));
        std::fs::write(&path, "[lobes]\nlock = true\ntargets = [\"~/.claude\"]\n").unwrap();
        let p = load_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(p.lobes_lock());
        assert_eq!(p.lobes_targets(), &["~/.claude".to_string()]);
    }

    // ---- gap-closing tests ------------------------------------------------

    // POL-1/POL-2: the locate seam is the only pure surface for the system/env
    // precedence. The real `load()` consults the actual per-OS system path
    // (`/etc`, `%PROGRAMDATA%`) and the process environment, neither of which is
    // safely manipulable from a hermetic unit test. `locate_with` takes both
    // inputs explicitly and contains the entire precedence decision (load() does
    // nothing but compute existence and forward to it), so exercising every
    // combination here is a complete test of the precedence behavior. Confirming
    // the seam is sufficient: load() adds only `Path::exists` + a read, which the
    // load_file/parse tests already cover.
    // spec: POL-1
    // spec: POL-2
    #[test]
    fn locate_with_is_the_complete_precedence_surface() {
        let system = Path::new("/etc/mind/policy.toml");
        let env = Path::new("/tmp/x.toml");
        // Exhaustive truth table over (system?, env?).
        assert_eq!(locate_with(None, None), None);
        assert_eq!(locate_with(None, Some(env)), Some(env.to_path_buf()));
        assert_eq!(locate_with(Some(system), None), Some(system.to_path_buf()));
        // Both present: system wins, env never consulted.
        assert_eq!(
            locate_with(Some(system), Some(env)),
            Some(system.to_path_buf())
        );
    }

    // POL-10: `glob_match` directly, asserting `*` never crosses a `/` and that
    // segment counts must align. These hit the matcher at its own seam so the
    // boundary cases are unambiguous about which layer rejects them.
    // spec: POL-10
    #[test]
    fn glob_match_segment_count_alignment() {
        // More pattern segments than identity segments => no match.
        assert!(!glob_match("a/b/c", "a/b"));
        // Fewer pattern segments than identity segments => no match.
        assert!(!glob_match("a/b", "a/b/c"));
        // A lone `*` is a single segment and cannot span the whole identity.
        assert!(!glob_match("*", "a/b"));
        assert!(glob_match("*", "anything"));
        // Equal segment counts with per-segment wildcards.
        assert!(glob_match("*/*/*", "github.com/acme/repo"));
        assert!(!glob_match("*/*", "github.com/acme/repo"));
        // A `*` segment never swallows a `/`.
        assert!(!glob_match("github.com/*", "github.com/acme/repo"));
    }

    // POL-10: `segment_match` edge cases - leading/trailing `*`, multiple `*` in
    // one segment, and empty pattern vs empty segment. These are the boundaries
    // the backtracking matcher is most likely to mishandle.
    // spec: POL-10
    #[test]
    fn segment_match_wildcard_edges() {
        // Empty pattern matches only the empty segment.
        assert!(segment_match("", ""));
        assert!(!segment_match("", "x"));
        // A lone `*` matches anything including empty.
        assert!(segment_match("*", ""));
        assert!(segment_match("*", "anything"));
        // Leading `*`.
        assert!(segment_match("*baseline", "agent-baseline"));
        assert!(segment_match("*baseline", "baseline"));
        assert!(!segment_match("*baseline", "baseline-x"));
        // Trailing `*`.
        assert!(segment_match("agent-*", "agent-baseline"));
        assert!(segment_match("agent-*", "agent-"));
        assert!(!segment_match("agent-*", "baseline"));
        // Multiple `*` in one segment, with required literals between/around.
        assert!(segment_match("a*b*c", "axbyc"));
        assert!(segment_match("a*b*c", "abc"));
        assert!(segment_match("*a*", "xay"));
        assert!(!segment_match("a*b*c", "axby"));
        // Adjacent stars collapse and still anchor the trailing literal.
        assert!(segment_match("a**c", "axyzc"));
        assert!(!segment_match("a**c", "axyz"));
        // A literal that cannot be satisfied even with backtracking.
        assert!(!segment_match("*z", "abc"));
    }

    // POL-10: a `*` wildcard matches a multibyte Unicode run within a segment,
    // and a multibyte literal must match exactly. Guards against any byte- vs
    // char-indexing assumption in the matcher.
    // spec: POL-10
    #[test]
    fn segment_match_unicode() {
        assert!(segment_match("café-*", "café-baseline"));
        assert!(segment_match("*-café", "acme-café"));
        assert!(segment_match("*", "日本語"));
        assert!(segment_match("日*語", "日本語"));
        assert!(!segment_match("café", "cafe"));
        // Through the public allow path with a Unicode owner segment.
        let p = parse("[sources]\nallow = [\"github.com/café/*\"]\n").unwrap();
        assert!(p.allow_matches("github.com/café/repo"));
        assert!(!p.allow_matches("github.com/cafe/repo"));
    }

    // POL-5: deny_unknown_fields must fire on EACH nested table, not just the
    // top level. An unknown key under [sources], under [lobes], and under an
    // [[sources.auto_meld]] entry each fail closed.
    // spec: POL-5
    #[test]
    fn deny_unknown_fields_on_each_table() {
        // Top-level unknown table.
        assert!(matches!(
            parse("[bogus]\nx = 1\n").unwrap_err(),
            MindError::InvalidPolicy { .. }
        ));
        // Unknown key under [sources].
        assert!(matches!(
            parse("[sources]\nalloww = []\n").unwrap_err(),
            MindError::InvalidPolicy { .. }
        ));
        // Unknown key under [lobes].
        assert!(matches!(
            parse("[lobes]\nlocked = true\n").unwrap_err(),
            MindError::InvalidPolicy { .. }
        ));
        // Unknown key under an [[sources.auto_meld]] entry.
        let am = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
branch = "main"
"#;
        assert!(matches!(
            parse(am).unwrap_err(),
            MindError::InvalidPolicy { .. }
        ));
    }

    // POL-21/POL-31: when both pinned and lock are true and an entry violates
    // both invariants, validate runs pinned first and returns the deterministic,
    // clearly-named pinned error (not the lock one). The error is stable across
    // runs (no ordering nondeterminism).
    // spec: POL-21
    // spec: POL-31
    #[test]
    fn pinned_and_lock_violation_is_deterministic() {
        // `other/x` is both unpinned (POL-21) and outside allow (POL-31).
        let text = r#"
[sources]
pinned = true
lock = true
allow = ["github.com/acme/*"]
[[sources.auto_meld]]
repo = "github.com/other/x"
"#;
        // Run repeatedly to catch any nondeterminism.
        for _ in 0..5 {
            match parse(text).unwrap_err() {
                MindError::InvalidPolicy { reason, .. } => {
                    assert!(reason.contains("github.com/other/x"), "reason: {reason}");
                    // Pinned check is evaluated first, so its message wins.
                    assert!(
                        reason.contains("tag or ref"),
                        "expected the pinned error to win deterministically: {reason}"
                    );
                    assert!(
                        reason.contains("pinned"),
                        "names the offending control: {reason}"
                    );
                }
                other => panic!("expected InvalidPolicy, got {other:?}"),
            }
        }
    }

    // POL-21/POL-31: pinned + lock together with NO auto_meld entries is a valid
    // policy (the invariants are vacuously satisfied).
    // spec: POL-21
    // spec: POL-31
    #[test]
    fn pinned_and_lock_with_no_auto_meld_is_valid() {
        let text = r#"
[sources]
pinned = true
lock = true
allow = ["github.com/acme/*"]
"#;
        let p = parse(text).unwrap();
        assert!(p.pinned());
        assert!(p.lock());
        assert!(p.auto_meld().is_empty());
        // validate() (the public, system-path-attributed entry point) also passes.
        assert!(p.validate().is_ok());
    }

    // POL-5: InvalidPolicy attributes the failure to the offending file for both
    // parse-time (unknown key) and validate-time (invariant) failures.
    // spec: POL-5
    #[test]
    fn invalid_policy_names_the_file() {
        let path = Path::new("/org/policies/acme.toml");
        // Parse failure (unknown key) carries the path.
        match parse_str("[sources]\nnope = 1\n", path).unwrap_err() {
            MindError::InvalidPolicy { path: p, .. } => {
                assert_eq!(p, "/org/policies/acme.toml");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
        // Validate failure (pinned invariant) carries the same path.
        let unpinned = r#"
[sources]
pinned = true
[[sources.auto_meld]]
repo = "github.com/acme/a"
"#;
        match parse_str(unpinned, path).unwrap_err() {
            MindError::InvalidPolicy { path: p, reason } => {
                assert_eq!(p, "/org/policies/acme.toml");
                assert!(reason.contains("github.com/acme/a"), "reason: {reason}");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
        // The two-pins parse-time normalization error is likewise attributed.
        let two = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "v1"
ref = "abc"
"#;
        match parse_str(two, path).unwrap_err() {
            MindError::InvalidPolicy { path: p, .. } => {
                assert_eq!(p, "/org/policies/acme.toml");
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    // POL-30: a policy mixing every pin shape round-trips so each maps to the
    // right Pin, and the entries preserve declaration order. Complements
    // auto_meld_parses_each_pin_variant by mixing the variants and asserting
    // order-sensitive identity.
    // spec: POL-30
    #[test]
    fn auto_meld_mixed_pins_round_trip_in_order() {
        let text = r#"
[[sources.auto_meld]]
repo = "github.com/acme/floating"

[[sources.auto_meld]]
repo = "github.com/acme/branched"
follow_branch = "release"

[[sources.auto_meld]]
repo = "github.com/acme/reffed"
ref = "9f3a1c2e"

[[sources.auto_meld]]
repo = "github.com/acme/tagged"
tag = "v2.0.0"
"#;
        let p = parse(text).unwrap();
        let expected = vec![
            AutoMeld {
                repo: "github.com/acme/floating".into(),
                pin: Pin::DefaultBranch,
            },
            AutoMeld {
                repo: "github.com/acme/branched".into(),
                pin: Pin::FollowBranch("release".into()),
            },
            AutoMeld {
                repo: "github.com/acme/reffed".into(),
                pin: Pin::Ref("9f3a1c2e".into()),
            },
            AutoMeld {
                repo: "github.com/acme/tagged".into(),
                pin: Pin::Tag("v2.0.0".into()),
            },
        ];
        assert_eq!(p.auto_meld(), expected.as_slice());
    }

    // error.rs: smoke-test the Display formatting of the policy error variants so
    // a change to an `#[error(...)]` template is caught. InvalidPolicy is the
    // policy-core error (POL-5 fail-closed message); SourceNotAllowed and
    // UnpinnedSourceForbidden have no core POL ID in this shard (their enforcement
    // is POL-11/POL-20, owned by sibling shards), so they are asserted without a
    // spec cite.
    // spec: POL-5
    #[test]
    fn invalid_policy_display() {
        let e = MindError::InvalidPolicy {
            path: "/etc/mind/policy.toml".into(),
            reason: "auto_meld entry 'x' must declare a tag or ref".into(),
        };
        assert_eq!(
            e.to_string(),
            "invalid managed policy at /etc/mind/policy.toml: \
             auto_meld entry 'x' must declare a tag or ref"
        );
    }

    #[test]
    fn enforcement_error_display() {
        let not_allowed = MindError::SourceNotAllowed {
            identity: "gitlab.com/x/y".into(),
        };
        assert_eq!(
            not_allowed.to_string(),
            "source 'gitlab.com/x/y' is not permitted by the managed policy's allowlist"
        );

        let unpinned = MindError::UnpinnedSourceForbidden {
            identity: "github.com/acme/a".into(),
        };
        assert_eq!(
            unpinned.to_string(),
            "source 'github.com/acme/a' must be pinned to a tag or ref: \
             the managed policy forbids floating branches"
        );
    }
}
