//! Structured error types for `mind`.
//!
//! Every fallible operation returns [`Result<T>`], which carries a [`MindError`].
//! We deliberately avoid stringly-typed errors (e.g. `anyhow`) so callers and
//! tests can match on the precise failure and so messages stay consistent.

use std::path::PathBuf;
use std::process::ExitStatus;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, MindError>;

/// The item kinds `mind` knows how to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemKind {
    Skill,
    Agent,
    Rule,
    /// Helper tooling (scripts or a compiled binary) other items reference. A
    /// tool installs to the store but is not linked into an agent home by
    /// default: the harness does not discover it; items reach it by path token.
    Tool,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Skill => "skill",
            ItemKind::Agent => "agent",
            ItemKind::Rule => "rule",
            ItemKind::Tool => "tool",
        }
    }

    /// Parse a kind from its lowercase string form.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "skill" => Some(ItemKind::Skill),
            "agent" => Some(ItemKind::Agent),
            "rule" => Some(ItemKind::Rule),
            "tool" => Some(ItemKind::Tool),
            _ => None,
        }
    }

    /// The plural directory name for this kind, used by the source-repo
    /// convention layout, the `~/.claude` link layout, and `~/.mind/store`
    /// (`skills`/`agents`/`rules`/`tools`). The single source of truth for the
    /// kind-to-directory mapping; `from_dir` is its inverse.
    pub fn dir(self) -> &'static str {
        match self {
            ItemKind::Skill => "skills",
            ItemKind::Agent => "agents",
            ItemKind::Rule => "rules",
            ItemKind::Tool => "tools",
        }
    }

    /// The kind for a plural directory name, the inverse of [`dir`](Self::dir).
    pub fn from_dir(s: &str) -> Option<Self> {
        match s {
            "skills" => Some(ItemKind::Skill),
            "agents" => Some(ItemKind::Agent),
            "rules" => Some(ItemKind::Rule),
            "tools" => Some(ItemKind::Tool),
            _ => None,
        }
    }

    /// The kinds linked into an agent home: every kind except `Tool`, which is
    /// store-only and reached by reference (tooling.md TOOL-3).
    pub const LINKABLE: [ItemKind; 3] = [ItemKind::Skill, ItemKind::Agent, ItemKind::Rule];
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// All the ways a `mind` operation can fail.
#[derive(Debug, thiserror::Error)]
pub enum MindError {
    #[error("could not locate the home directory")]
    HomeDirNotFound,

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to (de)serialize {what}: {source}")]
    Json {
        what: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid mind.toml at {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to write {path}: {source}")]
    TomlWrite {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },

    #[error("'{path}' is not a configured agent home (lobe)")]
    UnknownLobe { path: String },

    #[error("mind.toml at {path}: {msg}")]
    MindToml { path: PathBuf, msg: String },

    #[error(
        "'{spec}' is not a valid repo spec (expected 'owner/repo', a github shorthand, or a git URL)"
    )]
    InvalidRepoSpec { spec: String },

