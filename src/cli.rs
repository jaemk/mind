//! The `mind` command-line surface.
//!
//! Verbs use a knowledge metaphor:
//!   meld   -> connect to a source repo
//!   learn  -> install an item
//!   forget -> remove an item
//!   sync   -> refresh source catalogs
//!   evolve -> upgrade installed items
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
        repo: String,

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
    Learn {
        /// Item ref or glob: `name`, `skill:name`, `owner/repo#name`, `'review*'`, `'*'`.
        item: String,

        /// Show what would be installed without installing anything.
        #[arg(short = 'n', long = "dry-run")]
        dry_run: bool,
    },

    /// Remove an installed item, or many via a glob.
    #[command(visible_alias = "unlearn")]
    Forget {
        /// The installed item ref or glob: `name`, `skill:name`, `'review*'`, `'*'`.
        item: String,
    },

    /// Refresh every melded source's clone and catalog.
    Sync {
        /// After refreshing, run an `evolve` pass (report + prompt) to apply upgrades.
        #[arg(long)]
        evolve: bool,
    },

    /// Upgrade installed items to their latest source version.
    ///
    /// By default this only *reports* pending upgrades (hash and commit deltas
    /// plus a compare link per source) and prompts before changing anything.
    Evolve {
        /// Apply upgrades without the interactive [y/N] prompt.
        #[arg(short = 'y', long = "yes")]
        yes: bool,

        /// Only upgrade this item; default is every installed item.
        item: Option<String>,
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

    /// Search melded catalogs for available items.
    Probe {
        /// Substring to match against item names; empty lists everything.
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
    },

    /// Validate a source for publishing (author-side, read-only).
    ///
    /// `<target>` may be a local path, a melded-source selector (same forms as
    /// `unmeld`), or a repo spec (same forms as `meld`). A repo spec is
    /// shallow-cloned to a temp area and removed afterward.
    Review {
        /// The source to validate: a local path, melded-source selector, or repo spec.
        target: String,

        /// Evaluate the source under this prospective prefix (affects effective
        /// names, `{{ns:}}` expansion, and the unguarded-reference scan).
        #[arg(long = "as", value_name = "PREFIX")]
        alias: Option<String>,
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
