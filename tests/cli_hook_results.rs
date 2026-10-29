//! Integration tests for hook result records, the hook history, and hook
//! output logs (spec/install-hooks.md "Hook results").
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture (local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   HOOK-160: one record per hook considered (ok, failed, skipped, declined)
//!   HOOK-161: records in the `--json` result document and the error envelope
//!   HOOK-162: the closing frame line carries the exit status
//!   HOOK-163: the capped history file
//!   HOOK-164: `hook-history` sets the cap; `0` turns the history off
//!   HOOK-165: `--hook-log` / `hook-log` save each hook's output
//!   HOOK-166: owned logs are deleted with their history record
//!   HOOK-167: `hooks history`
//!   HOOK-168: `hooks list` shows each hook's last result
//!   HOOK-169: `hooks history --show` prints saved output
//!   HOOK-170: `--hook-log-keep failed` keeps only failed hooks' logs
//!   HOOK-171: a Ctrl-C during a hook is recorded as `interrupted`
//!   CLI-252:  the `--hook-log` flag
//!   CLI-253:  the `hooks history` subcommand
//!   CLI-254:  `hooks history --outcome/--event/--since`
//!   CLI-255:  the hook exit codes

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

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
    code: Option<i32>,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-hr-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(&source.join("README.md"), "# fixture\n");
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    /// A sandbox whose source declares one install hook named `setup`.
    fn with_hook(name: &str, run: &str) -> Sandbox {
        let sb = Sandbox::new(name);
        sb.write_and_commit(
            "mind.toml",
            &format!("[[hooks]]\nname = \"setup\"\nrun = \"{run}\"\nevent = \"install\"\n"),
        );
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env_stdin(args, &[], "")
    }

    fn mind_env_stdin(&self, args: &[&str], envs: &[(&str, &str)], stdin: &str) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_TTY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn mind");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
            code: out.status.code(),
        }
    }

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn config(&self, body: &str) {
        write(&self.mind_home.join("config.toml"), body);
    }

    fn history_path(&self) -> PathBuf {
        self.mind_home.join("hooks").join("history.jsonl")
    }

    fn history(&self) -> Vec<serde_json::Value> {
        match std::fs::read_to_string(self.history_path()) {
            Ok(text) => text
                .lines()
                .map(|l| serde_json::from_str(l).expect("history line is JSON"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write(path: &Path, contents: &str) {
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
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

fn json(r: &Run) -> serde_json::Value {
    serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout))
}

// ---------------------------------------------------------------------------
// Result records (HOOK-160 / HOOK-161 / HOOK-162)
// ---------------------------------------------------------------------------

/// A hook that ran and exited 0 is reported in the verb's `--json` document
/// with its event, label, exit code, and run time.
#[test]
fn meld_json_reports_a_successful_hook() {
    // spec: HOOK-160 HOOK-161
    let sb = Sandbox::with_hook("ok-src", "true");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let doc = json(&r);
    let hooks = doc["hooks"].as_array().expect("a `hooks` array");
    assert_eq!(hooks.len(), 1, "{doc}");
    let h = &hooks[0];
    assert_eq!(h["outcome"], "ok", "{h}");
    assert_eq!(h["event"], "install");
    assert_eq!(h["label"], "setup");
    assert_eq!(h["command"], "true");
    assert_eq!(h["verb"], "meld");
    assert_eq!(h["exit_code"], 0);
    assert!(h["signal"].is_null());
    assert!(h["duration_ms"].is_u64(), "{h}");
    assert!(
        h["commit"].is_string(),
        "the source commit is recorded: {h}"
    );
    assert!(h["log"].is_null(), "no log without --hook-log: {h}");
    assert!(h["source"].as_str().unwrap().ends_with("ok-src"), "{h}");
    assert!(h.get("item").is_none(), "a source hook has no item: {h}");
}

/// A failing hook's record rides the error envelope, with its exit code.
#[test]
fn a_failed_hook_is_reported_in_the_error_envelope() {
    // spec: HOOK-160 HOOK-161
    let sb = Sandbox::with_hook("fail-src", "exit 3");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks"]);
    assert!(!r.success, "a failing hook fails the meld");
    assert_eq!(r.code, Some(3), "a hook failure exits 3 (CLI-255)");
    let doc = json(&r);
    assert_eq!(doc["error"]["kind"], "hook-failed", "{doc}");
    let hooks = doc["hooks"].as_array().expect("a `hooks` array");
    let last = hooks.last().unwrap();
    assert_eq!(last["outcome"], "failed", "{last}");
    assert_eq!(last["exit_code"], 3, "{last}");
}

/// A hook skipped for want of a terminal is recorded as `skipped`.
#[test]
fn a_hook_skipped_without_a_terminal_is_recorded_as_skipped() {
    // spec: HOOK-160 HOOK-161
    let sb = Sandbox::with_hook("skip-src", "true");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let doc = json(&r);
    let h = &doc["hooks"][0];
    assert_eq!(h["outcome"], "skipped", "{doc}");
    assert!(
        h["exit_code"].is_null(),
        "a hook that did not run has no exit: {h}"
    );
    assert!(h["duration_ms"].is_null(), "{h}");
}

/// A hook the user is asked about and declines is recorded as `declined`.
#[test]
fn a_hook_declined_at_the_prompt_is_recorded_as_declined() {
    // spec: HOOK-160 HOOK-163
    let sb = Sandbox::with_hook("decline-src", "true");
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind_env_stdin(
        &["hooks", "run", "decline-src"],
        &[("MIND_TTY", "1")],
        "n\n",
    );
    assert!(r.success, "hooks run: {} {}", r.stdout, r.stderr);
    let history = sb.history();
    let last = history.last().expect("a history record");
    assert_eq!(last["outcome"], "declined", "{last}");
    assert_eq!(last["verb"], "hooks run", "{last}");
}

/// A hook the user aborts at under `--json` still reaches the caller: the
/// run answers with a document whose `hooks` names the abort.
#[test]
fn an_aborted_hook_is_reported_under_json() {
    // spec: HOOK-160 HOOK-161 CLI-153
    let sb = Sandbox::with_hook("abort-src", "true");
    let r = sb.mind_env_stdin(
        &["meld", &sb.source_spec(), "--json"],
        &[("MIND_TTY", "1")],
        "a\n",
    );
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.trim().is_empty(),
        "no document; stderr: {}",
        r.stderr
    );
    let doc = json(&r);
    assert_eq!(doc["hooks"][0]["outcome"], "aborted", "{doc}");
    assert_eq!(doc["outcome"], "aborted", "{doc}");

    // unmeld: the source's uninstall hook, aborted at its prompt.
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nname = \"bye\"\nrun = \"true\"\nevent = \"uninstall\"\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let r = sb.mind_env_stdin(
        &["unmeld", "abort-src", "--json"],
        &[("MIND_TTY", "1")],
        "a\n",
    );
    assert!(r.success, "unmeld: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.trim().is_empty(),
        "no document; stderr: {}",
        r.stderr
    );
    let doc = json(&r);
    assert_eq!(doc["hooks"][0]["outcome"], "aborted", "{doc}");
    assert_eq!(doc["outcome"], "aborted", "{doc}");
}

/// No hooks considered, no `hooks` member.
#[test]
fn a_run_with_no_hooks_has_no_hooks_member() {
    // spec: HOOK-161
    let sb = Sandbox::new("nohooks-src");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(json(&r).get("hooks").is_none(), "{}", r.stdout);
}

/// An item hook's record names the item.
#[test]
fn an_item_hook_record_names_the_item() {
    // spec: HOOK-160
    let sb = Sandbox::new("item-src");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"scan\"\npath = \"skills/scan\"\ninstall = \"true\"\n",
    );
    sb.write_and_commit("skills/scan/SKILL.md", "---\ndescription: s\n---\n# s\n");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );
    let r = sb.mind(&["learn", "scan", "--json", "--accept-hooks"]);
    assert!(r.success, "learn: {} {}", r.stdout, r.stderr);
    let doc = json(&r);
    let h = &doc["hooks"][0];
    assert_eq!(h["item"], "skill:scan", "{doc}");
    assert_eq!(h["event"], "install");
    assert_eq!(h["outcome"], "ok");
}

