//! Integration tests for `mind hooks run` and `mind hooks list`.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture (local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   HOOK-100: hooks run reuses the same disclosure+consent+run machinery
//!   HOOK-101: source install hooks recorded (pending check, already-ran skip,
//!             --force override)
//!   HOOK-102: item-level hooks (install/uninstall) run at the store location
//!   HOOK-103: --event build re-installs transactionally; error on source target
//!   HOOK-104: hooks list reports declared hooks without running them
//!   HOOK-105: an exact match against a registered source identity (including
//!             an item-link instance's `#path` identity) resolves as a source
//!             target ahead of the `#`-split item-ref heuristic
//!   HOOK-106: a skip for want of a terminal names the cause and the exact
//!             remedy, including the event the run selected
//!   HOOK-107: a source run that had work and could not consent to any of it
//!             is an error; nothing-to-do stays exit 0
//!   HOOK-108: the same accounting on item targets
//!   HOOK-109: the `MIND_TTY` seam, and the interactive branches it reaches
//!   CLI-194:  `hooks` verb and target parsing (source vs. item ref)
//!   CLI-195:  `hooks run` with --event and --force flags
//!   CLI-196:  `hooks list` subcommand

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
    /// The process exit code, so a test can pin CLI-175's "1 for a runtime
    /// error" rather than settling for "not zero".
    code: Option<i32>,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-hk-{}-{n}", std::process::id()));
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

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
            code: out.status.code(),
        }
    }

    /// Like [`Sandbox::mind`], but with extra environment variables and a
    /// piped stdin body. `MIND_TTY=1` (HOOK-109) plus a scripted reply is how
    /// a headless test reaches the interactive consent branches, which
    /// `Sandbox::mind` (stdin at `/dev/null`, no override) never takes.
    fn mind_env_stdin(&self, args: &[&str], envs: &[(&str, &str)], stdin: &str) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
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

    /// Like [`Sandbox::mind_env_stdin`], but bounded: the child is killed and
    /// the run flagged as timed out if it has not exited within `secs`.
    ///
    /// `mind_env_stdin` writes the whole reply and then blocks in
    /// `wait_with_output` with no deadline, so a run that prompts more times
    /// than the script answers -- or one that reads a reply the harness never
    /// sends -- would hang the suite instead of failing it. This variant also
    /// drains stdout and stderr on their own threads and writes stdin on a
    /// third, so neither a full output pipe nor a large reply can deadlock the
    /// parent against the child.
    fn mind_env_stdin_bounded(
        &self,
        args: &[&str],
        envs: &[(&str, &str)],
        stdin: &str,
        secs: u64,
    ) -> (Run, bool) {
        use std::io::Read as _;

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn mind");

        let mut sin = child.stdin.take().expect("piped stdin");
        let body = stdin.to_string();
        let writer = std::thread::spawn(move || {
            let _ = sin.write_all(body.as_bytes());
        });
        let mut sout = child.stdout.take().expect("piped stdout");
        let mut serr = child.stderr.take().expect("piped stderr");
        let out_t = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = sout.read_to_end(&mut s);
            s
        });
        let err_t = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = serr.read_to_end(&mut s);
            s
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mut status = None;
        while std::time::Instant::now() < deadline {
            match child.try_wait().expect("try_wait") {
                Some(s) => {
                    status = Some(s);
                    break;
                }
                None => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        let timed_out = status.is_none();
        if timed_out {
            let _ = child.kill();
        }
        // Unconditional, so the child is reaped on both paths (`wait` returns
        // the status `try_wait` already collected when it succeeded).
        let _ = child.wait();
        let _ = writer.join();
        let stdout = String::from_utf8_lossy(&out_t.join().unwrap_or_default()).into_owned();
        let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default()).into_owned();
        let run = Run {
            stdout,
            stderr,
            success: status.is_some_and(|s| s.success()),
            code: status.and_then(|s| s.code()),
        };
        (run, timed_out)
    }

    /// An absolute path inside this sandbox for a hook to touch. Hook working
    /// directories differ by target (a source hook runs in the clone dir, an
    /// item hook in the item's store dir), so a sentinel written to an absolute
    /// sandbox path is observable from the test whichever ran it.
    fn sentinel(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    /// A `touch <abs>` shell command for [`Sandbox::sentinel`].
    fn touch(&self, name: &str) -> String {
        format!("touch {}", self.sentinel(name).display())
    }

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// A `file://` item link into this sandbox's source repo (LNK-1).
    fn link(&self, tail: &str) -> String {
        format!("file://{}/{tail}", self.source.to_string_lossy())
    }

    /// The registered identity of an item-link instance for `path` (LNK-4):
    /// `local/<base>/<repo>#<path>`.
    fn link_identity(&self, path: &str) -> String {
        format!(
            "local/{}/{}#{path}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
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

// ---------------------------------------------------------------------------
// hooks list -- source target (HOOK-104 / CLI-196)
// ---------------------------------------------------------------------------

/// `hooks list <source>` displays declared hooks without running any.
#[test]
fn hooks_list_source_shows_declared_hooks() {
    // spec: HOOK-104 CLI-196
    let sb = Sandbox::new("hooks-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"build helper\"\n",
            "run = \"echo build\"\n",
            "event = \"install\"\n",
            "\n",
            "[[hooks]]\n",
            "name = \"cleanup\"\n",
            "run = \"echo clean\"\n",
            "event = \"uninstall\"\n",
        ),
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld failed: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind(&["hooks", "list", "hooks-src"]);
    assert!(r.success, "hooks list failed: {}\n{}", r.stdout, r.stderr);

    let out = r.stdout;
    assert!(
        out.contains("build helper") || out.contains("echo build"),
        "install hook label should appear: {out}"
    );
    assert!(
        out.contains("cleanup") || out.contains("echo clean"),
        "uninstall hook label should appear: {out}"
    );
    assert!(
        out.contains("[install]"),
        "install event tag should appear: {out}"
    );
    assert!(
        out.contains("[uninstall]"),
        "uninstall event tag should appear: {out}"
    );
}

/// CLI-217 names `hooks list` as one of the four verbs with no `--json`
/// output; `main.rs`'s `json_reserves_stdout` excludes it on that premise, and
/// `list_source_hooks` genuinely never reads `ctx().json`. Pins the premise:
/// `--json` changes nothing about the output (still plain text with the
/// `[install]`/`[uninstall]` tags), not silently-empty or malformed JSON.
#[test]
fn hooks_list_json_flag_is_ignored_and_still_prints_plain_text() {
    // spec: CLI-217 HOOK-104 CLI-196
    let sb = Sandbox::new("hooks-list-json");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let plain = sb.mind(&["hooks", "list", "hooks-list-json"]);
    let json = sb.mind(&["--json", "hooks", "list", "hooks-list-json"]);
    assert!(
        plain.success && json.success,
        "{} {}",
        plain.stderr,
        json.stderr
    );
    assert_eq!(
        plain.stdout, json.stdout,
        "`--json` must not change hooks list's output at all (no JSON branch exists)"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(json.stdout.trim()).is_err(),
        "hooks list --json must not accidentally look like a JSON document: {:?}",
        json.stdout
    );
}

/// `hooks list <source>` on a source with no hooks prints a note, not an error.
#[test]
fn hooks_list_source_no_hooks_prints_note() {
    // spec: HOOK-104 CLI-196
    let sb = Sandbox::new("no-hooks");
    // No mind.toml -> no hooks declared.
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld failed: {}", r.stderr);

    let r = sb.mind(&["hooks", "list", "no-hooks"]);
    assert!(
        r.success,
        "hooks list should succeed even with no hooks: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no source-level hooks"),
        "should note absence of hooks: {}",
        r.stdout
    );
}

/// `hooks list <unknown>` fails when no source matches the selector.
#[test]
fn hooks_list_unknown_source_fails() {
    // spec: CLI-196
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld failed: {}", r.stderr);

    let r = sb.mind(&["hooks", "list", "does-not-exist"]);
    assert!(!r.success, "should fail for unknown source");
}

// ---------------------------------------------------------------------------
// hooks list -- item target (HOOK-104 / CLI-196)
// ---------------------------------------------------------------------------

/// `hooks list <source>#<item>` shows the item's hooks without running them.
#[test]
fn hooks_list_item_shows_hooks() {
    // spec: HOOK-104 CLI-196
    let sb = Sandbox::new("item-hooks-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"myscan\"\n",
            "path = \"skills/myscan\"\n",
            "install = \"echo scan-installed\"\n",
            "uninstall = \"echo scan-removed\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/myscan/SKILL.md",
        "---\ndescription: scan skill\n---\n# scan\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld failed: {}", r.stderr);
    let r = sb.mind(&["learn", "myscan", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn failed: {}", r.stderr);

    let r = sb.mind(&["hooks", "list", "item-hooks-src#myscan"]);
    assert!(
        r.success,
        "hooks list item failed: {}\n{}",
        r.stdout, r.stderr
    );

    let out = r.stdout;
    assert!(
        out.contains("[install]"),
        "install hook should be listed: {out}"
    );
    assert!(
        out.contains("echo scan-installed") || out.contains("scan-installed"),
        "install hook command should appear: {out}"
    );
}

/// `hooks list <source>#<unknown>` fails when the item is not installed.
#[test]
fn hooks_list_item_not_installed_fails() {
    // spec: CLI-196
    let sb = Sandbox::new("item-miss");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "list", "item-miss#ghost"]);
    assert!(!r.success, "should fail when item not installed");
}

// ---------------------------------------------------------------------------
// hooks run -- error: --event build on source target (HOOK-103 / CLI-195)
// ---------------------------------------------------------------------------

/// `hooks run --event build <source>` (no `#`) is rejected immediately.
#[test]
fn hooks_run_build_event_on_source_target_errors() {
    // spec: HOOK-103 CLI-195
    let sb = Sandbox::new("bld-src");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "build", "bld-src"]);
    assert!(
        !r.success,
        "should fail with build-event-requires-item-target"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("build") && combined.contains("item"),
        "error should mention build and item: {combined}"
    );
}

// ---------------------------------------------------------------------------
// hooks run -- source target, install event (HOOK-100 / HOOK-101)
// ---------------------------------------------------------------------------

/// In non-TTY, `hooks run --event install <source>` skips the hook, but
/// because there WAS a hook to run and consent was unavailable for it (not a
/// terminal), the command is now a non-zero exit naming the cause and the
/// exact `--dangerously-skip-install-hook-check` remedy (HOOK-106/HOOK-107),
/// not a silent exit 0 (U43).
#[test]
fn hooks_run_source_install_skips_in_non_tty() {
    // spec: HOOK-100 HOOK-101 HOOK-106 HOOK-107
    let sb = Sandbox::new("src-skip");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // The registered source identity is `local/<base-name>/src-skip`, not the
    // bare "src-skip" selector typed on the command line.
    let identity = format!(
        "local/{}/src-skip",
        sb.base.file_name().unwrap().to_string_lossy()
    );

    // stdin is null (non-TTY): the hook is skipped, and the run is now an
    // error since it had work to do and consent was unavailable for it.
    let r = sb.mind(&["hooks", "run", "--event", "install", "src-skip"]);
    assert!(
        !r.success,
        "a non-TTY skip with a hook that existed must be a non-zero exit: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    // Output should note the skip, the cause, and the exact remedy.
    assert!(
        out.contains("skipped") || out.contains("skip"),
        "should mention skip in non-TTY: {out}"
    );
    assert!(
        out.contains("not a terminal"),
        "should name the cause: {out}"
    );
    assert!(
        out.contains(&format!(
            "mind hooks run {identity} --event install --dangerously-skip-install-hook-check"
        )),
        "should print the exact copy-pasteable remedy, re-selecting the event \
         the run selected (HOOK-106): {out}"
    );
    // The error itself also names the target and the reason.
    assert!(
        r.stderr.contains("skipped for want of consent"),
        "should surface the HooksNotRun error: {}",
        r.stderr
    );
}

/// `hooks run --dangerously-skip-install-hook-check` actually runs the hook.
#[test]
fn hooks_run_source_install_runs_with_dangerously_skip() {
    // spec: HOOK-100 HOOK-101 CLI-195
    let sb = Sandbox::new("src-run");
    // The hook creates a sentinel file relative to the clone dir.
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"write sentinel\"\n",
            "run = \"touch ran.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "src-run",
    ]);
    assert!(
        r.success,
        "hooks run with skip flag should succeed: {}\n{}",
        r.stdout, r.stderr
    );

    // For a locally-melded source, clone_dir == sb.source (the source repo
    // itself, not a copy under mind_home/sources/). The sentinel is created there.
    let sentinel = sb.source.join("ran.sentinel");
    assert!(
        sentinel.exists(),
        "hook should have created ran.sentinel in the clone dir ({})",
        sb.source.display()
    );
}

