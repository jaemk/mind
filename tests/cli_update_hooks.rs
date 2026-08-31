//! Integration tests for update hooks and for an item's own hook declaration
//! sites.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture (local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   HOOK-121: a source's update hooks run at `upgrade`, never at `meld`, and
//!             are baselined at the meld commit so an unmoved source runs none
//!   HOOK-122: update hooks supersede install hooks on an update; a source that
//!             declares none re-runs its install hooks unchanged
//!   HOOK-124: an update hook's run is recorded under its own event, so it is
//!             not re-offered at the same commit and does not alias the install
//!             hook's record
//!   HOOK-55:  a recorded install hook the source no longer declares is not
//!             replayed from local state
//!   HOOK-56:  a consumer `--install-hook` override covers the update event too
//!   HOOK-125: an item's update hooks replace its install hooks on a re-install
//!   HOOK-130: `install:`/`update:`/`uninstall:` frontmatter on any kind
//!   HOOK-131: a scoped `mind.toml` in a skill's own directory
//!   HOOK-133: an item manifest is item content: copied into the store, hashed

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
        let base = std::env::temp_dir().join(format!("mind-uh-{}-{n}", std::process::id()));
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

    /// The registry identity a local-path meld gives this fixture:
    /// `local/<parent dir>/<repo dir>` (STO-13).
    fn source_id(&self) -> String {
        let owner = self
            .base
            .file_name()
            .expect("base has a name")
            .to_string_lossy();
        let repo = self
            .source
            .file_name()
            .expect("source has a name")
            .to_string_lossy();
        format!("local/{owner}/{repo}")
    }

    /// A shell command appending one line to an absolute sandbox path, so a
    /// test can count how many times a hook ran (not just whether it ran).
    fn tally(&self, name: &str) -> String {
        format!("echo ran >> {}", self.base.join(name).display())
    }

    /// How many times the hook writing `name` has run.
    fn runs(&self, name: &str) -> usize {
        std::fs::read_to_string(self.base.join(name))
            .map(|s| s.lines().count())
            .unwrap_or(0)
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
// Source update hooks (HOOK-121, HOOK-122, HOOK-124)
// ---------------------------------------------------------------------------

/// A source that declares an update hook runs it at `upgrade` INSTEAD of
/// re-running its install hook, and does not run it at `meld`.
#[test]
fn source_update_hook_replaces_the_install_rerun_at_upgrade() {
    // spec: HOOK-121 HOOK-122
    let sb = Sandbox::new("update-src");
    let install = sb.tally("install.log");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[[hooks]]\nrun = \"{install}\"\n\n[[hooks]]\nrun = \"{update}\"\nevent = \"update\"\n"
        ),
    );

    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(sb.runs("install.log"), 1, "meld runs the install hook");
    assert_eq!(
        sb.runs("update.log"),
        0,
        "an update hook must not run at meld: {}",
        r.stdout
    );

    // The source advances.
    sb.write_and_commit("README.md", "# fixture v2\n");
    assert!(sb.mind(&["sync"]).success, "sync must succeed");

    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        1,
        "the update hook must run at upgrade: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("install.log"),
        1,
        "the install hook must NOT be re-run when an update hook exists: {}",
        r.stdout
    );
}

/// A source that declares no update hook keeps the unchanged behavior: its
/// install hook is re-offered when the source advances.
#[test]
fn source_without_an_update_hook_reruns_its_install_hook() {
    // spec: HOOK-122 HOOK-11
    let sb = Sandbox::new("plain-src");
    let install = sb.tally("install.log");
    sb.write_and_commit("mind.toml", &format!("[[hooks]]\nrun = \"{install}\"\n"));

    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(sb.runs("install.log"), 1);

    sb.write_and_commit("README.md", "# fixture v2\n");
    assert!(sb.mind(&["sync"]).success, "sync must succeed");
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("install.log"),
        2,
        "an advanced source re-runs its install hook: {}",
        r.stdout
    );
}

