//! Integration tests for install-profile tracking at `upgrade`, disabling a
//! profile with `forget <source> --profile`, and the profile display in
//! `recall` / `probe` (spec/profiles.md, PROF-5 at upgrade, PROF-20 carry-over,
//! PROF-21..25, PROF-29).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. A maintainer change is a new commit in the fixture repo,
//! which `upgrade` fetches. No network. The fixture mirrors
//! tests/cli_profiles.rs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// `default` includes `basic`; `full` includes `basic` plus every agent and
/// `skill:deploy` (which depends on `skill:helper`, a closure item in no
/// profile). `skill:test` is in no profile.
const PROFILES: &str = "[profiles]\n\
default = [\"basic\"]\n\
basic = [\"skill:review\", \"rule:style\"]\n\
full = [\"basic\", \"agent:*\", \"skill:deploy\"]\n";

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
    fn with_toml(name: &str, mind_toml: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-profiles-track-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        for skill in ["review", "test", "helper"] {
            write_file(
                &source.join(format!("skills/{skill}/SKILL.md")),
                &format!("---\nname: {skill}\ndescription: {skill} skill\n---\n# {skill}\n"),
            );
        }
        write_file(
            &source.join("skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: deploy skill\n---\n# deploy\nUses {{ns:helper}}.\n",
        );
        write_file(
            &source.join("agents/planner.md"),
            "---\nname: planner\ndescription: Planner agent\n---\n# planner\n",
        );
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

    fn new(name: &str) -> Sandbox {
        Sandbox::with_toml(name, PROFILES)
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

    /// A maintainer change: write `rel` in the fixture repo and commit it.
    fn commit_file(&self, rel: &str, contents: &str) {
        write_file(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "change"]);
    }

    /// Replace the fixture's `mind.toml` and commit it.
    fn commit_toml(&self, mind_toml: &str) {
        self.commit_file("mind.toml", mind_toml);
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, &[], None)
    }

    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_TTY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(match stdin {
                Some(_) => Stdio::piped(),
                None => Stdio::null(),
            });
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("run mind");
        if let Some(text) = stdin {
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(text.as_bytes())
                .expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait for mind");
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

    fn source_entry(&self) -> Value {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap();
        let v: Value = serde_json::from_str(&raw).expect("sources.json is JSON");
        let list = v
            .get("sources")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| v.as_array().cloned())
            .unwrap_or_default();
        list.into_iter()
            .find(|s| s["name"].as_str() == Some(self.identity().as_str()))
            .expect("source registered")
    }

    fn recorded_profiles(&self) -> Option<Vec<String>> {
        strings(&self.source_entry()["profiles"])
    }

    fn recorded_snapshot(&self) -> Option<Vec<String>> {
        strings(&self.source_entry()["profile_snapshot"])
    }

    fn manifest_item(&self, key: &str) -> Option<Value> {
        let raw = std::fs::read_to_string(self.mind_home.join("manifest.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).expect("manifest.json is JSON");
        v["items"].get(key).cloned()
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

    fn reasons(&self, key: &str) -> Vec<String> {
        self.manifest_item(key)
            .and_then(|i| strings(&i["profiles"]))
            .unwrap_or_default()
    }

    fn explicit(&self, key: &str) -> bool {
        self.manifest_item(key)
            .and_then(|i| i["explicit"].as_bool())
            .unwrap_or(false)
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
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} in {dir:?}");
}

fn json_doc(run: &Run) -> Value {
    serde_json::from_str(run.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is one JSON object ({e}): {}", run.stdout))
}

/// [`PROFILES`] with `skill:test` added to `basic`.
const PROFILES_PLUS_TEST: &str = "[profiles]\n\
default = [\"basic\"]\n\
basic = [\"skill:review\", \"rule:style\", \"skill:test\"]\n\
full = [\"basic\", \"agent:*\", \"skill:deploy\"]\n";

/// [`PROFILES`] with `rule:style` dropped from `basic`.
const PROFILES_MINUS_STYLE: &str = "[profiles]\n\
default = [\"basic\"]\n\
basic = [\"skill:review\"]\n\
full = [\"basic\", \"agent:*\", \"skill:deploy\"]\n";

// ----- PROF-21: additions -----

#[test]
fn upgrade_installs_a_new_profile_member_and_snapshots_it() {
    // spec: PROF-21 -- a member the maintainer adds is listed as an addition
    // naming its profile; applying the upgrade installs and tags it.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    sb.commit_toml(PROFILES_PLUS_TEST);

    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(
        r.stdout.contains("newly in an enabled profile"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("skill:test"), "{}", r.stdout);
    assert!(r.stdout.contains("(profile default)"), "{}", r.stdout);
    assert_eq!(
        sb.installed(),
        svec(&["rule:style", "skill:review", "skill:test"])
    );
    assert_eq!(sb.reasons("skill:test"), svec(&["default"]));
    assert!(!sb.explicit("skill:test"));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review", "skill:test"]))
    );

    // Applied once: the next pass has nothing to offer.
    let again = sb.ok(&["upgrade", "--yes"]);
    assert!(
        again.stdout.contains("everything is up to date"),
        "{}",
        again.stdout
    );
}

#[test]
fn addition_pulls_in_its_dependency_closure() {
    // spec: PROF-21 PROF-6 -- an addition installs through the learn path, so
    // its closure comes with it and carries the member's profile.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\", \"skill:deploy\"]\n\
         full = [\"basic\", \"agent:*\"]\n",
    );
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("plus 1 dependency item"), "{}", r.stdout);
    assert!(sb.installed().contains(&"skill:helper".to_string()));
    assert_eq!(sb.reasons("skill:deploy"), svec(&["default"]));
    assert_eq!(sb.reasons("skill:helper"), svec(&["default"]));
    let snap = sb.recorded_snapshot().unwrap();
    assert!(snap.contains(&"skill:deploy".to_string()), "{snap:?}");
    assert!(
        !snap.contains(&"skill:helper".to_string()),
        "a closure item is not a member: {snap:?}"
    );
}

#[test]
fn declined_addition_stays_out_of_the_snapshot_and_is_offered_again() {
    // spec: PROF-21 -- a declined addition is not installed, stays out of the
    // snapshot, and the next upgrade offers it again.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_PLUS_TEST);

    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("n\n"));
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("skill:test"), "{}", r.stdout);
    assert!(r.stdout.contains("aborted"), "{}", r.stdout);
    assert!(sb.manifest_item("skill:test").is_none());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );

    let again = sb.ok(&["upgrade", "--yes"]);
    assert!(again.stdout.contains("skill:test"), "{}", again.stdout);
    assert!(sb.manifest_item("skill:test").is_some());
}

