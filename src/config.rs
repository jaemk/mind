//! User configuration for `mind`, stored at `~/.mind/config.toml`.

use std::path::Path;

use serde::Deserialize;

use crate::error::{MindError, Result};

/// Persistent settings. Currently just the agent homes to link items into.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Directories `mind` links installed items into. Empty means "use the
    /// default" (see [`crate::paths::Paths::agent_homes`]). `~` is expanded.
    #[serde(default)]
    pub homes: Vec<String>,
}

impl Config {
    /// Load `config.toml` from the mind home, returning defaults if absent.
    pub fn load(mind_home: &Path) -> Result<Config> {
        let file = mind_home.join("config.toml");
        match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).map_err(|e| MindError::Toml {
                path: file.clone(),
                source: e,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }
}
