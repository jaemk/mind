//! The installed-item manifest: what `mind` has placed into `~/.claude`.

use serde::{Deserialize, Serialize};

use crate::error::{ItemKind, MindError, Result};
use crate::paths::Paths;
use crate::source::RecordedHook;

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
    /// The item's install hooks recorded as run or offered (HOOK-110). Mirrors
    /// the source-level mechanism (`RecordedHook`, HOOK-55): a hook that RAN
    /// carries `ran_at = Some(<commit it ran at>)`; a hook that was offered but
    /// skipped (a non-TTY `hooks run`) carries `ran_at = None`. Absent in a
    /// manifest written before HOOK-110 deserializes as empty, so an item
    /// installed by an older binary is treated as having no recorded runs (its
    /// install hooks are re-offered on the next `hooks run`, same as before this
    /// field existed).
    #[serde(default)]
    pub install_hooks: Vec<RecordedHook>,
}

impl InstalledItem {
    /// Manifest key, using the effective installed name, e.g. `skill:jk:review`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }

    /// The key sanitized for a terminal (DSC-95): the installed name is
    /// source-derived and can carry ANSI/control/bidi code points. Use this at
    /// every human/`--json` print site; `key()` stays raw so it keeps matching
    /// the manifest map and the store/link paths.
    pub fn display_key(&self) -> String {
        crate::sanitize::strip_ansi(&self.key())
    }

