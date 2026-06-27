//! User configuration for `mind`, stored at `~/.mind/config.toml`.

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};
use crate::paths::Paths;

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

    /// Prefer SSH for melded remotes. When true, `meld` rewrites an https remote
    /// (e.g. a `owner/repo` shorthand) to its `git@host:owner/repo` SSH form, so
    /// cloning uses the user's SSH key/agent instead of prompting for an https
    /// username/password (CLI-19). Local paths and explicit `git@`/`ssh://` specs
    /// are unaffected. Default false (https).
    #[serde(default)]
    pub ssh: bool,

    /// Default destination source for `absorb` (ABS-2). When set, `absorb` uses
    /// this path as the destination unless `--to` or `MIND_ABSORB_TO` is given.
    /// Saved interactively when the user chooses a destination via the ABS-3 prompt
    /// and confirms saving (ABS-4). `~` is expanded at use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorb_to: Option<String>,
}

impl Config {
    /// Load `config.toml` from the mind home, returning defaults if absent.
    pub fn load(paths: &Paths) -> Result<Config> {
        let file = paths.config_file();
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
    pub fn save(&self, paths: &Paths) -> Result<()> {
        std::fs::create_dir_all(&paths.mind_home)
            .map_err(|e| MindError::io(&paths.mind_home, e))?;
        let file = paths.config_file();
        let text = toml::to_string(self).map_err(|e| MindError::TomlWrite {
            path: file.clone(),
            source: e,
        })?;
        Paths::atomic_write(&file, text.as_bytes())
    }
}
