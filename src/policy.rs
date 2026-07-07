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
//! Enforcement (`meld`/`sync`/`upgrade`/`config lobes`/`review`) consumes the
//! public API here. A few accessors are retained for completeness and tests beyond
//! what the call sites use, so the allow keeps the API surface warning-free.
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
    /// Install every item the source offers after successful provisioning (POL-58).
    /// When false (the default), `sync` registers the source only.
    pub install: bool,
    /// Run build hooks during the headless install pass (POL-59). Only meaningful
    /// when `install = true`; ignored otherwise.
    pub run_build_hooks: bool,
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
    /// `[sources].allow-local`: whether local-path and `file://` melds are
    /// permitted under a lock (POL-56/57). Defaults to `true` (preserves
    /// existing behavior).
    allow_local: bool,
    /// `[[sources.auto_meld]]` base set (POL-30).
    auto_meld: Vec<AutoMeld>,
    /// `[lobes].lock`: pin the effective agent homes to `lobes_targets` (POL-40).
    lobes_lock: bool,
    /// `[lobes].targets`: policy-provided agent homes (POL-40/41).
    lobes_targets: Vec<String>,
    /// `[binary].self-update`: control over `mind evolve` (POL-51..54).
    self_update: SelfUpdateControl,
}

// --- TOML wire shape --------------------------------------------------------
//
// The parse is two-phase (POL-61):
//   1. A permissive probe (`RawPolicyProbe`) reads only `min-mind-version`
//      while silently ignoring all other keys. The version gate runs here.
//   2. The strict parse (`RawPolicy`, `deny_unknown_fields`) validates every
//      known key and rejects unknowns (POL-5).
// This ordering ensures a future-schema policy with `min-mind-version` set
// gives a clear "upgrade mind" error (POL-62) instead of an opaque
// "unknown field" error on whatever new key triggered the old binary.

/// Phase-1 probe: reads only `min-mind-version`; ignores all other keys
/// (NO `deny_unknown_fields`). Used to check the schema-version gate before
/// the strict parse fires (POL-61).
#[derive(Debug, Default, Deserialize)]
struct RawPolicyProbe {
    #[serde(default, rename = "min-mind-version")]
    min_mind_version: Option<String>,
}

/// The policy decision for `mind evolve` / `self-update` (POL-51..54).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfUpdateControl {
    /// `evolve` is allowed to any version (default when the key is absent or true).
    Allowed,
    /// `evolve` is disabled entirely; `--check` is also gated (POL-52).
    Disabled,
    /// `evolve` may only target this pinned version (behaves as `--version <pin>`).
    Pinned(String),
}

/// Raw deserialization shim for `[binary].self-update`, which accepts a bool OR
/// a version string (TOML's type-tagged values require an untagged enum).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSelfUpdate {
    Bool(bool),
    Pin(String),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinary {
    #[serde(default, rename = "self-update")]
    self_update: Option<RawSelfUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    /// Recognized by the strict parse so it is not rejected as an unknown key
    /// (POL-61). The actual version comparison runs in the phase-1 probe before
    /// this struct is ever populated.
    #[serde(default, rename = "min-mind-version")]
    min_mind_version: Option<String>,
    #[serde(default)]
    sources: RawSources,
    #[serde(default)]
    lobes: RawLobes,
    #[serde(default)]
    binary: RawBinary,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSources {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    lock: bool,
    #[serde(default)]
    pinned: bool,
    /// `allow-local = false` forbids local-path and `file://` melds under lock
    /// regardless of `allow` patterns (POL-56). Defaults to `true` so that
    /// existing policies without this key continue to work unchanged.
    #[serde(default = "default_true", rename = "allow-local")]
    allow_local: bool,
    #[serde(default)]
    auto_meld: Vec<RawAutoMeld>,
}

impl Default for RawSources {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            lock: false,
            pinned: false,
            allow_local: true,
            auto_meld: Vec::new(),
        }
    }
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
    /// Install all items from the source after successful provisioning (POL-58).
    #[serde(default)]
    install: bool,
    /// Run build hooks during the headless install pass (POL-59).
    #[serde(default, rename = "run-build-hooks")]
    run_build_hooks: bool,
}

