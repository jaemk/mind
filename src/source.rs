//! Melded sources: the GitHub (or arbitrary git) repos `mind` pulls items from.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{MindError, Result};
use crate::paths::Paths;

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

    /// A browser URL to compare two commits, for `mind evolve` output.
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
        std::fs::write(&file, json).map_err(|e| MindError::io(&file, e))
    }

    pub fn find(&self, name: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.name == name)
    }
}

#[cfg(test)]
mod tests {
    // spec: CLI-11 (repo spec parsing), CLI-61 (compare url), STO-13 (identity)
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
        assert_eq!(parse_spec("james/agents").unwrap().name, "github.com/james/agents");
        assert_eq!(parse_spec("bob/agents").unwrap().name, "github.com/bob/agents");
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
}
