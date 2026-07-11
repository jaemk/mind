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
//!   CLI-194:  `hooks` verb and target parsing (source vs. item ref)
//!   CLI-195:  `hooks run` with --event and --force flags
//!   CLI-196:  `hooks list` subcommand

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

/// In non-TTY, `hooks run --event install <source>` skips hooks and exits 0.
/// (HOOK-22: non-TTY always skips; this is not an error for optional hooks.)
#[test]
fn hooks_run_source_install_skips_in_non_tty() {
    // spec: HOOK-100 HOOK-101
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

    // stdin is null (non-TTY): hook is skipped, command exits 0.
    let r = sb.mind(&["hooks", "run", "--event", "install", "src-skip"]);
    assert!(
        r.success,
        "non-TTY skip should exit 0: {}\n{}",
        r.stdout, r.stderr
    );
    // Output should note the skip.
    let out = r.stdout;
    assert!(
        out.contains("skipped") || out.contains("skip"),
        "should mention skip in non-TTY: {out}"
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

/// In non-TTY, `hooks run --event uninstall <source>` skips and exits 0.
#[test]
fn hooks_run_source_uninstall_skips_in_non_tty() {
    // spec: HOOK-100
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
        r.success,
        "uninstall non-TTY skip should exit 0: {}\n{}",
        r.stdout, r.stderr
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

/// `hooks run --event uninstall <source>#<item>` in non-TTY skips the hook.
#[test]
fn hooks_run_item_uninstall_hook_skips_in_non_tty() {
    // spec: HOOK-102 CLI-194
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

    // In non-TTY, uninstall hook is skipped (not an error).
    let r = sb.mind(&[
        "hooks",
        "run",
        "--event",
        "uninstall",
        "item-uninstall-src#fetcher",
    ]);
    assert!(
        r.success,
        "non-TTY uninstall skip should exit 0: {}\n{}",
        r.stdout, r.stderr
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
    // spec: HOOK-101 HOOK-55
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

    // First run in non-TTY: the hook is skipped (records ran_at = None).
    let r = sb.mind(&["hooks", "run", "--event", "install", "skip-pending"]);
    assert!(r.success, "skip run: {}\n{}", r.stdout, r.stderr);
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
// Helper functions
// ---------------------------------------------------------------------------

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