impl RawAutoMeld {
    /// Normalize the at-most-one-pin TOML shape into the shared [`Pin`] enum.
    /// More than one of `tag`/`ref`/`follow_branch` is invalid (POL-5). Each
    /// value is validated by [`crate::git::validate_ref_value`] (DSC-66 /
    /// POL-33) before constructing the [`Pin`], so hostile values such as
    /// `--upload-pack=...` are rejected as an invalid policy.
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

        /// Validate a pin value, mapping `MindError::InvalidRef` to
        /// `MindError::InvalidPolicy` so the error is attributed to the policy
        /// file rather than to a generic ref parsing context.
        fn validate_pin(value: &str, repo: &str, path: &Path) -> Result<()> {
            // spec: POL-33
            crate::git::validate_ref_value(value).map_err(|e| MindError::InvalidPolicy {
                path: path.display().to_string(),
                reason: format!(
                    "auto_meld entry '{}' has an invalid pin value {:?}: {}",
                    repo, value, e
                ),
            })
        }

        let pin = if let Some(tag) = self.tag {
            validate_pin(&tag, &self.repo, path)?;
            Pin::Tag(tag)
        } else if let Some(r) = self.ref_ {
            validate_pin(&r, &self.repo, path)?;
            Pin::Ref(r)
        } else if let Some(branch) = self.follow_branch {
            validate_pin(&branch, &self.repo, path)?;
            Pin::FollowBranch(branch)
        } else {
            Pin::DefaultBranch
        };
        Ok(AutoMeld {
            repo: self.repo,
            pin,
            install: self.install,
            run_build_hooks: self.run_build_hooks,
        })
    }
}

