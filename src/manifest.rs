//! The installed-item manifest: what `mind` has placed into `~/.claude`.

use serde::{Deserialize, Serialize};

use crate::error::{ItemKind, MindError, Result};
use crate::paths::Paths;
use crate::sanitize::ItemKey;
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

    /// `requires:` entries (DEP-4) dropped at install because the item came
    /// from a single-item item-link instance, whose catalog cannot contain the
    /// sibling they name (LNK-18). Recorded so the degradation stays
    /// discoverable after the install-time warning scrolls away: `recall` shows
    /// it on the item, `introspect` reports it, and it rides in the `--json`
    /// item shape (LNK-19, STO-21). Empty (and omitted from the file) for every
    /// ordinary install, so an item installed by an older binary, or from a
    /// normal source, deserializes exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_requires: Vec<String>,

    /// The install profiles this item was installed for (PROF-20), as a
    /// member or as a closure item (PROF-6). Empty (and omitted from the file)
    /// for an item installed by name, and for every item installed before
    /// profiles existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,

    /// Set when the consumer also asked for this item by name while it carries
    /// profile reasons (PROF-20), so disabling its profiles keeps it (PROF-25).
    /// Omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub explicit: bool,

    /// The release version `commit` resolved from, when the source is
    /// version-pinned (VER-60), so `upgrade` can show the version delta.
    /// Omitted for any other pin, and for items installed before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `skip_serializing_if` helper: omit a `false` flag from `manifest.json`.
fn is_false(b: &bool) -> bool {
    !*b
}

impl InstalledItem {
    /// Whether this item was asked for by name (PROF-20): it carries no profile
    /// reason at all, or it was marked explicit on top of them. An explicit
    /// item is never removed by disabling a profile (PROF-25).
    pub fn is_explicit(&self) -> bool {
        self.profiles.is_empty() || self.explicit
    }

    /// Manifest key, using the effective installed name, e.g. `skill:jk:review`.
    ///
    /// Returns an [`ItemKey`] (DSC-95): the installed name is source-derived
    /// and can carry ANSI/control/bidi code points, so the raw and display
    /// readings are distinct types. `.as_str()` for identity (map/set lookups,
    /// the manifest key, path composition); `.display()` at every
    /// human/`--json` print site -- `ItemKey` has no `Display`, so passing it
    /// straight to `println!`/`format!` is a compile error.
    pub fn key(&self) -> ItemKey {
        ItemKey::new(format!("{}:{}", self.kind.as_str(), self.name))
    }