/// The closing frame line says how the hook exited.
#[test]
fn the_closing_frame_carries_the_exit_status() {
    // spec: HOOK-162
    let sb = Sandbox::with_hook("frame-src", "exit 4");
    let r = sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"]);
    assert!(!r.success);
    assert!(
        r.stdout.contains("====== (end hook: setup, exit 4, "),
        "the closing frame must name the exit code: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// History (HOOK-163 / HOOK-164)
// ---------------------------------------------------------------------------

/// Every run appends to the history, which keeps only the newest
/// `hook-history` records.
#[test]
fn the_history_keeps_the_newest_records_up_to_the_cap() {
    // spec: HOOK-163 HOOK-164
    let sb = Sandbox::with_hook("cap-src", "true");
    sb.config("hook-history = 2\n");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"])
            .success
    );
    for _ in 0..2 {
        let r = sb.mind(&["hooks", "run", "cap-src", "--force", "--accept-hooks"]);
        assert!(r.success, "hooks run: {} {}", r.stdout, r.stderr);
    }
    let history = sb.history();
    assert_eq!(history.len(), 2, "{history:?}");
    assert!(
        history.iter().all(|h| h["verb"] == "hooks run"),
        "the meld record was evicted: {history:?}"
    );
    assert_ne!(
        history[0]["run"], history[1]["run"],
        "each process has its own run id"
    );
}

/// The default cap is 100.
#[test]
fn the_default_cap_is_one_hundred() {
    // spec: HOOK-163
    let sb = Sandbox::with_hook("default-cap-src", "true");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );
    let lines: String = (0..105)
        .map(|n| format!("{{\"at\":\"t{n}\",\"run\":\"r\",\"verb\":\"meld\",\"event\":\"install\",\"label\":\"x\",\"source\":\"s\",\"command\":\"true\",\"outcome\":\"ok\"}}\n"))
        .collect();
    write(&sb.history_path(), &lines);
    assert!(
        sb.mind(&["hooks", "run", "default-cap-src", "--accept-hooks"])
            .success
    );
    assert_eq!(sb.history().len(), 100);
}

/// `hook-history = 0` turns the history off.
#[test]
fn a_zero_cap_turns_the_history_off() {
    // spec: HOOK-164 HOOK-167
    let sb = Sandbox::with_hook("off-src", "true");
    sb.config("hook-history = 0\n");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"])
            .success
    );
    assert!(
        !sb.history_path().exists(),
        "no history file with the cap at 0"
    );
    let r = sb.mind(&["hooks", "history"]);
    assert!(r.success);
    assert!(r.stdout.contains("hook history is off"), "{}", r.stdout);

    // A file left from before the cap went to 0 is still shown, but the
    // note says nothing new lands in it.
    let line = "{\"at\":\"t\",\"run\":\"r\",\"verb\":\"meld\",\"event\":\"install\",\"label\":\"old\",\"source\":\"s\",\"command\":\"true\",\"outcome\":\"ok\"}\n";
    write(&sb.history_path(), line);
    let r = sb.mind(&["hooks", "history"]);
    assert!(r.stdout.contains("hook history is off"), "{}", r.stdout);
    assert!(r.stdout.contains("'old'"), "{}", r.stdout);
    // Under `--json` the note still appears, on stderr, outside the document.
    let r = sb.mind(&["hooks", "history", "--json"]);
    assert_eq!(json(&r)["records"].as_array().unwrap().len(), 1);
    assert!(r.stderr.contains("hook history is off"), "{}", r.stderr);
    assert!(
        sb.mind(&["hooks", "run", "off-src", "--force", "--accept-hooks"])
            .success
    );
    assert_eq!(
        sb.history().len(),
        1,
        "nothing is appended with the cap at 0"
    );
}

