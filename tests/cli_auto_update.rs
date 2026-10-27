//! Integration tests for auto-update (spec/auto-update.md).
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
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-auto-{}-{n}", std::process::id()));
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

    /// Commit `agent` as the `dev` agent's whole file plus `mind_toml` as the
    /// root `mind.toml`, and tag it when `tag` is given. Returns the sha.
    fn commit(&self, agent: &str, mind_toml: &str, tag: Option<&str>) -> String {
        write_file(&self.source.join("agents/dev.md"), agent);
        write_file(&self.source.join("mind.toml"), mind_toml);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "c"]);
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.source)
            .output()
            .unwrap();
        let sha = String::from_utf8(out.stdout).unwrap().trim().to_string();
        if let Some(tag) = tag {
            git(&self.source, &["tag", tag, &sha]);
        }
        sha
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
        self.mind_env(args, &[], None)
    }

    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_TTY")
            .env_remove("MIND_POLICY_FILE")
            .env_remove("MIND_AGENT_HOMES")
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
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().expect("wait for mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn entry(&self) -> Value {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        v["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"].as_str() == Some(self.identity().as_str()))
            .cloned()
            .expect("source is melded")
    }

    fn installed_agent(&self) -> String {
        std::fs::read_to_string(self.claude_home.join("agents/dev.md")).unwrap_or_default()
    }

    fn state(&self) -> Value {
        std::fs::read_to_string(self.mind_home.join("auto-update.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null)
    }

    fn policy(&self, body: &str) -> String {
        let file = self.base.join("policy.toml");
        write_file(&file, body);
        file.to_string_lossy().into_owned()
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

fn agent(body: &str) -> String {
    format!("---\nname: dev\ndescription: dev\n---\n{body}\n")
}

fn toml(extra: &str) -> String {
    format!("[source]\nrelease-tags = \"v{{version}}\"\n{extra}")
}

/// A source at v1.0.0 melded `^1` with every item installed and `grant`
/// passed to `--auto-update` (none when empty).
fn melded(name: &str, declare: &str, grant: &str) -> Sandbox {
    let sb = Sandbox::new(name);
    sb.commit(&agent("one-zero"), &toml(declare), Some("v1.0.0"));
    let mut args = vec!["meld", "--pin", "version=^1", "--yes"];
    let spec = sb.spec();
    args.insert(1, &spec);
    if !grant.is_empty() {
        args.push(grant);
    }
    let r = sb.mind(&args);
    assert!(r.success, "meld: {}{}", r.stdout, r.stderr);
    assert!(sb.installed_agent().contains("one-zero"));
    sb
}

#[test]
fn a_declared_level_is_recorded_and_a_bad_one_fails_the_meld() {
    // spec: AUP-5
    let sb = melded("auto-declare", "auto-update = true\n", "");
    assert_eq!(sb.entry()["pin"]["value"]["auto_update"], "compatible");
    assert!(
        sb.entry().get("auto_update").is_none(),
        "declaring grants nothing"
    );

    for bad in ["false", "\"off\"", "\"sometimes\""] {
        let sb = Sandbox::new("auto-declare-bad");
        sb.commit(
            &agent("x"),
            &toml(&format!("auto-update = {bad}\n")),
            Some("v1.0.0"),
        );
        let r = sb.mind(&["meld", &sb.spec(), "--pin", "version=^1", "--yes"]);
        assert!(!r.success, "{bad}: {}", r.stdout);
        assert!(r.stderr.contains("auto-update"), "{bad}: {}", r.stderr);
    }
}

#[test]
fn meld_records_a_grant_and_an_off_revokes_it() {
    // spec: AUP-8
    let sb = melded(
        "auto-grant",
        "auto-update = \"breaking\"\n",
        "--auto-update",
    );
    assert_eq!(sb.entry()["auto_update"], "breaking", "the declared level");

    let r = sb.mind(&["meld", &sb.spec(), "--auto-update=compatible"]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(sb.entry()["auto_update"], "compatible");

    let r = sb.mind(&["meld", &sb.spec(), "--auto-update=off"]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(sb.entry()["auto_update"], "off");

    let r = sb.mind(&["meld", &sb.spec(), "--auto-update=sometimes"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("invalid --auto-update value"),
        "{}",
        r.stderr
    );

    // A bare grant on a source that declares nothing is compatible.
    let sb = melded("auto-grant-plain", "", "--auto-update");
    assert_eq!(sb.entry()["auto_update"], "compatible");
}

#[test]
fn the_consumer_grant_wins_over_the_declaration() {
    // spec: AUP-6 AUP-10
    // Breaking granted to a source that declares only compatible.
    let declare = "auto-update = \"compatible\"\n";
    let sb = melded("auto-override", declare, "--auto-update=breaking");
    assert_eq!(sb.entry()["auto_update"], "breaking");
    let r = sb.mind(&["recall", "--sources", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).unwrap();
    assert_eq!(
        v["items"][0]["auto_update"],
        serde_json::json!({"declared": "compatible", "granted": "breaking", "effective": "breaking"}),
        "{v}"
    );

    // The maintainer drops the declaration and recommends a new major: the
    // grant still takes it.
    sb.commit(&agent("two-zero"), &toml(""), Some("v2.0.0"));
    sb.commit(&agent("tip"), &toml("pin-version = \"^2\"\n"), None);
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^2");
    assert!(sb.installed_agent().contains("two-zero"));
}

#[test]
fn only_a_range_pin_can_be_granted() {
    // spec: AUP-2
    let sb = Sandbox::new("auto-linked");
    sb.commit(&agent("x"), &toml(""), Some("v1.0.0"));
    // A plain local meld stays linked to its working tree: no version pin.
    let r = sb.mind(&["meld", &sb.spec(), "--yes", "--auto-update"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout
            .contains("auto-update applies only to a version range pin"),
        "{}",
        r.stdout
    );
    assert!(sb.entry().get("auto_update").is_none());

    // An exact hold is a version pin but not a range.
    let r = sb.mind(&[
        "meld",
        &sb.spec(),
        "--pin",
        "version==1.0.0",
        "--auto-update",
    ]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("only to a version range pin"),
        "{}",
        r.stdout
    );
    assert!(sb.entry().get("auto_update").is_none());
}

#[test]
fn a_first_meld_on_a_terminal_asks_for_the_declared_level() {
    // spec: AUP-9
    let ask = |declare: &str, stdin: &str| -> (Sandbox, Run) {
        let sb = Sandbox::new("auto-ask");
        sb.commit(&agent("x"), &toml(declare), Some("v1.0.0"));
        let r = sb.mind_env(
            &["meld", &sb.spec(), "--pin", "version=^1", "--register-only"],
            &[("MIND_TTY", "1")],
            Some(stdin),
        );
        assert!(r.success, "{}{}", r.stdout, r.stderr);
        (sb, r)
    };
    // Declining breaking offers compatible, which defaults to yes.
    let (sb, r) = ask("auto-update = \"breaking\"\n", "n\n\n");
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(
        all.contains("asks to update automatically, including breaking releases [y/N]"),
        "{all}"
    );
    assert!(
        all.contains("asks to update automatically (compatible releases) [Y/n]"),
        "{all}"
    );
    assert_eq!(sb.entry()["auto_update"], "compatible");

    let (sb, _) = ask("auto-update = \"breaking\"\n", "y\n");
    assert_eq!(sb.entry()["auto_update"], "breaking");

    let (sb, _) = ask("auto-update = true\n", "n\n");
    assert!(sb.entry().get("auto_update").is_none(), "declined");

    // No terminal, or --yes: nothing is granted and nothing is asked.
    let sb = melded("auto-ask-none", "auto-update = true\n", "");
    assert!(sb.entry().get("auto_update").is_none());
}

#[test]
fn recall_shows_the_level_and_notes_a_raised_declaration() {
    // spec: AUP-10 AUP-11
    let sb = melded(
        "auto-recall",
        "auto-update = \"breaking\"\n",
        "--auto-update=compatible",
    );
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.stdout.contains("auto-update compatible"), "{}", r.stdout);
    assert!(
        r.stdout.contains(&format!(
            "asks to update automatically, including breaking releases; allow it with `mind meld '{}' --auto-update=breaking`",
            sb.identity()
        )),
        "{}",
        r.stdout
    );
    let r = sb.mind(&["recall"]);
    assert!(r.stdout.contains("auto-update compatible"), "{}", r.stdout);

    let r = sb.mind(&["recall", "--sources", "--json"]);
    let v: Value = serde_json::from_str(r.stdout.trim()).unwrap();
    assert_eq!(
        v["items"][0]["auto_update"],
        serde_json::json!({"declared": "breaking", "granted": "compatible", "effective": "compatible"}),
        "{v}"
    );

    // Revoked: no level, and no nudge.
    assert!(sb.mind(&["meld", &sb.spec(), "--auto-update=off"]).success);
    let r = sb.mind(&["recall", "--sources"]);
    assert!(!r.stdout.contains("auto-update"), "{}", r.stdout);
}

#[test]
fn a_run_takes_a_compatible_release_without_asking() {
    // spec: AUP-1 AUP-15
    let sb = melded("auto-run", "", "--auto-update");
    let new = sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    // A terminal does not make the run interactive.
    let r = sb.mind_env(&["auto-update", "run"], &[("MIND_TTY", "1")], None);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains(&format!(
            "auto-update: moved {} 1.0.0 -> 1.1.0",
            sb.identity()
        )),
        "{}",
        r.stdout
    );
    assert_eq!(sb.entry()["commit"], new.as_str());
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^1");
    assert!(sb.installed_agent().contains("one-one"), "items upgraded");

    let r = sb.mind(&["auto-update", "run"]);
    assert!(
        r.stdout.contains("everything is up to date"),
        "{}",
        r.stdout
    );
}

#[test]
fn a_run_leaves_ungranted_sources_alone() {
    // spec: AUP-1 AUP-10
    let sb = melded("auto-ungranted", "auto-update = true\n", "");
    sb.commit(
        &agent("one-one"),
        &toml("auto-update = true\n"),
        Some("v1.1.0"),
    );
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(sb.installed_agent().contains("one-zero"));
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
}

#[test]
fn a_run_honors_yanked_releases() {
    // spec: AUP-16
    let sb = melded("auto-yanked", "", "--auto-update");
    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    sb.commit(&agent("tip"), &toml("yanked = [\"1.1.0\"]\n"), None);
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
    assert!(sb.installed_agent().contains("one-zero"));
}

#[test]
fn a_run_honors_min_mind_version_and_moved_tags() {
    // spec: AUP-16
    // A newer release the binary is too old for is skipped.
    let sb = melded("auto-min-version", "", "--auto-update");
    sb.commit(
        &agent("one-one"),
        &toml("min-mind-version = \"99.0\"\n"),
        Some("v1.1.0"),
    );
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
    assert!(sb.installed_agent().contains("one-zero"));

    // The resolved release's tag moved upstream: the run does not follow it.
    let sb = melded("auto-moved-tag", "", "--auto-update");
    let old = sb.entry()["commit"].as_str().unwrap().to_string();
    let moved = sb.commit(&agent("one-zero-moved"), &toml(""), None);
    git(&sb.source, &["tag", "-f", "v1.0.0", &moved]);
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert_eq!(sb.entry()["commit"], old.as_str());
    assert!(sb.installed_agent().contains("one-zero"));
    assert!(!sb.installed_agent().contains("moved"));
}

#[test]
fn a_run_skips_a_source_the_locked_allowlist_no_longer_admits() {
    // spec: AUP-15
    let sb = melded("auto-allowlist", "", "--auto-update");
    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    let locked = sb.policy("[sources]\nallow = [\"github.com/acme/*\"]\nlock = true\n");
    let r = sb.mind_env(
        &["auto-update", "run"],
        &[("MIND_POLICY_FILE", locked.as_str())],
        None,
    );
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
    assert!(sb.installed_agent().contains("one-zero"));
}

#[test]
fn breaking_moves_follow_the_recommended_range() {
    // spec: AUP-12 AUP-13
    let declare = "auto-update = \"breaking\"\n";
    let sb = melded("auto-major", declare, "--auto-update");
    sb.commit(&agent("two-zero"), &toml(declare), Some("v2.0.0"));

    // Tagged but not yet recommended: nothing breaking is taken.
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}", r.stderr);
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^1");
    assert!(sb.installed_agent().contains("one-zero"));

    // Recommended: the range moves to the recommendation.
    sb.commit(
        &agent("tip"),
        &toml(&format!("{declare}pin-version = \"^2\"\n")),
        None,
    );
    let r = sb.mind(&["auto-update", "run", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let v: Value = serde_json::from_str(r.stdout.trim()).unwrap();
    // spec: CLI-153
    assert_eq!(v["schema"], 1, "{v}");
    assert_eq!(v["action"], "auto-update run", "{v}");
    assert_eq!(v["outcome"], "ok", "{v}");
    assert_eq!(
        v["moved"][0],
        format!("{} 1.0.0 -> 2.0.0", sb.identity()),
        "{v}"
    );
    assert_eq!(
        v["breaking"],
        serde_json::json!([{
            "source": sb.identity(),
            "from": "1.0.0",
            "to": "2.0.0",
            "applied": true,
            "recommended": true,
        }]),
        "{v}"
    );
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^2");
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "2.0.0");
    assert!(sb.installed_agent().contains("two-zero"));
}

#[test]
fn a_compatible_grant_never_takes_a_recommended_major() {
    // spec: AUP-10 AUP-12
    let declare = "auto-update = \"breaking\"\n";
    let sb = melded("auto-major-compat", declare, "--auto-update=compatible");
    sb.commit(&agent("two-zero"), &toml(declare), Some("v2.0.0"));
    sb.commit(
        &agent("tip"),
        &toml(&format!("{declare}pin-version = \"^2\"\n")),
        None,
    );
    assert!(sb.mind(&["auto-update", "run"]).success);
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^1");
}

#[test]
fn a_move_that_would_run_a_hook_is_held() {
    // spec: AUP-17
    // An item hook on the new release.
    let sb = melded("auto-held-item", "", "--auto-update");
    let old = sb.entry()["commit"].as_str().unwrap().to_string();
    sb.commit(
        "---\nname: dev\ndescription: dev\nupdate: echo updated\n---\none-one\n",
        &toml(""),
        Some("v1.1.0"),
    );
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains(&format!(
            "auto-update: held {} 1.1.0: agent:dev has a hook that needs consent",
            sb.identity()
        )),
        "{}",
        r.stdout
    );
    assert_eq!(
        sb.entry()["commit"],
        old.as_str(),
        "the source did not move"
    );
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
    assert!(sb.installed_agent().contains("one-zero"));
    // The clone went back too, so recall sees nothing out of date.
    let r = sb.mind(&["recall"]);
    assert!(!r.stdout.contains("outdated"), "{}", r.stdout);

    // The clone goes back to the recorded commit even when the tag it was
    // resolved from has since moved upstream.
    let sb = melded("auto-held-moved-tag", "", "--auto-update");
    let old = sb.entry()["commit"].as_str().unwrap().to_string();
    let moved = sb.commit(&agent("one-zero-moved"), &toml(""), None);
    git(&sb.source, &["tag", "-f", "v1.0.0", &moved]);
    sb.commit(
        "---\nname: dev\ndescription: dev\nupdate: echo updated\n---\none-one\n",
        &toml(""),
        Some("v1.1.0"),
    );
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("held"), "{}", r.stdout);
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(sb.mind_home.join("sources").join(sb.identity()))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), old);

    // A source-level update hook on the new release.
    let sb = melded("auto-held-source", "", "--auto-update");
    sb.commit(
        &agent("one-one"),
        &toml("\n[[hooks]]\nrun = \"echo hi\"\nevent = \"update\"\n"),
        Some("v1.1.0"),
    );
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("hook needs consent"), "{}", r.stdout);
    assert_eq!(sb.entry()["pin"]["value"]["resolved"], "1.0.0");
    assert!(sb.installed_agent().contains("one-zero"));
}

#[test]
fn a_run_steps_aside_while_another_mind_holds_the_lock() {
    // spec: AUP-18
    let sb = melded("auto-locked", "", "--auto-update");
    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(sb.mind_home.join(".lock"))
        .unwrap();
    let mut held = fd_lock::RwLock::new(file);
    let guard = held.write().unwrap();
    let r = sb.mind(&["auto-update", "run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("another mind command is running"),
        "{}",
        r.stdout
    );
    assert!(sb.installed_agent().contains("one-zero"));
    drop(guard);
    assert!(sb.mind(&["auto-update", "run"]).success);
    assert!(sb.installed_agent().contains("one-one"));
}

#[test]
fn if_stale_skips_a_run_newer_than_the_duration() {
    // spec: AUP-19
    let sb = melded("auto-stale", "", "--auto-update");
    assert!(sb.mind(&["auto-update", "run", "--if-stale", "1d"]).success);
    assert!(sb.state()["last_run"].is_u64(), "{}", sb.state());

    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    let r = sb.mind(&["auto-update", "run", "--if-stale", "1d"]);
    assert!(r.stdout.contains("nothing to do"), "{}", r.stdout);
    assert!(sb.installed_agent().contains("one-zero"));

    write_file(&sb.mind_home.join("auto-update.json"), "{\"last_run\": 0}");
    assert!(sb.mind(&["auto-update", "run", "--if-stale", "1d"]).success);
    assert!(sb.installed_agent().contains("one-one"));

    let r = sb.mind(&["auto-update", "run", "--if-stale", "1w"]);
    assert!(!r.success);
    assert!(r.stderr.contains("invalid duration"), "{}", r.stderr);
}

#[test]
fn background_returns_at_once_and_the_run_finishes_detached() {
    // spec: AUP-20
    let sb = melded("auto-background", "", "--auto-update");
    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    let r = sb.mind(&["auto-update", "run", "--if-stale", "1d", "--background"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.is_empty(), "prints nothing: {}", r.stdout);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !sb.installed_agent().contains("one-one") || !sb.state()["last_run"].is_u64() {
        assert!(
            std::time::Instant::now() < deadline,
            "the detached run never finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn config_auto_update_installs_and_removes_the_trigger() {
    // spec: AUP-20
    let sb = Sandbox::new("auto-config");
    let settings = sb.claude_home.join("settings.json");
    write_file(&settings, "{\"model\": \"opus\"}\n");
    let r = sb.mind(&["config", "auto-update", "on", "--every", "12h"]);
    assert!(r.success, "{}", r.stderr);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    assert_eq!(
        v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "mind auto-update run --if-stale 12h --background"
    );
    let r = sb.mind(&["config", "auto-update", "on", "--every", "12h"]);
    assert!(r.stdout.contains("already installed"), "{}", r.stdout);

    let r = sb.mind(&["config", "auto-update", "off"]);
    assert!(r.success, "{}", r.stderr);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"model": "opus"}));

    let r = sb.mind(&["config", "auto-update", "on", "--every", "soon"]);
    assert!(!r.success);

    // A harness preset lobe is skill-only, so it gets a note, not a trigger.
    let gemini = sb.base.join("home");
    std::fs::create_dir_all(&gemini).unwrap();
    let r = sb.mind_env(
        &[
            "config",
            "lobes",
            "add",
            "--preset",
            "gemini",
            &gemini.to_string_lossy(),
        ],
        &[],
        None,
    );
    assert!(r.success, "{}", r.stderr);
    let r = sb.mind(&["config", "auto-update", "on"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("is not a Claude lobe"), "{}", r.stdout);
    assert!(!gemini.join(".gemini/config/settings.json").exists());
}

#[test]
fn the_next_recall_or_upgrade_summarizes_a_run_once() {
    // spec: AUP-21
    let sb = melded("auto-summary", "", "--auto-update");
    sb.commit(&agent("one-one"), &toml(""), Some("v1.1.0"));
    assert!(sb.mind(&["auto-update", "run"]).success);
    let log = std::fs::read_to_string(sb.mind_home.join("auto-update.log")).unwrap();
    assert!(
        log.contains(&format!("moved {} 1.0.0 -> 1.1.0", sb.identity())),
        "{log}"
    );

    // --json neither prints nor clears it.
    let r = sb.mind(&["recall", "--json"]);
    assert!(!r.stderr.contains("auto-update:"), "{}", r.stderr);
    let r = sb.mind(&["recall"]);
    assert!(
        r.stderr
            .contains(&format!("auto-update: {} 1.0.0 -> 1.1.0", sb.identity())),
        "{}",
        r.stderr
    );
    let r = sb.mind(&["upgrade"]);
    assert!(
        !r.stderr.contains("auto-update:"),
        "shown once: {}",
        r.stderr
    );
}

#[test]
fn a_managed_policy_caps_the_level() {
    // spec: AUP-30
    let declare = "auto-update = \"breaking\"\n";
    let sb = melded("auto-policy", declare, "--auto-update");
    sb.commit(&agent("two-zero"), &toml(declare), Some("v2.0.0"));
    sb.commit(
        &agent("tip"),
        &toml(&format!("{declare}pin-version = \"^2\"\n")),
        None,
    );

    let capped = sb.policy("[sources]\nauto-update = \"compatible\"\n");
    let env = [("MIND_POLICY_FILE", capped.as_str())];
    let r = sb.mind_env(&["recall", "--sources", "--json"], &env, None);
    let v: Value = serde_json::from_str(r.stdout.trim()).unwrap();
    assert_eq!(
        v["items"][0]["auto_update"]["effective"], "compatible",
        "{v}"
    );
    assert!(sb.mind_env(&["auto-update", "run"], &env, None).success);
    assert_eq!(sb.entry()["pin"]["value"]["range"], "^1");

    let off = sb.policy("[sources]\nauto-update = \"off\"\n");
    let env = [("MIND_POLICY_FILE", off.as_str())];
    let r = sb.mind_env(&["config", "auto-update", "on"], &env, None);
    assert!(!r.success);
    assert!(
        r.stderr.contains("turned off by the managed policy"),
        "{}",
        r.stderr
    );

    let bad = sb.policy("[sources]\nauto-update = \"sometimes\"\n");
    let r = sb.mind_env(
        &["auto-update", "run"],
        &[("MIND_POLICY_FILE", bad.as_str())],
        None,
    );
    assert!(!r.success);
    assert!(r.stderr.contains("[sources].auto-update"), "{}", r.stderr);
}