// ---------------------------------------------------------------------------
// FORMERLY-LEAKING PROGRESS LINES in `hooks_cmd.rs`. Two `println!` sites here
// carry no `note:`/`warning:` marker and are not composed with `out.warn()`,
// so the earlier CLI-217 conversion's decidable scope rule -- "any println!
// whose text begins `note:` or `warning:`, or which is composed with
// `out.warn()`" -- did not select them, for the same reason it did not select
// `install.rs`'s `println!("running build hook for {}", ...)` or
// `commands.rs`'s `println!("running install hook '{}' for {}", ...)`. That
// marker-prefix rule is the reason this class kept shipping.
//
// Neither line was converted. Under `--json` the process runs with fd 1
// pointed at stderr for the whole run (main.rs's `json_stdout`), so no
// `println!` in this file -- or any other -- can reach stdout, and `main`
// writes the single document at the end. Both still require
// `--dangerously-skip-install-hook-check` to reach without a TTY.
// ---------------------------------------------------------------------------

/// `src/hooks_cmd.rs:run_source_hooks`'s `println!("running {event} hook
/// '{}' for {}", ...)` (the progress announcement right before a hook actually
/// executes) polluted `--json` stdout when a later hook in the same run failed:
/// the first hook's announcement was already on stdout before the second
/// hook's failure reached main's CLI-181 envelope.
#[test]
fn hooks_run_running_hook_note_is_one_document() {
    // spec: CLI-217 CLI-181
    let sb = Sandbox::new("running-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"first\"\n",
            "run = \"true\"\n",
            "event = \"install\"\n",
            "[[hooks]]\n",
            "name = \"second\"\n",
            "run = \"false\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "running-leak",
    ]);
    assert!(
        !r.success,
        "the second hook's failure must still fail the run: {}\n{}",
        r.stdout, r.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        doc["error"]["kind"].is_string(),
        "the one document is the CLI-181 error envelope: {doc:#}"
    );
    assert!(
        r.stderr.contains("running install hook 'first'"),
        "the announcement must still be emitted, on stderr: {:?}",
        r.stderr
    );
}

/// `src/hooks_cmd.rs:run_item_build`'s `println!("rebuilding {} via
/// transactional reinstall", ...)` polluted `--json` stdout the same way, when
/// the transactional reinstall that follows it failed.
#[test]
fn hooks_run_build_rebuilding_note_is_one_document() {
    // spec: CLI-217 CLI-181 HOOK-103
    let sb = Sandbox::new("rebuild-leak");
    sb.write_and_commit("skills/widget/SKILL.md", "---\ndescription: d\n---\n# w\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&[
        "learn",
        "widget",
        "--dangerously-skip-install-hook-check",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    // Break the source so the transactional reinstall staging step -- which
    // runs after `run_item_build` has already printed "rebuilding ..." --
    // fails: a `{{ns:}}` reference to a non-existent sibling is a
    // `BadReference` install-time error (NS-28).
    write(
        &sb.source.join("skills/widget/SKILL.md"),
        "---\ndescription: d\n---\n# w\nSee {{ns:does-not-exist}}.\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "break reference"]);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "build",
        "--dangerously-skip-build-hook-check",
        "rebuild-leak#widget",
    ]);
    assert!(
        !r.success,
        "the rebuild must fail once the item is gone from the source: {}\n{}",
        r.stdout, r.stderr
    );
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        r.stderr.contains("rebuilding"),
        "the announcement must still be emitted, on stderr: {:?}",
        r.stderr
    );
}

/// After a hook runs at the current commit, re-running without --force skips it.
#[test]
fn hooks_run_source_install_already_ran_not_rerun() {
    // spec: HOOK-101
    let sb = Sandbox::new("src-repeat");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"counter\"\n",
            "run = \"echo RAN >> counter.log\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // First run: hook executes (dangerously-skip).
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "src-repeat",
    ]);
    assert!(r.success, "first run: {}\n{}", r.stdout, r.stderr);

    // For a locally-melded source, the hook runs in sb.source (the working tree).
    let log1 = read_log_file(&sb.source.join("counter.log"));
    assert_eq!(log1.len(), 1, "hook should have run exactly once: {log1:?}");

    // Second run without --force: already ran at current commit, should skip.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "src-repeat",
    ]);
    assert!(r.success, "second run: {}\n{}", r.stdout, r.stderr);
    let log2 = read_log_file(&sb.source.join("counter.log"));
    assert_eq!(
        log2.len(),
        1,
        "hook should not have run again (already-ran): {log2:?}"
    );
}

/// `--force` overrides the already-ran guard and reruns the hook.
#[test]
fn hooks_run_source_install_force_reruns_hook() {
    // spec: HOOK-101 CLI-195
    let sb = Sandbox::new("src-force");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"counter\"\n",
            "run = \"echo RAN >> counter.log\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // First run.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "src-force",
    ]);
    assert!(r.success, "first run: {}", r.stderr);

    let log1 = read_log_file(&sb.source.join("counter.log"));
    assert_eq!(log1.len(), 1, "first run count");

    // Second run with --force: should run again.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--force",
        "--dangerously-skip-install-hook-check",
        "src-force",
    ]);
    assert!(r.success, "force re-run: {}\n{}", r.stdout, r.stderr);
    let log2 = read_log_file(&sb.source.join("counter.log"));
    assert_eq!(log2.len(), 2, "--force should cause a second run: {log2:?}");
}

// ---------------------------------------------------------------------------
// hooks run -- source target, uninstall event (HOOK-100)
// ---------------------------------------------------------------------------

/// In non-TTY, `hooks run --event uninstall <source>` skips the hook, and
/// (like the install case) is now a non-zero exit since a hook existed and
/// consent was unavailable for it (HOOK-107).
#[test]
fn hooks_run_source_uninstall_skips_in_non_tty() {
    // spec: HOOK-100 HOOK-107
    let sb = Sandbox::new("src-unskip");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"teardown\"\n",
            "run = \"echo teardown-ran\"\n",
            "event = \"uninstall\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "uninstall", "src-unskip"]);
    assert!(
        !r.success,
        "uninstall non-TTY skip with a hook that existed must be a non-zero exit: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("skipped for want of consent"),
        "should surface the HooksNotRun error: {}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// hooks run -- source with no hooks (CLI-194)
// ---------------------------------------------------------------------------

/// `hooks run --event install <source>` on a source with no install hooks
/// prints a note and exits 0.
#[test]
fn hooks_run_source_no_hooks_prints_note() {
    // spec: CLI-194 HOOK-100
    let sb = Sandbox::new("no-hook-src");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "no-hook-src"]);
    assert!(
        r.success,
        "should succeed with no hooks: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no") && r.stdout.contains("hook"),
        "should note absence of hooks: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// hooks run -- item target, install/uninstall events (HOOK-102)
// ---------------------------------------------------------------------------

/// `hooks run --event install <source>#<item>` runs the item's install hook at
/// its store location (with --dangerously-skip-install-hook-check).
#[test]
fn hooks_run_item_install_hook_runs() {
    // spec: HOOK-102 CLI-194
    let sb = Sandbox::new("item-install-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"scanner\"\n",
            "path = \"skills/scanner\"\n",
            "install = \"touch hook-ran.sentinel\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    // Learn the item; skip its install hook for the learn step.
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    // Now explicitly run the item's install hook.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "item-install-src#scanner",
    ]);
    assert!(
        r.success,
        "hooks run item install: {}\n{}",
        r.stdout, r.stderr
    );

    // The sentinel should now exist under the item's store dir at
    // mind_home/store/skill/scanner/.
    let store_sentinel = sb
        .mind_home
        .join("store")
        .join("skill")
        .join("scanner")
        .join("hook-ran.sentinel");
    assert!(
        store_sentinel.exists(),
        "item install hook should have created hook-ran.sentinel in the store at {}",
        store_sentinel.display()
    );
}

/// `hooks run --event uninstall <source>#<item>` in non-TTY skips the hook,
/// and (HOOK-108) reports that it could not consent to it rather than exiting
/// 0 as it did while the accounting was source-scoped.
#[test]
fn hooks_run_item_uninstall_hook_skips_in_non_tty() {
    // spec: HOOK-102 HOOK-108 CLI-194
    let sb = Sandbox::new("item-uninstall-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"fetcher\"\n",
            "path = \"skills/fetcher\"\n",
            "uninstall = \"echo fetcher-removed\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/fetcher/SKILL.md",
        "---\ndescription: fetcher\n---\n# fetcher\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "fetcher"]);
    assert!(r.success, "learn: {}", r.stderr);

    // In non-TTY the uninstall hook is skipped, and the skip is reported.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "uninstall",
        "item-uninstall-src#fetcher",
    ]);
    assert!(
        r.stdout.contains("skipped uninstall hook"),
        "the hook must be skipped, not run: {}",
        r.stdout
    );
    assert!(
        !r.success,
        "an item uninstall hook skipped for want of a terminal must be \
         reported, not a silent exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains(
            "mind hooks run item-uninstall-src#fetcher --event uninstall \
             --dangerously-skip-install-hook-check"
        ),
        "the remedy must re-select the uninstall event: {}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// hooks run -- item target, build event (HOOK-103)
// ---------------------------------------------------------------------------

/// `hooks run --event build <source>#<item>` re-installs the item transactionally.
/// With --dangerously-skip-build-hook-check, a build hook runs unattended.
#[test]
fn hooks_run_item_build_reinstalls() {
    // spec: HOOK-103 CLI-195
    let sb = Sandbox::new("item-bld");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"builder\"\n",
            "path = \"skills/builder\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/builder/SKILL.md",
        "---\ndescription: builder\n---\n# builder\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "builder"]);
    assert!(r.success, "learn: {}", r.stderr);

    // Re-install via build event.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "build",
        "--dangerously-skip-build-hook-check",
        "item-bld#builder",
    ]);
    assert!(
        r.success,
        "hooks run build should reinstall: {}\n{}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("rebuild")
            || combined.contains("reinstall")
            || combined.contains("builder"),
        "output should mention rebuild: {combined}"
    );
}

// ---------------------------------------------------------------------------
// CLI-194 multi-match fan-out: a selector matching several sources/items runs
// each in turn (the core CLI-194 claim, absent from the implementor's suite).
// ---------------------------------------------------------------------------

/// A `*` source selector matching several melded sources runs each source's
/// install hook in turn.
#[test]
fn hooks_run_source_glob_runs_each_matched_source() {
    // spec: CLI-194 HOOK-101
    let sb = Sandbox::new("multi-a");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"a-setup\"\n",
            "run = \"touch a-ran.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );

    // A second, independent source repo in the same sandbox.
    let src_b = sb.base.join("multi-b");
    init_source_repo(
        &src_b,
        concat!(
            "[[hooks]]\n",
            "name = \"b-setup\"\n",
            "run = \"touch b-ran.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld a: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["meld", src_b.to_string_lossy().as_ref()]);
    assert!(r.success, "meld b: {}\n{}", r.stdout, r.stderr);

    // A single `*` selector matches BOTH melded sources; each source's install
    // hook must run in turn.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "*",
    ]);
    assert!(r.success, "hooks run '*': {}\n{}", r.stdout, r.stderr);

    // Locally-melded sources run their hooks in the source tree itself.
    assert!(
        sb.source.join("a-ran.sentinel").exists(),
        "source a's install hook must run under a fan-out selector"
    );
    assert!(
        src_b.join("b-ran.sentinel").exists(),
        "source b's install hook must run under a fan-out selector"
    );
}

/// An item ref whose name is a glob (`<source>#*`) matching several installed
/// items runs each item's install hook in turn.
#[test]
fn hooks_run_item_glob_runs_each_matched_item() {
    // spec: CLI-194 HOOK-102
    let sb = Sandbox::new("multi-item-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"alpha\"\n",
            "path = \"skills/alpha\"\n",
            "install = \"touch alpha-ran.sentinel\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"beta\"\n",
            "path = \"skills/beta\"\n",
            "install = \"touch beta-ran.sentinel\"\n",
        ),
    );
    sb.write_and_commit("skills/alpha/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/beta/SKILL.md", "---\ndescription: b\n---\n# b\n");

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "alpha", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn alpha: {}", r.stderr);
    let r = sb.mind(&["learn", "beta", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn beta: {}", r.stderr);

    // `<source>#*` matches BOTH installed items; each item's install hook runs.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "multi-item-src#*",
    ]);
    assert!(r.success, "hooks run item glob: {}\n{}", r.stdout, r.stderr);

    let alpha = sb.mind_home.join("store/skill/alpha/alpha-ran.sentinel");
    let beta = sb.mind_home.join("store/skill/beta/beta-ran.sentinel");
    assert!(
        alpha.exists(),
        "alpha's install hook must run under an item glob: {}",
        alpha.display()
    );
    assert!(
        beta.exists(),
        "beta's install hook must run under an item glob: {}",
        beta.display()
    );
}

// ---------------------------------------------------------------------------
// HOOK-101 / HOOK-55 pending semantics: a SKIP records no run-commit and stays
// pending, unlike a RUN which records the commit and suppresses a plain re-run.
// ---------------------------------------------------------------------------

