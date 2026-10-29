//! Integration tests for items new to a source installed whole: recording the
//! source as whole-tracked, `upgrade` installing what it gained, and the
//! `--skip-new` / `--ignore-new` opt-outs (spec/lifecycle.md, LIFE-53..56).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. A maintainer change is a new commit in the fixture repo. No
//! network.

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

impl Run {
    fn all(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

impl Sandbox {
    /// A source offering `skill:review`, `skill:test`, and `rule:style`, plus
    /// `mind_toml` at the root when it is not empty.
    fn with_toml(mind_toml: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-upgrade-new-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("lib");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        for skill in ["review", "test"] {
            write_file(
                &source.join(format!("skills/{skill}/SKILL.md")),
                &format!("---\nname: {skill}\ndescription: {skill} skill\n---\n# {skill}\n"),
            );
        }
        write_file(
            &source.join("rules/style.md"),
            "---\ndescription: Style rule\n---\n# style\n",
        );
        if !mind_toml.is_empty() {
            write_file(&source.join("mind.toml"), mind_toml);
        }
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    fn new() -> Sandbox {
        Sandbox::with_toml("")
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// A maintainer adds a skill and commits it.
    fn add_skill(&self, name: &str) {
        write_file(
            &self.source.join(format!("skills/{name}/SKILL.md")),
            &format!("---\nname: {name}\ndescription: {name} skill\n---\n# {name}\n"),
        );
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "add skill"]);
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
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

    fn ok(&self, args: &[&str]) -> Run {
        let r = self.mind(args);
        assert!(r.success, "mind {args:?}: {} {}", r.stdout, r.stderr);
        r
    }

    fn registry(&self) -> Value {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap();
        serde_json::from_str(&raw).expect("sources.json is JSON")
    }

    /// The fixture's one registry entry.
    fn source_entry(&self) -> Value {
        self.registry()["sources"][0].clone()
    }

    fn whole_snapshot(&self) -> Option<Vec<String>> {
        strings(&self.source_entry()["whole_snapshot"])
    }

    /// Drop the source's `whole_snapshot`, as a registry written before it
    /// existed has none.
    fn clear_whole_snapshot(&self) {
        let mut reg = self.registry();
        reg["sources"][0]
            .as_object_mut()
            .unwrap()
            .remove("whole_snapshot");
        std::fs::write(
            self.mind_home.join("sources.json"),
            serde_json::to_string_pretty(&reg).unwrap(),
        )
        .unwrap();
    }

    fn installed(&self) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(self.mind_home.join("manifest.json")) else {
            return Vec::new();
        };
        let v: Value = serde_json::from_str(&raw).expect("manifest.json is JSON");
        let mut keys: Vec<String> = v["items"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        keys
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| {
        a.iter()
            .map(|s| s.as_str().unwrap_or_default().to_string())
            .collect()
    })
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git");
    assert!(st.success(), "git {args:?} failed");
}

const ALL_THREE: [&str; 3] = ["rule:style", "skill:review", "skill:test"];

#[test]
fn installing_a_source_whole_records_its_catalog() {
    // spec: LIFE-53
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    assert_eq!(sb.whole_snapshot(), Some(svec(&ALL_THREE)));
}

#[test]
fn learning_the_last_missing_item_records_the_source() {
    // spec: LIFE-53 -- a cherry-picked source is not tracked until every item
    // in it is installed, by whatever route.
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--register-only"]);
    sb.ok(&["learn", "review", "--yes"]);
    assert_eq!(sb.whole_snapshot(), None);
    sb.ok(&["learn", "test", "--yes"]);
    assert_eq!(sb.whole_snapshot(), None);
    sb.ok(&["learn", "style", "--yes"]);
    assert_eq!(sb.whole_snapshot(), Some(svec(&ALL_THREE)));
}

#[test]
fn upgrade_installs_an_item_new_to_a_source_installed_whole() {
    // spec: LIFE-54
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.add_skill("deploy");
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(
        r.stdout.contains("1 new item in sources installed whole"),
        "{}",
        r.all()
    );
    assert!(r.stdout.contains("skill:deploy"), "{}", r.all());
    assert!(sb.installed().contains(&"skill:deploy".to_string()));
    assert!(
        sb.whole_snapshot()
            .unwrap()
            .contains(&"skill:deploy".to_string())
    );
    let again = sb.ok(&["upgrade", "--yes"]);
    assert!(
        again.stdout.contains("everything is up to date"),
        "{}",
        again.all()
    );
}

#[test]
fn upgrade_leaves_a_cherry_picked_source_alone() {
    // spec: LIFE-53 LIFE-54
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--register-only"]);
    sb.ok(&["learn", "review", "--yes"]);
    sb.add_skill("deploy");
    sb.ok(&["upgrade", "--yes"]);
    assert_eq!(sb.installed(), svec(&["skill:review"]));
    assert_eq!(sb.whole_snapshot(), None);
}

#[test]
fn a_forgotten_item_is_not_offered_again() {
    // spec: LIFE-54
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["forget", "test", "--yes"]);
    sb.add_skill("deploy");
    sb.ok(&["upgrade", "--yes"]);
    let installed = sb.installed();
    assert!(
        installed.contains(&"skill:deploy".to_string()),
        "{installed:?}"
    );
    assert!(
        !installed.contains(&"skill:test".to_string()),
        "{installed:?}"
    );
}

#[test]
fn an_item_removed_upstream_leaves_the_snapshot() {
    // spec: LIFE-54
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    std::fs::remove_dir_all(sb.source.join("skills/test")).unwrap();
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "drop test"]);
    sb.ok(&["upgrade", "--yes"]);
    assert_eq!(
        sb.whole_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

#[test]
fn a_source_installed_whole_before_tracking_is_tracked_by_its_next_upgrade() {
    // spec: LIFE-53 LIFE-54 -- recorded from the pre-fetch catalog, so the
    // item the fetch brings in is new. The source is cloned (pinned to a
    // branch) so the fetch, not the working tree, carries the new item.
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--pin", "branch=main", "--yes"]);
    sb.clear_whole_snapshot();
    sb.add_skill("deploy");
    sb.ok(&["upgrade", "--yes"]);
    assert!(sb.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn skip_new_defers_new_items_to_the_next_upgrade() {
    // spec: LIFE-55
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.add_skill("deploy");
    let r = sb.ok(&["upgrade", "--yes", "--skip-new"]);
    assert!(
        r.all().contains("skipped 1 new item(s) (--skip-new)"),
        "{}",
        r.all()
    );
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
    assert_eq!(sb.whole_snapshot(), Some(svec(&ALL_THREE)));
    sb.ok(&["upgrade", "--yes"]);
    assert!(sb.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn ignore_new_records_new_items_without_installing_them() {
    // spec: LIFE-55
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.add_skill("deploy");
    let r = sb.ok(&["upgrade", "--yes", "--ignore-new"]);
    assert!(
        r.all().contains("ignored new item skill:deploy"),
        "{}",
        r.all()
    );
    assert!(r.all().contains("mind learn 'skill:deploy'"), "{}", r.all());
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
    sb.ok(&["upgrade", "--yes"]);
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
    sb.ok(&["learn", "deploy", "--yes"]);
    assert!(sb.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn skip_new_and_ignore_new_conflict() {
    // spec: LIFE-55
    let sb = Sandbox::new();
    let r = sb.mind(&["upgrade", "--skip-new", "--ignore-new"]);
    assert!(!r.success, "{}", r.all());
    let r = sb.mind(&["sync", "--upgrade", "--skip-new", "--ignore-new"]);
    assert!(!r.success, "{}", r.all());
}

#[test]
fn sync_upgrade_takes_the_opt_outs() {
    // spec: LIFE-55
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.add_skill("deploy");
    let r = sb.ok(&["sync", "--upgrade", "--yes", "--skip-new"]);
    assert!(
        r.all().contains("skipped 1 new item(s) (--skip-new)"),
        "{}",
        r.all()
    );
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
    sb.ok(&["sync", "--upgrade", "--yes", "--ignore-new"]);
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
    sb.ok(&["sync", "--upgrade", "--yes"]);
    assert!(!sb.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn sync_opt_outs_require_upgrade() {
    // spec: LIFE-55
    let sb = Sandbox::new();
    assert!(!sb.mind(&["sync", "--skip-new"]).success);
    assert!(!sb.mind(&["sync", "--ignore-new"]).success);
}

#[test]
fn json_result_carries_new_and_ignored_items() {
    // spec: LIFE-56
    let sb = Sandbox::new();
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.add_skill("deploy");
    let r = sb.ok(&["--json", "upgrade", "--yes"]);
    let v: Value = serde_json::from_str(&r.stdout).expect("one JSON document");
    let data = if v.get("data").is_some() {
        &v["data"]
    } else {
        &v
    };
    assert_eq!(data["new_items"][0]["key"], "skill:deploy", "{v}");
    assert!(data.get("ignored_new_items").is_none(), "{v}");

    sb.add_skill("lint");
    let r = sb.ok(&["--json", "upgrade", "--yes", "--ignore-new"]);
    let v: Value = serde_json::from_str(&r.stdout).expect("one JSON document");
    let data = if v.get("data").is_some() {
        &v["data"]
    } else {
        &v
    };
    assert_eq!(data["ignored_new_items"][0]["key"], "skill:lint", "{v}");
    assert!(data.get("new_items").is_none(), "{v}");
}

#[test]
fn the_opt_outs_apply_to_profile_additions() {
    // spec: LIFE-55 PROF-21
    let toml = "[profiles]\ndefault = [\"skill:review\"]\n";
    let sb = Sandbox::with_toml(toml);
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    assert_eq!(sb.installed(), svec(&["skill:review"]));
    sb.ok(&["upgrade", "--yes"]); // settle
    write_file(
        &sb.source.join("mind.toml"),
        "[profiles]\ndefault = [\"skill:review\", \"skill:test\"]\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "grow default"]);

    let r = sb.ok(&["upgrade", "--yes", "--skip-new"]);
    assert!(r.all().contains("skipped 1 new item(s)"), "{}", r.all());
    assert_eq!(sb.installed(), svec(&["skill:review"]));

    let r = sb.ok(&["upgrade", "--yes", "--ignore-new"]);
    assert!(
        r.all().contains("ignored new item skill:test"),
        "{}",
        r.all()
    );
    sb.ok(&["upgrade", "--yes"]);
    assert_eq!(sb.installed(), svec(&["skill:review"]));
}