impl Policy {
    /// Load the managed policy, or `None` when none is configured (POL-4 inert).
    ///
    /// Reads the fixed per-OS system path; falls back to `$MIND_POLICY_FILE` only
    /// when no system file exists (POL-1, POL-2). A parse error, unknown key, or
    /// failed [`validate`](Policy::validate) is `Err` (POL-5 fail closed).
    ///
    /// When the policy is loaded from the real system path, emits warnings to
    /// stderr if the file or its parent directory is not securely owned/permissioned
    /// (POL-64). The check is skipped for `$MIND_POLICY_FILE` paths (POL-65).
    pub fn load() -> Result<Option<Policy>> {
        let env = std::env::var_os(ENV_VAR).map(PathBuf::from);
        let system = system_path();
        match locate_existing(&system, env.as_deref()) {
            Some(path) => {
                // spec: POL-65 -- permission check only for the real system path.
                let is_system = path == system;
                let (policy, warnings) = load_and_check(&path, is_system)?;
                for w in &warnings {
                    eprintln!("{w}");
                }
                Ok(Some(policy))
            }
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
    ///
    /// The production path is [`validate_at`](Policy::validate_at), called by
    /// `load_file` -> `parse_str` with the real file path so errors cite the
    /// actual file. This convenience wrapper attributes errors to the system
    /// policy path (`system_path()`) and is intended for tests only.
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
        // POL-40: every [lobes].targets entry must be a non-empty, non-whitespace
        // path. An empty string silently resolves to the current working directory
        // in Paths::agent_homes, which would link managed homes into wherever `mind`
        // runs. Fail closed at load time so the problem is never silently acted on.
        for target in &self.lobes_targets {
            if target.trim().is_empty() {
                return Err(MindError::InvalidPolicy {
                    path: path.display().to_string(),
                    reason: "[lobes].targets contains an empty or whitespace-only entry; \
                             each target must be a non-empty path"
                        .to_string(),
                });
            }
        }
        Ok(())
    }

    /// `[sources].lock` (POL-11/13).
    pub fn lock(&self) -> bool {
        self.lock
    }

    /// `[sources].allow-local` (POL-56/57). Defaults to `true`.
    pub fn allow_local(&self) -> bool {
        self.allow_local
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

    /// `[binary].self-update` control for `mind evolve` (POL-51..54).
    pub fn self_update_control(&self) -> &SelfUpdateControl {
        &self.self_update
    }
}

// --- Permission check (POL-64/POL-65) ---------------------------------------
//
// Returns the reasons why a path's ownership/mode is insecure.
// Pure: accepts (mode, uid) so unit tests pass known values without touching
// the filesystem. The `is_parent` flag tweaks the message wording and the
// suggested chmod value.
#[cfg(unix)]
fn policy_path_security_warnings(
    path_display: &str,
    mode: u32,
    uid: u32,
    is_parent: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let (kind, fix) = if is_parent {
        (
            "parent directory of managed policy",
            "chown root and chmod 755",
        )
    } else {
        ("managed policy", "chown root and chmod 644")
    };
    if mode & 0o022 != 0 {
        out.push(format!(
            "warning: {kind} {path_display} is group/world-writable; \
             a local user could alter enforced policy. {fix}."
        ));
    }
    if uid != 0 {
        out.push(format!(
            "warning: {kind} {path_display} is not root-owned (uid {uid}); \
             a local user could alter enforced policy. {fix}."
        ));
    }
    out
}

/// Check ownership and mode of `path` and its parent directory (POL-64).
/// Returns warning strings; empty when all checks pass or on non-unix.
/// Only called for the real system policy path (POL-65 skip for env path).
#[cfg(unix)]
fn check_policy_file_permissions(path: &Path) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    if let Ok(meta) = std::fs::metadata(path) {
        out.extend(policy_path_security_warnings(
            &path.display().to_string(),
            meta.mode(),
            meta.uid(),
            false,
        ));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(meta) = std::fs::metadata(parent) {
            out.extend(policy_path_security_warnings(
                &parent.display().to_string(),
                meta.mode(),
                meta.uid(),
                true,
            ));
        }
    }
    out
}

#[cfg(not(unix))]
fn check_policy_file_permissions(_path: &Path) -> Vec<String> {
    Vec::new()
}

/// Internal load used by `Policy::load`, with the `is_system_path` flag that
/// gates the permission check (POL-64/POL-65). Factored out so tests can verify
/// the check is skipped for the `$MIND_POLICY_FILE` path without needing to
/// create a real `/etc/mind/policy.toml`.
fn load_and_check(path: &Path, is_system_path: bool) -> Result<(Policy, Vec<String>)> {
    let policy = load_file(path)?;
    let warnings = if is_system_path {
        check_policy_file_permissions(path)
    } else {
        Vec::new()
    };
    Ok((policy, warnings))
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

/// Resolve the effective policy path, honoring only files that exist on disk: the
/// system path when present (POL-1), else `$MIND_POLICY_FILE` when present (POL-2).
/// A set-but-missing env path is treated as no policy (POL-4 inert), not a hard
/// error, mirroring the system-path existence check. Pure over its arguments.
fn locate_existing(system: &Path, env: Option<&Path>) -> Option<PathBuf> {
    let system = system.exists().then(|| system.to_path_buf());
    let env = env.filter(|p| p.exists());
    locate_with(system.as_deref(), env)
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
    // spec: POL-61 -- phase-1 probe: permissive parse to extract min-mind-version
    // before the strict deny_unknown_fields parse. This ensures that a policy
    // written for a newer schema gives a clear "upgrade mind" error (POL-62) on
    // an old binary instead of an opaque "unknown field" error.
    //
    // unwrap_or_default: if the TOML is syntactically malformed the probe yields
    // no version, and the strict parse below reports the real parse error.
    let probe: RawPolicyProbe = toml::from_str(text).unwrap_or_default();
    if let Some(v) = &probe.min_mind_version {
        // spec: POL-63 -- invalid version string fails closed at policy parse.
        if !crate::mindfile::is_plausible_version(v) {
            return Err(MindError::InvalidPolicy {
                path: path.display().to_string(),
                reason: format!(
                    "min-mind-version {v:?} is not a valid version string \
                     (expected dotted numeric, e.g. \"0.15.0\")"
                ),
            });
        }
        // spec: POL-62 -- version gate: give a clear error instead of an opaque
        // "unknown field" when the policy was written for a newer binary.
        let running = env!("CARGO_PKG_VERSION");
        if !crate::mindfile::version_at_least(running, v) {
            return Err(MindError::InvalidPolicy {
                path: path.display().to_string(),
                reason: format!(
                    "managed policy requires mind >= {v}, running {running}; upgrade mind"
                ),
            });
        }
    }

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

    // Resolve [binary].self-update into the typed control enum.
    // A pinned version is stripped of a leading 'v' and validated (POL-53/POL-5).
    let self_update = match raw.binary.self_update {
        None | Some(RawSelfUpdate::Bool(true)) => SelfUpdateControl::Allowed,
        Some(RawSelfUpdate::Bool(false)) => SelfUpdateControl::Disabled,
        Some(RawSelfUpdate::Pin(v)) => {
            let v = v.strip_prefix('v').unwrap_or(&v).to_string();
            if !crate::mindfile::is_plausible_version(&v) {
                return Err(MindError::InvalidPolicy {
                    path: path.display().to_string(),
                    reason: format!(
                        "[binary].self-update pin {:?} is not a valid version string \
                         (expected dotted numeric, e.g. \"0.14.0\")",
                        v
                    ),
                });
            }
            SelfUpdateControl::Pinned(v)
        }
    };

    let policy = Policy {
        allow: raw.sources.allow,
        lock: raw.sources.lock,
        pinned: raw.sources.pinned,
        allow_local: raw.sources.allow_local,
        auto_meld,
        lobes_lock: raw.lobes.lock,
        lobes_targets: raw.lobes.targets,
        self_update,
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
                install: false,
                run_build_hooks: false,
            }
        );
        assert_eq!(
            am[1],
            AutoMeld {
                repo: "github.com/acme/reffed".into(),
                pin: Pin::Ref("9f3a1c2e".into()),
                install: false,
                run_build_hooks: false,
            }
        );
        assert_eq!(
            am[2],
            AutoMeld {
                repo: "github.com/acme/branched".into(),
                pin: Pin::FollowBranch("release".into()),
                install: false,
                run_build_hooks: false,
            }
        );
        assert_eq!(
            am[3],
            AutoMeld {
                repo: "github.com/acme/floating".into(),
                pin: Pin::DefaultBranch,
                install: false,
                run_build_hooks: false,
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
        // POL-51: absent [binary].self-update defaults to Allowed.
        assert_eq!(p.self_update_control(), &SelfUpdateControl::Allowed);
        // POL-57: absent allow-local defaults to true (existing behavior preserved).
        assert!(p.allow_local());
    }

    // POL-56: allow-local = false parses and the accessor reflects it.
    // POL-57: allow-local = true (explicit) is the same as absent (true).
    // spec: POL-56
    // spec: POL-57
    #[test]
    fn allow_local_parses_correctly() {
        // Explicit false.
        let p = parse("[sources]\nallow-local = false\n").unwrap();
        assert!(!p.allow_local());

        // Explicit true.
        let p = parse("[sources]\nallow-local = true\n").unwrap();
        assert!(p.allow_local());

        // Absent defaults to true.
        let p = parse("[sources]\nlock = true\n").unwrap();
        assert!(p.allow_local());
    }

    // POL-51/POL-54: [binary].self-update absent or true -> Allowed.
    // spec: POL-51
    // spec: POL-54
    #[test]
    fn binary_self_update_absent_or_true_is_allowed() {
        // Absent [binary] table.
        let p = parse("").unwrap();
        assert_eq!(p.self_update_control(), &SelfUpdateControl::Allowed);

        // Explicit true.
        let p = parse("[binary]\nself-update = true\n").unwrap();
        assert_eq!(p.self_update_control(), &SelfUpdateControl::Allowed);
    }

    // POL-52: [binary].self-update = false disables evolve.
    // spec: POL-52
    #[test]
    fn binary_self_update_false_is_disabled() {
        let p = parse("[binary]\nself-update = false\n").unwrap();
        assert_eq!(p.self_update_control(), &SelfUpdateControl::Disabled);
    }

    // POL-53: [binary].self-update = "<version>" pins evolve to that version.
    // spec: POL-53
    #[test]
    fn binary_self_update_version_string_is_pinned() {
        let p = parse("[binary]\nself-update = \"0.14.0\"\n").unwrap();
        assert_eq!(
            p.self_update_control(),
            &SelfUpdateControl::Pinned("0.14.0".to_string())
        );

        // Leading 'v' is stripped.
        let p = parse("[binary]\nself-update = \"v1.2.3\"\n").unwrap();
        assert_eq!(
            p.self_update_control(),
            &SelfUpdateControl::Pinned("1.2.3".to_string())
        );
    }

    // POL-53/POL-5: an invalid version string in [binary].self-update is a
    // hard parse error (fail closed).
    // spec: POL-53
    // spec: POL-5
    #[test]
    fn binary_self_update_invalid_version_string_is_error() {
        // Non-numeric component.
        let err = parse("[binary]\nself-update = \"not-a-version\"\n").unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("not a valid version string"),
                    "reason: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // Empty string.
        let err = parse("[binary]\nself-update = \"\"\n").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "empty version must be invalid: {err:?}"
        );

        // Trailing dot.
        let err = parse("[binary]\nself-update = \"1.2.\"\n").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "trailing dot must be invalid: {err:?}"
        );
    }

