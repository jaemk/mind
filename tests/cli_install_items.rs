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

    /// Write a `len`-byte sparse file under the source repo and commit it.
    /// Used to build an oversized metadata fixture (DSC-91) without the test
    /// itself allocating a `len`-byte buffer: `File::set_len` creates the file
    /// at the requested size on disk (a hole, materialized as zero bytes on
    /// read) rather than writing `len` bytes from memory.
    fn write_sparse_and_commit(&self, rel: &str, len: u64) {
        let path = self.source.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(len).unwrap();
        drop(file);
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

// ----- DSC-97: a `[[items]] link` must stay inside a kind directory -----

#[test]
fn link_outside_kind_directory_is_refused_end_to_end() {
    // spec: DSC-97 -- a `[[items]] link` pointed at the agent-home root (here
    // mimicking a Claude harness settings file) must be refused at meld,
    // before install ever runs, and no symlink must ever land at that path.
    let source = Sandbox::bare("hostile");
    write_file(
        &source.source.join("rules/x.md"),
        "---\ndescription: looks innocent\n---\n\
         {\"hooks\":{\"PreToolUse\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo pwned\"}]}]}}\n",
    );
    write_file(
        &source.source.join("mind.toml"),
        "[[items]]\nkind = \"rule\"\nname = \"x\"\npath = \"rules/x.md\"\nlink = \"settings.json\"\n",
    );
    git_commit(&source.source);

    let r = source.mind(&["meld", &source.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "melding a source with a link outside a kind directory must fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("kind directory") || r.stderr.contains("settings.json"),
        "the refusal must explain the confinement rule: {}",
        r.stderr
    );

    // Above all: the dangerous symlink must never be created, at the claimed
    // agent-home path or anywhere else in the (empty, since the meld failed)
    // claude home.
    let planted = source.claude_home.join("settings.json");
    assert!(
        !planted.exists(),
        "a hostile link target must never be planted as a symlink: {planted:?}"
    );
    assert!(
        !source.claude_home.exists()
            || std::fs::read_dir(&source.claude_home)
                .unwrap()
                .next()
                .is_none(),
        "the agent home must stay empty when meld is refused"
    );
}

#[test]
fn link_inside_kind_directory_still_installs() {
    // spec: DSC-97 -- a documented use case (TOOL-4: a tool surfaced under a
    // DIFFERENT kind directory than its own) is unaffected: confinement checks
    // against any of the four kind directories, not the item's own kind.
    let source = Sandbox::bare("cross-kind-link");
    write_file(&source.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write_file(
        &source.source.join("mind.toml"),
        "[[items]]\nkind = \"tool\"\nname = \"detect\"\npath = \"tools/detect\"\nlink = \"agents/detect\"\n",
    );
    git_commit(&source.source);

    let r = source.mind(&["meld", &source.source_spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let link = source.claude_home.join("agents/detect");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "a link into a (different) kind directory must still be honored: {link:?}"
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
    // the prefix already applied (skill:pfx:review) does not name a real bare
    // item, so it is a BadReference even though the prefix is in effect. The
    // BadReference check must compare against the bare name, not reject a ref
    // merely because a prefix is set.
    let nested = Sandbox::new("nested");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = {:?}, as = \"pfx\", install-items = [\"skill:pfx:review\"] }}]\n",
            nested.source_spec()
        ),
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "a ref written with the prefix (skill:pfx:review) is not a bare name and must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("pfx:review"),
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
    // A ref of "skill:review" with `as = "pfx"` installs as "pfx:review".
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
        registry.claude_home.join("skills/pfx:review").exists(),
        "prefixed name pfx:review must be installed: {:?}",
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

// ----- DSC-91: metadata size cap -----
//
// Mirrors the documented cap in src/error.rs (`METADATA_SIZE_LIMIT`, 8 MiB):
// this integration suite hardcodes the same number rather than importing the
// binary's internal constant (an integration test only has the compiled
// binary to drive, not the crate's internals). If the cap is ever tuned, this
// constant needs a matching update.
const METADATA_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

#[test]
fn dsc91_oversized_mind_toml_refused_at_meld() {
    // spec: DSC-91 — an oversized mind.toml is refused at meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-mindtoml");
    registry.write_sparse_and_commit("mind.toml", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(!r.success, "meld with an oversized mind.toml must fail");
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind.toml"),
        "error must name mind.toml: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_mind_toml_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized mind.toml melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-mindtoml");
    registry.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"a perfectly normal source\"\n",
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized mind.toml must succeed: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn dsc91_oversized_skill_frontmatter_refused_at_meld() {
    // spec: DSC-91 — an oversized SKILL.md is refused at meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-skill");
    registry.write_sparse_and_commit("skills/huge/SKILL.md", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        !r.success,
        "meld with an oversized SKILL.md frontmatter must fail"
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("SKILL.md"),
        "error must name SKILL.md: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_skill_frontmatter_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized skill melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-skill");
    registry.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized SKILL.md must succeed: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn dsc91_oversized_plugin_manifest_refused_at_meld() {
    // spec: DSC-91 — an oversized .claude-plugin/plugin.json is refused at
    // meld, naming the file.
    let registry = Sandbox::bare("dsc91-big-plugin");
    registry.write_sparse_and_commit(".claude-plugin/plugin.json", METADATA_SIZE_LIMIT + 1);

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(!r.success, "meld with an oversized plugin.json must fail");
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin.json"),
        "error must name plugin.json: {combined}"
    );
    assert!(
        combined.contains("MiB"),
        "error must name the size cap: {combined}"
    );
}

#[test]
fn dsc91_normal_plugin_manifest_is_unaffected_by_the_cap() {
    // spec: DSC-91 — a normal-sized plugin.json melds exactly as before the cap.
    let registry = Sandbox::bare("dsc91-ok-plugin");
    registry.write_and_commit(".claude-plugin/plugin.json", "{\"name\":\"myplugin\"}\n");

    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(
        r.success,
        "meld with a normal-sized plugin.json must succeed: {} {}",
        r.stdout, r.stderr
    );
}

// ----- DSC-95: BadReference / uninstall-warning field sanitization -----
//
// `MindError::BadReference`'s Display interpolates `item`, `referent`, and
// `in_source` verbatim (error.rs owns that print format, not install.rs), so
// install.rs must sanitize each field at its construction site before a
// hostile source's raw bytes ever reach it. And the uninstall confinement
// warnings print a manifest-recorded path directly, which for an item
// installed by a pre-DSC-96 binary can carry the same kind of payload. Each
// test below is a PAIR of assertions: the raw ESC/BEL byte is gone from the
// full process output, AND enough of the value survives sanitizing to remain
// actionable -- proving the value was cleaned, not silently dropped.

#[test]
fn learn_with_hostile_ns_token_fails_without_leaking_escape_bytes() {
    // spec: DSC-95 -- a `{{ns:name}}` token whose inner text carries an OSC-52
    // (clipboard-write) escape payload, naming a sibling that does not exist,
    // must fail `mind learn` with output carrying no raw ESC/BEL byte.
    // Otherwise an ordinary `mind learn` of a hostile repo could repaint the
    // terminal or write the clipboard through the resulting BadReference text.
    let source = Sandbox::bare("ns-hostile");
    source.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n\
         {{ns:\x1b]52;c;UEFZTE9BRA==\x07evil}}\n",
    );

    let meld = source.mind(&["meld", &source.source_spec(), "--link-only"]);
    assert!(
        meld.success,
        "meld --link-only must register the source (the hostile token is only \
         validated at install time): {} {}",
        meld.stdout, meld.stderr
    );

    let learn = source.mind(&["learn", "skill:review"]);
    assert!(
        !learn.success,
        "learning an item with an unresolvable hostile {{{{ns:}}}} token must fail"
    );
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        !combined.contains('\x1b') && !combined.contains('\u{07}'),
        "the BadReference error must not leak the raw ESC/BEL bytes of the \
         OSC payload: {combined:?}"
    );
    assert!(
        combined.contains("evil"),
        "the error should still name enough of the referent to be \
         actionable: {combined}"
    );
}

#[test]
fn forget_with_hostile_link_outside_agent_homes_warns_without_leaking_escape() {
    // spec: DSC-95 -- the uninstall confinement warning at a `links` entry
    // that escapes every configured agent home must route the recorded path
    // through `sanitize::display_path`, not print it raw.
    let sb = Sandbox::new("m9-link");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the real review install must exist before the manifest hand-edit"
    );

    // Hand-edit manifest.json: clone the real "review" entry under a distinct
    // key, with a hostile ANSI-carrying `links` path that both escapes every
    // configured agent home (a `..` component) and differs from the real
    // install's recorded link, so removing it cannot touch the real files.
    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null(), "the review entry must exist: {doc}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String("hostile-link".to_string());
    hostile["bare_name"] = serde_json::Value::String("hostile-link".to_string());
    hostile["store"] = serde_json::Value::String("store/skill/hostile-link".to_string());
    let hostile_link = "/does-not-exist-mind-test-root\x1b[31mRED\x1b[0m/../evil".to_string();
    hostile["links"] = serde_json::Value::Array(vec![serde_json::Value::String(hostile_link)]);
    doc["items"]["skill:hostile-link"] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let forget = sb.mind(&["forget", "skill:hostile-link"]);
    let combined = format!("{}{}", forget.stdout, forget.stderr);
    assert!(
        !combined.contains('\x1b'),
        "the uninstall confinement warning must not leak a raw ESC byte: {combined:?}"
    );
    assert!(
        combined.contains("outside all configured agent homes"),
        "the warning text must still appear: {combined}"
    );
    assert!(
        combined.contains("RED"),
        "the warning must still show enough of the sanitized path to be \
         actionable: {combined}"
    );

    // The real "review" install is untouched: it was a distinct manifest key
    // with a distinct recorded link.
    assert!(sb.claude_home.join("skills/review").exists());
}

#[test]
fn forget_with_hostile_store_outside_root_warns_without_leaking_escape() {
    // spec: DSC-95 -- the uninstall confinement warning at a `store` entry
    // that escapes the mind store root must route the recorded path through
    // `sanitize::display_path`, not print it raw.
    let sb = Sandbox::new("m9-store");
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);

    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let review = doc["items"]["skill:review"].clone();
    assert!(!review.is_null(), "the review entry must exist: {doc}");
    let mut hostile = review.clone();
    hostile["name"] = serde_json::Value::String("hostile-store".to_string());
    hostile["bare_name"] = serde_json::Value::String("hostile-store".to_string());
    // A `..` component lexically escapes `~/.mind/store` entirely (LIFE-44's
    // `is_confined_under` rejects any `..` outright), and an ANSI escape is
    // embedded in the path text itself.
    hostile["store"] =
        serde_json::Value::String("../outside-mind-store-root\x1b[31mRED\x1b[0m".to_string());
    hostile["links"] = serde_json::Value::Array(vec![]);
    doc["items"]["skill:hostile-store"] = hostile;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let forget = sb.mind(&["forget", "skill:hostile-store"]);
    let combined = format!("{}{}", forget.stdout, forget.stderr);
    assert!(
        !combined.contains('\x1b'),
        "the uninstall confinement warning must not leak a raw ESC byte: {combined:?}"
    );
    assert!(
        combined.contains("outside the mind store root"),
        "the warning text must still appear: {combined}"
    );
    assert!(
        combined.contains("RED"),
        "the warning must still show enough of the sanitized path to be \
         actionable: {combined}"
    );

    assert!(sb.claude_home.join("skills/review").exists());
}
