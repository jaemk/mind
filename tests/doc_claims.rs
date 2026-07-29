//! Integration tests that turn prose claims in `docs/` and `CHANGELOG.md` into
//! enforced behavior.
//!
//! The documentation for a change is written by a different pass than the
//! implementation, so a doc sentence can describe behavior that never landed,
//! or overstate behavior that landed with a caveat. Every claim that is
//! observable from the CLI belongs in a test; this file holds the ones with no
//! other home.
//!
//! Hermetic: local git repos in temp dirs, `MIND_HOME`/`CLAUDE_HOME` pointed at
//! temp dirs, and an executable stub earlier on `PATH` wherever a regression
//! could otherwise reach the network.
//!
//! Claims covered:
//!   - docs/src/authoring.md + docs/src/commands.md: `review` reads a target
//!     naming an existing directory as that local path (CLI-214) -- including
//!     the two-segment relative form the docs single out, and the registry
//!     precedence the docs' "always" wording omits.
//!   - docs/src/troubleshooting.md: the dead-local-source recovery recipe
//!     (`mind recall --sources`, then `mind unmeld <name>`) actually works
//!     end to end after the directory is gone (CLI-213).
//!   - docs/src/configuration.md: `config lobes add` backfills already-installed
//!     items into a new lobe only for the kinds the lobe admits (HARN-17), not
//!     every installed item.
//!   - docs/src/configuration.md: `config lobes add` creates its target
//!     directory immediately only on the managed (non-`--snapshot`) path
//!     (HARN-15).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    fn new(tag: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-doc-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Sandbox {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    /// Run `mind` from `cwd`, optionally with `path_prefix` prepended to `PATH`
    /// (used to shadow `git` so a regression cannot reach a real remote).
    fn mind_in(&self, cwd: &Path, path_prefix: Option<&Path>, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .current_dir(cwd)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if let Some(prefix) = path_prefix {
            let orig = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{}:{orig}", prefix.display()));
        }
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_in(&self.base, None, args)
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

fn write_exec(path: &Path, contents: &str) {
    write(path, contents);
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
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

fn init_repo(dir: &Path) {
    write(&dir.join("README.md"), "# fixture\n");
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "initial"]);
}

/// A `git` stub that records every invocation and always fails, so a
/// regression that took the remote-clone branch is a recorded local failure
/// rather than a live network call.
fn stub_git(bin_dir: &Path, log: &Path) -> PathBuf {
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log:?}\nexit 1\n",
        log = log
    );
    write_exec(&bin_dir.join("git"), &script);
    bin_dir.to_path_buf()
}

// ---------------------------------------------------------------------------
// docs/src/authoring.md:75-78 and docs/src/commands.md:112 --
// "A target naming an existing directory is always read as that local path,
// even when it also looks like a valid `owner/repo` spec".
// ---------------------------------------------------------------------------

