//! Melded sources: the GitHub (or arbitrary git) repos `mind` pulls items from.

use std::path::PathBuf;

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

/// A recorded install hook for a source (HOOK-55).
///
/// Tracks the command and the commit at which it last ran. When `ran_at` is
/// `None` the hook was recorded but skipped, so `upgrade` should re-offer it.
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
    /// Consumer-chosen namespace from `meld --as`, if any. Overrides the repo's
    /// own `[source].prefix`. Persisted (never changed by `sync`).
    #[serde(default)]
    pub alias: Option<String>,
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
    /// The item path of an item-link source instance (LNK-4): the skill
    /// directory's repo-root-relative path parsed from a deep tree/blob URL.
    /// When set, the source's identity (`name`) carries a `#<path>` suffix and
    /// its catalog is exactly that one skill (LNK-7). None for an ordinary
    /// source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_path: Option<String>,
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
    /// The install hooks recorded for this source (HOOK-55). Supersedes the
    /// legacy single `install_hook`/`install_hook_commit` pair, which is
    /// migrated into this on load. Each entry records the command and the commit
    /// it last ran at.
    #[serde(default)]
    pub install_hooks: Vec<RecordedHook>,
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
    /// suffix. Equal to `name` for an ordinary source. This is what managed
    /// policy allowlists match against (LNK-11).
    pub fn base_identity(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
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
        paths
            .sources_dir()
            .join(&self.host)
            .join(&self.owner)
            .join(&self.repo)
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
            self.install_hooks.push(RecordedHook {
                command: cmd,
                ran_at,
            });
        }
        // Clear legacy fields regardless so they stop being emitted.
        self.install_hook = None;
        self.install_hook_commit = None;
    }

    /// The recorded install hooks whose last-run commit differs from `current`
    /// (i.e. never run, or the source has advanced) - the ones `upgrade` should
    /// re-offer (HOOK-55). `current` is the source's current commit.
    pub fn pending_install_hooks(&self, current: Option<&str>) -> Vec<&RecordedHook> {
        self.install_hooks
            .iter()
            .filter(|h| h.ran_at.is_none() || h.ran_at.as_deref() != current)
            .collect()
    }
}