    // POL-5: [binary] with an unknown key is rejected (deny_unknown_fields).
    // spec: POL-5
    #[test]
    fn binary_unknown_key_is_error() {
        let err = parse("[binary]\nauto-update = true\n").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "unknown [binary] key must be rejected: {err:?}"
        );
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

    // POL-2/POL-4: `locate_existing` honors only files that exist. A set-but-missing
    // `$MIND_POLICY_FILE` resolves to no policy (inert), not a hard error, so a stale
    // env path never fails a command (and never makes concurrent tests flaky).
    // Pure over real temp files; touches no process env or system path.
    // spec: POL-2
    // spec: POL-4
    #[test]
    fn locate_existing_ignores_missing_files() {
        let dir = std::env::temp_dir();
        let present = dir.join(format!("mind-policy-present-{}.toml", std::process::id()));
        std::fs::write(&present, "").unwrap();
        let missing = dir.join(format!("mind-policy-missing-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        let absent_system = dir.join(format!("mind-policy-no-system-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&absent_system);

        // A set-but-missing env path with no system file => no policy.
        assert_eq!(locate_existing(&absent_system, Some(&missing)), None);
        // An existing env path (no system file) => that path.
        assert_eq!(
            locate_existing(&absent_system, Some(&present)),
            Some(present.clone())
        );
        // An existing system path wins over the env path (POL-1/POL-2 precedence).
        assert_eq!(
            locate_existing(&present, Some(&present)),
            Some(present.clone())
        );
        // No env set, no system file => no policy.
        assert_eq!(locate_existing(&absent_system, None), None);
        std::fs::remove_file(&present).ok();
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
                install: false,
                run_build_hooks: false,
            },
            AutoMeld {
                repo: "github.com/acme/branched".into(),
                pin: Pin::FollowBranch("release".into()),
                install: false,
                run_build_hooks: false,
            },
            AutoMeld {
                repo: "github.com/acme/reffed".into(),
                pin: Pin::Ref("9f3a1c2e".into()),
                install: false,
                run_build_hooks: false,
            },
            AutoMeld {
                repo: "github.com/acme/tagged".into(),
                pin: Pin::Tag("v2.0.0".into()),
                install: false,
                run_build_hooks: false,
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

    // POL-40: a [lobes].targets entry that is empty or all-whitespace must be
    // rejected at validation time (fail closed) so an empty string never silently
    // resolves to the current working directory in Paths::agent_homes.
    // spec: POL-40
    #[test]
    fn lobes_targets_empty_entry_is_invalid() {
        // Literally empty string entry.
        let empty = r#"
[lobes]
targets = [""]
"#;
        let err = parse(empty).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("empty or whitespace"),
                    "reason should name the problem: {reason}"
                );
                assert!(
                    reason.contains("[lobes].targets"),
                    "reason should name the field: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // Whitespace-only entry.
        let whitespace = "[lobes]\ntargets = [\"   \"]\n";
        let err = parse(whitespace).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "whitespace-only target must also be rejected: {err:?}"
        );

        // Mixed list: one valid entry, one empty - still rejected.
        let mixed = "[lobes]\ntargets = [\"~/.claude\", \"\"]\n";
        let err = parse(mixed).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "mixed list with an empty entry must be rejected: {err:?}"
        );

        // A normal, non-empty target passes validation.
        let ok = "[lobes]\ntargets = [\"~/.claude\"]\n";
        let p = parse(ok).expect("non-empty target must be valid");
        assert_eq!(p.lobes_targets(), &["~/.claude".to_string()]);

        // Multiple valid targets also pass.
        let multi = "[lobes]\ntargets = [\"~/.claude\", \"/opt/agent-home\"]\n";
        let p = parse(multi).expect("multiple non-empty targets must be valid");
        assert_eq!(p.lobes_targets().len(), 2);
    }