#[test]
fn non_interactive_upgrade_without_yes_installs_no_addition() {
    // spec: PROF-21 -- the addition is under upgrade's gate: EOF on stdin is a
    // decline, so nothing installs and nothing is snapshotted.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_PLUS_TEST);
    let r = sb.mind(&["upgrade"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.manifest_item("skill:test").is_none());
    assert!(
        !sb.recorded_snapshot()
            .unwrap()
            .contains(&"skill:test".to_string())
    );
}

#[test]
fn addition_already_installed_is_snapshotted_and_tagged_not_proposed() {
    // spec: PROF-21 PROF-20 -- an item the consumer already learned by name
    // that joins a profile is recorded, not offered; it is marked explicit so
    // disabling the profile later keeps it.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["learn", "skill:test"]);
    assert!(sb.reasons("skill:test").is_empty());
    sb.commit_toml(PROFILES_PLUS_TEST);

    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(
        !r.stdout.contains("newly in an enabled profile"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("everything is up to date"),
        "{}",
        r.stdout
    );
    assert_eq!(sb.reasons("skill:test"), svec(&["default"]));
    assert!(sb.explicit("skill:test"));
    assert!(
        sb.recorded_snapshot()
            .unwrap()
            .contains(&"skill:test".to_string())
    );
}

// ----- PROF-22: departures -----

#[test]
fn departure_is_reported_once_never_uninstalled_and_names_forget() {
    // spec: PROF-22 CLI-225 -- a key that left the expansion is reported once,
    // removed from the snapshot, left installed, and the report names a
    // shell-quoted `mind forget <key>`.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_MINUS_STYLE);

    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(
        r.stdout.contains("rule:style left profile default"),
        "{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("`mind forget 'rule:style'`"),
        "{}",
        r.stdout
    );
    assert!(
        sb.manifest_item("rule:style").is_some(),
        "never uninstalled"
    );
    assert_eq!(sb.recorded_snapshot(), Some(svec(&["skill:review"])));

    let again = sb.ok(&["upgrade", "--yes"]);
    assert!(!again.all().contains("left profile"), "{}", again.all());
}

#[test]
fn departure_of_an_explicit_item_does_not_suggest_forget() {
    // spec: PROF-22 -- `mind forget` is named only for an item installed for
    // profiles alone; one also learned by name is not.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["learn", "rule:style"]);
    assert!(sb.explicit("rule:style"));
    sb.commit_toml(PROFILES_MINUS_STYLE);
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("rule:style left profile"), "{}", r.stdout);
    assert!(!r.stdout.contains("mind forget"), "{}", r.stdout);
}

#[test]
fn departure_is_persisted_even_when_the_upgrade_is_declined() {
    // spec: PROF-22 PROF-21 -- the departure leaves the snapshot on a declined
    // pass, while the declined addition stays out of it.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.mind_env(&["upgrade"], &[("MIND_TTY", "1")], Some("n\n"));
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("rule:style left profile"), "{}", r.stdout);
    assert_eq!(sb.recorded_snapshot(), Some(svec(&["skill:review"])));
    assert!(sb.manifest_item("skill:test").is_none());
}

// ----- PROF-23: forgotten members -----

#[test]
fn forgotten_member_is_not_offered_again_only_new_members_are() {
    // spec: PROF-23 -- a member the consumer forgot stays in the snapshot, so
    // upgrade does not re-propose it; a member added later is offered.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["forget", "skill:review"]);
    assert!(
        sb.recorded_snapshot()
            .unwrap()
            .contains(&"skill:review".to_string())
    );

    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(
        r.stdout.contains("everything is up to date"),
        "{}",
        r.stdout
    );
    assert!(sb.manifest_item("skill:review").is_none());

    sb.commit_toml(PROFILES_PLUS_TEST);
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("skill:test"), "{}", r.stdout);
    assert!(!r.stdout.contains("skill:review"), "{}", r.stdout);
    assert!(sb.manifest_item("skill:review").is_none());
    assert!(sb.manifest_item("skill:test").is_some());
}

