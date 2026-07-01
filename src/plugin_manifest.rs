//! Plugin manifest parsing for Claude plugin discovery (MKT-1..11).
//!
//! Provides pure parsing and validation for `.claude-plugin/plugin.json`
//! (single plugin, MKT-3) and `.claude-plugin/marketplace.json` (marketplace
//! catalog, MKT-7). No filesystem walking: that is catalog's job (shard 3).
//!
//! # Display sanitization
//! Names and descriptions are returned raw from this module. The consumer
//! (catalog/commands) applies `strip_ansi` at display time (MKT-9). This module
//! does not depend on `commands::strip_ansi`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{MindError, Result};

// ---------------------------------------------------------------------------
// Manifest file locators
// ---------------------------------------------------------------------------

/// Relative path of a single-plugin manifest within its repo/plugin root.
pub const PLUGIN_MANIFEST_SUBPATH: &str = ".claude-plugin/plugin.json";

/// Relative path of a marketplace catalog manifest within its repo root.
pub const MARKETPLACE_MANIFEST_SUBPATH: &str = ".claude-plugin/marketplace.json";

/// Absolute path of the single-plugin manifest under `root`.
pub fn plugin_manifest_path(root: &Path) -> PathBuf {
    root.join(PLUGIN_MANIFEST_SUBPATH)
}

/// Absolute path of the marketplace manifest under `root`.
pub fn marketplace_manifest_path(root: &Path) -> PathBuf {
    root.join(MARKETPLACE_MANIFEST_SUBPATH)
}

/// Returns `Some(path)` when `.claude-plugin/plugin.json` exists under `root`.
pub fn find_plugin_manifest(root: &Path) -> Option<PathBuf> {
    let p = plugin_manifest_path(root);
    p.is_file().then_some(p)
}

/// Returns `Some(path)` when `.claude-plugin/marketplace.json` exists under `root`.
pub fn find_marketplace_manifest(root: &Path) -> Option<PathBuf> {
    let p = marketplace_manifest_path(root);
    p.is_file().then_some(p)
}

// ---------------------------------------------------------------------------
// Safe-path guard (MKT-9, DSC-71..73)
// ---------------------------------------------------------------------------

/// True when `rel` is a safe repo-root-relative or link-target path:
/// non-empty, not absolute, not `~`-rooted, and containing no `..` (parent)
/// component or NUL byte. Subdirectories separated by `/` are allowed.
///
/// Mirrors the private `is_safe_link_rel` function in `catalog.rs` (DSC-72,
/// DSC-73) exactly. Duplicated here because that function is private to its
/// module and this module must stand alone per the shard spec. The logic
/// is attributed to that original (catalog.rs ~line 426).
pub fn is_safe_manifest_path(rel: &str) -> bool {
    if rel.is_empty() || rel.contains('\0') || rel.starts_with('~') {
        return false;
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return false;
    }
    p.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

// ---------------------------------------------------------------------------
// PluginManifest — single plugin (MKT-3, MKT-5, MKT-6)
// ---------------------------------------------------------------------------

/// Deserialized `.claude-plugin/plugin.json`.
///
/// Permissive for optional Claude keys (author, homepage, license, keywords,
/// component overrides such as `commands`, `hooks`, `mcpServers`, `lineStyle`,
/// `outputStyles`, `statusLine`, etc.) that mind does not use — those are
/// simply ignored. Strictness applies only to the required `name` field and
/// to well-formed JSON. Unknown optional fields do not cause a parse error.
///
/// The `name` (MKT-5) is the default effective prefix for that plugin's items.
/// `version` and `description` (MKT-6) are informational metadata.
///
/// NOTE: `name` and `description` are returned raw; the consumer applies
/// `strip_ansi` at display time (MKT-9).
#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    /// The plugin name; doubles as the default namespace prefix (MKT-5).
    pub name: String,
    /// Declared plugin version (informational, MKT-6).
    #[serde(default)]
    pub version: Option<String>,
    /// Plugin description (overrides per-item frontmatter per DSC-32, MKT-6).
    #[serde(default)]
    pub description: Option<String>,
}

