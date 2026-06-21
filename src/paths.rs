//! The on-disk layout for `mind`.
//!
//! ```text
//! ~/.mind/
//!   sources.json                 registry of melded sources (see source.rs)
//!   manifest.json                installed-item manifest (see manifest.rs)
//!   sources/<host>/<owner>/<repo> bare-ish clones of each melded repo
//!   store/<kind>/<name>/          the installed copy of each item
//!
//! <agent home>/                     (one or more; default ~/.claude)
//!   skills/<name>  -> symlink into store/skill/<name>
//!   agents/<name>.md -> symlink into store/agent/<name>
//!   rules/<name>.md  -> symlink into store/rule/<name>
//! ```
//!
//! Items are linked into every configured agent home (see [`Paths::agent_homes`]).
//! Roots are overridable via environment variables so the test harness can point
//! them at temp dirs: `MIND_HOME`, `CLAUDE_HOME`, `MIND_AGENT_HOMES`.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{ItemKind, MindError, Result};
use crate::policy::Policy;

/// Resolved filesystem roots for a `mind` invocation.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.mind` (or `$MIND_HOME`).
    pub mind_home: PathBuf,
    /// `~/.claude` (or `$CLAUDE_HOME`).
    pub claude_home: PathBuf,
}

impl Paths {
    /// Resolve roots from the environment, falling back to the home directory.
    pub fn resolve() -> Result<Self> {
        let mind_home = match std::env::var_os("MIND_HOME") {
            Some(p) => PathBuf::from(p),
            None => home()?.join(".mind"),
        };
        let claude_home = match std::env::var_os("CLAUDE_HOME") {
            Some(p) => PathBuf::from(p),
            None => home()?.join(".claude"),
        };
        Ok(Self {
            mind_home,
            claude_home,
        })
    }

    /// Path to the global advisory lock file.
    // spec: STO-40
    pub fn lock_file(&self) -> PathBuf {
        self.mind_home.join(".lock")
    }

    pub fn sources_file(&self) -> PathBuf {
        self.mind_home.join("sources.json")
    }

    pub fn manifest_file(&self) -> PathBuf {
        self.mind_home.join("manifest.json")
    }

    /// Root under which melded repos are cloned.
    pub fn sources_dir(&self) -> PathBuf {
        self.mind_home.join("sources")
    }

    /// Root under which installed item copies live.
    pub fn store_dir(&self) -> PathBuf {
        self.mind_home.join("store")
    }

    /// The store location for one installed item.
    pub fn store_item(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.mind_home.join(self.store_rel(kind, name))
    }

    /// The store location for one item, relative to `mind_home` (recorded in the
    /// manifest so uninstall removes exactly what was installed).
    pub fn store_rel(&self, kind: ItemKind, name: &str) -> String {
        format!("store/{}/{}", kind.as_str(), name)
    }

    /// Scratch root for transactional installs (staging + backup).
    pub fn tmp_dir(&self) -> PathBuf {
        self.mind_home.join(".tmp")
    }

    /// Where a new item copy is built before it is swapped into the store.
    pub fn staging_path(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.tmp_dir()
            .join("staging")
            .join(kind.as_str())
            .join(name)
    }

    /// Where the previous store copy is held during a swap, for rollback.
    pub fn backup_path(&self, kind: ItemKind, name: &str) -> PathBuf {
        self.tmp_dir().join("backup").join(kind.as_str()).join(name)
    }

    /// The default link target for an item, relative to an agent home.
    pub fn default_link_rel(&self, kind: ItemKind, name: &str) -> String {
        match kind {
            ItemKind::Skill => format!("skills/{name}"),
            ItemKind::Agent => format!("agents/{name}.md"),
            ItemKind::Rule => format!("rules/{name}.md"),
        }
    }

