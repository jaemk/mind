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
}