/// A skipped install hook (non-TTY) records no run-commit, so it stays pending:
/// a later bypassed run still offers and executes it.
#[test]
fn hooks_run_source_install_skip_stays_pending() {
    // spec: HOOK-101 HOOK-55 HOOK-107
    let sb = Sandbox::new("skip-pending");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"touch ran.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // First run in non-TTY: the hook is skipped (records ran_at = None). The
    // run itself is now a non-zero exit (HOOK-107: a hook existed and consent
    // was unavailable), but the skip-records-no-commit behavior under test
    // still happens before that error is returned.
    let r = sb.mind(&["hooks", "run", "--event", "install", "skip-pending"]);
    assert!(
        !r.success,
        "a non-TTY skip with a hook that existed must be a non-zero exit: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.source.join("ran.sentinel").exists(),
        "a skipped hook must not have executed"
    );

    // Second run WITH the bypass: because a skip records no run-commit, the hook
    // is still pending and now runs. If a skip had wrongly recorded the current
    // commit, the pending filter would suppress it here and the sentinel would
    // be absent.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "skip-pending",
    ]);
    assert!(r.success, "bypassed run: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.source.join("ran.sentinel").exists(),
        "a skipped install hook stays pending and runs on a later bypassed run"
    );
}

// ---------------------------------------------------------------------------
// HOOK-107: "ran nothing" is an error only when consent was unavailable for
// hooks that actually existed. "Nothing to do" (no hooks declared, or every
// hook already ran at the current commit) stays exit 0.
// ---------------------------------------------------------------------------

/// A non-TTY `hooks run` where the install hook already ran at the current
/// commit has NOTHING to do (HOOK-101's pending filter excludes it before
/// consent is ever asked), so it stays exit 0 even with no
/// `--dangerously-skip-install-hook-check` -- unlike the pending case in
/// `hooks_run_source_install_skips_in_non_tty`, which is now an error.
#[test]
fn hooks_run_source_install_already_ran_non_tty_stays_exit_zero() {
    // spec: HOOK-107 HOOK-101
    let sb = Sandbox::new("already-ran-quiet");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // Run it once via the bypass so it is recorded at the current commit.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "already-ran-quiet",
    ]);
    assert!(
        r.success,
        "first (bypassed) run: {}\n{}",
        r.stdout, r.stderr
    );

    // A second, non-TTY run with no bypass: the hook is already up to date, so
    // there is nothing to do -- this must stay exit 0, not HooksNotRun.
    let r = sb.mind(&["hooks", "run", "--event", "install", "already-ran-quiet"]);
    assert!(
        r.success,
        "a run with nothing to do must stay exit 0 even in non-TTY: {}\n{}",
        r.stdout, r.stderr
    );
}

// ---------------------------------------------------------------------------
// HOOK-104 / CLI-196: hooks list surfaces pending state and the recorded
// last-ran commit for a source install hook.
// ---------------------------------------------------------------------------

/// `hooks list` shows an install hook as pending before it runs and reports the
/// commit it last ran at once recorded.
#[test]
fn hooks_list_source_shows_pending_then_recorded_commit() {
    // spec: HOOK-104 CLI-196 HOOK-55
    let sb = Sandbox::new("list-status");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo hi\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // Before any run, the install hook is pending.
    let r = sb.mind(&["hooks", "list", "list-status"]);
    assert!(r.success, "list before: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("pending"),
        "an unrun install hook must list as pending: {}",
        r.stdout
    );

    // Run it (records the source's current commit as the run-commit).
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "list-status",
    ]);
    assert!(r.success, "run: {}\n{}", r.stdout, r.stderr);

    // Now list reports the commit it last ran at and no longer shows pending.
    let r = sb.mind(&["hooks", "list", "list-status"]);
    assert!(r.success, "list after: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("ran at"),
        "a recorded install hook must show its last-ran commit: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("pending"),
        "a hook recorded at the current commit must not show as pending: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-103 / LIFE-4: a failed rebuild via --event build leaves the live store
// copy untouched (transactional path).
// ---------------------------------------------------------------------------

/// `hooks run --event build` whose build hook fails leaves the existing store
/// copy intact (LIFE-4): the prior build output survives.
#[test]
fn hooks_run_build_failure_leaves_live_copy_untouched() {
    // spec: HOOK-103
    let sb = Sandbox::new("bld-fail-src");
    let trigger = sb.base.join("fail-trigger");
    let trigger_str = trigger.to_string_lossy().into_owned();
    // The build succeeds while the trigger is absent (writing built.sentinel into
    // staging, which lands in the store) and fails once the trigger exists.
    let mind_toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"bt\"\n",
            "path = \"tools/bt\"\n",
            "build = \"test ! -f {trigger} && touch built.sentinel\"\n",
        ),
        trigger = trigger_str,
    );
    sb.write_and_commit("mind.toml", &mind_toml);
    sb.write_and_commit("tools/bt/TOOL.md", "---\ndescription: bt\n---\n# bt\n");

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    // First install: the build succeeds and the store copy gets built.sentinel.
    let r = sb.mind(&["learn", "tool:bt", "--dangerously-skip-build-hook-check"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let store = sb.mind_home.join("store/tool/bt");
    let sentinel = store.join("built.sentinel");
    let tool_md = store.join("TOOL.md");
    assert!(
        sentinel.exists(),
        "the first build must create the sentinel: {}",
        sentinel.display()
    );
    assert!(tool_md.exists(), "the store copy must exist after install");

    // Arm the trigger so the next build fails.
    write(&trigger, "x");

    // Rebuild via --event build: the build hook now exits non-zero.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "build",
        "--dangerously-skip-build-hook-check",
        "bld-fail-src#bt",
    ]);
    assert!(
        !r.success,
        "a rebuild must fail when the build hook exits non-zero: {}\n{}",
        r.stdout, r.stderr
    );

    // LIFE-4: the live store copy is untouched -- content still present.
    assert!(
        tool_md.exists(),
        "the store TOOL.md must survive a failed rebuild: {}",
        tool_md.display()
    );
    assert!(
        sentinel.exists(),
        "the prior build output must survive a failed rebuild: {}",
        sentinel.display()
    );
}

// ---------------------------------------------------------------------------
// HOOK-105: exact-source-identity precedence over the `#`-split heuristic --
// reaches an item-link instance's source-level hooks by its own `#<path>`
// identity, and leaves ordinary item-ref resolution (including the triple-`#`
// form for an item inside a link instance) unregressed.
// ---------------------------------------------------------------------------

/// `hooks list <link-instance-identity>` -- where the identity itself carries
/// a `#<path>` suffix (LNK-4) -- reaches the instance's SOURCE-level hooks
/// declared in the linked repo's root `mind.toml`, instead of being misread as
/// an item ref (`source=local/.../repo`, `name=<path>`) that matches nothing.
#[test]
fn hooks_list_link_instance_identity_reaches_source_hooks() {
    // spec: HOOK-105
    let sb = Sandbox::new("link-repo");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"link setup\"\n",
            "run = \"echo link-setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review skill\n---\n# review\n",
    );

    // Register the item-link instance (LNK-6): `learn <url>` registers and
    // installs the single linked skill; the instance's own identity is
    // `local/<base>/<repo>#skills/review` (LNK-4).
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/review");

    // Before HOOK-105, this target would be misparsed as an item ref
    // (source="local/base/repo", name="skills/review") matching no installed
    // item, and error NotInstalled.
    let r = sb.mind(&["hooks", "list", &identity]);
    assert!(
        r.success,
        "hooks list <link-identity> should reach the source target: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    assert!(
        out.contains("source:"),
        "should be read as a source target, not an item ref: {out}"
    );
    assert!(
        out.contains("link setup") || out.contains("echo link-setup-ran"),
        "the instance's declared source hook must be listed: {out}"
    );
}

/// A plain `<source>#<item>` target that does NOT exactly match any
/// registered source identity still resolves as an item ref: the HOOK-105
/// exact-match check must not swallow ordinary item targets.
#[test]
fn hooks_list_plain_source_hash_item_still_resolves_as_item() {
    // spec: HOOK-105
    let sb = Sandbox::new("plain-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"widget\"\n",
            "path = \"skills/widget\"\n",
            "install = \"echo widget-installed\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: widget\n---\n# widget\n",
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "widget", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    // "plain-src#widget" is not itself a registered source identity (the
    // registered source's identity is just "plain-src"), so it must resolve
    // as an item ref, exactly as before HOOK-105.
    let r = sb.mind(&["hooks", "list", "plain-src#widget"]);
    assert!(
        r.success,
        "hooks list <source>#<item> must still resolve as an item ref: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    assert!(
        out.contains("item:"),
        "should be read as an item target, not a source: {out}"
    );
    assert!(
        out.contains("echo widget-installed") || out.contains("widget-installed"),
        "the item's install hook should be listed: {out}"
    );
}

/// The triple-`#` form `<link-identity>#<item>` (the link instance's own
/// `#<path>` identity plus the item-ref separator) still resolves as an item
/// ref for that item, not as the source: HOOK-105's exact-match check only
/// fires when the WHOLE target matches a registered source identity, so
/// appending `#<item>` to it must fall through to ordinary item-ref parsing.
#[test]
fn hooks_list_item_inside_link_instance_still_resolves_as_item() {
    // spec: HOOK-105
    let sb = Sandbox::new("link-item-repo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review skill\n---\n# review\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/review");
    let target = format!("{identity}#review");

    let r = sb.mind(&["hooks", "list", &target]);
    assert!(
        r.success,
        "hooks list <link-identity>#<item> should resolve as an item ref: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    assert!(
        out.contains("item:"),
        "should be read as an item target, not a source: {out}"
    );
}

/// `hooks run` (not just `hooks list`) against a link-instance identity also
/// reaches and executes the instance's SOURCE-level hooks. `resolve_hook_target`
/// (HOOK-105) is shared by both entry points, but only `list` was exercised
/// above; this confirms `run` actually resolves AND runs the hook rather than
/// just resolving to the right branch.
#[test]
fn hooks_run_link_instance_identity_reaches_and_runs_source_hooks() {
    // spec: HOOK-105
    let sb = Sandbox::new("link-run-repo");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"link setup\"\n",
            "run = \"touch link-setup.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review skill\n---\n# review\n",
    );

    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/review");

    // Before HOOK-105, this would be misparsed as an item ref
    // (source="local/.../repo", name="skills/review") and error NotInstalled,
    // never reaching the source-level hook runner.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        &identity,
    ]);
    assert!(
        r.success,
        "hooks run <link-identity> should run the source's install hook: {}\n{}",
        r.stdout, r.stderr
    );

    // A link instance is always a cloned snapshot (never a linked working
    // tree, per LNK-4), so the hook ran in the source's clone under
    // mind_home/sources/, not sb.source itself.
    let sentinel = find_sentinel(&sb.mind_home, "link-setup.sentinel");
    assert!(
        sentinel.is_some(),
        "the instance's install hook should have run in its clone dir: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// The triple-`#` composed form also resolves correctly through `hooks run`,
/// not just `hooks list`: it must run against the ITEM (using its installed
/// store location, HOOK-102), not be misread as the link instance's source.
#[test]
fn hooks_run_item_inside_link_instance_resolves_as_item_not_source() {
    // spec: HOOK-105
    let sb = Sandbox::new("link-item-run-repo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review skill\n---\n# review\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/review");
    let target = format!("{identity}#review");

    let r = sb.mind(&["hooks", "run", "--event", "install", &target]);
    assert!(
        r.success,
        "hooks run <link-identity>#<item> should resolve and run as an item ref: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    // The item declares no install hooks, so the item-hook runner's "no hooks
    // declared" note names the ITEM key, confirming item resolution (not a
    // source run, which would print "running install hook ... for <source>"
    // or "no install hooks declared for source ...").
    assert!(
        out.contains("no install hooks declared for skill:review"),
        "should resolve to the installed item skill:review, not the source: {out}"
    );
    assert!(
        !out.contains("no install hooks declared for source"),
        "must not be misread as a source target: {out}"
    );
}

/// An `@alias`-suffixed source identity (no `#`, STO-58) as a `hooks`/`hooks
/// list` target resolves as a source. `@alias` carries no `#`, so the ordinary
/// no-`#`-is-source heuristic already covers it, but the exact-match check
/// added for HOOK-105 runs unconditionally first: this confirms it still finds
/// the aliased identity and does not regress this pre-existing case.
#[test]
fn hooks_list_alias_suffixed_source_identity_resolves_as_source() {
    // spec: HOOK-105
    let sb = Sandbox::new("alias-src");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"alias setup\"\n",
            "run = \"echo alias-setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );

    let r = sb.mind(&["meld", "--namespace", "jk", &sb.source_spec()]);
    assert!(r.success, "meld --namespace: {}\n{}", r.stdout, r.stderr);

    let identity = format!(
        "local/{}/{}@jk",
        sb.base.file_name().unwrap().to_string_lossy(),
        sb.source.file_name().unwrap().to_string_lossy(),
    );

    let r = sb.mind(&["hooks", "list", &identity]);
    assert!(
        r.success,
        "hooks list <alias-identity> should resolve as a source: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    assert!(
        out.contains("source:"),
        "should be read as a source target: {out}"
    );
    assert!(
        out.contains("alias setup") || out.contains("echo alias-setup-ran"),
        "the aliased source's declared hook must be listed: {out}"
    );
}

/// A target that contains `#` but matches NO registered source (by exact
/// string or by the ordinary split) and names no installed item still takes
/// the ordinary item-ref error path -- a clean `NotInstalled`/`ItemNotFound`
/// failure, not a panic and not a misreported "source not found".
#[test]
fn hooks_list_unknown_hash_target_errors_cleanly_as_item_not_found() {
    // spec: HOOK-105
    let sb = Sandbox::new("unknown-hash-src");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "list", "totally/unknown-repo#ghost-item"]);
    assert!(
        !r.success,
        "an unresolvable #-target must fail, not succeed: {}\n{}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("not installed") || combined.contains("no item matches"),
        "should be the ordinary item-ref error, not a source-not-found misreport: {combined}"
    );
    assert!(!combined.contains("panic"), "must not panic: {combined}");
}