// ----- PROF-24: undeclared profiles -----

#[test]
fn undeclared_enabled_profile_is_dropped_with_a_warning() {
    // spec: PROF-24 -- `full` vanishes from the table: it leaves the recorded
    // set with a warning naming it, and its former members are reported as
    // departures (never uninstalled).
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\"]\n",
    );
    let r = sb.ok(&["upgrade", "--yes"]);
    let all = r.all();
    assert!(
        all.contains("profile 'full' is no longer declared"),
        "{all}"
    );
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert!(all.contains("agent:planner left profile full"), "{all}");
    assert!(all.contains("skill:deploy left profile full"), "{all}");
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
    assert!(sb.manifest_item("agent:planner").is_some());
    assert!(sb.manifest_item("skill:deploy").is_some());
}

// ----- PROF-5 at upgrade -----

#[test]
fn exact_member_naming_no_item_is_bad_reference_at_upgrade() {
    // spec: PROF-5 -- an exact member the source does not offer fails the
    // upgrade with BadReference naming it; nothing is installed or recorded.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\", \"skill:nope\"]\n",
    );
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("skill:nope"), "{}", r.stderr);
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

// ----- PROF-20: reasons survive a content upgrade -----

#[test]
fn content_upgrade_keeps_profile_reasons_and_explicit() {
    // spec: PROF-20 -- the reinstall builds a fresh manifest entry; the
    // upgrade carries the old entry's reasons and explicit mark over.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["learn", "rule:style"]);
    assert!(sb.explicit("rule:style"));
    let old_hash = sb.manifest_item("skill:review").unwrap()["hash"].clone();
    sb.commit_file(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: review skill\n---\n# review v2\n",
    );
    sb.commit_file(
        "rules/style.md",
        "---\ndescription: Style rule\n---\n# style v2\n",
    );
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("upgraded"), "{}", r.stdout);
    let item = sb.manifest_item("skill:review").unwrap();
    assert_ne!(item["hash"], old_hash, "the content was upgraded");
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
    assert!(!sb.explicit("skill:review"));
    assert_eq!(sb.reasons("rule:style"), svec(&["default"]));
    assert!(sb.explicit("rule:style"), "explicit survives the upgrade");
}

#[test]
fn rename_upgrade_keeps_profile_reasons() {
    // spec: PROF-20 -- a prefix change re-keys the item; its reasons move to
    // the new key.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(&format!("[source]\nprefix = \"jk\"\n\n{PROFILES}"));
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("skill:jk:review"), "{}", r.stdout);
    assert!(sb.manifest_item("skill:review").is_none());
    assert_eq!(sb.reasons("skill:jk:review"), svec(&["default"]));
}

// ----- --json -----

#[test]
fn json_upgrade_reports_additions_and_departures() {
    // spec: PROF-21 PROF-22 -- the `--json` result carries both arrays.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.ok(&["upgrade", "--yes", "--json"]);
    let doc = json_doc(&r);
    let adds = doc["profile_additions"].as_array().expect("additions");
    assert_eq!(adds.len(), 1, "{doc}");
    assert_eq!(adds[0]["key"], "skill:test");
    assert_eq!(adds[0]["profiles"], serde_json::json!(["default"]));
    let deps = doc["profile_departures"].as_array().expect("departures");
    assert_eq!(deps.len(), 1, "{doc}");
    assert_eq!(deps[0]["key"], "rule:style");
    assert_eq!(deps[0]["installed"], true);
    assert_eq!(deps[0]["forget"], "mind forget 'rule:style'");
    assert!(
        doc["installed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "skill:test"),
        "{doc}"
    );
}

#[test]
fn json_upgrade_without_yes_refuses_an_addition_but_persists_departures() {
    // spec: PROF-21 PROF-22 LIFE-45 -- `--json` is non-interactive: an
    // addition needs --yes; the departure was already recorded.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.mind(&["upgrade", "--json"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(sb.manifest_item("skill:test").is_none());
    assert_eq!(sb.recorded_snapshot(), Some(svec(&["skill:review"])));
}

#[test]
fn untracked_source_is_not_profile_tracked_at_upgrade() {
    // spec: PROF-21 -- a source installed by the install-all offer (no
    // `default`) records nothing and upgrade proposes no additions for it.
    let sb = Sandbox::with_toml("plain", "[profiles]\nbasic = [\"skill:review\"]\n");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml("[profiles]\nbasic = [\"skill:review\", \"skill:test\"]\n");
    let r = sb.ok(&["upgrade", "--yes", "--json"]);
    let doc = json_doc(&r);
    assert!(doc.get("profile_additions").is_none(), "{doc}");
    assert!(doc.get("profile_departures").is_none(), "{doc}");
}

// ----- PROF-25: forget <source> --profile -----

#[test]
fn forget_profile_removes_items_installed_only_for_it() {
    // spec: PROF-25 -- disabling `full` forgets its own members and closure,
    // keeps the items `default` still covers (reasons stripped), and
    // recomputes the snapshot.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert_eq!(
        sb.installed(),
        svec(&[
            "agent:planner",
            "rule:style",
            "skill:deploy",
            "skill:helper",
            "skill:review"
        ])
    );
    let r = sb.ok(&["forget", &sb.identity(), "--profile", "full", "--yes"]);
    assert!(r.stdout.contains("disabled profile 'full'"), "{}", r.stdout);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

#[test]
fn forget_profile_uses_the_glob_gate() {
    // spec: PROF-25 CLI-42 LIFE-45 -- removing several items without --yes
    // previews and refuses off a TTY (and under --json); nothing changes.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    let before = sb.installed();
    let r = sb.mind(&["forget", &sb.identity(), "--profile", "full"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stdout.contains("would remove 3 item(s)"), "{}", r.stdout);
    let r = sb.mind(&["forget", &sb.identity(), "--profile", "full", "--json"]);
    assert!(!r.success, "{}", r.stdout);
    assert_eq!(sb.installed(), before);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));

    // A TTY decline changes nothing either.
    let r = sb.mind_env(
        &["forget", &sb.identity(), "--profile", "full"],
        &[("MIND_TTY", "1")],
        Some("n\n"),
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed(), before);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
}

