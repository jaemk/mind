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

/// An item kind as accepted on the command line (`--kind skill|agent|rule`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KindArg {
    Skill,
    Agent,
    Rule,
}

impl KindArg {
    pub fn to_kind(self) -> ItemKind {
        match self {
            KindArg::Skill => ItemKind::Skill,
            KindArg::Agent => ItemKind::Agent,
            KindArg::Rule => ItemKind::Rule,
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
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Meld with a source repo so its items become available.
    Meld {
        /// Repo spec: `owner/repo`, `github:owner/repo`, or a full git URL.
        /// Defaults to the current directory (`.`) when omitted (CLI-25).
        repo: Option<String>,

        /// Namespace every item from this source under this prefix
        /// (overrides the repo's own `[source].prefix`).
        #[arg(long = "as", value_name = "PREFIX")]
        alias: Option<String>,

        /// Track a named branch (overrides the repo's [source] pin directive).
        /// At most one of --follow-branch, --pin-tag, --pin-ref may be given
        /// (CLI-17: more than one is a `ConflictingPin` error).
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
        /// Persisted on the source and used by later scans and sync (CLI-16).
        #[arg(long = "root", value_name = "DIR")]
        roots: Vec<String>,

        /// Supply or override the source's install hook: a shell command run
        /// after checkout to build the tooling its items rely on. Before it runs,
        /// a prompt offers three choices: run it, skip it but still install the
        /// source (the default), or abort and install nothing. Overriding a
        /// declared `[source].install` is shown loudly in that prompt. Use
        /// `mind review <repo>` to see a source's declared hook before melding.
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

        /// Install the melded source's items without the confirmation prompt
        /// (also installs in a non-TTY context). Ignored with `--link-only`.
        #[arg(short = 'y', long)]
        yes: bool,

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

        /// Also uninstall every item installed from this source.
        #[arg(long)]
        forget: bool,
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

        /// Show what would be installed without installing anything.
        #[arg(short = 'n', long = "dry-run")]
        dry_run: bool,

        /// Install the dependency closure without the interactive [y/N] prompt.
        #[arg(short = 'y', long = "yes")]
        yes: bool,

        /// Overwrite a link target that already exists and is not managed by
        /// mind (a user's file/dir/foreign link). Without it, a conflict prompts
        /// on a TTY and otherwise refuses.
        #[arg(short = 'f', long)]
        force: bool,
    },

    /// Remove an installed item, or many via a glob.
    #[command(visible_alias = "unlearn")]
    Forget {
        /// The installed item ref or glob: `name`, `skill:name`, `'review*'`, `'*'`.
        item: String,
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
        /// Apply upgrades without the interactive [y/N] prompt.
        #[arg(short = 'y', long = "yes")]
        yes: bool,

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
        /// Replace the binary without the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Update to this exact version instead of the latest release.
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// List installed items, or show one item's details.
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

        /// Emit JSON instead of the human-readable table.
        #[arg(long)]
        json: bool,
    },

    /// Search melded catalogs for available items, or launch the interactive TUI.
    ///
    /// With a TTY and no opt-out, `probe` launches the interactive browser.
    /// Falls back to the catalog listing when `--no-tui`, `--json`, or stdout
    /// is not a TTY (piped or redirected). The query, `--kind`, and `--source`
    /// arguments seed the initial search/filter state in both modes.
    Probe {
        /// Case-insensitive substring matched against item names and descriptions; empty lists everything.
        query: Option<String>,

        /// Only list items of this kind.
        #[arg(long, value_enum)]
        kind: Option<KindArg>,

        /// Only list items from a source matching this selector.
        #[arg(long)]
        source: Option<String>,

        /// Emit JSON instead of the human-readable table.
        #[arg(long)]
        json: bool,

        /// Skip the interactive TUI and use the plain catalog listing.
        #[arg(long)]
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
        /// spec. Defaults to the current directory (`.`) when omitted (CLI-26).
        /// Cannot be used with `--policy`.
        #[arg(conflicts_with = "policy")]
        target: Option<String>,

        /// Evaluate the source under this prospective prefix (affects effective
        /// names, `{{ns:}}` expansion, and the unguarded-reference scan).
        /// Ignored when `--policy` is given.
        #[arg(long = "as", value_name = "PREFIX")]
        alias: Option<String>,

        /// Validate a managed policy TOML file at this path instead of a source.
        /// Cannot be used with `<target>`; supply exactly one of the two.
        #[arg(long, value_name = "PATH", conflicts_with = "target")]
        policy: Option<std::path::PathBuf>,
    },

    /// Diagnose drift, broken symlinks, and unsynced sources.
    Introspect {
        /// Repair what is fixable without changing versions (recreate missing links).
        #[arg(long)]
        fix: bool,

        /// Emit JSON instead of the human-readable report.
        #[arg(long)]
        json: bool,
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
    /// Add an agent home.
    Add {
        /// Directory to link items into (a leading `~` is expanded at use).
        path: String,
    },

    /// List configured agent homes.
    List,

    /// Remove an agent home.
    Remove {
        /// The configured directory to drop.
        path: String,
    },
}