/// Leading/trailing whitespace around a link-instance identity target is
/// trimmed before the exact-match check (HOOK-105 spec: "the whole target
/// string, trimmed"), so it still resolves as a source.
#[test]
fn hooks_list_link_instance_identity_with_surrounding_whitespace_still_matches() {
    // spec: HOOK-105
    let sb = Sandbox::new("ws-link-repo");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"link setup\"\n",
            "run = \"echo link-setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review skill\n---\n# review\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/review");
    let padded = format!("  {identity}\t\n");

    let r = sb.mind(&["hooks", "list", &padded]);
    assert!(
        r.success,
        "hooks list <padded link-identity> should still resolve as a source: {}\n{}",
        r.stdout, r.stderr
    );
    let out = r.stdout;
    assert!(
        out.contains("source:"),
        "whitespace-padded exact identity should still be read as a source target: {out}"
    );
    assert!(
        out.contains("link setup") || out.contains("echo link-setup-ran"),
        "the instance's declared source hook must be listed: {out}"
    );
}

// ---------------------------------------------------------------------------
// HOOK-105: genuine ambiguity between a registered source identity and the
// item-ref reading of the same string, and its two disambiguation escapes.
// ---------------------------------------------------------------------------

/// When a target string exactly matches BOTH a registered source's identity
/// AND, under the ordinary `#`-split heuristic, an installed item, `hooks
/// list` must error naming both disambiguated forms rather than silently
/// picking the source the way the exact-match check alone would.
#[test]
fn hooks_target_exactly_matching_source_and_item_errors_ambiguous() {
    // spec: HOOK-105
    let sb = Sandbox::new("amb-src");
    // A root-level skill directory `dev/SKILL.md`, declared via an explicit
    // mind.toml item so its bare (and effective) name is exactly `dev` --
    // matching the basename a link straight into the same root-level path
    // carries as its own identity suffix.
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"dev\"\n",
            "path = \"dev\"\n",
        ),
    );
    sb.write_and_commit("dev/SKILL.md", "---\ndescription: dev skill\n---\n# dev\n");

    // Register the link instance FIRST (register-only, nothing installed yet),
    // so the NS-43 cross-source collision check has nothing to conflict with:
    // its identity is exactly "local/<base>/amb-src#dev" -- spelled
    // identically to the item-ref reading of that string (source
    // "local/<base>/amb-src", item "dev").
    let r = sb.mind(&["meld", &sb.link("tree/main/dev"), "--register-only"]);
    assert!(r.success, "link meld: {}\n{}", r.stdout, r.stderr);

    // Now plain meld + install: registers source "local/<base>/amb-src" with
    // skill:dev installed under it. No collision yet (the link installed
    // nothing), so this succeeds even non-interactively.
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    // The link instance also offers its own catalog item named `dev` (LNK-7),
    // so a bare `learn dev` would itself be ambiguous across the two sources;
    // qualify by source so this installs from the PLAIN source only.
    let r = sb.mind(&[
        "learn",
        "amb-src#dev",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "learn: {}", r.stderr);

    let ambiguous = sb.link_identity("dev");

    let r = sb.mind(&["hooks", "list", &ambiguous]);
    assert!(
        !r.success,
        "an exact source/item collision must error, not silently resolve: {}\n{}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("ambiguous"),
        "must say ambiguous: {combined}"
    );
    assert!(
        combined.contains(&ambiguous),
        "must name the ambiguous target: {combined}"
    );
    assert!(
        combined.contains(&format!("source:{ambiguous}")),
        "must name the source-targeting escape: {combined}"
    );
    assert!(
        combined.contains("skill:dev"),
        "must name the kind-qualified item escape: {combined}"
    );

    // `hooks run` (not just `hooks list`) must also refuse the ambiguous
    // target rather than picking a winner.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        &ambiguous,
    ]);
    assert!(
        !r.success,
        "hooks run must also refuse the ambiguous target: {}\n{}",
        r.stdout, r.stderr
    );
}

/// The same identity-colliding setup as above, but WITHOUT installing the
/// colliding item: the target then matches only the registered source, and
/// resolves as a source exactly as before HOOK-105's ambiguity check existed
/// (the ambiguity check must not fire when there is no actual collision).
#[test]
fn hooks_target_source_only_match_resolves_as_source() {
    // spec: HOOK-105
    let sb = Sandbox::new("src-only");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"root setup\"\n",
            "run = \"echo root-setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    sb.write_and_commit("dev/SKILL.md", "---\ndescription: dev skill\n---\n# dev\n");

    // Only the link instance is registered; the repo's plain source is never
    // melded, so no item named `dev` is ever installed anywhere.
    let r = sb.mind(&["learn", &sb.link("tree/main/dev")]);
    assert!(r.success, "learn <url>: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("dev");
    let r = sb.mind(&["hooks", "list", &identity]);
    assert!(
        r.success,
        "an unambiguous exact source match must still resolve as a source: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("source:"),
        "must be read as a source target: {}",
        r.stdout
    );
}

/// A target containing `#` that does not exactly name any registered source
/// (so it can never collide) but does resolve to an installed item is read as
/// an item, exactly as before HOOK-105.
#[test]
fn hooks_target_item_only_match_resolves_as_item() {
    // spec: HOOK-105
    let sb = Sandbox::new("item-only");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"widget\"\n",
            "path = \"skills/widget\"\n",
            "install = \"echo widget-installed\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: widget\n---\n# widget\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "widget", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    // "item-only#widget" is not the registered source's identity (that is
    // just "item-only"), so it can only resolve as an item ref.
    let r = sb.mind(&["hooks", "list", "item-only#widget"]);
    assert!(
        r.success,
        "must resolve as an item ref: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("item:"),
        "must be read as an item target: {}",
        r.stdout
    );
}

/// The kind-qualified escape (`<source>#<kind>:<name>`) disambiguates toward
/// the item even when the plain form of the same target is ambiguous: it
/// never equals a registered source's exact identity (identities carry no
/// `kind:` segment), so it always falls through to ordinary item-ref parsing.
#[test]
fn hooks_target_kind_qualified_escape_forces_item_resolution() {
    // spec: HOOK-105
    let sb = Sandbox::new("amb-kind-esc");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"dev\"\n",
            "path = \"dev\"\n",
            "install = \"echo dev-installed\"\n",
        ),
    );
    sb.write_and_commit("dev/SKILL.md", "---\ndescription: dev skill\n---\n# dev\n");

    let r = sb.mind(&["meld", &sb.link("tree/main/dev"), "--register-only"]);
    assert!(r.success, "link meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    // Qualify by source: the link instance also offers a catalog item named
    // `dev` (LNK-7), so a bare `learn dev` would itself be ambiguous.
    let r = sb.mind(&[
        "learn",
        "amb-kind-esc#dev",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "learn: {}", r.stderr);

    // The plain identity string is ambiguous (as pinned above); the
    // kind-qualified form is not, since it never matches a registered source.
    let source_part = format!(
        "local/{}/{}",
        sb.base.file_name().unwrap().to_string_lossy(),
        sb.source.file_name().unwrap().to_string_lossy(),
    );
    let escaped = format!("{source_part}#skill:dev");

    let r = sb.mind(&["hooks", "list", &escaped]);
    assert!(
        r.success,
        "the kind-qualified escape must resolve unambiguously: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("item:"),
        "must be read as an item target: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("echo dev-installed") || r.stdout.contains("dev-installed"),
        "must list the installed item's own hook: {}",
        r.stdout
    );
}

/// The `source:` escape disambiguates toward the source even when the plain
/// form of the same target is ambiguous: it always forces the source reading,
/// bypassing both the exact-match check and the ambiguity check.
#[test]
fn hooks_target_source_prefix_escape_forces_source_resolution() {
    // spec: HOOK-105
    let sb = Sandbox::new("amb-src-esc");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"dev\"\n",
            "path = \"dev\"\n",
        ),
    );
    sb.write_and_commit("dev/SKILL.md", "---\ndescription: dev skill\n---\n# dev\n");

    let r = sb.mind(&["meld", &sb.link("tree/main/dev"), "--register-only"]);
    assert!(r.success, "link meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    // Qualify by source: the link instance also offers a catalog item named
    // `dev` (LNK-7), so a bare `learn dev` would itself be ambiguous.
    let r = sb.mind(&[
        "learn",
        "amb-src-esc#dev",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "learn: {}", r.stderr);

    let identity = sb.link_identity("dev");
    let escaped = format!("source:{identity}");

    let r = sb.mind(&["hooks", "list", &escaped]);
    assert!(
        r.success,
        "the source: escape must resolve unambiguously: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("source:"),
        "must be read as a source target: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-107 x CLI-181/CLI-182: the structured error envelope for the
// "ran nothing because consent was unavailable" case. HOOK-107 explicitly
// claims this gives `hooks run --json` non-empty output on the no-op path, but
// no test drove `--json` on that path.
// ---------------------------------------------------------------------------

/// `mind --json hooks run` on the HOOK-107 path emits the CLI-181 envelope on
/// stdout with the stable `hooks-not-run` kind slug (CLI-182), and exits with
/// code 1 exactly (CLI-175), not merely "non-zero".
#[test]
fn hooks_run_hooks_not_run_emits_json_error_envelope() {
    // spec: HOOK-107 CLI-181 CLI-182
    let sb = Sandbox::new("json-not-run");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "install",
        "json-not-run",
    ]);
    assert!(
        !r.success,
        "the HOOK-107 path must fail under --json too: {}\n{}",
        r.stdout, r.stderr
    );
    assert_eq!(
        r.code,
        Some(1),
        "a MindError must exit 1 (CLI-175), not another non-zero code: {}\n{}",
        r.stdout,
        r.stderr
    );

    let envelope = json_envelope(&r.stdout);
    assert_eq!(
        envelope["schema"], 1,
        "envelope must carry schema 1: {envelope}"
    );
    assert_eq!(
        envelope["error"]["kind"], "hooks-not-run",
        "the stable per-variant kind slug must be emitted: {envelope}"
    );
    let message = envelope["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("message must be a string: {envelope}"));
    assert!(
        message.contains("skipped for want of consent"),
        "the envelope message must be the full Display text: {message}"
    );
    assert!(
        message.contains("--dangerously-skip-install-hook-check"),
        "the envelope message must carry the remedy: {message}"
    );
    assert!(
        message.contains("json-not-run"),
        "the envelope message must name the target: {message}"
    );
}

/// CLI-181 says nothing is written to stderr *by the main error handler* on
/// the JSON error path (main.rs's own `error: ...` / `caused by: ...` prose
/// is suppressed there, matching `json_error_emits_envelope_on_stdout_not_stderr`
/// in `tests/cli.rs`). It does not mean stderr is silent overall: CLI-217
/// routes an advisory note fired earlier in the same run to stderr rather
/// than dropping it, and the HOOK-106 skip note is exactly such a note. So
/// this run's stderr carries the routed note, but never the main handler's
/// own error text -- the two spec entries are complementary, not in tension.
#[test]
fn hooks_run_hooks_not_run_json_writes_only_the_routed_note_to_stderr() {
    // spec: HOOK-107 CLI-181 CLI-217
    let sb = Sandbox::new("json-stderr");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "install",
        "json-stderr",
    ]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("error:"),
        "CLI-181: the main error handler must not write its own error text to \
         stderr under --json: {:?}",
        r.stderr
    );
    assert!(
        r.stderr.contains("note: skipped install hook 'setup'"),
        "CLI-217: the HOOK-106 note is routed to stderr, not dropped, under \
         --json: {:?}",
        r.stderr
    );
}

/// Under `--json`, the HOOK-106 skip note now routes through `render::note`
/// (CLI-217), so it lands on stderr instead of stdout; stdout carries only
/// the CLI-181 envelope, and `mind --json hooks run x | jq .error.kind`
/// parses cleanly.
#[test]
fn hooks_run_json_stdout_is_only_the_envelope() {
    // spec: HOOK-107 CLI-181 CLI-217
    let sb = Sandbox::new("json-pure");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "hooks", "run", "--event", "install", "json-pure"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
}

// ---------------------------------------------------------------------------
// HOOK-107 boundaries: the exact predicate is `existed > 0 && ran == 0 &&
// skipped_for_consent > 0`, accumulated across every matched source. Each
// factor gets a test that fails if that factor is dropped.
// ---------------------------------------------------------------------------

