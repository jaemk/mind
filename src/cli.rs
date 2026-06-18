//! The `mind` command-line surface.
//!
//! Verb theme mirrors `brew` but on a knowledge metaphor:
//!   brew tap     -> mind meld     (connect to a repo)
//!   brew install -> mind learn    (install an item)
//!   brew uninstall -> mind forget (remove an item)
//!   brew update  -> mind sync     (refresh source catalogs)
//!   brew upgrade -> mind evolve   (upgrade installed items)
//!   brew list    -> mind recall   (what's installed)
//!   brew search  -> mind probe    (find available items)
//!   brew doctor  -> mind introspect (diagnose drift)

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "mind",
    version,
    about = "A brew-like manager for agent tooling: skills, agents, and rules.",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Meld with a source repo so its items become available (like `brew tap`).
    Meld {
        /// Repo spec: `owner/repo`, `github:owner/repo`, or a full git URL.
        repo: String,

        /// Namespace every item from this source under this prefix
        /// (overrides the repo's own `[source].prefix`).
        #[arg(long = "as", value_name = "PREFIX")]
        alias: Option<String>,
    },

    /// Unmeld a source, removing its clone and catalog entry.
    Unmeld {
        /// The source name (see `mind recall --sources`).
        name: String,
    },

    /// Install items into ~/.claude (like `brew install`).
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

    /// Remove an installed item (like `brew uninstall`).
    Forget {
        /// The installed item ref.
        item: String,
    },

    /// Refresh every melded source's clone and catalog (like `brew update`).
    Sync,

    /// Upgrade installed items to their latest source version (like `brew upgrade`).
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

    /// List installed items, or show one item's details (like `brew list` / `info`).
    Recall {
        /// Show melded sources instead of installed items.
        #[arg(long)]
        sources: bool,

        /// Show details for a single installed item.
        item: Option<String>,
    },

    /// Search melded catalogs for available items (like `brew search`).
    Probe {
        /// Substring to match against item names; empty lists everything.
        query: Option<String>,
    },

    /// Diagnose drift, broken symlinks, and unsynced sources (like `brew doctor`).
    Introspect,
}