#[test]
fn forget_profile_keeps_explicit_items_and_their_dependencies() {
    // spec: PROF-25 DEP-60 PROF-20 -- skill:deploy was learned by name, so it
    // stays; skill:helper is only for `full` but deploy depends on it, so it
    // is kept and named.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    sb.ok(&["learn", "skill:deploy"]);
    assert!(sb.explicit("skill:deploy"));
    let r = sb.ok(&["forget", &sb.identity(), "--profile", "full", "--yes"]);
    assert!(
        r.stdout
            .contains("kept skill:helper (needed by skill:deploy)"),
        "{}",
        r.stdout
    );
    assert_eq!(
        sb.installed(),
        svec(&["rule:style", "skill:deploy", "skill:helper", "skill:review"])
    );
    assert!(sb.reasons("skill:deploy").is_empty(), "full stripped");
    assert!(sb.explicit("skill:deploy"));

    let r = sb.ok(&[
        "forget",
        &sb.identity(),
        "--profile",
        "default",
        "--yes",
        "--json",
    ]);
    let doc = json_doc(&r);
    assert_eq!(doc["outcome"], "disabled");
    // skill:helper keeps its stale `full` reason, so it is still a candidate,
    // and still kept for its dependent; --json names it in `kept`.
    assert_eq!(
        doc["kept"],
        serde_json::json!(["skill:helper (needed by skill:deploy)"])
    );
    assert_eq!(
        sb.installed(),
        svec(&["skill:deploy", "skill:helper"]),
        "default's own items are gone"
    );
}

#[test]
fn forget_profile_default_untracks_the_source() {
    // spec: PROF-25 -- `--profile default` disables default; with no profile
    // left both registry fields are cleared.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let r = sb.ok(&[
        "forget",
        &sb.identity(),
        "--profile",
        "default",
        "--yes",
        "--json",
    ]);
    let doc = json_doc(&r);
    assert_eq!(doc["outcome"], "disabled");
    let mut removed = strings(&doc["removed"]).unwrap();
    removed.sort();
    assert_eq!(removed, svec(&["rule:style", "skill:review"]));
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);
    assert_eq!(sb.recorded_snapshot(), None);
}

#[test]
fn forget_profile_works_when_the_remaining_table_is_broken() {
    // spec: PROF-25 PROF-5 -- the remaining set's expansion hits an exact
    // member naming no item: disabling still works, warns naming the error,
    // forgets per the plan, and leaves the snapshot unchanged.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    let snapshot_before = sb.recorded_snapshot();
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\", \"skill:nope\"]\n\
         full = [\"basic\", \"agent:*\", \"skill:deploy\"]\n",
    );
    sb.ok(&["sync"]);
    let r = sb.ok(&["forget", &sb.identity(), "--profile", "full", "--yes"]);
    let all = r.all();
    assert!(all.contains("could not expand"), "{all}");
    assert!(all.contains("skill:nope"), "{all}");
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert_eq!(sb.recorded_snapshot(), snapshot_before);
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
}

#[test]
fn forget_profile_not_enabled_changes_nothing() {
    // spec: PROF-25 -- a profile that is not enabled is a clear no-op.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let r = sb.ok(&["forget", &sb.identity(), "--profile", "full", "--yes"]);
    assert!(r.stdout.contains("'full' is not enabled"), "{}", r.stdout);
    assert!(r.stdout.contains("nothing changed"), "{}", r.stdout);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));

    let bad = sb.mind(&["forget", &sb.identity(), "--profile", "Bad", "--yes"]);
    assert!(!bad.success);
    assert!(
        bad.stderr.contains("invalid profile name"),
        "{}",
        bad.stderr
    );
    let nosrc = sb.mind(&["forget", "nope/none", "--profile", "full", "--yes"]);
    assert!(!nosrc.success);
}

// ----- PROF-29: display -----