/// `--force` turns a hook that had "nothing to do" into work: an already-ran
/// install hook is reconsidered, skipped for want of a terminal, and the run
/// must now be a HOOK-107 error. This is the exact boundary between
/// `hooks_run_source_install_already_ran_non_tty_stays_exit_zero` (exit 0) and
/// an error, and it turns on `--force` alone.
#[test]
fn hooks_run_force_on_already_ran_hook_in_non_tty_is_hooks_not_run() {
    // spec: HOOK-107 HOOK-101 CLI-195
    let sb = Sandbox::new("force-not-run");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup-ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // Record it at the current commit via the bypass.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "force-not-run",
    ]);
    assert!(r.success, "bypassed run: {}\n{}", r.stdout, r.stderr);

    // Without --force this is a no-op that exits 0 (pinned elsewhere). With
    // --force the hook is considered again, so there IS work, and a non-TTY
    // run cannot consent to it.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--force",
        "force-not-run",
    ]);
    assert!(
        !r.success,
        "--force reconsiders an already-ran hook, so a non-TTY run has work it \
         cannot consent to and must be a HOOK-107 error: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("1 hook(s) had work to do"),
        "must report exactly the one reconsidered hook: {}",
        r.stderr
    );
}

/// The skipped count in the HOOK-107 error is a real count, not a constant:
/// two pending hooks in one source produce `2 hook(s)`, and each gets its own
/// HOOK-106 note.
#[test]
fn hooks_run_hooks_not_run_counts_every_skipped_hook() {
    // spec: HOOK-106 HOOK-107
    let sb = Sandbox::new("count-two");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"first\"\n",
            "run = \"echo one\"\n",
            "event = \"install\"\n",
            "\n",
            "[[hooks]]\n",
            "name = \"second\"\n",
            "run = \"echo two\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "count-two"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("2 hook(s) had work to do"),
        "the skipped count must be the real number of skipped hooks: {}",
        r.stderr
    );
    // HOOK-106: every skipped hook gets its own cause-and-remedy note.
    let notes = r
        .stdout
        .lines()
        .filter(|l| l.contains("not a terminal"))
        .count();
    assert_eq!(
        notes, 2,
        "each skipped hook must get its own HOOK-106 note: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("'first'") && r.stdout.contains("'second'"),
        "each note must name its own hook label: {}",
        r.stdout
    );
}

/// A source that declares only an UNINSTALL hook has nothing to do for
/// `--event install`: the event filter empties the hook list before anything is
/// counted, so this stays exit 0. If `existed` were incremented from the
/// unfiltered hook list, this would wrongly become a HOOK-107 error.
#[test]
fn hooks_run_install_event_ignores_an_uninstall_only_source_and_exits_zero() {
    // spec: HOOK-107 CLI-195
    let sb = Sandbox::new("uninstall-only");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"teardown\"\n",
            "run = \"echo teardown\"\n",
            "event = \"uninstall\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "uninstall-only"]);
    assert!(
        r.success,
        "an uninstall-only source has no install work, so --event install stays \
         exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no install hooks declared"),
        "should note the absence for the selected event: {}",
        r.stdout
    );
}

/// The HOOK-107 counters accumulate across a fan-out selector. With one source
/// whose hook already ran (nothing to do) and one with a pending hook, the run
/// errors and reports exactly ONE skipped hook -- the no-op source must
/// contribute nothing to either counter.
#[test]
fn hooks_run_glob_counts_only_the_source_with_pending_work() {
    // spec: HOOK-107 CLI-194
    let sb = Sandbox::new("glob-mixed-a");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"a-setup\"\n",
            "run = \"echo a\"\n",
            "event = \"install\"\n",
        ),
    );
    let src_b = sb.base.join("glob-mixed-b");
    init_source_repo(
        &src_b,
        concat!(
            "[[hooks]]\n",
            "name = \"b-setup\"\n",
            "run = \"echo b\"\n",
            "event = \"install\"\n",
        ),
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld a: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["meld", src_b.to_string_lossy().as_ref()]);
    assert!(r.success, "meld b: {}\n{}", r.stdout, r.stderr);

    // Retire source A's hook only: it now has nothing to do.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "glob-mixed-a",
    ]);
    assert!(r.success, "retire a: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "*"]);
    assert!(
        !r.success,
        "a fan-out with one pending source must be a HOOK-107 error: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("1 hook(s) had work to do"),
        "only source b's hook had work; source a must not be counted: {}",
        r.stderr
    );
    // Only source b's hook is noted as skipped.
    assert!(
        r.stdout.contains("'b-setup'"),
        "source b's pending hook must be noted: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("'a-setup'"),
        "source a's retired hook must never be offered, so it cannot be noted \
         as skipped: {}",
        r.stdout
    );
}

/// A fan-out where EVERY matched source has nothing to do stays exit 0, even
/// though the selector matched several sources: `existed` stays 0 across the
/// whole walk.
#[test]
fn hooks_run_glob_with_nothing_to_do_anywhere_stays_exit_zero() {
    // spec: HOOK-107 CLI-194
    let sb = Sandbox::new("glob-quiet-a");
    // Source a declares no hooks at all.
    let src_b = sb.base.join("glob-quiet-b");
    init_source_repo(
        &src_b,
        concat!(
            "[[hooks]]\n",
            "name = \"b-setup\"\n",
            "run = \"echo b\"\n",
            "event = \"install\"\n",
        ),
    );

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld a: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["meld", src_b.to_string_lossy().as_ref()]);
    assert!(r.success, "meld b: {}\n{}", r.stdout, r.stderr);

    // Retire source b's only hook; source a never had one.
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "glob-quiet-b",
    ]);
    assert!(r.success, "retire b: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "*"]);
    assert!(
        r.success,
        "no source had work, so the fan-out must stay exit 0: {}\n{}",
        r.stdout, r.stderr
    );
}

/// A run that DID execute its hooks is never a HOOK-107 error even though
/// hooks existed: `ran > 0` clears the predicate. Pins that the bypass path
/// cannot regress into reporting a failure.
#[test]
fn hooks_run_with_bypass_never_reports_hooks_not_run() {
    // spec: HOOK-107
    let sb = Sandbox::new("bypass-clean");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "bypass-clean",
    ]);
    assert!(r.success, "bypassed run: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("not a terminal") && !r.stderr.contains("want of consent"),
        "a bypassed run consents to every hook, so neither the HOOK-106 note \
         nor the HOOK-107 error may appear: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// HOOK-108: an ITEM target is accounted for exactly as a source target is. An
/// item with an install hook, skipped for want of a terminal, is a non-zero
/// exit naming the target and the remedy, not the silent exit 0 it used to be
/// (this test previously pinned that silence, when HOOK-107 was source-scoped).
/// The hook still must not run.
#[test]
fn hooks_run_item_target_pending_install_hook_is_hooks_not_run_in_non_tty() {
    // spec: HOOK-108 HOOK-102
    let sb = Sandbox::new("item-silent");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"scanner\"\n",
            "path = \"skills/scanner\"\n",
            "install = \"touch item-hook-ran.sentinel\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    let sentinel = sb
        .mind_home
        .join("store/skill/scanner/item-hook-ran.sentinel");
    let _ = std::fs::remove_file(&sentinel);

    let r = sb.mind(&["hooks", "run", "--event", "install", "item-silent#scanner"]);
    assert!(
        !r.success,
        "an item target with a hook it could not consent to must be a non-zero \
         exit: {}\n{}",
        r.stdout, r.stderr
    );
    assert_eq!(
        r.code,
        Some(1),
        "a MindError must exit 1 (CLI-175): {}\n{}",
        r.stdout,
        r.stderr
    );
    assert!(
        r.stderr.contains("1 hook(s) had work to do"),
        "the error must count the item's skipped hook: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains(
            "mind hooks run item-silent#scanner --event install \
             --dangerously-skip-install-hook-check"
        ),
        "the error must carry the copy-pasteable remedy naming the target as \
         typed: {}",
        r.stderr
    );
    assert!(
        !sentinel.exists(),
        "the item hook must not have run in a non-TTY run: {}",
        sentinel.display()
    );
}

// ---------------------------------------------------------------------------
// HOOK-106: the remedy string itself.
// ---------------------------------------------------------------------------

/// The HOOK-106 note names the RESOLVED source identity, not the abbreviated
/// selector the user typed, so the printed command is unambiguous even when
/// the typed selector was a substring that happened to match.
#[test]
fn hooks_run_skip_note_remedy_names_the_resolved_source_identity() {
    // spec: HOOK-106
    let sb = Sandbox::new("remedy-identity");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let identity = format!(
        "local/{}/remedy-identity",
        sb.base.file_name().unwrap().to_string_lossy()
    );

    // The selector below is a bare substring of the identity, and a `*` glob
    // would be a second abbreviated form; both must resolve into a note
    // naming the full identity.
    let r = sb.mind(&["hooks", "run", "--event", "install", "*"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains(&format!(
            "mind hooks run {identity} --event install --dangerously-skip-install-hook-check"
        )),
        "the note's remedy must name the resolved identity, not the '*' \
         selector: {}",
        r.stdout
    );
}

/// For `--event uninstall`, the HOOK-106 note and the HOOK-107 error both
/// re-select the event in the printed remedy. Without it the suggested command
/// runs the source's INSTALL hooks instead (`--event` defaults to `install`),
/// which is different code than the one that was skipped.
#[test]
fn hooks_run_uninstall_skip_remedy_carries_the_event() {
    // spec: HOOK-106
    let sb = Sandbox::new("remedy-event");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"teardown\"\n",
            "run = \"echo teardown\"\n",
            "event = \"uninstall\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "uninstall", "remedy-event"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("--event uninstall"),
        "the remedy for an uninstall skip must re-select the uninstall event, \
         otherwise it silently runs the install hooks instead: {combined}"
    );
}

/// The HOOK-107 failure still persists the skip state it recorded before
/// erroring: the hook stays pending, and `hooks list` says so. A `return Err`
/// placed before `registry.save` would lose that.
#[test]
fn hooks_not_run_error_still_persists_the_recorded_skip() {
    // spec: HOOK-107 HOOK-101
    let sb = Sandbox::new("persist-skip");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "persist-skip"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);

    // The skip was recorded with ran_at = None (HOOK-101) BEFORE the error was
    // returned, so the hook is still pending and the registry file survived.
    let r = sb.mind(&["hooks", "list", "persist-skip"]);
    assert!(r.success, "hooks list after the error: {}", r.stderr);
    assert!(
        r.stdout.contains("pending"),
        "the skipped hook must remain pending after the HOOK-107 error: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-108: the HOOK-107 accounting on item targets.
// ---------------------------------------------------------------------------

/// The skipped count for an item target is a real count across every item the
/// ref matched: two items with one install hook each report `2 hook(s)`.
#[test]
fn hooks_run_item_glob_counts_every_skipped_item_hook() {
    // spec: HOOK-108 CLI-194
    let sb = Sandbox::new("item-count");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"alpha\"\n",
            "path = \"skills/alpha\"\n",
            "install = \"touch alpha-ran.sentinel\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"beta\"\n",
            "path = \"skills/beta\"\n",
            "install = \"touch beta-ran.sentinel\"\n",
        ),
    );
    sb.write_and_commit("skills/alpha/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/beta/SKILL.md", "---\ndescription: b\n---\n# b\n");

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "alpha", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn alpha: {}", r.stderr);
    let r = sb.mind(&["learn", "beta", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn beta: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "item-count#*"]);
    assert!(
        !r.success,
        "both items had a hook and neither could be consented to: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("2 hook(s) had work to do"),
        "the count must accumulate across every matched item: {}",
        r.stderr
    );
}

/// An item target whose item declares no hook for the selected event has
/// nothing to do, so it stays exit 0: `existed` never leaves 0.
#[test]
fn hooks_run_item_with_no_hooks_for_the_event_stays_exit_zero() {
    // spec: HOOK-108 HOOK-102
    let sb = Sandbox::new("item-nowork");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"plain\"\n",
            "path = \"skills/plain\"\n",
            "install = \"echo installed\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/plain/SKILL.md",
        "---\ndescription: plain\n---\n# plain\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "plain", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    // The item declares an install hook but no uninstall hook.
    let r = sb.mind(&["hooks", "run", "--event", "uninstall", "item-nowork#plain"]);
    assert!(
        r.success,
        "no hook for the selected event is nothing to do, so exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no uninstall hooks declared"),
        "should note the absence for the selected event: {}",
        r.stdout
    );
}

