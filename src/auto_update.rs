//! Auto-update (spec/auto-update.md): the level a source updates at
//! unattended, the run's state and report, and the harness trigger.
//!
//! The run itself lives in `commands.rs` (`auto_update_run`), next to the
//! sync and upgrade machinery it drives. This module holds the parts that do
//! not need it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};
use crate::paths::Paths;

/// How far a source may move without being asked (AUP-1). Ordered, so the
/// policy cap is a `min` (AUP-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Compatible,
    Breaking,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "off" => Some(Level::Off),
            "compatible" => Some(Level::Compatible),
            "breaking" => Some(Level::Breaking),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Compatible => "compatible",
            Level::Breaking => "breaking",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The level a run applies (AUP-10): what the consumer granted (`off` when
/// nothing was), capped by the managed policy. The maintainer's declaration
/// is only a request (AUP-6), so it plays no part.
// spec: AUP-6 AUP-10 AUP-30
pub fn effective(granted: Option<Level>, cap: Option<Level>) -> Level {
    granted
        .unwrap_or(Level::Off)
        .min(cap.unwrap_or(Level::Breaking))
}

/// A `--if-stale` / `--every` duration in seconds (AUP-19): a positive whole
/// number followed by `m`, `h`, or `d`.
// spec: AUP-19
pub fn parse_duration(s: &str) -> Result<u64> {
    let bad = || MindError::BadDuration {
        value: s.to_string(),
    };
    let unit = s.chars().last().ok_or_else(bad)?;
    let scale = match unit {
        'm' => 60,
        'h' => 60 * 60,
        'd' => 24 * 60 * 60,
        _ => return Err(bad()),
    };
    let n: u64 = s[..s.len() - 1].parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    n.checked_mul(scale).ok_or_else(bad)
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What one run did (AUP-21). Each line is already sanitized.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub time: u64,
    /// `<source> 1.4.2 -> 1.5.0`, one per source moved.
    #[serde(default)]
    pub moved: Vec<String>,
    /// `<source> 1.5.0: <why>`, one per source held for consent (AUP-17).
    #[serde(default)]
    pub held: Vec<String>,
    /// `<source>: <error>`, one per source that failed.
    #[serde(default)]
    pub failed: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.held.is_empty() && self.failed.is_empty()
    }
}

/// The run state kept under `~/.mind` (AUP-19, AUP-21): when the last run
/// completed, and the reports no interactive command has shown yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unshown: Vec<Report>,
}

pub fn state_file(paths: &Paths) -> PathBuf {
    paths.mind_home.join("auto-update.json")
}

pub fn log_file(paths: &Paths) -> PathBuf {
    paths.mind_home.join("auto-update.log")
}

impl State {
    /// The recorded state. A missing or unreadable file is a fresh state: it
    /// only throttles and reports, so it never fails a command.
    pub fn load(paths: &Paths) -> State {
        std::fs::read_to_string(state_file(paths))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        crate::paths::mkdir_p(&paths.mind_home)?;
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| MindError::io(state_file(paths), std::io::Error::other(e)))?;
        Paths::atomic_write(&state_file(paths), &json)
    }

    /// Whether a run is due under `--if-stale <every>` (AUP-19).
    pub fn is_stale(&self, every: u64, now: u64) -> bool {
        self.last_run.is_none_or(|t| now.saturating_sub(t) >= every)
    }
}

/// Append `report` to the run log, one line per entry. Best effort: the log
/// is for the user to read, and losing a line must not fail the run.
pub fn append_log(paths: &Paths, report: &Report) {
    use std::io::Write as _;
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file(paths))
    else {
        return;
    };
    let t = report.time;
    for (what, lines) in [
        ("moved", &report.moved),
        ("held", &report.held),
        ("failed", &report.failed),
    ] {
        for line in lines {
            let _ = writeln!(f, "{t} {what} {line}");
        }
    }
    if report.is_empty() {
        let _ = writeln!(f, "{t} up to date");
    }
}

/// The one-line summary of `reports` an interactive `recall` or `upgrade`
/// prints (AUP-21), `None` when there is nothing to say.
// spec: AUP-21
pub fn summary(reports: &[Report]) -> Option<String> {
    let moved: Vec<&str> = reports
        .iter()
        .flat_map(|r| r.moved.iter().map(String::as_str))
        .collect();
    let held = reports.iter().map(|r| r.held.len()).sum::<usize>();
    let failed = reports.iter().map(|r| r.failed.len()).sum::<usize>();
    let mut parts: Vec<String> = Vec::new();
    if !moved.is_empty() {
        parts.push(moved.join(", "));
    }
    if held > 0 {
        parts.push(format!(
            "{held} source{} held for hook consent (run mind upgrade)",
            if held == 1 { "" } else { "s" }
        ));
    }
    if failed > 0 {
        parts.push(format!(
            "{failed} failure{} (see auto-update.log)",
            if failed == 1 { "" } else { "s" }
        ));
    }
    (!parts.is_empty()).then(|| format!("auto-update: {}", parts.join("; ")))
}

