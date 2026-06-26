//! Integration tests for the recall/probe outdated marker's hash-error
//! handling (CLI-75) and the `--source` / `unmeld` glob-selector validation
//! (CLI-28, CLI-86).
//!
//! Each test drives the real `mind` binary against a hermetic fixture source
//! (a local git repo, no network), using isolated MIND_HOME / CLAUDE_HOME temp
//! dirs, exactly as tests/cli.rs does. This file is its own crate, so it carries
//! a minimal copy of that harness.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Minimal fixture harness (mirrors tests/cli.rs)
// ---------------------------------------------------------------------------

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
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-rhash-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
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

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// Git-init the source repo so `meld` can clone it.
    fn git_init(&self) {
        git(
            &self.source,
            &["-c", "init.defaultBranch=main", "init", "-q"],
        );
        git(&self.source, &["config", "user.email", "t@t"]);
        git(&self.source, &["config", "user.name", "t"]);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "initial"]);
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
// CLI-75: a hash-computation error makes the outdated marker FLAG (not hide).
// ---------------------------------------------------------------------------

/// After learning a skill, make its source content impossible to hash (a
/// dangling symlink inside the skill dir makes `hash_path`'s file read fail),
/// then assert `recall` marks the item outdated rather than silently up to date.
///
/// This is the discrepancy CLI-75 calls out: `upgrade` aborts via `?` on a hash
/// error, but the best-effort listing marker cannot abort the whole listing, so
/// it errs toward flagging. The pre-fix code used `.ok().is_some_and(..)`, which
/// read a hash error as "up to date" and hid the drift.
/// spec: CLI-75
#[test]
fn recall_flags_outdated_when_source_content_cannot_be_hashed() {
    let sb = Sandbox::new("agents");
    let skill_dir = sb.source.join("skills/review");
    write(
        &skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: review the diff\n---\n# review\n",
    );
    sb.git_init();

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");
    let learned = sb.mind(&["learn", "review"]);
    assert!(learned.success, "learn failed: {}", learned.stderr);

    // Sanity: a freshly learned item is NOT outdated.
    let before = sb.mind(&["recall"]);
    assert!(before.success, "recall failed: {}", before.stderr);
    assert!(
        before.stdout.contains("review"),
        "recall should list the learned skill: {}",
        before.stdout
    );
    assert!(
        !before.stdout.contains("outdated"),
        "a freshly learned item must not be marked outdated: {}",
        before.stdout
    );

    // Make the skill's source content unhashable: a dangling symlink inside the
    // skill dir is treated as a file by hash_path, so the read fails with ENOENT.
    // The catalog still lists the skill (its SKILL.md is intact), so the marker
    // path runs and must now flag the item.
    let dangling = skill_dir.join("broken-link");
    std::os::unix::fs::symlink(skill_dir.join("does-not-exist"), &dangling)
        .expect("create dangling symlink");

    let after = sb.mind(&["recall"]);
    assert!(
        after.success,
        "recall must not abort the listing on a hash error: stdout={} stderr={}",
        after.stdout, after.stderr
    );
    assert!(
        after.stdout.contains("review"),
        "the item must still be listed (best-effort marker, no abort): {}",
        after.stdout
    );
    assert!(
        after.stdout.contains("outdated"),
        "a hash-computation error must count as drift and flag the item: {}",
        after.stdout
    );
}

/// Shared setup for the hash-error marker tests: meld a one-skill source, learn
/// the skill, then plant a dangling symlink inside the source skill dir so its
/// content can no longer be hashed (the broken link is read as a file and fails
/// with ENOENT). The catalog still lists the skill because SKILL.md is intact, so
/// every outdated-marker site runs its hash check against an Err.
///
/// Returns the sandbox with the item already learned and the source poisoned.
fn sandbox_with_unhashable_learned_skill() -> Sandbox {
    let sb = Sandbox::new("agents");
    let skill_dir = sb.source.join("skills/review");
    write(
        &skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: review the diff\n---\n# review\n",
    );
    sb.git_init();

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");
    let learned = sb.mind(&["learn", "review"]);
    assert!(learned.success, "learn failed: {}", learned.stderr);

    // Poison the source content: a dangling symlink inside the skill dir makes
    // hash_path's file read fail. SKILL.md remains, so the item still scans.
    let dangling = skill_dir.join("broken-link");
    std::os::unix::fs::symlink(skill_dir.join("does-not-exist"), &dangling)
        .expect("create dangling symlink");
    sb
}

