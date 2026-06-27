//! Integration tests for the `install-items` subset directive (DSC-62/63/64).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. No network. The fixture mirrors the pattern in tests/cli.rs.

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
    fn build(name: &str, with_fixture: bool) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-install-items-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        if with_fixture {
            write_file(
                &source.join("skills/review/SKILL.md"),
                "---\nname: review\ndescription: Review the diff\n---\n# review\n",
            );
            write_file(
                &source.join("agents/dev.md"),
                "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
            );
            write_file(
                &source.join("rules/style.md"),
                "---\ndescription: Style rule\n---\n# style\n",
            );
        } else {
            write_file(&source.join("README.md"), "# registry\n");
        }

        git_init(&source);
        sb
    }

    /// A source repo with items (one skill, one agent, one rule).
    fn new(name: &str) -> Sandbox {
        Sandbox::build(name, true)
    }

    /// A source repo with no items (e.g. a pure super-source).
    fn bare(name: &str) -> Sandbox {
        Sandbox::build(name, false)
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn mind(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Write a file under the source repo and commit it.
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write_file(&self.source.join(rel), contents);
        git_commit(&self.source);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git_init(dir: &Path) {
    for args in [
        vec!["-c", "init.defaultBranch=main", "init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "initial"],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} in {dir:?}");
    }
}

fn git_commit(dir: &Path) {
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "fixture"]] {
        let _ = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ----- DSC-62: subset install via install-items -----

#[test]
fn install_items_subset_is_offered_not_others() {
    // spec: DSC-62 — melding a super-source with install-items = ["skill:review"]
    // offers only the named item for install (via --yes auto-install); the other
    // items of the nested source are registered but not installed.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // Meld with --yes: the listed subset should install.
    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The named item is installed.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review (in install-items) must be installed: {:?}",
        registry.claude_home
    );

    // The non-listed items are NOT installed.
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev (not in install-items) must NOT be installed"
    );
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "rule:style (not in install-items) must NOT be installed"
    );

    // But the non-listed items are still available (can be learned explicitly).
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("agent:dev"),
        "agent:dev must still be available via probe: {}",
        probe.stdout
    );
}

#[test]
fn install_items_other_items_remain_available_and_learnable() {
    // spec: DSC-62 — the source's non-listed items stay registered and available;
    // they can be learned explicitly after the super-source is melded.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // Explicitly learn a non-listed item.
    let learn = registry.mind(&["learn", "agent:dev"]);
    assert!(
        learn.success,
        "explicitly learning a non-listed item must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed after explicit `learn`"
    );
}

#[test]
fn install_items_yes_flag_installs_subset_non_interactively() {
    // spec: DSC-62 — CLI-23: --yes installs the subset without prompting
    // (non-interactive / non-TTY run with --yes must install).
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\", \"agent:dev\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {}", r.stderr);

    // Both listed items are installed.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review must be installed with --yes"
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed with --yes"
    );
    // The unlisted item is not installed.
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "rule:style (not in install-items) must not be installed"
    );
}

#[test]
fn install_items_link_only_installs_nothing() {
    // spec: DSC-62 — CLI-23: --link-only skips install, even when install-items
    // is non-empty; only registration occurs.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(r.success, "meld --link-only failed: {}", r.stderr);

    // Nothing installed under link-only.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "--link-only must not install any items, even when install-items is set"
    );

    // The nested source is still registered.
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("nested"),
        "nested source must be registered under --link-only: {}",
        sources.stdout
    );
}

#[test]
fn install_items_empty_installs_nothing() {
    // spec: DSC-62 — install-items = [] is equivalent to install = false:
    // the nested source is registered, no items are offered for install.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // No items are installed.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "install-items = [] must not install any items"
    );
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "install-items = [] must not install any items"
    );
    // Items are still available.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:review"),
        "items must still be available after install-items = []: {}",
        probe.stdout
    );
}

// ----- DSC-62: recursive overrides install-items -----

#[test]
fn recursive_overrides_install_items_installs_all() {
    // spec: DSC-62 — meld --recursive is the superset: it installs every nested
    // source's items regardless of install-items, so install-items is effectively
    // ignored under --recursive.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--recursive", "--yes"]);
    assert!(r.success, "meld --recursive failed: {}", r.stderr);

    // All items installed, not just the listed subset.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review must be installed under --recursive"
    );
    assert!(
        registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev must be installed under --recursive (overrides install-items)"
    );
    assert!(
        registry.claude_home.join("rules/style.md").exists(),
        "rule:style must be installed under --recursive (overrides install-items)"
    );
}

#[test]
fn install_items_non_tty_without_yes_installs_nothing_but_notes() {
    // spec: DSC-62 — CLI-23: in a non-interactive (non-TTY) run without --yes,
    // the subset is NOT installed; a note points at `mind learn`. The test
    // harness pipes stdin (Stdio::null), so the run is non-TTY.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // Plain meld: no --yes, no --link-only -> non-TTY note path.
    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Nothing is installed without --yes in a non-TTY run.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "non-TTY meld without --yes must NOT install the subset"
    );
    // A note points the user at how to install it.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("learn") && combined.contains("nested"),
        "a note should point at `mind learn` for the nested source: {combined}"
    );
}