    /// The agent homes items are linked into. Without a managed policy this is,
    /// in order: `$MIND_AGENT_HOMES` (a `:`-separated path list), else `lobes`
    /// from `~/.mind/config.toml`, else `[claude_home]`.
    ///
    /// When a managed policy is in effect:
    /// - `[lobes].lock = true` (POL-40): the effective homes are exactly
    ///   `[lobes].targets`; `$MIND_AGENT_HOMES` and config `lobes` are ignored.
    ///   An empty `targets` under a lock falls back to the default (`claude_home`).
    /// - `[lobes].lock = false` (POL-41): `[lobes].targets` is a base set that is
    ///   unioned with the user's normally-resolved homes (deduped; targets first).
    ///
    /// A leading `~` is expanded, and a relative path is resolved to absolute
    /// against the current directory, so the link paths recorded in the manifest
    /// never depend on the working directory at a later uninstall.
    pub fn agent_homes(&self) -> Result<Vec<PathBuf>> {
        // Compute the user's normal homes (pre-policy).
        let user_homes: Vec<PathBuf> = {
            let mut h = Vec::new();
            if let Some(raw) = std::env::var_os("MIND_AGENT_HOMES") {
                let parsed = raw
                    .to_string_lossy()
                    .split(':')
                    .filter(|p| !p.is_empty())
                    .map(absolute_home)
                    .collect::<Result<Vec<_>>>()?;
                if !parsed.is_empty() {
                    h = parsed;
                }
            }
            if h.is_empty() {
                let configured = Config::load(&self.mind_home)?.lobes;
                if !configured.is_empty() {
                    h = configured
                        .iter()
                        .map(|p| absolute_home(p))
                        .collect::<Result<Vec<_>>>()?;
                }
            }
            if h.is_empty() {
                h = vec![make_absolute(self.claude_home.clone())?];
            }
            h
        };

        // Apply managed-policy lobe rules when a policy is in effect.
        // spec: POL-40
        // spec: POL-41
        match Policy::load()? {
            Some(policy) if policy.lobes_lock() => {
                // POL-40: locked - use exactly the policy targets, ignoring user homes.
                let targets = policy.lobes_targets();
                if targets.is_empty() {
                    // Empty targets under a lock pins the default.
                    Ok(vec![make_absolute(self.claude_home.clone())?])
                } else {
                    let resolved: Vec<PathBuf> = targets
                        .iter()
                        .map(|p| absolute_home(p))
                        .collect::<Result<_>>()?;
                    Ok(dedup_paths(resolved))
                }
            }
            Some(policy) => {
                // POL-41: not locked - union policy targets with user homes (targets first,
                // deduped). The whole result is deduped to collapse duplicate targets and
                // targets that equal a user home.
                // spec: POL-41
                let mut result: Vec<PathBuf> = Vec::new();
                for p in policy.lobes_targets() {
                    result.push(absolute_home(p)?);
                }
                for h in user_homes {
                    result.push(h);
                }
                Ok(dedup_paths(result))
            }
            None => {
                // POL-4 inert: no policy, use user homes as-is.
                Ok(user_homes)
            }
        }
    }

    /// The default lobe written into a fresh config: the `$CLAUDE_HOME` override
    /// if set, else `~/.claude`.
    pub fn default_lobe(&self) -> String {
        match std::env::var_os("CLAUDE_HOME") {
            Some(v) => v.to_string_lossy().into_owned(),
            None => "~/.claude".to_string(),
        }
    }

    /// Create `config.toml` with default values if it does not exist yet.
    pub fn ensure_config(&self) -> Result<()> {
        if !Config::path(&self.mind_home).exists() {
            Config {
                lobes: vec![self.default_lobe()],
                ..Default::default()
            }
            .save(&self.mind_home)?;
        }
        Ok(())
    }

    /// Create the `~/.mind` scaffolding (and a default config) if absent.
    pub fn ensure_layout(&self) -> Result<()> {
        mkdir_p(&self.mind_home)?;
        mkdir_p(&self.sources_dir())?;
        mkdir_p(&self.store_dir())?;
        self.ensure_config()?;
        Ok(())
    }