/// An update hook's run is recorded against the commit it ran at, so a second
/// `upgrade` at the same commit does not run it again.
#[test]
fn source_update_hook_is_recorded_and_not_rerun_at_the_same_commit() {
    // spec: HOOK-124
    let sb = Sandbox::new("recorded-src");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"{update}\"\nevent = \"update\"\n"),
    );

    assert!(
        sb.mind(&[
            "meld",
            &sb.source_spec(),
            "--dangerously-skip-install-hook-check",
        ])
        .success
    );
    sb.write_and_commit("README.md", "# fixture v2\n");
    assert!(sb.mind(&["sync"]).success);
    assert!(
        sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"])
            .success
    );
    assert_eq!(sb.runs("update.log"), 1);

    // Nothing advanced: the recorded run-commit is current, so nothing pends.
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "second upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        1,
        "an update hook recorded at the current commit must not re-run: {}",
        r.stdout
    );
}

/// `meld` then `upgrade` with the source sitting on the very commit it was
/// melded at runs NO update hook: the update event means the source has moved,
/// and it has not. Only once it advances does the hook become pending.
#[test]
fn an_unmoved_source_runs_no_update_hook_on_the_first_upgrade() {
    // spec: HOOK-121
    let sb = Sandbox::new("baseline-src");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"{update}\"\nevent = \"update\"\n"),
    );

    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        0,
        "an update hook must not run at meld"
    );

    // Nothing moved. `sync` records the same commit; `upgrade` must find the
    // update hook settled at it.
    assert!(sb.mind(&["sync"]).success, "sync must succeed");
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        0,
        "an unmoved source must not run its update hook: {}\n{}",
        r.stdout,
        r.stderr
    );

    // `hooks list` reports the baseline as such, not as a run that happened.
    let r = sb.mind(&["hooks", "list", &sb.source_id()]);
    assert!(r.success, "hooks list: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("recorded at meld"),
        "the baseline must not be reported as a run: {}",
        r.stdout
    );

    // The source advances: now it is pending and runs.
    sb.write_and_commit("README.md", "# fixture v2\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        1,
        "an advanced source runs its update hook: {}",
        r.stdout
    );
}

