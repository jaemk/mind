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
    /// The install hook command in effect for this source (HOOK-31), if any:
    /// the maintainer's `[source].install` or a consumer `meld --install-hook`
    /// override. `None` when the source has no hook. Persisted; lets `recall`/
    /// `introspect` report a source has a hook and `upgrade` detect a changed
    /// command.
    #[serde(default)]
    pub install_hook: Option<String>,
    /// The commit the install hook last ran at (HOOK-31), or `None` if the hook
    /// is recorded but has not been run yet (the user skipped it). Lets `upgrade`
    /// detect the source advanced past the last hook run and re-prompt (HOOK-11).
    #[serde(default)]
    pub install_hook_commit: Option<String>,
}

impl Source {
    /// On-disk clone location: `<mind>/sources/<host>/<owner>/<repo>`.
    pub fn clone_dir(&self, paths: &Paths) -> PathBuf {
        paths
            .sources_dir()
            .join(&self.host)
            .join(&self.owner)
            .join(&self.repo)
    }

    /// A browser URL to compare two commits, for `mind upgrade` output.
    pub fn compare_url(&self, from: &str, to: &str) -> Option<String> {
        if self.host == "github.com" {
            Some(format!(
                "https://github.com/{}/{}/compare/{from}...{to}",
                self.owner, self.repo
            ))
        } else {
            None
        }
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
        install_hook: None,
        install_hook_commit: None,
    }
}

/// The persisted registry of melded sources.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub sources: Vec<Source>,
}

impl Registry {
    /// Load the registry, returning an empty one if the file does not exist.
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.sources_file();
        match std::fs::read(&file) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| MindError::json("sources.json", e))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(e) => Err(MindError::io(&file, e)),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        paths.ensure_layout()?;
        let file = paths.sources_file();
        let json =
            serde_json::to_vec_pretty(self).map_err(|e| MindError::json("sources.json", e))?;
        Paths::atomic_write(&file, &json)
    }

    pub fn find(&self, name: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    // spec: CLI-11 (repo spec parsing), CLI-61 (compare url), STO-13 (identity)
    // spec: STO-18 (pin serde round-trip)
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
    fn compare_url_only_for_github() {
        let gh = parse_spec("foo/bar").unwrap();
        assert_eq!(
            gh.compare_url("aaaa", "bbbb").as_deref(),
            Some("https://github.com/foo/bar/compare/aaaa...bbbb")
        );
        let local = parse_spec("/tmp/x").unwrap();
        assert_eq!(local.compare_url("aaaa", "bbbb"), None);
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
    fn install_hook_fields_round_trip_and_default_absent() {
        // spec: HOOK-31
        // Older sources.json without the fields => both None (serde default).
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

        // A source carrying both fields round-trips losslessly.
        let mut s = parse_spec("acme/tools").unwrap();
        s.install_hook = Some("make install".into());
        s.install_hook_commit = Some("abc1234".into());
        let json = serde_json::to_string(&s).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.install_hook.as_deref(),
            Some("make install"),
            "install_hook did not round-trip"
        );
        assert_eq!(
            back.install_hook_commit.as_deref(),
            Some("abc1234"),
            "install_hook_commit did not round-trip"
        );
    }
}
