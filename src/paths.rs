//! The on-disk layout for `mind`.
//!
//! ```text
//! ~/.mind/
//!   sources.json                 registry of melded sources (see source.rs)
//!   manifest.json                installed-item manifest (see manifest.rs)
//!   sources/<host>/<owner>/<repo> bare-ish clones of each melded repo
//!   store/<kind>/<name>/          the installed copy of each item
//!
//! ~/.claude/
//!   skills/<name>  -> symlink into store/skill/<name>
//!   agents/<name>.md -> symlink into store/agent/<name>
//!   rules/<name>.md  -> symlink into store/rule/<name>
//! ```
//!
//! Every root is overridable via an environment variable so the test harness
//! can point them at temp dirs: `MIND_HOME`, `CLAUDE_HOME`.

use std::path::{Path, PathBuf};

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

    /// The default link target for an item, relative to `claude_home`.
    pub fn default_link_rel(&self, kind: ItemKind, name: &str) -> String {
        match kind {
            ItemKind::Skill => format!("skills/{name}"),
            ItemKind::Agent => format!("agents/{name}.md"),
            ItemKind::Rule => format!("rules/{name}.md"),
        }
    }

    /// Resolve a claude-home-relative link target to an absolute path.
    pub fn link_from_rel(&self, rel: &str) -> PathBuf {
        self.claude_home.join(rel)
    }

    /// Create the `~/.mind` scaffolding if it does not yet exist.
    pub fn ensure_layout(&self) -> Result<()> {
        mkdir_p(&self.mind_home)?;
        mkdir_p(&self.sources_dir())?;
        mkdir_p(&self.store_dir())?;
        Ok(())
    }
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().ok_or(MindError::HomeDirNotFound)
}

/// `mkdir -p` that tags failures with the offending path.
pub fn mkdir_p(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| MindError::io(path, e))
}