/// One command declared for BOTH events is two records, keyed by event. An
/// install run at the current commit must not settle the update hook: keyed by
/// command alone, `hooks run --event update` would exit 0 having run nothing.
#[test]
fn an_install_run_does_not_settle_the_same_command_on_the_update_event() {
    // spec: HOOK-124
    let sb = Sandbox::new("aliased-src");
    let both = sb.tally("both.log");
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[[hooks]]\nrun = \"{both}\"\n\n[[hooks]]\nrun = \"{both}\"\nevent = \"update\"\n"
        ),
    );

    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}", r.stderr);
    assert_eq!(sb.runs("both.log"), 1, "the install hook ran at meld");

    // Advance, then run the INSTALL hook on demand at the new commit, so its
    // record sits exactly at the source's current commit.
    sb.write_and_commit("README.md", "# fixture v2\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&[
        "hooks",
        "run",
        &sb.source_id(),
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "hooks run install: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("both.log"),
        2,
        "the install hook ran at the new commit"
    );

    // The update hook's own record is still the meld baseline, so it is pending
    // at this commit and must run.
    let r = sb.mind(&[
        "hooks",
        "run",
        &sb.source_id(),
        "--event",
        "update",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "hooks run update: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("both.log"),
        3,
        "the update hook is not settled by the install hook's run: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// A `hooks run` whose every hook is held back by the pending filter says so,
/// names the commit, and prints the `--force` invocation, instead of exiting 0
/// in silence (which reads as "this source declares no hooks").
#[test]
fn hooks_run_reports_nothing_pending_instead_of_exiting_silently() {
    // spec: HOOK-126
    let sb = Sandbox::new("settled-src");
    let install = sb.tally("install.log");
    sb.write_and_commit("mind.toml", &format!("[[hooks]]\nrun = \"{install}\"\n"));

    assert!(
        sb.mind(&[
            "meld",
            &sb.source_spec(),
            "--dangerously-skip-install-hook-check",
        ])
        .success
    );
    assert_eq!(sb.runs("install.log"), 1);

    let r = sb.mind(&[
        "hooks",
        "run",
        &sb.source_id(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "hooks run: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs("install.log"), 1, "nothing pending, so nothing ran");
    let out = format!("{}{}", r.stdout, r.stderr);
    assert!(
        out.contains("nothing pending") && out.contains("--force"),
        "a run with nothing pending must say so and name --force: {out}"
    );

    // And `--force` does run it, which is what the note advertises.
    let r = sb.mind(&[
        "hooks",
        "run",
        &sb.source_id(),
        "--force",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "forced hooks run: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("install.log"),
        2,
        "--force re-runs the settled hook"
    );
}

/// A recorded install hook the source has since WITHDRAWN is not re-offered at
/// the next `upgrade`: the record says the command ran, never that the source
/// still stands behind it.
#[test]
fn a_withdrawn_install_hook_is_not_replayed_at_upgrade() {
    // spec: HOOK-55
    let sb = Sandbox::new("withdrawn-src");
    let install = sb.tally("install.log");
    sb.write_and_commit("mind.toml", &format!("[[hooks]]\nrun = \"{install}\"\n"));

    assert!(
        sb.mind(&[
            "meld",
            &sb.source_spec(),
            "--dangerously-skip-install-hook-check",
        ])
        .success
    );
    assert_eq!(sb.runs("install.log"), 1);

    // The source withdraws the hook and advances.
    sb.write_and_commit("mind.toml", "[source]\ndescription = \"no hooks\"\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("install.log"),
        1,
        "a command the source no longer declares must not be replayed: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// A curator's `[[discover.sources.hooks]]` update entry (DSC-61) is recorded
/// on the nested source at `meld` and offered at `upgrade`. It is declared in
/// the parent's manifest, so without the record it is invisible to the upgrade
/// selector and can never run at all.
#[test]
fn a_curated_update_hook_runs_at_upgrade_and_not_at_meld() {
    // spec: HOOK-127 HOOK-121
    let nested = Sandbox::new("curated-nested");
    let registry = Sandbox::new("curated-registry");
    let update = registry.tally("curated-update.log");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n\n\
             [[discover.sources.hooks]]\nrun = \"{update}\"\nevent = \"update\"\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&[
        "meld",
        &registry.source_spec(),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        registry.runs("curated-update.log"),
        0,
        "an update hook must not run at meld: {}",
        r.stdout
    );

    // The nested source advances.
    nested.write_and_commit("README.md", "# fixture v2\n");
    assert!(registry.mind(&["sync"]).success, "sync must succeed");
    let r = registry.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        registry.runs("curated-update.log"),
        1,
        "the curated update hook must run at upgrade: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// A consumer who melded with `--install-hook` keeps their command on the
/// update path too: a source that moves its command to `event = "update"` does
/// not slip past the override, and the consumer's own command still runs.
#[test]
fn a_meld_install_hook_override_covers_the_update_event() {
    // spec: HOOK-56 HOOK-122
    let sb = Sandbox::new("override-src");
    let theirs = sb.tally("theirs.log");
    let mine = sb.tally("mine.log");
    sb.write_and_commit("mind.toml", &format!("[[hooks]]\nrun = \"{theirs}\"\n"));

    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--install-hook",
        &mine,
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs("mine.log"), 1, "the consumer's command ran");
    assert_eq!(
        sb.runs("theirs.log"),
        0,
        "the author's command was replaced"
    );

    // The source moves the same command to the update event and advances.
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"{theirs}\"\nevent = \"update\"\n"),
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("theirs.log"),
        0,
        "the override must cover the update event: {}\n{}",
        r.stdout,
        r.stderr
    );
    assert_eq!(
        sb.runs("mine.log"),
        2,
        "the consumer's command is what runs on the update path: {}\n{}",
        r.stdout,
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// Item update hooks (HOOK-125)
// ---------------------------------------------------------------------------

/// An item's update hooks replace its install hooks when the item is
/// re-installed over an existing install; a first install runs the install
/// hooks.
#[test]
fn item_update_hook_replaces_the_install_hook_on_upgrade() {
    // spec: HOOK-125 HOOK-130
    let sb = Sandbox::new("item-src");
    let install = sb.tally("install.log");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        &format!(
            "---\ndescription: scanner\ninstall: {install}\nupdate: {update}\n---\n# scanner\n"
        ),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.runs("install.log"), 1, "a first install runs `install`");
    assert_eq!(sb.runs("update.log"), 0, "a first install skips `update`");

    // The item's content changes upstream.
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        &format!(
            "---\ndescription: scanner\ninstall: {install}\nupdate: {update}\n---\n# scanner v2\n"
        ),
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("update.log"),
        1,
        "an upgraded item runs its update hook: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("install.log"),
        1,
        "an upgraded item must not re-run its install hook: {}",
        r.stdout
    );
}

/// A `learn` naming an already-installed item installs nothing (CLI-157), so it
/// runs neither the install nor the update hooks: the re-install path that
/// swaps update hooks in is `upgrade`, not a repeat `learn`.
#[test]
fn relearning_an_installed_item_is_a_no_op_and_runs_no_hook() {
    // spec: HOOK-125 CLI-157
    let sb = Sandbox::new("relearn-src");
    let install = sb.tally("install.log");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        &format!(
            "---\ndescription: scanner\ninstall: {install}\nupdate: {update}\n---\n# scanner\n"
        ),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"])
            .success
    );
    assert_eq!(sb.runs("install.log"), 1);
    assert_eq!(sb.runs("update.log"), 0);

    // Same item, same name, already installed: nothing is installed again.
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "re-learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("already installed"),
        "a repeat learn is a no-op: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("update.log"),
        0,
        "a no-op learn runs no update hook: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("install.log"),
        1,
        "a no-op learn re-runs no install hook: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// Where an item declares its hooks (HOOK-130, HOOK-131, HOOK-133)
// ---------------------------------------------------------------------------

/// A skill declares its install hook in its own `SKILL.md` frontmatter, with no
/// root manifest involved, and it runs at `learn`.
#[test]
fn skill_frontmatter_install_hook_runs_at_learn() {
    // spec: HOOK-130
    let sb = Sandbox::new("fm-src");
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\ninstall: touch installed.sentinel\n---\n# scanner\n",
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    let store = sb.mind_home.join("store").join("skill").join("scanner");
    assert!(
        store.join("installed.sentinel").exists(),
        "the frontmatter install hook must run in the item's store dir: {}",
        r.stdout
    );
}

/// A skill declares its hooks in a scoped `mind.toml` in its own directory. The
/// manifest is item content: it is copied into the store, and editing it
/// upstream is drift the next `upgrade` picks up.
#[test]
fn item_directory_manifest_hook_runs_and_is_item_content() {
    // spec: HOOK-131 HOOK-133
    let sb = Sandbox::new("manifest-src");
    let install = sb.tally("install.log");
    let update = sb.tally("update.log");
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    sb.write_and_commit(
        "skills/scanner/mind.toml",
        &format!("[[hooks]]\nrun = \"{install}\"\nname = \"Set up\"\n"),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.runs("install.log"),
        1,
        "the item manifest's install hook must run: {}",
        r.stdout
    );
    let store = sb.mind_home.join("store").join("skill").join("scanner");
    assert!(
        store.join("mind.toml").exists(),
        "the item manifest is copied into the store with the rest of the item"
    );

    // spec: HOOK-133 -- editing the manifest upstream is drift like any other
    // file change, and the upgraded item picks up the new hook set.
    sb.write_and_commit(
        "skills/scanner/mind.toml",
        &format!("[[hooks]]\nrun = \"{update}\"\nevent = \"update\"\n"),
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("upgraded skill:scanner"),
        "an edited item manifest is drift: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("update.log"),
        1,
        "the newly declared update hook runs on the upgrade: {}",
        r.stdout
    );
    assert_eq!(
        sb.runs("install.log"),
        1,
        "the replaced install hook must not run again: {}",
        r.stdout
    );
}
