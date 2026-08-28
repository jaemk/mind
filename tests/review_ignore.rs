//! Integration tests for `mind review` respecting an item's ignore set
//! (spec/ignore.md, IGN-11): a file the ignore set excludes must be invisible
//! to `review`'s findings, not just to the install copy.
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo, with `MIND_HOME`/`CLAUDE_HOME` pointed at temp dirs. No network.
//! Fixture style mirrors tests/cli_ignore.rs and tests/review_hooks.rs.

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
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-rvi-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("lib");
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

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
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

    /// Write a file and commit it (tracked).
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    /// Write a file WITHOUT committing it (stays untracked).
    fn write_untracked(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
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
// M3: Check 5 (`{{ns:}}` resolution) must not see an ignored file's content.
// ---------------------------------------------------------------------------

/// An ignored file naming a nonexistent sibling via `{{ns:}}` must not surface
/// as a `bad-reference` finding: the ignore set excludes the file, so it is
/// not part of the item `review` inspects.
/// spec: IGN-11, CLI-132
#[test]
fn an_ignored_file_with_a_bad_ns_reference_is_not_reported() {
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit(
        "skills/a/scratch/draft.md",
        "handoff to {{ns:nonexistent}}\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");

    let r = sb.mind(&["review", &sb.spec()]);
    assert!(
        r.success,
        "an ignored file's bad reference must not be a hard finding: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("bad-reference"),
        "the ignored file must be invisible to the reference scan: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// M3: Check 12 (`duplicate-tooling`) must not see an ignored file's content.
// ---------------------------------------------------------------------------

/// A byte-identical helper shared across two items must not surface as a
/// `duplicate-tooling` advisory when the shared file is ignored in both.
/// spec: IGN-11, CLI-144
#[test]
fn an_ignored_duplicate_helper_is_not_reported() {
    let sb = Sandbox::new();
    sb.write_and_commit("skills/one/SKILL.md", "---\ndescription: 1\n---\n# 1\n");
    sb.write_and_commit("skills/two/SKILL.md", "---\ndescription: 2\n---\n# 2\n");
    sb.write_and_commit("skills/one/helper.py", "print('shared')\n");
    sb.write_and_commit("skills/two/helper.py", "print('shared')\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"helper.py\"]\n");

    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory-only: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("duplicate-tooling"),
        "an ignored shared helper must not be reported as duplicate tooling: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// M6: Check 13 (tool `bin`/`TOOL.md` unshipped-tooling, CLI-190) must not see
// an ignored bin target.
// ---------------------------------------------------------------------------

/// A tool's `bin` file that is both untracked AND ignored must not surface as
/// `unshipped-tooling`: the ignore set removes it from what mind installs, so
/// its untracked status is no longer a shipping defect worth flagging.
/// spec: IGN-11, CLI-190
#[test]
fn an_ignored_untracked_bin_is_not_reported_as_unshipped() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "tools/detect/TOOL.md",
        "---\ndescription: detect\nbin: detect.sh\n---\n# detect\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"detect.sh\"]\n");
    // detect.sh is written LAST, after every commit, so nothing subsequently
    // stages it: it stays untracked AND ignored. Without the IGN-11 filter,
    // Check 13 would flag it as unshipped-tooling purely because it is
    // untracked.
    sb.write_untracked("tools/detect/detect.sh", "#!/bin/sh\necho hi\n");

    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory-only: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("unshipped-tooling"),
        "an ignored, untracked bin file must not be reported: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// M6: Check 14 (`{{self}}`/`{{path:}}` unshipped-tooling, CLI-191) must not
// see an ignored reference target.
// ---------------------------------------------------------------------------

/// A `{{self}}`-addressed file that is both untracked AND ignored must not
/// surface as `unshipped-tooling`.
/// spec: IGN-11, CLI-191
#[test]
fn an_ignored_untracked_self_target_is_not_reported_as_unshipped() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/one/SKILL.md",
        "---\ndescription: one\n---\nrun {{self}}/helper.py here\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"helper.py\"]\n");
    // helper.py is written LAST, after every commit, so nothing subsequently
    // stages it: it stays untracked AND ignored. Without the IGN-11 filter,
    // Check 14 would flag it purely because it resolves locally and is
    // untracked.
    sb.write_untracked("skills/one/helper.py", "print('hi')\n");

    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory-only: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("unshipped-tooling"),
        "an ignored, untracked {{{{self}}}} target must not be reported: {}",
        r.stdout
    );
}

/// A `{{path:name}}`-addressed file must be filtered against the ignore set of
/// the item that OWNS the target directory, not the referencing item's ignore
/// set. Item `one` (the referencer) declares no ignore of its own; the target
/// file lives under sibling item `b`, which ignores it. If Check 14 filtered
/// using the referencing item's ignore set instead of the owner's, this file
/// would still be reported.
/// spec: IGN-11, CLI-191
#[test]
fn an_ignored_untracked_path_target_uses_the_owning_items_ignore_set() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/one/SKILL.md",
        "---\ndescription: one\n---\nsee {{path:b}}/helper.py for details\n",
    );
    sb.write_and_commit("skills/b/SKILL.md", "---\ndescription: b\n---\n# b\n");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"one\"\npath = \"skills/one\"\n\
         [[items]]\nkind = \"skill\"\nname = \"b\"\npath = \"skills/b\"\n\
         ignore = [\"helper.py\"]\n",
    );
    // helper.py lives under sibling `b`, is written LAST (after every commit,
    // so nothing subsequently stages it), and is ignored ONLY by `b`'s own
    // [[items]] declaration -- `one` has no ignore at all.
    sb.write_untracked("skills/b/helper.py", "print('hi')\n");

    let r = sb.mind(&["review", &sb.spec()]);
    assert!(r.success, "advisory-only: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("unshipped-tooling"),
        "the owning item's ignore set must govern the {{{{path:}}}} target, \
         not the referencing item's: {}",
        r.stdout
    );
}
