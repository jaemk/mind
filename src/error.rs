//! Structured error types for `mind`.
//!
//! Every fallible operation returns [`Result<T>`], which carries a [`MindError`].
//! We deliberately avoid stringly-typed errors (e.g. `anyhow`) so callers and
//! tests can match on the precise failure and so messages stay consistent.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, MindError>;

/// Size cap for a source-controlled metadata file read during discovery: a
/// `mind.toml`, an item's frontmatter block (`SKILL.md`/agent/rule `.md`), or a
/// Claude plugin/marketplace manifest (`.claude-plugin/plugin.json` /
/// `marketplace.json`). DSC-91.
///
/// These are hand-authored text files a maintainer edits directly; the largest
/// legitimate one in this repo's own examples is a few KB. 8 MiB is chosen as a
/// generous ceiling that sits orders of magnitude above any real metadata file
/// while still bounding how much memory a single melded source can force `mind`
/// to allocate while scanning or installing it. This cap covers metadata reads
/// ONLY -- item content (an item tree's `{{ns:}}` expansion at install, the
/// unguarded-reference scan, `review`, the TUI preview, and content hashing)
/// stays uncapped (see spec/discovery.md DSC-90).
pub const METADATA_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

/// Read `path` into a `String`, refusing (with [`MindError::MetadataTooLarge`])
/// a file at or above [`METADATA_SIZE_LIMIT`] bytes -- WITHOUT first allocating
/// the whole file. Reads at most `METADATA_SIZE_LIMIT + 1` bytes via
/// `Read::take`, so an oversized file's cost is bounded by the cap, not by its
/// actual size.
///
/// Shared by every metadata reader (`mindfile.rs`, `frontmatter.rs`,
/// `plugin_manifest.rs`) so the limit and the error are defined exactly once.
/// A non-UTF-8 file surfaces as a plain [`MindError::Io`] (matching
/// `std::fs::read_to_string`'s behavior); other I/O failures (e.g. not found)
/// also surface as `MindError::Io`, leaving the caller to decide how to treat
/// them (several metadata readers treat a NotFound source as "absent", not an
/// error).
pub fn read_capped_metadata(path: &Path) -> Result<String> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).map_err(|e| MindError::io(path, e))?;
    let mut buf = Vec::new();
    file.take(METADATA_SIZE_LIMIT + 1)
        .read_to_end(&mut buf)
        .map_err(|e| MindError::io(path, e))?;
    if buf.len() as u64 > METADATA_SIZE_LIMIT {
        return Err(MindError::MetadataTooLarge {
            path: path.to_path_buf(),
            limit: METADATA_SIZE_LIMIT,
        });
    }
    String::from_utf8(buf).map_err(|e| {
        MindError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        )
    })
}

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

/// Why a path-reference token or `requires` entry failed to resolve, so a
/// [`MindError::BadReference`] (and the `review` `bad-reference` finding) can name
/// the specific cause instead of one blanket message. The causes read very
/// differently to a maintainer -- a genuine typo/miss, a real tool whose
/// entrypoint just did not ship, a name that is ambiguous across kinds, a
/// forbidden cross-source ref, or a malformed ref -- and conflating them sends a
/// debugging session down the wrong trail (tooling.md TOOL-17/TOOL-18,
/// dependencies.md DEP-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadRefReason {
    /// The referent names no matching sibling item (a plain miss).
    NoMatch,
    /// A `{{tools:name}}` referent names a real sibling tool, but that tool has
    /// no resolvable entrypoint (`bin`): no `TOOL.md`/`mind.toml` `bin` and no
    /// convention entrypoint file present in the source (tooling.md TOOL-5).
    ToolNoBin,
    /// The referent (a bare `{{path:name}}` or a bare `requires` name) matches
    /// more than one sibling across kinds and carries no `kind:` qualifier to
    /// disambiguate (tooling.md TOOL-18, dependencies.md DEP-7).
    AmbiguousKind,
    /// A `requires` entry is source-qualified (`owner/repo#name`); `requires` is
    /// intra-source only and never crosses sources (dependencies.md DEP-5/DEP-7).
    CrossSource,
    /// A `requires` entry is not a parseable item ref at all (dependencies.md
    /// DEP-7).
    InvalidRef,
}