#[test]
fn recall_sources_lists_declared_profiles_and_marks_enabled() {
    // spec: PROF-29
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let r = sb.ok(&["recall", "--sources"]);
    assert!(r.stdout.contains("profiles:"), "{}", r.stdout);
    assert!(
        r.stdout.contains("basic, default (enabled), full"),
        "{}",
        r.stdout
    );

    let doc = json_doc(&sb.ok(&["recall", "--sources", "--json"]));
    let src = &doc["items"][0];
    assert_eq!(
        src["profiles"]["basic"],
        serde_json::json!(["skill:review", "rule:style"])
    );
    assert_eq!(src["profiles"]["default"], serde_json::json!(["basic"]));
    assert_eq!(src["enabled_profiles"], serde_json::json!(["default"]));
}

#[test]
fn source_without_profiles_shows_no_profile_noise() {
    // spec: PROF-29 -- nothing new for a source that declares none.
    let sb = Sandbox::with_toml("plain", "");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let r = sb.ok(&["recall", "--sources"]);
    assert!(!r.stdout.contains("profiles:"), "{}", r.stdout);
    assert!(!r.stdout.contains("(enabled)"), "{}", r.stdout);
    let doc = json_doc(&sb.ok(&["recall", "--sources", "--json"]));
    assert!(doc["items"][0].get("profiles").is_none(), "{doc}");
    assert!(doc["items"][0].get("enabled_profiles").is_none(), "{doc}");
    let p = sb.ok(&["probe", "--no-tui"]);
    assert!(!p.stdout.contains("profiles:"), "{}", p.stdout);
    let doc = json_doc(&sb.ok(&["probe", "--json"]));
    assert!(doc["items"][0].get("profiles").is_none(), "{doc}");
}

#[test]
fn probe_lists_declared_profiles_and_marks_enabled() {
    // spec: PROF-29
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    let r = sb.ok(&["probe", "--no-tui"]);
    assert!(
        r.stdout
            .contains("basic, default (enabled), full (enabled)"),
        "{}",
        r.stdout
    );
    let doc = json_doc(&sb.ok(&["probe", "--json"]));
    let row = &doc["items"][0];
    assert_eq!(
        row["profiles"]["full"],
        serde_json::json!(["basic", "agent:*", "skill:deploy"])
    );
    assert_eq!(
        row["enabled_profiles"],
        serde_json::json!(["default", "full"])
    );
}

#[test]
fn recall_item_names_its_profiles() {
    // spec: PROF-29 -- `recall <item>` names the profiles it was installed
    // for, in text and under --json.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    let r = sb.ok(&["recall", "skill:review"]);
    assert!(r.stdout.contains("  profile default, full"), "{}", r.stdout);
    let doc = json_doc(&sb.ok(&["recall", "skill:review", "--json"]));
    assert_eq!(doc["profiles"], serde_json::json!(["default", "full"]));

    // An item installed by name names none.
    sb.ok(&["learn", "skill:test"]);
    let r = sb.ok(&["recall", "skill:test"]);
    assert!(!r.stdout.contains("  profile "), "{}", r.stdout);
}

// ----- Outside-in additions (certification pass) -----

/// A second, unrelated source `name` (no profiles) registered into `sb`'s
/// homes with `--register-only`, so it never collides on installed keys.
fn register_other(sb: &Sandbox, name: &str, _mind_toml: &str) -> PathBuf {
    let dir = sb.base.join(format!("o-{name}")).join(name);
    write_file(
        &dir.join(format!("skills/only-{name}/SKILL.md")),
        &format!("---\nname: only-{name}\ndescription: only {name}\n---\n# only\n"),
    );
    git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "initial"]);
    sb.ok(&["meld", &dir.to_string_lossy(), "--register-only"]);
    dir
}

#[test]
fn sync_upgrade_scoped_to_the_source_tracks_profiles() {
    // spec: PROF-21 PROF-22 -- `sync <source> --upgrade` runs the same
    // source-scoped pass, so an addition installs and a departure is recorded.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.ok(&["sync", &sb.identity(), "--upgrade", "--yes"]);
    assert!(r.stdout.contains("skill:test"), "{}", r.all());
    assert!(r.all().contains("rule:style left profile"), "{}", r.all());
    assert!(sb.manifest_item("skill:test").is_some());
    assert_eq!(sb.reasons("skill:test"), svec(&["default"]));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["skill:review", "skill:test"]))
    );
}

#[test]
fn sync_upgrade_scoped_to_another_source_leaves_this_one_untracked_for_now() {
    // spec: PROF-21 -- a source outside the `sync <filter>` scope is not
    // re-expanded: its addition stays pending, snapshot unchanged.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let _other = register_other(&sb, "other", "");
    sb.commit_toml(PROFILES_PLUS_TEST);
    sb.ok(&["sync", "other", "--upgrade", "--yes"]);
    assert!(sb.manifest_item("skill:test").is_none());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
    // The unscoped pass then picks it up.
    sb.ok(&["upgrade", "--yes"]);
    assert!(sb.manifest_item("skill:test").is_some());
}

