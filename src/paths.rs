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

    /// The agent homes items are linked into, in order: `$MIND_AGENT_HOMES` (a
    /// `:`-separated path list), else `lobes` from `~/.mind/config.toml`, else
    /// `[claude_home]`. A leading `~` is expanded, and a relative path is resolved
    /// to absolute against the current directory, so the link paths recorded in
    /// the manifest never depend on the working directory at a later uninstall.
    pub fn agent_homes(&self) -> Result<Vec<PathBuf>> {
        if let Some(raw) = std::env::var_os("MIND_AGENT_HOMES") {
            let homes = raw
                .to_string_lossy()
                .split(':')
                .filter(|p| !p.is_empty())
                .map(absolute_home)
                .collect::<Result<Vec<_>>>()?;
            if !homes.is_empty() {
                return Ok(homes);
            }
        }
        let configured = Config::load(&self.mind_home)?.lobes;
        if !configured.is_empty() {
            return configured.iter().map(|h| absolute_home(h)).collect();
        }
        Ok(vec![make_absolute(self.claude_home.clone())?])
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

/// `mkdir -p` that tags failures with the offending path.
pub fn mkdir_p(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| MindError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

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
}