    /// Convenience for [`ItemKey::display`] on `self.key()` (DSC-95).
    pub fn display_key(&self) -> String {
        self.key().display()
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
            // A dropped entry is raw `requires:` frontmatter text (LNK-19), so
            // it is source-controlled exactly like `description` and is
            // sanitized on the same terms.
            dropped_requires: self.dropped_requires.iter().map(|e| s(e)).collect(),
            // Profile names pass PROF-2 before they are recorded, so they are
            // safe to print as-is.
            profiles: self.profiles.clone(),
            explicit: self.explicit,
            version: self.version.as_deref().map(s),
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
        self.items.insert(item.key().into(), item);
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
            dropped_requires: Vec::new(),
            profiles: Vec::new(),
            explicit: false,
            version: None,
        }
    }

    /// Install reasons (PROF-20) round-trip through `manifest.json`; an entry
    /// without them (by-name or pre-feature) omits both keys, reads back
    /// explicit, and an older manifest without the keys still loads.
    // spec: PROF-20
    #[test]
    fn profile_reasons_round_trip_and_default_absent() {
        let (base, paths) = tmp_paths();
        let mut tagged = stub_item("tagged");
        tagged.profiles = vec!["default".into(), "full".into()];
        tagged.explicit = true;
        let mut member = stub_item("member");
        member.profiles = vec!["full".into()];
        let plain = stub_item("plain");
        let mut manifest = Manifest::default();
        manifest.insert(tagged.clone());
        manifest.insert(member.clone());
        manifest.insert(plain.clone());
        manifest.save(&paths).expect("save manifest");

        let raw = std::fs::read_to_string(base.join("manifest.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let plain_obj = &v["items"]["skill:plain"];
        assert!(plain_obj.get("profiles").is_none(), "{plain_obj}");
        assert!(plain_obj.get("explicit").is_none(), "{plain_obj}");
        assert!(v["items"]["skill:member"].get("explicit").is_none());

        let loaded = Manifest::load(&paths).expect("load manifest");
        let t = &loaded.items["skill:tagged"];
        assert_eq!(t.profiles, tagged.profiles);
        assert!(t.explicit && t.is_explicit());
        let m = &loaded.items["skill:member"];
        assert_eq!(m.profiles, vec!["full"]);
        assert!(
            !m.explicit && !m.is_explicit(),
            "profile-only item is not explicit"
        );
        let p = &loaded.items["skill:plain"];
        assert!(p.profiles.is_empty() && !p.explicit && p.is_explicit());

        // The display copy keeps both.
        let d = t.sanitized_for_display();
        assert_eq!(d.profiles, tagged.profiles);
        assert!(d.explicit);

        // A manifest written before profiles existed has neither key.
        std::fs::write(
            base.join("manifest.json"),
            r#"{"version":1,"items":{"skill:old":{
                "kind":"skill","name":"old","bare_name":"old",
                "source":"local/test","commit":"abc123","hash":"deadbeef",
                "store":"store/skill/old","links":[]
            }}}"#,
        )
        .unwrap();
        let old = Manifest::load(&paths).expect("must load a pre-profile manifest");
        let o = &old.items["skill:old"];
        assert!(o.profiles.is_empty() && !o.explicit && o.is_explicit());
        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: PROF-20
    #[test]
    fn is_explicit_truth_table_and_display_copy() {
        let mut i = stub_item("x");
        assert!(i.is_explicit(), "no reasons: asked for by name");
        i.explicit = true;
        assert!(i.is_explicit(), "explicit with no reasons");
        i.explicit = false;
        i.profiles = vec!["full".into()];
        assert!(!i.is_explicit(), "reasons, not explicit");
        i.explicit = true;
        assert!(i.is_explicit(), "reasons plus explicit");

        // The display copy sanitizes source text but leaves reasons verbatim
        // and keeps the flag.
        i.description = Some("d\u{1b}[31m".into());
        let d = i.sanitized_for_display();
        assert!(!d.description.as_ref().unwrap().contains('\u{1b}'));
        assert_eq!(d.profiles, vec!["full"]);
        assert!(d.explicit && d.is_explicit());
        let mut plain = stub_item("y");
        plain.profiles = vec!["a".into()];
        let d = plain.sanitized_for_display();
        assert!(!d.explicit && !d.is_explicit());
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
        let back = loaded.items.get(item.key().as_str()).expect("item present");
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

    /// DSC-95 format-pin: `Manifest.items` is `BTreeMap<String, InstalledItem>`
    /// -- `ItemKey` never appears as a field type anywhere in the persisted
    /// shape, only as a transient value built by `key()`/`display_key()` and
    /// immediately converted back to a plain `String` (`.into()`, at
    /// `insert`). So the `ItemKey` newtype is a compile-time discipline on
    /// call sites, not a change to what gets written to disk. This test pins
    /// that plain fact: a `manifest.json` written in the pre-`ItemKey` shape
    /// (a bare `kind:name` object key, ordinary string fields) must still
    /// load, and a manifest saved back out must still key `items` by that
    /// same bare string, byte-for-byte the same shape as before this type
    /// existed.
    // spec: DSC-95 STO-50
    #[test]
    fn manifest_format_is_unchanged_by_the_itemkey_refactor() {
        let (base, paths) = tmp_paths();
        let before = r#"{"version":1,"items":{"skill:review":{
            "kind":"skill","name":"review","bare_name":"review",
            "source":"local/test","commit":"abc123","hash":"deadbeef",
            "store":"store/skill/review","links":["/home/x/.claude/skills/review"],
            "description":"a review skill"
        }}}"#;
        std::fs::write(base.join("manifest.json"), before).unwrap();

        let loaded = Manifest::load(&paths).expect("load pre-refactor-shaped manifest");
        // The map is still keyed by the bare `kind:name` string, not by an
        // ItemKey wrapper object or anything else -- a plain `&str` lookup
        // resolves it directly.
        let item = loaded
            .items
            .get("skill:review")
            .expect("bare `kind:name` string key must resolve directly");
        assert_eq!(item.key().as_str(), "skill:review");

        // Re-save and inspect the raw bytes: the round trip must still be a
        // bare-string-keyed items object, byte-for-byte the same shape.
        loaded.save(&paths).expect("save");
        let raw = std::fs::read_to_string(paths.manifest_file()).expect("read back");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
        let items = value
            .get("items")
            .and_then(|v| v.as_object())
            .expect("items must be a JSON object");
        match items.get("skill:review") {
            Some(serde_json::Value::Object(obj)) => {
                assert_eq!(
                    obj.get("name").and_then(|v| v.as_str()),
                    Some("review"),
                    "the item object's own fields must be unaffected: {raw}"
                );
            }
            other => panic!(
                "expected 'skill:review' to key the item object directly (not a wrapper), got {other:?}: {raw}"
            ),
        }

        // A fresh load of the round-tripped file resolves the same item the
        // same way.
        let reloaded = Manifest::load(&paths).expect("reload");
        let reloaded_item = reloaded.items.get("skill:review").expect("item present");
        assert_eq!(reloaded_item.name, item.name);
        assert_eq!(reloaded_item.key().as_str(), item.key().as_str());
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
