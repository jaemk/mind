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
        flat_skills: false,
        install_hooks: Vec::new(),
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
    ///
    /// Migrates any legacy `install_hook`/`install_hook_commit` pairs into
    /// `install_hooks` transparently on load (HOOK-55).
    pub fn load(paths: &Paths) -> Result<Self> {
        let file = paths.sources_file();
        match std::fs::read(&file) {
            Ok(bytes) => {
                let mut reg: Registry = serde_json::from_slice(&bytes)
                    .map_err(|e| MindError::json("sources.json", e))?;
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
}
