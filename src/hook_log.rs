//! Hook result records (HOOK-160..HOOK-169): one record per hook `mind`
//! considered in this run (ran, failed, skipped, declined, or aborted).
//!
//! Records are collected for the run's `--json` document (HOOK-161), appended
//! to the capped history file `~/.mind/hooks/history.jsonl` (HOOK-163), and,
//! when output logging is on (`--hook-log`, HOOK-165), point at the file that
//! holds the hook's output.
//!
//! The run state is process-wide, like `json_stdout`: `main` configures it once
//! before dispatch, and every hook site records into it without threading a
//! handle through each verb. A process that never configures it (a unit test)
//! still collects records in memory but writes no history and no logs.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::sanitize::strip_ansi;

/// The history cap when `hook-history` is not set (HOOK-163).
pub const DEFAULT_HISTORY: usize = 100;

/// What happened to one hook (HOOK-160).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// It ran and exited 0.
    Ok,
    /// It ran and exited non-zero or was killed by a signal, or it could not
    /// be started.
    Failed,
    /// It was not run for want of consent: no terminal to ask on and no
    /// `--accept-hooks` for its event.
    Skipped,
    /// The user was asked and chose not to run it.
    Declined,
    /// The user was asked and aborted the operation.
    Aborted,
    /// `mind` was interrupted (Ctrl-C) while the hook ran.
    // spec: HOOK-171
    Interrupted,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Skipped => "skipped",
            Outcome::Declined => "declined",
            Outcome::Aborted => "aborted",
            Outcome::Interrupted => "interrupted",
        }
    }

    /// Whether the hook was started, so its record carries a run status.
    pub fn ran(self) -> bool {
        matches!(self, Outcome::Ok | Outcome::Failed | Outcome::Interrupted)
    }
}

/// Which logs a logging run keeps (HOOK-170).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum LogKeep {
    /// Every hook's log.
    #[default]
    All,
    /// Only the logs of hooks that did not succeed; a successful hook's log is
    /// deleted as soon as it exits.
    Failed,
}

/// One hook's result (HOOK-160). Every source-controlled string is sanitized
/// (DSC-95) when the record is built, so the record can be printed, emitted
/// under `--json`, and persisted without a second pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRecord {
    /// When the hook finished (or was skipped), RFC 3339 UTC.
    pub at: String,
    /// The invocation that considered the hook; shared by every record of one
    /// `mind` process.
    pub run: String,
    /// The verb that considered it (`meld`, `upgrade`, `hooks run`, ...).
    pub verb: String,
    /// `install`, `update`, `uninstall`, or `build`.
    pub event: String,
    /// The heading of the hook's frame (HOOK-30): a source hook's name
    /// (`[[hooks]].name`), else its command (HOOK-51); for an item lifecycle
    /// or build hook, its event.
    pub label: String,
    /// The source identity.
    pub source: String,
    /// `kind:name` for an item hook; absent for a source hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    pub command: String,
    /// The declared commands `command` stood in for, when it is a consumer's
    /// override (HOOK-56); empty, and omitted, otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<String>,
    /// The source commit the hook ran against, when known.
    #[serde(default)]
    pub commit: Option<String>,
    pub outcome: Outcome,
    /// The exit code, for a hook that ran and exited.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// The terminating signal, for a hook killed by one.
    #[serde(default)]
    pub signal: Option<i32>,
    /// Wall-clock run time, for a hook that ran.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Why a hook that never started failed (a spawn error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The file holding the hook's output, when output logging was on.
    #[serde(default)]
    pub log: Option<String>,
}

impl HookRecord {
    /// A one-line status for text output: `exit 0, 2.1s`, `signal 9, 300ms`,
    /// `not started: <reason>`, or the bare outcome for a hook that did not run.
    /// A run under a second is shown in milliseconds, so a fast hook does not
    /// read as `0.0s`.
    pub fn status_text(&self) -> String {
        let secs = self
            .duration_ms
            .map(|ms| {
                if ms < 1000 {
                    format!(", {ms}ms")
                } else {
                    format!(", {:.1}s", ms as f64 / 1000.0)
                }
            })
            .unwrap_or_default();
        match (self.exit_code, self.signal, &self.error) {
            (Some(code), _, _) => format!("exit {code}{secs}"),
            (None, Some(sig), _) => format!("signal {sig}{secs}"),
            (None, None, Some(e)) => format!("not started: {e}"),
            (None, None, None) => self.outcome.as_str().to_string(),
        }
    }
}

