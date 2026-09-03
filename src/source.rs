//! Melded sources: the GitHub (or arbitrary git) repos `mind` pulls items from.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};
use crate::paths::Paths;

/// The version-pin recorded on a melded source (STO-18).
///
/// Persisted at meld time and never changed by `sync`. The implicit default
/// (when absent from sources.json) is `DefaultBranch`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Pin {
    /// Track the remote default branch (no explicit pin; the implicit default).
    #[default]
    DefaultBranch,
    /// Track a named branch: reset to that branch tip on sync.
    FollowBranch(String),
    /// Fixed to a tag: re-fetches tags on sync; resets to that tag (moves if
    /// the upstream tag was re-pointed, stays if it was not).
    Tag(String),
    /// Fixed to a specific commit sha: effectively immutable across syncs.
    Ref(String),
}

/// A recorded install hook for an ITEM (HOOK-110), kept in
/// [`crate::manifest::InstalledItem::install_hooks`].
///
/// Tracks the command and the commit at which it last ran. When `ran_at` is
/// `None` the hook was recorded but skipped, so `upgrade` should re-offer it.
/// The source-level record is [`RecordedSourceHook`], which additionally
/// carries the event and the provenance a source hook needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedHook {
    /// The install hook command that was offered.
    pub command: String,
    /// The source commit this hook last RAN at; `None` if it was recorded but
    /// skipped (so `upgrade` can re-offer it). Mirrors the old
    /// `install_hook_commit == None` "recorded but never run" state.
    #[serde(default)]
    pub ran_at: Option<String>,
}

/// The lifecycle event a source-level hook run is recorded under (HOOK-124).
///
/// Only the install and update events are recorded: an uninstall hook fires at
/// `unmeld` (or on demand) and is never pending (HOOK-55), so it has no run
/// state to key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordedEvent {
    Install,
    Update,
}

impl RecordedEvent {
    /// The event's spelling, matching `HookEvent::as_str`.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordedEvent::Install => "install",
            RecordedEvent::Update => "update",
        }
    }

    /// The recorded counterpart of a lifecycle event, or `None` for an event
    /// that carries no run state (uninstall, HOOK-55).
    pub fn of(event: crate::mindfile::HookEvent) -> Option<RecordedEvent> {
        match event {
            crate::mindfile::HookEvent::Install => Some(RecordedEvent::Install),
            crate::mindfile::HookEvent::Update => Some(RecordedEvent::Update),
            crate::mindfile::HookEvent::Uninstall => None,
        }
    }
}

/// Where a recorded source hook's command came from, when it is NOT one the
/// source's own manifest declares.
///
/// A record with no origin is an ordinary declared hook: the clone's
/// `mind.toml` is its source of truth, and `upgrade` only re-offers it while
/// the clone still declares it (HOOK-55). The two variants below have no such
/// backing declaration, so they are carried on the record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookOrigin {
    /// A consumer `meld --install-hook` command (HOOK-56). It replaces the
    /// source's declared install AND update hooks, so it outlives any single
    /// declaration and must be re-applied at every `upgrade`.
    Override,
    /// A curator `[[discover.sources.hooks]]` entry (DSC-61, HOOK-127). It
    /// lives in the PARENT super-source's manifest, not in this source's
    /// clone, so the clone can never be asked what it declares.
    Curated,
}

/// A recorded source-level hook (HOOK-55, HOOK-124, HOOK-127).
///
/// The record is keyed by `(command, event)`: the same command declared for
/// both the install and the update event is two independent records, so
/// running one never marks the other as already run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedSourceHook {
    /// The effective hook command that was offered.
    pub command: String,
    /// The source commit this hook last RAN at; `None` if it was recorded but
    /// skipped (so `upgrade` can re-offer it). Mirrors the old
    /// `install_hook_commit == None` "recorded but never run" state.
    #[serde(default)]
    pub ran_at: Option<String>,
    /// The event this record belongs to (HOOK-124). Absent in a `sources.json`
    /// written before the record carried one, where every entry was an install
    /// hook, so `None` reads as `Install`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<RecordedEvent>,
    /// The hook's declared `name`, kept only for a record with an `origin`: a
    /// curated or overriding hook has no manifest entry in the clone to read a
    /// label back from. `None` for a declared hook (its label comes from the
    /// clone's `mind.toml`, HOOK-51).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The hook's declared `optional` flag, kept for the same reason as `name`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// Where the command came from, when it is not declared by the source's own
    /// manifest (HOOK-56, HOOK-127). `None` = declared by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<HookOrigin>,
    /// True when `ran_at` is a BASELINE rather than a run: the commit the hook
    /// was recorded at without ever running (HOOK-121: a declared update hook
    /// recorded at `meld`, so it is not pending until the source moves).
    /// Cleared the moment the hook actually runs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub baseline: bool,
}

/// `skip_serializing_if` helper: omit a `false` flag from `sources.json`.
fn is_false(b: &bool) -> bool {
    !*b
}

impl RecordedSourceHook {
    /// A declared install-event record, the shape every entry had before the
    /// record carried an event.
    pub fn install(command: impl Into<String>, ran_at: Option<String>) -> RecordedSourceHook {
        RecordedSourceHook {
            command: command.into(),
            ran_at,
            event: Some(RecordedEvent::Install),
            name: None,
            optional: false,
            origin: None,
            baseline: false,
        }
    }

    /// The event this record belongs to; a record with none is an install
    /// record (HOOK-124, back-compat).
    pub fn event(&self) -> RecordedEvent {
        self.event.unwrap_or(RecordedEvent::Install)
    }

    /// Whether this record is the one for `(command, event)`.
    pub fn is(&self, command: &str, event: RecordedEvent) -> bool {
        self.command == command && self.event() == event
    }

    /// Whether the hook is pending at `current` (HOOK-55): never run (or
    /// skipped), or last run at a different commit than the source is on now.
    pub fn pending(&self, current: Option<&str>) -> bool {
        self.ran_at.is_none() || self.ran_at.as_deref() != current
    }

    /// The label a disclosure shows: the recorded `name` when it carries one,
    /// else the command (HOOK-51).
    pub fn label(&self) -> &str {
        match self.name.as_deref() {
            Some(n) if !n.trim().is_empty() => n,
            _ => &self.command,
        }
    }
}

/// How a source's items were discovered, when they came from a Claude plugin
/// manifest rather than convention or `mind.toml` (MKT-10). Recorded at meld
/// time and shown by `recall --sources` / the probe source view so a
/// native-plugin source is distinguishable from a convention or `mind.toml`
/// source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestOrigin {
    /// Items came from a single `.claude-plugin/plugin.json`.
    ClaudePlugin,
    /// Items came from a `.claude-plugin/marketplace.json` catalog.
    ClaudeMarketplace,
}

/// One melded source repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Source identity `host/owner/repo`, e.g. `github.com/james/agents`. Unique
    /// per registry; equals the clone path under `sources/`.
    pub name: String,
    /// Clone URL, e.g. `https://github.com/james/agents`.
    pub url: String,
    /// Host segment used for the on-disk path, e.g. `github.com`.
    pub host: String,
    /// Owner segment, e.g. `james`.
    pub owner: String,
    /// Repo segment, e.g. `agents`.
    pub repo: String,
    /// Commit last seen by `mind sync` (40-char sha), or `None` if never synced.
    #[serde(default)]
    pub commit: Option<String>,
    /// Repo description from its `mind.toml [source]`, if any.
    #[serde(default)]
    pub description: Option<String>,
    /// The effective consumer namespace prefix (the display prefix applied to
    /// item names). Set from `meld --as`, a curated entry's `as`/`namespace`, a
    /// marketplace entry name, an accepted `[source].prefix`, or a collision
    /// prompt. Overrides the repo's own `[source].prefix`. Persisted (never
    /// changed by `sync`). This is NOT the source's identity; see `as_alias`.
    #[serde(default)]
    pub alias: Option<String>,
    /// The identity alias (STO-58): the namespace declared BEFORE the clone -
    /// the consumer `--as`, a curated `[discover].sources` `as`/`namespace`, or a
    /// marketplace entry name. It is the part of the alias that discriminates one
    /// source instance of a repo from another, so it (not the display `alias`)
    /// feeds the `@<alias>` identity suffix (STO-58) and the per-instance clone
    /// path (STO-59). `None` for a bare meld, and for a source whose only prefix
    /// is an accepted `[source].prefix` or a collision-resolved prefix (those are
    /// decided after the clone and are display-only). Absent in older
    /// sources.json deserializes as `None`, so a legacy source keeps its bare
    /// identity and existing clone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_alias: Option<String>,
    /// The version pin (STO-18). Persisted at meld and not changed by sync.
    /// Absent in older sources.json files deserializes as `DefaultBranch`.
    #[serde(default)]
    pub pin: Pin,
    /// Consumer-supplied scan roots from `meld --root` (STO-17). When set,
    /// convention discovery scans under each of these repo-root-relative dirs
    /// instead of the repo root. Persisted at meld and not changed by sync.
    /// None => use `[source].roots` from mind.toml, or the repo root.
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    /// Consumer `--flat-skills` override (STO-44, DSC-75): when true, convention
    /// discovery finds skills as bare-name directories at each scan root (no
    /// `skills/` container). Persisted at meld and not changed by sync. False (or
    /// absent in older sources.json) means fall back to the source's own
    /// `[source].flat-skills` or the `skills/` container (DSC-74).
    #[serde(default)]
    pub flat_skills: bool,
    /// Consumer-supplied additive scan roots from `meld --add-root` (STO-55,
    /// DSC-84): convention-scanned in addition to whatever discovery layer is
    /// authoritative for the source (a plugin manifest, an authoritative
    /// mind.toml, or the ordinary convention scan). Persisted at meld and not
    /// changed by sync. None means no additional roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_roots: Option<Vec<String>>,
    /// The item path of an item-link source instance (LNK-4): the linked item's
    /// repo-root-relative path parsed from a deep tree/blob URL (a skill
    /// directory, or the `.md` file itself for a file link, LNK-20).
    /// When set, the source's identity (`name`) carries a `#<path>` suffix and
    /// its catalog is exactly that one item (LNK-7). None for an ordinary
    /// source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_path: Option<String>,
    /// The consumer's explicit kind for a file link (STO-81, LNK-21/LNK-22):
    /// `meld/learn --kind <kind>` or a `[discover].sources` entry's `kind =`.
    /// Persisted at meld and not changed by `sync`. None means the kind is
    /// resolved from the clone on every scan (containing directory, else
    /// frontmatter), which is also how a source registered before this field
    /// reads back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_kind: Option<crate::error::ItemKind>,
    /// The manifest origin of this source's items (MKT-10), when they came from
    /// a Claude plugin manifest (`.claude-plugin/plugin.json` or
    /// `marketplace.json`). `None` for a convention- or `mind.toml`-discovered
    /// source. Persisted at meld; shown by `recall --sources` / probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ManifestOrigin>,
    /// The plugin `version` declared in a `.claude-plugin` manifest (MKT-6),
    /// recorded for display only. Informational: drift/upgrade still compare
    /// source content hash and commit, never this value. `None` when the source
    /// did not come from a plugin manifest or declared no version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// The hooks recorded for this source (HOOK-55, HOOK-124). Supersedes the
    /// legacy single `install_hook`/`install_hook_commit` pair, which is
    /// migrated into this on load. Each entry records the command, the event it
    /// belongs to, and the commit it last ran at. Install and update hooks share
    /// the vec but not a key: the record is keyed by `(command, event)`.
    #[serde(default)]
    pub install_hooks: Vec<RecordedSourceHook>,
    /// Legacy: the install hook command in effect for this source (HOOK-31).
    /// Load-only; migrated into `install_hooks` by `migrate_legacy_hook` and
    /// not re-emitted once migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_hook: Option<String>,
    /// Legacy: the commit the install hook last ran at (HOOK-31). Load-only;
    /// migrated into `install_hooks` by `migrate_legacy_hook` and not
    /// re-emitted once migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_hook_commit: Option<String>,
}

impl Source {
    /// Whether this is a local-path source (`host == "local"`).
    pub fn is_local(&self) -> bool {
        self.host == "local"
    }

    /// The base repo identity `host/owner/repo`, without an item-link `#path`
    /// or consumer-alias `@alias` suffix. Equal to `name` for an ordinary,
    /// unaliased source. This is what managed policy allowlists match against
    /// (LNK-11) and what the compare/browse URLs derive from (CLI-176).
    pub fn base_identity(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
    }

    /// The canonical identity `name` derived from this source's parts (STO-13,
    /// STO-58, LNK-4): `host/owner/repo`, plus a `#<item_path>` suffix for an
    /// item-link instance, plus a trailing `@<alias>` segment for a consumer
    /// prefix. `@<alias>` is always last, so it composes with `#<path>` as
    /// `host/owner/repo#<path>@<alias>`.
    pub fn compute_name(&self) -> String {
        let mut n = self.base_identity();
        if let Some(p) = &self.item_path {
            n.push('#');
            n.push_str(p);
        }
        if let Some(a) = &self.as_alias
            && !a.is_empty()
        {
            n.push('@');
            n.push_str(a);
        }
        n
    }

    /// Recompute `name` from the source's parts (`host/owner/repo`, item_path,
    /// identity alias). Idempotent.
    pub fn refresh_name(&mut self) {
        self.name = self.compute_name();
    }

    /// Set the identity alias (STO-58) - the pre-clone `--as`/curated-`as`/
    /// marketplace-entry namespace - and update the identity `name`. The same
    /// value seeds the effective display prefix (`alias`); a later post-clone
    /// override (an accepted `[source].prefix` or a collision prompt) may change
    /// `alias` without touching identity. `Some("")` is the explicit no-prefix
    /// override: it adds no `@` suffix, so the identity is the bare
    /// `host/owner/repo` (equal to an unaliased meld).
    pub fn apply_alias(&mut self, alias: Option<String>) {
        self.as_alias = alias.clone();
        self.alias = alias;
        self.refresh_name();
    }

    /// Whether this source is read live from its working tree rather than a clone
    /// (CLI-27): a local source with no pin in effect. A pinned local source is
    /// cloned (a snapshot at the pin), so pinning still works. `mind` never deletes
    /// a linked source's directory (it is the user's working tree).
    pub fn is_linked(&self) -> bool {
        self.is_local() && self.pin == Pin::DefaultBranch
    }

    /// Where `mind` reads this source's content. A linked source is its working
    /// tree (`url` is the path); any other source (remote, or a pinned local) lives
    /// in the cloned sources tree.
    pub fn clone_dir(&self, paths: &Paths) -> PathBuf {
        if self.is_linked() {
            return PathBuf::from(&self.url);
        }
        // spec: STO-59 STO-70 -- see `clone_dir_leaf` for the full leaf
        // formula (repo, item_path, and identity alias each contribute).
        let leaf = clone_dir_leaf(
            &self.repo,
            self.item_path.as_deref(),
            self.as_alias.as_deref(),
        );
        paths
            .sources_dir()
            .join(&self.host)
            .join(&self.owner)
            .join(leaf)
    }

    /// A browser URL to compare two commits, for `mind upgrade` output.
    ///
    /// Emitted for https remotes that use the GitHub `/compare/<old>...<new>`
    /// URL shape (spec: CLI-176, CLI-188). This covers GitHub.com, GitHub
    /// Enterprise Server, and Gitea/Forgejo instances. SSH remotes and local
    /// paths return `None` because there is no web host to link to.
    ///
    /// Hosts whose hostname contains "gitlab" or "bitbucket" (case-insensitive)
    /// use a different compare URL shape and therefore also return `None`; those
    /// hosts previously received a GitHub-shaped link that would 404 (CLI-188).
    pub fn compare_url(&self, from: &str, to: &str) -> Option<String> {
        if !self.url.starts_with("https://") {
            return None;
        }
        // spec: CLI-188 - suppress for known non-GitHub URL shapes
        let host_lower = self.host.to_ascii_lowercase();
        if host_lower.contains("gitlab") || host_lower.contains("bitbucket") {
            return None;
        }
        Some(format!(
            "https://{}/{}/{}/compare/{from}...{to}",
            self.host, self.owner, self.repo
        ))
    }

    /// A browser URL to view the tree at a specific commit, for the hook
    /// consent disclosure (HOOK-24).
    ///
    /// Uses the same host guard as `compare_url` (spec: CLI-176, CLI-188):
    /// https remotes on GitHub-shaped hosts (not gitlab/bitbucket) return
    /// `https://<host>/<owner>/<repo>/tree/<commit>`. SSH remotes and local
    /// paths return `None` because there is no web host to link to.
    ///
    /// Hosts whose name contains "gitlab" or "bitbucket" (case-insensitive)
    /// also return `None` (CLI-188); those use a different URL shape.
    pub fn browse_url(&self, commit: &str) -> Option<String> {
        // spec: HOOK-24 - same host guard as compare_url (CLI-176, CLI-188)
        if !self.url.starts_with("https://") {
            return None;
        }
        let host_lower = self.host.to_ascii_lowercase();
        if host_lower.contains("gitlab") || host_lower.contains("bitbucket") {
            return None;
        }
        Some(format!(
            "https://{}/{}/{}/tree/{commit}",
            self.host, self.owner, self.repo
        ))
    }

    /// The SSH clone URL (`git@host:owner/repo`) for this source's identity.
    pub fn ssh_url(&self) -> String {
        format!("git@{}:{}/{}", self.host, self.owner, self.repo)
    }

    /// Switch the clone URL to SSH when `prefer_ssh` is set and this is an http
    /// or https remote (not a local path). The new URL is persisted on the source,
    /// so later `sync`s reuse SSH too. A no-op for local paths and for URLs that
    /// are already SSH (an explicit `git@...` or `ssh://`).
    pub fn prefer_ssh(&mut self, prefer_ssh: bool) {
        // spec: DSC-66 (hardening) - rewrite both http:// and https:// remotes
        // to the SSH form; a plain http:// remote is just as likely to carry a
        // credential in the URL and deserves the same treatment.
        if prefer_ssh
            && self.host != "local"
            && (self.url.starts_with("https://") || self.url.starts_with("http://"))
        {
            self.url = self.ssh_url();
        }
    }

