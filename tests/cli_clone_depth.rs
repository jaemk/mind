//! Integration tests for clone depth (spec/discovery.md DSC-118..121).
//!
//! The source is a local git repo reached through a remote-looking URL: git's
//! `url.<file://base>.insteadOf` rewrites it to `file://`, so `--depth` is
//! honored (a plain-path clone ignores it) while everything stays on disk. No
//! network.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

const REMOTE_PREFIX: &str = "https://git.example.test/acme/";

struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    /// A git repo named `name` on `main` holding two commits; returns the
    /// sandbox and the first commit's sha.
    fn new(name: &str, mind_toml: &str) -> (Sandbox, String) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-depth-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        std::fs::create_dir_all(&source).unwrap();
        for args in [
            vec!["-c", "init.defaultBranch=main", "init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(&source, &args);
        }
        let sb = Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let first = sb.commit("first", "");
        sb.commit("second", mind_toml);
        (sb, first)
    }

    /// Commit an agent described `desc` and `mind_toml` (none when empty).
    fn commit(&self, desc: &str, mind_toml: &str) -> String {
        write_file(
            &self.source.join("agents/dev.md"),
            &format!("---\nname: dev\ndescription: {desc}\n---\n# dev\n"),
        );
        if !mind_toml.is_empty() {
            write_file(&self.source.join("mind.toml"), mind_toml);
        }
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", desc]);
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn name(&self) -> String {
        self.source
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn remote_spec(&self) -> String {
        format!("{REMOTE_PREFIX}{}", self.name())
    }

    fn clone_dir(&self) -> PathBuf {
        self.mind_home
            .join("sources/git.example.test/acme")
            .join(self.name())
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_TTY")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                format!("url.file://{}/.insteadOf", self.base.display()),
            )
            .env("GIT_CONFIG_VALUE_0", REMOTE_PREFIX)
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn meld(&self, extra: &[&str]) {
        let spec = self.remote_spec();
        let mut args = vec!["meld", spec.as_str(), "--yes"];
        args.extend_from_slice(extra);
        let r = self.mind(&args);
        assert!(r.success, "meld {extra:?}: {}\n{}", r.stdout, r.stderr);
    }

    /// Whether the source's clone is shallow.
    fn shallow(&self) -> bool {
        let out = Command::new("git")
            .args(["rev-parse", "--is-shallow-repository"])
            .current_dir(self.clone_dir())
            .output()
            .unwrap();
        assert!(out.status.success(), "no clone at {:?}", self.clone_dir());
        String::from_utf8(out.stdout).unwrap().trim() == "true"
    }

    /// Whether the source's clone holds commit `sha`.
    fn has_commit(&self, sha: &str) -> bool {
        Command::new("git")
            .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
            .current_dir(self.clone_dir())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    /// The source's `sources.json` entry.
    fn entry(&self) -> Value {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let want = format!("git.example.test/acme/{}", self.name());
        v["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"].as_str() == Some(want.as_str()))
            .cloned()
            .expect("source is melded")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} in {dir:?}");
}

#[test]
fn meld_clones_shallow_by_default() {
    // spec: DSC-118
    let (sb, first) = Sandbox::new("plain", "");
    sb.meld(&[]);
    assert!(sb.shallow(), "a plain meld clones at depth 1");
    assert!(!sb.has_commit(&first), "only the tip is fetched");
    assert!(
        sb.entry().get("full_history").is_none(),
        "the flag is not written when off: {}",
        sb.entry()
    );
}

#[test]
fn meld_full_history_clones_complete_and_sync_keeps_it() {
    // spec: DSC-121 DSC-119
    let (sb, first) = Sandbox::new("full", "");
    sb.meld(&["--full-history"]);
    assert!(!sb.shallow(), "--full-history clones the whole history");
    assert!(sb.has_commit(&first));
    assert_eq!(sb.entry()["full_history"], Value::Bool(true));

    let third = sb.commit("third", "");
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    assert!(sb.has_commit(&third), "sync moved to the new tip");
    assert!(!sb.shallow(), "sync keeps the history");
    assert!(sb.has_commit(&first));
}

#[test]
fn full_history_on_remeld_deepens_and_persists() {
    // spec: DSC-121
    let (sb, first) = Sandbox::new("remeld", "");
    sb.meld(&[]);
    assert!(sb.shallow());
    sb.meld(&["--full-history"]);
    assert!(!sb.shallow(), "a re-meld with --full-history deepens");
    assert!(sb.has_commit(&first));
    assert_eq!(sb.entry()["full_history"], Value::Bool(true));
}

#[test]
fn full_history_on_a_linked_local_source_notes_it_does_not_apply() {
    // spec: DSC-121 -- a linked local source is the user's working tree; both
    // a fresh meld and a re-meld say the flag does not apply.
    let (sb, _first) = Sandbox::new("linked", "");
    let path = sb.source.to_string_lossy().into_owned();
    for _ in 0..2 {
        let r = sb.mind(&["meld", &path, "--full-history", "--yes"]);
        assert!(r.success, "{}\n{}", r.stdout, r.stderr);
        assert!(
            r.stderr
                .contains("is your own working tree, so --full-history does not apply"),
            "{}",
            r.stderr
        );
    }
}

#[test]
fn source_declared_full_history_deepens_meld() {
    // spec: DSC-120
    let (sb, first) = Sandbox::new("declared", "[source]\nhistory = \"full\"\n");
    sb.meld(&[]);
    assert!(!sb.shallow(), "history = \"full\" deepens the clone");
    assert!(sb.has_commit(&first));
    assert!(
        sb.entry().get("full_history").is_none(),
        "the source's declaration is not the consumer flag"
    );
}
