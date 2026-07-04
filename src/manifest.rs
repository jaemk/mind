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
        ItemKind::parse(&raw).ok_or_else(|| D::Error::custom(format!("unknown item kind '{raw}'")))
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
    /// Manifest key, using the effective installed name, e.g. `skill:jk:review`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }
}

/// The persisted set of installed items, keyed by `kind:name`.
///
/// The `version` field (STO-50) carries the schema version. A reader that finds
/// a version greater than `MANIFEST_VERSION` fails with `StateTooNew`. A missing
/// field is treated as version 1 for backward compatibility with pre-version files.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version (STO-50). Absent => 1 (backward compatibility).
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub items: std::collections::BTreeMap<String, InstalledItem>,
}

/// The maximum schema version this binary can read.
const MANIFEST_VERSION: u32 = 1;

fn default_version() -> u32 {
    1
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            version: MANIFEST_VERSION,
            items: Default::default(),
        }
    }
}

impl Manifest {
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.manifest_file();
        match std::fs::read(&file) {
            Ok(bytes) => {
                let m: Manifest = serde_json::from_slice(&bytes)
                    .map_err(|e| MindError::json("manifest.json", e))?;
                // spec: STO-50 STO-51
                if m.version > MANIFEST_VERSION {
                    return Err(MindError::StateTooNew {
                        what: "manifest.json",
                        found: m.version,
                        supported: MANIFEST_VERSION,
                    });
                }
                Ok(m)
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
        Paths::atomic_write(&file, &json)
    }

    pub fn insert(&mut self, item: InstalledItem) {
        self.items.insert(item.key(), item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn tmp_paths() -> (std::path::PathBuf, Paths) {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-manifest-ver-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        (base, paths)
    }

    #[test]
    fn manifest_missing_version_is_treated_as_one() {
        // spec: STO-50 -- a manifest.json with no "version" field must be read
        // as version 1 (backward compatibility with pre-version files).
        let (base, paths) = tmp_paths();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("manifest.json"), r#"{"items":{}}"#).unwrap();
        let m = Manifest::load(&paths).expect("must load without version field");
        assert_eq!(m.version, 1, "missing version must default to 1");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_version_one_loads_ok() {
        // spec: STO-50 -- version 1 is the maximum supported version; loading it
        // must succeed.
        let (base, paths) = tmp_paths();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("manifest.json"), r#"{"version":1,"items":{}}"#).unwrap();
        let m = Manifest::load(&paths).expect("version 1 must load");
        assert_eq!(m.version, 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn manifest_too_new_version_is_state_too_new_error() {
        // spec: STO-50 STO-51 -- a version > 1 must be a StateTooNew error
        // naming manifest.json, the found version, and the supported version.
        let (base, paths) = tmp_paths();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("manifest.json"), r#"{"version":99,"items":{}}"#).unwrap();
        let err = Manifest::load(&paths).unwrap_err();
        match err {
            MindError::StateTooNew {
                what,
                found,
                supported,
            } => {
                assert_eq!(what, "manifest.json");
                assert_eq!(found, 99);
                assert_eq!(supported, MANIFEST_VERSION);
            }
            other => panic!("expected StateTooNew, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