/// `--event build` is outside the HOOK-108 accounting: it re-installs through
/// the transactional path, where a non-TTY build-hook skip is a step of the
/// install and leaves the command at exit 0.
#[test]
fn hooks_run_item_build_skipped_in_non_tty_stays_exit_zero() {
    // spec: HOOK-108 HOOK-103
    let sb = Sandbox::new("item-build-nowork");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"builder\"\n",
            "path = \"tools/builder\"\n",
            "build = \"touch built.sentinel\"\n",
        ),
    );
    sb.write_and_commit("tools/builder/run.sh", "#!/bin/sh\necho hi\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "builder"]);
    assert!(r.success, "learn: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "build",
        "item-build-nowork#builder",
    ]);
    assert!(
        r.success,
        "a build-event run is not part of the HOOK-108 accounting and stays \
         exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind_home
            .join("store/tool/builder/built.sentinel")
            .exists(),
        "the build hook itself must still be skipped in a non-TTY run: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-109: the MIND_TTY seam, and the interactive branches it makes
// reachable -- notably HOOK-106's distinction between a non-TTY skip (cause +
// remedy, an error) and an interactive decline (plain note, exit 0).
// ---------------------------------------------------------------------------

/// With `MIND_TTY` forcing the interactive branch, a declined source hook is an
/// interactive decline, not a consent failure: the plain HOOK-106 note with no
/// cause and no remedy, and exit 0 (the hook existed but nothing was skipped
/// *for want of consent*, so HOOK-107's predicate does not fire).
#[test]
fn hooks_run_interactive_decline_prints_the_plain_note_and_exits_zero() {
    // spec: HOOK-109 HOOK-106 HOOK-107
    let sb = Sandbox::new("tty-decline");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"touch declined.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind_env_stdin(
        &["hooks", "run", "--event", "install", "tty-decline"],
        &[("MIND_TTY", "1")],
        "n\n",
    );
    assert!(
        r.stdout.contains("====== hook: setup ======"),
        "MIND_TTY must take the prompting branch, which discloses the hook \
         first: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skipped install hook 'setup'"),
        "a declined hook is still noted as skipped: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("not a terminal"),
        "an interactive decline has no non-TTY cause to name: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("--dangerously-skip-install-hook-check"),
        "an interactive decline is an active choice, so no unattended remedy \
         is suggested: {}",
        r.stdout
    );
    assert!(
        r.success,
        "an interactive decline is not a consent failure, so exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.source.join("declined.sentinel").exists(),
        "the declined hook must not have run"
    );
}

/// The other arm of the same seam: an accepted prompt runs the hook, with no
/// `--dangerously-skip-install-hook-check` anywhere in the invocation.
#[test]
fn hooks_run_interactive_accept_runs_the_hook() {
    // spec: HOOK-109 HOOK-100
    let sb = Sandbox::new("tty-accept");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"touch accepted.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind_env_stdin(
        &["hooks", "run", "--event", "install", "tty-accept"],
        &[("MIND_TTY", "1")],
        "y\n",
    );
    assert!(r.success, "accepted run: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("running install hook 'setup'"),
        "the accepted hook must be reported as running: {}",
        r.stdout
    );
    assert!(
        sb.source.join("accepted.sentinel").exists(),
        "the accepted hook must have run: {}",
        r.stdout
    );
}

/// Priority-4 side effect of the CLI-217 fd redirect: under `--json`, fd 1 is
/// pointed at stderr for the WHOLE run (main.rs's `json_stdout`), including
/// while `hook::decide` is soliciting an interactive answer
/// (`--dangerously-skip-...` is not passed here, and `MIND_TTY=1` forces the
/// prompting branch). The disclosure and the prompt text are `print!`/
/// `println!` in src/hook.rs, exactly as exposed to the redirect as anything
/// else on this path -- this confirms they still reach the user (on stderr)
/// and that reading the answer from stdin still works with fd 1 no longer
/// pointing at the real stdout.
#[test]
fn hooks_run_json_interactive_accept_prompts_on_stderr_and_runs_the_hook() {
    // spec: CLI-217 HOOK-109 HOOK-100
    let sb = Sandbox::new("tty-accept-json");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"touch accepted.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind_env_stdin(
        &[
            "--json",
            "hooks",
            "run",
            "--event",
            "install",
            "tty-accept-json",
        ],
        &[("MIND_TTY", "1")],
        "y\n",
    );
    assert!(r.success, "accepted run --json: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.source.join("accepted.sentinel").exists(),
        "the accepted hook must have run even though fd 1 is redirected under \
         --json: {} {}",
        r.stdout,
        r.stderr
    );
    // `hooks run --json` answers a SUCCESSFUL run with nothing on stdout (only
    // the HOOK-107 no-op path gets a CLI-181 envelope; see HOOK-107 in
    // spec/install-hooks.md), so the interesting assertion here is where the
    // prompt went, not whether stdout parses as JSON.
    assert!(
        r.stdout.is_empty(),
        "hooks run --json answers a successful run with nothing on stdout \
         (HOOK-107): {:?}",
        r.stdout
    );
    assert!(
        r.stderr.contains("====== hook: setup ======") && r.stderr.contains("Run this hook?"),
        "the interactive disclosure and prompt must still reach the user, on \
         stderr, with fd 1 redirected: {:?}",
        r.stderr
    );
    assert!(
        r.stderr.contains("running install hook 'setup'"),
        "the progress line must also be visible on stderr: {:?}",
        r.stderr
    );
}

/// A falsey `MIND_TTY` value keeps the non-TTY behavior even with a real reply
/// waiting on stdin: the override decides, and `0` means "not a terminal".
#[test]
fn hooks_run_falsey_mind_tty_keeps_the_non_tty_skip() {
    // spec: HOOK-109 HOOK-106
    let sb = Sandbox::new("tty-off");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"touch never.sentinel\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind_env_stdin(
        &["hooks", "run", "--event", "install", "tty-off"],
        &[("MIND_TTY", "0")],
        "y\n",
    );
    assert!(
        r.stdout.contains("not a terminal"),
        "MIND_TTY=0 must keep the non-TTY skip and its cause: {}",
        r.stdout
    );
    assert!(
        !r.success,
        "the non-TTY skip is still a HOOK-107 error: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.source.join("never.sentinel").exists(),
        "a hook must never run on a reply the non-TTY branch did not read"
    );
}

/// The item-target arm of the same distinction: an interactively declined item
/// hook is not a HOOK-108 error (exit 0), and the hook does not run.
#[test]
fn hooks_run_item_interactive_decline_exits_zero() {
    // spec: HOOK-108 HOOK-109
    let sb = Sandbox::new("item-decline");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"scanner\"\n",
            "path = \"skills/scanner\"\n",
            "install = \"touch item-declined.sentinel\"\n",
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);

    // `learn` with the bypass already ran the item's install hook; clear its
    // sentinel so the assertion below observes only this run.
    let sentinel = sb
        .mind_home
        .join("store/skill/scanner/item-declined.sentinel");
    std::fs::remove_file(&sentinel).expect("learn should have created the sentinel");

    let r = sb.mind_env_stdin(
        &["hooks", "run", "--event", "install", "item-decline#scanner"],
        &[("MIND_TTY", "1")],
        "n\n",
    );
    assert!(
        r.success,
        "an interactively declined item hook is an active choice, not a \
         consent failure: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sentinel.exists(),
        "the declined item hook must not have run: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-106 as an executable contract, not a string.
//
// The point of the remedy is that it can be pasted and run with nothing to
// fill in. Asserting the text only proves the text; these tests take the
// command the run actually printed, split it into argv, and execute it, then
// assert the *skipped hook* ran and no other did.
// ---------------------------------------------------------------------------

/// Take the `mind ...` command a HOOK-106 note or a HOOK-107/HOOK-108 error
/// offers as its remedy and return it as argv with the leading `mind` dropped,
/// ready to hand back to [`Sandbox::mind`]. Panics when `text` carries no
/// remedy, so a round-trip test can never pass by quietly finding nothing to
/// run.
fn remedy_argv(text: &str) -> Vec<String> {
    const LEAD: &str = "re-run with '";
    let start = text
        .find(LEAD)
        .unwrap_or_else(|| panic!("no remedy in output: {text:?}"));
    let rest = &text[start + LEAD.len()..];
    let end = rest
        .find('\'')
        .unwrap_or_else(|| panic!("unterminated remedy quote: {text:?}"));
    let mut argv: Vec<String> = rest[..end].split_whitespace().map(str::to_string).collect();
    assert_eq!(
        argv.first().map(String::as_str),
        Some("mind"),
        "the remedy must be a `mind` invocation: {text:?}"
    );
    argv.remove(0);
    assert!(
        !argv.iter().any(|a| a.contains('<') || a.contains('>')),
        "the remedy must carry no placeholder to fill in: {argv:?}"
    );
    argv
}

fn as_args(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

/// The install-event round trip: the command the skip note printed, run
/// verbatim, runs the hook that was skipped.
#[test]
fn remedy_from_a_source_install_skip_runs_the_skipped_hook_verbatim() {
    // spec: HOOK-106 HOOK-107
    let sb = Sandbox::new("remedy-rt-install");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"setup\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("install-ran.sentinel")
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let skipped = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--force",
        "remedy-rt-install",
    ]);
    assert!(!skipped.success, "must fail: {}", skipped.stderr);
    assert!(
        !sb.sentinel("install-ran.sentinel").exists(),
        "the hook must not have run yet"
    );

    let argv = remedy_argv(&skipped.stderr);
    let redo = sb.mind(&as_args(&argv));
    assert!(
        redo.success,
        "the printed remedy must run clean: {argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
    assert!(
        sb.sentinel("install-ran.sentinel").exists(),
        "the remedy must actually run the skipped install hook: {argv:?}\n{}",
        redo.stdout
    );
}

/// The regression HOOK-106 exists for, proved by execution rather than by
/// string match: after an `--event uninstall` skip, the printed remedy must run
/// the UNINSTALL hook. Without the `--event` segment it would run the source's
/// install hook instead -- different code, silently substituted.
#[test]
fn remedy_from_an_uninstall_skip_runs_the_uninstall_hook_not_the_install_one() {
    // spec: HOOK-106
    let sb = Sandbox::new("remedy-rt-uninstall");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"setup\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
                "\n",
                "[[hooks]]\n",
                "name = \"cleanup\"\n",
                "run = \"{}\"\n",
                "event = \"uninstall\"\n",
            ),
            sb.touch("install-ran.sentinel"),
            sb.touch("uninstall-ran.sentinel"),
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    assert!(
        !sb.sentinel("install-ran.sentinel").exists(),
        "meld in a non-TTY must have skipped the install hook"
    );

    let skipped = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "uninstall",
        "remedy-rt-uninstall",
    ]);
    assert!(!skipped.success, "must fail: {}", skipped.stderr);

    // Both the note (stdout) and the error (stderr) advertise a remedy; run
    // each of them, since HOOK-106 promises the substitution in both places.
    for (channel, text) in [("note", &skipped.stdout), ("error", &skipped.stderr)] {
        // Execute first and judge by side effect, so a failure here reports
        // what the printed command DID, not what it said.
        let argv = remedy_argv(text);
        let redo = sb.mind(&as_args(&argv));
        assert!(
            redo.success,
            "the {channel} remedy must run clean: {argv:?}\n{}\n{}",
            redo.stdout, redo.stderr
        );
        assert!(
            !sb.sentinel("install-ran.sentinel").exists(),
            "the {channel} remedy ran the source's INSTALL hook instead of the \
             uninstall hook that was skipped -- the printed command silently \
             names different code: {argv:?}\n{}",
            redo.stdout
        );
        assert!(
            sb.sentinel("uninstall-ran.sentinel").exists(),
            "the {channel} remedy must run the hook that was skipped: {argv:?}\n{}",
            redo.stdout
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--event" && w[1] == "uninstall"),
            "the {channel} remedy must re-select the uninstall event: {argv:?}"
        );
        std::fs::remove_file(sb.sentinel("uninstall-ran.sentinel")).expect("clear sentinel");
    }
}

/// The item-target round trip in the `<source>#<item>` spelling: the HOOK-108
/// error's remedy, run verbatim, runs that item's hook.
#[test]
fn remedy_from_an_item_target_runs_the_items_hook_verbatim() {
    // spec: HOOK-106 HOOK-108
    let sb = Sandbox::new("remedy-rt-item");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"scanner\"\n",
                "path = \"skills/scanner\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("item-install-ran.sentinel")
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner"]);
    assert!(r.success, "learn: {}", r.stderr);
    assert!(
        !sb.sentinel("item-install-ran.sentinel").exists(),
        "a non-TTY learn must have skipped the item hook"
    );

    let skipped = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "remedy-rt-item#scanner",
    ]);
    assert!(!skipped.success, "must fail: {}", skipped.stderr);

    let argv = remedy_argv(&skipped.stderr);
    let redo = sb.mind(&as_args(&argv));
    assert!(
        redo.success,
        "the printed remedy must run clean: {argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
    assert!(
        sb.sentinel("item-install-ran.sentinel").exists(),
        "the remedy must actually run the skipped item hook: {argv:?}\n{}",
        redo.stdout
    );
}

/// The combination the remedy is most likely to get wrong: an ITEM target on
/// the UNINSTALL event. The error's remedy, run verbatim, must run the item's
/// uninstall hook and leave its install hook alone.
#[test]
fn remedy_from_an_item_uninstall_target_runs_the_uninstall_hook_only() {
    // spec: HOOK-106 HOOK-108
    let sb = Sandbox::new("remedy-rt-item-un");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"scanner\"\n",
                "path = \"skills/scanner\"\n",
                "install = \"{}\"\n",
                "uninstall = \"{}\"\n",
            ),
            sb.touch("item-inst.sentinel"),
            sb.touch("item-uninst.sentinel"),
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner"]);
    assert!(r.success, "learn: {}", r.stderr);

    let skipped = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "uninstall",
        "remedy-rt-item-un#scanner",
    ]);
    assert!(!skipped.success, "must fail: {}", skipped.stderr);

    let argv = remedy_argv(&skipped.stderr);
    let redo = sb.mind(&as_args(&argv));
    assert!(
        redo.success,
        "the printed remedy must run clean: {argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
    assert!(
        sb.sentinel("item-uninst.sentinel").exists(),
        "the remedy must run the item's UNINSTALL hook: {argv:?}\n{}",
        redo.stdout
    );
    assert!(
        !sb.sentinel("item-inst.sentinel").exists(),
        "the remedy must not fall back to the item's install hook: \
         {argv:?}\n{}",
        redo.stdout
    );
}

