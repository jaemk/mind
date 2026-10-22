//! Integration tests for release version pins (spec/versions.md).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo whose commits carry semver release tags, melded by filesystem path
//! with `MIND_HOME`/`CLAUDE_HOME` pointed at temp dirs. No network.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
    /// An empty git repo named `name` on branch `main`.
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-versions-{}-{n}", std::process::id()));
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
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
    }

    /// Commit an agent whose description is `desc`, plus `mind_toml` as the
    /// root `mind.toml` (removed when empty). Returns the commit sha.
    fn commit(&self, desc: &str, mind_toml: &str) -> String {
        write_file(
            &self.source.join("agents/dev.md"),
            &format!("---\nname: dev\ndescription: {desc}\n---\n# dev\n"),
        );
        let toml = self.source.join("mind.toml");
        if mind_toml.is_empty() {
            let _ = std::fs::remove_file(&toml);
        } else {
            write_file(&toml, mind_toml);
        }
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", desc]);
        self.head()
    }

    fn head(&self) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn tag(&self, name: &str, sha: &str) {
        git(&self.source, &["tag", "-f", name, sha]);
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn identity(&self) -> String {
        format!(
            "local/{}/{}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// The registry entry of this sandbox's source, or `None` when not melded.
    fn entry(&self) -> Option<Value> {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).expect("sources.json is JSON");
        v["sources"]
            .as_array()?
            .iter()
            .find(|s| s["name"].as_str() == Some(self.identity().as_str()))
            .cloned()
    }

    /// The recorded pin and commit.
    fn pin(&self) -> (Value, String) {
        let e = self.entry().expect("source is melded");
        (
            e["pin"].clone(),
            e["commit"].as_str().unwrap_or("").to_string(),
        )
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

const RELEASE_TAGS: &str = "[source]\nrelease-tags = \"v{version}\"\n";

/// Releases v1.0.0, v1.1.0, v2.0.0 plus a prerelease and a non-semver tag,
/// then a default-branch tip past them. Returns the sandbox and the shas of
/// the three releases.
fn released(name: &str, tip_toml: &str) -> (Sandbox, [String; 3]) {
    let sb = Sandbox::new(name);
    let a = sb.commit("one-zero", RELEASE_TAGS);
    sb.tag("v1.0.0", &a);
    let b = sb.commit("one-one", RELEASE_TAGS);
    sb.tag("v1.1.0", &b);
    sb.tag("v1.2", &b);
    let rc = sb.commit("two-rc", RELEASE_TAGS);
    sb.tag("v2.0.0-rc.1", &rc);
    let c = sb.commit("two-zero", RELEASE_TAGS);
    sb.tag("v2.0.0", &c);
    sb.commit("unreleased", tip_toml);
    (sb, [a, b, c])
}

#[test]
fn meld_resolves_the_highest_release_in_range() {
    // spec: VER-30 VER-40 VER-42
    let (sb, [_a, b, _c]) = released("ver-meld", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["kind"], "version", "{pin}");
    assert_eq!(pin["value"]["range"], "^1", "{pin}");
    assert_eq!(pin["value"]["resolved"], "1.1.0", "{pin}");
    assert_eq!(pin["value"]["tag"], "v1.1.0", "{pin}");
    // The highest stable release, outside the range; the prerelease is not it.
    assert_eq!(pin["value"]["highest"], "2.0.0", "{pin}");
    assert_eq!(commit, b, "the commit the resolved tag points at");
}

#[test]
fn meld_with_no_release_in_range_registers_nothing() {
    // spec: VER-42
    let (sb, _) = released("ver-none", RELEASE_TAGS);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^3"]);
    assert!(!r.success, "meld must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("no release matches ^3") && r.stderr.contains("highest release is 2.0.0"),
        "{}",
        r.stderr
    );
    assert!(sb.entry().is_none(), "nothing is registered");
}

#[test]
fn a_range_that_does_not_parse_is_a_usage_error() {
    // spec: VER-22
    let (sb, _) = released("ver-bad-range", RELEASE_TAGS);
    for bad in ["version=~1", "version=v1", "version=1.2.3.4", "version="] {
        let r = sb.mind(&["meld", &sb.spec(), "--pin", bad]);
        assert!(!r.success, "{bad}: {}", r.stdout);
        assert!(
            r.stderr.contains("invalid --pin value"),
            "{bad}: {}",
            r.stderr
        );
        assert!(sb.entry().is_none(), "{bad}: nothing is registered");
    }
}

#[test]
fn no_release_tags_uses_the_default_template() {
    // spec: VER-4
    let sb = Sandbox::new("ver-default-template");
    let a = sb.commit("bare", "");
    sb.tag("1.0.0", &a);
    let b = sb.commit("prefixed", "");
    sb.tag("v1.3.0", &b);
    sb.tag("release-1.9.0", &b);
    sb.commit("tip", "");
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.3.0", "{pin}");
    assert_eq!(commit, b);

    // No tag a release at all: the meld fails and registers nothing.
    let sb = Sandbox::new("ver-no-releases");
    let a = sb.commit("only", "");
    sb.tag("stable", &a);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no tag is a release"), "{}", r.stderr);
    assert!(sb.entry().is_none());
}

#[test]
fn the_template_comes_from_the_default_branch() {
    // spec: VER-3
    // The releases themselves declare no template; only the default-branch
    // tip does, with a prefix no default-template tag would match.
    let sb = Sandbox::new("ver-tip-template");
    let a = sb.commit("one", "");
    sb.tag("skills-v1.0.0", &a);
    sb.tag("v1.5.0", &a);
    sb.commit("tip", "[source]\nrelease-tags = \"skills-v{version}\"\n");
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, _) = sb.pin();
    assert_eq!(pin["value"]["tag"], "skills-v1.0.0", "{pin}");
}

#[test]
fn sync_moves_to_a_new_compatible_release() {
    // spec: VER-43
    let (sb, [_a, b, c]) = released("ver-sync", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // A new 1.x release, and a 3.0.0 past the range.
    let d = sb.commit("one-two", RELEASE_TAGS);
    sb.tag("v1.2.0", &d);
    let e = sb.commit("three", RELEASE_TAGS);
    sb.tag("v3.0.0", &e);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(commit, d, "moved to the new in-range release");
    assert_eq!(pin["value"]["range"], "^1", "sync never changes the range");
    assert_eq!(pin["value"]["resolved"], "1.2.0", "{pin}");
    assert_eq!(pin["value"]["tag"], "v1.2.0", "{pin}");
    assert_eq!(pin["value"]["highest"], "3.0.0", "{pin}");
    let _ = c;

    // Nothing new: sync stays put.
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    assert_eq!(sb.pin().1, d);
}

#[test]
fn sync_does_not_follow_a_moved_release_tag() {
    // spec: VER-44
    let (sb, [_a, b, _c]) = released("ver-moved", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // Re-point v1.1.0 at a new commit, with no newer 1.x release.
    let moved = sb.commit("moved", RELEASE_TAGS);
    sb.tag("v1.1.0", &moved);
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync: {}", r.stderr);
    assert!(
        r.stderr.contains("release tag v1.1.0 moved"),
        "warns about the moved tag: {}",
        r.stderr
    );
    let (pin, commit) = sb.pin();
    assert_eq!(commit, b, "keeps the recorded commit");
    assert_eq!(pin["value"]["tag"], "v1.1.0");

    // Re-melding with --pin accepts the moved tag.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "re-pin: {}", r.stderr);
    assert_eq!(sb.pin().1, moved);
}

#[test]
fn a_yanked_release_is_never_selected() {
    // The yanked list is read from the default-branch tip.
    let tip = "[source]\nrelease-tags = \"v{version}\"\nyanked = [\"1.1.0\"]\n";
    let (sb, [a, _b, _c]) = released("ver-yanked", tip);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"]);
    assert!(r.success, "meld: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["resolved"], "1.0.0", "{pin}");
    assert_eq!(commit, a);
}

#[test]
fn re_pinning_switches_between_range_exact_and_other_pins() {
    // spec: VER-80
    let (sb, [a, b, c]) = released("ver-repin", RELEASE_TAGS);
    assert!(
        sb.mind(&["meld", &sb.spec(), "--pin", "version=^1"])
            .success
    );
    assert_eq!(sb.pin().1, b);

    // Step back to an older release with an exact hold.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version==1.0.0"]);
    assert!(r.success, "exact: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["value"]["range"], "=1.0.0", "{pin}");
    assert_eq!(commit, a);

    // Over to another pin kind, then back to a range.
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "tag=v2.0.0"]);
    assert!(r.success, "tag: {}", r.stderr);
    assert_eq!(sb.pin().0["kind"], "tag");
    assert_eq!(sb.pin().1, c);
    let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^2"]);
    assert!(r.success, "range: {}", r.stderr);
    let (pin, commit) = sb.pin();
    assert_eq!(pin["kind"], "version");
    assert_eq!(pin["value"]["resolved"], "2.0.0", "{pin}");
    assert_eq!(commit, c);
}

#[test]
fn a_version_range_is_unpinned_under_require_pinned_policy() {
    // A range floats, so POL-20's pinned policy refuses it; an exact version
    // is pinned.
    let (sb, [a, _b, _c]) = released("ver-policy", RELEASE_TAGS);
    let policy = sb.base.join("policy.toml");
    write_file(&policy, "[sources]\npinned = true\n");
    let run = |pin: &str| {
        Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(["meld", &sb.spec(), "--pin", pin])
            .env("MIND_HOME", &sb.mind_home)
            .env("CLAUDE_HOME", &sb.claude_home)
            .env("MIND_POLICY_FILE", &policy)
            .stdin(Stdio::null())
            .output()
            .expect("run mind")
    };
    let out = run("version=^1");
    assert!(!out.status.success(), "a range must be refused");
    assert!(sb.entry().is_none());
    let out = run("version==1.0.0");
    assert!(
        out.status.success(),
        "an exact version is pinned: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(sb.pin().1, a);
}
