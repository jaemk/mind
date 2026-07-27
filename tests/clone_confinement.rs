//! S31: a hand-tampered or stale `sources.json` entry whose `host`/`owner`/
//! `repo` parts traverse outside the managed sources tree must never let
//! `mind` delete or clone outside `~/.mind` (spec/storage.md STO-69).
//!
//! `Registry::load` revalidates every entry and drops one that fails (STO-68),
//! so the primary defense already keeps a traversing entry from ever reaching
//! a command. These tests assert the OBSERVABLE outcome that holds regardless
//! of which defense catches it: the out-of-tree victim directory (and its
//! contents) survive `sync` and `unmeld` untouched, and `unmeld` never prints
//! a success line for it. `src/commands.rs`'s `clone_dir_checked` unit tests
//! (STO-69) pin the second, independent guard directly.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

/// A throwaway `MIND_HOME`/`CLAUDE_HOME` pair with no source repo: these tests
/// hand-write `sources.json` directly rather than melding.
struct Env {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

impl Env {
    fn new(label: &str) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "mind-clone-confine-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let mind_home = base.join("mind");
        let claude_home = base.join("claude");
        std::fs::create_dir_all(&mind_home).unwrap();
        std::fs::create_dir_all(&claude_home).unwrap();
        Env {
            base,
            mind_home,
            claude_home,
        }
    }

    /// Write `sources.json` under this env's `MIND_HOME` verbatim.
    fn write_sources_json(&self, json: &str) {
        std::fs::write(self.mind_home.join("sources.json"), json).unwrap();
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Create a directory with one marker file inside it, and return its path
/// plus the marker's expected contents.
fn make_victim(dir: &Path) -> (PathBuf, &'static str) {
    let marker = dir.join("marker.txt");
    let contents = "do not delete me";
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(&marker, contents).unwrap();
    (marker, contents)
}

fn assert_victim_intact(marker: &Path, expected: &str, label: &str) {
    assert!(
        marker.exists(),
        "{label}: the out-of-tree victim file must survive at {}",
        marker.display()
    );
    let got = std::fs::read_to_string(marker).unwrap();
    assert_eq!(
        got, expected,
        "{label}: the out-of-tree victim file's contents must be untouched"
    );
}

// ---- Shape 1: host="..", owner="..", repo="victim" ------------------------
//
// `Source::clone_dir` resolves this to
// `<mind_home>/sources/../../victim` == `<mind_home's parent>/victim` -- one
// level above MIND_HOME, i.e. `<base>/victim` in this harness.

#[test]
fn unmeld_leaves_traversal_victim_intact_shape_parent_dir_parts() {
    // spec: STO-69
    let env = Env::new("unmeld-shape1");
    let (marker, contents) = make_victim(&env.base.join("victim"));

    env.write_sources_json(
        r#"{
  "version": 1,
  "sources": [
    {
      "name": "../../victim",
      "url": "https://example.com/victim",
      "host": "..",
      "owner": "..",
      "repo": "victim"
    }
  ]
}"#,
    );

    let r = env.mind(&["unmeld", "../../victim"]);
    assert!(
        !r.success,
        "unmeld of a traversing source must not report success: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("unmelded"),
        "no success line naming the traversing source may be printed: stdout={}",
        r.stdout
    );
    assert_victim_intact(&marker, contents, "unmeld shape1");
}

#[test]
fn sync_leaves_traversal_victim_intact_shape_parent_dir_parts() {
    // spec: STO-69
    let env = Env::new("sync-shape1");
    let (marker, contents) = make_victim(&env.base.join("victim"));

    env.write_sources_json(
        r#"{
  "version": 1,
  "sources": [
    {
      "name": "../../victim",
      "url": "https://example.com/victim",
      "host": "..",
      "owner": "..",
      "repo": "victim"
    }
  ]
}"#,
    );

    // sync must not delete/re-clone into the traversal target regardless of
    // whether it succeeds overall (with the entry dropped at load, there is
    // nothing left to sync, so the command may report "no sources melded").
    let _ = env.mind(&["sync"]);
    assert_victim_intact(&marker, contents, "sync shape1");
}

// ---- Shape 2: host="../../victim2", owner=".", repo="." -------------------
//
// Same resolved target one level up (`<base>/victim2`), reached with the
// traversal packed into `host` instead and `.` used for `owner`/`repo`.

#[test]
fn unmeld_leaves_traversal_victim_intact_shape_dotted_owner_repo() {
    // spec: STO-69
    let env = Env::new("unmeld-shape2");
    let (marker, contents) = make_victim(&env.base.join("victim2"));

    env.write_sources_json(
        r#"{
  "version": 1,
  "sources": [
    {
      "name": "traversal2",
      "url": "https://example.com/victim2",
      "host": "../../victim2",
      "owner": ".",
      "repo": "."
    }
  ]
}"#,
    );

    let r = env.mind(&["unmeld", "traversal2"]);
    assert!(
        !r.success,
        "unmeld of a traversing source must not report success: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("unmelded"),
        "no success line naming the traversing source may be printed: stdout={}",
        r.stdout
    );
    assert_victim_intact(&marker, contents, "unmeld shape2");
}

#[test]
fn sync_leaves_traversal_victim_intact_shape_dotted_owner_repo() {
    // spec: STO-69
    let env = Env::new("sync-shape2");
    let (marker, contents) = make_victim(&env.base.join("victim2"));

    env.write_sources_json(
        r#"{
  "version": 1,
  "sources": [
    {
      "name": "traversal2",
      "url": "https://example.com/victim2",
      "host": "../../victim2",
      "owner": ".",
      "repo": "."
    }
  ]
}"#,
    );

    let _ = env.mind(&["sync"]);
    assert_victim_intact(&marker, contents, "sync shape2");
}