    #[error(
        "'{name}' is not a valid item ref (expected 'name', 'skill:name', 'agent:name', 'rule:name', or 'owner/repo#name')"
    )]
    InvalidItemRef { name: String },

    #[error("source '{name}' is already melded (from {url})")]
    SourceExists { name: String, url: String },

    #[error("no source named '{name}' is melded")]
    SourceNotFound { name: String },

    #[error("'{pattern}' is not a valid glob selector: {source}")]
    InvalidPattern {
        pattern: String,
        #[source]
        source: glob::PatternError,
    },

    #[error("'{query}' matches multiple sources: {}; use the full owner/repo", candidates.join(", "))]
    AmbiguousSource {
        query: String,
        candidates: Vec<String>,
    },

    #[error(
        "no item matches '{query}' across {sources} melded source(s); run `mind sync` then `mind probe`"
    )]
    ItemNotFound { query: String, sources: usize },

    #[error("'{query}' is ambiguous; matches: {}", candidates.join(", "))]
    AmbiguousItem {
        query: String,
        candidates: Vec<String>,
    },

    #[error("'{name}' is not installed")]
    NotInstalled { name: String },

    #[error("sync failed for {failed} of {total} source(s); see the messages above")]
    SyncFailed { failed: usize, total: usize },

    #[error(
        "source '{source_name}' requires mind >= {required}, but this is mind {running}; upgrade mind"
    )]
    IncompatibleVersion {
        source_name: String,
        required: String,
        running: String,
    },

    #[error(
        "{path} already exists and is not managed by mind; remove it (or `mind forget` the item) before installing"
    )]
    LinkOccupied { path: String },

    #[error("{item}: reference {referent} does not match any item in source '{in_source}'")]
    BadReference {
        item: String,
        /// The offending token as written, e.g. `{{ns:foo}}` or `{{tools:bar}}`.
        referent: String,
        in_source: String,
    },

    #[error("git {} failed for {url}{}: {}",
        args.join(" "),
        status_suffix(*status),
        if stderr.is_empty() { "<no stderr>" } else { stderr })]
    Git {
        url: String,
        args: Vec<String>,
        status: Option<ExitStatus>,
        stderr: String,
    },

    #[error("git executable not found on PATH; install git to meld and sync sources")]
    GitNotFound,

    #[error(
        "conflicting pin flags: {first} and {second} cannot both be given; supply at most one of --follow-branch, --pin-tag, --pin-ref"
    )]
    ConflictingPin { first: String, second: String },

    #[error("source '{source_name}': scan root '{root}' is not a directory in the clone")]
    InvalidRoot { source_name: String, root: String },

    #[error(
        "source '{source_name}': {kind} '{name}' appears under more than one scan root; roots must not yield the same item"
    )]
    DuplicateItem {
        source_name: String,
        kind: ItemKind,
        name: String,
    },

    #[error("review found {hard} hard error(s); see the findings above")]
    ReviewFailed { hard: usize },

    // Constructed by the policy-enforcement paths (meld/sync/upgrade gating);
    // until those land nothing builds these, so allow dead_code on just them.
    #[error("source '{identity}' is not permitted by the managed policy's allowlist")]
    SourceNotAllowed { identity: String },

    #[error(
        "source '{identity}' must be pinned to a tag or ref: the managed policy forbids floating branches"
    )]
    UnpinnedSourceForbidden { identity: String },

    #[error("invalid managed policy at {path}: {reason}")]
    InvalidPolicy { path: String, reason: String },

    #[error(
        "the agent homes are locked by the managed policy ([lobes].lock); `config lobes {action}` is refused"
    )]
    LobesLocked { action: String },

    #[error(
        "install hook for source '{identity}' failed{}: {}\n  command: {command}",
        status_suffix(*status),
        if stderr.is_empty() { "(no output)" } else { stderr }
    )]
    HookFailed {
        identity: String,
        command: String,
        status: Option<ExitStatus>,
        stderr: String,
    },

    #[error("no prebuilt `mind` binary for this platform ({os}/{arch}); build from source instead")]
    UnsupportedPlatform { os: String, arch: String },

    #[error("failed to download {url}: {reason}")]
    DownloadFailed { url: String, reason: String },

    #[error("the downloaded release archive did not contain a 'mind' binary")]
    ReleaseAssetEmpty,

    #[error(
        "cannot replace the running binary at {path}: it is not writable; reinstall with elevated privileges (e.g. sudo) or, for a Homebrew install, run `brew upgrade mind`"
    )]
    TargetNotWritable { path: String },

    #[error("'{path}' is not a directory")]
    NotADirectory { path: String },

    #[error("{action} needs confirmation; re-run with --yes (or in an interactive terminal)")]
    ConfirmationRequired { action: String },

    /// ABS-5: the destination path exists but is not a git repository.
    #[error(
        "'{path}' is not a git repository; absorb requires a git destination (use --to to choose one)"
    )]
    DestinationNotRepo { path: String },

    /// ABS-6: the destination already contains an item at the convention path.
    #[error("destination already has {kind}:{name} at {dest_path}; use --force to overwrite")]
    AbsorbCollision {
        kind: String,
        name: String,
        dest_path: String,
    },
}

fn status_suffix(status: Option<ExitStatus>) -> String {
    match status.and_then(|s| s.code()) {
        Some(code) => format!(" (exit {code})"),
        None => String::new(),
    }
}

impl MindError {
    /// Build an [`MindError::Io`] tagged with the path it happened at.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        MindError::Io {
            path: path.into(),
            source,
        }
    }

    /// Build an [`MindError::Json`] tagged with what was being processed.
    pub fn json(what: impl Into<String>, source: serde_json::Error) -> Self {
        MindError::Json {
            what: what.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_failed_displays_identity_and_command() {
        // spec: HOOK-30
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: "boom".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("github.com/acme/tools"), "msg: {msg}");
        assert!(msg.contains("make install"), "msg: {msg}");
        assert!(msg.contains("boom"), "msg: {msg}");
    }

    // spec: HOOK-30
    // A silent hook failure (no stdout/stderr) must render "(no output)" so the
    // error message does not point at framed output blocks that were never printed.
    #[test]
    fn hook_failed_silent_exit_renders_no_output() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "exit 1".into(),
            status: None,
            stderr: String::new(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("(no output)"),
            "silent failure must say '(no output)', not 'see the hook's output above': {msg}"
        );
        assert!(
            !msg.contains("see the hook"),
            "must not point to framed output when nothing was printed: {msg}"
        );
    }

    // spec: HOOK-30
    // A hook failure with stderr content must include that content in the message,
    // not the "(no output)" fallback.
    #[test]
    fn hook_failed_with_stderr_renders_stderr_not_no_output() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: "some diagnostic".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("some diagnostic"),
            "stderr content must appear in the message: {msg}"
        );
        assert!(
            !msg.contains("(no output)"),
            "must not say '(no output)' when stderr was captured: {msg}"
        );
    }
}
