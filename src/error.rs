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
    /// store-only and reached by reference (tooling.md TOOL-3). Also the "all
    /// kinds" default for a lobe with no `kinds` filter (HARN-1).
    pub const LINKABLE: [ItemKind; 3] = [ItemKind::Skill, ItemKind::Agent, ItemKind::Rule];

    /// Parse a list of kind strings into [`ItemKind`]s, rejecting any unknown
    /// string with [`MindError::UnknownKind`]. Used by the config `kinds` filter
    /// (HARN-1) and the harness presets (HARN-4).
    pub fn parse_kinds(strs: &[String]) -> Result<Vec<ItemKind>> {
        strs.iter()
            .map(|s| ItemKind::parse(s).ok_or_else(|| MindError::UnknownKind { kind: s.clone() }))
            .collect()
    }
}

impl std::fmt::Display for ItemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Format a conflicts list for display in error messages.
///
/// Each tuple is `(kind, effective_name, existing_source)`. Used by the
/// [`MindError::SkillCollision`] `#[error(...)]` format string.
fn format_conflicts(conflicts: &[(String, String, String)]) -> String {
    conflicts
        .iter()
        .map(|(k, n, s)| format!("  {k}:{n} (already installed from '{s}')"))
        .collect::<Vec<_>>()
        .join("\n")
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

    #[error("invalid config at {path}: {msg}")]
    ConfigToml { path: PathBuf, msg: String },

    #[error("failed to write {path}: {source}")]
    TomlWrite {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },

    #[error("'{path}' is not a configured agent home (lobe)")]
    UnknownLobe { path: String },

    #[error("'{kind}' is not a valid item kind (expected one of: skill, agent, rule, tool)")]
    UnknownKind { kind: String },

    #[error("'{name}' is not a known lobe preset (expected one of: gemini, codex, universal)")]
    UnknownPreset { name: String },

    #[error("`config lobes add` needs a path or `--preset <name>`")]
    LobeTargetRequired,

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

    #[error(
        "'{prefix}' cannot be used as a namespace prefix: it is a reserved item-kind word (skill, agent, rule, tool), which would make a prefixed name indistinguishable from a kind-qualified ref"
    )]
    ReservedPrefix { prefix: String },

    /// NS-28: prefix contains a path-unsafe character or structure.
    #[error(
        "'{prefix}' cannot be used as a namespace prefix: it must be a single safe path component (no `/`, `\\`, `:`, `.`, `..`, leading `~`, NUL, or control characters)"
    )]
    UnsafePrefix { prefix: String },

    #[error(
        "cannot change the namespace of source '{src_name}': the following items are installed ({items}); run `mind forget <item>` for each before changing the namespace",
        items = items.join(", ")
    )]
    NamespaceLocked {
        src_name: String,
        items: Vec<String>,
    },

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
        "no item matches '{query}'{}",
        if *sources == 0 {
            "; no sources are melded yet -- run `mind meld <repo>` to add one".to_string()
        } else {
            format!(" across {sources} melded source(s); run `mind sync` then `mind probe`")
        }
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
        "{path} already exists and is not managed by mind; remove it (or `mind forget` the item) before installing, or re-run with `--force` to overwrite"
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

    // `source` is reserved by thiserror (it auto-treats a field named `source` as
    // the error source, which must impl `Error`); use `super_source` instead.
    #[error(
        "melding '{super_source}' produced no discoverable items: it has no items of its own and every nested source failed to register"
    )]
    CuratorAllNestedFailed { super_source: String },

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

    // Constructed by the policy-enforcement paths (meld/sync/upgrade gating).
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
        if *printed_output { "(see output above)" } else if stderr.is_empty() { "(no output)" } else { stderr.as_str() }
    )]
    HookFailed {
        identity: String,
        command: String,
        status: Option<ExitStatus>,
        /// The stderr captured from the hook process, or empty when the hook's
        /// output was already streamed live to the terminal (`printed_output` true).
        stderr: String,
        /// True when the hook produced output that was already printed to the
        /// terminal in framed blocks before the failure was detected. When true,
        /// the Display shows "(see output above)" instead of "(no output)".
        printed_output: bool,
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

    /// DSC-66: a pin/ref value that would be misinterpreted as a git option was
    /// rejected at parse time before it could reach a git subprocess.
    #[error("invalid ref value '{value}': {reason}")]
    InvalidRef { value: String, reason: String },

    /// ABS-6: the destination already contains an item at the convention path.
    #[error("destination already has {kind}:{name} at {dest_path}; use --force to overwrite")]
    AbsorbCollision {
        kind: String,
        name: String,
        dest_path: String,
    },

    /// NS-41: two agents from different sources share the same harness name and
    /// would overwrite each other's agent-home link.
    #[error(
        "agent '{name}' from source '{incoming}' conflicts with the installed agent from \
         '{existing}': both link as agents/{name}.md in the agent home -- \
         run `mind forget agent:{name}` (or the prefixed name) to remove the existing agent first"
    )]
    AgentCollision {
        /// The bare harness name (frontmatter `name:`) that both agents share.
        name: String,
        /// The source of the already-installed agent.
        existing: String,
        /// The source of the agent being installed.
        incoming: String,
    },

    /// Cross-source skill/rule/tool name collision detected at `meld` (NS-43/NS-45).
    /// One or more incoming items share a `(kind, effective_name)` with an already-
    /// installed item from a different source, and the session is non-interactive.
    #[error(
        "name collision: the following items from the incoming source conflict with \
         already-installed items:\n{}\nRun `mind meld --namespace {suggested} <repo>` \
         to namespace the incoming source.",
        format_conflicts(conflicts)
    )]
    SkillCollision {
        /// Each conflict: `(kind, effective_name, existing_source)`.
        conflicts: Vec<(String, String, String)>,
        /// Suggested namespace prefix (the repo name / last URL component).
        suggested: String,
    },

    /// NS-28: effective item name contains path-traversal characters.
    #[error(
        "unsafe effective name '{name}': contains path-traversal characters or resolves to a relative component (`.`/`..`); refusing to build store or link paths from it"
    )]
    UnsafeName { name: String },

    /// STO-47: downloaded archive SHA-256 does not match the published digest.
    #[error(
        "digest mismatch for {url}: expected {expected}, got {actual}; the download may be corrupted or tampered with"
    )]
    DigestMismatch {
        url: String,
        expected: String,
        actual: String,
    },

    /// STO-50/STO-51: state file was written by a newer mind and uses an unknown schema version.
    #[error(
        "{what} uses schema version {found} but this mind only supports up to version {supported}; upgrade mind to read it"
    )]
    StateTooNew {
        what: &'static str,
        found: u32,
        supported: u32,
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

    // HARN-1/HARN-4: the new lobe-related errors render actionable messages
    // (the kind/preset list and the add-needs-a-target hint).
    #[test]
    fn lobe_errors_render_actionable_messages() {
        // spec: HARN-1
        // spec: HARN-4
        let unknown_kind = MindError::UnknownKind {
            kind: "wizard".into(),
        }
        .to_string();
        assert!(unknown_kind.contains("wizard"), "{unknown_kind}");
        assert!(
            unknown_kind.contains("skill") && unknown_kind.contains("tool"),
            "UnknownKind must list the valid kinds: {unknown_kind}"
        );

        let unknown_preset = MindError::UnknownPreset {
            name: "emacs".into(),
        }
        .to_string();
        assert!(unknown_preset.contains("emacs"), "{unknown_preset}");
        // spec: HARN-4 -- only the three real presets (gemini, codex, universal).
        assert!(
            unknown_preset.contains("gemini")
                && unknown_preset.contains("codex")
                && unknown_preset.contains("universal"),
            "UnknownPreset must list the valid presets: {unknown_preset}"
        );
        assert!(
            !unknown_preset.contains("antigravity"),
            "UnknownPreset must not mention removed presets: {unknown_preset}"
        );

        let needs_target = MindError::LobeTargetRequired.to_string();
        assert!(
            needs_target.contains("path") && needs_target.contains("--preset"),
            "LobeTargetRequired must mention both a path and --preset: {needs_target}"
        );
    }

    // HARN-1: parse_kinds rejects the first unknown string with UnknownKind and
    // accepts a well-formed list in order.
    #[test]
    fn parse_kinds_accepts_known_rejects_unknown() {
        // spec: HARN-1
        let ok = ItemKind::parse_kinds(&["skill".into(), "agent".into(), "rule".into()]).unwrap();
        assert_eq!(ok, vec![ItemKind::Skill, ItemKind::Agent, ItemKind::Rule]);

        let err = ItemKind::parse_kinds(&["skill".into(), "wizard".into()]).unwrap_err();
        assert!(
            matches!(err, MindError::UnknownKind { ref kind } if kind == "wizard"),
            "the first unknown kind must surface as UnknownKind: {err:?}"
        );
    }

    #[test]
    fn namespace_locked_displays_items_and_forget_hint() {
        // spec: NS-30 CLI-161 - the lock error names the source, lists every
        // installed item, and directs the user to `mind forget` before changing
        // the namespace.
        let e = MindError::NamespaceLocked {
            src_name: "github.com/acme/agents".into(),
            items: vec!["skill:review".into(), "agent:dev".into()],
        }
        .to_string();
        assert!(e.contains("github.com/acme/agents"), "{e}");
        assert!(
            e.contains("skill:review") && e.contains("agent:dev"),
            "must list every installed item: {e}"
        );
        assert!(e.contains("forget"), "must direct the user to forget: {e}");
        assert!(e.contains("namespace"), "must mention the namespace: {e}");
    }

    #[test]
    fn hook_failed_displays_identity_and_command() {
        // spec: HOOK-30
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: "boom".into(),
            printed_output: false,
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
            printed_output: false,
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
            printed_output: false,
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

    // spec: HOOK-30
    // When a hook produced output that was already streamed to the terminal
    // (`printed_output` true), HookFailed must say "(see output above)" rather
    // than the misleading "(no output)" -- even when stderr is empty, because
    // the diagnostics were already visible on screen.
    #[test]
    fn hook_failed_with_printed_output_renders_see_output_above_not_no_output() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: String::new(),
            printed_output: true,
        };
        let msg = e.to_string();
        assert!(
            msg.contains("(see output above)"),
            "printed_output=true must say '(see output above)': {msg}"
        );
        assert!(
            !msg.contains("(no output)"),
            "must not say '(no output)' when output was already shown: {msg}"
        );
        // Identity and command must still appear.
        assert!(
            msg.contains("github.com/acme/tools"),
            "missing identity: {msg}"
        );
        assert!(msg.contains("make install"), "missing command: {msg}");
    }

    // NS-43 / NS-45: SkillCollision lists all conflicting items, names the
    // existing source for each, and suggests --namespace with the repo name.
    #[test]
    fn skill_collision_renders_conflict_list_and_namespace_hint() {
        // spec: NS-43 NS-45
        let e = MindError::SkillCollision {
            conflicts: vec![
                (
                    "skill".into(),
                    "review".into(),
                    "github.com/acme/agents".into(),
                ),
                (
                    "rule".into(),
                    "style".into(),
                    "github.com/acme/rules".into(),
                ),
            ],
            suggested: "acme".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("name collision"),
            "must contain 'name collision': {msg}"
        );
        assert!(
            msg.contains("skill:review"),
            "must list skill:review: {msg}"
        );
        assert!(msg.contains("rule:style"), "must list rule:style: {msg}");
        assert!(
            msg.contains("github.com/acme/agents"),
            "must name the existing source: {msg}"
        );
        assert!(
            msg.contains("--namespace acme"),
            "must suggest --namespace with the repo name: {msg}"
        );
    }

    // spec: HOOK-30
    // printed_output=true takes priority over a non-empty stderr field (the field
    // is empty in production but this guards the priority rule explicitly).
    #[test]
    fn hook_failed_printed_output_priority_over_stderr_content() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: "some content".into(),
            printed_output: true,
        };
        let msg = e.to_string();
        assert!(
            msg.contains("(see output above)"),
            "printed_output=true must take priority: {msg}"
        );
        assert!(
            !msg.contains("(no output)"),
            "must not say '(no output)': {msg}"
        );
    }

    #[test]
    fn item_not_found_no_sources_hints_meld() {
        // When no sources are melded, the error must direct the user to `mind meld`
        // rather than `mind sync` (which would be useless with no sources).
        let e = MindError::ItemNotFound {
            query: "review".into(),
            sources: 0,
        }
        .to_string();
        assert!(e.contains("review"), "must include query: {e}");
        assert!(
            e.contains("meld"),
            "no-sources hint must mention `meld`: {e}"
        );
        assert!(
            !e.contains("sync"),
            "no-sources path must not suggest `sync`: {e}"
        );
    }

    #[test]
    fn item_not_found_with_sources_hints_sync_probe() {
        // With sources present the hint directs the user to sync then probe.
        let e = MindError::ItemNotFound {
            query: "review".into(),
            sources: 3,
        }
        .to_string();
        assert!(e.contains("review"), "must include query: {e}");
        assert!(e.contains("3"), "must include source count: {e}");
        assert!(e.contains("sync"), "must mention `sync`: {e}");
        assert!(e.contains("probe"), "must mention `probe`: {e}");
        // Must not suggest running `mind meld` (only appropriate when sources == 0).
        // The word "melded" may appear in the count phrase "across N melded source(s)".
        assert!(
            !e.contains("mind meld") && !e.contains("meld <"),
            "with sources must not suggest `meld`: {e}"
        );
    }

    #[test]
    fn link_occupied_includes_force_hint() {
        // spec: LIFE-41 -- the `--force` remedy must be surfaced in the error.
        let e = MindError::LinkOccupied {
            path: "/home/user/.claude/skills/foo".into(),
        }
        .to_string();
        assert!(
            e.contains("--force"),
            "LinkOccupied must mention --force: {e}"
        );
        assert!(
            e.contains("/home/user/.claude/skills/foo"),
            "must include the path: {e}"
        );
    }

    #[test]
    fn digest_mismatch_includes_url_and_digests() {
        // spec: STO-47
        let e = MindError::DigestMismatch {
            url: "https://example.com/mind-0.1.0.tar.gz".into(),
            expected: "abc123".into(),
            actual: "def456".into(),
        }
        .to_string();
        assert!(e.contains("abc123"), "must include expected digest: {e}");
        assert!(e.contains("def456"), "must include actual digest: {e}");
        assert!(
            e.contains("https://example.com/mind-0.1.0.tar.gz"),
            "must include URL: {e}"
        );
    }

    #[test]
    fn state_too_new_names_file_and_versions() {
        // spec: STO-51
        let e = MindError::StateTooNew {
            what: "sources.json",
            found: 3,
            supported: 1,
        }
        .to_string();
        assert!(e.contains("sources.json"), "must name the file: {e}");
        assert!(e.contains("3"), "must name the found version: {e}");
        assert!(e.contains("1"), "must name the supported version: {e}");
        assert!(e.contains("upgrade"), "must suggest upgrading: {e}");
    }

    #[test]
    fn unsafe_prefix_error_mentions_prefix() {
        // spec: NS-28
        let e = MindError::UnsafePrefix {
            prefix: "../evil".into(),
        }
        .to_string();
        assert!(e.contains("../evil"), "must include the prefix: {e}");
    }
}
