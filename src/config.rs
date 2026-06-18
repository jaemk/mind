//! User configuration for `mind`, stored at `~/.mind/config.toml`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};

/// Persistent settings. Currently just the agent homes ("lobes") to link items
/// into.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Agent homes ("lobes") `mind` links installed items into. Empty means "use
    /// the default" (see [`crate::paths::Paths::agent_homes`]). `~` is expanded
    /// at resolution time; entries are stored verbatim.
    #[serde(default)]
    pub lobes: Vec<String>,
}

impl Config {
    /// The config file path under a mind home.
    pub fn path(mind_home: &Path) -> PathBuf {
        mind_home.join("config.toml")
    }

    /// Load `config.toml` from the mind home, returning defaults if absent.
    pub fn load(mind_home: &Path) -> Result<Config> {
        let file = Self::path(mind_home);
        match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).map_err(|e| MindError::Toml {
                path: file.clone(),
                source: e,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    /// Write the config back to `config.toml`, creating the mind home if needed.
    pub fn save(&self, mind_home: &Path) -> Result<()> {
        std::fs::create_dir_all(mind_home).map_err(|e| MindError::io(mind_home, e))?;
        let file = Self::path(mind_home);
        let text = toml::to_string(self).map_err(|e| MindError::TomlWrite {
            path: file.clone(),
            source: e,
        })?;
        std::fs::write(&file, text).map_err(|e| MindError::io(&file, e))
    }
}