    // POL-33: auto_meld pin values are passed through validate_ref_value (DSC-66).
    // A leading `-` (injection via option-like value), whitespace, and `..` (git
    // range syntax) are all rejected as an invalid policy.
    // spec: POL-33
    #[test]
    fn pin_values_are_validated_against_ref_rules() {
        // Leading dash is rejected (looks like a git option).
        let leading_dash = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "--upload-pack=/tmp/evil"
"#;
        let err = parse(leading_dash).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("--upload-pack=/tmp/evil"),
                    "reason should quote the hostile value: {reason}"
                );
                assert!(
                    reason.contains("github.com/acme/a"),
                    "reason should name the entry: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // Whitespace in a tag is rejected.
        let with_space = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "v1 0"
"#;
        let err = parse(with_space).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "whitespace in tag must be rejected: {err:?}"
        );

        // `..` range syntax in a ref is rejected.
        let with_range = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
ref = "HEAD..evil"
"#;
        let err = parse(with_range).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "'..' in ref must be rejected: {err:?}"
        );

        // Leading dash in follow_branch is rejected.
        let dash_branch = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
follow_branch = "-bad-branch"
"#;
        let err = parse(dash_branch).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "leading dash in follow_branch must be rejected: {err:?}"
        );

        // Empty pin value is rejected.
        let empty_tag = "[[sources.auto_meld]]\nrepo = \"github.com/acme/a\"\ntag = \"\"\n";
        let err = parse(empty_tag).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "empty tag must be rejected: {err:?}"
        );

        // Normal values are accepted.
        let ok = r#"
