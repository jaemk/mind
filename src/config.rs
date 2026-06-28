//! User configuration for `mind`, stored at `~/.mind/config.toml`.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};

use crate::error::{ItemKind, MindError, Result};
use crate::paths::Paths;

/// One agent home ("lobe") in the config, optionally carrying a `kinds` filter
/// (HARN-1). A lobe with no filter (`kinds == None`) admits all linkable kinds,
/// which is the historical behavior; a filtered lobe admits only the listed
/// kinds.
///
/// Serialization is shape-preserving for backward compatibility: a no-`kinds`
/// entry round-trips as a bare string (`"~/.claude"`), exactly as the original
/// `lobes = ["~/.claude"]` config did. A filtered entry serializes as an inline
/// table (`{ path = "~/.gemini", kinds = ["skill", "agent"] }`). On the way in,
/// both a bare string and the table form parse; an unknown table key or an
/// invalid kind string is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobeEntry {
    /// The lobe directory. `~` is expanded at resolution time; stored verbatim.
    pub path: String,
    /// The kinds this lobe admits, or `None` for "all linkable kinds".
    pub kinds: Option<Vec<ItemKind>>,
}

impl LobeEntry {
    /// A lobe path with no kinds filter (admits all kinds).
    pub fn bare(path: impl Into<String>) -> Self {
        LobeEntry {
            path: path.into(),
            kinds: None,
        }
    }

    /// The lobe directory path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The kinds filter, or `None` for "all linkable kinds".
    pub fn kinds(&self) -> Option<&[ItemKind]> {
        self.kinds.as_deref()
    }
}

impl Serialize for LobeEntry {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.kinds {
            // Backward compat: a no-kinds lobe is a bare string, so an existing
            // `lobes = ["~/.claude"]` config round-trips unchanged.
            None => serializer.serialize_str(&self.path),
            Some(kinds) => {
                let mut st = serializer.serialize_struct("LobeEntry", 2)?;
                st.serialize_field("path", &self.path)?;
                let kind_strs: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
                st.serialize_field("kinds", &kind_strs)?;
                st.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for LobeEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LobeEntryVisitor;

        impl<'de> Visitor<'de> for LobeEntryVisitor {
            type Value = LobeEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a lobe path string or a { path, kinds } table")
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<LobeEntry, E>
            where
                E: de::Error,
            {
                Ok(LobeEntry::bare(v.to_string()))
            }

            fn visit_string<E>(self, v: String) -> std::result::Result<LobeEntry, E>
            where
                E: de::Error,
            {
                Ok(LobeEntry::bare(v))
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<LobeEntry, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut path: Option<String> = None;
                let mut kinds: Option<Vec<String>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "path" => {
                            if path.is_some() {
                                return Err(de::Error::duplicate_field("path"));
                            }
                            path = Some(map.next_value()?);
                        }
                        "kinds" => {
                            if kinds.is_some() {
                                return Err(de::Error::duplicate_field("kinds"));
                            }
                            kinds = Some(map.next_value()?);
                        }
                        // deny_unknown_fields semantics on the table variant.
                        other => {
                            return Err(de::Error::unknown_field(other, &["path", "kinds"]));
                        }
                    }
                }
                let path = path.ok_or_else(|| de::Error::missing_field("path"))?;
                let kinds = match kinds {
                    Some(strs) => Some(ItemKind::parse_kinds(&strs).map_err(de::Error::custom)?),
                    None => None,
                };
                Ok(LobeEntry { path, kinds })
            }
        }

        deserializer.deserialize_any(LobeEntryVisitor)
    }
}

/// Persistent settings. Currently just the agent homes ("lobes") to link items
/// into.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Agent homes ("lobes") `mind` links installed items into. Empty means "use
    /// the default" (see [`crate::paths::Paths::agent_homes`]). Each entry is
    /// either a bare path string or a `{ path, kinds }` table (HARN-1). `~` is
    /// expanded at resolution time; entries are stored verbatim.
    #[serde(default)]
    pub lobes: Vec<LobeEntry>,

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare-string lobe entry parses to a no-kinds lobe (backward compat for an
    /// existing `lobes = ["~/.claude"]` config).
    // spec: HARN-1
    #[test]
    fn bare_string_lobe_parses_as_all_kinds() {
        let cfg: Config = toml::from_str("lobes = [\"~/.claude\"]\n").unwrap();
        assert_eq!(cfg.lobes.len(), 1);
        assert_eq!(cfg.lobes[0].path(), "~/.claude");
        assert_eq!(cfg.lobes[0].kinds(), None, "bare entry admits all kinds");
    }

    /// A table lobe entry parses its path and `kinds` filter.
    // spec: HARN-1
    #[test]
    fn table_lobe_parses_kinds() {
        let cfg: Config =
            toml::from_str("lobes = [{ path = \"~/.gemini\", kinds = [\"skill\", \"agent\"] }]\n")
                .unwrap();
        assert_eq!(cfg.lobes.len(), 1);
        assert_eq!(cfg.lobes[0].path(), "~/.gemini");
        assert_eq!(
            cfg.lobes[0].kinds(),
            Some([ItemKind::Skill, ItemKind::Agent].as_slice())
        );
    }

    /// A no-kinds lobe round-trips back to a bare string (so an existing config is
    /// rewritten unchanged), while a filtered lobe round-trips to a table.
    // spec: HARN-1
    #[test]
    fn lobe_entries_round_trip_by_shape() {
        let cfg = Config {
            lobes: vec![
                LobeEntry::bare("~/.claude"),
                LobeEntry {
                    path: "~/.gemini".to_string(),
                    kinds: Some(vec![ItemKind::Skill, ItemKind::Agent]),
                },
            ],
            ..Default::default()
        };
        let text = toml::to_string(&cfg).unwrap();
        // The bare entry serializes as a plain string, not a table.
        assert!(
            text.contains("\"~/.claude\""),
            "bare lobe must serialize as a string: {text}"
        );
        assert!(
            text.contains("kinds"),
            "filtered lobe must serialize with its kinds: {text}"
        );

        // And it parses back to the same value.
        let reparsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(reparsed.lobes, cfg.lobes);
        assert_eq!(reparsed.lobes[0].kinds(), None);
        assert_eq!(
            reparsed.lobes[1].kinds(),
            Some([ItemKind::Skill, ItemKind::Agent].as_slice())
        );
    }

    /// An unknown key inside a lobe table is rejected (deny_unknown_fields on the
    /// table variant).
    // spec: HARN-1
    #[test]
    fn table_lobe_rejects_unknown_key() {
        let err = toml::from_str::<Config>(
            "lobes = [{ path = \"~/.gemini\", kinds = [\"skill\"], bogus = true }]\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("bogus") || err.to_string().contains("unknown"),
            "unknown table key must be rejected: {err}"
        );
    }

    /// An invalid kind string in a lobe table is rejected.
    // spec: HARN-1
    #[test]
    fn table_lobe_rejects_invalid_kind() {
        let err = toml::from_str::<Config>("lobes = [{ path = \"~/.x\", kinds = [\"wizard\"] }]\n")
            .unwrap_err();
        assert!(
            err.to_string().contains("wizard") || err.to_string().contains("valid item kind"),
            "invalid kind must be rejected: {err}"
        );
    }
}