/// The same round trip for the kind-qualified spelling (`<source>#<kind>:<name>`,
/// HOOK-105's explicit item escape): the error must echo the target as typed, so
/// the remedy resolves back to the same item instead of re-entering the
/// source/item ambiguity.
#[test]
fn remedy_from_a_kind_qualified_item_target_round_trips() {
    // spec: HOOK-106 HOOK-108 HOOK-105
    let sb = Sandbox::new("remedy-rt-kind");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"scanner\"\n",
                "path = \"skills/scanner\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("kind-item-ran.sentinel")
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner"]);
    assert!(r.success, "learn: {}", r.stderr);

    let target = "remedy-rt-kind#skill:scanner";
    let skipped = sb.mind(&["hooks", "run", "--event", "install", target]);
    assert!(!skipped.success, "must fail: {}", skipped.stderr);

    let argv = remedy_argv(&skipped.stderr);
    assert!(
        argv.iter().any(|a| a == target),
        "the remedy must name the target exactly as typed: {argv:?}"
    );
    let redo = sb.mind(&as_args(&argv));
    assert!(
        redo.success,
        "the printed remedy must run clean: {argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
    assert!(
        sb.sentinel("kind-item-ran.sentinel").exists(),
        "the remedy must run the skipped item hook: {argv:?}\n{}",
        redo.stdout
    );
}

/// An item-link instance's identity carries its own `#<path>` (LNK-4), so the
/// remedy for a skip on that target embeds a `#` that must survive the round
/// trip and still resolve as a SOURCE (HOOK-105), not as an item ref.
#[test]
fn remedy_from_a_link_instance_identity_round_trips_as_a_source_target() {
    // spec: HOOK-106 HOOK-105 HOOK-107
    let sb = Sandbox::new("remedy-rt-link");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"setup\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("link-hook-ran.sentinel")
        ),
    );
    sb.write_and_commit(
        "skills/deep/SKILL.md",
        "---\ndescription: deep\n---\n# deep\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/deep")]);
    assert!(r.success, "learn link: {}\n{}", r.stdout, r.stderr);

    let identity = sb.link_identity("skills/deep");
    let skipped = sb.mind(&["hooks", "run", "--event", "install", "--force", &identity]);
    assert!(
        !skipped.success,
        "must fail: {}\n{}",
        skipped.stdout, skipped.stderr
    );

    let argv = remedy_argv(&skipped.stderr);
    assert!(
        argv.contains(&identity),
        "the remedy must carry the link instance's full identity: {argv:?}"
    );
    let redo = sb.mind(&as_args(&argv));
    assert!(
        redo.success,
        "the printed remedy must run clean: {argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
    assert!(
        sb.sentinel("link-hook-ran.sentinel").exists(),
        "the remedy must run the link instance's skipped source hook: \
         {argv:?}\n{}",
        redo.stdout
    );
}

/// The one place the remedy is NOT a literal command: a glob selector. The
/// HOOK-106 note substitutes the resolved source identity, but the HOOK-107
/// error echoes the selector as typed, so the error's remedy for `'*'` is
/// `mind hooks run * ...` -- an unquoted shell glob, not something to paste.
/// Pinned so the divergence between the two remedies is a decision and not a
/// surprise.
#[test]
fn glob_selector_remedy_differs_between_the_note_and_the_error() {
    // spec: HOOK-106 HOOK-107
    let sb = Sandbox::new("remedy-glob");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo setup\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let identity = format!(
        "local/{}/remedy-glob",
        sb.base.file_name().unwrap().to_string_lossy()
    );
    let r = sb.mind(&["hooks", "run", "--event", "install", "*"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);

    let note_argv = remedy_argv(&r.stdout);
    assert!(
        note_argv.contains(&identity),
        "the note's remedy resolves the glob to the concrete identity: \
         {note_argv:?}"
    );
    let err_argv = remedy_argv(&r.stderr);
    assert_eq!(
        err_argv.iter().find(|a| a.as_str() == "*"),
        Some(&"*".to_string()),
        "the error's remedy still carries the selector as typed, so a glob \
         target's error remedy is not literally paste-able: {err_argv:?}"
    );
    // The note's remedy, unlike the error's, is executable as printed.
    let redo = sb.mind(&as_args(&note_argv));
    assert!(
        redo.success,
        "the note's remedy must run clean: {note_argv:?}\n{}\n{}",
        redo.stdout, redo.stderr
    );
}

// ---------------------------------------------------------------------------
// HOOK-108: does the tally's PREDICTION match what actually happened?
//
// `HookTally::offered` mirrors install.rs's decision ladder rather than
// observing it, so these tests pin the cases where a mirror could drift: the
// count is of hooks (not items), a hard failure outranks the tally, and a
// bypassed run really did run.
// ---------------------------------------------------------------------------

/// An item target under the bypass runs its hook and never reports
/// `HooksNotRun`: the tally's `ran += count` prediction must match reality.
/// (The pre-existing bypass test uses a SOURCE target, so this arm was
/// unexercised.)
#[test]
fn hooks_run_item_target_under_bypass_runs_and_reports_nothing() {
    // spec: HOOK-108 HOOK-102
    let sb = Sandbox::new("item-bypass");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"scanner\"\n",
                "path = \"skills/scanner\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("bypass-item-ran.sentinel")
        ),
    );
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner"]);
    assert!(r.success, "learn: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "item-bypass#scanner",
    ]);
    assert!(r.success, "bypassed item run: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("want of consent"),
        "a bypassed run consents to every hook: {}",
        r.stderr
    );
    assert!(
        sb.sentinel("bypass-item-ran.sentinel").exists(),
        "the tally counted the hook as RAN, so it had better have run: {}",
        r.stdout
    );
}

/// The tally counts HOOKS, not items: one item declaring two install hooks
/// (the `install` scalar plus an `[[items.hooks]]` entry, HOOK-86) reports two.
/// A tally that incremented per item would say `1`.
#[test]
fn hooks_run_item_with_two_install_hooks_counts_hooks_not_items() {
    // spec: HOOK-108 HOOK-102
    let sb = Sandbox::new("item-two-hooks");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"dual\"\n",
                "path = \"skills/dual\"\n",
                "install = \"{}\"\n",
                "\n",
                "[[items.hooks]]\n",
                "name = \"second\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("dual-one.sentinel"),
            sb.touch("dual-two.sentinel"),
        ),
    );
    sb.write_and_commit("skills/dual/SKILL.md", "---\ndescription: d\n---\n# d\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "dual"]);
    assert!(r.success, "learn: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "item-two-hooks#dual"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("2 hook(s) had work to do"),
        "one item with two install hooks must be counted as two: {}",
        r.stderr
    );

    // And the tally's own prediction holds: the bypassed re-run runs both.
    let argv = remedy_argv(&r.stderr);
    let redo = sb.mind(&as_args(&argv));
    assert!(redo.success, "remedy: {}\n{}", redo.stdout, redo.stderr);
    assert!(
        sb.sentinel("dual-one.sentinel").exists() && sb.sentinel("dual-two.sentinel").exists(),
        "both counted hooks must actually run: {}",
        redo.stdout
    );
}

/// A glob matching a mix of items with and without a hook for the event counts
/// only the ones that had work: `existed` must not be inflated by the hookless
/// item, whose branch returns before `offered` is reached.
#[test]
fn hooks_run_item_glob_over_mixed_items_counts_only_the_hooked_one() {
    // spec: HOOK-108 CLI-194
    let sb = Sandbox::new("item-mixed");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"alpha\"\n",
                "path = \"skills/alpha\"\n",
                "install = \"{}\"\n",
                "\n",
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"beta\"\n",
                "path = \"skills/beta\"\n",
            ),
            sb.touch("mixed-alpha.sentinel")
        ),
    );
    sb.write_and_commit("skills/alpha/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/beta/SKILL.md", "---\ndescription: b\n---\n# b\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "alpha"]);
    assert!(r.success, "learn alpha: {}", r.stderr);
    let r = sb.mind(&["learn", "beta"]);
    assert!(r.success, "learn beta: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "install", "item-mixed#*"]);
    assert!(!r.success, "must fail: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("1 hook(s) had work to do"),
        "the hookless item must contribute nothing to the count: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no install hooks declared"),
        "the hookless item is still reported as having nothing to do: {}",
        r.stdout
    );
}

/// A hook that fails outranks the tally: `HookFailed` propagates out of the
/// per-item loop before `nothing_could_consent` is consulted, so the reported
/// error is the failure, not "could not get consent". The tally had already
/// optimistically counted the hook as RAN, which is exactly the prediction that
/// would be wrong if the failure were swallowed.
#[test]
fn hooks_run_item_hook_failure_outranks_the_hooks_not_run_report() {
    // spec: HOOK-108 HOOK-102
    let sb = Sandbox::new("item-fails");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"boom\"\n",
            "path = \"skills/boom\"\n",
            "install = \"exit 3\"\n",
        ),
    );
    sb.write_and_commit("skills/boom/SKILL.md", "---\ndescription: b\n---\n# b\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "boom"]);
    assert!(r.success, "learn: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "item-fails#boom",
    ]);
    assert!(!r.success, "a failing hook must fail the run: {}", r.stdout);
    assert!(
        !r.stderr.contains("want of consent"),
        "the reported error must be the failure, not a consent report: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("hook") && r.stderr.contains("failed"),
        "the failure must be reported as a hook failure: {}",
        r.stderr
    );
}

/// Under a glob, the first failing item's hook stops the batch: the second
/// item's hook never runs. This pins that the `?` in the per-item loop is not
/// deferred to the end-of-run tally check.
#[test]
fn hooks_run_item_glob_stops_at_the_first_failing_hook() {
    // spec: HOOK-108 CLI-194
    let sb = Sandbox::new("item-halt");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                // The manifest is a BTreeMap keyed `kind:name`, so `alpha`
                // is visited before `zeta`.
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"alpha\"\n",
                "path = \"skills/alpha\"\n",
                "install = \"{} && exit 3\"\n",
                "\n",
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"zeta\"\n",
                "path = \"skills/zeta\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("halt-alpha.sentinel"),
            sb.touch("halt-zeta.sentinel"),
        ),
    );
    sb.write_and_commit("skills/alpha/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/zeta/SKILL.md", "---\ndescription: z\n---\n# z\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "alpha"]);
    assert!(r.success, "learn alpha: {}", r.stderr);
    let r = sb.mind(&["learn", "zeta"]);
    assert!(r.success, "learn zeta: {}", r.stderr);

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "item-halt#*",
    ]);
    assert!(
        !r.success,
        "the batch must fail: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        sb.sentinel("halt-alpha.sentinel").exists(),
        "the first item's hook must have started: {}",
        r.stdout
    );
    assert!(
        !sb.sentinel("halt-zeta.sentinel").exists(),
        "a failure must stop the batch before the next item: {}",
        r.stdout
    );
}

/// An item whose install hook has already been run by `learn` is offered again
/// on every `hooks run --event install`: item hooks carry no run-commit record,
/// so unlike a source install hook (which becomes "nothing to do") this stays a
/// HOOK-108 error forever in a non-TTY. Pinned because it is the one asymmetry
/// between the source and item accounting.
#[test]
fn item_install_hooks_have_no_already_ran_record_so_they_re_report_forever() {
    // spec: HOOK-108 HOOK-107
    let sb = Sandbox::new("item-noreco");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"skill\"\n",
                "name = \"scanner\"\n",
                "path = \"skills/scanner\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("noreco.sentinel")
        ),
    );
    sb.write_and_commit("skills/scanner/SKILL.md", "---\ndescription: s\n---\n# s\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}", r.stderr);
    assert!(
        sb.sentinel("noreco.sentinel").exists(),
        "learn with the bypass must have run the item hook"
    );

    // Twice, to show the report does not settle after the first time.
    for attempt in 1..=2 {
        let r = sb.mind(&["hooks", "run", "--event", "install", "item-noreco#scanner"]);
        assert!(
            !r.success,
            "attempt {attempt}: an item hook already run by learn is still \
             offered and still unconsentable: {}\n{}",
            r.stdout, r.stderr
        );
        assert!(
            r.stderr.contains("1 hook(s) had work to do"),
            "attempt {attempt}: {}",
            r.stderr
        );
    }
}

// ---------------------------------------------------------------------------
// HOOK-108 boundary: --event build stays outside the accounting.
// ---------------------------------------------------------------------------