/// Where a hook ran from, beyond what `hook::run_hook` already takes: the item
/// it belongs to (for an item hook), the commit it ran against, and the
/// declared commands an override replaced (HOOK-56).
#[derive(Debug, Clone, Copy, Default)]
pub struct Site<'a> {
    pub item: Option<&'a str>,
    pub commit: Option<&'a str>,
    pub replaced: &'a [String],
}

/// What `main` hands [`configure`].
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub verb: String,
    /// The history file, and how many records it keeps. `None` when the cap
    /// is 0 (history off).
    pub history: Option<(PathBuf, usize)>,
    /// Where this run writes hook output logs (HOOK-165), already
    /// [`anchor`]ed; `None` when logging is off. Whether a log is pruned is
    /// decided by where it is, not by how this was set (HOOK-166).
    pub logs: Option<PathBuf>,
    /// Which logs to keep (HOOK-170).
    pub keep: LogKeep,
    /// The owned log directory (HOOK-166), already [`anchor`]ed.
    pub owned_logs: PathBuf,
}

/// `dir` as an absolute path with `.` and `..` resolved lexically (HOOK-165):
/// a record outlives the working directory it was made in, and a path that
/// only appears to lie under the owned log directory (`logs/../..`) must not
/// be read as owned (HOOK-166).
pub fn anchor(dir: &Path) -> PathBuf {
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(dir))
            .unwrap_or_else(|_| dir.to_path_buf())
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[derive(Default)]
struct Run {
    /// Set by [`configure`]. Until then nothing is kept, so a process that
    /// runs hooks without `main` (a unit test) leaves no records behind for
    /// an unrelated test to find.
    configured: bool,
    id: String,
    verb: String,
    history: Option<(PathBuf, usize)>,
    logs: Option<PathBuf>,
    keep: LogKeep,
    owned_logs: PathBuf,
    records: Vec<HookRecord>,
    /// Number of log files opened, for unique names within the run dir.
    seq: usize,
    /// A history write failed and the warning was already printed.
    warned: bool,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

fn with_run<T>(f: impl FnOnce(&mut Run) -> T) -> T {
    let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    let run = guard.get_or_insert_with(|| Run {
        id: run_id(),
        ..Run::default()
    });
    f(run)
}

/// Set this process's run state. Called once by `main` before dispatch.
pub fn configure(cfg: RunConfig) {
    with_run(|run| {
        run.configured = true;
        run.verb = cfg.verb;
        run.history = cfg.history;
        run.logs = cfg.logs;
        run.keep = cfg.keep;
        run.owned_logs = cfg.owned_logs;
    });
}

/// The log directory the `hook-log` config key names (HOOK-165), anchored:
/// `true` or an empty path is the owned directory `owned`, `false` or absent
/// is no logging. A relative path is relative to `mind_home`, the directory
/// holding `config.toml`, not to wherever `mind` runs.
pub fn config_logs(
    setting: Option<&crate::config::HookLogSetting>,
    owned: &Path,
    mind_home: &Path,
) -> Option<PathBuf> {
    use crate::config::HookLogSetting;
    match setting {
        Some(HookLogSetting::Enabled(true)) => Some(owned.to_path_buf()),
        // An empty `hook-log = ""` names no directory, so it means the owned
        // one rather than the working directory.
        Some(HookLogSetting::Dir(dir)) if dir.trim().is_empty() => Some(owned.to_path_buf()),
        Some(HookLogSetting::Dir(dir)) => {
            Some(anchor(&mind_home.join(crate::commands::expand_tilde(dir))))
        }
        Some(HookLogSetting::Enabled(false)) | None => None,
    }
}

/// This run's logging as [`configure`] set it (CLI-241): the log directory,
/// which logs it keeps, and the owned directory. `None` before `configure`.
pub fn logging() -> Option<(Option<PathBuf>, LogKeep, PathBuf)> {
    with_run(|run| {
        run.configured
            .then(|| (run.logs.clone(), run.keep, run.owned_logs.clone()))
    })
}

/// Whether this run keeps only failed hooks' logs (HOOK-170).
pub fn keeps_failed_only() -> bool {
    with_run(|run| run.keep == LogKeep::Failed)
}

/// Delete a log a successful hook does not need (HOOK-170), and its run
/// directory once that is empty.
pub fn discard_log(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// Every record this run produced so far, in order.
pub fn records() -> Vec<HookRecord> {
    with_run(|run| run.records.clone())
}

/// Build a record for a hook that did not run, and file it (HOOK-160). A skip
/// with no terminal is `skipped`; a skip on a terminal is the user's answer,
/// so `declined`.
pub fn not_run(source: &str, site: Site, event: &str, label: &str, command: &str) {
    let outcome = if crate::hook::is_tty() {
        Outcome::Declined
    } else {
        Outcome::Skipped
    };
    file(base(source, site, event, label, command, outcome));
}

/// File a record for a required hook whose prompt was answered with abort.
pub fn aborted(source: &str, site: Site, event: &str, label: &str, command: &str) {
    file(base(source, site, event, label, command, Outcome::Aborted));
}

/// The record for a hook that ran (or failed to start). `run_hook` fills in
/// the exit fields and files it with [`file`].
pub fn base(
    source: &str,
    site: Site,
    event: &str,
    label: &str,
    command: &str,
    outcome: Outcome,
) -> HookRecord {
    let (run, verb) = with_run(|run| (run.id.clone(), run.verb.clone()));
    HookRecord {
        at: String::new(),
        run,
        verb,
        event: event.to_string(),
        label: strip_ansi(label),
        source: strip_ansi(source),
        item: site.item.map(strip_ansi),
        command: strip_ansi(command),
        replaced: site.replaced.iter().map(|c| strip_ansi(c)).collect(),
        commit: site.commit.filter(|c| !c.is_empty()).map(strip_ansi),
        outcome,
        exit_code: None,
        signal: None,
        duration_ms: None,
        error: None,
        log: None,
    }
}

/// Stamp `rec` with the current time, keep it for the run's document, and
/// append it to the history (HOOK-163). A history write failure warns once on
/// stderr and never fails the hook or the verb: the history is a record of
/// what happened, not part of it.
pub fn file(mut rec: HookRecord) {
    rec.at = rfc3339(now_secs());
    let history = with_run(|run| {
        if !run.configured {
            return None;
        }
        run.records.push(rec.clone());
        run.history.clone()
    });
    let Some((path, cap)) = history else {
        return;
    };
    // The owned log root is fixed (`~/.mind/hooks/logs`, beside the history),
    // whether or not this run logs, so an evicted record's log is removed even
    // by a run that did not turn logging on.
    let owned_root = owned_logs_dir_for(&path);
    if let Err(e) = append_history(&path, cap, &rec, &owned_root) {
        let first = with_run(|run| !std::mem::replace(&mut run.warned, true));
        if first {
            eprintln!(
                "warning: could not record hook result in {}: {e}",
                path.display()
            );
        }
    }
}

/// The owned log directory that sits next to a history file.
fn owned_logs_dir_for(history: &Path) -> PathBuf {
    history
        .parent()
        .map(|p| p.join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// The history file under `mind_home` (HOOK-163).
pub fn history_file(mind_home: &Path) -> PathBuf {
    mind_home.join("hooks").join("history.jsonl")
}

/// The owned log directory under `mind_home` (HOOK-166).
pub fn owned_logs_dir(mind_home: &Path) -> PathBuf {
    mind_home.join("hooks").join("logs")
}

/// Read the history, oldest first. A missing file is an empty history; a line
/// that does not parse (a hand edit, a record from a newer `mind`) is dropped
/// rather than failing the read.
pub fn read_history(path: &Path) -> std::io::Result<Vec<HookRecord>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Append `rec` and keep only the newest `cap` records (HOOK-163). The file is
/// rewritten atomically, so a reader never sees a half-written history. Each
/// evicted record's log is deleted when it lives under `owned_root`
/// (HOOK-166); a log the user directed elsewhere is left alone.
fn append_history(
    path: &Path,
    cap: usize,
    rec: &HookRecord,
    owned_root: &Path,
) -> std::io::Result<()> {
    let mut records = read_history(path)?;
    records.push(rec.clone());
    let evicted: Vec<HookRecord> = if records.len() > cap {
        records.drain(..records.len() - cap).collect()
    } else {
        Vec::new()
    };
    let mut out = String::new();
    for r in &records {
        out.push_str(&serde_json::to_string(r).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::paths::Paths::atomic_write_private(path, out.as_bytes())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    for r in evicted {
        // Only a plain `.log` file under the owned root, spelled with no `..`:
        // the history is a file on disk, and a `log` value edited to point
        // elsewhere must not turn eviction into deleting it.
        if let Some(log) = r.log.as_deref().map(Path::new)
            && log.starts_with(owned_root)
            && log.extension().is_some_and(|e| e == "log")
            && !log
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            let _ = std::fs::remove_file(log);
            // The run's directory goes once its last log does.
            if let Some(parent) = log.parent()
                && parent != owned_root
            {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

/// An open output log for one hook (HOOK-165).
pub struct Log {
    pub path: PathBuf,
    pub file: std::fs::File,
}

/// Open the output log for the next hook, when logging is on: one file per
/// hook under `<dir>/<run>/`, named `<seq>-<event>-<target>.log`. A failure to
/// create it warns and runs the hook unlogged (inherited streams) rather than
/// refusing to run it.
pub fn open_log(source: &str, site: Site, event: &str, label: &str, command: &str) -> Option<Log> {
    let (dir, run_id, seq, prune) = with_run(|run| {
        let dir = run.logs.clone()?;
        run.seq += 1;
        // spec: HOOK-166 -- with the history off nothing evicts owned logs, so
        // the run's first owned log clears out the stale ones instead.
        let prune = (run.seq == 1 && run.history.is_none() && dir.starts_with(&run.owned_logs))
            .then(|| run.owned_logs.clone());
        Some((dir, run.id.clone(), run.seq, prune))
    })?;
    if let Some(owned) = prune {
        let cutoff = std::time::SystemTime::now().checked_sub(OWNED_LOG_MAX_AGE);
        if let Some(cutoff) = cutoff {
            prune_stale_logs(&owned, &owned, cutoff);
        }
    }
    // A source hook's name tells several hooks of one source and event apart;
    // an item hook's label is its event, already in the name (HOOK-165).
    let name = match site.item {
        Some(item) => slug(&format!("{source}-{item}")),
        None => format!("{}-{}", slug(source), slug(label)),
    };
    let run_dir = dir.join(&run_id);
    let path = run_dir.join(format!("{seq:03}-{event}-{name}.log"));
    let opened = std::fs::create_dir_all(&run_dir).and_then(|_| {
        let mut file = create_log(&path)?;
        // An item hook's label is its event (HOOK-160), so only a source
        // hook's label is named.
        let (what, named) = match site.item {
            Some(item) => (
                format!("{}#{}", strip_ansi(source), strip_ansi(item)),
                String::new(),
            ),
            None => (strip_ansi(source), format!(" '{}'", strip_ansi(label))),
        };
        writeln!(
            file,
            "# {event} hook{named} for {what}\n# command: {}",
            strip_ansi(command)
        )?;
        Ok(file)
    });
    match opened {
        Ok(file) => Some(Log { path, file }),
        Err(e) => {
            eprintln!(
                "warning: could not open hook log {}: {e}; the hook's output is not saved",
                path.display()
            );
            None
        }
    }
}

/// How old an owned log may get while the history is off (HOOK-166).
const OWNED_LOG_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 86_400);

/// Delete each `.log` file under `dir` last modified before `cutoff`, and each
/// directory below `root` that leaves empty (HOOK-166). Symlinks are neither
/// followed nor deleted, and nothing but a `.log` file is ever removed.
// spec: HOOK-166
fn prune_stale_logs(root: &Path, dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            prune_stale_logs(root, &path, cutoff);
        } else if meta.is_file()
            && path.extension().is_some_and(|e| e == "log")
            && meta.modified().is_ok_and(|m| m < cutoff)
        {
            let _ = std::fs::remove_file(&path);
        }
    }
    if dir != root {
        // Fails, harmlessly, while anything is left in it.
        let _ = std::fs::remove_dir(dir);
    }
}

/// Create a new log file at `path`, readable by its owner only (HOOK-165).
/// `create_new` refuses anything already there, a file or a symlink, so a log
/// directory another user can write to (`--hook-log=/tmp/x`) cannot redirect
/// the write onto, and truncate, a file of the user's; mode 0600 keeps hook
/// output, which can carry tokens, from other users.
// spec: HOOK-165
fn create_log(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// A filename-safe form of `s`: ASCII alphanumerics, `.`, `_`, and `-` kept,
/// everything else `_`, capped at 80 bytes (so a name of two slugs stays
/// well under the usual 255-byte file name limit).
fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(80);
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// This process's run id: a compact UTC timestamp plus the pid, so ids sort by
/// time and two runs in the same second stay distinct.
fn run_id() -> String {
    let (y, mo, d, h, mi, s) = civil(now_secs());
    format!(
        "{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z-{}",
        std::process::id()
    )
}

/// `secs` since the epoch as RFC 3339 UTC (`2026-10-28T14:03:09Z`).
pub fn rfc3339(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Split epoch seconds into a UTC civil date and time (Howard Hinnant's
/// days-to-civil algorithm).
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(mo <= 2);
    (y, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// RAII guard that removes a temp directory when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "mind-hook-log-test-{}-{tag}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rec(n: usize, log: Option<String>) -> HookRecord {
        HookRecord {
            at: format!("t{n}"),
            run: "r".into(),
            verb: "meld".into(),
            event: "install".into(),
            label: format!("h{n}"),
            source: "local/a/b".into(),
            item: None,
            command: "true".into(),
            replaced: Vec::new(),
            commit: None,
            outcome: Outcome::Ok,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(1),
            error: None,
            log,
        }
    }

    // spec: HOOK-163
    #[test]
    fn history_keeps_the_newest_cap_records() {
        let dir = TempDir::new("hook-history-cap");
        let path = dir.path().join("hooks").join("history.jsonl");
        let owned = dir.path().join("hooks").join("logs");
        for n in 0..5 {
            append_history(&path, 3, &rec(n, None), &owned).unwrap();
        }
        let labels: Vec<String> = read_history(&path)
            .unwrap()
            .into_iter()
            .map(|r| r.label)
            .collect();
        assert_eq!(labels, vec!["h2", "h3", "h4"]);
    }

    // spec: HOOK-166
    #[test]
    fn eviction_deletes_owned_logs_but_not_user_logs() {
        let dir = TempDir::new("hook-history-evict");
        let path = dir.path().join("hooks").join("history.jsonl");
        let owned = dir.path().join("hooks").join("logs");
        let run_dir = owned.join("run1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let owned_log = run_dir.join("001-install-x.log");
        std::fs::write(&owned_log, "x").unwrap();
        let user_dir = dir.path().join("elsewhere");
        std::fs::create_dir_all(&user_dir).unwrap();
        let user_log = user_dir.join("001-install-y.log");
        std::fs::write(&user_log, "y").unwrap();

        append_history(
            &path,
            2,
            &rec(0, Some(owned_log.display().to_string())),
            &owned,
        )
        .unwrap();
        append_history(
            &path,
            2,
            &rec(1, Some(user_log.display().to_string())),
            &owned,
        )
        .unwrap();
        append_history(&path, 2, &rec(2, None), &owned).unwrap();
        assert!(!owned_log.exists(), "an evicted owned log must be deleted");
        assert!(!run_dir.exists(), "an emptied run dir must be removed");
        append_history(&path, 2, &rec(3, None), &owned).unwrap();
        assert!(
            user_log.exists(),
            "a log outside the owned dir is never deleted"
        );
    }

    // spec: HOOK-166
    #[test]
    fn eviction_refuses_a_log_path_that_escapes_the_owned_dir() {
        let dir = TempDir::new("hook-history-escape");
        let path = dir.path().join("hooks").join("history.jsonl");
        let owned = dir.path().join("hooks").join("logs");
        std::fs::create_dir_all(&owned).unwrap();
        let victim = dir.path().join("sources.json");
        std::fs::write(&victim, "{}").unwrap();
        let sneaky = owned.join("..").join("..").join("sources.json");
        let not_a_log = owned.join("keep.json");
        std::fs::write(&not_a_log, "{}").unwrap();

        append_history(
            &path,
            1,
            &rec(0, Some(sneaky.display().to_string())),
            &owned,
        )
        .unwrap();
        append_history(
            &path,
            1,
            &rec(1, Some(not_a_log.display().to_string())),
            &owned,
        )
        .unwrap();
        append_history(&path, 1, &rec(2, None), &owned).unwrap();
        assert!(victim.exists(), "a `..` path must not be deleted");
        assert!(not_a_log.exists(), "only `.log` files are deleted");
    }

    // spec: HOOK-165 HOOK-166
    #[test]
    fn anchor_makes_absolute_and_resolves_dot_dot() {
        let got = anchor(Path::new("/a/hooks/logs/../../x/./y"));
        assert_eq!(got, PathBuf::from("/a/x/y"));
        let rel = anchor(Path::new("rel"));
        assert!(rel.is_absolute() && rel.ends_with("rel"), "{rel:?}");
    }

    // spec: HOOK-163
    #[test]
    fn unparseable_history_lines_are_dropped() {
        let dir = TempDir::new("hook-history-junk");
        let path = dir.path().join("history.jsonl");
        let good = serde_json::to_string(&rec(1, None)).unwrap();
        std::fs::write(&path, format!("not json\n{good}\n\n")).unwrap();
        let got = read_history(&path).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "h1");
    }

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_793_196_189), "2026-10-28T14:03:09Z");
    }

    // spec: HOOK-160 HOOK-162
    #[test]
    fn status_text_names_exit_signal_or_spawn_error() {
        let mut r = rec(0, None);
        r.duration_ms = Some(2100);
        assert_eq!(r.status_text(), "exit 0, 2.1s");
        r.exit_code = None;
        r.signal = Some(9);
        assert_eq!(r.status_text(), "signal 9, 2.1s");
        r.duration_ms = Some(12);
        assert_eq!(r.status_text(), "signal 9, 12ms");
        r.signal = None;
        r.duration_ms = None;
        r.error = Some("no such dir".into());
        assert_eq!(r.status_text(), "not started: no such dir");
        r.error = None;
        r.outcome = Outcome::Skipped;
        assert_eq!(r.status_text(), "skipped");
    }

    // spec: HOOK-163
    #[cfg(unix)]
    #[test]
    fn the_history_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new("hook-history-mode");
        let path = dir.path().join("hooks").join("history.jsonl");
        let owned = dir.path().join("hooks").join("logs");
        append_history(&path, 3, &rec(0, None), &owned).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // spec: HOOK-165
    #[test]
    fn a_log_is_created_new_and_private() {
        let dir = TempDir::new("hook-log-create");
        let path = dir.path().join("001-install-x.log");
        create_log(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(create_log(&path).is_err(), "an existing file is not reused");

        #[cfg(unix)]
        {
            let victim = dir.path().join("victim");
            std::fs::write(&victim, "keep me").unwrap();
            let planted = dir.path().join("002-install-x.log");
            std::os::unix::fs::symlink(&victim, &planted).unwrap();
            assert!(
                create_log(&planted).is_err(),
                "a planted symlink is refused"
            );
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        }
    }

    #[test]
    fn slug_keeps_safe_characters_only() {
        assert_eq!(slug("local/a/b-skill:x y"), "local_a_b-skill_x_y");
    }
}
