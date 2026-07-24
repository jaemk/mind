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

/// The lifecycle event for `mind hooks run --event`.
// spec: CLI-195
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookEventArg {
    /// Run install hooks (default). Valid for source and item targets.
    Install,
    /// Run uninstall hooks. Valid for source and item targets.
    Uninstall,
    /// Re-install the item through the transactional build path (stage, build,
    /// swap). Valid for item targets only; a source has no build hook.
    Build,
}

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
    about = "A manager for agent tooling: skills, agents, rules, and tools.",
    propagate_version = true
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of formatted text.
    #[arg(long, global = true, help_heading = "Global options")]
    pub json: bool,

    /// Skip confirmation prompts (assume yes).
    #[arg(short = 'y', long, global = true, help_heading = "Global options")]
    pub yes: bool,

    /// Force plain ASCII output (no color, no Unicode glyphs).
    #[arg(long, global = true, help_heading = "Global options")]
    pub ascii: bool,

    /// Emit extra advisory output (e.g. unguarded-reference warnings on meld).
    #[arg(short = 'v', long, global = true, help_heading = "Global options")]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Meld with a source repo and install its items (prompts to install; use `--register-only` to skip).
    // spec: CLI-173
    #[command(visible_alias = "add")]
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
        /// - Item link:    `https://host/owner/repo/tree/<ref>/<path>` (or the
        ///   `blob/.../SKILL.md` form): registers a single-item source offering
        ///   just that skill
        ///
        /// Defaults to the current directory (`.`) when omitted.
        repo: Option<String>,

        /// Namespace every item from this source under this prefix
        /// (overrides the repo's own `[source].prefix`). The prefix is part of
        /// the source's identity: melding an already-melded repo under a
        /// different prefix registers a distinct `host/owner/repo@<prefix>`
        /// instance that coexists with the original, rather than re-prefixing it.
        /// `--as` is a hidden deprecated alias for backwards compatibility.
        // spec: CLI-163 - short moved to -N (uppercase); -n is reserved for --dry-run.
        #[arg(short = 'N', long = "namespace", alias = "as", value_name = "PREFIX")]
        alias: Option<String>,

        /// Set the version this source tracks (overrides the repo's [source] pin
        /// directive). Takes one required value. `HEAD` freezes the current
        /// resolved tip (the point that would otherwise be melded) to its commit.
        /// A bare `<tag|sha|branch>` resolves that ref to its current commit and
        /// freezes it (a snapshot, not a track). `branch=<name>` follows that
        /// branch (floating; `sync` advances it); `tag=<name>` follows that tag
        /// (re-points on `sync` if it moves). With no `--pin`, the repo's
        /// `[source]` pin directive (else the remote default branch) applies.
        // spec: CLI-200, CLI-201
        #[arg(long, value_name = "HEAD|REF|branch=NAME|tag=NAME")]
        pin: Option<String>,

        /// Deprecated: use `--pin branch=<name>`.
        // spec: CLI-202
        #[arg(long, value_name = "BRANCH", hide = true)]
        follow_branch: Option<String>,

        /// Deprecated: use `--pin tag=<name>`.
        // spec: CLI-202
        #[arg(long, value_name = "TAG", hide = true)]
        pin_tag: Option<String>,

        /// Deprecated: use `--pin <commit>`.
        // spec: CLI-202
        #[arg(long, value_name = "COMMIT", hide = true)]
        pin_ref: Option<String>,

        /// Set the source's convention-scan roots to one or more repo-root-relative
        /// directories (repeatable). Overrides `[source].roots` in mind.toml.
        /// Persisted on the source and used by later scans and sync.
        #[arg(long = "root", value_name = "DIR")]
        roots: Vec<String>,

        /// Add convention-scan roots that compose with the source's own
        /// discovery (repeatable): a plugin/marketplace manifest or an
        /// authoritative mind.toml keeps its items and each added root is
        /// convention-scanned in addition, so items the source does not
        /// declare become installable. Unlike --root, this does not override
        /// or suppress anything. Persisted on the source and used by later
        /// scans and sync.
        #[arg(long = "add-root", value_name = "DIR")]
        add_roots: Vec<String>,

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

        /// Run item build hooks without the safety prompt during the install-all
        /// pass. This executes arbitrary code from the source; only use it for a
        /// source you trust. Without this flag, a non-TTY run skips build hooks
        /// and prints a note, so the item's tooling is not built.
        #[arg(long)]
        dangerously_skip_build_hook_check: bool,

        /// Only register the source; do not prompt to install its items. By
        /// default, `meld` previews the source's items and offers to install them
        /// all (the interactive form of `learn '<source>#*'`).
        // spec: CLI-165 - canonical name; --link-only is a hidden deprecated alias.
        #[arg(long, alias = "link-only")]
        register_only: bool,

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

        /// Generate `.claude-plugin/marketplace.json` for Claude plugin
        /// compatibility. Skipped if the file already exists.
        #[arg(long)]
        marketplace: bool,

        /// Set `flat-skills = true` in `[source]` in `mind.toml` and use flat
        /// skill layout for discovery. With `--marketplace`, populates the
        /// `skills` array in the generated manifest.
        #[arg(long)]
        flat_skills: bool,

        /// Override the plugin name / `[source].prefix` written to `mind.toml`
        /// and the marketplace manifest `name` field.
        // spec: CLI-163 - short moved to -N (uppercase).
        #[arg(short = 'N', long)]
        namespace: Option<String>,
    },

    /// Unmeld a source, uninstalling every item the source installed.
    ///
    /// Unmelds a source and uninstalls every item the source installed; use
    /// `--keep-items` to keep them.
    ///
    /// The source name may be the full `host/owner/repo` or an unambiguous
    /// trailing suffix (e.g. `repo` or `owner/repo`). A glob removes all
    /// matching sources.
    // spec: CLI-174 - long_about leads with the uninstall default.
    // spec: CLI-172 - the former `detach` alias is removed.
    Unmeld {
        /// The source name (see `mind recall --sources`).
        name: String,

        /// Remove only the source, leaving its installed items in place (the
        /// opt-out from the default item removal, mirroring `meld --register-only`).
        // spec: CLI-166 - canonical name; --unlink-only is a hidden deprecated alias.
        #[arg(long, alias = "unlink-only")]
        keep_items: bool,

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
    // spec: CLI-172
    #[command(visible_alias = "install")]
    Learn {
        /// Item ref or glob: `name`, `skill:name`, `owner/repo#name`, `'review*'`, `'*'`.
        /// Also accepts a deep tree/blob URL to one skill
        /// (`https://host/owner/repo/tree/<ref>/<path>`): the repo registers as a
        /// single-item source and the skill installs in one step.
        item: String,

        /// Install every item of the source named by the ref. Shorthand for the
        /// `<source>#*` selector; rejected if the ref already has a `#` selector.
        #[arg(long)]
        all: bool,

        /// For a deep tree/blob URL, freeze the link's branch ref to its current
        /// commit when registering the single-item source, so the instance is an
        /// immutable snapshot instead of tracking the branch. This is a bare flag
        /// (unlike `meld --pin`, which takes a value): the ref comes from the URL.
        /// Ignored (with a note) for a non-URL item ref, which names an
        /// already-melded source.
        // spec: CLI-200
        #[arg(long)]
        pin: bool,

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

        /// Run an item's build hook without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY run skips the build hook and prints a
        /// note, so the item's tooling is not built.
        #[arg(long)]
        dangerously_skip_build_hook_check: bool,
    },

    /// Remove an installed item, or many via a glob.
    ///
    /// With `--unmanaged`, the removal is scoped to unmanaged lobe items only
    /// (the deliberate inverse of the default, which matches managed items only).
    /// With no `<item>` and `--unmanaged`, every unmanaged item across all
    /// configured lobes is removed.
    // spec: CLI-172 - added `uninstall` visible alias; `unlearn` kept.
    #[command(visible_aliases = ["unlearn", "uninstall"])]
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
    // spec: CLI-172
    #[command(visible_alias = "update")]
    Sync {
        /// After refreshing, run an `upgrade` pass (report + prompt) to apply upgrades.
        /// Deprecated: prefer `mind upgrade` which now syncs first by default (CLI-169).
        #[arg(long)]
        upgrade: bool,

        /// Run install-hook re-runs without the safety prompt during the
        /// `--upgrade` pass (executes arbitrary code; only with `--upgrade`).
        #[arg(long, requires = "upgrade")]
        dangerously_skip_install_hook_check: bool,

        /// Run item build hooks without the safety prompt during the `--upgrade`
        /// pass. This executes arbitrary code from the source; only use it for a
        /// source you trust. Without this flag, a non-TTY run skips build hooks
        /// and prints a note. Only valid with `--upgrade`.
        #[arg(long, requires = "upgrade")]
        dangerously_skip_build_hook_check: bool,
    },

    /// Upgrade installed items to their latest source version.
    ///
    /// By default, syncs each involved source first (fetches from remote), then
    /// reports pending upgrades and prompts before applying. Use `--no-sync` to
    /// skip the fetch and compute deltas from the current clone.
    // spec: CLI-169
    Upgrade {
        /// Only upgrade this item; default is every installed item.
        item: Option<String>,

        /// Skip the automatic source sync before computing deltas. By default,
        /// `upgrade` fetches each involved source before checking for changes.
        // spec: CLI-169
        #[arg(long)]
        no_sync: bool,

        /// Re-run a source's install hook without the safety prompt when its
        /// commit advanced. This executes arbitrary code from the source; only
        /// use it for a source you trust. Without this flag, a non-TTY upgrade
        /// (CI, scripts) skips the hook re-run and just prints a note; pass this
        /// to run it unattended.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,

        /// Run item build hooks without the safety prompt. This executes
        /// arbitrary code from the source; only use it for a source you trust.
        /// Without this flag, a non-TTY upgrade skips build hooks and prints a
        /// note, so the item's tooling is not rebuilt.
        #[arg(long)]
        dangerously_skip_build_hook_check: bool,
    },

    /// Update the `mind` binary itself to the latest release (or `--version`).
    ///
    /// Downloads the release binary for this platform and replaces the running
    /// executable in place. `--check` reports whether an update is available and
    /// changes nothing. Without `--yes` it prompts before replacing.
    // Disable clap's auto `--version` flag on this subcommand so the explicit
    // `--version <VERSION>` argument below (pin a target release) owns the name.
    // spec: CLI-172
    #[command(disable_version_flag = true, visible_alias = "self-update")]
    Evolve {
        /// Report whether an update is available, then exit without changing anything.
        #[arg(long)]
        check: bool,
        /// Update to this exact version instead of the latest release.
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },

    /// List installed items, or show one item's details.
    // spec: CLI-172 - added `list` visible alias; `status` kept.
    #[command(visible_aliases = ["status", "list"])]
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
    // spec: CLI-172
    #[command(visible_alias = "search")]
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
        // spec: CLI-164, TUI-54 - long-only; former -n short removed (CLI-163 reserves -n for --dry-run).
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
        /// spec. Defaults to the current directory (`.`) when omitted.
        /// Cannot be used with `--policy`.
        #[arg(conflicts_with = "policy")]
        target: Option<String>,

        /// Evaluate the source under this prospective prefix (affects effective
        /// names, `{{ns:}}` expansion, and the unguarded-reference scan).
        /// Ignored when `--policy` is given. `--as` is a hidden deprecated
        /// alias for backwards compatibility.
        // spec: CLI-163 - short moved to -N (uppercase).
        #[arg(short = 'N', long = "namespace", alias = "as", value_name = "PREFIX")]
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
    // spec: CLI-172
    #[command(visible_alias = "doctor")]
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

    /// Link installed skills into a project's harness skills directory.
    ///
    /// A shorthand for `config lobes add [dir] --preset <name>`. The project
    /// directory `dir` defaults to the current working directory; `--preset`
    /// defaults to `windsurf` (lobe at `<dir>/.windsurf`, skill-only). Existing
    /// installed skills are linked (or, with `--snapshot`, copied as real files)
    /// into the project dir immediately.
    ///
    /// For a managed (non-snapshot) add, gitignore guidance is printed: the skills
    /// directory contains symlinks into `~/.mind/store` and should be gitignored so
    /// the symlinks are not committed.
    // spec: CLI-198 HARN-11
    #[command(name = "link-project")]
    LinkProject {
        /// Project directory to link into (default: current working directory).
        dir: Option<String>,

        /// Harness preset to use (default: windsurf).
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,

        /// Lobe subdirectory under the project dir instead of the preset default.
        /// Conflicts with `--preset`.
        #[arg(long, value_name = "REL", conflicts_with = "preset")]
        subdir: Option<String>,

        /// Write frozen real-file copies instead of registering a managed lobe.
        #[arg(long)]
        snapshot: bool,

        /// Overwrite a colliding target in snapshot mode.
        #[arg(short = 'f', long)]
        force: bool,
    },

    /// Print a shell completion script to stdout.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Print the mind man page (roff) to stdout.
    Man,

    /// Run or list a source's or item's hooks on demand, outside the
    /// meld/learn/forget/upgrade flows.
    ///
    /// Lets you run hooks that were skipped earlier, re-run a hook whose effect
    /// was later lost (a deleted build output or side effect), or re-run an
    /// install or uninstall whose prior run failed for a transient reason.
    ///
    /// Subcommands:
    ///   `hooks run <target>`  -- run hooks with the same disclosure+consent model
    ///   `hooks list <target>` -- list declared hooks without running any
    // spec: CLI-194 CLI-195 CLI-196
    Hooks {
        #[command(subcommand)]
        action: HooksCmd,
    },

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
    // spec: CLI-172 - the former `target` alias is removed.
    Lobes {
        #[command(subcommand)]
        action: LobesCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum LobesCmd {
    /// Add an agent home, by path or by a `--preset <name>` harness preset.
    ///
    /// With `--preset`, the lobe path is `[base]/preset.rel_path`. For a global
    /// preset (gemini, codex, universal) the default base is `~`; for a project
    /// preset (windsurf) the default base is cwd. An explicit positional `base`
    /// overrides the default. With `--subdir <REL>` (no preset), the lobe path is
    /// `[base]/<REL>` with a skill-only kinds filter; base defaults to cwd.
    // spec: HARN-10 CLI-199
    Add {
        /// Base directory for the lobe. With `--preset`, the lobe lives at
        /// `base/preset.rel_path`; with `--subdir`, at `base/<REL>`; as a bare
        /// argument, the lobe IS the directory (all kinds, no filter). A leading
        /// `~` is expanded. Omitting this for a project preset defaults to cwd;
        /// omitting it without any flag is `LobeTargetRequired`.
        path: Option<String>,

        /// Add a known harness preset (gemini, codex, universal, windsurf).
        /// Resolves the lobe path and kinds filter from the preset definition.
        /// May be combined with a positional base directory.
        #[arg(long, value_name = "NAME")]
        preset: Option<String>,

        /// Lobe subdirectory under the base (or cwd when base is omitted).
        /// The lobe path is `base/<REL>` with a skill-only kinds filter.
        /// Conflicts with `--preset`.
        #[arg(long, value_name = "REL", conflicts_with = "preset")]
        subdir: Option<String>,

        /// Write frozen real-file copies of installed items instead of
        /// registering a managed lobe. No config entry is created. A colliding
        /// target blocks the copy unless `--force` is also given.
        #[arg(long)]
        snapshot: bool,

        /// Overwrite a colliding target in snapshot mode.
        #[arg(short = 'f', long)]
        force: bool,
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

        /// Before removing the config entry, convert symlinks confined under
        /// this lobe to frozen real-file copies of the store content (detach
        /// mode). The links are stripped from the manifest and the entry is
        /// dropped from config. Without this flag, symlinks are left in place
        /// and the lobe is simply unregistered.
        // spec: HARN-12 CLI-199
        #[arg(long)]
        snapshot: bool,
    },
}

/// Subcommands of `mind hooks`.
// spec: CLI-194 CLI-195 CLI-196
#[derive(Debug, Subcommand)]
pub enum HooksCmd {
    /// Run a source's or item's hooks on demand, outside meld/learn/forget/upgrade.
    ///
    /// `<target>` is a source selector (e.g. `repo`, `owner/repo`, or a glob
    /// like `'*'`) or an item ref `<source>#<item>` (e.g. `agents#skill:scan`).
    /// A ref that matches several sources or items runs each in turn.
    ///
    /// For a source target with `--event install` (the default), only *pending*
    /// hooks run by default -- those that never ran or did not run at the current
    /// commit. `--force` re-runs every install hook regardless. For an item
    /// target, hooks always run (item hooks carry no recorded run state).
    ///
    /// Every hook goes through the same disclosure and consent prompt as an
    /// automatic run; it is never more silently than meld/learn would run it.
    // spec: CLI-194 CLI-195
    Run {
        /// The hook target: a source selector (e.g. `repo`, `owner/repo`, `'*'`)
        /// or an item ref `<source>#<item>` (e.g. `agents#skill:scan`).
        target: String,

        /// The lifecycle event to run (default: install).
        ///
        /// `install` and `uninstall` are valid for source and item targets.
        /// `build` is valid only for an item target and re-installs the item
        /// through the transactional path (stage, expand, build, swap), leaving
        /// the existing copy untouched if the build fails.
        #[arg(long, value_enum, default_value = "install")]
        event: HookEventArg,

        /// For a source install run: re-run every install hook even if it was
        /// already recorded at the current commit (for lost outputs or transient
        /// failures), mirroring `meld --force`.
        #[arg(long)]
        force: bool,

        /// Run install hooks without the safety prompt. This executes arbitrary
        /// code from the source; only use it for a source you trust. Without this
        /// flag, a non-TTY run skips install hooks and prints a note.
        #[arg(long)]
        dangerously_skip_install_hook_check: bool,

        /// Run the item build hook without the safety prompt when `--event build`
        /// is given. This executes arbitrary code; only use it for a source
        /// you trust.
        #[arg(long)]
        dangerously_skip_build_hook_check: bool,
    },

    /// List the hooks for a source or item without running any.
    ///
    /// For a source target, shows all hooks declared in the source's `mind.toml`
    /// with their event, required/optional flag, and command; for source install
    /// hooks, also shows whether the hook is pending and the commit it last ran
    /// at. Also lists any hooks declared by the source's installed items.
    ///
    /// For an item ref `<source>#<item>`, shows only that item's hooks.
    // spec: CLI-196
    List {
        /// The hook target: a source selector or `<source>#<item>` item ref.
        target: String,
    },
}