    /// A display copy with every source-controlled string sanitized (DSC-95).
    /// Serialize this (not `self`) into a `--json` document so a bidi/ANSI bare
    /// name -- which `name`, `bare_name`, `store`, and each `link` embed -- cannot
    /// ride the document to a terminal (serde escapes ESC but not a bidi
    /// override). The persisted manifest is untouched; this is display-only.
    ///
    /// `install_hooks[].command` is sanitized too (CLI-232): it is fully
    /// source-controlled shell text (the source declared it; HOOK-110 records
    /// it verbatim as offered/run), and `recall <item> --json` serializes this
    /// copy -- a richer payload than the bare name this function was
    /// originally written to defuse, so leaving it out of `s(...)` would have
    /// been an asymmetry inside the very function meant to close DSC-95's gaps.
    /// `ran_at` (a commit hash) is not source-*controlled* text in the same
    /// sense and is left as-is.
    pub fn sanitized_for_display(&self) -> InstalledItem {
        let s = |v: &str| crate::sanitize::strip_ansi(v);
        InstalledItem {
            kind: self.kind,
            name: s(&self.name),
            bare_name: s(&self.bare_name),
            source: s(&self.source),
            commit: self.commit.clone(),
            hash: self.hash.clone(),
            store: s(&self.store),
            links: self.links.iter().map(|l| s(l)).collect(),
            description: self.description.as_deref().map(s),
            install_hooks: self
                .install_hooks
                .iter()
                .map(|h| crate::source::RecordedHook {
                    command: s(&h.command),
                    ran_at: h.ran_at.clone(),
                })
                .collect(),
        }
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

    // STO-31: a malformed manifest is a `Json` error naming the file, not a
    // silent empty manifest (which would read as "nothing installed" and let a
    // later save drop every recorded item). Normal tests only ever write valid
    // JSON, so this branch needs a hand-built document to reach.
    // spec: STO-31
    #[test]
    fn malformed_manifest_json_is_a_json_error_naming_the_file() {
        let (base, paths) = tmp_paths();
        std::fs::write(base.join("manifest.json"), "{\"items\": [ truncated").unwrap();
        match Manifest::load(&paths) {
            Err(MindError::Json { what, .. }) => assert_eq!(what, "manifest.json"),
            other => panic!("expected a Json error naming manifest.json, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
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

    fn stub_item(name: &str) -> InstalledItem {
        InstalledItem {
            kind: ItemKind::Skill,
            name: name.to_string(),
            bare_name: name.to_string(),
            source: "local/test".to_string(),
            commit: "abc123".to_string(),
            hash: "deadbeef".to_string(),
            store: format!("store/skill/{name}"),
            links: Vec::new(),
            description: None,
            install_hooks: Vec::new(),
        }
    }

    /// An item's `install_hooks` record (STO-75) survives a save/load
    /// round-trip through `manifest.json`, including both a hook that ran
    /// (`ran_at = Some(commit)`) and one that was offered and skipped
    /// (`ran_at = None`).
    // spec: STO-75 HOOK-110
    #[test]
    fn install_hooks_record_round_trips_through_save_and_load() {
        let (base, paths) = tmp_paths();
        let mut item = stub_item("scanner");
        item.install_hooks = vec![
            RecordedHook {
                command: "touch ran.sentinel".to_string(),
                ran_at: Some("abc123".to_string()),
            },
            RecordedHook {
                command: "touch skipped.sentinel".to_string(),
                ran_at: None,
            },
        ];
        let mut manifest = Manifest::default();
        manifest.insert(item.clone());
        manifest.save(&paths).expect("save manifest");

        let loaded = Manifest::load(&paths).expect("load manifest");
        let back = loaded.items.get(&item.key()).expect("item present");
        assert_eq!(
            back.install_hooks, item.install_hooks,
            "install_hooks must round-trip unchanged"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A `manifest.json` written before `install_hooks` existed (STO-75) has no
    /// such key on its item objects. It must deserialize as an empty set rather
    /// than fail to load, so an item installed by an older binary is simply
    /// treated as having no recorded install-hook runs.
    // spec: STO-75 HOOK-110
    #[test]
    fn install_hooks_missing_from_an_older_manifest_deserializes_as_empty() {
        let (base, paths) = tmp_paths();
        std::fs::write(
            base.join("manifest.json"),
            r#"{"version":1,"items":{"skill:scanner":{
                "kind":"skill","name":"scanner","bare_name":"scanner",
                "source":"local/test","commit":"abc123","hash":"deadbeef",
                "store":"store/skill/scanner","links":[]
            }}}"#,
        )
        .unwrap();
        let m = Manifest::load(&paths).expect("must load a pre-HOOK-110 manifest");
        let item = m.items.get("skill:scanner").expect("item present");
        assert!(
            item.install_hooks.is_empty(),
            "a missing install_hooks field must default to empty, got {:?}",
            item.install_hooks
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `sanitized_for_display` (DSC-95) must sanitize `install_hooks[].command`,
    /// not just the name/path fields: it is fully source-controlled shell text
    /// that `recall <item> --json` serializes from this exact copy (M9 -- a
    /// richer payload than the bare name this function exists to defuse).
    // spec: CLI-232 DSC-95
    #[test]
    fn sanitized_for_display_strips_ansi_from_install_hook_commands() {
        let mut item = stub_item("evil");
        item.install_hooks = vec![
            RecordedHook {
                command: "echo \x1b[31mran\x1b[0m".to_string(),
                ran_at: Some("abc123".to_string()),
            },
            RecordedHook {
                command: "echo \x1b]0;evil\x07skipped".to_string(),
                ran_at: None,
            },
        ];
        let display = item.sanitized_for_display();
        assert_eq!(display.install_hooks.len(), 2);
        for hook in &display.install_hooks {
            assert!(
                !hook.command.contains('\x1b'),
                "sanitized_for_display must strip ANSI/escape bytes from a hook \
                 command, got {:?}",
                hook.command
            );
        }
        assert_eq!(display.install_hooks[0].command, "echo ran");
        assert_eq!(display.install_hooks[0].ran_at, Some("abc123".to_string()));
        assert_eq!(display.install_hooks[1].ran_at, None);
        // The persisted item is untouched -- this is display-only.
        assert!(item.install_hooks[0].command.contains('\x1b'));
    }
}