/// The motivating case CLI-214 names: a two-segment RELATIVE path, which
/// `parse_spec` would otherwise read as `owner/repo` and shallow-clone from
/// github.com. `review` must read it as the local directory, print the note
/// naming the reading it took, and make no clone attempt at all.
///
/// The existing unit test for this (`src/review.rs`
/// `resolve_target_prefers_an_existing_absolute_directory`) passes an
/// ABSOLUTE path, which `parse_spec` already resolves locally; it therefore
/// never exercises the branch that fixes the reported bug.
#[test]
fn review_reads_a_two_segment_relative_path_as_a_local_dir_without_cloning() {
    // spec: CLI-214
    let sb = Sandbox::new("rel-review");
    let work = sb.base.join("work");
    let bin = sb.base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let git_log = sb.base.join("git.log");
    let path_prefix = stub_git(&bin, &git_log);

    // `<work>/skills/greet` is a source directory in its own right, carrying a
    // malformed mind.toml. That is a HARD review finding, so the exit status
    // itself proves these bytes -- and not a clone of github.com/skills/greet
    // -- are what got read.
    write(
        &work.join("skills/greet/mind.toml"),
        "this is not = valid = toml\n",
    );

    let r = sb.mind_in(&work, Some(&path_prefix), &["review", "skills/greet"]);
    assert!(
        !git_log.exists(),
        "review must not invoke git at all for a local directory target; \
         recorded invocations: {:?}",
        std::fs::read_to_string(&git_log).unwrap_or_default()
    );
    assert!(
        !r.success,
        "the local directory's malformed mind.toml is a hard finding, so this \
         must exit non-zero -- proving the local bytes were read: {}\n{}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind.toml") || combined.to_lowercase().contains("toml"),
        "the failure must be the local mind.toml parse, not a clone failure: {combined}"
    );
    // CLI-214: the note names the reading taken and the escape to force the
    // remote one.
    assert!(
        r.stderr.contains("skills/greet") && r.stderr.contains("mind review 'github:skills/greet'"),
        "the shadow note must name the target and the remote-forcing escape: {}",
        r.stderr
    );
}

/// The clean-directory counterpart: a valid local source at a two-segment
/// relative path reviews cleanly and still makes no clone attempt, so the
/// CLI-214 branch is not merely turning a clone failure into a different
/// failure.
#[test]
fn review_relative_path_to_a_clean_local_source_exits_zero_without_cloning() {
    // spec: CLI-214
    let sb = Sandbox::new("rel-clean");
    let work = sb.base.join("work");
    let bin = sb.base.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let git_log = sb.base.join("git.log");
    let path_prefix = stub_git(&bin, &git_log);

    write(
        &work.join("skills/greet/skills/hello/SKILL.md"),
        "---\ndescription: say hello\n---\n# hello\n",
    );

    let r = sb.mind_in(&work, Some(&path_prefix), &["review", "skills/greet"]);
    assert!(
        r.success,
        "a clean local source at a two-segment relative path must review \
         cleanly: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !git_log.exists(),
        "no clone may be attempted: {:?}",
        std::fs::read_to_string(&git_log).unwrap_or_default()
    );
}

/// DOCUMENTATION INACCURACY, pinned as behavior: the docs say a target naming
/// an existing directory is read as that local path "always". It is not. Per
/// CLI-214's own wording, the check sits BETWEEN the registry-selector match
/// and repo-spec parsing, so a registered source whose identity the target
/// matches (by exact or suffix match) wins over an identically named directory
/// in the cwd. `src/review.rs:165` states the real precedence:
/// "exact/suffix registry match > existing local directory > remote spec".
///
/// This test pins that precedence so the behavior cannot drift, and stands as
/// the counterexample to the word "always" in docs/src/authoring.md:76 and
/// docs/src/commands.md:112.
#[test]
fn review_registry_selector_wins_over_an_identically_named_local_directory() {
    // spec: CLI-214
    let sb = Sandbox::new("registry-wins");

    // The melded source is named `greet` and is CLEAN (reviews exit 0).
    let melded = sb.base.join("greet");
    write(
        &melded.join("skills/hello/SKILL.md"),
        "---\ndescription: say hello\n---\n# hello\n",
    );
    init_repo(&melded);
    let r = sb.mind(&["meld", melded.to_string_lossy().as_ref(), "--register-only"]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    // A DIFFERENT directory, also named `greet`, in the working directory,
    // carrying a malformed mind.toml (a hard finding: non-zero exit).
    let work = sb.base.join("work");
    write(
        &work.join("greet/mind.toml"),
        "this is not = valid = toml\n",
    );

    let r = sb.mind_in(&work, None, &["review", "greet"]);
    assert!(
        r.success,
        "the registry match wins, so the CLEAN melded source is what gets \
         reviewed -- if the local ./greet had won, its malformed mind.toml \
         would be a hard finding and this would exit non-zero: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no issues found"),
        "the clean melded source is what was reviewed: {}",
        r.stdout
    );
}

