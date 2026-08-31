//! Integration tests for update hooks and for an item's own hook declaration
//! sites.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture (local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   HOOK-121: a source's update hooks run at `upgrade`, never at `meld`
//!   HOOK-122: update hooks supersede install hooks on an update; a source that
//!             declares none re-runs its install hooks unchanged
//!   HOOK-124: an update hook's run is recorded, so it is not re-offered at the
//!             same commit
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