impl std::fmt::Display for BadRefReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BadRefReason::NoMatch => f.write_str("does not match any item"),
            BadRefReason::ToolNoBin => {
                f.write_str("names a tool with no resolvable entrypoint (bin)")
            }
            BadRefReason::AmbiguousKind => {
                f.write_str("is ambiguous across kinds; add a kind qualifier")
            }
            BadRefReason::CrossSource => {
                f.write_str("crosses sources; a requires entry is intra-source only")
            }
            BadRefReason::InvalidRef => f.write_str("is not a valid item ref"),
        }
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

    // Names the next command, mirroring `ItemNotFound`'s house style (CLI-179).
    #[error(
        "'{path}' is not a configured agent home (lobe); run 'mind config lobes list' to see configured lobes"
    )]
    UnknownLobe { path: String },

    #[error("'{kind}' is not a valid item kind (expected one of: skill, agent, rule, tool)")]
    UnknownKind { kind: String },

    #[error(
        "'{name}' is not a known lobe preset (expected one of: gemini, codex, universal, windsurf)"
    )]
    UnknownPreset { name: String },

    #[error("`config lobes add` needs a path or `--preset <name>`")]
    LobeTargetRequired,

    #[error("mind.toml at {path}: {msg}")]
    MindToml { path: PathBuf, msg: String },

    /// A non-`mind.toml` source manifest (a Claude `plugin.json` /
    /// `marketplace.json`) that is malformed or schema-invalid. Kept distinct from
    /// [`MindError::MindToml`] so the message names the actual file rather than
    /// mislabeling a JSON manifest as "mind.toml at ..."; `{path}` and the caller's
    /// `{msg}` (which already names the file kind) carry the specifics.
    #[error("{path}: {msg}")]
    Manifest { path: PathBuf, msg: String },

    /// DSC-91: a hand-authored source-controlled metadata file (`mind.toml`, an
    /// item's frontmatter block, or a Claude plugin/marketplace manifest)
    /// exceeded [`METADATA_SIZE_LIMIT`]. Refused before the whole file is read
    /// into memory (see [`read_capped_metadata`]).
    #[error(
        "{path} exceeds the {} MiB size cap for a hand-authored metadata file (mind.toml, an \
         item's frontmatter, or a plugin/marketplace manifest); trim the file, or move large \
         content out of it, and try again",
        limit / (1024 * 1024)
    )]
    MetadataTooLarge { path: PathBuf, limit: u64 },

    /// CLI-215: this message previously omitted the local-path forms
    /// `parse_spec` has always accepted (a bare `/abs/path`, `./rel/path`,
    /// `../rel/path`, or `file:///abs/path`), even though `mind review --help`
    /// documents them as a valid `<target>`. Naming them here closes that gap.
    #[error(
        "'{spec}' is not a valid repo spec (expected 'owner/repo', a github shorthand, a git URL, \
         or a local path: '/abs/path', './rel/path', '../rel/path', or 'file:///abs/path')"
    )]
    InvalidRepoSpec { spec: String },

    /// CLI-204: a repo spec that parses structurally but whose `host`, `owner`,
    /// or `repo` part is not a single safe path component. Those three parts are
    /// both the source's identity segments and its clone-path components, so a
    /// part carrying `/`, `..`, or a control character would break identity
    /// segmentation (and therefore allowlist matching, POL-67) and escape the
    /// sources tree. Kept distinct from [`MindError::InvalidRepoSpec`] so the
    /// message names the offending part rather than the whole spec.
    #[error(
        "'{spec}' is not a usable repo spec: its {part} part '{value}' {reason}; each of host, owner, and repo must be a single path component"
    )]
    UnsafeRepoSpec {
        spec: String,
        part: &'static str,
        value: String,
        reason: &'static str,
    },

    /// LNK-1/LNK-2: a URL that carries a `tree`/`blob` link marker (so it is
    /// unambiguously an attempted item link, not a plain repo spec) but whose
    /// tail fails to parse as one. Kept distinct from [`MindError::InvalidRepoSpec`]
    /// so the message names the two link shapes instead of telling a user who
    /// pasted a real forge URL that it is not a valid URL at all (`reason`
    /// carries the specific defect: missing ref, missing skill path, a `blob`
    /// link not ending in `/SKILL.md`, or an unsafe path/ref value).
    #[error(
        "'{url}' is not a valid item-link URL ({reason}); expected '<repo-url>/tree/<ref>/<skill-dir>' or '<repo-url>/blob/<ref>/<skill-dir>/SKILL.md' (GitLab's '/-/tree/' and '/-/blob/' forms also work)"
    )]
    BadItemLink { url: String, reason: String },

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

    // Names the next command, mirroring `ItemNotFound`'s house style (CLI-179).
    #[error(
        "no source named '{name}' is melded; run 'mind recall --sources' to list melded sources"
    )]
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

    // spec: CLI-179
    #[error(
        "no item matches '{query}'{}",
        if *sources == 0 {
            "; no sources are melded yet -- run `mind meld <repo>` to add one".to_string()
        } else {
            format!(" across {sources} melded source(s); run `mind probe` to search available items")
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

    #[error("{item}: reference {referent} {reason} in source '{in_source}'")]
    BadReference {
        item: String,
        /// The offending token as written, e.g. `{{ns:foo}}` or `{{tools:bar}}`.
        referent: String,
        /// Why it did not resolve, so the message names the specific cause
        /// (TOOL-17). A `NoMatch` keeps the historical "does not match any item"
        /// wording.
        reason: BadRefReason,
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

    #[error("conflicting pin flags: {first} and {second} cannot both be given; supply at most one")]
    ConflictingPin { first: String, second: String },

    #[error(
        "invalid --pin value '{value}': expected HEAD, a ref (tag/sha/branch), 'branch=<name>', or 'tag=<name>'"
    )]
    BadPinSpec { value: String },

    #[error("source '{source_name}': scan root '{root}' is not a directory in the clone")]
    InvalidRoot { source_name: String, root: String },

    #[error(
        "source '{source_name}': linked path '{path}' is not a skill directory in the clone (no SKILL.md)"
    )]
    LinkNotASkill { source_name: String, path: String },

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
        "local-path and file:// melds are forbidden by the managed policy \
         ([sources].allow-local = false)"
    )]
    LocalMeldForbidden { identity: String },

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

    /// STO-66: `gh attestation verify` ran and reported the downloaded release
    /// archive does not carry a valid, matching build-provenance attestation.
    /// The swap is aborted and the existing binary is left in place.
    #[error(
        "build provenance verification failed for the downloaded release archive: {reason}; \
         evolve is aborting without replacing the running binary. If this release predates \
         build-provenance attestations, install it another way (resources/install.sh or a \
         manual download) instead of `mind evolve`"
    )]
    AttestationVerificationFailed { reason: String },

    /// POL-52/POL-53: `evolve` was refused or redirected by the managed policy.
    /// The `detail` field carries the human-readable reason: "self-update is
    /// disabled by the managed policy" for the disabled case (POL-52), or the
    /// specific mismatch message for the pinned-version-conflict case (POL-53).
    #[error("{detail}")]
    SelfUpdatePolicy { detail: String },

    /// STO-50/STO-51: state file was written by a newer mind and uses an unknown schema version.
    #[error(
        "{what} uses schema version {found} but this mind only supports up to version {supported}; upgrade mind to read it"
    )]
    StateTooNew {
        what: &'static str,
        found: u32,
        supported: u32,
    },

    /// HOOK-103: `--event build` is valid only for an item target; a source has
    /// no build hook. Checked before running anything.
    // spec: HOOK-103 CLI-195
    #[error(
        "--event build is valid only for an item target (a source has no build hook); use <source>#<item> to target an item"
    )]
    BuildEventRequiresItemTarget,

    /// HOOK-100: a required hook was aborted by the user at the three-way prompt
    /// during `mind hooks run`. The command exits non-zero; any hooks that ran
    /// earlier in the same session are not rolled back.
    // spec: HOOK-100
    #[error("hook '{label}' was aborted by user; not running remaining hooks")]
    HookAborted { label: String },

    /// STO-56: the base directory for a project-scoped lobe does not exist.
    /// Returned by `resolve_lobe` when an explicit `base` is given but the
    /// directory is absent, so mind refuses to fabricate a path into a
    /// nonexistent project.
    // spec: STO-56
    #[error("lobe base directory does not exist: {}", path.display())]
    LobeBaseMissing { path: PathBuf },

    /// STO-69: a source's clone path (`Source::clone_dir`) resolves outside the
    /// managed sources tree, or through a `..` component -- e.g. a hand-edited
    /// or corrupted `sources.json` entry with a traversing `host`/`owner`/`repo`
    /// part. Refused before any filesystem mutation (clone or delete) is
    /// attempted at that path, so a stale/tampered registry entry cannot make
    /// `mind` write or remove content outside `~/.mind/sources`. Not raised for
    /// a linked local source (`Source::is_linked`), whose "clone dir" is the
    /// user's own working tree by design.
    // spec: STO-69
    #[error(
        "refusing to use clone path '{path}' for source '{identity}': it resolves outside the managed sources tree"
    )]
    UnsafeClonePath { path: PathBuf, identity: String },

    /// HOOK-105: a `hooks run`/`hooks list` target string exactly names both a
    /// registered source identity (an item-link instance's own `host/owner/repo#path`
    /// identity, LNK-4, when the linked skill sits at a single top-level path
    /// segment) and, under the ordinary `<source>#<item>` reading of that same
    /// string, an installed item. Resolving silently either way depends on
    /// registry state invisible to the caller, so this is reported instead of a
    /// silent pick. `item_forms` are the kind-qualified refs
    /// (`<source>#<kind>:<name>`) that unambiguously target each matching item;
    /// a `source:<target>` prefix unambiguously targets the source.
    // spec: HOOK-105
    #[error(
        "'{target}' is ambiguous: it names both the registered source '{target}' and installed item(s) matching the same string; \
         to target the source, run `mind hooks run 'source:{target}'`; to target an item, use a kind-qualified ref, e.g. `mind hooks run '{}'`",
        item_forms.first().cloned().unwrap_or_default()
    )]
    AmbiguousHookTarget {
        target: String,
        /// The kind-qualified `<source>#<kind>:<name>` ref for each installed
        /// item the target string also names.
        item_forms: Vec<String>,
    },

    /// CLI-212/CLI-213: a linked local source's (`Source::is_linked`) working
    /// tree has vanished since it was melded - a relocated or deleted `/tmp`
    /// fixture, or a `cd` past a relative clean-up. Raised by
    /// `catalog::scan_source` before `MindToml::load`'s generic
    /// NotFound-as-absent handling or a convention-scan `InvalidRoot` obscure
    /// the real cause with a less actionable message. The whole-registry walk
    /// (`catalog::scan`) catches exactly this variant and degrades (warns on
    /// stderr, keeps going with the sources that DID scan) so one dead linked
    /// source does not take down `recall`/`probe`/`learn`; a targeted
    /// single-source scan (`meld`, a fresh clone) still hard-fails on it since
    /// in practice a just-cloned directory cannot itself be gone.
    // spec: CLI-212 CLI-213
    #[error(
        "source '{source_name}': linked working tree '{path}' is gone; run 'mind unmeld \
         {source_name}' to drop it, or restore the directory"
    )]
    LinkedSourceGone { source_name: String, path: String },

    /// HOOK-107 (source targets) / HOOK-108 (item targets): a `hooks run
    /// <target>` invocation found at least one hook that existed and needed
    /// running, but EVERY one of them was skipped for want of consent (no
    /// terminal, and `--dangerously-skip-install-hook-check` not given) - so
    /// the run did nothing, silently, in the state where a provisioning script
    /// most needs to know it. A run with nothing to do (no hooks declared, or
    /// every install hook already ran at the current commit) is unaffected and
    /// stays exit 0; only "there was work and consent was unavailable" is this
    /// error.
    ///
    /// `event` is the lifecycle event the run selected, carried so the printed
    /// remedy re-selects it (HOOK-106): without it the suggested command
    /// re-runs the default `install` hooks, which for an `uninstall` run is
    /// different code than the one that was skipped.
    // spec: HOOK-106 HOOK-107 HOOK-108
    #[error(
        "hooks run '{target}': {skipped} hook(s) had work to do but were skipped for want of \
         consent (not a terminal); re-run with 'mind hooks run {target} --event {event} \
         --dangerously-skip-install-hook-check' to run them unattended"
    )]
    HooksNotRun {
        target: String,
        event: String,
        skipped: usize,
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

    /// A stable kebab-case slug that identifies this error variant. Used as the
    /// `kind` field in the JSON error envelope emitted under `--json` (CLI-181).
    /// These slugs are API: they must not change once assigned.
    // spec: CLI-182
    pub fn kind(&self) -> &'static str {
        match self {
            MindError::HomeDirNotFound => "home-dir-not-found",
            MindError::Io { .. } => "io",
            MindError::Json { .. } => "json",
            MindError::Toml { .. } => "toml",
            MindError::ConfigToml { .. } => "config-toml",
            MindError::TomlWrite { .. } => "toml-write",
            MindError::UnknownLobe { .. } => "unknown-lobe",
            MindError::UnknownKind { .. } => "unknown-kind",
            MindError::UnknownPreset { .. } => "unknown-preset",
            MindError::LobeTargetRequired => "lobe-target-required",
            MindError::MindToml { .. } => "mind-toml",
            MindError::Manifest { .. } => "manifest",
            MindError::MetadataTooLarge { .. } => "metadata-too-large",
            MindError::InvalidRepoSpec { .. } => "invalid-repo-spec",
            MindError::UnsafeRepoSpec { .. } => "unsafe-repo-spec",
            MindError::BadItemLink { .. } => "bad-item-link",
            MindError::InvalidItemRef { .. } => "invalid-item-ref",
            MindError::ReservedPrefix { .. } => "reserved-prefix",
            MindError::UnsafePrefix { .. } => "unsafe-prefix",
            MindError::NamespaceLocked { .. } => "namespace-locked",
            MindError::SourceExists { .. } => "source-exists",
            MindError::SourceNotFound { .. } => "source-not-found",
            MindError::InvalidPattern { .. } => "invalid-pattern",
            MindError::AmbiguousSource { .. } => "ambiguous-source",
            MindError::ItemNotFound { .. } => "item-not-found",
            MindError::AmbiguousItem { .. } => "ambiguous-item",
            MindError::NotInstalled { .. } => "not-installed",
            MindError::SyncFailed { .. } => "sync-failed",
            MindError::IncompatibleVersion { .. } => "incompatible-version",
            MindError::LinkOccupied { .. } => "link-occupied",
            MindError::BadReference { .. } => "bad-reference",
            MindError::Git { .. } => "git",
            MindError::CuratorAllNestedFailed { .. } => "curator-all-nested-failed",
            MindError::GitNotFound => "git-not-found",
            MindError::ConflictingPin { .. } => "conflicting-pin",
            MindError::BadPinSpec { .. } => "bad-pin-spec",
            MindError::InvalidRoot { .. } => "invalid-root",
            MindError::LinkNotASkill { .. } => "link-not-a-skill",
            MindError::DuplicateItem { .. } => "duplicate-item",
            MindError::ReviewFailed { .. } => "review-failed",
            MindError::SourceNotAllowed { .. } => "source-not-allowed",
            MindError::LocalMeldForbidden { .. } => "local-meld-forbidden",
            MindError::UnpinnedSourceForbidden { .. } => "unpinned-source-forbidden",
            MindError::InvalidPolicy { .. } => "invalid-policy",
            MindError::LobesLocked { .. } => "lobes-locked",
            MindError::HookFailed { .. } => "hook-failed",
            MindError::UnsupportedPlatform { .. } => "unsupported-platform",
            MindError::DownloadFailed { .. } => "download-failed",
            MindError::ReleaseAssetEmpty => "release-asset-empty",
            MindError::TargetNotWritable { .. } => "target-not-writable",
            MindError::NotADirectory { .. } => "not-a-directory",
            MindError::ConfirmationRequired { .. } => "confirmation-required",
            MindError::DestinationNotRepo { .. } => "destination-not-repo",
            MindError::InvalidRef { .. } => "invalid-ref",
            MindError::AbsorbCollision { .. } => "absorb-collision",
            MindError::AgentCollision { .. } => "agent-collision",
            MindError::SkillCollision { .. } => "skill-collision",
            MindError::UnsafeName { .. } => "unsafe-name",
            MindError::DigestMismatch { .. } => "digest-mismatch",
            MindError::AttestationVerificationFailed { .. } => "attestation-verification-failed",
            MindError::SelfUpdatePolicy { .. } => "self-update-policy",
            MindError::StateTooNew { .. } => "state-too-new",
            MindError::BuildEventRequiresItemTarget => "build-event-requires-item-target",
            MindError::HookAborted { .. } => "hook-aborted",
            MindError::LobeBaseMissing { .. } => "lobe-base-missing",
            MindError::UnsafeClonePath { .. } => "unsafe-clone-path",
            MindError::AmbiguousHookTarget { .. } => "ambiguous-hook-target",
            MindError::LinkedSourceGone { .. } => "linked-source-gone",
            MindError::HooksNotRun { .. } => "hooks-not-run",
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
        // spec: HARN-4 -- the real presets (gemini, codex, universal, windsurf).
        assert!(
            unknown_preset.contains("gemini")
                && unknown_preset.contains("codex")
                && unknown_preset.contains("universal")
                && unknown_preset.contains("windsurf"),
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

    // spec: CLI-179
    #[test]
    fn item_not_found_with_sources_hints_probe_not_sync() {
        // With sources present the hint directs the user to probe; sync is not
        // mentioned because syncing cannot surface an item that does not exist.
        let e = MindError::ItemNotFound {
            query: "review".into(),
            sources: 3,
        }
        .to_string();
        assert!(e.contains("review"), "must include query: {e}");
        assert!(e.contains("3"), "must include source count: {e}");
        assert!(e.contains("probe"), "must mention `probe`: {e}");
        // sync must not appear -- it cannot help a name that will never exist.
        assert!(
            !e.contains("sync"),
            "with sources must not mention `sync`: {e}"
        );
        // Must not suggest running `mind meld` (only appropriate when sources == 0).
        // The word "melded" may appear in the count phrase "across N melded source(s)".
        assert!(
            !e.contains("mind meld") && !e.contains("meld <"),
            "with sources must not suggest `meld`: {e}"
        );
    }

    // U37: SourceNotFound must name the next command, mirroring ItemNotFound's
    // house style (CLI-179), so a typo'd source name points somewhere useful.
    #[test]
    fn source_not_found_hints_recall_sources() {
        let e = MindError::SourceNotFound {
            name: "jaemk/minds".into(),
        }
        .to_string();
        assert!(e.contains("jaemk/minds"), "must include the name: {e}");
        assert!(
            e.contains("mind recall --sources"),
            "must hint the exact next command: {e}"
        );
    }

    // U37: UnknownLobe must likewise name the next command.
    #[test]
    fn unknown_lobe_hints_config_lobes_list() {
        let e = MindError::UnknownLobe {
            path: "/no/such/lobe".into(),
        }
        .to_string();
        assert!(e.contains("/no/such/lobe"), "must include the path: {e}");
        assert!(
            e.contains("mind config lobes list"),
            "must hint the exact next command: {e}"
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
    fn attestation_verification_failed_names_reason_and_kind() {
        // spec: STO-66
        let e = MindError::AttestationVerificationFailed {
            reason: "no attestations found".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("no attestations found"),
            "must include the reason: {msg}"
        );
        assert!(
            msg.contains("aborting"),
            "must say evolve is aborting: {msg}"
        );
        assert_eq!(e.kind(), "attestation-verification-failed");
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

    // spec: CLI-182
    // kind() returns stable, non-empty kebab-case slugs. Spot-check a
    // representative sample; the exhaustive match in the impl guarantees every
    // variant has a slug.
    #[test]
    fn kind_slugs_are_stable() {
        assert_eq!(
            MindError::ItemNotFound {
                query: "x".into(),
                sources: 0
            }
            .kind(),
            "item-not-found"
        );
        assert_eq!(
            MindError::DigestMismatch {
                url: "u".into(),
                expected: "e".into(),
                actual: "a".into()
            }
            .kind(),
            "digest-mismatch"
        );
        assert_eq!(
            MindError::SelfUpdatePolicy { detail: "d".into() }.kind(),
            "self-update-policy"
        );
        assert_eq!(MindError::HomeDirNotFound.kind(), "home-dir-not-found");
        assert_eq!(MindError::GitNotFound.kind(), "git-not-found");
        assert_eq!(MindError::ReleaseAssetEmpty.kind(), "release-asset-empty");
        assert_eq!(MindError::LobeTargetRequired.kind(), "lobe-target-required");
        // spec: HOOK-103 CLI-195 -- the two `hooks run` error variants carry
        // stable slugs.
        assert_eq!(
            MindError::BuildEventRequiresItemTarget.kind(),
            "build-event-requires-item-target"
        );
        assert_eq!(
            MindError::HookAborted { label: "h".into() }.kind(),
            "hook-aborted"
        );
        // spec: LNK-14 -- the malformed-item-link error carries a stable slug.
        assert_eq!(
            MindError::BadItemLink {
                url: "u".into(),
                reason: "r".into()
            }
            .kind(),
            "bad-item-link"
        );
        // spec: CLI-212 CLI-213 -- a gone linked source's clone carries its own
        // stable slug, distinct from the generic scan failures.
        assert_eq!(
            MindError::LinkedSourceGone {
                source_name: "s".into(),
                path: "p".into(),
            }
            .kind(),
            "linked-source-gone"
        );
        // spec: HOOK-107 -- "ran nothing because consent was unavailable"
        // carries its own stable slug, distinct from HookAborted.
        assert_eq!(
            MindError::HooksNotRun {
                target: "t".into(),
                event: "install".into(),
                skipped: 1,
            }
            .kind(),
            "hooks-not-run"
        );

        // Every slug must be non-empty and kebab-case (lowercase, hyphens only).
        let samples: &[(&str, &MindError)] = &[
            (
                "item-not-found",
                &MindError::ItemNotFound {
                    query: "x".into(),
                    sources: 0,
                },
            ),
            ("home-dir-not-found", &MindError::HomeDirNotFound),
            ("git-not-found", &MindError::GitNotFound),
        ];
        for (expected, err) in samples {
            let slug = err.kind();
            assert_eq!(slug, *expected, "slug mismatch for variant");
            assert!(
                !slug.is_empty(),
                "slug must be non-empty for variant {expected}"
            );
            assert!(
                slug.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "slug must be lowercase kebab-case: {slug}"
            );
        }
    }

    // spec: CLI-212 CLI-213
    #[test]
    fn linked_source_gone_names_source_and_remedy() {
        // The message must name the source, the vanished path, and the exact
        // `unmeld` remedy so it is directly actionable.
        let e = MindError::LinkedSourceGone {
            source_name: "local/tmp/starter".into(),
            path: "/tmp/starter".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("local/tmp/starter"),
            "must name the source: {msg}"
        );
        assert!(msg.contains("/tmp/starter"), "must name the path: {msg}");
        assert!(
            msg.contains("mind unmeld local/tmp/starter"),
            "must give the copy-pasteable unmeld remedy: {msg}"
        );
    }

    // spec: HOOK-107 HOOK-106
    #[test]
    fn hooks_not_run_names_target_count_and_remedy() {
        let e = MindError::HooksNotRun {
            target: "myrepo".into(),
            event: "install".into(),
            skipped: 2,
        };
        let msg = e.to_string();
        assert!(msg.contains("myrepo"), "must name the target: {msg}");
        assert!(msg.contains('2'), "must name the skipped count: {msg}");
        assert!(
            msg.contains(
                "mind hooks run myrepo --event install --dangerously-skip-install-hook-check"
            ),
            "must give the copy-pasteable remedy: {msg}"
        );
    }

    /// HOOK-106: the remedy re-selects the event the run selected. Without the
    /// `--event` segment the printed command re-runs the source's INSTALL
    /// hooks, which is different code than the uninstall hooks that were
    /// skipped -- a silent substitution for anyone who copy-pastes it.
    // spec: HOOK-106
    #[test]
    fn hooks_not_run_remedy_carries_the_selected_event() {
        let e = MindError::HooksNotRun {
            target: "myrepo".into(),
            event: "uninstall".into(),
            skipped: 1,
        };
        let msg = e.to_string();
        assert!(
            msg.contains(
                "mind hooks run myrepo --event uninstall --dangerously-skip-install-hook-check"
            ),
            "the remedy must re-select the uninstall event: {msg}"
        );
        assert!(
            !msg.contains("--event install"),
            "the remedy must not name a different event than the run selected: {msg}"
        );
    }

    // spec: CLI-215
    #[test]
    fn invalid_repo_spec_names_local_path_forms() {
        // mind review --help documents local-path forms as an accepted target;
        // the error naming the accepted spec shapes must not omit them.
        let e = MindError::InvalidRepoSpec {
            spec: "bogus".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("./rel/path"), "must mention './': {msg}");
        assert!(msg.contains("../rel/path"), "must mention '../': {msg}");
        assert!(
            msg.contains("/abs/path"),
            "must mention an absolute path: {msg}"
        );
        assert!(msg.contains("file://"), "must mention 'file://': {msg}");
    }

    // spec: STO-56
    #[test]
    fn lobe_base_missing_displays_path() {
        // LobeBaseMissing must name the missing path in its message and carry the
        // correct kind slug ("lobe-base-missing").
        let path = std::path::PathBuf::from("/nonexistent/myproject");
        let e = MindError::LobeBaseMissing { path: path.clone() };
        let msg = e.to_string();
        assert!(
            msg.contains("/nonexistent/myproject"),
            "must include the path: {msg}"
        );
        assert!(
            msg.contains("does not exist"),
            "must say directory does not exist: {msg}"
        );
        assert_eq!(e.kind(), "lobe-base-missing", "kind slug must be stable");
    }

    // spec: STO-69
    #[test]
    fn unsafe_clone_path_names_path_and_identity() {
        // The message must name both the offending resolved path and the
        // source identity it belongs to, and carry the stable
        // "unsafe-clone-path" kind slug.
        let e = MindError::UnsafeClonePath {
            path: PathBuf::from("/home/user/evil"),
            identity: "../../victim".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("/home/user/evil"),
            "must include the offending path: {msg}"
        );
        assert!(
            msg.contains("../../victim"),
            "must include the source identity: {msg}"
        );
        assert!(
            msg.contains("sources tree"),
            "must say it resolves outside the sources tree: {msg}"
        );
        assert_eq!(e.kind(), "unsafe-clone-path", "kind slug must be stable");
    }

    // spec: HOOK-105
    #[test]
    fn ambiguous_hook_target_names_both_disambiguated_forms() {
        // The message must name the ambiguous target string once as the source
        // form (prefixed with `source:`) and once as a kind-qualified item ref,
        // so a reader can copy either escape directly.
        let e = MindError::AmbiguousHookTarget {
            target: "local/base/repo#dev".into(),
            item_forms: vec!["local/base/repo#skill:dev".into()],
        };
        let msg = e.to_string();
        assert!(
            msg.contains("local/base/repo#dev"),
            "must name the ambiguous target: {msg}"
        );
        assert!(
            msg.contains("source:local/base/repo#dev"),
            "must name the source-targeting escape: {msg}"
        );
        assert!(
            msg.contains("local/base/repo#skill:dev"),
            "must name the kind-qualified item escape: {msg}"
        );
        assert_eq!(
            e.kind(),
            "ambiguous-hook-target",
            "kind slug must be stable"
        );
    }

    #[test]
    fn self_update_policy_displays_detail() {
        // spec: POL-52 -- the disabled case reads "self-update is disabled by the
        // managed policy" (carried as `detail`).
        let disabled = MindError::SelfUpdatePolicy {
            detail: "self-update is disabled by the managed policy".into(),
        }
        .to_string();
        assert!(
            disabled.contains("disabled by the managed policy"),
            "disabled detail must appear: {disabled}"
        );

        // spec: POL-53 -- the pin-mismatch case names the pin and the conflict.
        let mismatch = MindError::SelfUpdatePolicy {
            detail:
                "managed policy pins self-update to 0.14.0; --version 0.15.0 conflicts with the pin"
                    .into(),
        }
        .to_string();
        assert!(mismatch.contains("0.14.0"), "must name the pin: {mismatch}");
        assert!(
            mismatch.contains("0.15.0"),
            "must name the requested version: {mismatch}"
        );
        assert!(
            mismatch.contains("conflicts"),
            "must say 'conflicts': {mismatch}"
        );
    }

    // ---- DSC-91: shared metadata size cap ----------------------------------

    use std::sync::atomic::{AtomicU32, Ordering};
    static CAP_N: AtomicU32 = AtomicU32::new(0);

    fn cap_tmp(label: &str) -> PathBuf {
        let n = CAP_N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("mind-metacap-{}-{label}-{n}", std::process::id()))
    }

    #[test]
    fn read_capped_metadata_reads_a_normal_file() {
        // spec: DSC-91
        let path = cap_tmp("ok");
        std::fs::write(&path, "hello metadata").unwrap();
        let text = read_capped_metadata(&path).expect("small file must read fine");
        assert_eq!(text, "hello metadata");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_capped_metadata_refuses_an_oversized_file_naming_it() {
        // spec: DSC-91
        // A file at (limit + 1) bytes must be refused with MetadataTooLarge
        // naming the path and the limit. Built as a sparse file (`set_len`) so
        // the test itself never allocates/writes METADATA_SIZE_LIMIT bytes.
        let path = cap_tmp("big");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(METADATA_SIZE_LIMIT + 1).unwrap();
        drop(file);
        let err = read_capped_metadata(&path).expect_err("oversized file must be refused");
        match &err {
            MindError::MetadataTooLarge { path: p, limit } => {
                assert_eq!(p, &path, "error must name the offending file");
                assert_eq!(*limit, METADATA_SIZE_LIMIT, "error must name the limit");
            }
            other => panic!("expected MetadataTooLarge, got: {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains(&path.display().to_string()),
            "message must name the file: {msg}"
        );
        assert!(msg.contains("8 MiB"), "message must name the limit: {msg}");
        assert!(
            msg.contains("trim") || msg.contains("move"),
            "message must say what to do: {msg}"
        );
        assert_eq!(err.kind(), "metadata-too-large", "kind slug must be stable");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_capped_metadata_a_file_exactly_at_the_limit_is_accepted() {
        // spec: DSC-91 -- the cap is exclusive: exactly `limit` bytes must pass.
        let path = cap_tmp("exact");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(METADATA_SIZE_LIMIT).unwrap();
        drop(file);
        let text = read_capped_metadata(&path).expect("a file exactly at the cap must be OK");
        assert_eq!(text.len() as u64, METADATA_SIZE_LIMIT);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_capped_metadata_missing_file_is_plain_io_error() {
        // spec: DSC-91 -- a nonexistent file surfaces as a plain Io error, not
        // MetadataTooLarge, so callers that treat "absent" specially still can.
        let path = cap_tmp("missing");
        let err = read_capped_metadata(&path).unwrap_err();
        assert!(
            matches!(err, MindError::Io { .. }),
            "missing file must be a plain Io error: {err:?}"
        );
    }
}