/// The complement: with NO registered source to match, the same string does
/// resolve to the local directory, so the registry precedence above is the
/// only thing that displaces it.
#[test]
fn review_local_directory_wins_when_no_source_is_registered() {
    // spec: CLI-214
    let sb = Sandbox::new("local-wins");
    let work = sb.base.join("work");
    write(
        &work.join("greet/mind.toml"),
        "this is not = valid = toml\n",
    );

    let r = sb.mind_in(&work, None, &["review", "greet"]);
    assert!(
        !r.success,
        "with nothing registered, ./greet is what is reviewed, and its \
         malformed mind.toml is a hard finding: {}\n{}",
        r.stdout, r.stderr
    );
}

// ---------------------------------------------------------------------------
// docs/src/troubleshooting.md:84-88 -- the dead-local-source recovery recipe.
// ---------------------------------------------------------------------------

/// The documented recovery steps, executed verbatim: after the melded
/// directory is deleted, `mind recall --sources` must still run and must print
/// a name, and `mind unmeld <that name>` must actually drop the source. The
/// existing CLI-213 coverage stops at "recall degrades"; the second half of
/// the recipe -- the step that makes the situation recoverable -- was never
/// exercised.
#[test]
fn dead_local_source_recovery_recipe_from_the_docs_works_end_to_end() {
    // spec: CLI-213
    let sb = Sandbox::new("dead-source");
    let source = sb.base.join("greet");
    write(
        &source.join("skills/hello/SKILL.md"),
        "---\ndescription: say hello\n---\n# hello\n",
    );
    init_repo(&source);
    let r = sb.mind(&["meld", source.to_string_lossy().as_ref(), "--register-only"]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    let identity = format!(
        "local/{}/greet",
        sb.base.file_name().unwrap().to_string_lossy()
    );

    // The directory goes away.
    std::fs::remove_dir_all(&source).unwrap();

    // Step 1: `mind recall --sources` to find the source's name.
    let r = sb.mind(&["recall", "--sources"]);
    assert!(
        r.success,
        "recall --sources must still run with the directory gone: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains(&identity),
        "the recipe says to read the source's name here, so the full identity \
         `unmeld` accepts must be printed: {}",
        r.stdout
    );

    // Step 2: `mind unmeld <name>` to drop it.
    let r = sb.mind(&["unmeld", &identity]);
    assert!(
        r.success,
        "unmeld must drop a source whose linked working tree is gone -- this \
         is the documented remedy, and it is the only way out: {}\n{}",
        r.stdout, r.stderr
    );

    // The source is gone, and the warning with it.
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.success, "recall after unmeld: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains(&identity),
        "the dropped source must no longer be listed: {}",
        r.stdout
    );
    let r = sb.mind(&["recall"]);
    assert!(
        r.success && !r.stderr.contains("is gone"),
        "the vanished-source warning must be gone once it is unmelded: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// The other half of the recipe's premise: `mind unmeld` of the vanished
/// source must not be blocked by the same scan failure that degrades the
/// listing verbs. If `unmeld` itself hard-failed on the gone directory, the
/// documented recovery would be a dead end.
#[test]
fn unmeld_of_a_vanished_source_does_not_require_the_directory_to_exist() {
    // spec: CLI-213
    let sb = Sandbox::new("unmeld-gone");
    let source = sb.base.join("ghost");
    write(
        &source.join("skills/hello/SKILL.md"),
        "---\ndescription: say hello\n---\n# hello\n",
    );
    init_repo(&source);
    assert!(
        sb.mind(&["meld", source.to_string_lossy().as_ref(), "--register-only"])
            .success
    );
    std::fs::remove_dir_all(&source).unwrap();

    // A bare suffix selector (what a user would actually type) must work too,
    // not only the full identity.
    let r = sb.mind(&["unmeld", "ghost"]);
    assert!(
        r.success,
        "unmeld by the short selector must work on a vanished source: {}\n{}",
        r.stdout, r.stderr
    );
}

// ---------------------------------------------------------------------------
// docs/src/configuration.md:88-89 -- "Already-installed items link into the
// new lobe as part of the same command" omits that the lobe's `kinds` filter
// still applies (HARN-17): a skill-only lobe backfills no rule.
// ---------------------------------------------------------------------------

/// A skill and a rule are installed BEFORE the lobe exists. Adding a
/// `windsurf`-preset (skill-only) lobe afterward must backfill the skill but
/// not the rule, proving the doc's unqualified "already-installed items link
/// in" statement is only true for the kinds the lobe admits.
#[test]
fn config_lobes_add_backfills_only_the_kinds_the_lobe_admits() {
    // spec: HARN-17
    let sb = Sandbox::new("kinds-backfill");
    let source = sb.base.join("agents");
    write(
        &source.join("skills/hello/SKILL.md"),
        "---\ndescription: say hello\n---\n# hello\n",
    );
    write(
        &source.join("rules/style-rule.md"),
        "---\ndescription: ASCII only\n---\n# style\n",
    );
    init_repo(&source);

    assert!(
        sb.mind(&["meld", source.to_string_lossy().as_ref(), "--register-only"])
            .success
    );
    assert!(sb.mind(&["learn", "hello"]).success, "learn skill");
    assert!(sb.mind(&["learn", "style-rule"]).success, "learn rule");

    let project = sb.base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let r = sb.mind(&[
        "config",
        "lobes",
        "add",
        project.to_string_lossy().as_ref(),
        "--preset",
        "windsurf",
    ]);
    assert!(r.success, "config lobes add: {}\n{}", r.stdout, r.stderr);

    let windsurf = project.join(".windsurf");
    assert!(
        std::fs::symlink_metadata(windsurf.join("skills/hello")).is_ok(),
        "the skill is a kind the windsurf lobe admits, so it must be backfilled: {}",
        r.stdout
    );
    assert!(
        std::fs::symlink_metadata(windsurf.join("rules/style-rule.md")).is_err(),
        "the windsurf preset is skill-only, so the already-installed rule must \
         NOT be backfilled into it: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// docs/src/configuration.md:86 -- "Adding a lobe creates its target directory
// immediately" omits that this is true only on the managed (non-`--snapshot`)
// path (HARN-15).
// ---------------------------------------------------------------------------

/// A plain (managed) `config lobes add` creates the target directory
/// immediately; the same command with `--snapshot` on an empty install set
/// creates nothing, so the doc's unqualified claim only holds for the managed
/// path.
#[test]
fn config_lobes_add_creates_target_dir_only_on_the_managed_path() {
    // spec: HARN-15
    let sb = Sandbox::new("mkdir-managed");

    let managed = sb.base.join("managed-target");
    assert!(!managed.exists(), "precondition: target must not pre-exist");
    let r = sb.mind(&["config", "lobes", "add", managed.to_string_lossy().as_ref()]);
    assert!(r.success, "config lobes add: {}\n{}", r.stdout, r.stderr);
    assert!(
        std::fs::symlink_metadata(&managed).is_ok(),
        "a managed lobe add must create its target directory immediately: {}",
        r.stdout
    );

    let snap = sb.base.join("snapshot-target");
    assert!(!snap.exists(), "precondition: target must not pre-exist");
    let r = sb.mind(&[
        "config",
        "lobes",
        "add",
        snap.to_string_lossy().as_ref(),
        "--snapshot",
    ]);
    assert!(r.success, "snapshot add: {}\n{}", r.stdout, r.stderr);
    assert!(
        std::fs::symlink_metadata(&snap).is_err(),
        "--snapshot must NOT create the target directory (HARN-15 is confined \
         to the managed path): {}",
        r.stdout
    );
}