#[test]
fn upgrade_source_tracks_the_named_source_profiles() {
    // spec: PROF-21 PROF-22 CLI-244 -- `upgrade --source <filter>` runs the
    // same source-scoped pass as `sync <filter> --upgrade`.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.ok(&["upgrade", "--source", &sb.identity(), "--yes"]);
    assert!(r.stdout.contains("skill:test"), "{}", r.all());
    assert!(r.all().contains("rule:style left profile"), "{}", r.all());
    assert!(sb.manifest_item("skill:test").is_some());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["skill:review", "skill:test"]))
    );
}

#[test]
fn upgrade_source_scoped_to_another_source_leaves_this_one_untracked() {
    // spec: PROF-21 CLI-244 -- a source outside `--source` is not re-expanded.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let _other = register_other(&sb, "other", "");
    sb.commit_toml(PROFILES_PLUS_TEST);
    sb.ok(&["upgrade", "--source", "other", "--yes"]);
    assert!(sb.manifest_item("skill:test").is_none());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

#[test]
fn upgrade_source_with_item_does_not_track_profiles() {
    // spec: PROF-21 CLI-244 -- an `[item]` narrows `--source` to named items,
    // so an addition (an item it did not name) is not installed.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_PLUS_TEST);
    sb.ok(&[
        "upgrade",
        "skill:review",
        "--source",
        &sb.identity(),
        "--yes",
    ]);
    assert!(sb.manifest_item("skill:test").is_none());
}

#[test]
fn item_scoped_upgrade_does_not_track_profiles() {
    // spec: PROF-21 -- `upgrade <item>` names an item; an addition is by
    // definition one it did not name, and departures are not recorded either.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"skill:test\"]\n",
    );
    let r = sb.ok(&["upgrade", "skill:review", "--yes"]);
    assert!(sb.manifest_item("skill:test").is_none(), "{}", r.all());
    assert!(!r.all().contains("left profile"), "{}", r.all());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

#[test]
fn locked_allowlist_policy_skips_tracking_for_a_blocked_source() {
    // spec: PROF-21 -- a profile-tracked source the locked allowlist no longer
    // permits is not re-expanded: nothing installs, nothing is recorded.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_PLUS_TEST);
    let policy = sb.base.join("policy-block.toml");
    write_file(
        &policy,
        "[sources]\nlock = true\nallow = [\"local/*/nothing-like-this\"]\n",
    );
    let p = policy.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["upgrade", "--yes", "--no-sync"],
        &[("MIND_POLICY_FILE", p.as_str())],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.manifest_item("skill:test").is_none(), "{}", r.all());
    assert!(
        !r.all().contains("newly in an enabled profile"),
        "{}",
        r.all()
    );
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
}

#[test]
fn locked_allowlist_policy_admitting_the_source_still_tracks() {
    // spec: PROF-21 -- control for the blocked case: an allowed source tracks.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(PROFILES_PLUS_TEST);
    let policy = sb.base.join("policy-allow.toml");
    write_file(
        &policy,
        "[sources]\nlock = true\nallow = [\"local/*/src\"]\n",
    );
    let p = policy.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["upgrade", "--yes"],
        &[("MIND_POLICY_FILE", p.as_str())],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.manifest_item("skill:test").is_some(), "{}", r.all());
}

#[test]
fn unscannable_profiled_source_is_skipped_not_a_bad_reference() {
    // spec: PROF-21 PROF-5 -- with its clone gone the source contributes no
    // catalog items; tracking must skip it rather than expand against an empty
    // catalog (which would fail every exact member as BadReference).
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let _other = register_other(&sb, "other", "");
    std::fs::remove_dir_all(&sb.source).unwrap();
    let r = sb.mind(&["upgrade", "--yes", "--no-sync"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.all().contains("could not check"), "{}", r.all());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
}

#[test]
fn failed_addition_keeps_earlier_members_and_is_offered_again() {
    // spec: PROF-21 PROF-19 -- an addition whose install fails partway: the
    // upgrade errors, the failed member stays out of the snapshot, and the next
    // pass (after the maintainer fixes it) installs what is missing.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_file(
        "skills/alpha/SKILL.md",
        "---\nname: alpha\ndescription: alpha\n---\n# alpha\n",
    );
    sb.commit_file(
        "skills/broken/SKILL.md",
        "---\nname: broken\ndescription: broken\n---\n# broken\nSee {{ns:ghost}}.\n",
    );
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\", \"skill:alpha\", \"skill:broken\"]\n",
    );
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        !r.success,
        "a bad reference must fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(r.all().contains("ghost"), "{}", r.all());
    assert!(sb.manifest_item("skill:broken").is_none());
    let snap = sb.recorded_snapshot().unwrap();
    assert!(!snap.contains(&"skill:broken".to_string()), "{snap:?}");
    // Whatever landed is snapshotted and tagged; whatever did not is not.
    let alpha_in = sb.manifest_item("skill:alpha").is_some();
    assert_eq!(
        snap.contains(&"skill:alpha".to_string()),
        alpha_in,
        "{snap:?}"
    );
    if alpha_in {
        assert_eq!(sb.reasons("skill:alpha"), svec(&["default"]));
    }

    sb.commit_file(
        "skills/broken/SKILL.md",
        "---\nname: broken\ndescription: broken\n---\n# broken\n",
    );
    sb.ok(&["upgrade", "--yes"]);
    assert!(sb.manifest_item("skill:broken").is_some());
    assert!(sb.manifest_item("skill:alpha").is_some());
    assert_eq!(sb.reasons("skill:broken"), svec(&["default"]));
}