// ---------------------------------------------------------------------------
// Output logs (HOOK-165 / HOOK-166)
// ---------------------------------------------------------------------------

const NOISY: &str = "echo to-out; echo to-err 1>&2";

/// A bare `--hook-log` saves the output under `~/.mind/hooks/logs`, tagged
/// by stream, and still shows it.
#[test]
fn a_bare_hook_log_saves_output_under_the_mind_home() {
    // spec: HOOK-165 CLI-252
    let sb = Sandbox::with_hook("log-src", NOISY);
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let doc = json(&r);
    let log = PathBuf::from(doc["hooks"][0]["log"].as_str().expect("a log path"));
    assert!(
        log.starts_with(sb.mind_home.join("hooks").join("logs")),
        "{log:?}"
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("out | to-out"), "{text}");
    assert!(text.contains("err | to-err"), "{text}");
    assert!(text.contains("# exit 0"), "{text}");
    assert!(text.contains("# command: echo to-out"), "{text}");
    // Still streamed: under --json mind's stdout is stderr.
    assert!(
        r.stderr.contains("to-out") && r.stderr.contains("to-err"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("hook output saved to"), "{}", r.stderr);
}

/// Logged output keeps each stream on its own channel in text mode.
#[test]
fn logged_output_still_reaches_the_terminal_streams() {
    // spec: HOOK-165
    let sb = Sandbox::with_hook("tee-src", NOISY);
    let r = sb.mind(&["meld", &sb.source_spec(), "--accept-hooks", "--hook-log"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("to-out") && !r.stdout.contains("to-err"),
        "{}",
        r.stdout
    );
    assert!(r.stderr.contains("to-err"), "{}", r.stderr);
}

/// `--hook-log=DIR` saves the output there.
#[test]
fn a_hook_log_dir_saves_output_there() {
    // spec: HOOK-165 CLI-252
    let sb = Sandbox::with_hook("logdir-src", NOISY);
    let dir = sb.base.join("my-logs");
    let flag = format!("--hook-log={}", dir.display());
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks", &flag]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    assert!(log.starts_with(&dir), "{log:?}");
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("out | to-out")
    );
}

/// A relative `--hook-log` directory is recorded as an absolute path, and a
/// directory inside the owned one is pruned like the rest of it.
#[test]
fn a_relative_or_owned_hook_log_dir_is_resolved() {
    // spec: HOOK-165 HOOK-166 CLI-252
    let sb = Sandbox::with_hook("logrel-src", NOISY);
    sb.config("hook-history = 1\n");
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args([
            "meld",
            &sb.source_spec(),
            "--json",
            "--accept-hooks",
            "--hook-log=rel-logs",
        ])
        .current_dir(&sb.base)
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .env_remove("MIND_TTY")
        .stdin(Stdio::null())
        .output()
        .expect("run mind");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = PathBuf::from(doc["hooks"][0]["log"].as_str().unwrap());
    assert!(log.is_absolute(), "{log:?}");
    // The working directory comes back resolved (macOS's /var is
    // /private/var), so compare against the resolved base.
    let base = sb.base.canonicalize().unwrap();
    assert!(log.starts_with(base.join("rel-logs")), "{log:?}");

    let inside = sb.mind_home.join("hooks").join("logs").join("mine");
    let flag = format!("--hook-log={}", inside.display());
    let r = sb.mind(&[
        "hooks",
        "run",
        "logrel-src",
        "--force",
        "--json",
        "--accept-hooks",
        &flag,
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let owned_log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    assert!(
        owned_log.starts_with(&inside),
        "the log is written where it was asked for: {owned_log:?}"
    );
    assert!(
        sb.mind(&["hooks", "run", "logrel-src", "--force", "--accept-hooks"])
            .success
    );
    assert!(
        !owned_log.exists(),
        "a log inside the owned dir is pruned with its record"
    );

    // `--hook-log=~/x` reaches mind unexpanded (the shell does not expand a
    // `~` after `=`), so mind expands it rather than making a literal `~`.
    let home = sb.base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let r = sb.mind_env_stdin(
        &[
            "hooks",
            "run",
            "logrel-src",
            "--force",
            "--json",
            "--accept-hooks",
            "--hook-log=~/tilde-logs",
        ],
        &[("HOME", &home.to_string_lossy())],
        "",
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    assert!(log.starts_with(home.join("tilde-logs")), "{log:?}");
    assert!(log.exists(), "{log:?}");
}

/// With a relative `MIND_HOME`, an owned log is still recorded as an absolute
/// path and still pruned with its record.
#[test]
fn a_relative_mind_home_still_owns_its_logs() {
    // spec: HOOK-165 HOOK-166
    let sb = Sandbox::with_hook("relhome-src", NOISY);
    sb.config("hook-history = 1\n");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .current_dir(&sb.base)
            .env("MIND_HOME", "mind")
            .env("CLAUDE_HOME", &sb.claude_home)
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
            .output()
            .expect("run mind")
    };
    let out = run(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = PathBuf::from(doc["hooks"][0]["log"].as_str().unwrap());
    assert!(log.is_absolute(), "{log:?}");
    assert!(log.exists(), "{log:?}");

    let out = run(&["hooks", "run", "relhome-src", "--force", "--accept-hooks"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!log.exists(), "the evicted owned log is pruned: {log:?}");
}

/// A `config.toml` that does not parse leaves the hook settings at their
/// defaults, and says so when it names a hook key, even on a verb that reads
/// no other setting.
#[test]
fn an_unreadable_config_with_hook_keys_warns() {
    // spec: HOOK-164
    let sb = Sandbox::with_hook("badcfg-src", NOISY);
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"])
            .success
    );
    sb.config("hook-log = true\nhook-log-keep = \"failures\"\n");
    let r = sb.mind(&[
        "hooks",
        "run",
        "badcfg-src",
        "--force",
        "--json",
        "--accept-hooks",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        json(&r)["hooks"][0]["log"].is_null(),
        "defaults: no log: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("hook settings") && r.stderr.contains("config.toml"),
        "{}",
        r.stderr
    );

    // A broken config that names no hook key does not add the warning.
    sb.config("ssh = \"maybe\"\n");
    let r = sb.mind(&["hooks", "history"]);
    assert!(!r.stderr.contains("hook settings"), "{}", r.stderr);
}

/// The `hook-log` config key turns logging on without the flag.
#[test]
fn the_hook_log_config_key_turns_logging_on() {
    // spec: HOOK-165
    let sb = Sandbox::with_hook("logcfg-src", NOISY);
    sb.config("hook-log = true\n");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(json(&r)["hooks"][0]["log"].is_string(), "{}", r.stdout);
}

/// A nested source whose install hook fails stops the meld with the hook's
/// error and exit code, rather than being skipped as a clone failure.
#[test]
fn a_nested_sources_failed_hook_stops_the_meld() {
    // spec: HOOK-172 CLI-255
    let nested = Sandbox::with_hook("nested-fail-src", "exit 7");
    let sb = Sandbox::new("curator-src");
    sb.write_and_commit("skills/hello/SKILL.md", "---\ndescription: hi\n---\n");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n",
            nested.source_spec()
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks"]);
    assert_eq!(r.code, Some(3), "{} {}", r.stdout, r.stderr);
    let doc = json(&r);
    assert_eq!(doc["error"]["kind"], "hook-failed", "{doc}");
    assert!(!r.stderr.contains("clone"), "{}", r.stderr);
}

/// A relative `hook-log` path is relative to `~/.mind`, not the working
/// directory, and `--no-hook-log` turns the config's logging off for a run.
#[test]
fn a_relative_hook_log_key_is_under_the_mind_home_and_no_hook_log_wins() {
    // spec: HOOK-165 CLI-256
    let sb = Sandbox::with_hook("logkeyrel-src", NOISY);
    sb.config("hook-log = \"rel-logs\"\n");
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &sb.source_spec(), "--json", "--accept-hooks"])
        .current_dir(&sb.base)
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .env_remove("MIND_TTY")
        .stdin(Stdio::null())
        .output()
        .expect("run mind");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let log = PathBuf::from(doc["hooks"][0]["log"].as_str().unwrap());
    assert!(log.starts_with(sb.mind_home.join("rel-logs")), "{log:?}");
    assert!(!sb.base.join("rel-logs").exists(), "not under the cwd");

    let r = sb.mind(&[
        "hooks",
        "run",
        "logkeyrel-src",
        "--force",
        "--json",
        "--accept-hooks",
        "--no-hook-log",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(json(&r)["hooks"][0]["log"].is_null(), "{}", r.stdout);

    // It contradicts the flags that turn logging on.
    let r = sb.mind(&["recall", "--no-hook-log", "--hook-log"]);
    assert!(!r.success && r.code == Some(2), "{}", r.stderr);
    let r = sb.mind(&["recall", "--no-hook-log", "--hook-log-keep", "failed"]);
    assert!(!r.success && r.code == Some(2), "{}", r.stderr);
}

/// An empty `--hook-log=` is a usage error; an empty `hook-log` path means
/// the owned directory, not the working directory.
#[test]
fn an_empty_hook_log_dir_means_the_owned_dir() {
    // spec: HOOK-165 CLI-252
    let sb = Sandbox::with_hook("logempty-src", NOISY);
    let owned = sb.mind_home.join("hooks").join("logs");
    let r = sb.mind(&["recall", "--hook-log="]);
    assert!(!r.success, "an empty --hook-log= is refused");
    assert!(r.stderr.contains("a value is required"), "{}", r.stderr);

    sb.config("hook-log = \"\"\n");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    assert!(log.starts_with(&owned), "{log:?}");
}

/// An owned log goes when its record leaves the history; a log in a
/// directory the user named stays.
#[test]
fn eviction_deletes_owned_logs_only() {
    // spec: HOOK-166
    let sb = Sandbox::with_hook("evict-src", NOISY);
    sb.config("hook-history = 1\n");
    let user_dir = sb.base.join("kept-logs");
    let flag = format!("--hook-log={}", user_dir.display());
    let r = sb.mind(&["meld", &sb.source_spec(), "--json", "--accept-hooks", &flag]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let user_log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());

    let r = sb.mind(&[
        "hooks",
        "run",
        "evict-src",
        "--force",
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert!(r.success, "hooks run: {} {}", r.stdout, r.stderr);
    let owned_log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    assert!(
        user_log.exists(),
        "a log the user directed elsewhere is never deleted"
    );
    assert!(owned_log.exists());

    let r = sb.mind(&["hooks", "run", "evict-src", "--force", "--accept-hooks"]);
    assert!(r.success, "hooks run: {} {}", r.stdout, r.stderr);
    assert!(
        !owned_log.exists(),
        "an owned log goes with its evicted record"
    );
}

// ---------------------------------------------------------------------------
// hooks history (HOOK-167) / hooks list (HOOK-168)
// ---------------------------------------------------------------------------

/// `hooks history` lists results oldest first, filters by target and
/// outcome, and limits to the newest N.
#[test]
fn hooks_history_lists_filters_and_limits() {
    // spec: HOOK-167 CLI-253
    let ok = Sandbox::with_hook("hist-ok", "true");
    assert!(
        ok.mind(&["meld", &ok.source_spec(), "--accept-hooks"])
            .success
    );
    // A second source melded into the same home, whose hook fails.
    let bad_src = ok.base.join("hist-bad");
    write(
        &bad_src.join("mind.toml"),
        "[[hooks]]\nname = \"boom\"\nrun = \"exit 2\"\nevent = \"install\"\n",
    );
    git(&bad_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&bad_src, &["config", "user.email", "t@t"]);
    git(&bad_src, &["config", "user.name", "t"]);
    git(&bad_src, &["add", "-A"]);
    git(&bad_src, &["commit", "-qm", "initial"]);
    assert!(
        ok.mind(&["meld", &bad_src.to_string_lossy(), "--register-only"])
            .success
    );
    assert!(
        !ok.mind(&["hooks", "run", "hist-bad", "--accept-hooks"])
            .success
    );

    let r = ok.mind(&["hooks", "history"]);
    assert!(r.success, "{}", r.stderr);
    // The register-only meld still offered `boom` and recorded the skip.
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", r.stdout);
    assert!(
        lines[0].contains(" ok ") && lines[0].contains("'setup' (exit 0, "),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("[meld]"), "{}", lines[0]);
    assert!(
        lines[1].contains(" skipped ") && lines[1].contains("'boom' at "),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].contains(" failed ") && lines[2].contains("'boom' (exit 2, "),
        "{}",
        lines[2]
    );
    assert!(lines[2].contains("[hooks run]"), "{}", lines[2]);

    let r = ok.mind(&["hooks", "history", "--failed", "--json"]);
    let doc = json(&r);
    assert_eq!(doc["action"], "hooks-history");
    let records = doc["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "{doc}");
    assert_eq!(records[0]["label"], "boom");

    let r = ok.mind(&["hooks", "history", "hist-bad", "--json"]);
    assert_eq!(json(&r)["records"].as_array().unwrap().len(), 2);

    let r = ok.mind(&["hooks", "history", "hist-ok", "--json"]);
    let records = json(&r)["records"].as_array().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["label"], "setup");

    let r = ok.mind(&["hooks", "history", "--limit", "1", "--json"]);
    let records = json(&r)["records"].as_array().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["label"], "boom", "--limit keeps the newest");
}

/// An item ref filters to that item's hooks.
#[test]
fn hooks_history_item_target_matches_only_that_item() {
    // spec: HOOK-167
    let sb = Sandbox::new("hist-item");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\nname = \"setup\"\nrun = \"true\"\nevent = \"install\"\n\n",
            "[[items]]\nkind = \"skill\"\nname = \"scan\"\npath = \"skills/scan\"\ninstall = \"true\"\n",
        ),
    );
    sb.write_and_commit("skills/scan/SKILL.md", "---\ndescription: s\n---\n# s\n");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes", "--accept-hooks"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let r = sb.mind(&["hooks", "history", "hist-item#skill:scan", "--json"]);
    let records = json(&r)["records"].as_array().unwrap().clone();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["item"], "skill:scan");
    let r = sb.mind(&["hooks", "history", "hist-item", "--json"]);
    assert_eq!(json(&r)["records"].as_array().unwrap().len(), 2);

    // spec: CLI-253 -- in text, a source hook names its label; an item hook's
    // label is its event, already on the line, so it is not repeated.
    let r = sb.mind(&["hooks", "history", "hist-item"]);
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{}", r.stdout);
    assert!(
        lines[0].contains("hist-item 'setup' (exit 0, "),
        "{}",
        r.stdout
    );
    assert!(lines[1].contains("#skill:scan (exit 0, "), "{}", r.stdout);
    assert!(!lines[1].contains("'install'"), "{}", r.stdout);
}

/// A target equal to a recorded identity that contains `#` (an item-link
/// instance) is read as that source; `source:` forces the source reading.
#[test]
fn hooks_history_reads_a_hash_identity_as_a_source() {
    // spec: CLI-253
    let sb = Sandbox::new("hist-hash");
    let line = "{\"at\":\"t\",\"run\":\"r\",\"verb\":\"meld\",\"event\":\"install\",\"label\":\"x\",\"source\":\"local/a/b#skills/x\",\"command\":\"true\",\"outcome\":\"ok\"}\n";
    write(&sb.history_path(), line);
    for target in ["local/a/b#skills/x", "source:local/a/b#skills/x"] {
        let r = sb.mind(&["hooks", "history", target, "--json"]);
        assert!(r.success, "{target}: {}", r.stderr);
        assert_eq!(json(&r)["records"].as_array().unwrap().len(), 1, "{target}");
    }
    let r = sb.mind(&["hooks", "history", "nomatch"]);
    assert!(
        r.stdout.contains("no recorded hook results match"),
        "{}",
        r.stdout
    );
}

/// `hooks list` shows each hook's newest history record.
#[test]
fn hooks_list_shows_the_last_result() {
    // spec: HOOK-168
    let sb = Sandbox::with_hook("last-src", "true");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--accept-hooks", "--hook-log"])
            .success
    );
    let r = sb.mind(&["hooks", "list", "last-src"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("last: ok (exit 0, "), "{}", r.stdout);
    assert!(r.stdout.contains(", log: "), "{}", r.stdout);
    let r = sb.mind(&["hooks", "list", "last-src", "--json"]);
    let doc = json(&r);
    let last = &doc["sources"][0]["hooks"][0]["last"];
    assert_eq!(last["outcome"], "ok", "{doc}");
    assert_eq!(last["exit_code"], 0);
}

/// An item hook's last result is found under the item, including for a
/// prefixed source, whose items are keyed by their prefixed name.
#[test]
fn hooks_list_shows_an_item_hooks_last_result() {
    // spec: HOOK-168
    let sb = Sandbox::new("lastitem-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\nprefix = \"jk\"\n\n",
            "[[items]]\nkind = \"skill\"\nname = \"scan\"\npath = \"skills/scan\"\ninstall = \"exit 4\"\n",
        ),
    );
    sb.write_and_commit("skills/scan/SKILL.md", "---\ndescription: s\n---\n# s\n");
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    // Without a terminal, learn skips the failing install hook; then run it
    // on demand so its failure is the newest record.
    let r = sb.mind(&["learn", "lastitem-src#jk:scan"]);
    assert!(r.success, "learn: {} {}", r.stdout, r.stderr);
    let r = sb.mind(&["hooks", "run", "lastitem-src#jk:scan", "--accept-hooks"]);
    assert!(!r.success, "the hook fails: {}", r.stdout);

    for target in ["lastitem-src", "lastitem-src#jk:scan"] {
        let r = sb.mind(&["hooks", "list", target]);
        assert!(r.success, "{target}: {}", r.stderr);
        assert!(
            r.stdout.contains("last: failed (exit 4, "),
            "{target}: {}",
            r.stdout
        );
    }
}

/// A hook with no history record shows no last result.
#[test]
fn hooks_list_without_history_shows_no_last_result() {
    // spec: HOOK-168
    let sb = Sandbox::with_hook("nolast-src", "true");
    sb.config("hook-history = 0\n");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"])
            .success
    );
    let r = sb.mind(&["hooks", "list", "nolast-src", "--json"]);
    let doc = json(&r);
    assert!(doc["sources"][0]["hooks"][0].get("last").is_none(), "{doc}");
}

