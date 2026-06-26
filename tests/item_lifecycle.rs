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
    // The advisory names the referencing item and the sibling it mentions.
    assert!(
        combined.contains("review"),
        "the advisory should name the mentioned sibling: {combined}"
    );
}