    /// Fold a legacy `install_hook`/`install_hook_commit` pair (from an older
    /// sources.json) into `install_hooks`, then clear the legacy fields so they
    /// are not re-emitted. A no-op when there is no legacy hook or it is already
    /// represented. Idempotent.
    pub fn migrate_legacy_hook(&mut self) {
        if self.install_hooks.is_empty()
            && let Some(cmd) = self.install_hook.take()
            && !cmd.trim().is_empty()
        {
            let ran_at = self.install_hook_commit.take();
            // A legacy pair is always an install hook (the update event did not
            // exist when it was written), so it migrates as one (HOOK-124).
            self.install_hooks
                .push(RecordedSourceHook::install(cmd, ran_at));
        }
        // Clear legacy fields regardless so they stop being emitted.
        self.install_hook = None;
        self.install_hook_commit = None;
    }

    /// The recorded INSTALL hooks `upgrade` should re-offer (HOOK-55): those
    /// whose last-run commit is absent or differs from `current` AND that the
    /// source still stands behind.
    ///
    /// `current` is the source's current commit. `declared` is the set of
    /// install-hook commands the clone CURRENTLY declares; a record outside it
    /// is not re-offered, so a command the source has stopped declaring (or one
    /// left behind by a manifest that no longer parses) is never replayed from
    /// local state alone under an `Event: install` disclosure. Two kinds of
    /// record have no declaration in the clone to match against and are kept
    /// regardless: a consumer `--install-hook` override (HOOK-56) and, while
    /// `keep_curated` holds, a curated `[[discover.sources.hooks]]` entry
    /// (DSC-61, HOOK-127). `keep_curated` is the DSC-60 gate: it is false once
    /// the source ships a `mind.toml` of its own, since curated values stop
    /// applying then.
    // spec: HOOK-55 HOOK-127
    pub fn pending_install_hooks(
        &self,
        current: Option<&str>,
        declared: &[String],
        keep_curated: bool,
    ) -> Vec<&RecordedSourceHook> {
        self.install_hooks
            .iter()
            .filter(|h| h.event() == RecordedEvent::Install)
            .filter(|h| match h.origin {
                Some(HookOrigin::Override) => true,
                Some(HookOrigin::Curated) => keep_curated,
                None => declared.iter().any(|d| d == &h.command),
            })
            .filter(|h| h.pending(current))
            .collect()
    }

    /// The consumer `meld --install-hook` command recorded on this source
    /// (HOOK-56), if any. It overrides the source's declared install AND update
    /// hooks at every later `upgrade`, so it is read back from the record
    /// rather than from the clone.
    // spec: HOOK-56
    pub fn override_command(&self) -> Option<&str> {
        self.install_hooks
            .iter()
            .find(|h| h.origin == Some(HookOrigin::Override))
            .map(|h| h.command.as_str())
    }

    /// Re-run the same validation `parse_spec` applies at construction time
    /// against this (already persisted) source's `host`/`owner`/`repo`,
    /// `as_alias`, and pin ref value (STO-68). `sources.json` can carry an
    /// entry written by an older binary that predates a tightened rule (e.g.
    /// 0.21.0's `parse_spec` accepted `repo: ".."`), so `Registry::load` calls
    /// this on every entry rather than trusting the file blindly. On failure,
    /// returns the offending part's label alongside the underlying error's
    /// message (not the error itself: `MindError` is large, and the caller
    /// only ever formats this into a warning string), so the caller can drop
    /// the entry and warn naming exactly which part was unsafe.
    fn revalidate(&self) -> std::result::Result<(), (&'static str, String)> {
        // spec: STO-68 -- same per-part rules `make_source` applies (CLI-204).
        validate_identity_part(&self.name, "host", &self.host, true, true)
            .map_err(|e| ("host", e.to_string()))?;
        validate_identity_part(&self.name, "owner", &self.owner, false, true)
            .map_err(|e| ("owner", e.to_string()))?;
        validate_identity_part(&self.name, "repo", &self.repo, true, true)
            .map_err(|e| ("repo", e.to_string()))?;
        if let Some(alias) = &self.as_alias {
            crate::namespace::validate_prefix(alias).map_err(|e| ("as_alias", e.to_string()))?;
        }
        let pin_value = match &self.pin {
            Pin::DefaultBranch => None,
            Pin::FollowBranch(v) | Pin::Tag(v) | Pin::Ref(v) => Some(v.as_str()),
        };
        if let Some(v) = pin_value {
            crate::git::validate_ref_value(v).map_err(|e| ("pin", e.to_string()))?;
        }
        Ok(())
    }
}

/// The clone-dir leaf under `sources/<host>/<owner>/` (STO-11, STO-59, STO-70).
///
/// Mirrors the full instance identity, `repo#<item_path>@<alias>`, so that
/// every distinct identity gets its own leaf and therefore its own
/// independent checkout:
/// - `repo`                          -- no item_path, no alias (STO-11).
/// - `repo@<alias>`                  -- alias only (STO-59).
/// - `repo#<enc>`                    -- item_path only (STO-70).
/// - `repo#<enc>@<alias>`            -- both (STO-70).
///
/// `<enc>` is `item_path` with `%` percent-encoded to `%25` and then `/` to
/// `%2F` (`encode_item_path_segment`): injective, human-readable, and
/// filesystem-safe (an item path can never contain `@`, `#`, `..`, or NUL,
/// LNK-10/LNK-16, so those need no escaping). If the encoded leaf would
/// exceed 120 bytes, the encoded segment is replaced with the 16-hex FNV of
/// `item_path` (`crate::hash::hash_str`) instead, keeping the leaf short
/// while still being deterministic per `item_path`.
fn clone_dir_leaf(repo: &str, item_path: Option<&str>, alias: Option<&str>) -> String {
    let alias_suffix = match alias {
        Some(a) if !a.is_empty() => format!("@{a}"),
        _ => String::new(),
    };
    let Some(item_path) = item_path else {
        return format!("{repo}{alias_suffix}");
    };
    let encoded = encode_item_path_segment(item_path);
    let leaf = format!("{repo}#{encoded}{alias_suffix}");
    if leaf.len() > 120 {
        let hashed = crate::hash::hash_str(item_path);
        format!("{repo}#{hashed}{alias_suffix}")
    } else {
        leaf
    }
}

/// Percent-encode `%` (as `%25`) then `/` (as `%2F`) in an item path, for
/// embedding it as a single filesystem-safe path component (STO-70). `%`
/// must be encoded first so a literal `%2F` already present in the input
/// (impossible today since `/` itself is not escaped elsewhere, but kept for
/// robustness) does not get double-decoded.
fn encode_item_path_segment(item_path: &str) -> String {
    item_path.replace('%', "%25").replace('/', "%2F")
}

/// Revalidate every source (STO-68), dropping (never hard-erroring) any entry
/// that fails and reporting each drop through `warn` once, with a message
/// naming `sources.json`, the entry's name, and the offending part. Pure and
/// independently testable; [`Registry::load`] wires `warn` to a stderr
/// `eprintln!`. Dropping rather than failing the whole load matters: an entry
/// that was valid under an older, looser `parse_spec` must not brick every
/// `mind` verb for a user who happens to be carrying it -- the drop is
/// persisted the next time `Registry::save` runs.
fn revalidate_sources(sources: Vec<Source>, mut warn: impl FnMut(&str)) -> Vec<Source> {
    sources
        .into_iter()
        .filter_map(|src| match src.revalidate() {
            Ok(()) => Some(src),
            Err((part, e)) => {
                warn(&format!(
                    "warning: sources.json: dropping source '{}': its {part} part failed \
                     validation ({e})",
                    src.name
                ));
                None
            }
        })
        .collect()
}

/// Parse a user-supplied repo spec into a [`Source`] (without touching disk).
///
/// Accepts:
/// - `owner/repo`                       -> github.com
/// - `github:owner/repo`                -> github.com
/// - `https://github.com/owner/repo`    -> as given
/// - `git@github.com:owner/repo.git`    -> ssh form
///
/// Prints the CLI-215 shadowing-directory note when the bare `owner/repo` form
/// also names a directory here. A caller that has ALREADY decided which reading
/// it is taking (and says so itself) must use [`parse_spec_quiet`] instead, so
/// the user is not told to use `./x` immediately before being told that `./x`
/// is exactly what was used (CLI-216).
pub fn parse_spec(spec: &str) -> Result<Source> {
    parse_spec_inner(spec, true)
}

/// [`parse_spec`] without the CLI-215 note: the same parse, no output.
///
/// For callers that parse a spec to ANSWER a question rather than to act on it
/// (`review`'s CLI-214 shadow note, which reports the reading it already took;
/// the DSC-93 nested-entry check, which only classifies an entry). The note is
/// about which reading a *clone* is about to take, so emitting it from these
/// sites is at best noise and at worst a contradiction (CLI-216).
/// spec: CLI-216
pub(crate) fn parse_spec_quiet(spec: &str) -> Result<Source> {
    parse_spec_inner(spec, false)
}

/// The shared parser. `note` enables the CLI-215 advisory on the bare
/// `owner/repo` branch; nothing else differs between the two entry points.
fn parse_spec_inner(spec: &str, note: bool) -> Result<Source> {
    let spec = spec.trim();
    let invalid = || MindError::InvalidRepoSpec {
        spec: spec.to_string(),
    };

    // Local path or file:// URL — meld a repo straight off disk (handy for
    // developing a source locally, and what the test harness uses). The owner is
    // the path's parent directory, so local repos sharing a basename stay
    // distinct (e.g. `/a/agents` -> `a/agents`, `/b/agents` -> `b/agents`).
    let local = spec.strip_prefix("file://");
    if local.is_some() || spec.starts_with('/') || spec.starts_with("./") || spec.starts_with("../")
    {
        let path = local.unwrap_or(spec);
        // Item link on a local repo (LNK-1): only the explicit `file://` form
        // is checked for a tree/blob marker, so a bare path that happens to
        // contain such a directory name stays a plain repo spec.
        if local.is_some()
            && let Some((repo_part, marker, rest)) = split_link_marker(path)
        {
            let (pin, item_path) = parse_link_tail(spec, marker, rest)?;
            // spec: STO-72 -- absolutize BEFORE identity derivation, so a
            // relative local repo path resolves against the real cwd instead of
            // `.`/`..` landing in the owner segment (which produced the wrong
            // `local/local/<repo>` identity for a spec like `./foo`) and so the
            // recorded `url` still resolves after a later `cd`.
            let abs_repo_part = absolutize(repo_part);
            let mut comps = abs_repo_part.trim_end_matches('/').rsplit('/');
            let repo_raw = comps.next().filter(|s| !s.is_empty()).ok_or_else(invalid)?;
            let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
            let owner = comps
                .next()
                .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                .unwrap_or("local");
            let mut source = make_source(spec, "local", owner, repo, abs_repo_part.clone())?;
            // spec: LNK-4 -- extended identity; the pin makes this a cloned
            // snapshot (never a linked working tree), so lifecycle matches a
            // remote link instance.
            source.name = format!("{}#{item_path}", source.name);
            source.item_path = Some(item_path);
            source.pin = pin;
            return Ok(source);
        }
        // spec: STO-72 -- same absolutization for the bare local-path branch.
        let abs_path = absolutize(path);
        let mut comps = abs_path.trim_end_matches('/').rsplit('/');
        let repo_raw = comps.next().filter(|s| !s.is_empty()).ok_or_else(invalid)?;
        let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
        let owner = comps
            .next()
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .unwrap_or("local");
        return make_source(spec, "local", owner, repo, abs_path.clone());
    }

    // SSH form: git@host:owner/repo(.git)
    if let Some(rest) = spec.strip_prefix("git@") {
        let (host, path) = rest.split_once(':').ok_or_else(invalid)?;
        let (owner, repo) = split_owner_repo(path).ok_or_else(invalid)?;
        return make_source(spec, host, &owner, &repo, spec.to_string());
    }

    // URL form: scheme://authority/owner/repo(.git)
    if let Some((scheme, rest)) = spec.split_once("://") {
        let (authority, path) = rest.split_once('/').ok_or_else(invalid)?;
        // spec: STO-67 -- an `ssh://` authority may carry a userinfo prefix
        // (`user@host`, e.g. `ssh://git@github.com/...`). Split on the LAST
        // `@` so the identity `host` is separated from the userinfo, while
        // `url` keeps the FULL authority (userinfo included) so `git clone`
        // still authenticates. Any other scheme refuses an embedded
        // credential outright: it would otherwise be persisted verbatim into
        // sources.json and echoed back by `dump`.
        let (userinfo, host) = match authority.rsplit_once('@') {
            Some((info, h)) => (Some(info), h),
            None => (None, authority),
        };
        if let Some(info) = userinfo {
            if scheme != "ssh" {
                return Err(MindError::UnsafeRepoSpec {
                    spec: spec.to_string(),
                    part: "host",
                    value: authority.to_string(),
                    reason: "embeds a credential (userinfo before '@'); a credential must not \
                             be persisted into sources.json or echoed back by dump",
                });
            }
            if info.is_empty() || info.contains(|c: char| c.is_control()) {
                return Err(MindError::UnsafeRepoSpec {
                    spec: spec.to_string(),
                    part: "host",
                    value: authority.to_string(),
                    reason: "the ssh userinfo is empty or contains a control character",
                });
            }
        }
        // Item link (LNK-1): a deep URL with a tree/blob segment naming one
        // skill inside the repo. Checked before the plain owner/repo shape,
        // which rejects any extra path segments.
        if let Some(source) = parse_item_link(spec, scheme, host, authority, path)? {
            return Ok(source);
        }
        let (owner, repo) = split_owner_repo(path).ok_or_else(invalid)?;
        let url = format!("{scheme}://{authority}/{owner}/{repo}");
        return make_source(spec, host, &owner, &repo, url);
    }

    // github: prefix shorthand
    let bare = spec.strip_prefix("github:").unwrap_or(spec);
    let (owner, repo) = split_owner_repo(bare).ok_or_else(invalid)?;
    // spec: CLI-215 -- warn BEFORE any clone when this owner/repo-shaped spec
    // also names an existing directory relative to the cwd. A two-segment
    // relative path like `skills/greet` is easy to type meaning "the directory
    // right here"; without this note the first sign of the misreading is a
    // surprise network clone. This does not change which reading `parse_spec`
    // takes here (still the remote spec, always) -- it only names the
    // ambiguity and the `./` form that means the directory instead.
    if note
        && let Ok(cwd) = std::env::current_dir()
        && local_dir_shadow(&cwd, bare)
    {
        eprintln!(
            "note: '{bare}' is also a directory here; to meld/review it as a local path instead \
             of a remote repo, use './{bare}'"
        );
    }
    let url = format!("https://github.com/{owner}/{repo}");
    make_source(spec, "github.com", &owner, &repo, url)
}

/// Resolve a local-path repo spec to an absolute path before it is persisted
/// (STO-72): join the current working directory when `path` is relative, then
/// resolve `.`/`..` components LEXICALLY -- no `canonicalize`, no disk access
/// beyond reading the cwd, so `parse_spec` stays stat-free and still works for
/// a path that does not exist yet. Without this, a relative spec like `./foo`
/// derived its owner from the literal string (`.` filtered out, falling back
/// to the `local` placeholder), producing the wrong `local/local/foo`
/// identity; and after a later `cd`, the same relative string recorded in
/// `Source::url` no longer resolves to the melded directory at all (U40).
fn absolutize(path: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    absolutize_against(&cwd, path)
}

/// Whether `spec` is the RELATIVE local-path form of a repo spec (`./rel/path`
/// or `../rel/path`), as opposed to an already-absolute local path or a remote
/// spec. Used by `mindfile::MindToml::load` (DSC-92) to find a curated
/// `[discover].sources` entry's `source` that needs rewriting against the
/// mind.toml's own directory rather than the consumer's cwd.
pub(crate) fn is_relative_local_spec(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../")
}

/// Pure half of [`absolutize`]: join `path` onto `base` unless it is already
/// absolute, then resolve `.`/`..` components lexically. Independently
/// testable against an explicit directory, with no cwd dependency. `pub(crate)`
/// so `mindfile::MindToml::load` can resolve a nested `[discover].sources`
/// entry's relative local path against the mind.toml's own directory (DSC-92),
/// not the consumer's cwd.
pub(crate) fn absolutize_against(base: &Path, path: &str) -> String {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    lexically_normalize(&joined)
}

/// Resolve `.`/`..` path components without touching the filesystem (no
/// `canonicalize`, no symlink resolution, no existence check): a `..` with
/// nothing to cancel (past the root, or at the very start of a relative path)
/// is kept as-is since there is nothing lexically available to pop.
fn lexically_normalize(path: &Path) -> String {
    use std::path::Component;
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(comp),
            },
            other => out.push(other),
        }
    }
    let mut result = PathBuf::new();
    for c in &out {
        result.push(c);
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result.to_string_lossy().into_owned()
}

/// Whether `spec` resolves to an existing directory when joined onto `base`
/// (unless `spec` is already absolute). The shared local-directory detection
/// behind `review`'s CLI-214 local-path preference and `parse_spec`'s CLI-215
/// shadow note. Pure: no cwd read, no network, so callers' logic is
/// independently testable against an explicit temp directory.
pub(crate) fn resolve_local_dir(base: &Path, spec: &str) -> Option<PathBuf> {
    let p = Path::new(spec);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    joined.is_dir().then_some(joined)
}

/// Whether `spec` shadows an existing directory relative to `base` (CLI-215).
pub(crate) fn local_dir_shadow(base: &Path, spec: &str) -> bool {
    resolve_local_dir(base, spec).is_some()
}