#[test]
fn addition_and_a_pending_rename_in_one_pass() {
    // spec: PROF-21 PROF-20 -- a prefix change (rename of every installed item)
    // and a new member in the same pass: the renamed items keep their reasons,
    // and the addition installs under the new effective name.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(&format!(
        "[source]\nprefix = \"jk\"\n\n{PROFILES_PLUS_TEST}"
    ));
    let r = sb.ok(&["upgrade", "--yes"]);
    assert!(r.stdout.contains("skill:jk:review"), "{}", r.stdout);
    assert_eq!(
        sb.installed(),
        svec(&["rule:jk:style", "skill:jk:review", "skill:jk:test"])
    );
    assert_eq!(sb.reasons("skill:jk:review"), svec(&["default"]));
    assert_eq!(sb.reasons("rule:jk:style"), svec(&["default"]));
    assert_eq!(sb.reasons("skill:jk:test"), svec(&["default"]));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review", "skill:test"]))
    );
    let again = sb.ok(&["upgrade", "--yes"]);
    assert!(
        again.stdout.contains("everything is up to date"),
        "{}",
        again.stdout
    );
}

#[test]
fn curate_scoped_upgrade_pass_records_a_departure() {
    // spec: PROF-22 PROF-27 -- curate's source-scoped upgrade pass tracks the
    // curated source: a member the maintainer drops is reported and leaves the
    // snapshot, while the item stays installed.
    let sb = Sandbox::new("src");
    let curator = sb.base.join("curator");
    write_file(
        &curator.join("mind.toml"),
        &format!(
            "[discover]\nsources = [\n{{ source = \"{}\" }}\n]\n",
            sb.spec()
        ),
    );
    git(&curator, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&curator, &["config", "user.email", "t@t"]);
    git(&curator, &["config", "user.name", "t"]);
    git(&curator, &["add", "-A"]);
    git(&curator, &["commit", "-qm", "initial"]);
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.ok(&["meld", &curator.to_string_lossy(), "--register-only"]);
    sb.ok(&["curate", "--adopt", &sb.identity(), "--yes"]);
    // curate only runs its upgrade pass for a source with out-of-date items,
    // so the maintainer also edits an installed item.
    sb.commit_file(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: review skill\n---\n# review v2\n",
    );
    sb.commit_toml(PROFILES_MINUS_STYLE);
    let r = sb.ok(&["curate", "--yes"]);
    assert!(r.all().contains("rule:style left profile"), "{}", r.all());
    assert!(sb.manifest_item("rule:style").is_some());
    assert_eq!(sb.recorded_snapshot(), Some(svec(&["skill:review"])));
}

// ----- forget --profile edge cases -----

#[test]
fn forget_profile_with_an_ambiguous_selector_is_refused() {
    // spec: PROF-25 -- a selector matching two sources fails; nothing changes.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    let _twin = register_other(&sb, "src", "");
    let before = sb.installed();
    let r = sb.mind(&["forget", "src", "--profile", "full", "--yes"]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.to_lowercase().contains("ambiguous") || r.stderr.contains("matches"),
        "{}",
        r.stderr
    );
    assert_eq!(sb.installed(), before);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
}

#[test]
fn forget_profile_selector_with_a_fragment_is_rejected() {
    // spec: PROF-25 -- the selector is a source selector with no `#` part.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let before = sb.installed();
    let r = sb.mind(&[
        "forget",
        "src#skills/review",
        "--profile",
        "default",
        "--yes",
    ]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed(), before);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
}

#[test]
fn forget_profile_on_an_item_link_source_is_a_no_op() {
    // spec: PROF-25 PROF-8 -- an item-link instance records no profiles, so
    // disabling one is the clear "not enabled" no-op and removes nothing.
    let sb = Sandbox::new("src");
    let link = format!("file://{}/tree/main/skills/review", sb.spec());
    sb.ok(&["learn", &link]);
    let installed = sb.installed();
    assert!(!installed.is_empty());
    let doc = json_doc(&sb.ok(&["recall", "--sources", "--json"]));
    let name = doc["items"][0]["name"].as_str().unwrap().to_string();
    assert!(name.contains('#'), "an item-link instance: {name}");
    // The plain selector (no `#`) cannot name the instance, and the exact
    // identity carries a `#`, which this verb rejects: either way it fails
    // cleanly and removes nothing.
    let plain = sb.mind(&["forget", "src", "--profile", "default", "--yes"]);
    assert!(!plain.success, "{} {}", plain.stdout, plain.stderr);
    let exact = sb.mind(&["forget", &name, "--profile", "default", "--yes"]);
    assert!(!exact.success, "{} {}", exact.stdout, exact.stderr);
    assert_eq!(sb.installed(), installed);
}