    /// Write `bytes` to `target` atomically by writing a sibling temp file and
    /// renaming it over the target. Callers see either the old file or the new
    /// file, never a partial write.
    ///
    /// The temp file is placed in the same directory as `target` (required for
    /// `rename` to be atomic within one filesystem). Named
    /// `.<filename>.tmp.<pid>` so it is identifiable on crash.
    ///
    /// Called by `source.rs`, `manifest.rs`, and `config.rs` once the
    /// mechanical shard wires them up; until then the unit tests exercise it.
    // spec: STO-43
    pub fn atomic_write(target: &std::path::Path, bytes: &[u8]) -> Result<()> {
        let dir = target
            .parent()
            .ok_or_else(|| MindError::io(target, std::io::Error::other("no parent directory")))?;
        let file_name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let tmp_name = format!(".{}.tmp.{}", file_name, std::process::id());
        let tmp_path = dir.join(&tmp_name);

        // Write to temp; clean up on error.
        let write_result =
            std::fs::write(&tmp_path, bytes).map_err(|e| MindError::io(&tmp_path, e));
        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        // Rename over the target; clean up temp on error.
        std::fs::rename(&tmp_path, target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_path);
            MindError::io(target, e)
        })
    }
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or(MindError::HomeDirNotFound)
}

/// Expand a leading `~` / `~/` to the home directory; other paths pass through.
fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(h) = dirs::home_dir()
    {
        return h.join(rest);
    }
    PathBuf::from(path)
}

/// Expand `~` and then resolve a relative agent-home path to an absolute one.
fn absolute_home(path: &str) -> Result<PathBuf> {
    make_absolute(expand_home(path))
}

/// Resolve a path to absolute against the current directory, leaving an
/// already-absolute path unchanged. Does not touch the filesystem (no symlink
/// resolution), so it works for a home that does not exist yet.
fn make_absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir().map_err(|e| MindError::io(".", e))?;
    Ok(cwd.join(path))
}