/// Migrate a local source's relative `url` to an absolute form (STO-73), once
/// the process cwd is known: a source melded before STO-72 (or a hand-edited
/// registry) may still carry a literal relative path. Only rewritten when the
/// absolutized path resolves to an EXISTING directory right now: if it does
/// not, the path is left exactly as recorded, so the "linked source is gone"
/// finding (CLI-212/CLI-213) names what the user actually typed rather than a
/// confusing absolutized guess. Only `url` is rewritten, never `name`:
/// identity is the manifest's back-reference key, and renaming it would orphan
/// every installed item. Returns whether a rewrite happened.
fn migrate_relative_local_url(base: &Path, src: &mut Source) -> bool {
    if !src.is_local() || Path::new(&src.url).is_absolute() {
        return false;
    }
    let abs = absolutize_against(base, &src.url);
    if Path::new(&abs).is_dir() {
        src.url = abs;
        true
    } else {
        false
    }
}

/// Split a spec's path at the first tree/blob marker (LNK-1). Returns
/// `(before, "tree"|"blob", after)`; the GitLab `/-/tree/` and `/-/blob/`
/// forms match at a smaller index than their embedded short forms, so the
/// earliest match is always the right one.
fn split_link_marker(s: &str) -> Option<(&str, &'static str, &str)> {
    const MARKERS: [(&str, &str); 4] = [
        ("/-/tree/", "tree"),
        ("/-/blob/", "blob"),
        ("/tree/", "tree"),
        ("/blob/", "blob"),
    ];
    MARKERS
        .iter()
        .filter_map(|(pat, kind)| s.find(pat).map(|idx| (idx, pat.len(), *kind)))
        .min_by_key(|&(idx, _, _)| idx)
        .map(|(idx, len, kind)| (&s[..idx], kind, &s[idx + len..]))
}

/// Parse the `<ref>/<path>` tail after a tree/blob marker (LNK-1, LNK-3,
/// LNK-10): the pin from the single ref segment and the validated skill
/// directory path.
///
/// A marker (`tree`/`blob`) was already found in `spec` by the caller, so any
/// failure here is an attempted item link that did not parse, not a plain
/// repo spec: every error is [`MindError::BadItemLink`] (LNK-14), never
/// [`MindError::InvalidRepoSpec`].
fn parse_link_tail(spec: &str, marker: &str, rest: &str) -> Result<(Pin, String)> {
    let bad = |reason: &str| MindError::BadItemLink {
        url: spec.to_string(),
        reason: reason.to_string(),
    };
    // spec: LNK-3 -- the ref is the single segment after tree/blob.
    let mut segs = rest.trim_matches('/').split('/');
    let r = segs
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("missing a ref (branch, tag, or commit) after tree/blob"))?;
    // spec: LNK-1 LNK-20 -- a path ending in /SKILL.md names the skill directory
    // that is its parent; any other `.md` path names that single file (an
    // agent, rule, or command); any other path names a skill directory. A blob
    // link must end in `.md`: a forge blob URL names a file.
    // spec: LNK-17 -- drop `.` segments before the path becomes identity, so a
    // `./`-variant URL dedups to the plain form's instance instead of
    // registering a second one for the same on-disk item.
    let mut parts: Vec<&str> = segs.filter(|p| *p != ".").collect();
    let last_is_md = parts.last().is_some_and(|p| p.ends_with(".md"));
    if parts.last() == Some(&"SKILL.md") {
        parts.pop();
    } else if marker == "blob" && !last_is_md {
        return Err(bad(
            "a blob link must name a `.md` file (an item file, or a skill's SKILL.md)",
        ));
    }
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(bad("missing the item path after the ref"));
    }
    let item_path = parts.join("/");
    // spec: LNK-10 -- safe relative path and a valid git ref value, rejected
    // before any clone.
    if !crate::plugin_manifest::is_safe_manifest_path(&item_path) {
        return Err(bad("the item path is not a safe repo-relative path"));
    }
    // spec: LNK-16 -- the same collision STO-64 closes for `repo`, one segment
    // over. A link instance's identity is `host/owner/repo#<item_path>`, and an
    // identity alias appends `@<alias>` to the whole thing, so an unaliased link
    // to `skills/foo@bar` and an `@bar`-aliased link to `skills/foo` would
    // compute the same identity. `#` would likewise produce a second marker.
    if item_path.contains(['@', '#']) {
        return Err(bad(
            "the item path may not contain '@' or '#'; they are identity markers",
        ));
    }
    if crate::git::validate_ref_value(r).is_err() {
        return Err(bad("the ref is not a valid git ref"));
    }
    // spec: LNK-3 -- a 40-hex ref pins the commit; anything else follows that
    // branch. Lifted into the standard pin resolution by meld.
    let pin = if r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()) {
        Pin::Ref(r.to_string())
    } else {
        Pin::FollowBranch(r.to_string())
    };
    Ok((pin, item_path))
}

/// Parse the path part of a forge URL as an item link (LNK-1..4):
/// `owner/repo/tree/<ref>/<path>`, `owner/repo/blob/<ref>/<path>/SKILL.md`,
/// and the GitLab `owner/repo/-/tree|blob/...` variants. Returns `Ok(None)`
/// when the path carries no tree/blob marker (a plain repo URL); a marker that
/// does not complete to a valid link is `BadItemLink` (LNK-2, LNK-14). The
/// owner/repo split before the marker is a malformed repo spec regardless of
/// the marker, so it stays `InvalidRepoSpec`.
///
/// `host` is the identity host (userinfo already stripped, STO-67); `authority`
/// is the full authority (userinfo included, e.g. `git@host`) used to build
/// `url` so an ssh link clone still authenticates.
fn parse_item_link(
    spec: &str,
    scheme: &str,
    host: &str,
    authority: &str,
    path: &str,
) -> Result<Option<Source>> {
    // spec: LNK-1 -- strip a query string / fragment pasted from a browser.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let Some((repo_part, marker, rest)) = split_link_marker(path) else {
        return Ok(None);
    };
    let invalid = || MindError::InvalidRepoSpec {
        spec: spec.to_string(),
    };
    let (owner, repo) = split_owner_repo(repo_part).ok_or_else(invalid)?;
    let (pin, item_path) = parse_link_tail(spec, marker, rest)?;
    let url = format!("{scheme}://{authority}/{owner}/{repo}");
    let mut source = make_source(spec, host, &owner, &repo, url)?;
    // spec: LNK-4 -- the extended identity keeps instances from the same repo
    // (and a plain meld of it) distinct; the clone path follows the name.
    source.name = format!("{}#{item_path}", source.name);
    source.item_path = Some(item_path);
    source.pin = pin;
    Ok(Some(source))
}

fn split_owner_repo(path: &str) -> Option<(String, String)> {
    let path = path.trim_matches('/');
    let (owner, repo) = path.split_once('/')?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// Reject a `host`, `owner`, or `repo` part that is not a single safe path
/// component (spec: CLI-204, STO-64).
///
/// These three parts are load-bearing twice over: they are joined with `/` to
/// form the source identity (STO-13), which managed policy matches segment by
/// segment (POL-10, POL-67), and they are joined as directory components to
/// form the clone path (STO-11), which is deleted and re-cloned by `meld`. A
/// part containing `/` would silently add identity segments and path
/// components; `.`/`..` would escape the sources tree; a control character
/// would corrupt any output that echoes the identity. None of these can occur
/// in a real forge identity, and the ssh branch of [`parse_spec`] splits only
/// on the first `:`, so without this check `git@evil/host@x:o/r` parses with a
/// `host` of `evil/host@x`.
///
/// `@`/`#` legality is per-part, not a blanket rule (STO-64):
/// - `host`: both rejected. Neither can legitimately occur in a real forge
///   host, and `@` is what makes the ssh form ambiguous.
/// - `owner`: `#` rejected, `@` allowed. A local path may legitimately carry
///   `@` (`/src/proj@v2/agents`), and it collides with nothing: the `@<alias>`
///   identity suffix (STO-58) only ever appends to `repo`. `#` would land
///   before the repo segment and confuse `#`-splitting in item refs
///   (`resolve.rs::parse_item_ref`) and hook targets (`CLI-194`).
/// - `repo`: both rejected. `@<alias>` (STO-58/STO-59) and the item-link
///   `#<path>` marker both append directly to `repo` in both the identity and
///   the clone-dir leaf, so a `repo` containing either character can collide
///   with a distinct, unrelated source's identity and clone path (STO-64).
fn validate_identity_part(
    spec: &str,
    part: &'static str,
    value: &str,
    reject_at: bool,
    reject_hash: bool,
) -> Result<()> {
    let bad = |reason: &'static str| MindError::UnsafeRepoSpec {
        spec: spec.to_string(),
        part,
        value: value.to_string(),
        reason,
    };
    if value.is_empty() {
        return Err(bad("is empty"));
    }
    if value == "." || value == ".." {
        return Err(bad("is a relative path component"));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(bad("contains a path separator"));
    }
    if value.contains(|c: char| c.is_control()) {
        return Err(bad("contains a control character"));
    }
    if reject_at && reject_hash && value.contains(['@', '#']) {
        return Err(bad("contains '@' or '#'"));
    }
    if reject_at && value.contains('@') {
        return Err(bad("contains '@'"));
    }
    if reject_hash && value.contains('#') {
        return Err(bad("contains '#'"));
    }
    Ok(())
}

fn make_source(spec: &str, host: &str, owner: &str, repo: &str, url: String) -> Result<Source> {
    // spec: CLI-204 STO-64 -- validate before constructing, so no Source with
    // an unsafe identity part is ever built, persisted, or used as a clone
    // path.
    validate_identity_part(spec, "host", host, true, true)?;
    validate_identity_part(spec, "owner", owner, false, true)?;
    validate_identity_part(spec, "repo", repo, true, true)?;
    Ok(Source {
        // Identity is `host/owner/repo` (matching the clone path), so repos that
        // share a basename or even an owner/repo across hosts stay distinct.
        name: format!("{host}/{owner}/{repo}"),
        url,
        host: host.to_string(),
        owner: owner.to_string(),
        repo: repo.to_string(),
        commit: None,
        description: None,
        alias: None,
        as_alias: None,
        pin: Pin::default(),
        roots: None,
        flat_skills: false,
        add_roots: None,
        item_path: None,
        item_kind: None,
        origin: None,
        plugin_version: None,
        install_hooks: Vec::new(),
        install_hook: None,
        install_hook_commit: None,
    })
}

/// The persisted registry of melded sources.
///
/// Schema version is checked during `load` via a private wrapper type (STO-50):
/// the public struct is unchanged so `commands.rs` struct literals continue to
/// compile. `save` always writes `"version": 1`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub sources: Vec<Source>,
}

/// Private serde wrapper for schema-version detection (STO-50).
#[derive(Deserialize)]
struct RegistryWithVersion {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    sources: Vec<Source>,
}

/// The maximum schema version this binary can read.
const REGISTRY_VERSION: u32 = 1;

fn default_version() -> u32 {
    1
}