/// `config show` reports the hook settings in effect.
#[test]
fn config_show_reports_the_hook_settings() {
    // spec: HOOK-164 HOOK-165 CLI-241
    let sb = Sandbox::new("cfg-src");
    let r = sb.mind(&["config", "show", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let doc = json(&r);
    assert_eq!(doc["hook_history"], 100, "{doc}");
    assert_eq!(doc["hook_log"], false, "{doc}");
    // spec: HOOK-170
    assert_eq!(doc["hook_log_keep"], "all", "{doc}");

    sb.config("hook-history = 7\nhook-log = \"~/logs\"\n");
    let r = sb.mind(&["config", "show"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("hook-history = 7"), "{}", r.stdout);
    assert!(r.stdout.contains("hook-log = \"~/logs\""), "{}", r.stdout);
    assert!(
        !r.stdout.contains("--hook-log"),
        "no flag note: {}",
        r.stdout
    );

    // The flags outrank the keys, and the report follows them.
    let r = sb.mind(&[
        "config",
        "show",
        "--json",
        "--hook-log",
        "--hook-log-keep",
        "failed",
    ]);
    assert!(r.success, "{}", r.stderr);
    let doc = json(&r);
    assert_eq!(doc["hook_log"], true, "{doc}");
    assert_eq!(doc["hook_log_keep"], "failed", "{doc}");
    let r = sb.mind(&["config", "show", "--hook-log-keep", "failed"]);
    assert!(r.stdout.contains("hook-log-keep = failed"), "{}", r.stdout);
    assert!(
        r.stdout.contains("set by --hook-log / --hook-log-keep"),
        "{}",
        r.stdout
    );
}

/// `hooks` and each of its subcommands show examples in `--help`, including
/// a source filter and an item ref.
#[test]
fn hooks_help_has_examples() {
    // spec: CLI-194 CLI-195 CLI-196 CLI-253
    let sb = Sandbox::new("help-src");
    for (args, examples) in [
        (
            &["hooks", "--help"][..],
            &[
                "mind hooks list owner/repo",
                "mind hooks history owner/repo --failed",
            ][..],
        ),
        (
            &["hooks", "run", "--help"][..],
            &[
                "mind hooks run owner/repo",
                "mind hooks run 'owner/*' --event update",
                "mind hooks run 'owner/repo#skill:scan'",
            ][..],
        ),
        (
            &["hooks", "list", "--help"][..],
            &[
                "mind hooks list 'owner/*'",
                "mind hooks list 'owner/repo#skill:scan'",
            ][..],
        ),
        (
            &["hooks", "history", "--help"][..],
            &[
                "mind hooks history owner/repo --failed",
                "mind hooks history 'source:host/owner/repo#skills/x'",
            ][..],
        ),
    ] {
        let r = sb.mind(args);
        assert!(r.success, "{args:?}: {}", r.stderr);
        assert!(r.stdout.contains("EXAMPLES:"), "{args:?}: {}", r.stdout);
        for example in examples {
            assert!(
                r.stdout.contains(example),
                "{args:?} must show `{example}`: {}",
                r.stdout
            );
        }
    }
}

// ---------------------------------------------------------------------------
// --show (HOOK-169), filters (CLI-254), keep (HOOK-170), interrupt (HOOK-171),
// exit codes (CLI-255), log names, overrides
// ---------------------------------------------------------------------------

/// `--show` prints a saved log after its line, or notes it is gone; under
/// `--json` each record with a log gains `output`.
#[test]
fn hooks_history_show_prints_the_saved_output() {
    // spec: HOOK-169
    let sb = Sandbox::with_hook("show-src", NOISY);
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());

    // Each log line stays a line of its own, in both modes.
    let r = sb.mind(&["hooks", "history", "--show"]);
    assert!(r.success, "{}", r.stderr);
    let lines: Vec<&str> = r.stdout.lines().collect();
    for want in ["out | to-out", "err | to-err", "# exit 0"] {
        assert!(lines.contains(&want), "{want:?} as a line: {}", r.stdout);
    }
    let r = sb.mind(&["hooks", "history", "--show", "--json"]);
    let output = json(&r)["records"][0]["output"]
        .as_str()
        .unwrap()
        .to_string();
    let lines: Vec<&str> = output.lines().collect();
    for want in ["out | to-out", "err | to-err", "# exit 0"] {
        assert!(lines.contains(&want), "{want:?} as a line: {output}");
    }
    let r = sb.mind(&["hooks", "history", "--json"]);
    assert!(
        json(&r)["records"][0].get("output").is_none(),
        "{}",
        r.stdout
    );

    std::fs::remove_file(&log).unwrap();
    let r = sb.mind(&["hooks", "history", "--show"]);
    assert!(r.stdout.contains("(no longer on disk)"), "{}", r.stdout);
    let r = sb.mind(&["hooks", "history", "--show", "--json"]);
    assert!(json(&r)["records"][0]["output"].is_null(), "{}", r.stdout);
}

/// With the history off, a run that logs to the owned directory deletes the
/// owned `.log` files older than 30 days, and the directories they leave
/// empty; newer logs and anything that is not a `.log` file stay.
#[test]
fn history_off_prunes_stale_owned_logs() {
    // spec: HOOK-166
    let sb = Sandbox::with_hook("prune-src", NOISY);
    sb.config("hook-history = 0\n");
    let owned = sb.mind_home.join("hooks").join("logs");
    let old_run = owned.join("20250101T000000Z-1");
    let fresh_run = owned.join("20260101T000000Z-2");
    std::fs::create_dir_all(&old_run).unwrap();
    std::fs::create_dir_all(&fresh_run).unwrap();
    let old_log = old_run.join("001-install-x.log");
    let fresh_log = fresh_run.join("001-install-x.log");
    let old_other = owned.join("notes.txt");
    for p in [&old_log, &fresh_log, &old_other] {
        std::fs::write(p, "x").unwrap();
    }
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
    for p in [&old_log, &old_other] {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
    }

    let r = sb.mind(&["meld", &sb.source_spec(), "--accept-hooks", "--hook-log"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(!old_log.exists() && !old_run.exists(), "the stale log goes");
    assert!(fresh_log.exists(), "a newer log stays");
    assert!(old_other.exists(), "only .log files are deleted");
}

/// `--outcome`, `--event`, and `--since` narrow the history; a bad `--since`
/// is refused.
#[test]
fn hooks_history_filters_by_outcome_event_and_time() {
    // spec: CLI-254
    let sb = Sandbox::with_hook("filt-src", "true");
    // No terminal and no --accept-hooks: skipped. Then run it: ok.
    assert!(sb.mind(&["meld", &sb.source_spec(), "--yes"]).success);
    assert!(
        sb.mind(&["hooks", "run", "filt-src", "--accept-hooks"])
            .success
    );
    let count = |args: &[&str]| {
        let mut all = vec!["hooks", "history", "--json"];
        all.extend_from_slice(args);
        let r = sb.mind(&all);
        assert!(r.success, "{args:?}: {} {}", r.stdout, r.stderr);
        json(&r)["records"].as_array().unwrap().len()
    };
    assert_eq!(count(&[]), 2);
    assert_eq!(count(&["--outcome", "skipped"]), 1);
    assert_eq!(count(&["--outcome", "ok,skipped"]), 2);
    assert_eq!(count(&["--outcome", "ok", "--failed"]), 1);
    assert_eq!(count(&["--event", "install"]), 2);
    assert_eq!(count(&["--event", "update,uninstall"]), 0);
    assert_eq!(count(&["--since", "1d"]), 2);
    assert_eq!(count(&["--since", "2000-01-01"]), 2);
    assert_eq!(count(&["--since", "2999-01-01T00:00"]), 0);

    let r = sb.mind(&["hooks", "history", "--since", "last-tuesday", "--json"]);
    assert_eq!(r.code, Some(1));
    assert_eq!(
        json(&r)["error"]["kind"],
        "bad-history-since",
        "{}",
        r.stdout
    );
    let r = sb.mind(&["hooks", "history", "--outcome", "skipped"]);
    assert!(
        r.stdout.contains("skipped") && !r.stdout.contains("  ok "),
        "{}",
        r.stdout
    );
}

/// `--hook-log-keep failed` drops a successful hook's log, keeps a failed
/// one's, and turns logging on by itself; the config key alone does not.
#[test]
fn hook_log_keep_failed_keeps_only_failures() {
    // spec: HOOK-170
    let ok = Sandbox::with_hook("keep-ok", NOISY);
    let r = ok.mind(&[
        "meld",
        &ok.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log-keep",
        "failed",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(json(&r)["hooks"][0]["log"].is_null(), "{}", r.stdout);
    assert!(!r.stderr.contains("hook output saved to"), "{}", r.stderr);
    let logs = ok.mind_home.join("hooks").join("logs");
    let left: Vec<_> = std::fs::read_dir(&logs)
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "no run dir left behind: {left:?}");

    let bad = Sandbox::with_hook("keep-bad", "echo broke; exit 4");
    let r = bad.mind(&[
        "meld",
        &bad.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log-keep=failed",
    ]);
    assert!(!r.success);
    let log = json(&r)["hooks"][0]["log"].as_str().unwrap().to_string();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("out | broke") && text.contains("# exit 4"),
        "{text}"
    );

    // The config key shapes logging that is on; alone it turns nothing on.
    let cfg = Sandbox::with_hook("keep-cfg", NOISY);
    cfg.config("hook-log-keep = \"failed\"\n");
    let r = cfg.mind(&["meld", &cfg.source_spec(), "--json", "--accept-hooks"]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(json(&r)["hooks"][0]["log"].is_null());
    assert!(!cfg.mind_home.join("hooks").join("logs").exists());
}

/// A Ctrl-C while a hook runs is recorded as `interrupted` and stops `mind`
/// with exit 130, whatever the hook's own exit. The hook signals its parent
/// (`mind`) directly, standing in for the terminal's process-group SIGINT.
#[test]
fn an_interrupted_hook_is_recorded_and_stops_mind() {
    // spec: HOOK-171 CLI-255
    let sb = Sandbox::with_hook("int-src", "kill -INT $PPID; sleep 0.3; echo after");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert_eq!(r.code, Some(130), "{} {}", r.stdout, r.stderr);
    let doc = json(&r);
    assert_eq!(doc["error"]["kind"], "hook-interrupted", "{doc}");
    let rec = doc["hooks"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(rec["outcome"], "interrupted", "{rec}");
    assert_eq!(rec["exit_code"], 0, "the hook itself ran to its end: {rec}");
    let text = std::fs::read_to_string(rec["log"].as_str().unwrap()).unwrap();
    assert!(
        text.contains("out | after") && text.contains("# exit 0"),
        "{text}"
    );
    assert_eq!(sb.history().last().unwrap()["outcome"], "interrupted");
}

/// A second Ctrl-C while a hook that ignores SIGINT still runs kills `mind`
/// at once, rather than leaving it waiting on the hook with no way out.
#[cfg(unix)]
#[test]
fn a_second_interrupt_ends_mind_while_the_hook_runs_on() {
    // spec: HOOK-171
    let sb = Sandbox::with_hook(
        "int2-src",
        "trap '' INT; kill -INT $PPID; sleep 0.3; kill -INT $PPID; sleep 1; echo after",
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"]);
    assert_eq!(
        r.code, None,
        "mind must die of the second SIGINT, not wait for the hook: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.history().is_empty(),
        "nothing is recorded on the forced exit"
    );
}

/// A failed hook exits 3 in both output modes; other errors stay at 1.
#[test]
fn a_failed_hook_exits_three() {
    // spec: CLI-255 CLI-175
    let sb = Sandbox::with_hook("exit-src", "exit 9");
    let r = sb.mind(&["meld", &sb.source_spec(), "--accept-hooks"]);
    assert_eq!(r.code, Some(3), "{}", r.stderr);
    // A failed meld leaves nothing registered; meld again with the hook
    // skipped (no terminal, no --accept-hooks) so `hooks run` has a target.
    assert!(sb.mind(&["meld", &sb.source_spec(), "--yes"]).success);
    let r = sb.mind(&["hooks", "run", "exit-src", "--accept-hooks", "--json"]);
    assert_eq!(r.code, Some(3), "{}", r.stdout);
    let r = sb.mind(&["hooks", "list", "no-such-source"]);
    assert_eq!(r.code, Some(1), "{}", r.stderr);
}

/// A source hook's log file is named for the hook too, so two hooks of one
/// source and event are told apart.
#[test]
fn a_source_hook_log_is_named_for_the_hook() {
    // spec: HOOK-165
    let sb = Sandbox::with_hook("name-src", NOISY);
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--hook-log",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let log = PathBuf::from(json(&r)["hooks"][0]["log"].as_str().unwrap().to_string());
    let name = log.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("001-install-") && name.ends_with("_name-src-setup.log"),
        "{name}"
    );
}

/// `hooks run` applies the consumer's recorded `--install-hook` override to
/// install and update hooks, as `upgrade` does: the declared commands it
/// replaced never run.
#[test]
fn hooks_run_applies_the_recorded_override() {
    // spec: HOOK-111 HOOK-56 HOOK-160
    let sb = Sandbox::new("runovr-src");
    let declared = sb.base.join("declared-ran");
    let declared_update = sb.base.join("declared-update-ran");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[[hooks]]\nname = \"setup\"\nrun = \"touch {}\"\nevent = \"install\"\n\n\
             [[hooks]]\nname = \"refresh\"\nrun = \"touch {}\"\nevent = \"update\"\n",
            declared.display(),
            declared_update.display()
        ),
    );
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--accept-hooks",
        "--install-hook",
        "echo mine",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    assert!(
        !declared.exists(),
        "meld ran the override, not the declared hook"
    );

    for (event, declared_cmd, marker) in [
        (
            "install",
            format!("touch {}", declared.display()),
            &declared,
        ),
        (
            "update",
            format!("touch {}", declared_update.display()),
            &declared_update,
        ),
    ] {
        let r = sb.mind(&[
            "hooks",
            "run",
            "runovr-src",
            "--event",
            event,
            "--force",
            "--accept-hooks",
            "--json",
        ]);
        assert!(r.success, "{event}: {} {}", r.stdout, r.stderr);
        let hooks = json(&r)["hooks"].as_array().unwrap().clone();
        assert_eq!(hooks.len(), 1, "{event}: one hook, the override: {hooks:?}");
        assert_eq!(hooks[0]["command"], "echo mine", "{event}");
        assert_eq!(
            hooks[0]["replaced"],
            serde_json::json!([declared_cmd]),
            "{event}"
        );
        assert!(
            !marker.exists(),
            "{event}: the replaced declared command must not run"
        );
    }

    // The disclosure names what the override replaced, as at meld.
    let r = sb.mind_env_stdin(
        &["hooks", "run", "runovr-src", "--force"],
        &[("MIND_TTY", "1")],
        "n\n",
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let out = format!("{}{}", r.stdout, r.stderr);
    assert!(
        out.contains("the user-supplied command replaces the source's declared command")
            && out.contains(&format!("touch {}", declared.display())),
        "{out}"
    );
}

/// An override's record names the declared command it replaced, so `hooks
/// list` shows the declared hook's last result and what ran in its place.
#[test]
fn an_override_result_is_shown_under_the_declared_hook() {
    // spec: HOOK-160 HOOK-168
    let sb = Sandbox::with_hook("ovr-src", "true");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks",
        "--install-hook",
        "echo mine",
    ]);
    assert!(r.success, "meld: {} {}", r.stdout, r.stderr);
    let rec = &json(&r)["hooks"][0];
    assert_eq!(rec["command"], "echo mine", "{rec}");
    assert_eq!(rec["replaced"], serde_json::json!(["true"]), "{rec}");

    let r = sb.mind(&["hooks", "list", "ovr-src"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("last: ok (exit 0, ") && r.stdout.contains("ran in its place: echo mine"),
        "{}",
        r.stdout
    );
}
