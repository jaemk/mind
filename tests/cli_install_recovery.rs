//! Recovery of interrupted install transactions (spec/lifecycle.md,
//! LIFE-59..63): the real binary is killed at every journaled step with
//! `MIND_TEST_CRASH_AT`, then a later command must find the installed items,
//! their links, and the manifests in one whole state, the old or the new.
//!
//! Hermetic: local git sources melded by path into temp homes. The crash
//! hook exists only in debug builds, which is what `cargo test` runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The exit status of an injected crash (`transaction::CRASH_STATUS`).
const CRASHED: i32 = 86;
/// More journaled steps than any scenario here takes.
const MAX_STEPS: u32 = 200;

struct Sandbox {
    base: PathBuf,
    default_home: PathBuf,
    extra: PathBuf,
    claude: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-install-rec-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let sb = Sandbox {
            default_home: base.join("default"),
            extra: base.join("extra"),
            claude: base.join("claude"),
            base,
        };
        fs::create_dir_all(&sb.claude).unwrap();
        sb
    }

    /// A git source `name` with one skill `skill` whose body is `body`.
    fn source(&self, name: &str, skill: &str, body: &str) -> PathBuf {
        let dir = self.base.join(name);
        write(
            &dir.join(format!("skills/{skill}/SKILL.md")),
            &format!("---\ndescription: {skill}\n---\n{body}\n"),
        );
        git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "initial"]);
        dir
    }

    fn change(&self, source: &Path, skill: &str, body: &str) {
        write(
            &source.join(format!("skills/{skill}/SKILL.md")),
            &format!("---\ndescription: {skill}\n---\n{body}\n"),
        );
        git(source, &["commit", "-qam", "change"]);
    }

    fn mind(&self, crash_at: Option<u32>, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.default_home)
            .env("MIND_HOMES", &self.extra)
            .env("CLAUDE_HOME", &self.claude)
            .env_remove("MIND_TEST_CRASH_AT")
            .stdin(Stdio::null());
        if let Some(n) = crash_at {
            cmd.env("MIND_TEST_CRASH_AT", n.to_string());
        }
        let out = cmd.output().unwrap();
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn ok(&self, args: &[&str]) -> Run {
        let r = self.mind(None, args);
        assert_eq!(r.code, Some(0), "{args:?}: {} {}", r.stdout, r.stderr);
        r
    }

    /// Run `args` crashing at step `n`. `None` when the command finished.
    fn crash(&self, n: u32, args: &[&str]) -> Option<Run> {
        let r = self.mind(Some(n), args);
        match r.code {
            Some(CRASHED) => Some(r),
            Some(0) => None,
            _ => panic!("{args:?} at step {n}: {} {}", r.stdout, r.stderr),
        }
    }

    /// Recover with `recall`, itself interrupted at each of its own steps
    /// first, so recovering an interrupted recovery is covered too.
    fn recover(&self) {
        for n in 1..MAX_STEPS {
            if self.crash(n, &["recall"]).is_none() {
                return;
            }
        }
        panic!("recovery never finished");
    }

    fn manifest_items(&self) -> Vec<(String, Value)> {
        let mut items = Vec::new();
        for home in [&self.default_home, &self.extra] {
            let Ok(text) = fs::read_to_string(home.join("manifest.json")) else {
                continue;
            };
            let v: Value = serde_json::from_str(&text).unwrap();
            for (k, item) in v["items"].as_object().unwrap() {
                items.push((k.clone(), item.clone()));
            }
        }
        items
    }

    fn store_of(&self, item: &Value) -> PathBuf {
        let store = Path::new(item["store"].as_str().unwrap());
        if store.is_absolute() {
            store.to_path_buf()
        } else {
            self.default_home.join(store)
        }
    }

    /// The body `skill` is installed with, after checking the manifest entry,
    /// the store copy, and the link agree; `None` when it is not installed.
    fn installed(&self, skill: &str) -> Option<String> {
        let key = format!("skill:{skill}");
        let link = self.claude.join("skills").join(skill);
        let entry = self.manifest_items().into_iter().find(|(k, _)| *k == key);
        let Some((_, item)) = entry else {
            assert!(
                fs::symlink_metadata(&link).is_err()
                    || !fs::symlink_metadata(&link).unwrap().is_symlink(),
                "a link to {skill} is left with no manifest entry"
            );
            return None;
        };
        let store = self.store_of(&item);
        let body = fs::read_to_string(store.join("SKILL.md"))
            .unwrap_or_else(|e| panic!("store copy of {skill} at {store:?}: {e}"));
        assert_eq!(
            fs::read_link(&link).unwrap_or_else(|e| panic!("link {link:?}: {e}")),
            store,
            "{skill}'s link points at its store copy"
        );
        Some(body)
    }

    /// No journal or trash is left after a recovery.
    fn settled(&self) {
        for home in [&self.default_home, &self.extra] {
            assert!(
                !home.join(".tx-journal.json").exists(),
                "journal in {home:?}"
            );
            assert!(
                !home.join(".manifest-journal.json").exists(),
                "manifest journal in {home:?}"
            );
            let trash = home.join(".tmp/tx");
            if let Ok(entries) = fs::read_dir(&trash) {
                for entry in entries {
                    let entry = entry.unwrap().path();
                    assert!(
                        fs::read_dir(&entry).unwrap().next().is_none(),
                        "trash left at {entry:?}"
                    );
                }
            }
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn body_is(body: &Option<String>, text: &str) -> bool {
    body.as_deref().is_some_and(|b| b.contains(text))
}

// spec: LIFE-59 LIFE-61 LIFE-63
#[test]
fn an_upgrade_killed_at_any_step_recovers_whole() {
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let src = sb.source("src", "alpha", "v1");
        sb.ok(&["--yes", "meld", src.to_str().unwrap()]);
        assert!(body_is(&sb.installed("alpha"), "v1"));
        sb.change(&src, "alpha", "v2");
        let args = ["--yes", "upgrade"];
        if sb.crash(n, &args).is_none() {
            assert!(n > 1, "the upgrade crossed no crash point");
            assert!(body_is(&sb.installed("alpha"), "v2"));
            sb.settled();
            return;
        }
        sb.recover();
        let body = sb.installed("alpha");
        assert!(
            body_is(&body, "v1") || body_is(&body, "v2"),
            "step {n}: {body:?}"
        );
        sb.settled();
        // A clean rerun completes the upgrade.
        sb.ok(&args);
        assert!(body_is(&sb.installed("alpha"), "v2"), "step {n}");
        sb.settled();
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-59 LIFE-63
#[test]
fn a_forced_learn_killed_at_any_step_keeps_or_replaces_the_foreign_target() {
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let src = sb.source("src", "alpha", "v1");
        sb.ok(&["meld", src.to_str().unwrap()]);
        let foreign = sb.claude.join("skills/alpha");
        write(&foreign.join("SKILL.md"), "the user's own skill\n");
        let args = ["--yes", "learn", "alpha", "--force"];
        let crashed = sb.crash(n, &args).is_some();
        if crashed {
            sb.recover();
        }
        sb.settled();
        match sb.installed("alpha") {
            Some(body) => assert!(body.contains("v1"), "step {n}"),
            None => assert_eq!(
                fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
                "the user's own skill\n",
                "step {n}: the foreign target is restored"
            ),
        }
        if !crashed {
            assert!(n > 1);
            return;
        }
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-59 LIFE-63
#[test]
fn a_forget_killed_at_any_step_keeps_or_removes_the_whole_item() {
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let src = sb.source("src", "alpha", "v1");
        sb.ok(&["--yes", "meld", src.to_str().unwrap()]);
        let args = ["--yes", "forget", "alpha"];
        let crashed = sb.crash(n, &args).is_some();
        if crashed {
            sb.recover();
        }
        sb.settled();
        match sb.installed("alpha") {
            Some(body) => assert!(body.contains("v1"), "step {n}"),
            None => {
                let store = sb.default_home.join("store/skill/alpha");
                assert!(!store.exists(), "step {n}: store copy left behind");
            }
        }
        if !crashed {
            assert!(n > 1);
            return;
        }
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-61 LIFE-63
#[cfg(unix)]
#[test]
fn a_forget_over_a_repointed_link_recovers_instead_of_failing_closed() {
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let src = sb.source("src", "alpha", "v1");
        sb.ok(&["--yes", "meld", src.to_str().unwrap()]);
        // The user points the recorded link somewhere of their own.
        let elsewhere = sb.base.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let link = sb.claude.join("skills/alpha");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let args = ["--yes", "forget", "alpha"];
        let crashed = sb.crash(n, &args).is_some();
        if crashed {
            sb.recover();
        }
        sb.settled();
        let still = sb.manifest_items().iter().any(|(k, _)| k == "skill:alpha");
        if still {
            assert_eq!(fs::read_link(&link).unwrap(), elsewhere, "step {n}");
        } else {
            assert!(fs::symlink_metadata(&link).is_err(), "step {n}");
        }
        assert!(
            elsewhere.exists(),
            "step {n}: the link's target is untouched"
        );
        if !crashed {
            assert!(n > 1);
            return;
        }
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-60 LIFE-63
#[test]
fn an_upgrade_across_two_homes_commits_both_or_neither() {
    // Each item's install is its own savepoint and manifest save, so a crash
    // between them may leave one upgraded; each home's state stays whole.
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let a = sb.source("a", "alpha", "a1");
        let b = sb.source("b", "beta", "b1");
        sb.ok(&["--yes", "meld", a.to_str().unwrap()]);
        sb.ok(&[
            "--yes",
            "meld",
            b.to_str().unwrap(),
            "--home",
            sb.extra.to_str().unwrap(),
        ]);
        sb.change(&a, "alpha", "a2");
        sb.change(&b, "beta", "b2");
        let args = ["--yes", "upgrade"];
        if sb.crash(n, &args).is_none() {
            assert!(body_is(&sb.installed("alpha"), "a2"));
            assert!(body_is(&sb.installed("beta"), "b2"));
            sb.settled();
            assert!(n > 1);
            return;
        }
        sb.recover();
        sb.settled();
        let (alpha, beta) = (sb.installed("alpha"), sb.installed("beta"));
        assert!(body_is(&alpha, "a1") || body_is(&alpha, "a2"), "step {n}");
        assert!(body_is(&beta, "b1") || body_is(&beta, "b2"), "step {n}");
        sb.ok(&args);
        assert!(body_is(&sb.installed("alpha"), "a2"), "step {n}");
        assert!(body_is(&sb.installed("beta"), "b2"), "step {n}");
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-59 LIFE-63 HOME-8
#[test]
fn an_upgrade_moving_homes_killed_at_any_step_keeps_a_whole_item() {
    let mut n = 1;
    loop {
        let sb = Sandbox::new();
        let src = sb.source("src", "alpha", "v1");
        sb.ok(&["--yes", "meld", src.to_str().unwrap()]);
        // The source now declares the extra home: the upgrade reinstalls the
        // item there and removes the copy left in the default home.
        write(
            &src.join("mind.toml"),
            &format!("[source]\nmind-home = \"{}\"\n", sb.extra.display()),
        );
        let args = ["--yes", "upgrade"];
        let crashed = sb.crash(n, &args).is_some();
        if crashed {
            sb.recover();
        }
        sb.settled();
        assert!(body_is(&sb.installed("alpha"), "v1"), "step {n}");
        // A clean rerun finishes the move.
        sb.ok(&args);
        assert!(body_is(&sb.installed("alpha"), "v1"), "step {n}");
        assert!(sb.extra.join("store/skill/alpha").exists(), "step {n}");
        assert!(
            !sb.default_home.join("store/skill/alpha").exists(),
            "step {n}: the old copy is left behind"
        );
        if !crashed {
            assert!(n > 1);
            return;
        }
        n += 1;
        assert!(n < MAX_STEPS);
    }
}

// spec: LIFE-61
#[test]
fn a_journal_naming_a_path_outside_mind_is_refused_and_kept() {
    let sb = Sandbox::new();
    sb.ok(&["recall"]);
    let victim = sb.base.join("victim");
    write(&victim, "keep");
    let journal = sb.default_home.join(".tx-journal.json");
    let doc = serde_json::json!({
        "version": 1,
        "decision": "Preparing",
        "intents": [{"Move": {"from": victim, "to": sb.default_home.join("stolen")}}],
    });
    fs::write(&journal, serde_json::to_vec(&doc).unwrap()).unwrap();
    // Undo would move `stolen` back over the victim; refuse instead.
    write(&sb.default_home.join("stolen"), "planted");
    let r = sb.mind(None, &["--json", "recall"]);
    assert_ne!(r.code, Some(0));
    assert!(r.stdout.contains("\"transaction-journal\""), "{}", r.stdout);
    assert!(r.stdout.contains("remove it to go on"), "{}", r.stdout);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    assert!(journal.exists(), "the journal is kept for diagnosis");
}

// spec: LIFE-62
#[test]
fn recovery_reports_a_hook_that_was_interrupted() {
    let sb = Sandbox::new();
    sb.ok(&["recall"]);
    let journal = sb.default_home.join(".tx-journal.json");
    let doc = serde_json::json!({
        "version": 1,
        "decision": "Preparing",
        "intents": [{"Hook": {"label": "local/x/src install: build"}}],
    });
    fs::write(&journal, serde_json::to_vec(&doc).unwrap()).unwrap();
    let r = sb.ok(&["recall"]);
    let out = format!("{}{}", r.stdout, r.stderr);
    assert!(out.contains("rolled back an interrupted change"), "{out}");
    assert!(out.contains("'local/x/src install: build'"), "{out}");
    assert!(out.contains("not undone"), "{out}");
    assert!(!journal.exists());
}

// spec: LIFE-61
#[test]
fn a_dry_run_refuses_while_an_install_journal_is_pending() {
    let sb = Sandbox::new();
    sb.ok(&["recall"]);
    let journal = sb.extra.join(".tx-journal.json");
    fs::create_dir_all(&sb.extra).unwrap();
    let doc = serde_json::json!({"version": 1, "decision": "Preparing", "intents": []});
    let bytes = serde_json::to_vec(&doc).unwrap();
    fs::write(&journal, &bytes).unwrap();
    let r = sb.mind(None, &["--json", "upgrade", "--dry-run"]);
    assert_ne!(r.code, Some(0));
    assert!(r.stdout.contains("\"recovery-pending\""), "{}", r.stdout);
    assert_eq!(fs::read(&journal).unwrap(), bytes);
    // A mutating command recovers it first.
    sb.ok(&["recall"]);
    assert!(!journal.exists());
}