/// Load and validate `.claude-plugin/plugin.json` from `path`.
///
/// Returns `MindError::MindToml` when:
/// - the JSON is malformed
/// - `name` is absent or empty/whitespace-only
///
/// Returns `MindError::Io` for I/O failures.
pub fn load_plugin_manifest(path: &Path) -> Result<PluginManifest> {
    let text = std::fs::read_to_string(path).map_err(|e| MindError::io(path, e))?;
    let manifest: PluginManifest =
        serde_json::from_str(&text).map_err(|e| MindError::MindToml {
            path: path.to_path_buf(),
            msg: format!("invalid plugin.json: {e}"),
        })?;
    if manifest.name.trim().is_empty() {
        return Err(MindError::MindToml {
            path: path.to_path_buf(),
            msg: "plugin.json: 'name' must be a non-empty string".to_string(),
        });
    }
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// SkippedComponents (MKT-4)
// ---------------------------------------------------------------------------

/// Counts unsupported component kinds found at a plugin root (MKT-4).
///
/// Claude plugin components that have no `mind` equivalent — `commands/`,
/// `hooks/`, `.mcp.json`/`mcpServers`, LSP servers, monitors, themes, and
/// output-styles — are not installed. This struct holds counts per kind so
/// the caller can render an informative message rather than silently dropping
/// them.
///
/// Populated by the consumer (catalog/commands shard) from what it finds on
/// disk and in the manifest. This module owns the type and its rendering only.
#[derive(Debug, Default)]
pub struct SkippedComponents {
    pub commands: u32,
    pub hooks: u32,
    pub mcp_servers: u32,
    pub lsp_servers: u32,
    pub monitors: u32,
    pub themes: u32,
    pub output_styles: u32,
}

impl SkippedComponents {
    /// Total number of skipped component instances across all kinds.
    pub fn total(&self) -> u32 {
        self.commands
            + self.hooks
            + self.mcp_servers
            + self.lsp_servers
            + self.monitors
            + self.themes
            + self.output_styles
    }

    /// Human-readable summary, e.g. `"2 hooks, 1 mcp server not installed (no
    /// mind equivalent)"`. Returns `None` when nothing was skipped.
    pub fn summary(&self) -> Option<String> {
        if self.total() == 0 {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        Self::push_part(&mut parts, self.commands, "command", "commands");
        Self::push_part(&mut parts, self.hooks, "hook", "hooks");
        Self::push_part(&mut parts, self.mcp_servers, "mcp server", "mcp servers");
        Self::push_part(&mut parts, self.lsp_servers, "lsp server", "lsp servers");
        Self::push_part(&mut parts, self.monitors, "monitor", "monitors");
        Self::push_part(&mut parts, self.themes, "theme", "themes");
        Self::push_part(
            &mut parts,
            self.output_styles,
            "output style",
            "output styles",
        );
        Some(format!(
            "{} not installed (no mind equivalent)",
            parts.join(", ")
        ))
    }

    fn push_part(parts: &mut Vec<String>, count: u32, singular: &str, plural: &str) {
        if count > 0 {
            parts.push(format!(
                "{} {}",
                count,
                if count == 1 { singular } else { plural }
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// MarketplaceManifest — catalog of plugins (MKT-7, MKT-8)
// ---------------------------------------------------------------------------

/// A resolved plugin source: either an in-repo relative path (validated safe)
/// or an external git source spec (validated via `source::parse_spec`).
#[derive(Debug, PartialEq)]
pub enum PluginSource {
    /// A safe-relative path within the catalog repo (validated by
    /// `is_safe_manifest_path`). Fed to catalog as a scan root.
    InRepo { path: String },
    /// A repo spec to hand to `source::parse_spec`. Validated at load time;
    /// the raw spec string is stored for the consumer to re-parse.
    External { spec: String },
}

/// One entry in a marketplace catalog, resolved and validated.
#[derive(Debug)]
pub struct MarketplaceEntry {
    /// Per-entry name (namespaces the plugin's items per MKT-5/MKT-8).
    pub name: String,
    /// Resolved source: in-repo path or external spec.
    pub source: PluginSource,
    /// Declared version (informational, MKT-6/MKT-8).
    pub version: Option<String>,
    /// Description (MKT-6/MKT-8).
    pub description: Option<String>,
    /// Explicit skill paths declared by the entry (MKT-9).
    pub skills: Vec<String>,
}

/// Deserialized and validated `.claude-plugin/marketplace.json`.
#[derive(Debug)]
pub struct MarketplaceManifest {
    /// Top-level marketplace name. Parsed to mirror the manifest schema and to
    /// reject a malformed catalog; not otherwise consumed (mind keys a melded
    /// source by its repo identity, not the catalog's declared name).
    #[allow(dead_code)]
    pub name: String,
    entries: Vec<MarketplaceEntry>,
}

impl MarketplaceManifest {
    /// Consume and return the validated plugin entries.
    pub fn into_entries(self) -> Vec<MarketplaceEntry> {
        self.entries
    }

    /// Borrow the validated plugin entries. Test-only: production consumes the
    /// manifest via [`into_entries`](Self::into_entries).
    #[cfg(test)]
    pub fn entries(&self) -> &[MarketplaceEntry] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// Internal serde types for marketplace.json
// ---------------------------------------------------------------------------

/// Raw top-level marketplace document (permissive: unknown keys ignored).
#[derive(Deserialize)]
struct RawMarketplace {
    name: String,
    #[serde(default)]
    plugins: Vec<RawPluginEntry>,
}

/// One raw plugin entry; optional fields default to None.
#[derive(Deserialize)]
struct RawPluginEntry {
    name: String,
    source: RawSource,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
}

/// The `source` field in a marketplace entry: a string or a JSON object.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawSource {
    Str(String),
    Obj(HashMap<String, Value>),
}

// ---------------------------------------------------------------------------
// load_marketplace_manifest
// ---------------------------------------------------------------------------

/// Load and validate `.claude-plugin/marketplace.json` from `path`.
///
/// Validates every entry:
/// - In-repo paths are checked by `is_safe_manifest_path` (MKT-9/DSC-72).
/// - External specs are round-tripped through `source::parse_spec`; its error
///   is bubbled directly (MKT-9/DSC-66).
/// - Any pin/ref value found in an external source object is checked by
///   `git::validate_ref_value` (MKT-9/DSC-66).
///
/// Returns `MindError::MindToml` (or a bubbled variant) on any validation
/// failure. I/O failures return `MindError::Io`.
pub fn load_marketplace_manifest(path: &Path) -> Result<MarketplaceManifest> {
    let text = std::fs::read_to_string(path).map_err(|e| MindError::io(path, e))?;
    let raw: RawMarketplace = serde_json::from_str(&text).map_err(|e| MindError::MindToml {
        path: path.to_path_buf(),
        msg: format!("invalid marketplace.json: {e}"),
    })?;

    let mut entries = Vec::with_capacity(raw.plugins.len());
    for entry in raw.plugins {
        // spec: MKT-9 — a marketplace entry's name is required and must be
        // non-empty; an absent or whitespace-only name disables namespacing and
        // is rejected with the same MindToml class as load_plugin_manifest uses.
        if entry.name.trim().is_empty() {
            return Err(MindError::MindToml {
                path: path.to_path_buf(),
                msg: "marketplace.json: entry 'name' must be a non-empty string".to_string(),
            });
        }
        let source = resolve_source(entry.source, path)?;
        // Validate each skills path (MKT-9: same safe-path rule as in-repo source paths).
        for skill_path in &entry.skills {
            if !is_safe_manifest_path(skill_path) {
                return Err(MindError::MindToml {
                    path: path.to_path_buf(),
                    msg: format!(
                        "marketplace.json: skills path {:?} is unsafe (absolute, \
                         ~-rooted, contains .., or contains NUL)",
                        skill_path
                    ),
                });
            }
        }
        entries.push(MarketplaceEntry {
            name: entry.name,
            source,
            version: entry.version,
            description: entry.description,
            skills: entry.skills,
        });
    }

    Ok(MarketplaceManifest {
        name: raw.name,
        entries,
    })
}

/// Resolve a raw `source` field to a validated `PluginSource`.
fn resolve_source(raw: RawSource, manifest_path: &Path) -> Result<PluginSource> {
    match raw {
        RawSource::Str(s) => resolve_string_source(s, manifest_path),
        RawSource::Obj(obj) => {
            let spec = extract_external_spec(&obj, manifest_path)?;
            validate_object_pins(&obj)?;
            crate::source::parse_spec(&spec)?;
            Ok(PluginSource::External { spec })
        }
    }
}

/// Classify and validate a string `source` value.
///
/// Classification rules:
/// - Contains `://` (http/https/git URLs), starts with `git@`, or starts with
///   `github:` -> External.
/// - Matches bare `owner/repo` (exactly one `/`, both parts non-empty, second
///   part has no further `/`) -> External (github shorthand).
/// - Starts with `.`, `/`, or `~` -> InRepo (absolute paths fail safety check).
/// - Multi-segment paths (more than one `/`) or bare names (no `/`) -> InRepo.
fn resolve_string_source(s: String, manifest_path: &Path) -> Result<PluginSource> {
    if is_external_string(&s) {
        // Validate through parse_spec (bubbles InvalidRepoSpec on garbage).
        crate::source::parse_spec(&s)?;
        return Ok(PluginSource::External { spec: s });
    }

    // In-repo relative path: validate with the safe-path rule (MKT-9).
    if !is_safe_manifest_path(&s) {
        return Err(MindError::MindToml {
            path: manifest_path.to_path_buf(),
            msg: format!(
                "marketplace.json: in-repo plugin path {:?} is unsafe (absolute, \
                 ~-rooted, contains .., or contains NUL)",
                s
            ),
        });
    }
    Ok(PluginSource::InRepo { path: s })
}

/// True when the string looks like an external git source spec rather than a
/// repo-relative path.
fn is_external_string(s: &str) -> bool {
    // URL schemes (http://, https://, git://, etc.).
    if s.contains("://") {
        return true;
    }
    // SSH form: git@host:owner/repo.
    if s.starts_with("git@") {
        return true;
    }
    // Explicit github: prefix shorthand.
    if s.starts_with("github:") {
        return true;
    }
    // Relative-path or absolute-path prefixes -> in-repo (or rejected by safety).
    if s.starts_with('.') || s.starts_with('/') || s.starts_with('~') {
        return false;
    }
    // Bare owner/repo: exactly one '/', both parts non-empty, second has no
    // further '/'. Matches the `owner/repo` github shorthand that parse_spec
    // accepts.
    if let Some((owner, rest)) = s.split_once('/')
        && !owner.is_empty()
        && !rest.is_empty()
        && !rest.contains('/')
    {
        return true;
    }
    // Multi-segment paths (more than one '/') and bare names (no '/') -> InRepo.
    false
}

/// Extract an external spec string from a source object.
///
/// Priority: `url` field > `repo` field (treated as owner/repo shorthand).
/// Returns `MindError::MindToml` if neither key is present or has a string value.
fn extract_external_spec(obj: &HashMap<String, Value>, manifest_path: &Path) -> Result<String> {
    if let Some(Value::String(url)) = obj.get("url") {
        return Ok(url.clone());
    }
    if let Some(Value::String(repo)) = obj.get("repo") {
        return Ok(repo.clone());
    }
    Err(MindError::MindToml {
        path: manifest_path.to_path_buf(),
        msg: "marketplace.json: external source object must have a 'url' or 'repo' field"
            .to_string(),
    })
}

/// Validate any pin/ref fields embedded in a source object (MKT-9, DSC-66).
fn validate_object_pins(obj: &HashMap<String, Value>) -> Result<()> {
    for key in &["ref", "pin-ref", "pin-tag", "follow-branch", "branch"] {
        if let Some(Value::String(val)) = obj.get(*key) {
            crate::git::validate_ref_value(val)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Write `content` to a uniquely named temp file; return its path.
    /// The caller is responsible for cleanup (or the OS reclaims it).
    fn write_temp(content: &str, label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("mind-pm-{}-{}-{n}.json", std::process::id(), label));
        std::fs::write(&path, content).expect("write temp file");
        path
    }

    // ---------------------------------------------------------------------------
    // Path helpers
    // ---------------------------------------------------------------------------

    #[test]
    fn plugin_manifest_path_appends_subpath() {
        let root = Path::new("/my/repo");
        assert_eq!(
            plugin_manifest_path(root),
            Path::new("/my/repo/.claude-plugin/plugin.json")
        );
    }

    #[test]
    fn marketplace_manifest_path_appends_subpath() {
        let root = Path::new("/my/repo");
        assert_eq!(
            marketplace_manifest_path(root),
            Path::new("/my/repo/.claude-plugin/marketplace.json")
        );
    }

    #[test]
    fn find_plugin_manifest_returns_none_when_absent() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pm-find-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(find_plugin_manifest(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_plugin_manifest_returns_some_when_present() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pm-find2-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join(".claude-plugin");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let manifest = sub.join("plugin.json");
        std::fs::write(&manifest, r#"{"name":"x"}"#).expect("write");
        assert_eq!(find_plugin_manifest(&dir), Some(manifest));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_marketplace_manifest_returns_none_when_absent() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pm-mfind-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(find_marketplace_manifest(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_marketplace_manifest_returns_some_when_present() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mind-pm-mfind2-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join(".claude-plugin");
        std::fs::create_dir_all(&sub).expect("mkdir");
        let manifest = sub.join("marketplace.json");
        std::fs::write(&manifest, r#"{"name":"M","plugins":[]}"#).expect("write");
        assert_eq!(find_marketplace_manifest(&dir), Some(manifest));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------------------
    // is_safe_manifest_path
    // ---------------------------------------------------------------------------

    #[test]
    fn safe_path_accepts_valid_paths() {
        for path in &[
            "foo",
            "a/b/c.md",
            "plugins/myplugin",
            "deep/nested/path/file.txt",
            "./relative",
        ] {
            assert!(is_safe_manifest_path(path), "expected safe for {path:?}");
        }
    }

    #[test]
    fn safe_path_rejects_empty() {
        assert!(!is_safe_manifest_path(""), "empty string must be rejected");
    }

    #[test]
    fn safe_path_rejects_absolute() {
        assert!(
            !is_safe_manifest_path("/abs/path"),
            "absolute path must be rejected"
        );
        assert!(!is_safe_manifest_path("/"), "root slash must be rejected");
    }

    #[test]
    fn safe_path_rejects_tilde_root() {
        assert!(
            !is_safe_manifest_path("~/x"),
            "tilde-rooted path must be rejected"
        );
        assert!(!is_safe_manifest_path("~"), "bare tilde must be rejected");
    }

    #[test]
    fn safe_path_rejects_dotdot() {
        assert!(!is_safe_manifest_path(".."), ".. must be rejected");
        assert!(!is_safe_manifest_path("../up"), "../up must be rejected");
        assert!(!is_safe_manifest_path("a/../b"), "a/../b must be rejected");
    }

    #[test]
    fn safe_path_rejects_nul_byte() {
        assert!(!is_safe_manifest_path("a\0b"), "NUL byte must be rejected");
    }

    // ---------------------------------------------------------------------------
    // load_plugin_manifest
    // ---------------------------------------------------------------------------

    #[test]
    fn plugin_manifest_valid_full() {
        let path = write_temp(
            r#"{"name":"myplugin","version":"1.2.3","description":"A plugin"}"#,
            "pm-full",
        );
        let m = load_plugin_manifest(&path).expect("should parse");
        assert_eq!(m.name, "myplugin");
        assert_eq!(m.version.as_deref(), Some("1.2.3"));
        assert_eq!(m.description.as_deref(), Some("A plugin"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_valid_name_only() {
        let path = write_temp(r#"{"name":"minimal"}"#, "pm-min");
        let m = load_plugin_manifest(&path).expect("should parse");
        assert_eq!(m.name, "minimal");
        assert!(m.version.is_none());
        assert!(m.description.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_missing_name_is_mind_toml_error() {
        let path = write_temp(r#"{"version":"1.0","description":"no name"}"#, "pm-noname");
        let err = load_plugin_manifest(&path).unwrap_err();
        match err {
            MindError::MindToml { msg, .. } => {
                assert!(
                    msg.contains("plugin.json") || msg.contains("name") || msg.contains("missing"),
                    "error must mention the problem: {msg}"
                );
            }
            other => panic!("expected MindToml, got: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_empty_name_is_mind_toml_error() {
        let path = write_temp(r#"{"name":""}"#, "pm-emptyname");
        let err = load_plugin_manifest(&path).unwrap_err();
        match err {
            MindError::MindToml { msg, .. } => {
                assert!(msg.contains("name"), "error must mention 'name': {msg}");
            }
            other => panic!("expected MindToml, got: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_whitespace_name_is_mind_toml_error() {
        let path = write_temp(r#"{"name":"   "}"#, "pm-wsname");
        let err = load_plugin_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "whitespace-only name must be MindToml error: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_malformed_json_is_mind_toml_error() {
        let path = write_temp(r#"{not valid json"#, "pm-badjson");
        let err = load_plugin_manifest(&path).unwrap_err();
        match err {
            MindError::MindToml { msg, .. } => {
                assert!(
                    msg.contains("plugin.json"),
                    "error should mention plugin.json: {msg}"
                );
            }
            other => panic!("expected MindToml for malformed JSON, got: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plugin_manifest_extra_optional_keys_parse_ok() {
        // Claude's plugin.json legitimately carries many optional keys that mind
        // does not use. They must be silently ignored, not cause an error.
        let path = write_temp(
            r#"{
                "name": "rich-plugin",
                "version": "0.1.0",
                "description": "A rich plugin",
                "author": "Alice",
                "homepage": "https://example.com",
                "license": "MIT",
                "keywords": ["skill", "agent"],
                "mcpServers": { "tools": { "command": "npx" } },
                "hooks": { "install": "npm install" },
                "commands": ["cmd1", "cmd2"],
                "lineStyle": "round",
                "outputStyles": {}
            }"#,
            "pm-rich",
        );
        let m = load_plugin_manifest(&path).expect("extra optional keys must be ignored");
        assert_eq!(m.name, "rich-plugin");
        assert_eq!(m.version.as_deref(), Some("0.1.0"));
        assert_eq!(m.description.as_deref(), Some("A rich plugin"));
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------------------
    // load_marketplace_manifest
    // ---------------------------------------------------------------------------

    fn make_marketplace(plugins_json: &str) -> String {
        format!(r#"{{"name":"Test Market","plugins":[{plugins_json}]}}"#)
    }

    #[test]
    fn marketplace_in_repo_dotslash_string_source() {
        let json = make_marketplace(r#"{"name":"p1","source":"./plugins/p1"}"#);
        let path = write_temp(&json, "mkt-inrepo");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert_eq!(m.name, "Test Market");
        let entries = m.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "p1");
        assert!(
            matches!(&entries[0].source, PluginSource::InRepo { path } if path == "./plugins/p1"),
            "expected InRepo, got {:?}",
            entries[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_in_repo_multisegment_path_source() {
        // A path with more than one '/' is treated as InRepo (not owner/repo).
        let json = make_marketplace(r#"{"name":"p1","source":"plugins/sub/p1"}"#);
        let path = write_temp(&json, "mkt-multiseg");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert!(
            matches!(&m.entries()[0].source, PluginSource::InRepo { path } if path == "plugins/sub/p1"),
            "multi-segment path must be InRepo, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_external_url_string_source() {
        let json = make_marketplace(r#"{"name":"p2","source":"https://github.com/owner/plugin"}"#);
        let path = write_temp(&json, "mkt-url");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert!(
            matches!(
                &m.entries()[0].source,
                PluginSource::External { spec } if spec == "https://github.com/owner/plugin"
            ),
            "URL source must be External, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_external_owner_repo_string_source() {
        let json = make_marketplace(r#"{"name":"p3","source":"owner/plugin-repo"}"#);
        let path = write_temp(&json, "mkt-ownerrepo");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert!(
            matches!(
                &m.entries()[0].source,
                PluginSource::External { spec } if spec == "owner/plugin-repo"
            ),
            "owner/repo must be External, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_external_object_form_with_url() {
        let json =
            make_marketplace(r#"{"name":"p4","source":{"url":"https://github.com/owner/repo"}}"#);
        let path = write_temp(&json, "mkt-obj-url");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert!(
            matches!(
                &m.entries()[0].source,
                PluginSource::External { spec } if spec == "https://github.com/owner/repo"
            ),
            "object with url must be External, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_external_object_form_with_repo() {
        // Claude's { "source": "github", "repo": "owner/name" } pattern.
        let json =
            make_marketplace(r#"{"name":"p5","source":{"source":"github","repo":"owner/plugin"}}"#);
        let path = write_temp(&json, "mkt-obj-repo");
        let m = load_marketplace_manifest(&path).expect("should parse");
        assert!(
            matches!(
                &m.entries()[0].source,
                PluginSource::External { spec } if spec == "owner/plugin"
            ),
            "object with repo must be External via owner/repo, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_unsafe_in_repo_path_is_rejected() {
        // ../escape starts with '.' -> classified in-repo, then fails safety check.
        let json = make_marketplace(r#"{"name":"bad","source":"../escape"}"#);
        let path = write_temp(&json, "mkt-unsafe");
        let err = load_marketplace_manifest(&path).unwrap_err();
        match err {
            MindError::MindToml { msg, .. } => {
                assert!(
                    msg.contains("unsafe") || msg.contains("..") || msg.contains("escape"),
                    "error must describe the safety violation: {msg}"
                );
            }
            other => panic!("expected MindToml for unsafe path, got: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_absolute_in_repo_path_is_rejected() {
        // /absolute starts with '/' -> is_external_string returns false,
        // then is_safe_manifest_path returns false (absolute path).
        let json = make_marketplace(r#"{"name":"bad","source":"/absolute/path"}"#);
        let path = write_temp(&json, "mkt-abs");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "absolute in-repo path must be MindToml error: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_garbage_external_spec_is_error() {
        // "github:badspec" starts with "github:" -> External, then parse_spec
        // returns InvalidRepoSpec because "badspec" has no '/'.
        let json = make_marketplace(r#"{"name":"bad","source":"github:badspec"}"#);
        let path = write_temp(&json, "mkt-garbage");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRepoSpec { .. }),
            "garbage spec must bubble InvalidRepoSpec: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_entry_metadata_preserved() {
        let json = make_marketplace(
            r#"{"name":"p","source":"./p","version":"2.0","description":"A plugin"}"#,
        );
        let path = write_temp(&json, "mkt-meta");
        let m = load_marketplace_manifest(&path).expect("should parse");
        let e = &m.entries()[0];
        assert_eq!(e.version.as_deref(), Some("2.0"));
        assert_eq!(e.description.as_deref(), Some("A plugin"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_empty_plugins_list_is_ok() {
        let json = r#"{"name":"Empty Market","plugins":[]}"#;
        let path = write_temp(json, "mkt-empty");
        let m = load_marketplace_manifest(&path).expect("empty plugins list is valid");
        assert_eq!(m.name, "Empty Market");
        assert!(m.entries().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_malformed_json_is_mind_toml_error() {
        let path = write_temp(r#"{not json"#, "mkt-badjson");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "malformed JSON must be MindToml error: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_object_source_bad_ref_pin_is_rejected() {
        // spec: MKT-9
        // DSC-66: a pin/ref value embedded in an external source OBJECT is run
        // through git::validate_ref_value. A `..` (ambiguous git range) must be
        // rejected so a melded marketplace cannot smuggle a dangerous ref. The
        // existing suite only covers a bad *string* spec (github:badspec); the
        // object-form pin-validation path was untested.
        let json =
            make_marketplace(r#"{"name":"p","source":{"url":"https://x/y.git","ref":"a..b"}}"#);
        let path = write_temp(&json, "mkt-obj-badref");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRef { .. }),
            "an object source with a '..'-bearing ref must bubble InvalidRef: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_object_source_leading_dash_branch_is_rejected() {
        // spec: MKT-9
        // DSC-66: a `branch` value that looks like a git option (leading '-')
        // must be rejected by validate_object_pins, not passed through to a git
        // invocation. Covers a different pin key ("branch") and reject reason
        // than the `..` case above.
        let json =
            make_marketplace(r#"{"name":"p","source":{"repo":"owner/plugin","branch":"-evil"}}"#);
        let path = write_temp(&json, "mkt-obj-dashbranch");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRef { .. }),
            "a branch beginning with '-' must bubble InvalidRef: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_object_source_valid_ref_pin_is_accepted() {
        // spec: MKT-9
        // The pin-validation guard must NOT reject a well-formed ref: a normal
        // tag/branch value passes and the entry resolves to External. This pins
        // that the guard is precise (rejects only bad values), not a blanket ban
        // on the pin keys.
        let json = make_marketplace(
            r#"{"name":"p","source":{"url":"https://github.com/owner/repo","ref":"v1.2.3"}}"#,
        );
        let path = write_temp(&json, "mkt-obj-goodref");
        let m = load_marketplace_manifest(&path).expect("a valid ref must parse");
        assert!(
            matches!(
                &m.entries()[0].source,
                PluginSource::External { spec } if spec == "https://github.com/owner/repo"
            ),
            "object with url + valid ref must be External, got {:?}",
            m.entries()[0].source
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_object_source_missing_url_and_repo_is_error() {
        // An object without "url" or "repo" has no usable spec.
        let json = make_marketplace(r#"{"name":"p","source":{"kind":"git"}}"#);
        let path = write_temp(&json, "mkt-obj-bad");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "object source without url/repo must be MindToml: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn marketplace_into_entries_consumes() {
        let json = make_marketplace(r#"{"name":"p","source":"./p"}"#);
        let path = write_temp(&json, "mkt-consume");
        let m = load_marketplace_manifest(&path).expect("should parse");
        let entries = m.into_entries();
        assert_eq!(entries.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    // spec: MKT-9
    // A marketplace entry with an empty name must be rejected with MindToml,
    // matching the same guard applied by load_plugin_manifest. A valid sibling
    // entry in the same list does not suppress the error.
    #[test]
    fn marketplace_entry_empty_name_is_rejected() {
        let json = format!(
            r#"{{"name":"Test Market","plugins":[{},{}]}}"#,
            r#"{"name":"good","source":"./good"}"#, r#"{"name":"","source":"./bad"}"#,
        );
        let path = write_temp(&json, "mkt-emptyname");
        let err = load_marketplace_manifest(&path).unwrap_err();
        match err {
            MindError::MindToml { msg, .. } => {
                assert!(msg.contains("name"), "error must mention 'name': {msg}");
            }
            other => panic!("expected MindToml for empty entry name, got: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    // spec: MKT-9
    // A marketplace entry whose name is whitespace-only (e.g. "  ") must also be
    // rejected; the trim() guard catches this case just as it does in
    // load_plugin_manifest.
    #[test]
    fn marketplace_entry_whitespace_name_is_rejected() {
        let json = make_marketplace(r#"{"name":"  ","source":"./p"}"#);
        let path = write_temp(&json, "mkt-wsname");
        let err = load_marketplace_manifest(&path).unwrap_err();
        assert!(
            matches!(err, MindError::MindToml { .. }),
            "whitespace-only entry name must be MindToml error: {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------------------
    // SkippedComponents::summary
    // ---------------------------------------------------------------------------

    #[test]
    fn skipped_none_returns_none() {
        let s = SkippedComponents::default();
        assert!(s.summary().is_none(), "no skipped -> None");
    }

    #[test]
    fn skipped_one_hook_singular() {
        let s = SkippedComponents {
            hooks: 1,
            ..Default::default()
        };
        let summary = s.summary().expect("one hook -> Some");
        assert!(
            summary.contains("1 hook"),
            "should say '1 hook' (singular): {summary}"
        );
        assert!(
            !summary.contains("1 hooks"),
            "should NOT pluralize for count=1: {summary}"
        );
        assert!(
            summary.contains("not installed"),
            "should say 'not installed': {summary}"
        );
    }

    #[test]
    fn skipped_multiple_hooks_plural() {
        let s = SkippedComponents {
            hooks: 3,
            ..Default::default()
        };
        let summary = s.summary().expect("3 hooks -> Some");
        assert!(
            summary.contains("3 hooks"),
            "should say '3 hooks' (plural): {summary}"
        );
    }

    #[test]
    fn skipped_one_mcp_server_singular() {
        let s = SkippedComponents {
            mcp_servers: 1,
            ..Default::default()
        };
        let summary = s.summary().expect("one mcp server -> Some");
        assert!(
            summary.contains("1 mcp server"),
            "should say '1 mcp server' (singular): {summary}"
        );
        assert!(
            !summary.contains("1 mcp servers"),
            "should NOT pluralize mcp server for count=1: {summary}"
        );
    }

    #[test]
    fn skipped_multiple_kinds_comma_joined() {
        let s = SkippedComponents {
            commands: 2,
            hooks: 1,
            mcp_servers: 3,
            ..Default::default()
        };
        let summary = s.summary().expect("multiple kinds -> Some");
        assert!(
            summary.contains("2 commands"),
            "should include commands: {summary}"
        );
        assert!(
            summary.contains("1 hook"),
            "should include hook (singular): {summary}"
        );
        assert!(
            summary.contains("3 mcp servers"),
            "should include mcp servers: {summary}"
        );
        assert!(
            summary.contains(", "),
            "multiple parts must be comma-joined: {summary}"
        );
        assert!(
            summary.contains("not installed (no mind equivalent)"),
            "must include standard suffix: {summary}"
        );
    }

    #[test]
    fn skipped_output_styles_plural() {
        let s = SkippedComponents {
            output_styles: 2,
            ..Default::default()
        };
        let summary = s.summary().expect("output styles -> Some");
        assert!(
            summary.contains("2 output styles"),
            "should say '2 output styles': {summary}"
        );
    }

    #[test]
    fn skipped_all_kinds_renders_all() {
        let s = SkippedComponents {
            commands: 1,
            hooks: 1,
            mcp_servers: 1,
            lsp_servers: 1,
            monitors: 1,
            themes: 1,
            output_styles: 1,
        };
        let summary = s.summary().expect("all kinds -> Some");
        for expected in &[
            "command",
            "hook",
            "mcp server",
            "lsp server",
            "monitor",
            "theme",
            "output style",
        ] {
            assert!(
                summary.contains(expected),
                "summary must contain {expected:?}: {summary}"
            );
        }
    }
}
