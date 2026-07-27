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

// ---- STO-68: Registry::load drops a stale entry rather than bricking mind ---
//
// STO-68's whole point is graceful degradation: "hard-erroring would brick
// every `mind` verb for a user who happens to be carrying such a stale entry".
// Only the in-process `Registry::load` was driven; nothing proved the CLI verbs
// a user would actually reach for still work, warn, and hide the dropped entry.

/// A registry whose FIRST entry carries the exact value STO-68 names as its
/// motivating example (0.21.0's parser accepted `repo: ".."`), plus one healthy
/// entry that must survive.
const STALE_DOTDOT_REPO_REGISTRY: &str = r#"{
  "version": 1,
  "sources": [
    {
      "name": "example.com/acme/..",
      "url": "https://example.com/acme/x",
      "host": "example.com",
      "owner": "acme",
      "repo": ".."
    },
    {
      "name": "github.com/acme/agents",
      "url": "https://github.com/acme/agents",
      "host": "github.com",
      "owner": "acme",
      "repo": "agents"
    }
  ]
}"#;

#[test]
fn a_stale_dotdot_repo_entry_is_dropped_with_a_warning_and_does_not_brick_the_cli() {
    // spec: STO-68
    let env = Env::new("stale-repo");
    env.write_sources_json(STALE_DOTDOT_REPO_REGISTRY);

    let r = env.mind(&["recall", "--sources"]);
    assert!(
        r.success,
        "a stale entry must not brick a read-only verb: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("sources.json") && r.stderr.contains("repo"),
        "the warning must name sources.json and the offending part: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("example.com/acme/.."),
        "the warning must name the dropped entry: {}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("example.com"),
        "the dropped entry must not be listed as a melded source: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("github.com/acme/agents"),
        "the healthy entry must survive: {}",
        r.stdout
    );
}

#[test]
fn the_drop_is_not_written_back_until_a_verb_saves_the_registry() {
    // spec: STO-68 -- "The drop is not written back immediately; it becomes
    // permanent the next time `Registry::save` runs". A read-only verb must
    // leave sources.json byte-identical (the file is the only remaining record
    // of the entry), and a mutating verb must persist the drop.
    let env = Env::new("stale-persist");
    env.write_sources_json(STALE_DOTDOT_REPO_REGISTRY);
    let file = env.mind_home.join("sources.json");

    assert!(env.mind(&["recall", "--sources"]).success);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        STALE_DOTDOT_REPO_REGISTRY,
        "a read-only verb must not rewrite sources.json"
    );

    // `unmeld` of the surviving entry saves the registry, making the drop
    // permanent as a side effect.
    let u = env.mind(&["unmeld", "github.com/acme/agents", "--unlink-only"]);
    assert!(u.success, "unmeld failed: {} {}", u.stdout, u.stderr);
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(
        !after.contains("\"repo\": \"..\"") && !after.contains("\"repo\":\"..\""),
        "the drop must be persisted by the next registry save: {after}"
    );
}
