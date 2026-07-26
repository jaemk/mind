//! End-to-end tests for item-level lifecycle hooks: the `[[items.hooks]]` array
//! (HOOK-86), nested teardown ordering at `unmeld` (HOOK-87), and the
//! prefix-gated `init-source` unguarded-reference advisory (INIT-9).
//!
//! These drive the real `mind` binary against a hermetic, network-free fixture
//! (a local git repo melded by filesystem path, with MIND_HOME/CLAUDE_HOME
//! pointed at temp dirs), exactly as `tests/cli.rs` does. Where a non-TTY test
//! needs the hooks to actually run, it passes
//! `--dangerously-skip-install-hook-check` (HOOK-23/83).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway environment: a source git repo plus isolated MIND_HOME/CLAUDE_HOME.
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
    /// A source repo named `name` with no committed items yet (the test seeds
    /// its own files and `mind.toml`).
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-itlc-{}-{n}", std::process::id()));
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

    /// Run `mind` with its working directory set to `cwd` (for `init-source .`).
    fn mind_cwd(&self, args: &[&str], cwd: &Path) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .current_dir(cwd)
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

/// Read a log file the hooks append to, returning its non-empty lines in order.
fn read_log(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// HOOK-86: item-level [[items.hooks]] array
// ---------------------------------------------------------------------------

#[test]
fn item_hooks_array_runs_install_then_uninstall_in_declaration_order() {
    // spec: HOOK-86
    // An item declaring multiple [[items.hooks]] (two install, two uninstall)
    // runs each in declaration order at install (learn) and removal (forget).
    // Each hook appends a tagged line to a shared log; the order is asserted.
    let sb = Sandbox::new("arr");
    let log = sb.base.join("order.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    // Two install hooks and two uninstall hooks, plus a named one, interleaved so
    // declaration order (not event grouping) is what we observe per event.
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo i1 >> {lg}\"\n",
            "name = \"first install\"\n",
            "event = \"install\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo u1 >> {lg}\"\n",
            "event = \"uninstall\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo i2 >> {lg}\"\n",
            "event = \"install\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo u2 >> {lg}\"\n",
            "event = \"uninstall\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    let learn = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        learn.success,
        "learn should run both install hooks: {} {}",
        learn.stdout, learn.stderr
    );
    // Only the install hooks ran, in declaration order (i1 before i2).
    assert_eq!(read_log(&log), vec!["i1", "i2"], "install hooks in order");

    let forget = sb.mind(&[
        "forget",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        forget.success,
        "forget should run both uninstall hooks: {} {}",
        forget.stdout, forget.stderr
    );
    // The uninstall hooks appended after the install ones, in declaration order.
    assert_eq!(
        read_log(&log),
        vec!["i1", "i2", "u1", "u2"],
        "uninstall hooks run in declaration order after install"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "item removed after its uninstall hooks"
    );
}

#[test]
fn scalar_install_uninstall_still_work_as_one_required_hook_each() {
    // spec: HOOK-86
    // The scalar install/uninstall keys remain the one-required-hook shorthand:
    // an item declaring only the scalars runs each at the matching lifecycle step.
    let sb = Sandbox::new("scal");
    let log = sb.base.join("scalar.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"echo SCALAR-INSTALL >> {lg}\"\n",
            "uninstall = \"echo SCALAR-UNINSTALL >> {lg}\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert_eq!(read_log(&log), vec!["SCALAR-INSTALL"], "scalar install ran");

    assert!(
        sb.mind(&[
            "forget",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert_eq!(
        read_log(&log),
        vec!["SCALAR-INSTALL", "SCALAR-UNINSTALL"],
        "scalar uninstall ran on forget"
    );
}

// ---------------------------------------------------------------------------
// HOOK-87: nested teardown order (item uninstall before source uninstall)
// ---------------------------------------------------------------------------

#[test]
fn unmeld_runs_item_uninstall_hooks_before_source_uninstall_hooks() {
    // spec: HOOK-87
    // Teardown reverses install: at unmeld each installed item's uninstall hooks
    // run BEFORE the source's uninstall hooks. Each hook appends a tagged line to
    // a shared log; the item line must precede the source line.
    let sb = Sandbox::new("nest");
    let log = sb.base.join("teardown.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    // One item with an uninstall hook, plus a source-level uninstall hook.
    let toml = format!(
        concat!(
            "[[hooks]]\n",
            "run = \"echo SOURCE-UNINSTALL >> {lg}\"\n",
            "event = \"uninstall\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "uninstall = \"echo ITEM-UNINSTALL >> {lg}\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Register and install the item (link-only meld so the source hook does not
    // run at meld; the item install needs no hook here).
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success,
        "item must install before unmeld"
    );
    assert!(read_log(&log).is_empty(), "no teardown hooks at meld/learn");

    // unmeld: the item's uninstall hook then the source's uninstall hook.
    let unmeld = sb.mind(&["unmeld", "nest", "--dangerously-skip-install-hook-check"]);
    assert!(
        unmeld.success,
        "unmeld should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    assert_eq!(
        read_log(&log),
        vec!["ITEM-UNINSTALL", "SOURCE-UNINSTALL"],
        "item uninstall hook must fire before the source uninstall hook"
    );

    // The source is gone.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("nest"),
        "source must be removed after unmeld: {sources}"
    );
}

#[test]
fn unmeld_non_tty_skips_hooks_but_still_removes_source_and_items() {
    // spec: HOOK-87
    // A non-TTY `unmeld` WITHOUT --dangerously-skip-install-hook-check takes the
    // HOOK-22 skip path for every hook (item AND source uninstall), yet the source
    // is still removed and the item is still torn down. No hook side effect lands.
    let sb = Sandbox::new("ntty");
    let log = sb.base.join("skip.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[hooks]]\n",
            "run = \"echo SOURCE-UNINSTALL >> {lg}\"\n",
            "event = \"uninstall\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "uninstall = \"echo ITEM-UNINSTALL >> {lg}\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(
        sb.mind_home.join("store/skill/greet").exists(),
        "item installed before unmeld"
    );

    // No --dangerously-skip flag: non-TTY (stdin is /dev/null) skips every hook.
    let unmeld = sb.mind(&["unmeld", "ntty"]);
    assert!(
        unmeld.success,
        "non-TTY unmeld still succeeds: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    // HOOK-22: every hook was skipped, so NOTHING was appended to the log.
    assert!(
        read_log(&log).is_empty(),
        "non-TTY must skip both item and source uninstall hooks: {:?}",
        read_log(&log)
    );
    // But the teardown still happened: item removed, source gone.
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "item must still be removed even though its uninstall hook was skipped"
    );
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("ntty"),
        "source must still be removed: {sources}"
    );
}

#[test]
fn unmeld_yes_runs_item_uninstall_hooks_for_multiple_items_before_source() {
    // spec: HOOK-87
    // A multi-item `unmeld --yes` (bypassing the CLI-42 confirm) runs EACH item's
    // uninstall hooks before the source's uninstall hook. Two items each append a
    // tagged line; both must precede the source line.
    let sb = Sandbox::new("multi");
    let log = sb.base.join("multi.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/alpha/SKILL.md"),
        "---\ndescription: alpha\n---\n# alpha\n",
    );
    write(
        &sb.source.join("skills/beta/SKILL.md"),
        "---\ndescription: beta\n---\n# beta\n",
    );
    let toml = format!(
        concat!(
            "[[hooks]]\n",
            "run = \"echo SOURCE-UNINSTALL >> {lg}\"\n",
            "event = \"uninstall\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"alpha\"\n",
            "path = \"skills/alpha\"\n",
            "uninstall = \"echo ITEM-ALPHA >> {lg}\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"beta\"\n",
            "path = \"skills/beta\"\n",
            "uninstall = \"echo ITEM-BETA >> {lg}\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:alpha",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(
        sb.mind(&[
            "learn",
            "skill:beta",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );

    // --yes bypasses the multi-item CLI-42 confirm; --dangerously-skip runs hooks.
    let unmeld = sb.mind(&[
        "unmeld",
        "multi",
        "--yes",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        unmeld.success,
        "multi-item unmeld --yes should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    let lines = read_log(&log);
    // Both item hooks must run before the source hook (HOOK-87 inner-to-outer).
    let src_pos = lines
        .iter()
        .position(|l| l == "SOURCE-UNINSTALL")
        .expect("source uninstall hook must run");
    let alpha_pos = lines
        .iter()
        .position(|l| l == "ITEM-ALPHA")
        .expect("alpha uninstall hook must run");
    let beta_pos = lines
        .iter()
        .position(|l| l == "ITEM-BETA")
        .expect("beta uninstall hook must run");
    assert!(
        alpha_pos < src_pos && beta_pos < src_pos,
        "both item uninstall hooks must precede the source uninstall hook: {lines:?}"
    );
    assert!(
        !sb.mind_home.join("store/skill/alpha").exists()
            && !sb.mind_home.join("store/skill/beta").exists(),
        "both items removed after unmeld"
    );
}

// spec: HOOK-87
// Install half of the nested order: source install hook runs BEFORE item
// install hooks. At `meld` (without --link-only), the source's `[[hooks]]`
// install entry runs first (in meld_recursive), and item install hooks run
// later (in install_melded_source -> learn). Both happen in a single
// `mind meld --yes --dangerously-skip-install-hook-check` invocation.
// The log therefore records SOURCE-INSTALL first, then ITEM-INSTALL.
#[test]
fn meld_runs_source_install_hook_before_item_install_hooks() {
    // spec: HOOK-87
    let sb = Sandbox::new("instord");
    let log = sb.base.join("install_order.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    // Source-level install hook (appends SOURCE-INSTALL) plus an item-level
    // install hook (appends ITEM-INSTALL). Under HOOK-87 the source hook must
    // appear before the item hook.
    let toml = format!(
        concat!(
            "[[hooks]]\n",
            "run = \"echo SOURCE-INSTALL >> {lg}\"\n",
            "event = \"install\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"echo ITEM-INSTALL >> {lg}\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // --yes: install items immediately after meld (no interactive prompt).
    // --dangerously-skip-install-hook-check: run all hooks unattended (HOOK-23).
    // Without --link-only so both the source and item install hooks fire.
    let meld = sb.mind(&[
        "meld",
        &spec,
        "--yes",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        meld.success,
        "meld --yes --dangerously-skip should succeed: {} {}",
        meld.stdout, meld.stderr
    );

    let lines = read_log(&log);
    // Both hooks must have run.
    assert!(
        lines.contains(&"SOURCE-INSTALL".to_string()),
        "source install hook must run: {lines:?}"
    );
    assert!(
        lines.contains(&"ITEM-INSTALL".to_string()),
        "item install hook must run: {lines:?}"
    );
    // HOOK-87: source install runs BEFORE item install (outer-to-inner).
    let src_pos = lines
        .iter()
        .position(|l| l == "SOURCE-INSTALL")
        .expect("SOURCE-INSTALL must appear in the log");
    let item_pos = lines
        .iter()
        .position(|l| l == "ITEM-INSTALL")
        .expect("ITEM-INSTALL must appear in the log");
    assert!(
        src_pos < item_pos,
        "source install hook (pos {src_pos}) must precede item install hook (pos {item_pos}): {lines:?}"
    );
    // Confirm the item was actually installed.
    assert!(
        sb.mind_home.join("store/skill/greet").exists(),
        "item must be in the store after a successful meld --yes"
    );
}

#[test]
fn unmeld_item_uninstall_hook_failure_leaves_source_melded() {
    // spec: HOOK-87
    // A non-zero exit from an item's uninstall hook is a hard stop (HOOK-53/82):
    // the unmeld stops, the failing item stays installed, and the source stays
    // melded. The source's own uninstall hook (which runs LAST) must NOT have run.
    let sb = Sandbox::new("failun");
    let log = sb.base.join("fail.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    // Item uninstall hook exits non-zero; the source uninstall hook would append
    // a line we assert never appears (it runs only after items succeed).
    let toml = format!(
        concat!(
            "[[hooks]]\n",
            "run = \"echo SOURCE-UNINSTALL >> {lg}\"\n",
            "event = \"uninstall\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "uninstall = \"exit 7\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );

    let unmeld = sb.mind(&["unmeld", "failun", "--dangerously-skip-install-hook-check"]);
    assert!(
        !unmeld.success,
        "an item uninstall-hook failure must fail the unmeld: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    // HOOK-82: the failing item is left installed.
    assert!(
        sb.mind_home.join("store/skill/greet").exists(),
        "item must remain installed after its uninstall hook failed"
    );
    // The source's uninstall hook (runs last) must not have fired.
    assert!(
        !read_log(&log).contains(&"SOURCE-UNINSTALL".to_string()),
        "source uninstall hook must NOT run when an item uninstall hook failed: {:?}",
        read_log(&log)
    );
    // HOOK-53: the source remains melded.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("failun"),
        "source must remain melded after a failed item uninstall hook: {sources}"
    );

    // M3: tie the abort to the FAILING item uninstall hook.
    // The unmeld must have exited non-zero (already asserted above) AND its
    // output must surface the hook failure: either the "running uninstall hook
    // for skill:greet" indication (stdout) or the "failed (exit 7)" error text
    // (stderr, via MindError::HookFailed). Both together give the clearest
    // signal, but either suffices -- what matters is that the exit code came
    // from THIS hook, not from something else.
    let combined = format!("{}{}", unmeld.stdout, unmeld.stderr);
    assert!(
        combined.contains("greet"),
        "the failure output must reference the failing item 'greet': {combined}"
    );
    assert!(
        combined.contains("exit 7")
            || combined.contains("HookFailed")
            || combined.contains("failed"),
        "the failure output must surface the hook exit code or error: {combined}"
    );
}

#[test]
fn item_install_hook_failure_rolls_back_the_item_install() {
    // spec: HOOK-81
    // A non-zero exit from an item's install hook is a hard stop that rolls back
    // that item's install: its store copy and links are removed, leaving it not
    // installed. With TWO install hooks, a failure of the SECOND must still roll
    // back the whole item (the first hook's host side effect is not undone, but
    // the item itself is gone).
    let sb = Sandbox::new("failin");
    let log = sb.base.join("failin.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo FIRST-RAN >> {lg}\"\n",
            "event = \"install\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"exit 3\"\n",
            "event = \"install\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    let learn = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !learn.success,
        "a failing item install hook must fail the learn: {} {}",
        learn.stdout, learn.stderr
    );
    // The first hook ran (declaration order), proving the second one is what failed.
    assert_eq!(
        read_log(&log),
        vec!["FIRST-RAN"],
        "install hooks run in order; the first ran before the second failed"
    );
    // HOOK-81 rollback: the item's store copy is gone (not installed).
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the failed item install must be rolled back (store copy removed)"
    );
    // It is not recorded in the installed manifest (keyed `kind:name`).
    let manifest = std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        !manifest.contains("skill:greet"),
        "the rolled-back item must not be recorded in the manifest: {manifest}"
    );
}

#[test]
fn optional_item_hook_parses_and_runs_two_way_without_abort() {
    // spec: HOOK-86
    // Per HOOK-83, item hooks are always two-way (run/skip) regardless of
    // `optional`. Confirm `optional = true` parses and the hook runs unattended
    // under --dangerously-skip-install-hook-check (which runs optional and
    // required alike), and that a non-TTY install simply SKIPS it (no abort, the
    // item still installs).
    let sb = Sandbox::new("opt");
    let log = sb.base.join("opt.log");
    let lg = log.display();
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "\n",
            "[[items.hooks]]\n",
            "run = \"echo OPT-RAN >> {lg}\"\n",
            "optional = true\n",
            "event = \"install\"\n",
        ),
        lg = lg,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // Non-TTY without the flag: the optional hook is SKIPPED, item still installs.
    let learn_skip = sb.mind(&["learn", "skill:greet"]);
    assert!(
        learn_skip.success,
        "non-TTY install with an optional hook still installs the item: {} {}",
        learn_skip.stdout, learn_skip.stderr
    );
    assert!(
        sb.mind_home.join("store/skill/greet").exists(),
        "item installs even though its optional hook was skipped"
    );
    assert!(
        read_log(&log).is_empty(),
        "non-TTY skip: the optional hook must not have run: {:?}",
        read_log(&log)
    );

    // Forget it, then re-learn with the dangerous flag: the optional hook runs.
    assert!(sb.mind(&["forget", "skill:greet"]).success);
    let learn_run = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        learn_run.success,
        "optional hook runs unattended under the dangerous flag: {} {}",
        learn_run.stdout, learn_run.stderr
    );
    assert_eq!(
        read_log(&log),
        vec!["OPT-RAN"],
        "the optional install hook ran unattended"
    );
}

// ---------------------------------------------------------------------------
// INIT-9: prefix-gated unguarded-reference advisory
// ---------------------------------------------------------------------------

/// Seed a source with two agents where one mentions the other's bare name in
/// prose. `prefix` (when Some) is written as `[source].prefix` in `mind.toml`.
fn init_fixture(name: &str, prefix: Option<&str>) -> Sandbox {
    let sb = Sandbox::new(name);
    // `dev` mentions sibling `review` in bare prose (an unguarded reference).
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\nHand off to review when done.\n",
    );
    write(
        &sb.source.join("agents/review.md"),
        "---\ndescription: review agent\n---\n# review\n",
    );
    if let Some(p) = prefix {
        write(
            &sb.source.join("mind.toml"),
            &format!("[source]\nprefix = \"{p}\"\n"),
        );
    }
    sb
}

#[test]
fn init_source_without_prefix_emits_no_unguarded_reference_advisory() {
    // spec: INIT-9
    // With no effective prefix, a sibling named in bare prose is NOT flagged: an
    // unprefixed source's bare references resolve as written.
    let sb = init_fixture("noprefix", None);
    let r = sb.mind_cwd(&["init-source", "."], &sb.source);
    assert!(
        r.success,
        "init-source should succeed: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("unguarded-reference"),
        "no prefix => no unguarded-reference advisory: {combined}"
    );
}

#[test]
fn init_source_with_prefix_emits_the_unguarded_reference_advisory() {
    // spec: INIT-9
    // With an effective prefix in force ([source].prefix), the same bare-prose
    // sibling mention IS flagged as an unguarded-reference advisory.
    let sb = init_fixture("prefixed", Some("jk"));
    let r = sb.mind_cwd(&["init-source", "."], &sb.source);
    assert!(
        r.success,
        "init-source should succeed: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("unguarded-reference"),
        "a prefix in force must flag the bare reference: {combined}"
    );
    // The advisory names the referencing item AND the sibling it mentions,
    // both on the same line that carries the "unguarded-reference" kind token.
    // Format (from print_findings): "advisory [unguarded-reference]: agent:dev:
    // references sibling(s) in prose: review; ..."
    // Anchoring on the kind token plus the sibling name on a single line prevents
    // a spurious match if "review" appears in unrelated output chrome.
    let advisory_line = combined.lines().find(|l| l.contains("unguarded-reference"));
    assert!(
        advisory_line.is_some(),
        "must have a line containing 'unguarded-reference': {combined}"
    );
    let advisory_line = advisory_line.unwrap();
    assert!(
        advisory_line.contains("review"),
        "the unguarded-reference advisory line must name the sibling 'review': {advisory_line}"
    );
    assert!(
        advisory_line.contains("dev"),
        "the unguarded-reference advisory line must name the referencing item 'dev': {advisory_line}"
    );
}

// ---------------------------------------------------------------------------
// CLI-203: `learn <url> --pin` on an already-melded link prints a note that
// --pin was ignored, rather than silently dropping the flag.
// ---------------------------------------------------------------------------

#[test]
fn learn_url_pin_on_already_melded_link_prints_ignored_note() {
    // spec: CLI-203 — the first `learn <url>` registers the link instance; a
    // second `learn <url> --pin` finds it already melded, so the meld+pin step
    // is skipped. `learn` must say --pin was ignored (already melded) instead of
    // silently doing nothing, and the install still succeeds (exit 0).
    let sb = Sandbox::new("pin-note-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let url = format!("file://{}/tree/main/skills/review", sb.source_spec());

    // First learn: registers the link instance and installs the skill.
    let first = sb.mind(&["learn", &url]);
    assert!(
        first.success,
        "first `learn <url>` failed: {} {}",
        first.stdout, first.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the linked skill must install on first learn"
    );

    // Second learn with --pin: the instance is already melded, so --pin is a
    // no-op that must be announced.
    let second = sb.mind(&["learn", &url, "--pin"]);
    assert!(
        second.success,
        "second `learn <url> --pin` must still exit 0: {} {}",
        second.stdout, second.stderr
    );
    let combined = format!("{}{}", second.stdout, second.stderr);
    assert!(
        combined.contains("--pin") && combined.contains("already melded"),
        "a second `learn <url> --pin` must note that --pin was ignored because \
         the instance is already melded: {combined}"
    );
}

#[test]
fn learn_url_pin_note_suppressed_under_json() {
    // spec: CLI-203 — the note is suppressed under --json (consistent with the
    // neighboring meld notes), so machine consumers see clean output.
    let sb = Sandbox::new("pin-note-json-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let url = format!("file://{}/tree/main/skills/review", sb.source_spec());

    let first = sb.mind(&["learn", &url]);
    assert!(
        first.success,
        "first learn failed: {} {}",
        first.stdout, first.stderr
    );

    let second = sb.mind(&["--json", "learn", &url, "--pin"]);
    assert!(
        second.success,
        "second `--json learn <url> --pin` must exit 0: {} {}",
        second.stdout, second.stderr
    );
    assert!(
        !second.stdout.contains("already melded"),
        "the --pin-ignored note must be suppressed under --json (stdout): {}",
        second.stdout
    );
}

#[test]
fn learn_url_pin_first_learn_never_prints_ignored_note() {
    // spec: CLI-203 — the note fires only when the instance is ALREADY melded.
    // A first-ever `learn <url> --pin` registers and freezes the link (CLI-200)
    // through the normal meld+pin path; it must not print the "--pin ignored"
    // note, since the flag was honored, not dropped.
    let sb = Sandbox::new("pin-note-first-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let url = format!("file://{}/tree/main/skills/review", sb.source_spec());

    let first = sb.mind(&["learn", &url, "--pin"]);
    assert!(
        first.success,
        "first `learn <url> --pin` failed: {} {}",
        first.stdout, first.stderr
    );
    let combined = format!("{}{}", first.stdout, first.stderr);
    assert!(
        !combined.contains("already melded"),
        "a first-ever `learn <url> --pin` must not claim --pin was ignored \
         (it was honored at registration): {combined}"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the linked skill must still install on a pinned first learn"
    );
}

#[test]
fn learn_url_pin_after_alias_only_meld_registers_coexisting_bare_instance() {
    // spec: LNK-15 STO-58 — an aliased link instance
    // (`.../review#skills/review@myalias`) and the bare instance of the SAME
    // `<path>` (`.../review#skills/review`) are distinct, coexisting registry
    // entries (LNK-4, STO-58's per-instance model applied to links). So
    // `learn <url> --pin` after `meld <url> --as myalias` checks membership
    // under the BARE identity it is melding as (no alias here), finds it
    // unregistered, and registers a new, coexisting bare instance rather than
    // reusing or being blocked by the differently-aliased one. This is the
    // normal coexistence model, not a collision, so no CLI-203
    // "already melded" note is printed, and the aliased instance's pin and
    // items are left untouched. LNK-15 also claims the two instances'
    // LIFECYCLES are independent, not just that they coexist: below, after
    // both are registered, a `sync` proves pinning one (the bare instance,
    // frozen via `learn --pin`) does not affect the other's tracking (the
    // aliased instance, still following `main`).
    let sb = Sandbox::new("alias-coexist-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let url = format!("file://{}/tree/main/skills/review", sb.source_spec());

    // Meld under an identity alias only (no bare instance registered).
    let aliased = sb.mind(&["meld", &url, "--as", "myalias", "--yes"]);
    assert!(
        aliased.success,
        "aliased meld failed: {} {}",
        aliased.stdout, aliased.stderr
    );
    assert!(
        sb.claude_home.join("skills/myalias:review").exists(),
        "the aliased instance must install its item under the alias prefix"
    );

    // `learn <url> --pin`: the bare identity is not registered, so this
    // registers a NEW, coexisting bare instance and honors --pin on it.
    let learn = sb.mind(&["learn", &url, "--pin"]);
    assert!(
        learn.success,
        "`learn <url> --pin` failed: {} {}",
        learn.stdout, learn.stderr
    );
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        !combined.contains("already melded"),
        "registering a new coexisting bare instance must NOT print the \
         CLI-203 already-melded note (--pin was honored on the new \
         registration): {combined}"
    );

    // Both instances are now registered side by side.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("#skills/review@myalias"),
        "the aliased instance must remain registered: {sources}"
    );
    assert!(
        sources.contains("#skills/review")
            && !sources
                .lines()
                .filter(|l| l.contains("#skills/review") && !l.contains("@myalias"))
                .collect::<Vec<_>>()
                .is_empty(),
        "a distinct, coexisting bare instance must also be registered: {sources}"
    );

    // Both instances' items are installed independently; the aliased item is
    // untouched by the bare instance's registration.
    assert!(
        sb.claude_home.join("skills/myalias:review").exists(),
        "the aliased instance's item must remain installed and untouched"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the new bare instance's item must be installed"
    );

    // spec: LNK-15 -- coexistence is only half of LNK-15's claim; the other
    // half is that the two instances' LIFECYCLES are independent: pinning one
    // (the bare instance was frozen by `learn --pin`, CLI-200) must not affect
    // the other's tracking (the aliased instance still follows `main`, having
    // been melded with no `--pin`). Prove it observably: advance the source's
    // `main` branch, `sync`, and confirm the frozen bare instance's recorded
    // commit does NOT move while the still-following aliased instance's does.
    let sources_json = |sb: &Sandbox| -> serde_json::Value {
        let r = sb.mind(&["recall", "--sources", "--json"]);
        assert!(r.success, "recall --sources --json failed: {}", r.stderr);
        serde_json::from_str(&r.stdout).expect("recall --sources --json must be valid JSON")
    };
    let find_commit = |env: &serde_json::Value, name_contains: &str, exclude: &str| -> String {
        env["items"]
            .as_array()
            .expect("items array")
            .iter()
            .find(|s| {
                let n = s["name"].as_str().unwrap_or("");
                n.contains(name_contains) && (exclude.is_empty() || !n.contains(exclude))
            })
            .unwrap_or_else(|| panic!("no source with name containing {name_contains:?}"))["commit"]
            .as_str()
            .expect("commit must be a string")
            .to_string()
    };

    let before = sources_json(&sb);
    let bare_commit_before = find_commit(&before, "#skills/review", "@myalias");
    let aliased_commit_before = find_commit(&before, "#skills/review@myalias", "");

    // Advance the source repo past both instances' registration point.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill v2\n---\n# review\n",
    );

    let sync = sb.mind(&["sync"]);
    assert!(sync.success, "sync failed: {} {}", sync.stdout, sync.stderr);

    let after = sources_json(&sb);
    let bare_commit_after = find_commit(&after, "#skills/review", "@myalias");
    let aliased_commit_after = find_commit(&after, "#skills/review@myalias", "");

    assert_eq!(
        bare_commit_before, bare_commit_after,
        "the frozen (--pin) bare instance's commit must not move on sync, \
         regardless of the aliased instance's own sync"
    );
    assert_ne!(
        aliased_commit_before, aliased_commit_after,
        "the unpinned aliased instance must still advance to the new commit \
         on sync, unaffected by the other instance's pin"
    );
}

// ---------------------------------------------------------------------------
// STO-60: forking a second identity-aliased instance of an already-melded repo
// prints an explicit note that a new instance was registered.
// ---------------------------------------------------------------------------

#[test]
fn meld_fork_of_already_melded_repo_prints_new_instance_note() {
    // spec: STO-60 — a bare meld registers `local/<base>/<repo>`; a second meld
    // with `--as fork` registers the distinct instance `...@fork` (STO-58). The
    // second meld must print an explicit note that a new instance was registered
    // and the existing one remains, so the `@fork` suffix is not the only signal.
    let sb = Sandbox::new("fork-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let spec = sb.source_spec();

    // Bare meld: the base instance.
    let base = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        base.success,
        "bare meld failed: {} {}",
        base.stdout, base.stderr
    );
    assert!(
        !base.stdout.contains("registered a new instance"),
        "the first (bare) meld must NOT print the fork note: {}",
        base.stdout
    );

    // Fork: an identity-aliased instance of the same repo.
    let fork = sb.mind(&["meld", &spec, "--as", "fork", "--link-only"]);
    assert!(
        fork.success,
        "fork meld --as failed: {} {}",
        fork.stdout, fork.stderr
    );
    assert!(
        fork.stdout.contains("registered a new instance") && fork.stdout.contains("remains"),
        "forking a second aliased instance must print the new-instance note: {}",
        fork.stdout
    );
}

#[test]
fn meld_fork_note_suppressed_under_json() {
    // spec: STO-60 — the fork note is suppressed under --json.
    let sb = Sandbox::new("fork-json-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let spec = sb.source_spec();

    let base = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        base.success,
        "bare meld failed: {} {}",
        base.stdout, base.stderr
    );

    let fork = sb.mind(&["--json", "meld", &spec, "--as", "fork", "--link-only"]);
    assert!(
        fork.success,
        "fork meld --json failed: {} {}",
        fork.stdout, fork.stderr
    );
    assert!(
        !fork.stdout.contains("registered a new instance"),
        "the fork note must be suppressed under --json: {}",
        fork.stdout
    );
}

#[test]
fn meld_fork_third_instance_also_prints_new_instance_note() {
    // spec: STO-60 — the note must fire for EVERY new aliased instance beyond
    // the first, not just the second (guards against an off-by-one, e.g. a
    // guard that only checks "exactly one prior instance" instead of "at least
    // one"). base -> fork-one -> fork-two: both fork-one and fork-two must
    // print the note.
    let sb = Sandbox::new("fork-third-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    let spec = sb.source_spec();

    let base = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        base.success,
        "bare meld failed: {} {}",
        base.stdout, base.stderr
    );

    let fork1 = sb.mind(&["meld", &spec, "--as", "fork-one", "--link-only"]);
    assert!(
        fork1.success,
        "first fork meld failed: {} {}",
        fork1.stdout, fork1.stderr
    );
    assert!(
        fork1.stdout.contains("registered a new instance"),
        "the second instance overall (first fork) must print the note: {}",
        fork1.stdout
    );

    let fork2 = sb.mind(&["meld", &spec, "--as", "fork-two", "--link-only"]);
    assert!(
        fork2.success,
        "second fork meld failed: {} {}",
        fork2.stdout, fork2.stderr
    );
    // spec: STO-63 -- with two prior instances the note names both, so it reads
    // "instances <a>, <b> remain" rather than the singular "<a> remains", and
    // the names it prints are the registered identities, which are the handles
    // `unmeld` accepts. Naming the bare `host/owner/repo` would point at a name
    // that need not be registered at all.
    assert!(
        fork2.stdout.contains("registered a new instance") && fork2.stdout.contains("remain"),
        "a THIRD instance (second fork, with two prior instances already \
         registered) must also print the note: {}",
        fork2.stdout
    );
    assert!(
        fork2.stdout.contains("fork-one"),
        "the note must name the actual registered instances, which are the \
         handles `unmeld` accepts: {}",
        fork2.stdout
    );
}

#[test]
fn learn_url_second_item_link_instance_without_alias_does_not_print_fork_note() {
    // spec: STO-60 — the note is scoped to a NEW *aliased* instance. Two
    // different item-link instances of the SAME repo (different `#path`, both
    // un-aliased) share `base_identity()`, but neither carries an `as_alias`,
    // so registering the second must NOT print the fork note even though the
    // registry already holds a same-base entry.
    let sb = Sandbox::new("link-no-alias-src");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill\n---\n# review\n",
    );
    sb.write_and_commit(
        "skills/dev/SKILL.md",
        "---\nname: dev\ndescription: Dev skill\n---\n# dev\n",
    );
    let url_a = format!("file://{}/tree/main/skills/review", sb.source_spec());
    let url_b = format!("file://{}/tree/main/skills/dev", sb.source_spec());

    let first = sb.mind(&["learn", &url_a]);
    assert!(
        first.success,
        "first item-link learn failed: {} {}",
        first.stdout, first.stderr
    );
    assert!(
        !first.stdout.contains("registered a new instance"),
        "the first item-link instance must not print the fork note: {}",
        first.stdout
    );

    // A second, un-aliased item-link instance of the SAME base repo (different
    // #path): base_identity() matches the first, but as_alias is None on both,
    // so no fork note.
    let second = sb.mind(&["learn", &url_b]);
    assert!(
        second.success,
        "second item-link learn failed: {} {}",
        second.stdout, second.stderr
    );
    assert!(
        !second.stdout.contains("registered a new instance"),
        "a second un-aliased item-link instance of the same repo must NOT \
         print the fork note (it has no @alias): {}",
        second.stdout
    );
}