#[test]
fn install_items_remeld_honors_subset() {
    // spec: DSC-62 — re-melding an already-registered super-source honors
    // install-items on the re-meld too (DSC-58/DSC-62 apply on fresh meld AND
    // re-meld). First meld --link-only (registers but installs nothing), then a
    // re-meld --yes must install exactly the subset.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // First meld: link-only, so nothing installs but the chain is registered.
    let first = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(first.success, "initial meld failed: {}", first.stderr);
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "nothing should be installed after --link-only meld"
    );

    // Re-meld with --yes: the super-source is already melded, so this goes
    // through the remeld path. It must still honor install-items.
    let second = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(
        second.success,
        "re-meld failed: {} {}",
        second.stdout, second.stderr
    );

    // The subset installs on re-meld.
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "re-meld must honor install-items and install the subset"
    );
    // The unlisted items are still NOT installed on re-meld.
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "re-meld must not install items outside install-items"
    );
    assert!(
        !registry.claude_home.join("rules/style.md").exists(),
        "re-meld must not install items outside install-items"
    );
}

// ----- DSC-63: bad ref is a BadReference error at meld -----

#[test]
fn install_items_unknown_ref_errors_at_meld() {
    // spec: DSC-63 — a ref naming an item the nested source does NOT offer is a
    // BadReference error at meld, not a silent skip.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:nonexistent\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with an unknown install-items ref must fail"
    );
    // The error message must reference the bad ref.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("nonexistent"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_wrong_kind_ref_errors_at_meld() {
    // spec: DSC-63 — a ref of the wrong kind for an existing bare name is a
    // BadReference at meld. `review` exists only as a skill, so `agent:review`
    // names an item the nested source does not offer.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"agent:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "a wrong-kind ref (agent:review when review is a skill) must fail at meld"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("review"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_one_bad_ref_in_list_aborts_and_installs_nothing() {
    // spec: DSC-63 — a list with a valid and an invalid ref fails the whole meld
    // (BadReference, not a silent skip of just the bad one), and because the
    // error is raised before install, nothing from the subset is installed.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install-items = [\"skill:review\", \"skill:ghost\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "a list containing one unknown ref must fail the meld"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("ghost"),
        "error must name the bad ref: {combined}"
    );
    // The valid ref must NOT have been installed: validation precedes install.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "a bad ref aborts before install; the valid item must not be installed"
    );
}

#[test]
fn install_items_prefixed_name_ref_is_rejected() {
    // spec: DSC-63 — refs are BARE kind:name in source truth. A ref written with
    // the prefix already applied (skill:pfx-review) does not name a real bare
    // item, so it is a BadReference even though the prefix is in effect. The
    // BadReference check must compare against the bare name, not reject a ref
    // merely because a prefix is set.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:pfx-review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "a ref written with the prefix (skill:pfx-review) is not a bare name and must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("pfx-review"),
        "error must name the bad ref: {combined}"
    );
}

#[test]
fn install_items_bare_ref_accepted_despite_prefix_in_effect() {
    // spec: DSC-63 — the converse of the rejection test: a BARE ref must be
    // accepted (not rejected) even when a prefix is in effect for the entry, and
    // it installs under the prefixed effective name.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    // No --yes: must not error at meld (the bare ref is valid). Use --link-only
    // so the meld validates the ref but does not depend on install behavior.
    let r = registry.mind(&["meld", &registry.source_spec(), "--link-only"]);
    assert!(
        r.success,
        "a bare ref must be accepted when a prefix is in effect: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn install_items_prefix_applied_at_install_time() {
    // spec: DSC-63 — refs in install-items are bare (source truth); the prefix
    // in effect for the entry (`as`, DSC-39) is applied at install time.
    // A ref of "skill:review" with `as = "pfx"` installs as "pfx-review".
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // The item installs under the prefixed name.
    assert!(
        registry.claude_home.join("skills/pfx-review").exists(),
        "prefixed name pfx-review must be installed: {:?}",
        registry.claude_home
    );
    // The bare name link must NOT exist.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "bare name review must not exist when prefix is in effect"
    );
}

// ----- DSC-64: install = true + non-empty install-items is an error -----

#[test]
fn install_true_and_install_items_is_toml_error() {
    // spec: DSC-64 — install = true together with a non-empty install-items on
    // the same entry is a MindToml error at meld (mutually exclusive).
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true, install-items = [\"skill:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with install = true + non-empty install-items must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mutually exclusive"),
        "error must mention 'mutually exclusive': {combined}"
    );
}

#[test]
fn install_true_with_empty_install_items_is_not_an_error() {
    // spec: DSC-64 — install = true and install-items = [] is NOT an error;
    // the empty list overrides the boolean (both say "install nothing effectively").
    // This is allowed per the spec: the mutual-exclusion error is only for non-empty.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, install = true, install-items = [] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with install = true + empty install-items must succeed: {}",
        r.stderr
    );
}

// ----- DSC-62: install-items governs over install = false (absent) -----

#[test]
fn install_items_governs_when_present_regardless_of_install_flag() {
    // spec: DSC-62 — when install-items is present it governs; when absent the
    // install boolean governs. Two entries: one with install-items (governs),
    // one with install = true but no install-items (boolean governs).
    let nested_a = Sandbox::new("nested-a"); // has install-items
    let nested_b = Sandbox::bare("nested-b"); // has install = true
    nested_b.write_and_commit(
        "skills/special/SKILL.md",
        "---\nname: special\ndescription: Special skill\n---\n# special\n",
    );

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [\n  {{ source = {:?}, install-items = [\"skill:review\"] }},\n  {{ source = {:?}, install = true }}\n]\n",
            nested_a.source_spec(),
            nested_b.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {}", r.stderr);

    // nested-a: only skill:review installed (install-items governs).
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "skill:review (in install-items) must be installed"
    );
    assert!(
        !registry.claude_home.join("agents/dev.md").exists(),
        "agent:dev (not in install-items for nested-a) must not be installed"
    );

    // nested-b: all items installed (install = true, no install-items).
    assert!(
        registry.claude_home.join("skills/special").exists(),
        "skill:special from nested-b (install = true) must be installed"
    );
}