/// Print and clear the reports no interactive command has shown (AUP-21).
/// Best effort, on stderr, so a `--json` document is never touched.
pub fn show_unshown(paths: &Paths) {
    let mut state = State::load(paths);
    if state.unshown.is_empty() {
        return;
    }
    if let Some(line) = summary(&state.unshown) {
        eprintln!("{line}");
    }
    state.unshown.clear();
    let _ = state.save(paths);
}

/// The command the trigger runs (AUP-20). `mind` from PATH rather than this
/// binary's path, which a package manager's upgrade moves.
pub fn trigger_command(every: &str) -> String {
    format!("{TRIGGER_PREFIX} --if-stale {every} --background")
}

/// What marks a hook entry as the one `config auto-update on` added.
const TRIGGER_PREFIX: &str = "mind auto-update run";

fn is_trigger(hook: &serde_json::Value) -> bool {
    hook.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.starts_with(TRIGGER_PREFIX))
}

fn read_settings(file: &Path) -> Result<serde_json::Value> {
    match std::fs::read_to_string(file) {
        Ok(text) if text.trim().is_empty() => Ok(serde_json::json!({})),
        Ok(text) => {
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| MindError::SettingsJson {
                    path: file.to_path_buf(),
                    msg: e.to_string(),
                })?;
            if v.is_object() {
                Ok(v)
            } else {
                Err(MindError::SettingsJson {
                    path: file.to_path_buf(),
                    msg: "the top level is not an object".into(),
                })
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(MindError::io(file, e)),
    }
}

/// Write `v` to `file` atomically. A symlinked `settings.json` (a dotfiles
/// checkout) is written through, so the link survives, and the file keeps its
/// mode, since it may hold tokens under `env`.
fn write_settings(file: &Path, v: &serde_json::Value) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let target = match std::fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::canonicalize(file).map_err(|e| MindError::io(file, e))?
        }
        _ => file.to_path_buf(),
    };
    let dir = target
        .parent()
        .ok_or_else(|| MindError::io(&target, std::io::Error::other("no parent directory")))?;
    crate::paths::mkdir_p(dir)?;
    let mode = std::fs::metadata(&target)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644);
    let mut text = serde_json::to_string_pretty(v)
        .map_err(|e| MindError::io(&target, std::io::Error::other(e)))?;
    text.push('\n');
    let tmp = dir.join(format!(".settings.json.mind.{}", std::process::id()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut f| {
            // `mode` is masked by the umask at creation; set it exactly.
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            f.write_all(text.as_bytes())?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, &target));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        MindError::io(&target, e)
    })
}

/// Install the `SessionStart` trigger in `file` (a lobe's `settings.json`),
/// replacing an earlier one. Returns whether the file changed.
// spec: AUP-20
pub fn install_trigger(file: &Path, every: &str) -> Result<bool> {
    let mut v = read_settings(file)?;
    let before = v.clone();
    strip_trigger(&mut v);
    let entry = serde_json::json!({
        "hooks": [{ "type": "command", "command": trigger_command(every) }]
    });
    let bad = |what: &str| MindError::SettingsJson {
        path: file.to_path_buf(),
        msg: format!("{what} is not an object"),
    };
    let root = v.as_object_mut().ok_or_else(|| bad("the top level"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| bad("\"hooks\""))?;
    let start = hooks
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));
    let list = start
        .as_array_mut()
        .ok_or_else(|| MindError::SettingsJson {
            path: file.to_path_buf(),
            msg: "\"hooks.SessionStart\" is not an array".into(),
        })?;
    list.push(entry);
    if v == before {
        return Ok(false);
    }
    write_settings(file, &v)?;
    Ok(true)
}

/// Remove exactly the trigger `install_trigger` added from `file`, and any
/// container that leaves empty. Returns whether the file changed.
// spec: AUP-20
pub fn remove_trigger(file: &Path) -> Result<bool> {
    if !file.exists() {
        return Ok(false);
    }
    let mut v = read_settings(file)?;
    if !strip_trigger(&mut v) {
        return Ok(false);
    }
    write_settings(file, &v)?;
    Ok(true)
}

/// Whether `file` carries the trigger.
#[cfg(test)]
pub fn has_trigger(file: &Path) -> bool {
    read_settings(file)
        .map(|mut v| strip_trigger(&mut v))
        .unwrap_or(false)
}