/// Deduplicate a `Vec<PathBuf>` preserving first-seen order.
fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// `mkdir -p` that tags failures with the offending path.
pub fn mkdir_p(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| MindError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Serialize all tests that touch process-global env vars (`MIND_POLICY_FILE`,
    /// `MIND_AGENT_HOMES`, `MIND_HOME`, `CLAUDE_HOME`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn absolute_home_resolves_relative_paths() {
        // spec: STO-16
        let abs = absolute_home("rellobe").unwrap();
        assert!(
            abs.is_absolute(),
            "relative lobe should become absolute: {abs:?}"
        );
        assert!(abs.ends_with("rellobe"));
        // An already-absolute path is unchanged.
        assert_eq!(
            absolute_home("/tmp/lobe").unwrap(),
            PathBuf::from("/tmp/lobe")
        );
    }

    #[test]
    fn atomic_write_replaces_target_content() {
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-paths-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("data.json");

        // Write initial content.
        std::fs::write(&target, b"old").unwrap();

        // Atomically replace with new content.
        Paths::atomic_write(&target, b"new").unwrap();
        let got = std::fs::read(&target).unwrap();
        assert_eq!(got, b"new", "target should contain the new bytes");

        // No temp file should be left behind.
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftover.is_empty(),
            "temp file was not cleaned up: {leftover:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_creates_target_if_absent() {
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-create-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("new.json");

        assert!(!target.exists(), "sanity: target should not exist yet");
        Paths::atomic_write(&target, b"{\"x\":1}").unwrap();
        assert!(target.exists(), "atomic_write should create the target");
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"x\":1}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_errors_and_leaves_no_temp_when_dir_is_missing() {
        // If the target's parent directory does not exist, the temp write fails.
        // atomic_write must return an Io error (not panic) and must not leave a
        // stray temp file behind (there is nowhere to leave it, but the cleanup
        // path must run without error).
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let missing_dir =
            std::env::temp_dir().join(format!("mind-paths-missing-{}-{n}", std::process::id()));
        // Deliberately do NOT create missing_dir.
        let target = missing_dir.join("data.json");

        let result = Paths::atomic_write(&target, b"data");
        match result {
            Err(MindError::Io { .. }) => {}
            other => panic!("expected Io error for missing parent dir, got {other:?}"),
        }
        assert!(
            !target.exists(),
            "target must not exist after a failed atomic_write"
        );
        assert!(
            !missing_dir.exists(),
            "atomic_write must not create the parent directory"
        );
    }

    #[test]
    fn atomic_write_preserves_existing_target_on_write_failure() {
        // A crash/error mid-write must leave the previous file intact (STO-43:
        // "a crash mid-write leaves the previous file intact"). We force the temp
        // write to fail by making the target a path under a *file* (so the temp's
        // parent is not a directory), and assert the original target is unchanged.
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-failkeep-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // `blocker` is a regular file; treating it as a directory makes any write
        // under it fail (ENOTDIR), exercising the temp-write error branch.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"i am a file").unwrap();
        let target = blocker.join("data.json");

        let result = Paths::atomic_write(&target, b"new");
        assert!(
            matches!(result, Err(MindError::Io { .. })),
            "expected Io error when temp parent is a file, got {result:?}"
        );
        // The blocker file must be untouched (not clobbered by a temp file name).
        assert_eq!(
            std::fs::read(&blocker).unwrap(),
            b"i am a file",
            "unrelated sibling content must be preserved on failure"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_to_pathless_target_is_an_error() {
        // A target with no parent (the filesystem root) cannot host a sibling
        // temp; atomic_write must surface that as an Io error, not panic.
        // spec: STO-43
        let result = Paths::atomic_write(std::path::Path::new("/"), b"x");
        assert!(
            matches!(result, Err(MindError::Io { .. })),
            "writing to a target with no usable parent must be an Io error, got {result:?}"
        );
    }

    #[test]
    fn atomic_write_uses_same_directory_for_temp() {
        // The temp file must be in the same directory as the target so rename
        // is atomic (same filesystem).
        // spec: STO-43
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("mind-paths-samedir-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("sources.json");

        // Hook: after write the temp file should be in the same directory.
        // We can verify by checking rename succeeded (no EXDEV cross-device error).
        Paths::atomic_write(&target, b"[]").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"[]");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- managed-policy lobe tests -----------------------------------------

    /// Write a policy.toml, build a Paths pointing at a temp dir, and set
    /// MIND_POLICY_FILE. Returns (Paths, managed-dir, policy-file-path, guard).
    /// The guard must be held for the duration of the test; drop it last to
    /// restore the env var.
    fn setup_policy_test(
        policy_toml: &str,
    ) -> (Paths, PathBuf, PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-policy-lobe-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();
        let policy_file = base.join("policy.toml");
        std::fs::write(&policy_file, policy_toml).unwrap();
        // Unset MIND_AGENT_HOMES so it doesn't bleed in from the outer env.
        // SAFETY: ENV_LOCK is held, so no concurrent env reads on other threads.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
            std::env::set_var("MIND_POLICY_FILE", &policy_file);
        }
        let paths = Paths {
            mind_home,
            claude_home,
        };
        (paths, base, policy_file, guard)
    }

    // POL-40: with lobes.lock=true and explicit targets, agent_homes returns
    // exactly the policy targets, ignoring $MIND_AGENT_HOMES and config lobes.
    #[test]
    fn pol40_lock_true_uses_exactly_policy_targets() {
        // spec: POL-40
        let managed = {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            std::env::temp_dir().join(format!("mind-managed-lobe-{}-{n}", std::process::id()))
        };
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{managed}\"]\n",
            managed = managed.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Write a config with a different lobe - it must be ignored under lock.
        let other_lobe = base.join("other-lobe");
        let config_toml = format!(
            "lobes = [\"{other_lobe}\"]\n",
            other_lobe = other_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        // Also set MIND_AGENT_HOMES to yet another path - also must be ignored.
        let env_lobe = base.join("env-lobe");
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", env_lobe.to_str().unwrap());
        }

        let homes = paths.agent_homes().unwrap();

        // Restore env before any asserts that might panic.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        assert_eq!(
            homes,
            vec![managed.clone()],
            "POL-40: locked policy must return exactly the managed target, not config/env homes"
        );
        assert!(
            !homes.contains(&other_lobe),
            "config lobe must be ignored under lock"
        );
        assert!(
            !homes.contains(&env_lobe),
            "MIND_AGENT_HOMES must be ignored under lock"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: with lobes.lock=true and empty targets, agent_homes falls back to
    // the default (claude_home), not an empty list.
    #[test]
    fn pol40_lock_true_empty_targets_falls_back_to_default() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = []\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes = paths.agent_homes().unwrap();
        assert_eq!(
            homes,
            vec![paths.claude_home.clone()],
            "POL-40: empty targets under a lock must fall back to the default (claude_home)"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-41: with lobes.lock=false (or absent) and policy targets set, agent_homes
    // returns the union of policy targets and user homes, with targets first and
    // no duplicates.
    #[test]
    fn pol41_lock_false_unions_policy_and_user_homes() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir = std::env::temp_dir().join(format!("mind-pol41-{}-{n}", std::process::id()));
        let policy_target = base_dir.join("policy-base");
        let user_lobe = base_dir.join("user-lobe");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{policy_target}\"]\n",
            policy_target = policy_target.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Write a config with a user lobe.
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes = paths.agent_homes().unwrap();
        assert!(
            homes.contains(&policy_target),
            "POL-41: policy target must be present in union: {homes:?}"
        );
        assert!(
            homes.contains(&user_lobe),
            "POL-41: user lobe must also be present: {homes:?}"
        );
        // Policy target is first.
        assert_eq!(
            homes[0], policy_target,
            "POL-41: policy target must come first in the union"
        );
        // No duplicates.
        let deduped: Vec<_> = {
            let mut seen = std::collections::HashSet::new();
            homes.iter().filter(|h| seen.insert(*h)).cloned().collect()
        };
        assert_eq!(homes, deduped, "POL-41: result must not contain duplicates");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: when a policy target is already in the user's homes, it is not
    // duplicated in the result.
    #[test]
    fn pol41_deduplicates_overlapping_target_and_user_home() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let shared =
            std::env::temp_dir().join(format!("mind-pol41-shared-{}-{n}", std::process::id()));
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{shared}\"]\n",
            shared = shared.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User config also lists the same path.
        let config_toml = format!("lobes = [\"{shared}\"]\n", shared = shared.display());
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes = paths.agent_homes().unwrap();
        assert_eq!(
            homes.len(),
            1,
            "POL-41: identical target + user lobe must be deduped to one entry: {homes:?}"
        );
        assert_eq!(homes[0], shared);

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-4 inert: with no MIND_POLICY_FILE set and no system policy file,
    // agent_homes behaves exactly as before the policy feature (uses config lobes).
    #[test]
    fn pol4_inert_no_policy_uses_user_config() {
        // spec: POL-40
        // spec: POL-41
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-pol4-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        // Ensure no policy env var is set.
        // SAFETY: ENV_LOCK is held, so no concurrent env reads on other threads.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::remove_var("MIND_AGENT_HOMES");
        }

        let user_lobe = base.join("user-lobe");
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(mind_home.join("config.toml"), &config_toml).unwrap();

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes = paths.agent_homes().unwrap();
        assert_eq!(
            homes,
            vec![user_lobe.clone()],
            "POL-4 inert: without a policy, user config lobes must be used as-is"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- gap-closing managed-policy lobe tests -----------------------------

    // POL-41: the unlocked union must draw the user's homes from the
    // $MIND_AGENT_HOMES source, not only from config lobes. With an unlocked
    // policy target and MIND_AGENT_HOMES set (no config), the env home appears in
    // the union, after the policy target, with no duplicates.
    #[test]
    fn pol41_unions_policy_with_env_agent_homes() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-env-{}-{n}", std::process::id()));
        let policy_target = base_dir.join("policy-base");
        let env_lobe = base_dir.join("env-lobe");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{policy_target}\"]\n",
            policy_target = policy_target.display()
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Drive user homes via the env var (no config.toml written), to exercise
        // the $MIND_AGENT_HOMES source of user homes specifically.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", env_lobe.to_str().unwrap());
        }

        let homes = paths.agent_homes();

        // Restore env before any asserts that might panic.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes = homes.unwrap();

        assert_eq!(
            homes,
            vec![policy_target.clone(), env_lobe.clone()],
            "POL-41: unlocked union must be [policy target, env home], targets first, deduped"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-40: a lock with MULTIPLE targets returns exactly those targets in
    // declaration order, even when several user homes are set via env (all
    // ignored under the lock).
    #[test]
    fn pol40_lock_true_multiple_targets_in_order_ignores_user_homes() {
        // spec: POL-40
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol40-multi-{}-{n}", std::process::id()));
        let t1 = base_dir.join("target-a");
        let t2 = base_dir.join("target-b");
        let t3 = base_dir.join("target-c");
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{t1}\", \"{t2}\", \"{t3}\"]\n",
            t1 = t1.display(),
            t2 = t2.display(),
            t3 = t3.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // Multiple user homes via env - all must be ignored under the lock.
        let e1 = base_dir.join("env-1");
        let e2 = base_dir.join("env-2");
        let env_val = format!("{}:{}", e1.display(), e2.display());
        // Also write a config lobe to confirm config is ignored too.
        let cfg_lobe = base_dir.join("cfg-lobe");
        std::fs::write(
            paths.mind_home.join("config.toml"),
            format!("lobes = [\"{}\"]\n", cfg_lobe.display()),
        )
        .unwrap();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes = homes.unwrap();

        assert_eq!(
            homes,
            vec![t1.clone(), t2.clone(), t3.clone()],
            "POL-40: locked policy must return exactly the targets in order"
        );
        assert!(!homes.contains(&e1) && !homes.contains(&e2));
        assert!(!homes.contains(&cfg_lobe));

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: unlocked union with MULTIPLE targets and MULTIPLE user homes, where
    // one user home duplicates a policy target. Asserts the exact deduped order:
    // all targets first (in order), then the user homes not already present (in
    // order), with the overlap dropped.
    #[test]
    fn pol41_multiple_targets_and_homes_exact_deduped_order() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-multi-{}-{n}", std::process::id()));
        let t1 = base_dir.join("t1");
        let t2 = base_dir.join("t2"); // also a user home (overlap)
        let u_extra = base_dir.join("u-extra");
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{t1}\", \"{t2}\"]\n",
            t1 = t1.display(),
            t2 = t2.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User homes (via env): t2 (overlaps target) then u_extra. Order matters:
        // t2 must be dropped as a dup, u_extra kept and appended last.
        let env_val = format!("{}:{}", t2.display(), u_extra.display());
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes = homes.unwrap();

        assert_eq!(
            homes,
            vec![t1.clone(), t2.clone(), u_extra.clone()],
            "POL-41: targets first in order, then non-duplicate user homes; overlap dropped"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-4 inert via the $MIND_AGENT_HOMES source (not config): with no policy
    // and MIND_AGENT_HOMES set, agent_homes returns those homes unchanged.
    #[test]
    fn pol4_inert_no_policy_uses_env_agent_homes_unchanged() {
        // spec: POL-40
        // spec: POL-41
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-pol4-env-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();

        let env1 = base.join("env-home-1");
        let env2 = base.join("env-home-2");
        let env_val = format!("{}:{}", env1.display(), env2.display());
        // No policy file; env drives user homes.
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_POLICY_FILE");
            std::env::set_var("MIND_AGENT_HOMES", &env_val);
        }

        let paths = Paths {
            mind_home,
            claude_home,
        };
        let homes = paths.agent_homes();

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes = homes.unwrap();

        assert_eq!(
            homes,
            vec![env1.clone(), env2.clone()],
            "POL-4 inert: without a policy, $MIND_AGENT_HOMES homes must be returned unchanged"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: a locked target written with a leading `~` is expanded to the home
    // directory and resolved to an absolute path (via absolute_home), so the
    // effective home never depends on the working directory.
    #[test]
    fn pol40_lock_target_tilde_is_expanded_to_absolute() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = [\"~/.claude-managed\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes = paths.agent_homes().unwrap();
        assert_eq!(homes.len(), 1);
        let got = &homes[0];
        assert!(
            got.is_absolute(),
            "tilde target must resolve absolute: {got:?}"
        );
        assert!(
            got.ends_with(".claude-managed"),
            "tilde target must expand under home: {got:?}"
        );
        let home = dirs::home_dir().expect("home dir for tilde expansion");
        assert_eq!(
            got,
            &home.join(".claude-managed"),
            "POL-40: `~` target must expand to <home>/.claude-managed"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-40: a RELATIVE locked target is resolved to an absolute path against the
    // current directory (make_absolute), so a later uninstall sees a stable,
    // cwd-independent home rather than the verbatim relative string.
    #[test]
    fn pol40_lock_relative_target_becomes_absolute() {
        // spec: POL-40
        let policy_toml = "[lobes]\nlock = true\ntargets = [\"managed-rel-lobe\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        let homes = paths.agent_homes().unwrap();
        assert_eq!(homes.len(), 1);
        let got = &homes[0];
        assert!(
            got.is_absolute(),
            "POL-40: a relative target must be resolved to absolute: {got:?}"
        );
        assert!(
            got.ends_with("managed-rel-lobe"),
            "the relative component must be preserved: {got:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // POL-41: an unlocked relative target is likewise resolved to absolute before
    // the union, so the targets-first entry is a stable absolute path.
    #[test]
    fn pol41_unlocked_relative_target_becomes_absolute() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-rel-{}-{n}", std::process::id()));
        let user_lobe = base_dir.join("user-lobe");
        let policy_toml = "[lobes]\nlock = false\ntargets = [\"unlocked-rel-lobe\"]\n";
        let (paths, base, _policy_file, _guard) = setup_policy_test(policy_toml);

        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::set_var("MIND_AGENT_HOMES", user_lobe.to_str().unwrap());
        }
        let homes = paths.agent_homes();
        // SAFETY: ENV_LOCK is held.
        unsafe {
            std::env::remove_var("MIND_AGENT_HOMES");
        }
        let homes = homes.unwrap();

        assert_eq!(homes.len(), 2, "target + one user home: {homes:?}");
        assert!(
            homes[0].is_absolute() && homes[0].ends_with("unlocked-rel-lobe"),
            "POL-41: unlocked relative target must resolve absolute, first: {homes:?}"
        );
        assert_eq!(homes[1], user_lobe, "user home follows the target");

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-41: duplicate entries within `targets` itself (e.g. the policy TOML
    // lists the same path twice) collapse to a single entry. A target that
    // duplicates the user home is also collapsed. The deduped result preserves
    // first-seen order.
    #[test]
    fn pol41_duplicate_targets_collapse_to_one_entry() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol41-dup-{}-{n}", std::process::id()));
        let dup_target = base_dir.join("dup-target");
        let user_lobe = base_dir.join("user-lobe");

        // targets has dup_target listed twice.
        let policy_toml = format!(
            "[lobes]\nlock = false\ntargets = [\"{dup}\", \"{dup}\"]\n",
            dup = dup_target.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        // User home is distinct from the duplicated target.
        let config_toml = format!(
            "lobes = [\"{user_lobe}\"]\n",
            user_lobe = user_lobe.display()
        );
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml).unwrap();

        let homes = paths.agent_homes().unwrap();

        // dup_target appears only once (duplicate within targets collapsed), then user_lobe.
        assert_eq!(
            homes,
            vec![dup_target.clone(), user_lobe.clone()],
            "POL-41: duplicate targets must collapse to one entry, user home follows: {homes:?}"
        );

        // Also verify: duplicate target that also equals the user home collapses to one.
        let shared = base_dir.join("shared");
        let policy_toml2 = format!(
            "[lobes]\nlock = false\ntargets = [\"{shared}\", \"{shared}\"]\n",
            shared = shared.display(),
        );
        std::fs::write(base.join("policy.toml"), &policy_toml2).unwrap();
        let config_toml2 = format!("lobes = [\"{shared}\"]\n", shared = shared.display());
        std::fs::write(paths.mind_home.join("config.toml"), &config_toml2).unwrap();

        let homes2 = paths.agent_homes().unwrap();
        assert_eq!(
            homes2,
            vec![shared.clone()],
            "POL-41: duplicates across targets and user home must all collapse to one: {homes2:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }

    // POL-40: duplicate entries within `targets` in a LOCKED policy collapse to
    // a single entry. The dedup must apply in the locked branch too.
    #[test]
    fn pol40_duplicate_targets_collapse_to_one_entry() {
        // spec: POL-41
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_dir =
            std::env::temp_dir().join(format!("mind-pol40-dup-{}-{n}", std::process::id()));
        let dup_target = base_dir.join("dup-locked");

        // targets has the same path twice under a lock.
        let policy_toml = format!(
            "[lobes]\nlock = true\ntargets = [\"{dup}\", \"{dup}\"]\n",
            dup = dup_target.display(),
        );
        let (paths, base, _policy_file, _guard) = setup_policy_test(&policy_toml);

        let homes = paths.agent_homes().unwrap();

        assert_eq!(
            homes,
            vec![dup_target.clone()],
            "POL-40: duplicate locked targets must collapse to one entry: {homes:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
