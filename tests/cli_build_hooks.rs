//! Integration tests for item build hooks with `--dangerously-skip-build-hook-check`.
//!
//! Before HOOK-74, build hooks could not run in a non-TTY context at all (HOOK-72
//! skips them). These tests exercise the new headless path and its invariants:
//!
//! - (a) build hook runs during `learn --dangerously-skip-build-hook-check`
//! - (b) a failing build hook rolls the install back (HOOK-71)
//! - (c) the build hook re-runs when the item is reinstalled/upgraded (HOOK-73)
//! - (d) without the flag, a non-TTY context still skips the build hook (HOOK-72)

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
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-bld-{}-{n}", std::process::id()));
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
// (a) build hook runs and produces side effects under the flag (HOOK-71/74)
// ---------------------------------------------------------------------------

#[test]
fn build_hook_runs_when_flag_is_given() {
    // spec: HOOK-71 HOOK-74
    // With --dangerously-skip-build-hook-check, the build hook must execute
    // during `learn`. The hook creates a sentinel file inside the tool's staging
    // directory, which ends up in the store after the swap (HOOK-71: the hook
    // runs in staging, its output lands in the store atomically on success).
    let sb = Sandbox::new("bld");
    let sentinel_name = "built.txt";

    // A tool whose build hook writes a sentinel file.
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"mytool\"\n",
            "path = \"tools/mytool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    sb.write_and_commit(
        "tools/mytool/TOOL.md",
        "---\ndescription: test tool\n---\n# mytool\n",
    );
    sb.write_and_commit("mind.toml", &toml);

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let learn = sb.mind(&[
        "learn",
        "tool:mytool",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(
        learn.success,
        "learn with --dangerously-skip-build-hook-check must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    // The sentinel file must exist in the store (proving the hook ran in staging
    // and its output was swapped into the store, HOOK-71).
    let store_sentinel = sb.mind_home.join("store/tool/mytool").join(sentinel_name);
    assert!(
        store_sentinel.exists(),
        "build hook must have created {sentinel_name} in the store: {store_sentinel:?}"
    );
}

// ---------------------------------------------------------------------------
// (b) failing build hook rolls back the install (HOOK-71)
// ---------------------------------------------------------------------------

#[test]
fn failing_build_hook_rolls_back_install() {
    // spec: HOOK-71 HOOK-74
    // A build hook that exits non-zero is a hard stop: the staging copy is
    // discarded and the live install is untouched. With the flag the hook runs
    // (not skipped), so the failure is observable.
    let sb = Sandbox::new("fail");

    let toml = concat!(
        "[[items]]\n",
        "kind = \"tool\"\n",
        "name = \"badtool\"\n",
        "path = \"tools/badtool\"\n",
        "build = \"exit 7\"\n",
    );
    sb.write_and_commit(
        "tools/badtool/TOOL.md",
        "---\ndescription: failing build\n---\n# badtool\n",
    );
    sb.write_and_commit("mind.toml", toml);

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let learn = sb.mind(&[
        "learn",
        "tool:badtool",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(
        !learn.success,
        "learn must fail when the build hook exits non-zero (HOOK-71): {} {}",
        learn.stdout, learn.stderr
    );

    // The store copy must be absent: staging discarded, live install untouched.
    let store_path = sb.mind_home.join("store/tool/badtool");
    assert!(
        !store_path.exists(),
        "store copy must not exist after a failed build hook (HOOK-71): {store_path:?}"
    );

    // The error output must surface the hook failure.
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        combined.contains("exit 7") || combined.contains("failed") || combined.contains("build"),
        "failure output must reference the build hook error: {combined}"
    );
}

// ---------------------------------------------------------------------------
// (c) build hook re-runs on reinstall and upgrade (HOOK-73)
// ---------------------------------------------------------------------------

#[test]
fn build_hook_reruns_on_reinstall() {
    // spec: HOOK-73 HOOK-74
    // Every (re)install rebuilds the store copy from staging, so the build hook
    // runs again. A counter file in the tool dir lets us observe the number of
    // runs: the hook appends a line each time it executes.
    let sb = Sandbox::new("rerun");
    let run_log = sb.base.join("run.log");
    let log_path = run_log.display().to_string();

    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"counter\"\n",
            "path = \"tools/counter\"\n",
            "build = \"echo RAN >> {log}\"\n",
        ),
        log = log_path
    );
    sb.write_and_commit(
        "tools/counter/TOOL.md",
        "---\ndescription: counter tool\n---\n# counter\n",
    );
    sb.write_and_commit("mind.toml", &toml);

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // First learn: build hook runs once.
    let learn1 = sb.mind(&[
        "learn",
        "tool:counter",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(
        learn1.success,
        "first learn must succeed: {} {}",
        learn1.stdout, learn1.stderr
    );
    let lines_after_first = std::fs::read_to_string(&run_log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("RAN"))
        .count();
    assert_eq!(
        lines_after_first, 1,
        "build hook must have run once after the first learn (HOOK-73)"
    );

    // Forget and re-learn (reinstall): build hook runs again.
    assert!(sb.mind(&["forget", "tool:counter"]).success);
    let learn2 = sb.mind(&[
        "learn",
        "tool:counter",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(
        learn2.success,
        "second learn must succeed: {} {}",
        learn2.stdout, learn2.stderr
    );
    let lines_after_second = std::fs::read_to_string(&run_log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("RAN"))
        .count();
    assert_eq!(
        lines_after_second, 2,
        "build hook must re-run on reinstall (HOOK-73): total runs should be 2"
    );
}

#[test]
fn build_hook_reruns_on_upgrade() {
    // spec: HOOK-73 HOOK-74
    // `upgrade` reinstalls the item from the new staging copy, so the build hook
    // runs again whenever the item is upgraded. A log file lets us count runs.
    let sb = Sandbox::new("upg");
    let run_log = sb.base.join("upg.log");
    let log_path = run_log.display().to_string();

    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"utool\"\n",
            "path = \"tools/utool\"\n",
            "build = \"echo RAN >> {log}\"\n",
        ),
        log = log_path
    );
    sb.write_and_commit(
        "tools/utool/TOOL.md",
        "---\ndescription: upgradeable tool\n---\n# utool\n",
    );
    sb.write_and_commit("mind.toml", &toml);

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // First install.
    let learn = sb.mind(&["learn", "tool:utool", "--dangerously-skip-build-hook-check"]);
    assert!(
        learn.success,
        "initial learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert_eq!(
        std::fs::read_to_string(&run_log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("RAN"))
            .count(),
        1,
        "build hook ran once after initial learn"
    );

    // Modify the tool to create a new hash, then upgrade.
    sb.write_and_commit(
        "tools/utool/TOOL.md",
        "---\ndescription: upgradeable tool v2\n---\n# utool\n",
    );
    let upgrade = sb.mind(&["upgrade", "--yes", "--dangerously-skip-build-hook-check"]);
    assert!(
        upgrade.success,
        "upgrade must succeed: {} {}",
        upgrade.stdout, upgrade.stderr
    );

    let total_runs = std::fs::read_to_string(&run_log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("RAN"))
        .count();
    assert_eq!(
        total_runs, 2,
        "build hook must re-run on upgrade (HOOK-73): total runs should be 2, got {total_runs}"
    );
}

// ---------------------------------------------------------------------------
// (d) without the flag, non-TTY still skips (HOOK-72 regression guard)
// ---------------------------------------------------------------------------

#[test]
fn build_hook_is_skipped_without_flag_in_non_tty() {
    // spec: HOOK-72
    // Without --dangerously-skip-build-hook-check, the non-TTY (stdin=null)
    // context must skip the build hook and still succeed (the item installs
    // unbuilt). The sentinel file that the hook would create must NOT exist.
    let sb = Sandbox::new("skip");
    let sentinel_name = "should_not_exist.txt";

    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"skiptool\"\n",
            "path = \"tools/skiptool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    sb.write_and_commit(
        "tools/skiptool/TOOL.md",
        "---\ndescription: skip tool\n---\n# skiptool\n",
    );
    sb.write_and_commit("mind.toml", &toml);

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // No --dangerously-skip-build-hook-check: hook must be skipped.
    let learn = sb.mind(&["learn", "tool:skiptool"]);
    assert!(
        learn.success,
        "learn without the flag must still succeed (item installs unbuilt, HOOK-72): {} {}",
        learn.stdout, learn.stderr
    );

    // The item must be in the store (it installed, just not built).
    assert!(
        sb.mind_home.join("store/tool/skiptool").exists(),
        "item must be in the store even when its build hook was skipped"
    );

    // The sentinel must not exist: the hook did not run.
    let store_sentinel = sb.mind_home.join("store/tool/skiptool").join(sentinel_name);
    assert!(
        !store_sentinel.exists(),
        "build hook must NOT run in a non-TTY context without the flag (HOOK-72): {store_sentinel:?}"
    );

    // A note about the skipped hook must appear in output.
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        combined.contains("skipped") && combined.contains("build"),
        "output must mention that the build hook was skipped (HOOK-72): {combined}"
    );
}