/// `--event build` re-installs the item transactionally, which runs the BUILD
/// hook only. An item that declares both a build and an install hook has its
/// install hook left alone, so a "rebuild" does not re-apply the install side
/// effect. Pinned because the two hooks read as a pair in `mind.toml` and the
/// asymmetry is invisible from the CLI.
#[test]
fn hooks_run_item_build_does_not_rerun_the_items_install_hook() {
    // spec: HOOK-103 HOOK-108
    let sb = Sandbox::new("build-vs-install");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"tool\"\n",
                "name = \"builder\"\n",
                "path = \"tools/builder\"\n",
                "build = \"{}\"\n",
                "install = \"{}\"\n",
            ),
            sb.touch("built.sentinel"),
            sb.touch("installed.sentinel"),
        ),
    );
    sb.write_and_commit("tools/builder/run.sh", "#!/bin/sh\necho hi\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "builder"]);
    assert!(r.success, "learn: {}", r.stderr);
    assert!(
        !sb.sentinel("built.sentinel").exists() && !sb.sentinel("installed.sentinel").exists(),
        "a non-TTY learn skips both hooks"
    );

    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "build",
        "--dangerously-skip-build-hook-check",
        "build-vs-install#builder",
    ]);
    assert!(r.success, "build run: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.sentinel("built.sentinel").exists(),
        "the build hook must have run under its own bypass flag: {}",
        r.stdout
    );
    assert!(
        !sb.sentinel("installed.sentinel").exists(),
        "a --event build run must not re-run the item's INSTALL hook: {}",
        r.stdout
    );
}

/// `--event build` over a glob mixing an item with a build hook and one
/// without: the hookless item is a plain transactional re-install and the run
/// stays exit 0 (the HOOK-108 predicate is never consulted on this path, even
/// though the build hook was skipped for want of a terminal).
#[test]
fn hooks_run_item_build_glob_over_mixed_items_stays_exit_zero() {
    // spec: HOOK-103 HOOK-108 CLI-194
    let sb = Sandbox::new("build-mixed");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[items]]\n",
                "kind = \"tool\"\n",
                "name = \"alpha\"\n",
                "path = \"tools/alpha\"\n",
                "build = \"{}\"\n",
                "\n",
                "[[items]]\n",
                "kind = \"tool\"\n",
                "name = \"zeta\"\n",
                "path = \"tools/zeta\"\n",
            ),
            sb.touch("mixed-build.sentinel")
        ),
    );
    sb.write_and_commit("tools/alpha/run.sh", "#!/bin/sh\n");
    sb.write_and_commit("tools/zeta/run.sh", "#!/bin/sh\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "alpha"]);
    assert!(r.success, "learn alpha: {}", r.stderr);
    let r = sb.mind(&["learn", "zeta"]);
    assert!(r.success, "learn zeta: {}", r.stderr);

    let r = sb.mind(&["hooks", "run", "--event", "build", "build-mixed#*"]);
    assert!(
        r.success,
        "a build run over a mixed glob stays exit 0 even with the build hook \
         skipped: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.sentinel("mixed-build.sentinel").exists(),
        "the build hook is still skipped in a non-TTY run: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("rebuilding tool:alpha") && r.stdout.contains("rebuilding tool:zeta"),
        "both matched items must be re-installed: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// HOOK-109: the branches the seam makes reachable, and its blast radius.
// ---------------------------------------------------------------------------

/// The `HookAct::Abort` arm of `run_source_hooks`: a REQUIRED source hook the
/// user aborts is `HookAborted`, a non-zero exit, and stops the batch. Before
/// `MIND_TTY` existed this whole arm was unreachable from a test.
#[test]
fn hooks_run_interactive_abort_of_a_required_hook_stops_the_run() {
    // spec: HOOK-109 HOOK-100
    let sb = Sandbox::new("tty-abort");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"first\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
                "\n",
                "[[hooks]]\n",
                "name = \"second\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("abort-first.sentinel"),
            sb.touch("abort-second.sentinel"),
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    // Skip the first hook, abort at the second.
    let (r, timed_out) = sb.mind_env_stdin_bounded(
        &["hooks", "run", "--event", "install", "--force", "tty-abort"],
        &[("MIND_TTY", "1")],
        "n\na\n",
        60,
    );
    assert!(
        !timed_out,
        "the run must terminate: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !r.success,
        "an aborted required hook is a non-zero exit: {}\n{}",
        r.stdout, r.stderr
    );
    assert_eq!(r.code, Some(1), "a MindError exits 1: {}", r.stderr);
    assert!(
        r.stderr.contains("second"),
        "the error must name the aborted hook: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("want of consent"),
        "an abort is an active choice, not a consent failure: {}",
        r.stderr
    );
    assert!(
        !sb.sentinel("abort-first.sentinel").exists()
            && !sb.sentinel("abort-second.sentinel").exists(),
        "neither hook may run: {}",
        r.stdout
    );

    // The skip recorded before the abort must survive: `run_source_hooks`
    // saves a dirty registry on its way out of the abort arm.
    let registry = std::fs::read_to_string(sb.mind_home.join("sources.json"))
        .expect("sources.json must exist");
    assert!(
        registry.contains("abort-first.sentinel"),
        "the pre-abort skip must be persisted: {registry}"
    );
}

/// An OPTIONAL source hook prompts two-way (HOOK-52), so the abort reply has no
/// meaning there: `a` is an unrecognized answer and skips rather than aborting.
/// The run stays exit 0.
#[test]
fn hooks_run_interactive_optional_hook_never_aborts() {
    // spec: HOOK-109 HOOK-100
    let sb = Sandbox::new("tty-optional");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"maybe\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
                "optional = true\n",
            ),
            sb.touch("optional-ran.sentinel")
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let (r, timed_out) = sb.mind_env_stdin_bounded(
        &[
            "hooks",
            "run",
            "--event",
            "install",
            "--force",
            "tty-optional",
        ],
        &[("MIND_TTY", "1")],
        "a\n",
        60,
    );
    assert!(
        !timed_out,
        "the run must terminate: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("Run this optional hook?"),
        "an optional hook must take the two-way prompt: {}",
        r.stdout
    );
    assert!(
        r.success,
        "an optional hook can never abort the run: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.sentinel("optional-ran.sentinel").exists(),
        "an unclear reply must not run an optional hook: {}",
        r.stdout
    );
}

/// The bounded-input case the `mind_env_stdin` harness cannot express: two
/// hooks prompt, one reply is scripted. The second prompt reads EOF, which
/// HOOK-22 resolves as a skip, so the run terminates instead of blocking on a
/// reply that never comes.
#[test]
fn hooks_run_prompts_outnumbering_replies_terminate_on_eof() {
    // spec: HOOK-109 HOOK-100
    let sb = Sandbox::new("tty-short-input");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"first\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
                "\n",
                "[[hooks]]\n",
                "name = \"second\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("short-first.sentinel"),
            sb.touch("short-second.sentinel"),
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let (r, timed_out) = sb.mind_env_stdin_bounded(
        &[
            "hooks",
            "run",
            "--event",
            "install",
            "--force",
            "tty-short-input",
        ],
        &[("MIND_TTY", "1")],
        "y\n",
        60,
    );
    assert!(
        !timed_out,
        "a prompt with no reply left must resolve at EOF, not block: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.success,
        "an EOF skip is not an abort and not a consent failure: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        sb.sentinel("short-first.sentinel").exists(),
        "the answered hook must run: {}",
        r.stdout
    );
    assert!(
        !sb.sentinel("short-second.sentinel").exists(),
        "the unanswered hook must be skipped, never run on a stale reply: {}",
        r.stdout
    );
}

/// `MIND_TTY` set to the empty string is "not a terminal". A naive
/// implementation reading the variable as `var().ok().filter(non-empty)` would
/// fall through to `is_terminal()` instead; `var_os` returns `Some("")` here,
/// so the override applies and the non-TTY skip stands.
#[test]
fn empty_mind_tty_is_not_a_terminal_end_to_end() {
    // spec: HOOK-109
    let sb = Sandbox::new("tty-empty");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"setup\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("empty-tty.sentinel")
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let (r, timed_out) = sb.mind_env_stdin_bounded(
        &["hooks", "run", "--event", "install", "--force", "tty-empty"],
        &[("MIND_TTY", "")],
        "y\n",
        60,
    );
    assert!(!timed_out, "must terminate: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("not a terminal"),
        "an empty MIND_TTY must keep the non-TTY skip: {}",
        r.stdout
    );
    assert!(!r.success, "still a HOOK-107 error: {}", r.stdout);
    assert!(
        !sb.sentinel("empty-tty.sentinel").exists(),
        "the waiting reply must not be read: {}",
        r.stdout
    );
}

/// With `MIND_TTY` UNSET the seam is invisible: the answer comes from
/// `stdin().is_terminal()`, and a pipe is not a terminal, so a run with a real
/// reply waiting behaves exactly as the pre-seam binary did. This is the
/// control for every other test in the file, which relies on the unset default.
#[test]
fn unset_mind_tty_leaves_a_piped_stdin_non_interactive() {
    // spec: HOOK-109 HOOK-22
    let sb = Sandbox::new("tty-unset");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            concat!(
                "[[hooks]]\n",
                "name = \"setup\"\n",
                "run = \"{}\"\n",
                "event = \"install\"\n",
            ),
            sb.touch("unset-tty.sentinel")
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let (piped, timed_out) = sb.mind_env_stdin_bounded(
        &["hooks", "run", "--event", "install", "--force", "tty-unset"],
        &[],
        "y\n",
        60,
    );
    assert!(
        !timed_out,
        "must terminate: {}\n{}",
        piped.stdout, piped.stderr
    );
    assert!(
        piped.stdout.contains("not a terminal"),
        "a pipe is not a terminal when MIND_TTY is unset: {}",
        piped.stdout
    );
    assert!(
        !sb.sentinel("unset-tty.sentinel").exists(),
        "the reply on the pipe must not be read as consent: {}",
        piped.stdout
    );

    // Byte-for-byte the same as the /dev/null stdin path the rest of the suite
    // uses, so the seam changed nothing when the variable is absent.
    let null = sb.mind(&["hooks", "run", "--event", "install", "--force", "tty-unset"]);
    assert_eq!(
        null.stdout, piped.stdout,
        "an unset MIND_TTY must produce identical output on a pipe and on \
         /dev/null"
    );
    assert_eq!(null.code, piped.code, "and the same exit code");
}

/// `MIND_TTY` is process-wide, so it turns on the prompt paths of every other
/// verb too. The CLI-23 post-meld install prompt is the widest of them: it must
/// become reachable and correct, not reachable and broken. Both arms are
/// exercised -- an EOF reply declines (and installs nothing), an explicit `y`
/// installs -- and neither may hang.
#[test]
fn mind_tty_makes_other_verbs_prompts_reachable_and_correct() {
    // spec: HOOK-109 CLI-23
    let sb = Sandbox::new("tty-other-verb");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: widget\n---\n# widget\n",
    );

    // EOF at the prompt declines: nothing is installed, and the run ends.
    let (r, timed_out) =
        sb.mind_env_stdin_bounded(&["meld", &sb.source_spec()], &[("MIND_TTY", "1")], "", 60);
    assert!(
        !timed_out,
        "an EOF at another verb's prompt must not hang: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install these"),
        "MIND_TTY must make the CLI-23 prompt reachable: {}",
        r.stdout
    );
    assert!(
        !sb.claude_home.join("skills/widget").exists(),
        "an EOF reply must decline, not install: {}",
        r.stdout
    );

    // And an explicit yes installs, so the prompt is not merely printed.
    let (r, timed_out) = sb.mind_env_stdin_bounded(
        &["learn", "tty-other-verb#*"],
        &[("MIND_TTY", "1")],
        "y\n",
        60,
    );
    assert!(!timed_out, "must terminate: {}\n{}", r.stdout, r.stderr);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/widget").exists(),
        "the item must install: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract the CLI-181 JSON error envelope from a `--json` run's stdout.
///
/// Since CLI-217, `hooks_cmd`'s advisory notes route to stderr under `--json`
/// (see `hooks_run_json_stdout_is_only_the_envelope`), so stdout is normally
/// just the envelope already. This helper stays defensive regardless:
/// `print_json` pretty-prints, so the envelope begins at the first line that
/// is exactly `{`.
fn json_envelope(stdout: &str) -> serde_json::Value {
    let start = stdout
        .lines()
        .position(|l| l.trim_end() == "{")
        .unwrap_or_else(|| panic!("no JSON envelope found in stdout: {stdout:?}"));
    let body: String = stdout.lines().skip(start).collect::<Vec<_>>().join("\n");
    serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("envelope must parse as JSON ({e}): {body:?}"))
}

/// Initialise a second source git repo with a `mind.toml`, mirroring the setup
/// `Sandbox::new` does for the primary source.
fn init_source_repo(dir: &Path, mind_toml: &str) {
    write(&dir.join("README.md"), "# fixture\n");
    write(&dir.join("mind.toml"), mind_toml);
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "initial"]);
}

/// Walk `mind_home` looking for a file named `filename` inside `sources/`.
/// Returns the first match found, if any.
fn find_sentinel(mind_home: &Path, filename: &str) -> Option<PathBuf> {
    let sources_dir = mind_home.join("sources");
    walk_find(&sources_dir, filename)
}

fn walk_find(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = walk_find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

/// Read a log file at an explicit path, returning non-empty lines.
/// Returns an empty vec if the file doesn't exist.
fn read_log_file(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// Read a log file located in the clone dir under mind_home/sources/, returning
/// non-empty lines. Returns an empty vec if the file doesn't exist.
#[allow(dead_code)]
fn read_log_in_clone(mind_home: &Path, filename: &str) -> Vec<String> {
    match find_sentinel(mind_home, filename) {
        None => vec![],
        Some(path) => std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect(),
    }
}