[[sources.auto_meld]]
repo = "github.com/acme/a"
tag = "v1.4.0"

[[sources.auto_meld]]
repo = "github.com/acme/b"
ref = "9f3a1c2edeadbeef"

[[sources.auto_meld]]
repo = "github.com/acme/c"
follow_branch = "main"
"#;
        assert!(parse(ok).is_ok(), "normal pin values must be accepted");
    }

    // ----- POL-61/62/63: min-mind-version gate --------------------------------

    // POL-62: a policy declaring min-mind-version higher than the running binary
    // returns a clear "upgrade mind" error rather than an opaque unknown-field error.
    // POL-61: the check fires before the strict deny_unknown_fields parse.
    // spec: POL-61
    // spec: POL-62
    #[test]
    fn min_mind_version_too_high_is_clear_error() {
        // Unknown key alongside min-mind-version ensures the test confirms the
        // gate fires BEFORE the strict parse would reject the unknown key.
        let text = "min-mind-version = \"999.0.0\"\n[sources]\nlock = true\n";
        let path = Path::new("test-policy.toml");
        let err = parse_str(text, path).unwrap_err();
        match err {
            MindError::InvalidPolicy { path: p, reason } => {
                assert_eq!(p, "test-policy.toml");
                assert!(
                    reason.contains("requires mind >="),
                    "must name the gate: {reason}"
                );
                assert!(
                    reason.contains("999.0.0"),
                    "must name the required version: {reason}"
                );
                assert!(
                    reason.contains("upgrade mind"),
                    "must tell the user to upgrade: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    // POL-61: the phase-1 version gate fires BEFORE the phase-2 strict
    // deny_unknown_fields parse. A policy that declares BOTH a too-high
    // min-mind-version AND an unknown key (which the strict parse would reject)
    // must surface the version error, not the unknown-field error. This is the
    // core correctness claim of the two-phase parse: an old binary reading a
    // newer-schema policy gets "upgrade mind", never an opaque unknown-field
    // error on whatever new key the newer schema introduced.
    // spec: POL-61
    // spec: POL-62
    #[test]
    fn min_mind_version_gate_beats_unknown_field_error() {
        // `[future]` is an unknown top-level table; the strict parse rejects it.
        // The version gate must win regardless.
        let text = "min-mind-version = \"999.0.0\"\n[future]\nunknown-key = 1\n";
        let err = parse_str(text, Path::new("test-policy.toml")).unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("requires mind >=") && reason.contains("999.0.0"),
                    "version gate must win over the unknown-field error: {reason}"
                );
                assert!(
                    !reason.contains("unknown field") && !reason.contains("future"),
                    "must NOT be the strict-parse unknown-field error: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // Same claim with an unknown key inside a KNOWN table ([sources]): the
        // version gate still fires first, before deny_unknown_fields on [sources].
        let nested = "min-mind-version = \"999.0.0\"\n[sources]\nbogus-key = true\n";
        match parse_str(nested, Path::new("test-policy.toml")).unwrap_err() {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("requires mind >="),
                    "version gate must win over a nested unknown-field error: {reason}"
                );
                assert!(
                    !reason.contains("unknown field") && !reason.contains("bogus-key"),
                    "must not be the unknown-field error: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }
    }

    // POL-62: min-mind-version equal to the running binary is accepted (>= check).
    // spec: POL-62
    #[test]
    fn min_mind_version_equal_to_current_is_accepted() {
        let current = env!("CARGO_PKG_VERSION");
        let text = format!("min-mind-version = \"{current}\"\n[sources]\nlock = true\n");
        parse_str(&text, Path::new("test-policy.toml"))
            .expect("version == running binary must be accepted");
    }

    // POL-61: a min-mind-version lower than the running binary is silently OK.
    // spec: POL-61
    #[test]
    fn min_mind_version_lower_than_current_is_accepted() {
        let text = "min-mind-version = \"0.0.1\"\n";
        parse_str(text, Path::new("test-policy.toml"))
            .expect("version < running binary must be accepted");
    }

    // POL-61: min-mind-version absent means no gate (POL-4 inert).
    // spec: POL-61
    #[test]
    fn min_mind_version_absent_is_fine() {
        let text = "[sources]\nlock = true\n";
        parse_str(text, Path::new("test-policy.toml"))
            .expect("absent min-mind-version must be accepted");
    }

    // POL-63: an invalid version string in min-mind-version fails closed at
    // policy parse, consistent with POL-5.
    // spec: POL-63
    #[test]
    fn min_mind_version_invalid_string_fails_closed() {
        // Non-numeric component.
        let err = parse_str(
            "min-mind-version = \"not-a-version\"\n",
            Path::new("test-policy.toml"),
        )
        .unwrap_err();
        match err {
            MindError::InvalidPolicy { reason, .. } => {
                assert!(
                    reason.contains("not a valid version string"),
                    "must explain the problem: {reason}"
                );
            }
            other => panic!("expected InvalidPolicy, got {other:?}"),
        }

        // Empty string.
        let err =
            parse_str("min-mind-version = \"\"\n", Path::new("test-policy.toml")).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "empty version must be invalid: {err:?}"
        );

        // Trailing dot.
        let err = parse_str(
            "min-mind-version = \"1.2.\"\n",
            Path::new("test-policy.toml"),
        )
        .unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "trailing dot must be invalid: {err:?}"
        );

        // Prerelease suffix.
        let err = parse_str(
            "min-mind-version = \"0.14.0-rc1\"\n",
            Path::new("test-policy.toml"),
        )
        .unwrap_err();
        assert!(
            matches!(err, MindError::InvalidPolicy { .. }),
            "prerelease suffix must be invalid: {err:?}"
        );
    }

    // POL-61: min-mind-version is recognized by the strict parse (not an unknown
    // key) so a version-gated policy with no new keys round-trips cleanly.
    // spec: POL-61
    #[test]
    fn min_mind_version_is_not_rejected_by_strict_parse() {
        // min-mind-version <= running is OK; the key must be in RawPolicy so
        // deny_unknown_fields does not reject it in the strict phase-2 parse.
        let text = "min-mind-version = \"0.1.0\"\n[sources]\nlock = false\n";
        let p = parse_str(text, Path::new("test-policy.toml"))
            .expect("recognized key must not trigger unknown-field error");
        assert!(!p.lock(), "lock should be false");
    }

    // ----- POL-64/65: permission warning seam ---------------------------------

    // POL-65: when load_and_check is called with is_system_path=false (the
    // MIND_POLICY_FILE path), no permission check runs even for an insecure file.
    // spec: POL-65
    #[test]
    fn env_path_permissions_not_checked() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pol65-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.toml");
        std::fs::write(&path, "").unwrap();

        // On unix, make the file world-writable so a permission check WOULD fire.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o666);
            std::fs::set_permissions(&path, perms).unwrap();
        }

        // With is_system_path=false (env path) no warnings are returned.
        let (_, warnings) = load_and_check(&path, false).expect("load must succeed");
        assert!(
            warnings.is_empty(),
            "MIND_POLICY_FILE path must not be checked; got: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // POL-64: the pure helper returns warnings for group/world-writable mode.
    // Tested against known (mode, uid) pairs without touching the filesystem.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn permission_warns_on_group_writable_mode() {
        // Group-writable (mode & 0o020 != 0).
        let w = policy_path_security_warnings("/etc/mind/policy.toml", 0o664, 0, false);
        assert!(!w.is_empty(), "group-writable must warn");
        assert!(
            w[0].contains("group/world-writable"),
            "must name the problem: {}",
            w[0]
        );
        assert!(
            w[0].contains("/etc/mind/policy.toml"),
            "must name the path: {}",
            w[0]
        );
    }

    // POL-64: world-writable mode warns.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn permission_warns_on_world_writable_mode() {
        let w = policy_path_security_warnings("/etc/mind/policy.toml", 0o646, 0, false);
        assert!(!w.is_empty(), "world-writable must warn");
        assert!(w[0].contains("group/world-writable"), "message: {}", w[0]);
    }

    // POL-64: mode 0o644 with uid 0 (root-owned, not group/world-writable) => no warning.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn permission_no_warn_on_secure_root_owned_file() {
        let w = policy_path_security_warnings("/etc/mind/policy.toml", 0o644, 0, false);
        assert!(w.is_empty(), "root-owned 644 file must not warn: {w:?}");
    }

    // POL-64: non-root uid warns even when mode bits are fine.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn permission_warns_on_non_root_uid() {
        let w = policy_path_security_warnings("/etc/mind/policy.toml", 0o644, 1000, false);
        assert!(!w.is_empty(), "non-root uid must warn");
        assert!(
            w[0].contains("not root-owned"),
            "must name the uid problem: {}",
            w[0]
        );
    }

    // POL-64: parent-directory mode and uid are checked independently.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn permission_parent_dir_warnings_are_distinct() {
        // Parent dir group-writable + root-owned => one warning (mode only).
        let w = policy_path_security_warnings("/etc/mind", 0o775, 0, true);
        assert!(!w.is_empty(), "group-writable parent must warn");
        assert!(
            w[0].contains("parent directory"),
            "must say 'parent directory': {}",
            w[0]
        );
        assert!(
            w[0].contains("chown root and chmod 755"),
            "must suggest chmod 755: {}",
            w[0]
        );

        // Parent dir 755 + root-owned => no warnings.
        let w = policy_path_security_warnings("/etc/mind", 0o755, 0, true);
        assert!(w.is_empty(), "secure parent must not warn: {w:?}");
    }

    // POL-64: load_and_check with is_system_path=true on a real temp file that is
    // world-writable produces at least one mode warning on unix.
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn load_and_check_system_path_world_writable_warns() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pol64-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.toml");
        std::fs::write(&path, "").unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o666); // world-writable
        std::fs::set_permissions(&path, perms).unwrap();

        let (_, warnings) = load_and_check(&path, true).expect("load must succeed");
        let mode_warn = warnings.iter().any(|w| w.contains("group/world-writable"));
        assert!(
            mode_warn,
            "world-writable file with is_system_path=true must produce a mode warning: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // POL-64: load_and_check with is_system_path=true on a mode-secure file
    // does not produce a mode warning (uid warning may still fire in a non-root
    // test environment, which is expected and correct behavior -- the file is not
    // root-owned; we only assert the mode-based warning is absent).
    // spec: POL-64
    #[cfg(unix)]
    #[test]
    fn load_and_check_system_path_mode_secure_no_mode_warn() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pol64-sec-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        use std::os::unix::fs::PermissionsExt;
        // Explicitly set the parent dir to 0o755 so its mode does not trigger
        // a "group/world-writable" warning regardless of the process umask.
        let mut dir_perms = std::fs::metadata(&dir).unwrap().permissions();
        dir_perms.set_mode(0o755);
        std::fs::set_permissions(&dir, dir_perms).unwrap();

        let path = dir.join("policy.toml");
        std::fs::write(&path, "").unwrap();
        let mut file_perms = std::fs::metadata(&path).unwrap().permissions();
        file_perms.set_mode(0o644);
        std::fs::set_permissions(&path, file_perms).unwrap();

        let (_, warnings) = load_and_check(&path, true).expect("load must succeed");
        // Only check that mode-based warnings are absent; uid warnings may fire
        // in a non-root test context and are expected/correct.
        let mode_warn = warnings.iter().any(|w| w.contains("group/world-writable"));
        assert!(
            !mode_warn,
            "mode-secure file must not produce a group/world-writable warning: {warnings:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