#[test]
fn forget_profile_uninstall_hook_failure_rolls_back_and_saves_the_manifest() {
    // spec: PROF-25 LIFE-48 -- an uninstall hook that fails stops the pass: the
    // failing item is put back in the manifest, earlier removals stay saved, and
    // the source's recorded profiles are not changed.
    let sb = Sandbox::new("src");
    sb.commit_file(
        "agents/planner.md",
        "---\nname: planner\ndescription: Planner agent\nuninstall: exit 1\n---\n# planner\n",
    );
    sb.ok(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert!(sb.manifest_item("agent:planner").is_some());
    let r = sb.mind(&[
        "forget",
        &sb.identity(),
        "--profile",
        "full",
        "--yes",
        "--dangerously-skip-hook-check",
    ]);
    assert!(
        !r.success,
        "hook failure must fail the command: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.all().to_lowercase().contains("hook"),
        "the failure is the hook's: {}",
        r.all()
    );
    assert!(
        sb.manifest_item("agent:planner").is_some(),
        "the item whose hook failed stays recorded"
    );
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    // The manifest on disk is valid and consistent with the files.
    for key in sb.installed() {
        let item = sb.manifest_item(&key).unwrap();
        let store = item["store"].as_str().unwrap();
        assert!(sb.mind_home.join(store).exists(), "{key} store exists");
    }
}

#[test]
fn forget_last_profile_works_even_when_the_table_is_broken() {
    // spec: PROF-25 PROF-5 -- disabling the only enabled profile leaves an empty
    // remaining set, so the (now broken) table is never expanded: no warning,
    // items removed, both registry fields cleared.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml(
        "[profiles]\n\
         default = [\"basic\"]\n\
         basic = [\"skill:review\", \"rule:style\", \"skill:nope\"]\n",
    );
    sb.ok(&["sync"]);
    let r = sb.ok(&["forget", &sb.identity(), "--profile", "default", "--yes"]);
    assert!(!r.all().contains("could not expand"), "{}", r.all());
    assert!(sb.installed().is_empty(), "{}", r.all());
    assert_eq!(sb.recorded_profiles(), None);
    assert_eq!(sb.recorded_snapshot(), None);
}

#[test]
fn forget_last_profile_after_the_table_is_removed_entirely() {
    // spec: PROF-25 -- the maintainer deleted the whole [profiles] table; the
    // consumer can still disable the recorded profile and clean up.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    sb.commit_toml("[source]\ndescription = \"no profiles now\"\n");
    sb.ok(&["sync"]);
    sb.ok(&["forget", &sb.identity(), "--profile", "default", "--yes"]);
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);
}

// ----- display: probe / json compatibility -----

#[test]
fn probe_source_filter_excludes_the_profiled_source_and_its_block() {
    // spec: PROF-29 -- `probe --no-tui --source <other>` lists only that
    // source, so the profiled source's `profiles:` block does not appear.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let _other = register_other(&sb, "other", "");
    let r = sb.ok(&["probe", "--no-tui", "--source", "other"]);
    assert!(!r.stdout.contains("profiles:"), "{}", r.stdout);
    assert!(!r.stdout.contains("(enabled)"), "{}", r.stdout);
    let all = sb.ok(&["probe", "--no-tui"]);
    assert!(all.stdout.contains("profiles:"), "{}", all.stdout);
    assert!(all.stdout.contains("default (enabled)"), "{}", all.stdout);
}

#[test]
fn probe_json_rows_of_a_profileless_source_have_no_new_keys() {
    // spec: PROF-29 -- byte-compatible rows for a source without profiles, even
    // next to a profiled source whose rows do carry the new keys.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    let _other = register_other(&sb, "other", "");
    let doc = json_doc(&sb.ok(&["probe", "--json"]));
    let rows = doc["items"].as_array().expect("items");
    let legacy = [
        "installed",
        "kind",
        "name",
        "source",
        "hash",
        "description",
        "when_to_use",
        "unmanaged",
        "dependencies",
    ];
    let mut plain = 0;
    let mut profiled = 0;
    for row in rows {
        let obj = row.as_object().unwrap();
        if row["source"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/other")
        {
            plain += 1;
            for k in obj.keys() {
                assert!(
                    legacy.contains(&k.as_str()),
                    "new key {k} on a plain row: {row}"
                );
            }
        } else if row["source"] == sb.identity().as_str() {
            profiled += 1;
            assert!(
                obj.contains_key("profiles") && obj.contains_key("enabled_profiles"),
                "{row}"
            );
        }
    }
    assert!(plain > 0 && profiled > 0, "{doc}");
}

#[test]
fn probe_json_unmanaged_rows_carry_no_profile_keys() {
    // spec: PROF-29 -- an unmanaged lobe item has no source, so no profile keys.
    let sb = Sandbox::new("src");
    sb.ok(&["meld", &sb.spec(), "--yes"]);
    write_file(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\ndescription: by hand\n---\n# handmade\n",
    );
    let doc = json_doc(&sb.ok(&["probe", "--json"]));
    let rows = doc["items"].as_array().unwrap();
    let um: Vec<&Value> = rows.iter().filter(|r| r["unmanaged"] == true).collect();
    assert!(!um.is_empty(), "{doc}");
    for r in um {
        assert!(
            r.get("profiles").is_none() && r.get("enabled_profiles").is_none(),
            "{r}"
        );
    }
}