/// Parse a user-supplied repo spec into a [`Source`] (without touching disk).
///
/// Accepts:
/// - `owner/repo`                       -> github.com
/// - `github:owner/repo`                -> github.com
/// - `https://github.com/owner/repo`    -> as given
/// - `git@github.com:owner/repo.git`    -> ssh form
pub fn parse_spec(spec: &str) -> Result<Source> {
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
            let mut comps = repo_part.trim_end_matches('/').rsplit('/');
            let repo_raw = comps.next().filter(|s| !s.is_empty()).ok_or_else(invalid)?;
            let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
            let owner = comps
                .next()
                .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                .unwrap_or("local");
            let mut source = make_source("local", owner, repo, repo_part.to_string());
            // spec: LNK-4 -- extended identity; the pin makes this a cloned
            // snapshot (never a linked working tree), so lifecycle matches a
            // remote link instance.
            source.name = format!("{}#{item_path}", source.name);
            source.item_path = Some(item_path);
            source.pin = pin;
            return Ok(source);
        }
        let mut comps = path.trim_end_matches('/').rsplit('/');
        let repo_raw = comps.next().filter(|s| !s.is_empty()).ok_or_else(invalid)?;
        let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
        let owner = comps
            .next()
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .unwrap_or("local");
        return Ok(make_source("local", owner, repo, path.to_string()));
    }

    // SSH form: git@host:owner/repo(.git)
    if let Some(rest) = spec.strip_prefix("git@") {
        let (host, path) = rest.split_once(':').ok_or_else(invalid)?;
        let (owner, repo) = split_owner_repo(path).ok_or_else(invalid)?;
        return Ok(make_source(host, &owner, &repo, spec.to_string()));
    }

    // URL form: scheme://host/owner/repo(.git)
    if let Some((scheme, rest)) = spec.split_once("://") {
        let (host, path) = rest.split_once('/').ok_or_else(invalid)?;
        // Item link (LNK-1): a deep URL with a tree/blob segment naming one
        // skill inside the repo. Checked before the plain owner/repo shape,
        // which rejects any extra path segments.
        if let Some(source) = parse_item_link(spec, scheme, host, path)? {
            return Ok(source);
        }
        let (owner, repo) = split_owner_repo(path).ok_or_else(invalid)?;
        let url = format!("{scheme}://{host}/{owner}/{repo}");
        return Ok(make_source(host, &owner, &repo, url));
    }

    // github: prefix shorthand
    let bare = spec.strip_prefix("github:").unwrap_or(spec);
    let (owner, repo) = split_owner_repo(bare).ok_or_else(invalid)?;
    let url = format!("https://github.com/{owner}/{repo}");
    Ok(make_source("github.com", &owner, &repo, url))
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
fn parse_link_tail(spec: &str, marker: &str, rest: &str) -> Result<(Pin, String)> {
    let invalid = || MindError::InvalidRepoSpec {
        spec: spec.to_string(),
    };
    // spec: LNK-3 -- the ref is the single segment after tree/blob.
    let mut segs = rest.trim_matches('/').split('/');
    let r = segs.next().filter(|s| !s.is_empty()).ok_or_else(invalid)?;
    // spec: LNK-1 -- a blob link must end in /SKILL.md (the skill directory is
    // its parent); a tree link naming the SKILL.md directly is also accepted.
    let mut parts: Vec<&str> = segs.collect();
    if parts.last() == Some(&"SKILL.md") {
        parts.pop();
    } else if marker == "blob" {
        return Err(invalid());
    }
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(invalid());
    }
    let item_path = parts.join("/");
    // spec: LNK-10 -- safe relative path and a valid git ref value, rejected
    // before any clone.
    if !crate::plugin_manifest::is_safe_manifest_path(&item_path)
        || crate::git::validate_ref_value(r).is_err()
    {
        return Err(invalid());
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
/// does not complete to a valid link is `InvalidRepoSpec` (LNK-2).
fn parse_item_link(spec: &str, scheme: &str, host: &str, path: &str) -> Result<Option<Source>> {
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
    let url = format!("{scheme}://{host}/{owner}/{repo}");
    let mut source = make_source(host, &owner, &repo, url);
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

fn make_source(host: &str, owner: &str, repo: &str, url: String) -> Source {
    Source {
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
        pin: Pin::default(),
        roots: None,
        flat_skills: false,
        add_roots: None,
        item_path: None,
        origin: None,
        plugin_version: None,
        install_hooks: Vec::new(),
        install_hook: None,
        install_hook_commit: None,
    }
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
    /// Checks the schema version (STO-50) and migrates any legacy
    /// `install_hook`/`install_hook_commit` pairs into `install_hooks`
    /// transparently on load (HOOK-55).
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

    // spec: LNK-2
    #[test]
    fn link_marker_without_a_valid_tail_is_invalid_repo_spec() {
        // No item path after the ref.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/main"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
        // No ref at all.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
    }

    // spec: LNK-10
    #[test]
    fn link_unsafe_path_or_ref_is_rejected_at_parse() {
        // `..` in the item path.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/main/../../etc"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
        // A git-range ref value.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/a..b/skills/foo"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
        // A leading-dash (option-shaped) ref value.
        assert!(matches!(
            parse_spec("https://github.com/o/r/tree/-evil/skills/foo"),
            Err(MindError::InvalidRepoSpec { .. })
        ));
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

    #[test]
    fn parses_local_path() {
        let s = parse_spec("/home/james/dev/agents").unwrap();
        assert_eq!(s.host, "local");
        assert_eq!(s.owner, "dev"); // parent directory becomes the owner
        assert_eq!(s.repo, "agents");
        assert_eq!(s.name, "local/dev/agents");
        assert_eq!(s.url, "/home/james/dev/agents");
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
        let local = parse_spec("/home/james/dev/agents").unwrap();
        assert_eq!(local.compare_url("aaaa", "bbbb"), None);
        let file_url = parse_spec("file:///home/james/dev/agents").unwrap();
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
        let local = parse_spec("/home/james/dev/agents").unwrap();
        assert_eq!(local.browse_url("abc1234"), None);
        let file_url = parse_spec("file:///home/james/dev/agents").unwrap();
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
        let local = parse_spec("/home/james/dev/agents").unwrap();
        let url = local.browse_url("abc1234");
        assert_eq!(url, None, "precondition: local path derives no browse URL");

        let text = crate::hook::disclosure_text(
            "/home/james/dev/agents",
            "local",
            "abc1234",
            "/home/james/dev/agents",
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
        let hook = RecordedHook {
            command: "make install".into(),
            ran_at: Some("deadbeef".into()),
        };
        let json = serde_json::to_string(&hook).unwrap();
        let back: RecordedHook = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back, hook,
            "RecordedHook with ran_at=Some did not round-trip"
        );
    }

    #[test]
    fn recorded_hook_serde_round_trip_with_ran_at_none() {
        // spec: HOOK-55
        let hook = RecordedHook {
            command: "make install".into(),
            ran_at: None,
        };
        let json = serde_json::to_string(&hook).unwrap();
        // ran_at=None should be absent (default) in the emitted JSON.
        let back: RecordedHook = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back, hook,
            "RecordedHook with ran_at=None did not round-trip"
        );
    }

    #[test]
    fn install_hooks_vec_round_trips_on_source() {
        // spec: HOOK-55
        let mut s = parse_spec("acme/tools").unwrap();
        s.install_hooks = vec![
            RecordedHook {
                command: "make setup".into(),
                ran_at: Some("aaa".into()),
            },
            RecordedHook {
                command: "make install".into(),
                ran_at: None,
            },
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
        src.install_hooks = vec![RecordedHook {
            command: "pre-existing".into(),
            ran_at: Some("aaa".into()),
        }];
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
            RecordedHook {
                command: "hook-a".into(),
                ran_at: None,
            },
            // Ran at "aaa", current is "bbb" -> advanced -> pending.
            RecordedHook {
                command: "hook-b".into(),
                ran_at: Some("aaa".into()),
            },
            // Ran at "bbb", current is "bbb" -> up-to-date -> NOT pending.
            RecordedHook {
                command: "hook-c".into(),
                ran_at: Some("bbb".into()),
            },
        ];

        let pending = src.pending_install_hooks(Some("bbb"));
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
            RecordedHook {
                command: "hook-a".into(),
                ran_at: None,
            },
            RecordedHook {
                command: "hook-b".into(),
                ran_at: Some("aaa".into()),
            },
        ];
        let pending = src.pending_install_hooks(None);
        // hook-a: ran_at=None -> is_none() -> always pending.
        // hook-b: ran_at=Some("aaa") != current=None -> pending.
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].command, "hook-a");
        assert_eq!(pending[1].command, "hook-b");
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
}
