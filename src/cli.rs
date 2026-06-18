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

use clap::{Parser, Subcommand};

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
    },

    /// Unmeld a source, removing its clone and catalog entry.
    #[command(visible_alias = "detach")]
    Unmeld {
        /// The source name (see `mind recall --sources`).
        name: String,
    },

    /// Install items into ~/.claude.
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

    /// Remove an installed item.
    #[command(visible_alias = "unlearn")]
    Forget {
        /// The installed item ref.
        item: String,
    },

    /// Refresh every melded source's clone and catalog.
    Sync,

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
    },

    /// Search melded catalogs for available items.
    Probe {
        /// Substring to match against item names; empty lists everything.
        query: Option<String>,
    },

    /// Diagnose drift, broken symlinks, and unsynced sources.
    Introspect,
}
