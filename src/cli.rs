//! The `mind` command-line surface.
//!
//! Verbs use a knowledge metaphor:
//!   meld   -> connect to a source repo
//!   learn  -> install an item
//!   forget -> remove an item
//!   sync   -> refresh source catalogs
//!   upgrade -> upgrade installed items
//!   evolve -> upgrade the mind binary itself
//!   recall -> show what's installed
//!   probe  -> find available items
//!   introspect -> diagnose drift

use clap::{Parser, Subcommand, ValueEnum};

use crate::error::ItemKind;

/// An item kind as accepted on the command line (`--kind skill|agent|rule|tool`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KindArg {
    Skill,
    Agent,
    Rule,
    Tool,
}

impl KindArg {
    pub fn to_kind(self) -> ItemKind {
        match self {
            KindArg::Skill => ItemKind::Skill,
            KindArg::Agent => ItemKind::Agent,
            KindArg::Rule => ItemKind::Rule,
            KindArg::Tool => ItemKind::Tool,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "mind",
    version,
    about = "A manager for agent tooling: skills, agents, and rules.",
    propagate_version = true
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of formatted text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Skip confirmation prompts (assume yes).
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Force plain ASCII output (no color, no Unicode glyphs).
    #[arg(long, global = true)]
    pub ascii: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Meld with a source repo so its items become available.
    Meld {
        /// Repo spec to meld. Supported forms:
        ///
        /// - Local path:   `/path/to/repo`  or  `file:///path/to/repo`  or  `.`
        ///
        /// - GitHub HTTPS: `owner/repo`  or  `https://github.com/owner/repo`
        ///
        /// - GitHub SSH:   `git@github.com:owner/repo`
        ///
        /// - Any git URL:  `https://example.com/repo.git`
        ///
        /// Defaults to the current directory (`.`) when omitted.
        repo: Option<String>,

        /// Namespace every item from this source under this prefix
        /// (overrides the repo's own `[source].prefix`). `--as` is a
        /// hidden deprecated alias for backwards compatibility.
        #[arg(short = 'n', long = "namespace", alias = "as", value_name = "PREFIX")]
        alias: Option<String>,

        /// Track a named branch (overrides the repo's [source] pin directive).
        /// At most one of --follow-branch, --pin-tag, --pin-ref may be given;
        /// more than one is an error.
        #[arg(long, value_name = "BRANCH")]
        follow_branch: Option<String>,

        /// Fix to a tag (overrides the repo's [source] pin directive).
        /// At most one of --follow-branch, --pin-tag, --pin-ref may be given.
        #[arg(long, value_name = "TAG")]
        pin_tag: Option<String>,

        /// Fix to a specific commit (overrides the repo's [source] pin directive).
        /// At most one of --follow-branch, --pin-tag, --pin-ref may be given.
        #[arg(long, value_name = "COMMIT")]
        pin_ref: Option<String>,

        /// Set the source's convention-scan roots to one or more repo-root-relative
        /// directories (repeatable). Overrides `[source].roots` in mind.toml.
        /// Persisted on the source and used by later scans and sync.
        #[arg(long = "root", value_name = "DIR")]
        roots: Vec<String>,

        /// Force-enable flat skill discovery: skills are bare-name directories at
        /// each scan root, with no `skills/` container. Turns the layout on for a
        /// source that did not declare `[source].flat-skills`; there is no way to
        /// disable a source's declared flat layout. Applies to skills only (agent,
        /// rule, and tool discovery are unaffected) and to convention discovery
        /// only (ignored for an authoritative `mind.toml`). Persisted on the source
        /// and used by later scans and sync.
        #[arg(long)]
        flat_skills: bool,

        /// Supply or override the source's install hook: a shell command run
        /// after checkout to build the tooling its items rely on. Before it runs,
        /// a prompt offers three choices: run it (the default, a bare Enter),
        /// skip it but still install the source, or abort and install nothing.
        /// Overriding a declared `[source].install` is shown loudly in that
        /// prompt. Use `mind review <repo>` to see a source's declared hook
        /// before melding.
        #[arg(long, value_name = "CMD")]
        install_hook: Option<String>,

        /// Run the install hook without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY run (CI, scripts) skips the hook and just
        /// prints a note, so the tooling is not built; pass this to run it unattended.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,

        /// Only register the source; do not prompt to install its items. By
        /// default, `meld` previews the source's items and offers to install them
        /// all (the interactive form of `learn '<source>#*'`).
        #[arg(long)]
        link_only: bool,

        /// Offer to install the items of every nested source a super-source
        /// curates (`[discover].sources`), not just the super-source's own items
        /// and the nested sources the curator marked `install = true`.
        #[arg(short = 'r', long)]
        recursive: bool,

        /// When installing, overwrite link targets that already exist and are not
        /// managed by mind. Without it, a conflict prompts on a TTY.
        #[arg(short = 'f', long)]
        force: bool,
    },

    /// Scaffold a `mind.toml` and report the references among a source's items.
    ///
    /// For source maintainers: discovers the items the repo offers, reports which
    /// items reference which siblings, and creates a starter `mind.toml` if none
    /// exists. With `--template`, also rewrites bare sibling references into
    /// `{{ns:}}` tokens so the source stays resolvable under a prefix.
    InitSource {
        /// The source repo directory (default the current directory).
        path: Option<String>,

        /// Rewrite bare sibling references into `{{ns:name}}` tokens. This edits
        /// the repo's item files; it is heuristic, so review the result.
        #[arg(long)]
        template: bool,
    },

    /// Unmeld a source, removing its clone and catalog entry.
    #[command(visible_alias = "detach")]
    Unmeld {
        /// The source name (see `mind recall --sources`).
        name: String,

        /// Remove only the source, leaving its installed items in place (the
        /// opt-out from the default item removal, mirroring `meld --link-only`).
        #[arg(long)]
        unlink_only: bool,

        /// Supply or override the source's uninstall hook: a shell command run
        /// in the clone before the source is removed. Replaces the source's
        /// declared uninstall hook(s); the override is shown loudly in the prompt.
        #[arg(long, value_name = "CMD")]
        uninstall_hook: Option<String>,

        /// Run uninstall hooks without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY run skips the hook and prints a note.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,
    },

    /// Install items into every configured agent home (default ~/.claude).
    ///
    /// The item ref may be exact, or a glob to install many: `'*'` for everything,
    /// `'skill:*'` for all skills, `'owner/repo#*'` for all of one source.
    ///
    /// When selecting a subset of a source's items, `learn` also installs the
    /// intra-source `{{ns:}}` dependencies those items reference (the closure),
    /// printing a dependency tree and prompting before installing; `--dry-run`
    /// previews the closure without installing anything and `--yes` skips the prompt.
    Learn {
        /// Item ref or glob: `name`, `skill:name`, `owner/repo#name`, `'review*'`, `'*'`.
        item: String,

        /// Install every item of the source named by the ref. Shorthand for the
        /// `<source>#*` selector; rejected if the ref already has a `#` selector.
        #[arg(long)]
        all: bool,

        /// Show what would be installed without installing anything.
        #[arg(short = 'n', long = "dry-run")]
        dry_run: bool,

        /// Overwrite a link target that already exists and is not managed by
        /// mind (a user's file/dir/foreign link). Without it, a conflict prompts
        /// on a TTY and otherwise refuses.
        #[arg(short = 'f', long)]
        force: bool,

        /// Run an item's install hook without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY run skips the hook and prints a note.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,
    },

    /// Remove an installed item, or many via a glob.
    ///
    /// With `--unmanaged`, the removal is scoped to unmanaged lobe items only
    /// (the deliberate inverse of the default, which matches managed items only).
    /// With no `<item>` and `--unmanaged`, every unmanaged item across all
    /// configured lobes is removed.
    #[command(visible_alias = "unlearn")]
    Forget {
        /// The installed item ref or glob: `name`, `skill:name`, `'review*'`, `'*'`.
        /// With `--unmanaged`: the ref or glob scopes removal to unmanaged items only;
        /// omit to remove every unmanaged item across all lobes.
        #[arg(required_unless_present = "unmanaged")]
        item: Option<String>,

        /// Scope removal to unmanaged lobe items only (the deliberate inverse of the
        /// default, which matches managed items only). With no ref it removes every
        /// unmanaged item across all configured lobes.
        #[arg(long)]
        unmanaged: bool,

        /// Skip the dependents confirmation when removing a single item that other
        /// installed items depend on (DEP-60). Without this flag, `forget` warns and
        /// prompts; with it, removal proceeds immediately. Does not affect the
        /// multi-item glob confirmation (CLI-42).
        #[arg(short = 'f', long)]
        force: bool,

        /// Run an item's uninstall hook without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY run skips the hook and prints a note.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,
    },

    /// Refresh every melded source's clone and catalog.
    Sync {
        /// After refreshing, run an `upgrade` pass (report + prompt) to apply upgrades.
        #[arg(long)]
        upgrade: bool,

        /// Run install-hook re-runs without the safety prompt during the
        /// `--upgrade` pass (executes arbitrary code; only with `--upgrade`).
        #[arg(long, requires = "upgrade")]
        dangerously_skip_install_hook_check: bool,
    },

    /// Upgrade installed items to their latest source version.
    ///
    /// By default this only *reports* pending upgrades (hash and commit deltas
    /// plus a compare link per source) and prompts before changing anything.
    Upgrade {
        /// Only upgrade this item; default is every installed item.
        item: Option<String>,

        /// Re-run a source's install hook without the safety prompt when its
        /// commit advanced. This executes arbitrary code from the source; only
        /// use it for a source you trust. Without this flag, a non-TTY upgrade
        /// (CI, scripts) skips the hook re-run and just prints a note; pass this
        /// to run it unattended.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,
    },

    /// Update the `mind` binary itself to the latest release (or `--version`).
    ///
    /// Downloads the release binary for this platform and replaces the running
    /// executable in place. `--check` reports whether an update is available and
    /// changes nothing. Without `--yes` it prompts before replacing.
    // Disable clap's auto `--version` flag on this subcommand so the explicit
    // `--version <VERSION>` argument below (pin a target release) owns the name.
    #[command(disable_version_flag = true)]
    Evolve {
        /// Report whether an update is available, then exit without changing anything.
        #[arg(long)]
        check: bool,
        /// Update to this exact version instead of the latest release.
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// List installed items, or show one item's details.
    #[command(visible_alias = "status")]
    Recall {
        /// Show melded sources instead of installed items.
        #[arg(long)]
        sources: bool,

        /// Show details for a single installed item.
        item: Option<String>,

        /// Only list items of this kind (listing only).
        #[arg(long, value_enum)]
        kind: Option<KindArg>,

        /// Only list items from a source matching this selector (listing only).
        #[arg(long)]
        source: Option<String>,

        /// Render installed items as a dependency forest (DEP-61). With no item,
        /// shows the full forest; with an item ref, scopes to that item's subtree.
        #[arg(long)]
        tree: bool,
    },

    /// Search melded catalogs for available items, or launch the interactive TUI.
    ///
    /// With a TTY and no opt-out, `probe` launches the interactive browser.
    /// Falls back to the catalog listing when `--no-tui`, `--json` (global), or
    /// stdout is not a TTY (piped or redirected). The query, `--kind`, and
    /// `--source` arguments seed the initial search/filter state in both modes.
    Probe {
        /// Case-insensitive substring matched against item names and descriptions; empty lists everything.
        query: Option<String>,

        /// Only list items of this kind.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,

        /// Only list items from a source matching this selector.
        #[arg(long)]
        source: Option<String>,

        /// Skip the interactive TUI and use the plain catalog listing.
        // TUI-3: `-n` is the subcommand-scoped short for `--no-tui`; it does not
        // clash with `learn`'s `-n` (`--dry-run`) since clap shorts are local.
        #[arg(long, short = 'n')]
        no_tui: bool,
    },

    /// Validate a source or a managed policy file (author-side, read-only).
    ///
    /// Source mode: `<target>` may be a local path, a melded-source selector
    /// (same forms as `unmeld`), or a repo spec (same forms as `meld`). A repo
    /// spec is shallow-cloned to a temp area and removed afterward. With no
    /// `<target>`, the current directory is validated.
    ///
    /// Policy mode: `--policy <path>` validates a managed policy TOML file at an
    /// explicit path without consulting the system policy path or env. Supplying
    /// both `<target>` and `--policy` is an error.
    Review {
        /// The source to validate: a local path, melded-source selector, or repo
        /// spec. Defaults to the current directory (`.`) when omitted.
        /// Cannot be used with `--policy`.
        #[arg(conflicts_with = "policy")]
        target: Option<String>,

        /// Evaluate the source under this prospective prefix (affects effective
        /// names, `{{ns:}}` expansion, and the unguarded-reference scan).
        /// Ignored when `--policy` is given. `--as` is a hidden deprecated
        /// alias for backwards compatibility.
        #[arg(short = 'n', long = "namespace", alias = "as", value_name = "PREFIX")]
        alias: Option<String>,

        /// Validate a managed policy TOML file at this path instead of a source.
        /// Cannot be used with `<target>`; supply exactly one of the two.
        #[arg(long, value_name = "PATH", conflicts_with = "target")]
        policy: Option<std::path::PathBuf>,

        /// Rewrite the source in place: hardcoded install paths become tokens and
        /// bare sibling names become `{{ns:}}`. Local-path target only; the sole
        /// `review` mode that writes to disk. Ignored with `--policy`.
        #[arg(long, conflicts_with = "policy")]
        fix: bool,
    },

    /// Diagnose drift, broken symlinks, and unsynced sources.
    Introspect {
        /// Repair what is fixable without changing versions (recreate missing links).
        #[arg(long)]
        fix: bool,
    },

    /// View and edit configuration (`~/.mind/config.toml`).
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },

    /// Print a shell completion script to stdout.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Print the mind man page (roff) to stdout.
    Man,

    /// Write a super-source `mind.toml` reproducing the current melded and
    /// installed state so melding the output recreates the same source set.
    ///
    /// By default, `dump` filters each source to the items actually installed
    /// and stamps the per-entry install directive accordingly. Pass
    /// `--whole-sources` to emit `install = true` for every source regardless
    /// of how many items are installed.
    Dump {
        /// Write to this file instead of stdout.
        #[arg(long, value_name = "PATH")]
        output: Option<std::path::PathBuf>,

        /// Emit `install = true` for every melded source, ignoring how many
        /// of its items are currently installed.
        #[arg(long = "whole-sources")]
        whole_sources: bool,
    },

    /// Claim an unmanaged lobe item into a version-controlled source and install
    /// it as a managed item (the constructive inverse of `forget --unmanaged`).
    ///
    /// Resolves <ref> to a single unmanaged item (an exact `kind:name`; a kind
    /// prefix disambiguates across kinds). Moves the item to the destination source
    /// at the convention path for its kind (`skills/<name>/`, `agents/<name>.md`,
    /// `rules/<name>.md`), commits it, melds the source if not yet registered, and
    /// installs it via `learn`. After absorb the item is an ordinary managed item.
    ///
    /// The destination source is resolved from, in precedence order:
    ///   1. `--to <path>` (this flag) takes precedence over all others
    ///   2. `MIND_ABSORB_TO` environment variable
    ///   3. `absorb_to` key in `~/.mind/config.toml`
    ///
    /// If none of these is set and the run is interactive, `absorb` prompts for
    /// a destination and offers `~/.mind/personal` as the built-in default,
    /// creating and `git init`-ing it on demand. Passing `--yes` with no configured
    /// destination automatically uses (and persists) `~/.mind/personal` without
    /// prompting. In a non-interactive (non-TTY) run with no destination configured,
    /// `absorb` refuses with an error.
    Absorb {
        /// The unmanaged item ref: `name`, `skill:name`, `agent:name`, or
        /// `rule:name`. A kind prefix disambiguates when the same name exists
        /// across kinds. Glob refs are rejected (absorb claims exactly one item).
        item_ref: String,

        /// Destination source directory. Takes precedence over `MIND_ABSORB_TO`
        /// and the `absorb_to` key in `config.toml`.
        #[arg(long, value_name = "PATH")]
        to: Option<String>,

        /// Overwrite the destination convention path if it already exists
        /// (a `kind:name` collision). Without `--force`, a collision is an error.
        #[arg(short = 'f', long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print the current config and where the file lives.
    Show,

    /// Manage agent homes ("lobes") - the directories items are linked into.
    #[command(visible_alias = "target")]
    Lobes {
        #[command(subcommand)]
        action: LobesCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum LobesCmd {
    /// Add an agent home, by path or by a `--preset <name>` harness preset.
    Add {
        /// Directory to link items into (a leading `~` is expanded at use).
        /// Mutually exclusive with `--preset`; give exactly one.
        path: Option<String>,

        /// Add a known harness preset (gemini, codex, antigravity,
        /// antigravity-cli, universal): its parent path and kinds filter.
        #[arg(long, value_name = "NAME", conflicts_with = "path")]
        preset: Option<String>,
    },

    /// List configured agent homes.
    List,

    /// Detect installed harness homes and offer to add their presets.
    /// Honors the global `-y`/`--yes` flag to add without prompting.
    Detect,

    /// Remove an agent home.
    Remove {
        /// The configured directory to drop.
        path: String,
    },
}
