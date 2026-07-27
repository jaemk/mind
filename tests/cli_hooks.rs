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
            "mind hooks run {identity} --dangerously-skip-install-hook-check"
        )),
        "should print the exact copy-pasteable remedy: {out}"
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