// ---------------------------------------------------------------------------
// CLI-75: the OTHER marker surfaces (single-item recall, item-detail, probe,
// per-source listing via re-meld) must also flag a hash error as drift, not hide
// it. The existing test above only exercises the plain `recall` listing site.
// ---------------------------------------------------------------------------

/// `recall <item>` single-item lookup (the item-detail / status site). A hash
/// error must print the `status` line marking the item out of date. This site
/// uses the phrase "out of date" (not "outdated"), so we assert the exact phrase
/// to be sure we are reading the marker and not some incidental text.
/// spec: CLI-75
#[test]
fn recall_single_item_flags_outdated_when_source_unhashable() {
    let sb = sandbox_with_unhashable_learned_skill();

    // Sanity contrast: resolve the same item before poisoning would NOT be out of
    // date. We re-derive that by checking a fresh, unpoisoned sibling sandbox.
    let clean = Sandbox::new("agents");
    let clean_dir = clean.source.join("skills/review");
    write(
        &clean_dir.join("SKILL.md"),
        "---\nname: review\ndescription: review the diff\n---\n# review\n",
    );
    clean.git_init();
    let clean_spec = clean.source_spec();
    assert!(clean.mind(&["meld", &clean_spec]).success, "meld failed");
    assert!(clean.mind(&["learn", "review"]).success, "learn failed");
    let clean_detail = clean.mind(&["recall", "skill:review"]);
    assert!(clean_detail.success, "clean recall failed");
    assert!(
        clean_detail.stdout.contains("review"),
        "clean detail should show the item: {}",
        clean_detail.stdout
    );
    assert!(
        !clean_detail.stdout.contains("out of date"),
        "a freshly learned item must not be out of date: {}",
        clean_detail.stdout
    );

    // The poisoned source: single-item detail must flag it.
    let r = sb.mind(&["recall", "skill:review"]);
    assert!(
        r.success,
        "single-item recall must not abort on a hash error: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("review"),
        "the item detail must still print: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("out of date"),
        "a hash error must mark the single-item detail out of date: {}",
        r.stdout
    );
}

/// `probe <query>` row listing (the probe marker site). A hash error must append
/// the outdated token to the matching row, and probe must not abort.
/// spec: CLI-75
#[test]
fn probe_flags_outdated_when_source_unhashable() {
    // Clean contrast: a freshly learned, hashable item probes as NOT outdated.
    // This proves the assertion below discriminates outdated from up to date and
    // is not satisfied by some unrelated occurrence of the marker text.
    let clean = Sandbox::new("agents");
    write(
        &clean.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review the diff\n---\n# review\n",
    );
    clean.git_init();
    let clean_spec = clean.source_spec();
    assert!(clean.mind(&["meld", &clean_spec]).success, "meld failed");
    assert!(clean.mind(&["learn", "review"]).success, "learn failed");
    let clean_probe = clean.mind(&["probe", "review"]);
    assert!(clean_probe.success, "clean probe failed");
    assert!(
        clean_probe.stdout.contains("review") && !clean_probe.stdout.contains("outdated"),
        "a freshly learned item must probe as not outdated: {}",
        clean_probe.stdout
    );

    let sb = sandbox_with_unhashable_learned_skill();

    let r = sb.mind(&["probe", "review"]);
    assert!(
        r.success,
        "probe must not abort on a hash error: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("review"),
        "probe must still list the matching item: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("outdated"),
        "a hash error must flag the probe row outdated: {}",
        r.stdout
    );
}