/// Drop every trigger hook from `v`, pruning what that empties. Returns
/// whether anything was dropped.
fn strip_trigger(v: &mut serde_json::Value) -> bool {
    let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };
    let Some(list) = hooks.get_mut("SessionStart").and_then(|s| s.as_array_mut()) else {
        return false;
    };
    let mut dropped = false;
    for group in list.iter_mut() {
        if let Some(inner) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let n = inner.len();
            inner.retain(|h| !is_trigger(h));
            dropped |= inner.len() != n;
        }
    }
    if !dropped {
        return false;
    }
    list.retain(|g| {
        g.get("hooks")
            .and_then(|h| h.as_array())
            .is_none_or(|h| !h.is_empty())
    });
    if list.is_empty() {
        hooks.shift_remove("SessionStart");
    }
    if hooks.is_empty()
        && let Some(root) = v.as_object_mut()
    {
        root.shift_remove("hooks");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_effective_level_is_the_grant_capped_by_policy() {
        // spec: AUP-10 AUP-30
        use Level::*;
        assert_eq!(effective(None, None), Off, "nothing granted");
        assert_eq!(effective(Some(Compatible), None), Compatible);
        assert_eq!(effective(Some(Breaking), None), Breaking);
        assert_eq!(effective(Some(Off), None), Off, "revoked");
        assert_eq!(effective(Some(Breaking), Some(Compatible)), Compatible);
        assert_eq!(effective(Some(Compatible), Some(Breaking)), Compatible);
        assert_eq!(effective(Some(Breaking), Some(Off)), Off);
    }

    #[test]
    fn durations_take_minutes_hours_and_days() {
        // spec: AUP-19
        assert_eq!(parse_duration("30m").unwrap(), 1800);
        assert_eq!(parse_duration("12h").unwrap(), 43200);
        assert_eq!(parse_duration("1d").unwrap(), 86400);
        for bad in [
            "",
            "d",
            "0d",
            "1w",
            "1.5h",
            "-1d",
            "1 d",
            "99999999999999999999d",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn staleness_compares_against_the_last_run() {
        // spec: AUP-19
        let fresh = State::default();
        assert!(fresh.is_stale(60, 1000));
        let ran = State {
            last_run: Some(1000),
            ..State::default()
        };
        assert!(!ran.is_stale(60, 1059));
        assert!(ran.is_stale(60, 1060));
    }

    #[test]
    fn the_summary_names_moves_holds_and_failures() {
        // spec: AUP-21
        assert_eq!(summary(&[]), None);
        assert_eq!(summary(&[Report::default()]), None);
        let a = Report {
            moved: vec!["acme/skills 1.4.2 -> 1.5.0".into()],
            held: vec!["x 2.0.0: hook".into()],
            ..Report::default()
        };
        let b = Report {
            moved: vec!["acme/tools 0.3.0 -> 0.3.1".into()],
            failed: vec!["y: boom".into(), "z: boom".into()],
            ..Report::default()
        };
        assert_eq!(
            summary(&[a, b]).unwrap(),
            "auto-update: acme/skills 1.4.2 -> 1.5.0, acme/tools 0.3.0 -> 0.3.1; \
             1 source held for hook consent (run mind upgrade); 2 failures (see auto-update.log)"
        );
    }

    fn temp(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mind-auto-update-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_trigger_writes_through_a_symlink_and_keeps_the_mode() {
        // spec: AUP-20
        use std::os::unix::fs::PermissionsExt as _;
        let dir = temp("settings-link");
        let real = dir.join("dotfiles").join("settings.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "{\"env\": {\"TOKEN\": \"x\"}}\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = dir.join("settings.json");
        std::os::unix::fs::symlink(&real, &file).unwrap();

        assert!(install_trigger(&file, "1d").unwrap());
        let link = std::fs::symlink_metadata(&file).unwrap();
        assert!(link.file_type().is_symlink(), "the link survives");
        assert!(has_trigger(&real), "the target carries the trigger");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the mode is kept");

        assert!(remove_trigger(&file).unwrap());
        assert!(
            std::fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!has_trigger(&real));
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn the_trigger_is_added_beside_other_settings_and_removed_exactly() {
        // spec: AUP-20
        let dir = temp("settings");
        let file = dir.join("settings.json");
        let original = r#"{
  "model": "opus",
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "echo hi" } ] }
    ],
    "Stop": []
  },
  "permissions": { "allow": [] }
}
"#;
        std::fs::write(&file, original).unwrap();
        assert!(install_trigger(&file, "1d").unwrap());
        assert!(has_trigger(&file));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks", "permissions"], "key order kept");
        let start = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "{v}");
        assert_eq!(
            start[1]["hooks"][0]["command"],
            "mind auto-update run --if-stale 1d --background"
        );

        // Re-installing with a new interval replaces the old trigger.
        assert!(install_trigger(&file, "12h").unwrap());
        assert!(!install_trigger(&file, "12h").unwrap(), "already there");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let start = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "{v}");
        assert!(
            start[1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("--if-stale 12h")
        );

        assert!(remove_trigger(&file).unwrap());
        assert!(!remove_trigger(&file).unwrap());
        assert!(!has_trigger(&file));
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let before: serde_json::Value = serde_json::from_str(original).unwrap();
        assert_eq!(after, before, "only the trigger went");

        // A file holding only the trigger goes back to an empty object.
        let only = dir.join("only.json");
        assert!(install_trigger(&only, "1d").unwrap());
        assert!(remove_trigger(&only).unwrap());
        assert_eq!(std::fs::read_to_string(&only).unwrap().trim(), "{}");

        // A settings file that is not an object is refused, not overwritten.
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "[1]").unwrap();
        assert!(install_trigger(&bad, "1d").is_err());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "[1]");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
