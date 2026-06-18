//! The installed-item manifest: what `mind` has placed into `~/.claude`.

use serde::{Deserialize, Serialize};

use crate::error::{ItemKind, MindError, Result};
use crate::paths::Paths;

/// `serde` shim so [`ItemKind`] round-trips through JSON as a lowercase string.
mod kind_serde {
    use super::ItemKind;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(kind: &ItemKind, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(kind.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ItemKind, D::Error> {
        let raw = String::deserialize(d)?;
        match raw.as_str() {
            "skill" => Ok(ItemKind::Skill),
            "agent" => Ok(ItemKind::Agent),
            "rule" => Ok(ItemKind::Rule),
            other => Err(D::Error::custom(format!("unknown item kind '{other}'"))),
        }
    }
}

/// A single installed item, as recorded in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledItem {
    #[serde(with = "kind_serde")]
    pub kind: ItemKind,
    /// The effective installed name (possibly prefixed); also the manifest key.
    pub name: String,
    /// The bare source name. With `source` and `kind`, this is the item's stable
    /// identity, which survives a namespace/prefix change.
    pub bare_name: String,
    /// The source `name` this item came from.
    pub source: String,
    /// The source commit it was installed from.
    pub commit: String,
    /// Content hash of the *source* content (for drift / upgrade detection).
    pub hash: String,
    /// Store copy location, relative to `~/.mind` (the file registry).
    pub store: String,
    /// Absolute symlink paths created for this item, one per agent home.
    pub links: Vec<String>,
    /// One-line description captured at install time, for `recall`.
    #[serde(default)]
    pub description: Option<String>,
}

impl InstalledItem {
    /// Manifest key, using the effective installed name, e.g. `skill:jk-review`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }
}

/// The persisted set of installed items, keyed by `kind:name`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub items: std::collections::BTreeMap<String, InstalledItem>,
}

impl Manifest {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.manifest_file();
        match std::fs::read(&file) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| MindError::json("manifest.json", e))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure_layout()?;
        let file = paths.manifest_file();
        let json =
            serde_json::to_vec_pretty(self).map_err(|e| MindError::json("manifest.json", e))?;
        std::fs::write(&file, json).map_err(|e| MindError::io(&file, e))
    }

    pub fn insert(&mut self, item: InstalledItem) {
        self.items.insert(item.key(), item);
    }
}