/// The per-source listing site is `source_status`, reached by re-running `meld`
/// on an already-melded source (it prints each item with its install state). A
/// hash error must flag the item outdated there too, and the re-meld must not
/// abort.
/// spec: CLI-75
#[test]
fn remeld_source_status_flags_outdated_when_source_unhashable() {
    // Clean contrast: re-melding a hashable source shows the item NOT outdated, so
    // the assertion below truly discriminates rather than always passing.
    let clean = Sandbox::new("agents");
    write(
        &clean.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review the diff\n---\n# review\n",
    );
    clean.git_init();
    let clean_spec = clean.source_spec();
    assert!(clean.mind(&["meld", &clean_spec]).success, "meld failed");
    assert!(clean.mind(&["learn", "review"]).success, "learn failed");
    let clean_remeld = clean.mind(&["meld", &clean_spec]);
    assert!(clean_remeld.success, "clean re-meld failed");
    assert!(
        clean_remeld.stdout.contains("review") && !clean_remeld.stdout.contains("outdated"),
        "a freshly learned item must re-meld as not outdated: {}",
        clean_remeld.stdout
    );

    let sb = sandbox_with_unhashable_learned_skill();

    // Re-melding the already-melded source falls through to source_status.
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "re-meld status view must not abort on a hash error: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("review"),
        "the source status view must still list the item: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("outdated"),
        "a hash error must flag the source-status row outdated: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// CLI-28 / CLI-86: a malformed glob selector reports InvalidPattern, not
// SourceNotFound.
// ---------------------------------------------------------------------------

/// `unmeld '[bad'` carries glob metacharacters but is not a valid pattern. It
/// must report a clear invalid-pattern error rather than silently matching
/// nothing and surfacing as `SourceNotFound`.
/// spec: CLI-28
#[test]
fn unmeld_malformed_glob_reports_invalid_pattern_not_source_not_found() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );
    sb.git_init();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");

    let r = sb.mind(&["unmeld", "[bad", "--yes"]);
    assert!(!r.success, "malformed glob must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("not a valid glob selector"),
        "expected invalid-pattern error, got stderr={} stdout={}",
        r.stderr,
        r.stdout
    );
    assert!(
        !r.stderr.contains("no source named"),
        "malformed glob must NOT surface as SourceNotFound: {}",
        r.stderr
    );
}

/// The `recall --source '[bad'` filter shares the glob matcher, so a malformed
/// pattern must likewise report InvalidPattern rather than silently matching no
/// sources (which would read as an empty/uninformative listing).
/// spec: CLI-86
#[test]
fn recall_source_filter_malformed_glob_reports_invalid_pattern() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );
    sb.git_init();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");

    let r = sb.mind(&["recall", "--source", "[bad"]);
    assert!(
        !r.success,
        "malformed --source glob must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not a valid glob selector"),
        "expected invalid-pattern error, got stderr={} stdout={}",
        r.stderr,
        r.stdout
    );
}

/// The `probe --source '[bad'` filter shares the same matcher and must report
/// InvalidPattern as well.
/// spec: CLI-86
#[test]
fn probe_source_filter_malformed_glob_reports_invalid_pattern() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );
    sb.git_init();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");

    let r = sb.mind(&["probe", "--source", "[bad"]);
    assert!(
        !r.success,
        "malformed --source glob must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not a valid glob selector"),
        "expected invalid-pattern error, got stderr={} stdout={}",
        r.stderr,
        r.stdout
    );
}

/// A VALID glob selector is unaffected by the new validation step: `unmeld '*'`
/// still matches and removes the source.
/// spec: CLI-28
#[test]
fn unmeld_valid_glob_still_matches() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );
    sb.git_init();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld failed");

    let r = sb.mind(&["unmeld", "*", "--yes"]);
    assert!(
        r.success,
        "a valid glob must still match and unmeld: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}