impl Registry {
    /// Load the registry, returning an empty one if the file does not exist.
    ///
    /// Checks the schema version (STO-50), migrates any legacy
    /// `install_hook`/`install_hook_commit` pairs into `install_hooks`
    /// transparently on load (HOOK-55), and revalidates every entry's
    /// identity/prefix/pin fields, dropping (and warning about) any that fail
    /// under the current, possibly-tightened rules (STO-68).
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.sources_file();
        match std::fs::read(&file) {
            Ok(bytes) => {
                let raw: RegistryWithVersion = serde_json::from_slice(&bytes)
                    .map_err(|e| MindError::json("sources.json", e))?;
                // spec: STO-50 STO-51
                if raw.version > REGISTRY_VERSION {
                    return Err(MindError::StateTooNew {
                        what: "sources.json",
                        found: raw.version,
                        supported: REGISTRY_VERSION,
                    });
                }
                let mut reg = Registry {
                    sources: raw.sources,
                };
                for src in &mut reg.sources {
                    src.migrate_legacy_hook();
                }
                // spec: STO-68 -- drop (not hard-error) any entry that fails
                // revalidation, warning on stderr.
                reg.sources = revalidate_sources(reg.sources, |msg| eprintln!("{msg}"));
                // spec: STO-73 -- migrate a relative local `url` to an absolute
                // form when it still resolves, so a later `cd` cannot make an
                // already-registered local source unreachable (U40). Rewrites
                // `url` only, and only in memory here; it persists the next
                // time `Registry::save` runs, same as any other mutation.
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                for src in &mut reg.sources {
                    migrate_relative_local_url(&cwd, src);
                }
                Ok(reg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure_layout()?;
        let file = paths.sources_file();
        // Always write the current version (STO-50).
        let versioned = serde_json::json!({
            "version": REGISTRY_VERSION,
            "sources": self.sources,
        });
        let json = serde_json::to_vec_pretty(&versioned)
            .map_err(|e| MindError::json("sources.json", e))?;
        Paths::atomic_write(&file, &json)
    }

    pub fn find(&self, name: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    // spec: CLI-11 (repo spec parsing), CLI-61 (compare url), CLI-176 (compare url github shape)
    // spec: CLI-188 (gitlab/bitbucket suppression), STO-13 (identity), STO-18 (pin serde round-trip)
    use super::*;

    #[test]
    fn parses_owner_repo_shorthand() {
        let s = parse_spec("james/agents").unwrap();
        assert_eq!(s.host, "github.com");
        assert_eq!(s.owner, "james");
        assert_eq!(s.repo, "agents");
        assert_eq!(s.name, "github.com/james/agents");
        assert_eq!(s.url, "https://github.com/james/agents");
    }

    #[test]
    fn identity_is_host_owner_repo_so_basenames_can_repeat() {
        assert_eq!(
            parse_spec("james/agents").unwrap().name,
            "github.com/james/agents"
        );
        assert_eq!(
            parse_spec("bob/agents").unwrap().name,
            "github.com/bob/agents"
        );
        // Same basename, different owner -> distinct identities.
        assert_ne!(
            parse_spec("james/agents").unwrap().name,
            parse_spec("bob/agents").unwrap().name
        );
        // Same owner/repo, different host -> distinct identities.
        assert_ne!(
            parse_spec("https://github.com/a/b").unwrap().name,
            parse_spec("https://gitlab.com/a/b").unwrap().name
        );
    }

    #[test]
    fn parses_github_prefix() {
        let s = parse_spec("github:foo/bar").unwrap();
        assert_eq!(s.url, "https://github.com/foo/bar");
    }

    // ---- item links (LNK-1..4, LNK-10) ----

    // spec: LNK-1 LNK-3 LNK-4
    #[test]
    fn parses_github_tree_link() {
        let s = parse_spec("https://github.com/o/r/tree/main/skills/foo").unwrap();
        assert_eq!(s.name, "github.com/o/r#skills/foo");
        assert_eq!(s.url, "https://github.com/o/r");
        assert_eq!(s.item_path.as_deref(), Some("skills/foo"));
        assert_eq!(s.pin, Pin::FollowBranch("main".into()));
        assert_eq!(s.base_identity(), "github.com/o/r");
    }

    // spec: LNK-1
    #[test]
    fn parses_blob_link_and_strips_skill_md() {
        let s = parse_spec("https://github.com/o/r/blob/main/skills/foo/SKILL.md").unwrap();
        assert_eq!(s.item_path.as_deref(), Some("skills/foo"));
        // A tree link naming the SKILL.md directly is also accepted.
        let t = parse_spec("https://github.com/o/r/tree/main/skills/foo/SKILL.md").unwrap();
        assert_eq!(t.item_path.as_deref(), Some("skills/foo"));
        // A blob link NOT ending in SKILL.md is invalid, not a repo spec.
        assert!(parse_spec("https://github.com/o/r/blob/main/skills/foo").is_err());
    }

    // spec: LNK-1
    #[test]
    fn parses_gitlab_dash_link_forms() {
        let s = parse_spec("https://gitlab.com/o/r/-/tree/main/skills/foo").unwrap();
        assert_eq!(s.name, "gitlab.com/o/r#skills/foo");
        let b = parse_spec("https://gitlab.com/o/r/-/blob/v2/skills/foo/SKILL.md").unwrap();
        assert_eq!(b.pin, Pin::FollowBranch("v2".into()));
    }

    // spec: LNK-1
    #[test]
    fn link_query_and_fragment_are_stripped() {
        let s =
            parse_spec("https://github.com/o/r/blob/main/skills/foo/SKILL.md?plain=1#L10").unwrap();
        assert_eq!(s.item_path.as_deref(), Some("skills/foo"));
    }

    // spec: LNK-3
    #[test]
    fn link_forty_hex_ref_pins_the_commit() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let s = parse_spec(&format!("https://github.com/o/r/tree/{sha}/skills/foo")).unwrap();
        assert_eq!(s.pin, Pin::Ref(sha.into()));
    }

    // spec: LNK-2 LNK-14
    #[test]
    fn link_marker_without_a_valid_tail_is_bad_item_link() {
        // No item path after the ref.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/main"),
            Err(MindError::BadItemLink { .. })
        ));
        // No ref at all.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/"),
            Err(MindError::BadItemLink { .. })
        ));
        // The message names the expected shapes and the offending URL.
        let err = parse_spec("https://github.com/o/r/tree/main").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("tree/<ref>"), "{msg}");
        assert!(msg.contains("blob/<ref>"), "{msg}");
        assert!(
            msg.contains("https://github.com/o/r/tree/main"),
            "message must include the offending URL: {msg}"
        );
    }

    // spec: LNK-2
    #[test]
    fn a_plain_non_link_bad_spec_stays_invalid_repo_spec() {
        // No tree/blob marker at all: this is not an attempted item link, so
        // it must keep the generic repo-spec message, not BadItemLink.
        assert!(matches!(
            parse_spec("https://github.com/"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
    }

    // spec: LNK-10 LNK-14
    #[test]
    fn link_unsafe_path_or_ref_is_rejected_at_parse() {
        // `..` in the item path.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/main/../../etc"),
            Err(MindError::BadItemLink { .. })
        ));
        // A git-range ref value.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/a..b/skills/foo"),
            Err(MindError::BadItemLink { .. })
        ));
        // A leading-dash (option-shaped) ref value.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/-evil/skills/foo"),
            Err(MindError::BadItemLink { .. })
        ));
    }

    // spec: LNK-14
    #[test]
    fn gitlab_dash_link_bad_tail_is_bad_item_link() {
        // The GitLab `/-/tree/` and `/-/blob/` spellings must route the same
        // set of tail failures to BadItemLink as the plain `/tree/`/`/blob/`
        // forms do.
        // Missing ref entirely.
        assert!(matches!(
            parse_spec("https://gitlab.com/o/r/-/tree/"),
            Err(MindError::BadItemLink { .. })
        ));
        // Ref present, no skill path.
        assert!(matches!(
            parse_spec("https://gitlab.com/o/r/-/tree/main"),
            Err(MindError::BadItemLink { .. })
        ));
        // Blob link not ending in /SKILL.md.
        assert!(matches!(
            parse_spec("https://gitlab.com/o/r/-/blob/main/skills/foo"),
            Err(MindError::BadItemLink { .. })
        ));
        // Unsafe (`..`) path component.
        assert!(matches!(
            parse_spec("https://gitlab.com/o/r/-/tree/main/../etc"),
            Err(MindError::BadItemLink { .. })
        ));
        // The message still names the GitLab URL and the expected shapes.
        let err = parse_spec("https://gitlab.com/o/r/-/tree/main").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("https://gitlab.com/o/r/-/tree/main"),
            "message must include the offending URL: {msg}"
        );
        assert!(
            msg.contains("tree/<ref>") && msg.contains("blob/<ref>"),
            "{msg}"
        );
    }

    // spec: LNK-14
    #[test]
    fn file_link_bad_tail_is_bad_item_link() {
        // The local `file://` link form must route the same tail failures to
        // BadItemLink as a remote forge URL.
        assert!(matches!(
            parse_spec("file:///home/me/dev/agents/tree/main"),
            Err(MindError::BadItemLink { .. })
        ));
        assert!(matches!(
            parse_spec("file:///home/me/dev/agents/tree/"),
            Err(MindError::BadItemLink { .. })
        ));
        assert!(matches!(
            parse_spec("file:///home/me/dev/agents/blob/main/skills/foo"),
            Err(MindError::BadItemLink { .. })
        ));
        assert!(matches!(
            parse_spec("file:///home/me/dev/agents/tree/main/../../etc"),
            Err(MindError::BadItemLink { .. })
        ));
        let err = parse_spec("file:///home/me/dev/agents/tree/main").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("file:///home/me/dev/agents/tree/main"),
            "message must include the offending URL: {msg}"
        );
    }

    // spec: LNK-2 LNK-14
    #[test]
    fn file_link_bad_tail_with_no_repo_ahead_of_marker_is_still_bad_item_link() {
        // Ordering finding: for a remote URL, a malformed owner/repo ahead of
        // the marker is checked BEFORE the tail (LNK-2's "regardless of the
        // marker" carve-out), so it stays InvalidRepoSpec even when the tail
        // is also broken (see `github_malformed_owner_repo_wins_over_bad_tail`
        // below). The local file:// branch parses the tail FIRST (source.rs
        // parse_spec's local-link block calls parse_link_tail before slicing
        // owner/repo out of repo_part), so with NO repo path at all ahead of
        // the marker, a broken tail still reports BadItemLink rather than
        // falling through to InvalidRepoSpec. This pins the current (order-
        // dependent) behavior; if the two branches are ever unified this test
        // should be revisited.
        assert!(matches!(
            parse_spec("file:///tree/main"),
            Err(MindError::BadItemLink { .. })
        ));
    }

    // spec: LNK-2
    #[test]
    fn github_malformed_owner_repo_wins_over_bad_tail() {
        // Mirror of the file:// case above for the remote-URL branch: when
        // BOTH the owner/repo portion ahead of the marker is malformed AND the
        // tail is broken, LNK-2 says the malformed-owner/repo error wins
        // (InvalidRepoSpec), not BadItemLink.
        assert!(matches!(
            parse_spec("https://github.com/only-one-segment/tree/main"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
    }

    // spec: LNK-1
    #[test]
    fn uppercase_tree_segment_is_not_a_link_marker() {
        // The marker match is exact-case; an uppercase `/TREE/` segment is not
        // recognized, so this is not treated as an attempted item link at all.
        // With the extra path segments present, `owner/repo` parsing then
        // rejects it as a malformed repo spec (a slash inside what would be
        // the repo component), keeping the generic message per LNK-14's last
        // sentence ("no marker at all... keeps reporting InvalidRepoSpec").
        let err = parse_spec("https://github.com/o/r/TREE/main/skills/foo").unwrap_err();
        assert!(
            matches!(err, MindError::InvalidRepoSpec { .. }),
            "uppercase TREE must not be treated as a link marker: {err:?}"
        );
    }

    // spec: LNK-1 LNK-3
    #[test]
    fn doubled_slash_after_marker_absorbs_into_the_ref_not_an_error() {
        // A doubled slash right after the marker (`/tree//skills/foo`) is not
        // reported as "missing ref": `parse_link_tail` trims ALL leading
        // slashes off the tail before splitting on `/`, so the doubled slash
        // collapses and the first non-empty segment (`skills`) is taken as the
        // ref, leaving `foo` as the item path. This documents the actual
        // (surprising) parse rather than asserting an error, since the code
        // structurally cannot distinguish this from a single slash.
        let s = parse_spec("https://github.com/o/r/tree//skills/foo").unwrap();
        assert_eq!(s.pin, Pin::FollowBranch("skills".into()));
        assert_eq!(s.item_path.as_deref(), Some("foo"));
    }

    // spec: LNK-1 LNK-14
    #[test]
    fn query_and_fragment_noise_is_stripped_before_tail_validation_too() {
        // The query/fragment strip (LNK-1) happens before the tail is
        // validated, not just on the success path: a bad tail wrapped in
        // `?plain=1#L10` noise must still be diagnosed as BadItemLink, not
        // silently swallowed into a differently-shaped (and differently
        // erroring) spec.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/main?plain=1#L10"),
            Err(MindError::BadItemLink { .. })
        ));
        let err = parse_spec("https://github.com/o/r/blob/main/skills/foo?plain=1").unwrap_err();
        assert!(matches!(err, MindError::BadItemLink { .. }));
        // The reported url echoes the caller's original spec verbatim
        // (including the query string): only the internal marker/tail search
        // strips it, not the message.
        assert!(
            err.to_string().contains("?plain=1"),
            "the reported url is the original spec verbatim: {err}"
        );
    }

    // spec: LNK-1 LNK-4
    #[test]
    fn file_link_is_a_pinned_local_instance() {
        let s = parse_spec("file:///home/me/dev/agents/tree/main/skills/foo").unwrap();
        assert_eq!(s.host, "local");
        assert_eq!(s.name, "local/dev/agents#skills/foo");
        assert_eq!(s.url, "/home/me/dev/agents");
        assert_eq!(s.item_path.as_deref(), Some("skills/foo"));
        assert_eq!(s.pin, Pin::FollowBranch("main".into()));
        // The pin means it is a cloned snapshot, never a linked working tree.
        assert!(!s.is_linked());
        // A BARE local path is never marker-parsed: a repo dir literally named
        // `tree/main/...` stays a plain repo spec.
        let plain = parse_spec("/home/me/dev/agents/tree/main/skills/foo").unwrap();
        assert!(plain.item_path.is_none());
        assert_eq!(plain.repo, "foo");
    }

    // spec: LNK-1
    #[test]
    fn plain_repo_url_is_not_an_item_link() {
        let s = parse_spec("https://github.com/o/r").unwrap();
        assert!(s.item_path.is_none());
        assert_eq!(s.pin, Pin::DefaultBranch);
    }

    #[test]
    fn parses_https_url_and_strips_dot_git() {
        let s = parse_spec("https://github.com/foo/bar.git").unwrap();
        assert_eq!(s.host, "github.com");
        assert_eq!(s.repo, "bar");
        assert_eq!(s.url, "https://github.com/foo/bar");
    }

    #[test]
    fn parses_ssh_form() {
        let s = parse_spec("git@github.com:foo/bar.git").unwrap();
        assert_eq!(s.host, "github.com");
        assert_eq!(s.owner, "foo");
        assert_eq!(s.repo, "bar");
    }

    // ---- ssh:// userinfo (STO-67) ----

    // spec: STO-67 CLI-19
    #[test]
    fn ssh_url_with_userinfo_keeps_it_in_url_but_not_identity() {
        let s = parse_spec("ssh://git@github.com/acme/agents").unwrap();
        assert_eq!(s.host, "github.com");
        assert_eq!(s.owner, "acme");
        assert_eq!(s.repo, "agents");
        assert_eq!(s.name, "github.com/acme/agents");
        assert_eq!(s.url, "ssh://git@github.com/acme/agents");
    }

    // spec: STO-67
    #[test]
    fn ssh_url_with_userinfo_on_a_non_github_host() {
        let s = parse_spec("ssh://git@ghe.corp/team/agents.git").unwrap();
        assert_eq!(s.host, "ghe.corp");
        assert_eq!(s.owner, "team");
        assert_eq!(s.repo, "agents");
        assert_eq!(s.name, "ghe.corp/team/agents");
        assert_eq!(s.url, "ssh://git@ghe.corp/team/agents");
    }

    // spec: STO-67
    #[test]
    fn ssh_url_without_userinfo_still_parses() {
        let s = parse_spec("ssh://host/o/r").unwrap();
        assert_eq!(s.host, "host");
        assert_eq!(s.owner, "o");
        assert_eq!(s.repo, "r");
        assert_eq!(s.url, "ssh://host/o/r");
    }

    // spec: STO-67
    #[test]
    fn https_url_with_userinfo_is_refused() {
        let err = parse_spec("https://token@host/o/r").unwrap_err();
        match err {
            MindError::UnsafeRepoSpec {
                part,
                value,
                reason,
                ..
            } => {
                assert_eq!(part, "host");
                assert_eq!(value, "token@host");
                assert!(
                    reason.contains("credential"),
                    "reason must name the credential: {reason}"
                );
            }
            other => panic!("expected UnsafeRepoSpec naming host, got {other:?}"),
        }
    }

    // spec: STO-67 LNK-1 LNK-4
    #[test]
    fn ssh_item_link_keeps_userinfo_in_url() {
        let s = parse_spec("ssh://git@host/o/r/tree/main/skills/foo").unwrap();
        assert_eq!(s.name, "host/o/r#skills/foo");
        assert_eq!(s.url, "ssh://git@host/o/r");
        assert_eq!(s.item_path.as_deref(), Some("skills/foo"));
        assert_eq!(s.base_identity(), "host/o/r");
    }

    // spec: STO-67
    #[test]
    fn ssh_userinfo_control_character_is_rejected() {
        let err = parse_spec("ssh://git\u{7}@host/o/r").unwrap_err();
        match err {
            MindError::UnsafeRepoSpec { part, reason, .. } => {
                assert_eq!(part, "host");
                assert!(
                    reason.contains("control character"),
                    "reason must name the control character: {reason}"
                );
            }
            other => panic!("expected UnsafeRepoSpec naming host, got {other:?}"),
        }
    }

    // spec: STO-67
    // The "split on the LAST `@`" clause, driven with an authority that has
    // MORE than one `@`: a userinfo that itself contains an `@` (an email-shaped
    // login, e.g. `user@corp.example`) must not be mistaken for the host. A
    // first-`@` split would yield host `corp.example@git.host`, which then fails
    // host validation (`@` is refused in `host`, STO-64); a last-`@` split
    // yields the real host.
    #[test]
    fn ssh_authority_splits_on_the_last_at_not_the_first() {
        let s = parse_spec("ssh://user@corp.example@git.host/o/r").unwrap();
        assert_eq!(
            s.host, "git.host",
            "the host is the part after the LAST '@'"
        );
        assert_eq!(s.name, "git.host/o/r");
        assert_eq!(
            s.url, "ssh://user@corp.example@git.host/o/r",
            "the FULL authority (both '@'s) must stay in url so git clone authenticates"
        );
    }

    // spec: STO-67
    // The "empty userinfo" leg of the userinfo validation. Only the control
    // character leg was previously driven.
    #[test]
    fn ssh_empty_userinfo_is_rejected() {
        let err = parse_spec("ssh://@host/o/r").unwrap_err();
        match err {
            MindError::UnsafeRepoSpec {
                part,
                value,
                reason,
                ..
            } => {
                assert_eq!(part, "host");
                assert_eq!(value, "@host", "value is the FULL authority");
                assert!(
                    reason.contains("empty"),
                    "reason must name the empty userinfo: {reason}"
                );
            }
            other => panic!("expected UnsafeRepoSpec naming host, got {other:?}"),
        }
    }

    // spec: STO-67 CLI-204
    // "The part after the split is the identity host (validated with the same
    // rules as any other host, CLI-204)": the userinfo split must not become a
    // bypass of host validation. Each case is a host that CLI-204/STO-64
    // refuses, reached only through the ssh-userinfo branch.
    #[test]
    fn ssh_host_after_userinfo_is_still_validated_like_any_other_host() {
        for (spec, expect_reason) in [
            // An empty host (`ssh://git@/o/r`) after a legal userinfo.
            ("ssh://git@/o/r", "is empty"),
            // `#` is refused in host (STO-64).
            ("ssh://git@ho#st/o/r", "'#'"),
            // A path separator cannot appear in a single host component.
            ("ssh://git@ho\\st/o/r", "contains a path separator"),
            // A traversing host component.
            ("ssh://git@../o/r", "is a relative path component"),
        ] {
            match parse_spec(spec) {
                Err(MindError::UnsafeRepoSpec { part, reason, .. }) => {
                    assert_eq!(part, "host", "{spec} must be refused on the host part");
                    assert!(
                        reason.contains(expect_reason),
                        "{spec}: reason must be {expect_reason:?}, got {reason:?}"
                    );
                }
                other => panic!("{spec} must be refused as an unsafe host, got {other:?}"),
            }
        }
    }

    // spec: STO-67 STO-11
    // "feeds the source's identity and clone path (STO-11)": the userinfo must
    // never reach disk as a directory name.
    #[test]
    fn ssh_userinfo_never_reaches_the_clone_path() {
        let (base, paths) = tmp_paths_src();
        let s = parse_spec("ssh://git@git.host/o/r").unwrap();
        let dir = s.clone_dir(&paths);
        let shown = dir.display().to_string();
        assert!(
            !shown.contains("git@"),
            "the clone path must not embed the userinfo: {shown}"
        );
        assert!(
            dir.ends_with("git.host/o/r"),
            "clone path must be sources/<host>/<owner>/<repo>: {shown}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: STO-67
    // The credential refusal must fire for EVERY non-ssh scheme, and it must
    // fire BEFORE the item-link branch: a deep URL carrying a token is the most
    // likely way a credential would be pasted in, and it must not slip through
    // as a `BadItemLink` (or worse, be accepted).
    #[test]
    fn non_ssh_userinfo_is_refused_for_every_scheme_including_deep_links() {
        for spec in [
            "https://token@host/o/r",
            "http://user:pw@host/o/r",
            "git://token@host/o/r",
            // Deep item-link URLs: refused before parse_item_link runs.
            "https://token@host/o/r/tree/main/skills/foo",
            "http://user:pw@host/o/r/blob/main/skills/foo/SKILL.md",
        ] {
            match parse_spec(spec) {
                Err(MindError::UnsafeRepoSpec {
                    part,
                    value,
                    reason,
                    ..
                }) => {
                    assert_eq!(part, "host", "{spec}");
                    assert!(
                        value.contains('@'),
                        "{spec}: value must be the full authority, got {value:?}"
                    );
                    assert!(
                        reason.contains("credential"),
                        "{spec}: reason must name the credential, got {reason:?}"
                    );
                }
                other => panic!("{spec} must be refused as an embedded credential, got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_local_path() {
        let s = parse_spec("/home/user/dev/agents").unwrap();
        assert_eq!(s.host, "local");
        assert_eq!(s.owner, "dev"); // parent directory becomes the owner
        assert_eq!(s.repo, "agents");
        assert_eq!(s.name, "local/dev/agents");
        assert_eq!(s.url, "/home/user/dev/agents");
    }

    // CLI-204: the `host`, `owner`, and `repo` parts are both identity segments
    // and clone-path components, so a part that is not a single safe path
    // component is refused at parse time. The SSH form is the sharp edge: it
    // splits only on the first `:`, so without the check `host` absorbs
    // arbitrary path and marker characters.
    // spec: CLI-204
    #[test]
    fn rejects_unsafe_identity_parts() {
        let cases = [
            // A traversing host escapes the sources tree at clone/remove time.
            ("git@../../elsewhere:owner/repo", "host"),
            ("git@..:owner/repo", "host"),
            // A host with a `/` forges extra identity segments.
            ("git@evil/host:owner/repo", "host"),
            // A host with `@` is what makes the ssh form ambiguous.
            ("git@github.com/acme/agents@x:o/r", "host"),
            ("git@evil#frag:owner/repo", "host"),
            // A traversing owner escapes one level up.
            ("github:../x", "owner"),
        ];
        for (spec, part) in cases {
            match parse_spec(spec) {
                Err(MindError::UnsafeRepoSpec { part: p, .. }) => {
                    assert_eq!(p, part, "{spec}: wrong part named")
                }
                other => panic!("{spec}: expected UnsafeRepoSpec on {part}, got {other:?}"),
            }
        }
    }

    // STO-72: a traversing local path used to reach `rejects_unsafe_identity_parts`
    // above (`/src/parent/..` derived a literal `repo = ".."`, refused by
    // CLI-204). Absolutizing BEFORE identity derivation resolves the `..`
    // lexically first, so the derived `repo` is the real final path component
    // and is never `..` in the first place: this is a strictly safer outcome
    // (no traversal syntax ever reaches identity derivation at all), not a
    // regression of CLI-204's protection.
    // spec: STO-72
    #[test]
    fn absolutize_resolves_parent_dir_lexically_before_identity_derivation() {
        let s = parse_spec("/src/parent/..").expect(
            "STO-72: the '..' is resolved away before repo/owner derivation, so this now parses",
        );
        assert_eq!(s.url, "/src", "the '..' must be resolved lexically");
        assert_eq!(s.repo, "src");
    }

    // spec: STO-72
    #[test]
    fn absolutize_against_joins_relative_paths_onto_the_given_base() {
        let base = Path::new("/some/base/dir");
        assert_eq!(
            absolutize_against(base, "./foo"),
            "/some/base/dir/foo",
            "a './' path joins onto base"
        );
        assert_eq!(
            absolutize_against(base, "../foo"),
            "/some/base/foo",
            "a '../' path resolves one level up from base"
        );
        assert_eq!(
            absolutize_against(base, "/abs/path"),
            "/abs/path",
            "an already-absolute path is left unchanged (base is not joined)"
        );
        assert_eq!(
            absolutize_against(base, "a/./b/../c"),
            "/some/base/dir/a/c",
            "internal '.'/'..' components are resolved too"
        );
    }

    // spec: STO-72
    #[test]
    fn absolutize_against_keeps_a_leading_parent_dir_with_nothing_to_cancel() {
        // A '..' at the very start of an absolute path has nothing to pop
        // against (the root itself), so it is kept rather than panicking or
        // silently discarded; the caller (parse_spec) then fails cleanly at the
        // "repo is empty" check rather than escaping anywhere.
        let base = Path::new("/");
        assert_eq!(absolutize_against(base, "../.."), "/../..");
    }

    // STO-72: the regression fix's own headline example -- a relative `./foo`
    // spec must derive its owner from the REAL parent directory (whatever the
    // test process's actual cwd happens to be), not the literal `.` in the
    // spec string collapsing to the `local` placeholder (`local/local/foo`).
    // Reads (never mutates) the real cwd, so this is safe under parallel tests.
    // spec: STO-72
    #[test]
    fn relative_dot_slash_local_path_does_not_double_the_local_owner() {
        let cwd = std::env::current_dir().expect("must read cwd");
        let parent_name = cwd
            .file_name()
            .expect("cwd must have a basename")
            .to_string_lossy()
            .into_owned();
        let s = parse_spec("./foo").expect("a relative local path must still parse");
        assert_eq!(s.repo, "foo");
        assert_eq!(
            s.owner, parent_name,
            "owner must be the real parent directory name, not the 'local' placeholder"
        );
        assert_eq!(s.name, format!("local/{parent_name}/foo"));
        assert_eq!(s.url, cwd.join("foo").to_string_lossy());
    }

    // spec: STO-72
    // The `../rel/path` leg of the same rule (only `./foo` was driven). A
    // `../` spec is the one that most obviously breaks after a `cd`, and its
    // owner must come from the real grandparent directory -- again never the
    // `local` placeholder that the literal `..` segment used to collapse to.
    #[test]
    fn relative_parent_dir_local_path_derives_owner_from_the_real_grandparent() {
        let cwd = std::env::current_dir().expect("must read cwd");
        let parent = cwd.parent().expect("cwd must have a parent");
        let grandparent_name = parent
            .file_name()
            .expect("parent must have a basename")
            .to_string_lossy()
            .into_owned();
        let s = parse_spec("../foo").expect("a '../' local path must parse");
        assert_eq!(s.repo, "foo");
        assert_eq!(
            s.owner, grandparent_name,
            "owner must be the real grandparent directory, not the 'local' placeholder"
        );
        assert_eq!(s.name, format!("local/{grandparent_name}/foo"));
        assert_eq!(
            s.url,
            parent.join("foo").to_string_lossy(),
            "the persisted url must be the absolute resolution, not the literal '../foo'"
        );
    }

    // spec: STO-72 LNK-4
    // The third form STO-72 names: "the local branch of a `file://` item link".
    // This branch has its OWN absolutize call (a separate code path from the
    // bare local-path branch), and nothing drove it: a relative `file://` link
    // must persist an absolute `url` and derive its owner from the real parent,
    // while still carrying the item-link identity suffix.
    #[test]
    fn relative_file_url_item_link_is_absolutized_before_identity_derivation() {
        let cwd = std::env::current_dir().expect("must read cwd");
        let parent_name = cwd
            .file_name()
            .expect("cwd must have a basename")
            .to_string_lossy()
            .into_owned();
        let s = parse_spec("file://./foo/tree/main/skills/greet")
            .expect("a relative file:// item link must parse");
        assert_eq!(s.repo, "foo");
        assert_eq!(
            s.owner, parent_name,
            "owner must be the real parent directory, not the 'local' placeholder"
        );
        assert_eq!(s.name, format!("local/{parent_name}/foo#skills/greet"));
        assert_eq!(s.item_path.as_deref(), Some("skills/greet"));
        assert_eq!(
            s.url,
            cwd.join("foo").to_string_lossy(),
            "the link's repo url must be absolutized, not the literal './foo'"
        );
    }

    // CLI-204/STO-64: `@` stays legal in `owner` only. A local path may
    // legitimately carry it there (`/src/proj@v2/agents`), and it collides with
    // nothing: the `@<alias>` identity suffix (STO-58) only ever appends to
    // `repo`, never `owner`. This is the case that motivated allowing the
    // character at all, so it must not regress.
    // spec: CLI-204
    // spec: STO-64
    #[test]
    fn at_marker_is_allowed_in_owner_only() {
        let s = parse_spec("/src/proj@v2/agents").unwrap();
        assert_eq!(s.owner, "proj@v2");
        assert_eq!(s.base_identity(), "local/proj@v2/agents");
    }

    // LNK-16: the same collision one segment over. A link identity is
    // `host/owner/repo#<item_path>` and an alias appends `@<alias>` to the whole
    // thing, so an unaliased link to `skills/foo@bar` and an `@bar`-aliased link
    // to `skills/foo` would compute the same identity and clone dir.
    // spec: LNK-16
    #[test]
    fn at_and_hash_are_rejected_in_an_item_link_path() {
        // `@` is reachable in both link forms.
        for url in [
            "https://github.com/acme/repo/tree/main/skills/foo@bar",
            "file:///src/acme/repo/tree/main/skills/foo@bar",
        ] {
            match parse_spec(url) {
                Err(MindError::BadItemLink { reason, .. }) => assert!(
                    reason.contains("identity markers"),
                    "{url}: reason should name the marker rule, got {reason:?}"
                ),
                other => panic!("{url}: expected BadItemLink, got {other:?}"),
            }
        }
        // `#` reaches `item_path` only through the `file://` form: a remote URL
        // strips everything from the first `#` as a pasted browser fragment
        // (LNK-1), so it never becomes part of the path.
        match parse_spec("file:///src/acme/repo/tree/main/skills/foo#bar") {
            Err(MindError::BadItemLink { .. }) => {}
            other => panic!("expected BadItemLink for a `#` in a local link path, got {other:?}"),
        }
        let stripped = parse_spec("https://github.com/acme/repo/tree/main/skills/foo#bar").unwrap();
        assert_eq!(
            stripped.name, "github.com/acme/repo#skills/foo",
            "a remote URL's fragment is stripped, not smuggled into the path"
        );

        // A path free of markers is still accepted.
        let ok = parse_spec("https://github.com/acme/repo/tree/main/skills/foo").unwrap();
        assert_eq!(ok.name, "github.com/acme/repo#skills/foo");
    }

    // LNK-17: `.` segments are dropped before the path becomes identity, so a
    // `./`-variant URL parses to the same instance as the plain form instead
    // of registering a duplicate for the same on-disk skill.
    // spec: LNK-17
    #[test]
    fn curdir_segments_normalize_out_of_an_item_link_path() {
        let plain = parse_spec("https://github.com/acme/repo/tree/main/skills/foo").unwrap();
        for url in [
            "https://github.com/acme/repo/tree/main/./skills/foo",
            "https://github.com/acme/repo/tree/main/skills/./foo",
            "https://github.com/acme/repo/tree/main/./skills/./foo",
            "https://github.com/acme/repo/blob/main/skills/./foo/SKILL.md",
        ] {
            let s = parse_spec(url).unwrap();
            assert_eq!(
                s.name, plain.name,
                "{url}: must normalize to the plain instance identity"
            );
            assert_eq!(s.item_path.as_deref(), Some("skills/foo"), "{url}");
        }
        // A path that is nothing but `.` segments has no item path left.
        match parse_spec("https://github.com/acme/repo/tree/main/./.") {
            Err(MindError::BadItemLink { reason, .. }) => assert!(
                reason.contains("missing the item path"),
                "an all-dots path must report a missing item path, got {reason:?}"
            ),
            other => panic!("expected BadItemLink for an all-dots path, got {other:?}"),
        }
    }

    // STO-64: `@` and `#` are now rejected in `repo` -- both collide with an
    // identity suffix (`@<alias>`, STO-58) and/or the clone-dir leaf (STO-59),
    // and `#` also collides with the item-link marker (LNK-4). Driven through
    // `parse_spec` so the identity is built exactly as production builds it.
    // spec: STO-64
    #[test]
    fn at_and_hash_are_rejected_in_repo() {
        match parse_spec("/src/parent/blessed@evil") {
            Err(MindError::UnsafeRepoSpec { part, value, .. }) => {
                assert_eq!(part, "repo");
                assert_eq!(value, "blessed@evil");
            }
            other => panic!("expected UnsafeRepoSpec on repo, got {other:?}"),
        }
        match parse_spec("/src/parent/blessed#evil") {
            Err(MindError::UnsafeRepoSpec { part, value, .. }) => {
                assert_eq!(part, "repo");
                assert_eq!(value, "blessed#evil");
            }
            other => panic!("expected UnsafeRepoSpec on repo, got {other:?}"),
        }
        // Same via the SSH form, where repo is the segment after the ':owner/'.
        match parse_spec("git@github.com:owner/blessed@evil") {
            Err(MindError::UnsafeRepoSpec { part, .. }) => assert_eq!(part, "repo"),
            other => panic!("expected UnsafeRepoSpec on repo, got {other:?}"),
        }
    }

    // STO-64: `#` is now rejected in `owner` too -- it would land before the
    // repo segment and confuse `#`-splitting in item refs and hook targets.
    // spec: STO-64
    #[test]
    fn hash_is_rejected_in_owner() {
        match parse_spec("github:evil#owner/repo") {
            Err(MindError::UnsafeRepoSpec { part, value, .. }) => {
                assert_eq!(part, "owner");
                assert_eq!(value, "evil#owner");
            }
            other => panic!("expected UnsafeRepoSpec on owner, got {other:?}"),
        }
    }

    // STO-64: the regression this tightening closes. Before the fix, `repo`
    // permitted `@`/`#`, so a repo directory literally named `foo@bar` (an
    // unaliased meld) and a repo `foo` melded with an identity alias `bar`
    // (`meld --as bar`) produced the exact SAME `compute_name()` identity
    // (`local/<owner>/foo@bar`) AND the exact same `clone_dir()` leaf
    // (`foo@bar`) -- two genuinely different sources would collide in the
    // registry (the second treated as a re-meld of the first) and share one
    // clone on disk. This test pins the fix: the two no longer coincide,
    // because the literal `@` in the directory name is now refused before a
    // `Source` is ever built.
    // spec: STO-64
    #[test]
    fn distinct_sources_no_longer_collide_on_repo_at_alias_suffix() {
        let (base, paths) = tmp_paths_src();

        // A remote repo whose name literally contains `@bar` (host != "local",
        // so clone_dir takes the cloned-tree branch rather than the linked
        // working-tree shortcut, engaging the same identity/clone-leaf code
        // as the `--as bar` case below).
        let literal = parse_spec("https://forge.example.com/owner/foo@bar");
        // An ordinary repo `foo`, meant to be melded with `--as bar` (which
        // appends `@bar` to build the SAME identity/clone leaf the literal
        // repo name would have produced, had it been allowed through).
        let mut aliased =
            parse_spec("https://forge.example.com/owner/foo").expect("plain repo must parse");
        aliased.apply_alias(Some("bar".into()));

        match literal {
            Err(MindError::UnsafeRepoSpec { part, .. }) => {
                assert_eq!(
                    part, "repo",
                    "a literal `@` in the repo name must be refused as an unsafe repo part"
                );
            }
            Ok(s) => panic!(
                "unfixed behavior: a repo named `foo@bar` parsed successfully to identity \
                 {:?} (clone dir {:?}), colliding with the aliased instance's identity {:?} \
                 (clone dir {:?}) -- this is the exact collision STO-64 closes",
                s.compute_name(),
                s.clone_dir(&paths),
                aliased.compute_name(),
                aliased.clone_dir(&paths)
            ),
            other => panic!("expected UnsafeRepoSpec on repo, got {other:?}"),
        }

        // The aliased instance itself is unaffected: it still builds the
        // `@bar`-suffixed identity and clone leaf normally.
        assert_eq!(aliased.compute_name(), "forge.example.com/owner/foo@bar");
        assert!(
            aliased
                .clone_dir(&paths)
                .ends_with("forge.example.com/owner/foo@bar")
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_garbage_specs() {
        for bad in ["", "noslash", "trailing/", "/leading-only-after-strip"] {
            // "/..." is treated as a local path, so only the truly empty/oneword cases error.
            if bad.starts_with('/') {
                continue;
            }
            assert!(parse_spec(bad).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn ssh_url_uses_the_git_at_form() {
        // spec: CLI-19
        let s = parse_spec("james/agents").unwrap();
        assert_eq!(s.ssh_url(), "git@github.com:james/agents");
    }

    #[test]
    fn prefer_ssh_rewrites_https_remotes_only() {
        // spec: CLI-19
        // https shorthand -> ssh, and the rewrite persists on the source.
        let mut s = parse_spec("james/agents").unwrap();
        s.prefer_ssh(true);
        assert_eq!(s.url, "git@github.com:james/agents");

        // An explicit git@ URL is already SSH: unchanged.
        let mut g = parse_spec("git@github.com:foo/bar.git").unwrap();
        let before = g.url.clone();
        g.prefer_ssh(true);
        assert_eq!(g.url, before);

        // A local path is never rewritten.
        let mut l = parse_spec("/tmp/x").unwrap();
        let lbefore = l.url.clone();
        l.prefer_ssh(true);
        assert_eq!(l.url, lbefore);

        // prefer_ssh = false is a no-op: the https URL is kept.
        let mut h = parse_spec("james/agents").unwrap();
        h.prefer_ssh(false);
        assert_eq!(h.url, "https://github.com/james/agents");
    }

    #[test]
    fn prefer_ssh_rewrites_plain_http_url() {
        // spec: DSC-66 - an http:// (non-TLS) remote is rewritten to the SSH
        // form under prefer_ssh, the same as an https:// remote. Previously only
        // https:// was handled; an http://host/owner/repo URL was passed through
        // unchanged, leaving the same injection surface.
        let mut s = parse_spec("https://example.com/owner/repo").unwrap();
        // Manually set an http:// URL to simulate a plain-HTTP remote.
        s.url = "http://example.com/owner/repo".to_string();
        s.host = "example.com".to_string();
        s.prefer_ssh(true);
        assert_eq!(
            s.url, "git@example.com:owner/repo",
            "http:// remote must be rewritten to SSH form under prefer_ssh"
        );

        // http:// with prefer_ssh=false: unchanged.
        let mut s2 = parse_spec("https://example.com/owner/repo").unwrap();
        s2.url = "http://example.com/owner/repo".to_string();
        s2.host = "example.com".to_string();
        s2.prefer_ssh(false);
        assert_eq!(
            s2.url, "http://example.com/owner/repo",
            "prefer_ssh=false must leave an http:// URL unchanged"
        );
    }

    // spec: CLI-61, CLI-176
    #[test]
    fn compare_url_github_com_produces_correct_link() {
        // (a) github.com https remote -> same URL as before
        let gh = parse_spec("foo/bar").unwrap();
        assert_eq!(
            gh.compare_url("aaaa", "bbbb").as_deref(),
            Some("https://github.com/foo/bar/compare/aaaa...bbbb")
        );
    }

    #[test]
    fn compare_url_ghes_host_produces_same_shape() {
        // (b) GitHub Enterprise Server (any https host) -> same /compare/ shape on that host
        let ghes = parse_spec("https://github.example.com/acme/tools").unwrap();
        assert_eq!(
            ghes.compare_url("deadbeef", "cafebabe").as_deref(),
            Some("https://github.example.com/acme/tools/compare/deadbeef...cafebabe")
        );
        // Also verify a non-GitHub corporate forge host (neutral hostname -> GitHub shape)
        let corp = parse_spec("https://git.corp.internal/devtools/scripts").unwrap();
        assert_eq!(
            corp.compare_url("old", "new").as_deref(),
            Some("https://git.corp.internal/devtools/scripts/compare/old...new")
        );
    }

    // spec: CLI-188
    #[test]
    fn compare_url_gitlab_hosts_yield_none() {
        // gitlab.com and a self-hosted instance both use /-/compare/, not /compare/
        let gl = parse_spec("https://gitlab.com/org/project").unwrap();
        assert_eq!(gl.compare_url("aaaa", "bbbb"), None, "gitlab.com");

        let self_hosted = parse_spec("https://gitlab.corp.example.com/org/project").unwrap();
        assert_eq!(
            self_hosted.compare_url("aaaa", "bbbb"),
            None,
            "self-hosted gitlab"
        );
    }

    // spec: CLI-188
    #[test]
    fn compare_url_bitbucket_hosts_yield_none() {
        // bitbucket.org uses /branches/compare/, not /compare/
        let bb = parse_spec("https://bitbucket.org/org/repo").unwrap();
        assert_eq!(bb.compare_url("aaaa", "bbbb"), None, "bitbucket.org");
    }

    #[test]
    fn compare_url_ssh_remote_yields_none() {
        // (c) SSH remotes have no web host to link to
        let ssh = parse_spec("git@github.com:foo/bar.git").unwrap();
        assert_eq!(ssh.compare_url("aaaa", "bbbb"), None);
    }

    #[test]
    fn compare_url_local_path_yields_none() {
        // (d) local/file paths have no web host to link to
        let local = parse_spec("/home/user/dev/agents").unwrap();
        assert_eq!(local.compare_url("aaaa", "bbbb"), None);
        let file_url = parse_spec("file:///home/user/dev/agents").unwrap();
        assert_eq!(file_url.compare_url("aaaa", "bbbb"), None);
    }

    // ---- browse_url (HOOK-24) ----
    //
    // Same host guard as compare_url (CLI-176, CLI-188): https GitHub-shaped
    // hosts yield a /tree/<commit> URL; gitlab/bitbucket, SSH, and local paths
    // yield None.

    // spec: HOOK-24
    #[test]
    fn browse_url_github_com_produces_tree_link() {
        let gh = parse_spec("foo/bar").unwrap();
        assert_eq!(
            gh.browse_url("abc1234").as_deref(),
            Some("https://github.com/foo/bar/tree/abc1234")
        );
    }

    // spec: HOOK-24
    #[test]
    fn browse_url_ghes_host_produces_same_shape() {
        // GitHub Enterprise Server and neutral forge hosts use the same /tree/ shape.
        let ghes = parse_spec("https://github.example.com/acme/tools").unwrap();
        assert_eq!(
            ghes.browse_url("deadbeef").as_deref(),
            Some("https://github.example.com/acme/tools/tree/deadbeef")
        );
        let corp = parse_spec("https://git.corp.internal/devtools/scripts").unwrap();
        assert_eq!(
            corp.browse_url("cafebabe").as_deref(),
            Some("https://git.corp.internal/devtools/scripts/tree/cafebabe")
        );
    }

    // spec: HOOK-24
    #[test]
    fn browse_url_gitlab_hosts_yield_none() {
        let gl = parse_spec("https://gitlab.com/org/project").unwrap();
        assert_eq!(gl.browse_url("abc1234"), None, "gitlab.com");

        let self_hosted = parse_spec("https://gitlab.corp.example.com/org/project").unwrap();
        assert_eq!(
            self_hosted.browse_url("abc1234"),
            None,
            "self-hosted gitlab"
        );
    }

    // spec: HOOK-24
    #[test]
    fn browse_url_bitbucket_hosts_yield_none() {
        let bb = parse_spec("https://bitbucket.org/org/repo").unwrap();
        assert_eq!(bb.browse_url("abc1234"), None, "bitbucket.org");
    }

    // spec: HOOK-24
    #[test]
    fn browse_url_ssh_remote_yields_none() {
        let ssh = parse_spec("git@github.com:foo/bar.git").unwrap();
        assert_eq!(ssh.browse_url("abc1234"), None);
    }

    // spec: HOOK-24
    #[test]
    fn browse_url_local_path_yields_none() {
        let local = parse_spec("/home/user/dev/agents").unwrap();
        assert_eq!(local.browse_url("abc1234"), None);
        let file_url = parse_spec("file:///home/user/dev/agents").unwrap();
        assert_eq!(file_url.browse_url("abc1234"), None);
    }

    // spec: HOOK-24
    // End-to-end: a real github-shaped Source's derived browse_url, fed through
    // the real consent-disclosure builder, must render the exact commit-pinned
    // `Browse:` line. This closes the seam the isolated unit tests leave open:
    // browse_url derivation is tested against a Source, and disclosure_text is
    // tested against a hardcoded URL string, but nothing proves the value that
    // browse_url actually produces is the value the disclosure renders.
    #[test]
    fn browse_url_renders_pinned_tree_line_in_consent_disclosure() {
        let gh = parse_spec("foo/bar").unwrap();
        let commit = "abc1234";
        let url = gh.browse_url(commit);
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/foo/bar/tree/abc1234"),
            "precondition: github source derives a tree URL"
        );

        let text = crate::hook::disclosure_text(
            "github.com/foo/bar",
            "main",
            commit,
            "/home/user/.mind/sources/github.com/foo/bar",
            "make install",
            None,
            url.as_deref(),
        );
        assert!(
            text.contains("  Browse:    https://github.com/foo/bar/tree/abc1234\n"),
            "consent disclosure must render the derived commit-pinned browse line; got: {text}"
        );
    }

    // spec: HOOK-24
    // The mirror of the above for a source that yields no browse URL: a local
    // path's `None` must flow through the disclosure builder and suppress the
    // Browse line entirely (only the clone path is shown).
    #[test]
    fn browse_url_none_suppresses_browse_line_in_consent_disclosure() {
        let local = parse_spec("/home/user/dev/agents").unwrap();
        let url = local.browse_url("abc1234");
        assert_eq!(url, None, "precondition: local path derives no browse URL");

        let text = crate::hook::disclosure_text(
            "/home/user/dev/agents",
            "local",
            "abc1234",
            "/home/user/dev/agents",
            "make install",
            None,
            url.as_deref(),
        );
        assert!(
            !text.contains("Browse:"),
            "a None browse_url must suppress the Browse line end-to-end; got: {text}"
        );
    }

    #[test]
    fn pin_serde_round_trips() {
        // spec: STO-18
        // Each Pin variant must serialize to a tagged JSON object and deserialize
        // back losslessly.  Also verifies that a missing `pin` field (older
        // sources.json) deserializes as DefaultBranch.
        let cases = [
            (Pin::DefaultBranch, r#"{"kind":"default-branch"}"#),
            (
                Pin::FollowBranch("main".into()),
                r#"{"kind":"follow-branch","value":"main"}"#,
            ),
            (Pin::Tag("v1.0".into()), r#"{"kind":"tag","value":"v1.0"}"#),
            (
                Pin::Ref("abc1234".into()),
                r#"{"kind":"ref","value":"abc1234"}"#,
            ),
        ];
        for (pin, expected_json) in &cases {
            let json = serde_json::to_string(pin).unwrap();
            assert_eq!(json, *expected_json, "serialization mismatch for {pin:?}");
            let roundtripped: Pin = serde_json::from_str(&json).unwrap();
            assert_eq!(roundtripped, *pin, "round-trip failed for {pin:?}");
        }
        // Missing pin field in a Source's JSON -> DefaultBranch default.
        let src_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b"
        }"#;
        let src: Source = serde_json::from_str(src_json).unwrap();
        assert_eq!(
            src.pin,
            Pin::DefaultBranch,
            "absent pin should default to DefaultBranch"
        );
    }

    #[test]
    fn flat_skills_round_trips_and_defaults_false() {
        // spec: STO-44
        // The consumer `--flat-skills` override persists on the source and
        // round-trips; an older sources.json with no field deserializes as false.
        let mut s = parse_spec("acme/tools").unwrap();
        assert!(!s.flat_skills, "default must be false");
        s.flat_skills = true;
        let json = serde_json::to_string(&s).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert!(back.flat_skills, "flat_skills=true must round-trip");

        // Absent in an older sources.json => false.
        let legacy = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b"
        }"#;
        let src: Source = serde_json::from_str(legacy).unwrap();
        assert!(!src.flat_skills, "absent flat_skills must default to false");
    }

    #[test]
    fn install_hook_fields_round_trip_and_default_absent() {
        // spec: HOOK-31, HOOK-55
        // Older sources.json without any hook fields => legacy fields None, install_hooks empty.
        let src_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b"
        }"#;
        let src: Source = serde_json::from_str(src_json).unwrap();
        assert_eq!(
            src.install_hook, None,
            "absent install_hook should default to None"
        );
        assert_eq!(
            src.install_hook_commit, None,
            "absent install_hook_commit should default to None"
        );
        assert!(
            src.install_hooks.is_empty(),
            "absent install_hooks should default to empty"
        );

        // A source carrying the legacy fields can be deserialized (load-only path).
        // After calling migrate_legacy_hook the pair is folded into install_hooks
        // and the legacy fields are cleared (HOOK-55 migration).
        let mut s = parse_spec("acme/tools").unwrap();
        s.install_hook = Some("make install".into());
        s.install_hook_commit = Some("abc1234".into());
        s.migrate_legacy_hook();
        assert_eq!(s.install_hook, None, "legacy field cleared after migration");
        assert_eq!(
            s.install_hook_commit, None,
            "legacy commit field cleared after migration"
        );
        assert_eq!(s.install_hooks.len(), 1, "hook migrated into install_hooks");
        assert_eq!(s.install_hooks[0].command, "make install");
        assert_eq!(s.install_hooks[0].ran_at.as_deref(), Some("abc1234"));
    }

    // --- HOOK-55 tests ---

    #[test]
    fn recorded_hook_serde_round_trip_with_ran_at_some() {
        // spec: HOOK-55
        let hook = RecordedSourceHook::install("make install", Some("deadbeef".into()));
        let json = serde_json::to_string(&hook).unwrap();
        let back: RecordedSourceHook = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back, hook,
            "RecordedSourceHook with ran_at=Some did not round-trip"
        );
    }

    #[test]
    fn recorded_hook_serde_round_trip_with_ran_at_none() {
        // spec: HOOK-55
        let hook = RecordedSourceHook::install("make install", None);
        let json = serde_json::to_string(&hook).unwrap();
        // ran_at=None should be absent (default) in the emitted JSON.
        let back: RecordedSourceHook = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back, hook,
            "RecordedSourceHook with ran_at=None did not round-trip"
        );
    }

    /// An `install_hooks` entry written before the record carried an event has
    /// only `command` and `ran_at`. It must still load, and it must read as an
    /// INSTALL record: the update event did not exist when it was written, and
    /// silently reading it as some other event would either strand the hook or
    /// replay it under the wrong disclosure.
    // spec: HOOK-124
    #[test]
    fn an_old_format_record_with_no_event_loads_as_an_install_record() {
        let legacy = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b",
            "commit":"bbb",
            "install_hooks":[{"command":"make setup","ran_at":"aaa"}]
        }"#;
        let src: Source = serde_json::from_str(legacy).unwrap();
        assert_eq!(src.install_hooks.len(), 1, "the legacy entry must load");
        let rec = &src.install_hooks[0];
        assert_eq!(rec.event, None, "the stored event is absent");
        assert_eq!(
            rec.event(),
            RecordedEvent::Install,
            "an eventless record reads as an install record"
        );
        assert_eq!(rec.origin, None, "a legacy record has no origin");
        assert!(!rec.baseline, "a legacy record is a run, not a baseline");
        // It participates in the install-hook pending set exactly as before,
        // provided the clone still declares it.
        let declared = vec!["make setup".to_string()];
        assert_eq!(
            src.pending_install_hooks(Some("bbb"), &declared, true)
                .len(),
            1,
            "a legacy record that advanced past its run-commit is pending"
        );
    }

    #[test]
    fn install_hooks_vec_round_trips_on_source() {
        // spec: HOOK-55
        let mut s = parse_spec("acme/tools").unwrap();
        s.install_hooks = vec![
            RecordedSourceHook::install("make setup", Some("aaa".into())),
            RecordedSourceHook::install("make install", None),
        ];
        let json = serde_json::to_string(&s).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back.install_hooks.len(), 2);
        assert_eq!(back.install_hooks[0].command, "make setup");
        assert_eq!(back.install_hooks[0].ran_at.as_deref(), Some("aaa"));
        assert_eq!(back.install_hooks[1].command, "make install");
        assert_eq!(back.install_hooks[1].ran_at, None);
    }

    #[test]
    fn migrate_legacy_hook_with_commit() {
        // spec: HOOK-55
        // A legacy entry (install_hook + install_hook_commit) migrates into a
        // single RecordedHook with the right command and ran_at.
        let legacy_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b",
            "install_hook":"./setup.sh",
            "install_hook_commit":"cafebabe"
        }"#;
        let mut src: Source = serde_json::from_str(legacy_json).unwrap();
        // Legacy fields are present before migration.
        assert_eq!(src.install_hook.as_deref(), Some("./setup.sh"));
        assert_eq!(src.install_hook_commit.as_deref(), Some("cafebabe"));
        assert!(src.install_hooks.is_empty());

        src.migrate_legacy_hook();

        assert_eq!(src.install_hooks.len(), 1, "hook should have been migrated");
        assert_eq!(src.install_hooks[0].command, "./setup.sh");
        assert_eq!(src.install_hooks[0].ran_at.as_deref(), Some("cafebabe"));
        assert_eq!(src.install_hook, None, "legacy field should be cleared");
        assert_eq!(
            src.install_hook_commit, None,
            "legacy commit should be cleared"
        );

        // After migration the legacy fields must not appear in serialized JSON.
        let json = serde_json::to_string(&src).unwrap();
        assert!(
            !json.contains("install_hook_commit"),
            "legacy commit must not re-emit"
        );
        // install_hook key should not appear (it's None, skip_serializing_if applies).
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            v.get("install_hook").is_none(),
            "install_hook must not re-emit"
        );
    }

    #[test]
    fn migrate_legacy_hook_without_commit() {
        // spec: HOOK-55
        // A legacy entry with only install_hook (skipped run, no commit) migrates
        // with ran_at=None.
        let legacy_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b",
            "install_hook":"./setup.sh"
        }"#;
        let mut src: Source = serde_json::from_str(legacy_json).unwrap();
        src.migrate_legacy_hook();

        assert_eq!(src.install_hooks.len(), 1);
        assert_eq!(src.install_hooks[0].command, "./setup.sh");
        assert_eq!(
            src.install_hooks[0].ran_at, None,
            "skipped hook: ran_at should be None"
        );
        assert_eq!(src.install_hook, None);
    }

    #[test]
    fn migrate_legacy_hook_is_idempotent() {
        // spec: HOOK-55
        // Calling migrate_legacy_hook twice does not duplicate entries.
        let legacy_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b",
            "install_hook":"./setup.sh",
            "install_hook_commit":"deadbeef"
        }"#;
        let mut src: Source = serde_json::from_str(legacy_json).unwrap();
        src.migrate_legacy_hook();
        src.migrate_legacy_hook();
        assert_eq!(
            src.install_hooks.len(),
            1,
            "idempotent: should not duplicate"
        );
        assert_eq!(src.install_hooks[0].command, "./setup.sh");
    }

    #[test]
    fn migrate_legacy_hook_noop_when_install_hooks_already_populated() {
        // spec: HOOK-55
        // When install_hooks already has entries, migration must not add more,
        // even if legacy fields are also present.
        let mut src = parse_spec("acme/tools").unwrap();
        src.install_hooks = vec![RecordedSourceHook::install(
            "pre-existing",
            Some("aaa".into()),
        )];
        src.install_hook = Some("./old-hook.sh".into());
        src.install_hook_commit = Some("bbb".into());

        src.migrate_legacy_hook();

        assert_eq!(src.install_hooks.len(), 1, "must not add a second hook");
        assert_eq!(src.install_hooks[0].command, "pre-existing");
        assert_eq!(
            src.install_hook, None,
            "legacy field cleared even when skipped"
        );
        assert_eq!(
            src.install_hook_commit, None,
            "legacy commit cleared even when skipped"
        );
    }

    #[test]
    fn absent_install_hooks_defaults_to_empty() {
        // spec: HOOK-55
        // A sources.json entry with no hook fields at all deserializes with an
        // empty install_hooks vec (back-compat).
        let src_json = r#"{
            "name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b"
        }"#;
        let src: Source = serde_json::from_str(src_json).unwrap();
        assert!(
            src.install_hooks.is_empty(),
            "install_hooks should default to empty"
        );
    }

    #[test]
    fn pending_install_hooks_returns_unrun_and_advanced() {
        // spec: HOOK-55
        // pending_install_hooks returns entries whose ran_at differs from current.
        let mut src = parse_spec("acme/tools").unwrap();
        src.install_hooks = vec![
            // Never run (ran_at=None) -> pending regardless of current.
            RecordedSourceHook::install("hook-a", None),
            // Ran at "aaa", current is "bbb" -> advanced -> pending.
            RecordedSourceHook::install("hook-b", Some("aaa".into())),
            // Ran at "bbb", current is "bbb" -> up-to-date -> NOT pending.
            RecordedSourceHook::install("hook-c", Some("bbb".into())),
        ];

        let declared = vec![
            "hook-a".to_string(),
            "hook-b".to_string(),
            "hook-c".to_string(),
        ];
        let pending = src.pending_install_hooks(Some("bbb"), &declared, true);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].command, "hook-a");
        assert_eq!(pending[1].command, "hook-b");
    }

    #[test]
    fn pending_install_hooks_all_pending_when_no_current_commit() {
        // spec: HOOK-55
        // When current is None (commitless source), hooks with ran_at=None are
        // pending (a null run-commit must always be re-offered), and hooks with
        // ran_at=Some(_) are also pending (they differ from current=None).
        let mut src = parse_spec("acme/tools").unwrap();
        src.install_hooks = vec![
            RecordedSourceHook::install("hook-a", None),
            RecordedSourceHook::install("hook-b", Some("aaa".into())),
        ];
        let declared = vec!["hook-a".to_string(), "hook-b".to_string()];
        let pending = src.pending_install_hooks(None, &declared, true);
        // hook-a: ran_at=None -> is_none() -> always pending.
        // hook-b: ran_at=Some("aaa") != current=None -> pending.
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].command, "hook-a");
        assert_eq!(pending[1].command, "hook-b");
    }

    /// A recorded install hook the clone no longer declares is NOT re-offered:
    /// the source's manifest, not local state, decides what an `Event: install`
    /// disclosure may propose running. A consumer override (HOOK-56) and a
    /// curated entry (HOOK-127) have no declaration in the clone to match, so
    /// they survive; the curated one only while the DSC-60 gate holds.
    // spec: HOOK-55 HOOK-56 HOOK-127
    #[test]
    fn pending_install_hooks_drops_records_the_clone_no_longer_declares() {
        let mut src = parse_spec("acme/tools").unwrap();
        let mut curated = RecordedSourceHook::install("curated.sh", None);
        curated.origin = Some(HookOrigin::Curated);
        let mut overridden = RecordedSourceHook::install("mine.sh", None);
        overridden.origin = Some(HookOrigin::Override);
        src.install_hooks = vec![
            RecordedSourceHook::install("still-declared", None),
            RecordedSourceHook::install("withdrawn", None),
            curated,
            overridden,
        ];

        let declared = vec!["still-declared".to_string()];
        let pending: Vec<&str> = src
            .pending_install_hooks(Some("bbb"), &declared, true)
            .into_iter()
            .map(|h| h.command.as_str())
            .collect();
        assert_eq!(pending, vec!["still-declared", "curated.sh", "mine.sh"]);

        // The DSC-60 gate closed (the source now ships its own mind.toml): the
        // curated record stops applying, the override does not.
        let pending: Vec<&str> = src
            .pending_install_hooks(Some("bbb"), &declared, false)
            .into_iter()
            .map(|h| h.command.as_str())
            .collect();
        assert_eq!(pending, vec!["still-declared", "mine.sh"]);
    }

    /// The record is keyed by `(command, event)`: one command declared for both
    /// events is two records, and running one must not settle the other.
    // spec: HOOK-124
    #[test]
    fn a_command_recorded_for_two_events_keeps_two_independent_records() {
        let mut src = parse_spec("acme/tools").unwrap();
        let mut update = RecordedSourceHook::install("make setup", None);
        update.event = Some(RecordedEvent::Update);
        src.install_hooks = vec![
            RecordedSourceHook::install("make setup", Some("bbb".into())),
            update,
        ];

        let declared = vec!["make setup".to_string()];
        assert!(
            src.pending_install_hooks(Some("bbb"), &declared, true)
                .is_empty(),
            "the install record ran at the current commit, so nothing pends"
        );
        let update_rec = src
            .install_hooks
            .iter()
            .find(|h| h.is("make setup", RecordedEvent::Update))
            .expect("the update record is addressable by (command, event)");
        assert!(
            update_rec.pending(Some("bbb")),
            "the update record is untouched by the install record's run"
        );
    }

    /// `override_command` reads the consumer's `--install-hook` back off the
    /// source, which is what lets `upgrade` keep applying it (HOOK-56).
    // spec: HOOK-56
    #[test]
    fn override_command_reads_back_the_recorded_consumer_override() {
        let mut src = parse_spec("acme/tools").unwrap();
        assert_eq!(src.override_command(), None);
        let mut overridden = RecordedSourceHook::install("./mine.sh", None);
        overridden.origin = Some(HookOrigin::Override);
        src.install_hooks = vec![RecordedSourceHook::install("theirs.sh", None), overridden];
        assert_eq!(src.override_command(), Some("./mine.sh"));
    }

    #[test]
    fn origin_and_version_round_trip() {
        let mut s = parse_spec("acme/tools").unwrap();
        s.origin = Some(ManifestOrigin::ClaudePlugin);
        s.plugin_version = Some("1.2.3".into());
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains("\"claude-plugin\""),
            "serialized JSON must contain the kebab-case origin label"
        );
        assert!(
            json.contains("\"1.2.3\""),
            "serialized JSON must contain the plugin version"
        );
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back.origin, Some(ManifestOrigin::ClaudePlugin));
        assert_eq!(back.plugin_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn absent_origin_defaults_to_none_and_omits_keys() {
        // A legacy sources.json with no origin/plugin_version fields deserializes
        // with both as None.
        let legacy = r#"{"name":"local/a/b","url":"/a/b","host":"local","owner":"a","repo":"b"}"#;
        let src: Source = serde_json::from_str(legacy).unwrap();
        assert_eq!(src.origin, None, "absent origin must default to None");
        assert_eq!(
            src.plugin_version, None,
            "absent plugin_version must default to None"
        );

        // A freshly constructed source (both fields None) must not emit the keys
        // at all (skip_serializing_if = "Option::is_none").
        let fresh = parse_spec("acme/tools").unwrap();
        let json = serde_json::to_string(&fresh).unwrap();
        assert!(
            !json.contains("\"origin\""),
            "origin must be absent from JSON when None"
        );
        assert!(
            !json.contains("\"plugin_version\""),
            "plugin_version must be absent from JSON when None"
        );
    }

    // ---- STO-50/STO-51: schema version in sources.json ----------------------

    use std::sync::atomic::{AtomicU32, Ordering};
    static SRC_N: AtomicU32 = AtomicU32::new(0);

    // STO-31: a malformed registry is a `Json` error naming the file, not a
    // silent empty registry (which would look like "no sources melded" and
    // invite an overwrite of the real file). Normal tests only ever write valid
    // JSON, so this branch needs a hand-built document to reach.
    // spec: STO-31
    #[test]
    fn malformed_sources_json_is_a_json_error_naming_the_file() {
        let (base, paths) = tmp_paths_src();
        std::fs::write(base.join("sources.json"), "{ not json at all").unwrap();
        match Registry::load(&paths) {
            Err(MindError::Json { what, .. }) => assert_eq!(what, "sources.json"),
            other => panic!("expected a Json error naming sources.json, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    fn tmp_paths_src() -> (std::path::PathBuf, Paths) {
        let n = SRC_N.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-sources-ver-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths {
            mind_home: base.clone(),
            claude_home: base.join("claude"),
        };
        (base, paths)
    }

    #[test]
    fn registry_missing_version_is_treated_as_one() {
        // spec: STO-50 -- a sources.json with no "version" field must load
        // successfully (treated as version 1 for backward compatibility).
        let (base, paths) = tmp_paths_src();
        std::fs::write(base.join("sources.json"), r#"{"sources":[]}"#).unwrap();
        let r = Registry::load(&paths).expect("must load without version field");
        assert!(r.sources.is_empty(), "sources must be empty: {r:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn registry_version_one_loads_ok() {
        // spec: STO-50 -- version 1 is the maximum supported version.
        let (base, paths) = tmp_paths_src();
        std::fs::write(base.join("sources.json"), r#"{"version":1,"sources":[]}"#).unwrap();
        let r = Registry::load(&paths).expect("version 1 must load");
        assert!(
            r.sources.is_empty(),
            "version 1 must load successfully: {r:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn registry_too_new_version_is_state_too_new_error() {
        // spec: STO-50 STO-51 -- a version > 1 must be a StateTooNew error
        // naming sources.json, the found version, and the supported version.
        let (base, paths) = tmp_paths_src();
        std::fs::write(base.join("sources.json"), r#"{"version":42,"sources":[]}"#).unwrap();
        let err = Registry::load(&paths).unwrap_err();
        match err {
            MindError::StateTooNew {
                what,
                found,
                supported,
            } => {
                assert_eq!(what, "sources.json");
                assert_eq!(found, 42);
                assert_eq!(supported, REGISTRY_VERSION);
            }
            other => panic!("expected StateTooNew, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- Registry::load revalidation (STO-68) ----

    // spec: STO-68
    #[test]
    fn revalidate_sources_drops_offending_entry_and_warns_naming_the_part() {
        let mut bad = parse_spec("acme/victim").unwrap();
        bad.host = "..".to_string();
        bad.owner = "..".to_string();
        bad.repo = "victim".to_string();
        bad.name = "../../victim".to_string();
        let name_before = bad.name.clone();

        let mut warnings: Vec<String> = Vec::new();
        let kept = revalidate_sources(vec![bad], |msg| warnings.push(msg.to_string()));

        assert!(kept.is_empty(), "the offending entry must be dropped");
        assert_eq!(warnings.len(), 1, "exactly one warning must be emitted");
        assert!(
            warnings[0].contains("sources.json"),
            "warning must name sources.json: {}",
            warnings[0]
        );
        assert!(
            warnings[0].contains(&name_before),
            "warning must name the entry: {}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("host"),
            "warning must name the offending part (host, checked first): {}",
            warnings[0]
        );
    }

    // spec: STO-68
    #[test]
    fn revalidate_sources_keeps_valid_entries_and_warns_for_none() {
        let good = parse_spec("acme/agents").unwrap();
        let mut warnings: Vec<String> = Vec::new();
        let kept = revalidate_sources(vec![good], |msg| warnings.push(msg.to_string()));
        assert_eq!(kept.len(), 1, "a valid entry must be kept");
        assert!(warnings.is_empty(), "no warning for a valid entry");
    }

    // spec: STO-68
    #[test]
    fn revalidate_rejects_unsafe_as_alias_and_pin_ref() {
        // An as_alias that would fail validate_prefix (path traversal).
        let mut aliased = parse_spec("acme/agents").unwrap();
        aliased.as_alias = Some("../evil".to_string());
        assert!(
            aliased.revalidate().is_err(),
            "an unsafe as_alias must fail revalidation"
        );

        // A pin ref value that would fail git::validate_ref_value (a leading
        // dash looks like a git option).
        let mut pinned = parse_spec("acme/agents").unwrap();
        pinned.pin = Pin::FollowBranch("-evil".to_string());
        assert!(
            pinned.revalidate().is_err(),
            "an unsafe pin ref value must fail revalidation"
        );

        // A DefaultBranch pin (no value) always passes the pin check.
        let default_pin = parse_spec("acme/agents").unwrap();
        assert!(default_pin.revalidate().is_ok());
    }

    // spec: STO-68
    // The `Registry::load` end-to-end path: a hand-written sources.json
    // carrying an entry that predates the CLI-204 tightening (a `host` of
    // `".."`) must load successfully with that entry dropped, not fail the
    // whole load and not carry the offending entry through.
    #[test]
    fn registry_load_drops_a_pre_existing_unsafe_entry() {
        let (base, paths) = tmp_paths_src();
        std::fs::write(
            base.join("sources.json"),
            r#"{"version":1,"sources":[
                {"name":"../../victim","url":"https://x/y/victim","host":"..","owner":"..","repo":"victim"},
                {"name":"github.com/acme/agents","url":"https://github.com/acme/agents","host":"github.com","owner":"acme","repo":"agents"}
            ]}"#,
        )
        .unwrap();
        let reg = Registry::load(&paths).expect("load must succeed despite the bad entry");
        assert_eq!(
            reg.sources.len(),
            1,
            "the unsafe entry must be dropped, the safe one kept: {reg:?}"
        );
        assert_eq!(reg.sources[0].name, "github.com/acme/agents");
        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: STO-68
    // STO-68's own headline example is `repo: ".."` (what 0.21.0's parser let
    // through), and the warning must name the part that actually failed. The
    // existing coverage only ever fails on `host`, which is the FIRST part
    // checked, so it cannot tell "names the offending part" apart from "always
    // says host". Each case here makes exactly ONE part bad.
    #[test]
    fn revalidate_names_whichever_single_part_is_unsafe() {
        for (part, mutate) in [
            (
                "host",
                (|s: &mut Source| s.host = "..".to_string()) as fn(&mut Source),
            ),
            ("owner", |s: &mut Source| s.owner = "ow#ner".to_string()),
            ("repo", |s: &mut Source| s.repo = "..".to_string()),
            ("as_alias", |s: &mut Source| {
                s.as_alias = Some("../evil".to_string())
            }),
            ("pin", |s: &mut Source| {
                s.pin = Pin::Tag("--upload-pack=evil".to_string())
            }),
        ] {
            let mut src = parse_spec("acme/agents").unwrap();
            mutate(&mut src);
            let mut warnings: Vec<String> = Vec::new();
            let kept = revalidate_sources(vec![src], |m| warnings.push(m.to_string()));
            assert!(kept.is_empty(), "a bad {part} must drop the entry");
            assert_eq!(warnings.len(), 1, "one warning per dropped entry ({part})");
            assert!(
                warnings[0].contains(&format!("its {part} part")),
                "the warning must name the {part} part, got: {}",
                warnings[0]
            );
        }
    }

    // spec: STO-68
    // Two bad entries drop independently and warn once EACH (the spec says
    // "each drop prints one warning"), and a good entry between them survives.
    #[test]
    fn revalidate_sources_warns_once_per_dropped_entry_and_keeps_the_survivors() {
        let mut bad_a = parse_spec("acme/agents").unwrap();
        bad_a.repo = "..".to_string();
        bad_a.name = "a/bad".to_string();
        let good = parse_spec("acme/good").unwrap();
        let mut bad_b = parse_spec("acme/agents").unwrap();
        bad_b.owner = "..".to_string();
        bad_b.name = "b/bad".to_string();

        let mut warnings: Vec<String> = Vec::new();
        let kept = revalidate_sources(vec![bad_a, good, bad_b], |m| warnings.push(m.to_string()));
        assert_eq!(kept.len(), 1, "only the valid entry survives");
        assert_eq!(kept[0].name, "github.com/acme/good");
        assert_eq!(warnings.len(), 2, "one warning per drop: {warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("a/bad")));
        assert!(warnings.iter().any(|w| w.contains("b/bad")));
    }

    // spec: STO-68
    // "The drop is not written back immediately": `Registry::load` must not
    // rewrite sources.json as a side effect of dropping. Losing an entry is
    // recoverable only while the file on disk still holds it, so a read-only
    // verb must leave the file byte-identical.
    #[test]
    fn registry_load_does_not_rewrite_sources_json_when_it_drops_an_entry() {
        let (base, paths) = tmp_paths_src();
        let file = base.join("sources.json");
        let raw = r#"{"version":1,"sources":[
            {"name":"../../victim","url":"https://x/y/victim","host":"..","owner":"..","repo":"victim"}
        ]}"#;
        std::fs::write(&file, raw).unwrap();
        let reg = Registry::load(&paths).expect("load must succeed");
        assert!(reg.sources.is_empty(), "the entry is dropped in memory");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            raw,
            "load must not write the drop back to disk; it becomes permanent only on the next save"
        );
        // ...and it does become permanent on the next `save`.
        reg.save(&paths).expect("save must succeed");
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(
            !after.contains("victim"),
            "the drop must be persisted by the next save: {after}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn compute_name_composes_item_link_and_alias() {
        // spec: STO-58 LNK-4 -- `@<alias>` is always the trailing segment and
        // composes with an item-link `#<path>` as `host/owner/repo#<path>@<alias>`.
        let mut s = parse_spec("github:acme/agents").unwrap();
        assert_eq!(s.compute_name(), "github.com/acme/agents");
        s.apply_alias(Some("jk".into()));
        assert_eq!(s.name, "github.com/acme/agents@jk");
        // An empty alias is the no-prefix override: it adds no `@` suffix.
        s.apply_alias(Some(String::new()));
        assert_eq!(s.name, "github.com/acme/agents");
        assert_eq!(s.alias.as_deref(), Some(""));

        let mut link = parse_spec("https://github.com/acme/agents/tree/main/skills/foo").unwrap();
        assert_eq!(link.name, "github.com/acme/agents#skills/foo");
        link.apply_alias(Some("jk".into()));
        assert_eq!(link.name, "github.com/acme/agents#skills/foo@jk");
        assert_eq!(link.base_identity(), "github.com/acme/agents");
    }

    #[test]
    fn clone_dir_is_per_instance_for_an_alias() {
        // spec: STO-59 -- an aliased instance clones under `<repo>@<alias>`; an
        // unaliased source (and an empty-alias override) is unchanged at `<repo>`.
        let (base, paths) = tmp_paths_src();
        let mut s = parse_spec("github:acme/agents").unwrap();
        let bare = s.clone_dir(&paths);
        assert!(bare.ends_with("github.com/acme/agents"));
        s.apply_alias(Some("jk".into()));
        assert!(s.clone_dir(&paths).ends_with("github.com/acme/agents@jk"));
        s.apply_alias(Some(String::new()));
        assert_eq!(s.clone_dir(&paths), bare, "empty alias keeps the bare path");
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- per-instance clone dir for item links (STO-70) ----

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_bare_shape_unchanged() {
        assert_eq!(clone_dir_leaf("agents", None, None), "agents");
        // An empty alias is the explicit no-prefix override; same as None.
        assert_eq!(clone_dir_leaf("agents", None, Some("")), "agents");
    }

    // spec: STO-70 STO-59
    #[test]
    fn clone_dir_leaf_alias_only_shape_unchanged() {
        assert_eq!(clone_dir_leaf("agents", None, Some("jk")), "agents@jk");
    }

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_item_path_only_shape() {
        assert_eq!(
            clone_dir_leaf("agents", Some("skills/foo"), None),
            "agents#skills%2Ffoo"
        );
    }

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_item_path_and_alias_shape() {
        assert_eq!(
            clone_dir_leaf("agents", Some("skills/foo"), Some("jk")),
            "agents#skills%2Ffoo@jk"
        );
    }

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_distinguishes_two_item_paths_from_each_other_and_from_bare() {
        // This is the exact collision C31/STO-70 closes: before the fix these
        // three all resolved to the identical `agents` leaf.
        let a = clone_dir_leaf("agents", Some("skills/a"), None);
        let b = clone_dir_leaf("agents", Some("skills/b"), None);
        let bare = clone_dir_leaf("agents", None, None);
        assert_ne!(a, b, "two distinct item_paths must not share a leaf");
        assert_ne!(
            a, bare,
            "an item-link leaf must not collide with the bare leaf"
        );
        assert_ne!(
            b, bare,
            "an item-link leaf must not collide with the bare leaf"
        );
    }

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_percent_encodes_slash_and_percent_injectively() {
        // `/` -> `%2F`.
        assert_eq!(
            clone_dir_leaf("r", Some("a/b"), None),
            "r#a%2Fb",
            "slash must be percent-encoded"
        );
        // A literal `%` in the (already-restricted) item_path is encoded first,
        // so it cannot be misread as the start of one of our own escapes.
        assert_eq!(
            clone_dir_leaf("r", Some("a%b"), None),
            "r#a%25b",
            "percent must itself be percent-encoded"
        );
        // The leaf never contains a bare `/`, keeping it a single path
        // component (the whole point of encoding it).
        let leaf = clone_dir_leaf("r", Some("a/b/c"), None);
        assert!(!leaf.contains('/'), "encoded leaf must have no '/': {leaf}");
    }

    // spec: STO-70
    #[test]
    fn clone_dir_leaf_falls_back_to_a_hash_past_the_length_threshold() {
        let short_path = "skills/foo";
        let short_leaf = clone_dir_leaf("agents", Some(short_path), None);
        assert!(
            short_leaf.contains(short_path.replace('/', "%2F").as_str()),
            "a short item_path must stay readably encoded: {short_leaf}"
        );

        // A very long item_path pushes the leaf past 120 bytes.
        let long_path = format!("skills/{}", "x".repeat(200));
        let long_leaf = clone_dir_leaf("agents", Some(&long_path), None);
        assert!(
            long_leaf.len() <= 120,
            "an over-length leaf must fall back to a short hash: {} bytes",
            long_leaf.len()
        );
        assert!(
            !long_leaf.contains("xxxx"),
            "the fallback must not embed the readable path: {long_leaf}"
        );
        assert!(
            long_leaf.starts_with("agents#"),
            "the fallback must keep the repo#-prefix shape: {long_leaf}"
        );

        // The hash fallback is deterministic per item_path.
        let long_leaf_again = clone_dir_leaf("agents", Some(&long_path), None);
        assert_eq!(
            long_leaf, long_leaf_again,
            "the hash fallback must be stable for the same item_path"
        );

        // And an alias still composes onto the hash fallback.
        let long_leaf_aliased = clone_dir_leaf("agents", Some(&long_path), Some("jk"));
        assert!(
            long_leaf_aliased.ends_with("@jk"),
            "an alias must still append onto the hash fallback: {long_leaf_aliased}"
        );
    }

    // spec: STO-70
    // The exact threshold. "If the encoded leaf WOULD EXCEED 120 bytes" -- so
    // 120 stays readable and 121 flips to the hash. Only a 200-char path (far
    // past the edge) was previously driven, which passes under an off-by-one
    // in either direction.
    #[test]
    fn clone_dir_leaf_length_threshold_is_exclusive_at_120_bytes() {
        // Leaf shape is `r#<encoded>`: 1 (repo) + 1 ('#') + encoded.len().
        // With an item_path of plain ASCII (no '/' or '%'), encoded.len() ==
        // item_path.len(), so a path of N chars yields a leaf of N + 2.
        let at_limit = "x".repeat(118);
        let leaf = clone_dir_leaf("r", Some(&at_limit), None);
        assert_eq!(leaf.len(), 120);
        assert_eq!(
            leaf,
            format!("r#{at_limit}"),
            "a leaf of exactly 120 bytes must stay readable, not hash"
        );

        let over_limit = "x".repeat(119);
        let leaf = clone_dir_leaf("r", Some(&over_limit), None);
        assert_eq!(
            leaf,
            format!("r#{}", crate::hash::hash_str(&over_limit)),
            "121 bytes exceeds the limit and must fall back to the hash"
        );
    }

    // spec: STO-70
    // "This is injective (so two distinct item_paths can never collide on the
    // same encoded segment)". The dangerous pair is a path containing a literal
    // `%2F` versus one containing a real `/`: encoding `%` FIRST is what keeps
    // them apart. Encoding `/` first would map both to `a%2Fb`, so two distinct
    // item-link instances would share one checkout again -- exactly the C31 bug
    // STO-70 exists to close.
    #[test]
    fn clone_dir_leaf_encoding_is_injective_for_the_percent_slash_collision_pair() {
        let with_slash = clone_dir_leaf("r", Some("a/b"), None);
        let with_literal_escape = clone_dir_leaf("r", Some("a%2Fb"), None);
        assert_eq!(with_slash, "r#a%2Fb");
        assert_eq!(with_literal_escape, "r#a%252Fb");
        assert_ne!(
            with_slash, with_literal_escape,
            "'a/b' and 'a%2Fb' are distinct item paths and must not share a leaf"
        );
    }

    // spec: STO-70
    // End-to-end through `Source::clone_dir`, not just the pure leaf helper:
    // two item-link instances from the SAME repo (as would be created by
    // `parse_spec` on two different `tree/<ref>/<path>` URLs) resolve to
    // distinct clone paths, and neither collides with a plain meld of the
    // repo.
    #[test]
    fn clone_dir_end_to_end_distinguishes_link_instances_and_plain_meld() {
        let (base, paths) = tmp_paths_src();
        let link_a = parse_spec("https://github.com/o/r/tree/main/skills/a").unwrap();
        let link_b = parse_spec("https://github.com/o/r/tree/main/skills/b").unwrap();
        let plain = parse_spec("https://github.com/o/r").unwrap();

        let dir_a = link_a.clone_dir(&paths);
        let dir_b = link_b.clone_dir(&paths);
        let dir_plain = plain.clone_dir(&paths);

        assert_ne!(
            dir_a, dir_b,
            "two link instances must not share a clone dir"
        );
        assert_ne!(
            dir_a, dir_plain,
            "a link instance must not share a clone dir with a plain meld"
        );
        assert_ne!(
            dir_b, dir_plain,
            "a link instance must not share a clone dir with a plain meld"
        );
        assert!(
            dir_plain.ends_with("github.com/o/r"),
            "plain meld unaffected: {dir_plain:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn legacy_display_alias_keeps_bare_identity_and_clone() {
        // spec: STO-58 STO-59 -- a source registered before `as_alias` existed has
        // only the display `alias` and no `as_alias`, so it is NOT an identity
        // instance: its `name` stays bare and its clone stays at the bare path (no
        // migration, no relocation), preserving pre-feature behavior and keeping
        // its manifest `source` linkage intact.
        let (base, paths) = tmp_paths_src();
        std::fs::write(
            base.join("sources.json"),
            r#"{"version":1,"sources":[{"name":"github.com/acme/agents","url":"https://github.com/acme/agents","host":"github.com","owner":"acme","repo":"agents","alias":"jk"}]}"#,
        )
        .unwrap();
        let bare = paths
            .sources_dir()
            .join("github.com")
            .join("acme")
            .join("agents");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::write(bare.join("marker"), "x").unwrap();

        let reg = Registry::load(&paths).expect("load");
        let s = &reg.sources[0];
        assert_eq!(s.name, "github.com/acme/agents", "identity stays bare");
        assert_eq!(s.alias.as_deref(), Some("jk"), "display prefix preserved");
        assert_eq!(s.as_alias, None, "no identity alias for a legacy source");
        assert_eq!(
            s.clone_dir(&paths),
            bare,
            "clone stays at the bare path (no relocation)"
        );
        assert!(bare.join("marker").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- STO-73: Registry::load migrates a relative local url ----

    // spec: STO-73
    #[test]
    fn migrate_relative_local_url_rewrites_only_when_it_resolves() {
        let (base, _paths) = tmp_paths_src();
        let real_dir = base.join("real");
        std::fs::create_dir_all(&real_dir).unwrap();

        // A relative url that DOES resolve to a real directory gets rewritten.
        let mut resolves = parse_spec(&real_dir.to_string_lossy()).unwrap();
        let name_before = resolves.name.clone();
        resolves.url = "real".to_string();
        let rewritten = migrate_relative_local_url(&base, &mut resolves);
        assert!(rewritten, "a resolving relative url must be rewritten");
        assert_eq!(resolves.url, real_dir.to_string_lossy());
        assert_eq!(
            resolves.name, name_before,
            "the migration must never rewrite name, only url"
        );

        // A relative url that does NOT resolve is left exactly as recorded.
        let mut ghost = parse_spec("/home/user/dev/agents").unwrap();
        ghost.url = "does-not-exist".to_string();
        let name_before = ghost.name.clone();
        let rewritten = migrate_relative_local_url(&base, &mut ghost);
        assert!(
            !rewritten,
            "a non-resolving relative url must not be rewritten"
        );
        assert_eq!(ghost.url, "does-not-exist");
        assert_eq!(ghost.name, name_before, "name must never be rewritten");

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: STO-73
    #[test]
    fn migrate_relative_local_url_is_a_noop_for_remote_and_absolute_sources() {
        let (base, _paths) = tmp_paths_src();

        // A remote source is never touched (not local).
        let mut remote = parse_spec("acme/agents").unwrap();
        let remote_url_before = remote.url.clone();
        assert!(!migrate_relative_local_url(&base, &mut remote));
        assert_eq!(remote.url, remote_url_before);

        // An already-absolute local url is left alone (nothing to migrate).
        let mut abs = parse_spec("/already/absolute").unwrap();
        let abs_url_before = abs.url.clone();
        assert!(!migrate_relative_local_url(&base, &mut abs));
        assert_eq!(abs.url, abs_url_before);

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: STO-73
    // "ONLY when the resolved absolute path currently exists as a DIRECTORY".
    // The existing coverage tests "does not exist"; this tests "exists but is
    // not a directory", the case a plain `.exists()` check would get wrong. A
    // file is not a source working tree, so migrating to it would replace a
    // wrong-but-honest recorded path with a wrong-and-absolutized one, exactly
    // what STO-73 says not to do.
    #[test]
    fn migrate_relative_local_url_does_not_rewrite_when_the_target_is_a_file() {
        let (base, _paths) = tmp_paths_src();
        std::fs::write(base.join("not-a-dir"), "x").unwrap();

        let mut src = parse_spec("/home/user/dev/agents").unwrap();
        src.url = "not-a-dir".to_string();
        assert!(
            !migrate_relative_local_url(&base, &mut src),
            "a relative url resolving to a FILE must not be migrated"
        );
        assert_eq!(src.url, "not-a-dir", "the recorded url must be left as-is");

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: STO-73
    // End-to-end through `Registry::load`: a hand-written sources.json carrying
    // a local source with a RELATIVE url that resolves against the real
    // process cwd is rewritten to the absolute form in memory. Constructs the
    // resolving relative path as `../<unique>` against the real cwd so the
    // fixture directory lands next to (not inside) the crate's own working
    // tree, and cleans it up unconditionally.
    #[test]
    fn registry_load_migrates_a_resolving_relative_local_url() {
        let cwd = std::env::current_dir().expect("must read cwd");
        let unique = format!(
            "mind-sto73-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let target_abs = cwd.parent().expect("cwd must have a parent").join(&unique);
        std::fs::create_dir_all(&target_abs).unwrap();
        let relative = format!("../{unique}");

        let (base, paths) = tmp_paths_src();
        let sources_json = format!(
            r#"{{"version":1,"sources":[{{"name":"local/a/b","url":{:?},"host":"local","owner":"a","repo":"b"}}]}}"#,
            relative
        );
        std::fs::write(base.join("sources.json"), sources_json).unwrap();

        let reg = Registry::load(&paths).expect("load must succeed");
        assert_eq!(reg.sources.len(), 1);
        assert_eq!(
            reg.sources[0].url,
            target_abs.to_string_lossy(),
            "a resolving relative url must be migrated to its absolute form"
        );
        assert_eq!(
            reg.sources[0].name, "local/a/b",
            "name (identity) must never be rewritten by the migration"
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&target_abs);
    }

    // ---- CLI-214/CLI-215: shared local-directory shadow detection ----

    // spec: CLI-214 CLI-215
    #[test]
    fn resolve_local_dir_finds_an_existing_relative_directory_against_base() {
        let (base, _paths) = tmp_paths_src();
        let nested = base.join("skills").join("greet");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            resolve_local_dir(&base, "skills/greet"),
            Some(nested.clone())
        );
        assert_eq!(
            resolve_local_dir(&base, "skills/does-not-exist"),
            None,
            "a non-existent path must not be reported as a directory"
        );
        // An absolute spec is checked as-is, not joined onto base.
        assert_eq!(
            resolve_local_dir(&base, &nested.to_string_lossy()),
            Some(nested.clone())
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    // spec: CLI-215
    #[test]
    fn local_dir_shadow_is_true_only_when_resolve_local_dir_finds_something() {
        let (base, _paths) = tmp_paths_src();
        let nested = base.join("owner").join("repo");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(local_dir_shadow(&base, "owner/repo"));
        assert!(!local_dir_shadow(&base, "owner/nonexistent"));

        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- CLI-216: the quiet parse is the SAME parse ----

    /// Every repo spec in the product passes through `parse_spec`, and CLI-216
    /// split it into `parse_spec` / `parse_spec_quiet` over a shared
    /// `parse_spec_inner(spec, note)`. The split is only safe if `note` changes
    /// NOTHING but the advisory print: a divergence would silently give two
    /// callers different identities, clone URLs, pins, or item paths for the
    /// same string, and no caller would notice until a source was registered
    /// under the wrong name.
    ///
    /// So walk the whole grammar -- every branch of `parse_spec_inner`, plus
    /// the rejection paths -- and require the two entry points to agree
    /// structurally (`Debug` covers every field of `Source`, including the ones
    /// no other test reads).
    // spec: CLI-216
    #[test]
    fn parse_spec_quiet_agrees_with_parse_spec_across_the_whole_grammar() {
        // The `owner/repo` branch that carries the CLI-215 note is the ONLY
        // branch where the two differ at all, so it must be exercised. The
        // crate root is the cwd for a unit test, and `src/tui` is a real
        // directory there, so `src/tui` is an `owner/repo`-shaped spec that
        // shadows a local directory: the note fires for `parse_spec` and not
        // for `parse_spec_quiet`, and the returned value must still match.
        let cwd = std::env::current_dir().expect("cwd");
        assert!(
            local_dir_shadow(&cwd, "src/tui"),
            "fixture rotted: `src/tui` must be a directory relative to the test \
             cwd ({cwd:?}) for the CLI-215 note branch to be exercised here"
        );

        let specs = [
            // The shadowing bare form: the one branch that differs.
            "src/tui",
            // Bare / shorthand remote forms.
            "owner/repo",
            "github:owner/repo",
            "  owner/repo  ",
            "owner/repo.git",
            // URL forms, including a non-github host and the `.git` suffix.
            "https://github.com/owner/repo",
            "https://github.com/owner/repo.git",
            "https://gitlab.com/owner/repo",
            "http://example.com/owner/repo",
            "ssh://git@github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
            // SSH scp-like form.
            "git@github.com:owner/repo",
            "git@github.com:owner/repo.git",
            // Local path forms.
            "/abs/path/repo",
            "/abs/path/repo/",
            "./rel/repo",
            "../rel/repo",
            "file:///abs/path/repo",
            // Item links (LNK-1), remote and local, tree and blob, and the
            // GitLab `/-/` variant.
            "https://github.com/owner/repo/tree/main/skills/greet",
            "https://github.com/owner/repo/blob/abc1234/skills/greet/SKILL.md",
            "https://gitlab.com/owner/repo/-/tree/main/skills/greet",
            "file:///abs/path/repo/tree/main/skills/greet",
            // Rejections: the error must be identical too, not merely "an error".
            "",
            "   ",
            "notaspec",
            "owner/repo/extra",
            "https://github.com/owner",
            "https://github.com",
            "git@github.com",
            "https://user:pw@github.com/owner/repo",
            "ssh://@github.com/owner/repo",
        ];

        for spec in specs {
            let loud = parse_spec(spec);
            let quiet = parse_spec_quiet(spec);
            match (&loud, &quiet) {
                (Ok(a), Ok(b)) => assert_eq!(
                    format!("{a:?}"),
                    format!("{b:?}"),
                    "parse_spec and parse_spec_quiet must return the SAME Source \
                     for {spec:?}; the `note` flag may only change what is printed"
                ),
                (Err(a), Err(b)) => assert_eq!(
                    format!("{a:?}"),
                    format!("{b:?}"),
                    "parse_spec and parse_spec_quiet must reject {spec:?} with the \
                     SAME error"
                ),
                _ => panic!(
                    "parse_spec and parse_spec_quiet disagree on whether {spec:?} \
                     parses at all: loud={loud:?} quiet={quiet:?}"
                ),
            }
        }
    }

    /// The table above is only meaningful if it actually reaches the branches.
    /// Pin the shapes it is asserted to cover, so a future grammar change that
    /// makes one of them fall through to `InvalidRepoSpec` is visible here
    /// rather than silently weakening the equivalence check.
    // spec: CLI-216
    #[test]
    fn the_quiet_equivalence_table_reaches_every_parse_branch() {
        let host_of = |spec: &str| {
            parse_spec_quiet(spec)
                .unwrap_or_else(|e| panic!("{spec:?} must parse: {e:?}"))
                .host
        };
        assert_eq!(host_of("owner/repo"), "github.com");
        assert_eq!(host_of("github:owner/repo"), "github.com");
        assert_eq!(host_of("https://gitlab.com/o/r"), "gitlab.com");
        assert_eq!(host_of("git@github.com:o/r"), "github.com");
        assert_eq!(host_of("ssh://git@github.com/o/r"), "github.com");
        assert_eq!(host_of("/abs/path/repo"), "local");
        assert_eq!(host_of("file:///abs/path/repo"), "local");
        assert!(
            parse_spec_quiet("https://github.com/o/r/tree/main/skills/greet")
                .expect("remote item link must parse")
                .item_path
                .is_some(),
            "the remote item-link branch must be reachable from the table"
        );
        assert!(
            parse_spec_quiet("file:///abs/path/repo/tree/main/skills/greet")
                .expect("local item link must parse")
                .item_path
                .is_some(),
            "the local item-link branch must be reachable from the table"
        );
        assert!(
            parse_spec_quiet("notaspec").is_err(),
            "the rejection branch must be reachable from the table"
        );
    }
}
