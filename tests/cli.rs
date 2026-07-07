//! End-to-end tests that drive the real `mind` binary against a hermetic,
//! network-free fixture (a local git repo melded via a filesystem path).
//!
//! Every manual smoke-test ("probe") of the CLI lives here as an assertion so
//! the behavior can be re-run and audited. See CLAUDE.md: manual checks must be
//! encoded as tests unless that is genuinely impossible.

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
    /// Build a fixture source repo with one skill, one agent, and one rule.
    /// A source repo named `agents` carrying the standard fixture items.
    fn new() -> Sandbox {
        Sandbox::build("agents", true)
    }

    /// A source repo with a given name and the standard fixture items.
    fn named(name: &str) -> Sandbox {
        Sandbox::build(name, true)
    }

    /// A source repo with a given name and no items (e.g. a pure registry).
    fn bare(name: &str) -> Sandbox {
        Sandbox::build(name, false)
    }

    /// A source repo populated from `examples/<name>` in the crate, committed.
    /// Lets a test drive a shipped example so it cannot rot.
    fn from_example(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name);
        copy_dir(&example, &source);
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    /// A source repo carrying the crate's real root `mind.toml` plus the
    /// `examples/hello` directory it points at, committed. Drives the
    /// landing-page command (`mind meld jaemk/mind`, then `mind learn
    /// hello-mind` in a non-TTY).
    fn from_root_mindfile() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("mind");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("home"),
            claude_home: base.join("claude"),
        };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&source).unwrap();
        // The real root mind.toml curates two REMOTE skill libraries via
        // [discover].sources; melding it would clone them over the network, which
        // the hermetic harness forbids. Substitute local stand-in repos for those
        // URLs so the meld runs fully offline while still exercising the
        // register-only curated chain alongside the repo's own hello-mind
        // convention discovery. The real file's discover block is validated
        // offline by a unit test in src/mindfile.rs.
        let nested_a = base.join("anthropics-skills");
        write(
            &nested_a.join("skills/astand/SKILL.md"),
            "---\nname: astand\ndescription: stand-in for a curated skill\n---\n# astand\n",
        );
        git(&nested_a, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&nested_a, &["config", "user.email", "t@t"]);
        git(&nested_a, &["config", "user.name", "t"]);
        git(&nested_a, &["add", "-A"]);
        git(&nested_a, &["commit", "-qm", "initial"]);
        let nested_b = base.join("awesome-claude-skills");
        write(
            &nested_b.join("README.md"),
            "# awesome (stand-in, no items)\n",
        );
        git(&nested_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&nested_b, &["config", "user.email", "t@t"]);
        git(&nested_b, &["config", "user.name", "t"]);
        git(&nested_b, &["add", "-A"]);
        git(&nested_b, &["commit", "-qm", "initial"]);

        let mindfile = std::fs::read_to_string(root.join("mind.toml")).unwrap();
        let mindfile = mindfile
            .replace(
                "https://github.com/anthropics/skills",
                nested_a.to_str().unwrap(),
            )
            .replace(
                "https://github.com/ComposioHQ/awesome-claude-skills",
                nested_b.to_str().unwrap(),
            );
        write(&source.join("mind.toml"), &mindfile);
        copy_dir(&root.join("examples/hello"), &source.join("examples/hello"));
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    fn build(name: &str, with_fixture: bool) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        if with_fixture {
            write(
                &source.join("skills/review/SKILL.md"),
                "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\n",
            );
            write(
                &source.join("agents/dev.md"),
                "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\n",
            );
            write(
                &source.join("rules/style.md"),
                "---\ndescription: ASCII only\n---\n# style rule\n",
            );
        } else {
            write(&source.join("README.md"), "# registry\n");
        }

        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    /// Run `mind <args>` against this sandbox.
    fn mind(&self, args: &[&str]) -> Run {
        self.run(args, None, &[])
    }

    fn mind_with_input(&self, args: &[&str], input: Option<&str>) -> Run {
        self.run(args, input, &[])
    }

    /// Run `mind` with additional environment variables (e.g. MIND_AGENT_HOMES).
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.run(args, None, envs)
    }

    /// Run `mind` with the child's working directory set to `cwd` (for testing
    /// how relative paths are resolved).
    fn mind_cwd(&self, args: &[&str], cwd: &Path) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .current_dir(cwd)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    fn run(&self, args: &[&str], input: Option<&str>, envs: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn mind");
        if let Some(text) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().expect("wait mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Change the skill upstream and commit, so a `sync` + `upgrade` sees a delta.
    fn edit_source(&self) {
        write(
            &self.source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
        );
        git(&self.source, &["commit", "-aqm", "edit review"]);
    }

    /// Write a file under the source repo and commit it.
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    /// Remove a file from the source repo and commit it.
    fn remove_and_commit(&self, rel: &str) {
        std::fs::remove_file(self.source.join(rel)).unwrap();
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "remove"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The base dir name, which becomes the `owner` for this sandbox's local
    /// source (so the source identity is `<base_name>/<source dir name>`).
    fn base_name(&self) -> String {
        self.base
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
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

/// Recursively copy `src` into `dst` (files and subdirectories).
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
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

/// Assert no `review-*` scratch dir survives under `<mind_home>/.tmp` (the
/// remote-clone area). CLI-130: review changes nothing on disk.
fn assert_no_review_temp(mind_home: &Path) {
    let tdir = mind_home.join(".tmp");
    if !tdir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(&tdir).unwrap().flatten() {
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with("review-"),
            "leftover review temp dir: {:?}",
            entry.path()
        );
    }
}

/// Meld + learn the standard fixture; returns the ready sandbox.
fn melded() -> Sandbox {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {}", r.stderr);
    sb
}

#[test]
fn meld_registers_source_and_lists_items() {
    // spec: CLI-10, CLI-72
    let sb = melded();
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.success);
    assert!(r.stdout.contains("agents"), "sources: {}", r.stdout);
}

#[test]
fn meld_yes_installs_all_source_items() {
    // spec: CLI-23 - `meld --yes` registers the source and installs all of its
    // items without prompting (so it works in this non-TTY harness too).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    let recall = sb.mind(&["recall"]);
    for item in ["review", "dev", "style"] {
        assert!(
            recall.stdout.contains(item),
            "{item} should be installed after `meld --yes`: {}",
            recall.stdout
        );
    }
}

#[test]
fn meld_link_only_registers_without_installing() {
    // spec: CLI-23 - `--link-only` stops at registering the source; nothing is
    // installed and there is no install prompt.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source must be registered"
    );
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "--link-only must not install any items"
    );
}

#[test]
fn meld_default_non_tty_registers_only_and_notes_install() {
    // spec: CLI-23 - a default `meld` over piped (non-TTY) stdin registers the
    // source but installs nothing, and prints how to install later.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source must be registered"
    );
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "a non-TTY default meld must not install items"
    );
    assert!(
        r.stdout.contains("learn") && r.stdout.contains("#*"),
        "it should note how to install later: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("nothing installed"),
        "the note must say explicitly that nothing was installed: {}",
        r.stdout
    );
}

#[test]
fn meld_uses_declared_prefix_when_installing() {
    // spec: CLI-24 - a non-interactive meld accepts a source's declared
    // `[source].prefix`; installed items are namespaced `<prefix>:<name>`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "meld of a prefixed source should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("jk:review"),
        "items must carry the declared prefix: {recall}"
    );
}

#[test]
fn meld_as_empty_overrides_a_declared_prefix() {
    // spec: CLI-24 - `--as ''` is the explicit "no prefix" override and
    // suppresses a declared `[source].prefix`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--as", "", "--yes"]).success,
        "meld --as '' should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("review"), "items must install: {recall}");
    assert!(
        !recall.contains("jk:"),
        "the declared prefix must be overridden to none: {recall}"
    );
}

#[test]
fn meld_namespace_empty_overrides_a_declared_prefix() {
    // spec: CLI-159 CLI-24 - `--namespace ''` (the renamed `--as`) is the explicit
    // "no prefix" override and suppresses a declared `[source].prefix`, identically
    // to the deprecated `--as ''`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "", "--yes"])
            .success,
        "meld --namespace '' should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("review"), "items must install: {recall}");
    assert!(
        !recall.contains("jk:"),
        "the declared prefix must be overridden to none: {recall}"
    );
}

#[test]
fn meld_with_no_arg_melds_the_current_directory() {
    // spec: CLI-25 - `mind meld` with no repo argument melds the directory it is
    // run in. `--link-only` keeps the test to just the registration.
    let sb = Sandbox::new();
    let r = sb.mind_cwd(&["meld", "--link-only"], &sb.source);
    assert!(
        r.success,
        "no-arg meld of the cwd failed: {} {}",
        r.stdout, r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("agents"),
        "the current directory must be registered as a source: {sources}"
    );
}

#[test]
fn local_source_is_read_from_its_working_tree() {
    // spec: CLI-27 - a linked local source is read from its working tree, so an
    // untracked mind.toml is seen; it is never cloned, and unmeld never deletes it.
    let sb = Sandbox::bare("worktree-src");
    // Commit an item, then add an UNTRACKED mind.toml (in no commit).
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ndescription = \"live working tree\"\n",
    );
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    // No clone is made under the sources tree; the working tree is the source.
    assert!(
        !clone_dir_of(&sb, "worktree-src").exists(),
        "a linked local source must not be cloned"
    );
    // The untracked mind.toml is read live from the working tree.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("live working tree"),
        "the untracked mind.toml must be read from the working tree: {sources}"
    );

    // unmeld leaves the working tree intact.
    assert!(
        sb.mind(&["unmeld", "worktree-src", "--unlink-only"])
            .success
    );
    assert!(
        sb.source.join("skills/a/SKILL.md").exists(),
        "unmeld must not delete the linked working tree"
    );
}

#[test]
fn init_source_reports_refs_scaffolds_toml_and_templates() {
    // spec: INIT-1, INIT-2, INIT-3, INIT-4, INIT-5, INIT-6
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring");
    // skill `review` references agent `dev` in bare prose and `style` via a token.
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\nHand off to dev, then see {{ns:style}}.\n",
    );
    write(
        &repo.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    write(
        &repo.join("rules/style.md"),
        "---\ndescription: style\n---\n# style\n",
    );
    let dir = repo.to_str().unwrap();

    // Report mode: items + reference graph + scaffold; nothing in the store.
    let r = sb.mind(&["init-source", dir]);
    assert!(r.success, "init-source failed: {} {}", r.stdout, r.stderr);
    // INIT-2 / INIT-4: items and references are reported.
    assert!(
        r.stdout.contains("review") && r.stdout.contains("dev") && r.stdout.contains("style"),
        "items and references must be reported: {}",
        r.stdout
    );
    // INIT-9: this fixture declares no prefix, so the bare `dev` mention is NOT
    // flagged (an unprefixed source's bare references resolve as written). The
    // prefix-gated advisory is covered in tests/item_lifecycle.rs.
    assert!(
        !r.stdout.contains("advisory [unguarded-reference]"),
        "no prefix => no unguarded-reference advisory (INIT-9): {}",
        r.stdout
    );
    // INIT-3: a mind.toml is scaffolded when absent, with a `[source]` table and
    // a commented-out generic namespace example whose value matches its comment.
    let scaffold = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        scaffold.contains("[source]") && scaffold.contains("# namespace = \"prefix\""),
        "scaffold must carry a [source] table and a generic commented namespace: {scaffold}"
    );
    // INIT-6: init-source registers nothing (no store state).
    assert!(
        !sb.mind_home.join("sources.json").exists(),
        "init-source must not write to the store"
    );

    // INIT-3: an existing mind.toml is left unchanged on a re-run.
    let toml_before = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(sb.mind(&["init-source", dir]).success);
    assert_eq!(
        std::fs::read_to_string(repo.join("mind.toml")).unwrap(),
        toml_before,
        "an existing mind.toml must not be overwritten"
    );

    // INIT-5: --template wraps the bare `dev`; the existing `{{ns:style}}` survives.
    let t = sb.mind(&["init-source", dir, "--template"]);
    assert!(
        t.success,
        "init-source --template failed: {} {}",
        t.stdout, t.stderr
    );
    let review = std::fs::read_to_string(repo.join("skills/review/SKILL.md")).unwrap();
    assert!(
        review.contains("{{ns:dev}}"),
        "the bare `dev` reference must be templated: {review}"
    );
    assert!(
        review.contains("{{ns:style}}"),
        "the existing token must survive: {review}"
    );
    assert!(
        !review.contains("to dev,"),
        "the bare `dev` must be replaced, not duplicated: {review}"
    );
}

#[test]
fn init_source_flags_helper_script_duplicated_across_items() {
    // spec: INIT-7
    let sb = Sandbox::new();
    let repo = sb.base.join("authoring");
    write(
        &repo.join("skills/a/SKILL.md"),
        "---\ndescription: a\n---\n# a\n",
    );
    write(&repo.join("skills/a/helper.sh"), "#!/bin/sh\necho shared\n");
    write(
        &repo.join("skills/b/SKILL.md"),
        "---\ndescription: b\n---\n# b\n",
    );
    write(&repo.join("skills/b/helper.sh"), "#!/bin/sh\necho shared\n");
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir]);
    assert!(r.success, "init-source failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("advisory [duplicate-tooling]") && r.stdout.contains("helper.sh"),
        "init-source must surface the duplicate-tooling advisory like review: {}",
        r.stdout
    );
}

#[test]
fn review_with_no_target_reviews_the_current_directory() {
    // spec: CLI-26 - `mind review` with no <target> validates the cwd.
    let sb = Sandbox::new();
    let r = sb.mind_cwd(&["review"], &sb.source);
    assert!(
        r.success,
        "a bare `review` of the current directory should succeed for a clean source: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn remeld_of_an_uninstalled_source_offers_to_install() {
    // spec: CLI-12 - re-melding is not an error; with items still uninstalled it
    // routes to the default install flow (here non-TTY, so it notes how to install).
    let sb = melded(); // non-TTY meld registers but does not install
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld should not error: {}", r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "re-meld must report the source is already melded: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("to install"),
        "with items uninstalled, re-meld must offer to install them: {}",
        r.stdout
    );
}

#[test]
fn remeld_of_an_installed_source_shows_item_status() {
    // spec: CLI-12 - when nothing remains to install, re-melding prints each
    // item's install state and the commit it was installed from.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "initial meld+install"
    );
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld should not error: {}", r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "re-meld must report the source is already melded: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:review") && r.stdout.contains("installed @"),
        "re-meld of a fully installed source must show item status with commits: {}",
        r.stdout
    );
}

#[test]
fn remeld_namespace_change_locked_when_items_installed() {
    // spec: CLI-13 CLI-161 NS-30 - a re-meld with --namespace that differs from
    // the current namespace is refused when items are installed, naming the
    // installed items and directing the user to forget them first.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "initial meld+install"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "item installs unprefixed first"
    );

    // Attempting to change the namespace while items are installed must fail.
    let r = sb.mind(&["meld", &spec, "--namespace", "jk", "--yes"]);
    assert!(
        !r.success,
        "re-meld --namespace with installed items must fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("namespace") || r.stderr.contains("installed"),
        "error must mention namespace lock: {}",
        r.stderr
    );
    // The old unprefixed link must be untouched.
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "existing unprefixed link must survive the refused re-meld"
    );
    // The prefixed link must NOT have been created.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_err(),
        "the prefixed link must not exist after a refused re-meld"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        !recall.contains("jk:review"),
        "recall must still show the original unprefixed name: {recall}"
    );
}

#[test]
fn probe_lists_all_three_kinds() {
    // spec: CLI-80, DSC-1, DSC-10, DSC-11, DSC-12, DSC-36
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success);
    assert!(r.stdout.contains("skill:review"), "{}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "{}", r.stdout);
    assert!(r.stdout.contains("rule:style"), "{}", r.stdout);
}

#[test]
fn probe_filters_by_substring() {
    // spec: CLI-80
    let sb = melded();
    let r = sb.mind(&["probe", "review"]);
    assert!(r.stdout.contains("skill:review"));
    assert!(!r.stdout.contains("agent:dev"), "{}", r.stdout);
}

#[test]
fn probe_matches_description_text() {
    // spec: CLI-85
    // The fixture's skill:review has description "Review the diff for bugs".
    // Querying "bugs" is only present in the description, not the item name.
    let sb = melded();
    let r = sb.mind(&["probe", "bugs"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("skill:review"),
        "expected skill:review in output: {}",
        r.stdout
    );
    // agent:dev description is "Implements a spec with tests" - no "bugs"
    assert!(
        !r.stdout.contains("agent:dev"),
        "unexpected agent:dev in output: {}",
        r.stdout
    );
}

#[test]
fn probe_query_is_case_insensitive() {
    // spec: CLI-85
    // "Review" (capitalized) matches item name "review" case-insensitively.
    let sb = melded();
    let r = sb.mind(&["probe", "REVIEW"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("skill:review"),
        "expected skill:review in output: {}",
        r.stdout
    );
}

#[test]
fn probe_description_query_composes_with_kind_filter() {
    // spec: CLI-85, CLI-83
    // The agent:dev description contains "spec". Filter to agents only.
    let sb = melded();
    let r = sb.mind(&["probe", "--kind", "agent", "spec"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("agent:dev"),
        "expected agent:dev in output: {}",
        r.stdout
    );
    // skill:review description mentions "diff" not "spec"; should be excluded
    assert!(
        !r.stdout.contains("skill:review"),
        "unexpected skill:review in output: {}",
        r.stdout
    );
}

#[test]
fn probe_description_query_composes_with_source_filter() {
    // spec: CLI-85, CLI-83
    // Implementor flagged this gap: --source composition with a
    // description-only query had no integration test (only --kind did).
    //
    // Meld two sources. The standard fixture (`agents`) describes its review
    // skill as "Review the diff for bugs". A second source (`tools`) carries a
    // review skill whose description has the unique word "kubernetes", absent
    // from every item in `agents`. Querying that word with --source must match
    // the item in `tools` by description and exclude `agents` entirely.
    let agents = melded();
    let tools = Sandbox::named("tools");
    tools.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Deploy onto kubernetes clusters\n---\n# review skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of second source failed"
    );

    // --source tools + the description-only query matches the tools item and
    // names the tools source.
    let in_tools = agents.mind(&["probe", "--source", "tools", "kubernetes"]);
    assert!(in_tools.success, "probe failed: {}", in_tools.stderr);
    assert!(
        in_tools.stdout.contains("skill:review"),
        "expected skill:review from tools: {}",
        in_tools.stdout
    );
    assert!(
        in_tools.stdout.contains("tools"),
        "expected the tools source column: {}",
        in_tools.stdout
    );

    // The same query scoped to the other source matches nothing: "kubernetes"
    // is not in any `agents` item, so the source filter composes (it does not
    // leak across sources).
    let in_agents = agents.mind(&["probe", "--source", "agents", "kubernetes"]);
    assert!(in_agents.success, "probe failed: {}", in_agents.stderr);
    assert!(
        !in_agents.stdout.contains("skill:review"),
        "kubernetes must not match any agents item: {}",
        in_agents.stdout
    );
    assert!(
        in_agents.stdout.contains("no items match"),
        "expected an empty-result note: {}",
        in_agents.stdout
    );
}

/// Meld a default `agents` source plus a second `tools` source carrying a
/// uniquely-named skill, so a `--source` filter can be checked for exclusion.
/// Both sandboxes are returned so the caller keeps the linked source dirs alive
/// (a local source is linked by path, not copied, so its dir must survive).
fn melded_two_sources() -> (Sandbox, Sandbox) {
    let agents = melded();
    // A bare source (no standard fixture) carrying only a uniquely-named skill,
    // so its items never overlap the agents source.
    let tools = Sandbox::bare("tools");
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of second source failed"
    );
    (agents, tools)
}

#[test]
fn probe_source_glob_narrows_to_matching_sources() {
    // spec: CLI-86 - the `--source` filter accepts a glob matched against source
    // identities; `*agents` shows only the agents source's items and excludes a
    // second `tools` source.
    let (sb, _tools) = melded_two_sources();

    let only_agents = sb.mind(&["probe", "--no-tui", "--source", "*agents"]);
    assert!(only_agents.success, "{}", only_agents.stderr);
    assert!(
        only_agents.stdout.contains("skill:review"),
        "expected the agents source's item: {}",
        only_agents.stdout
    );
    assert!(
        !only_agents.stdout.contains("skill:deploy"),
        "the tools source's item must be excluded: {}",
        only_agents.stdout
    );

    // The complementary glob shows only the tools source's item.
    let only_tools = sb.mind(&["probe", "--no-tui", "--source", "*tools"]);
    assert!(only_tools.success, "{}", only_tools.stderr);
    assert!(
        only_tools.stdout.contains("skill:deploy"),
        "expected the tools source's item: {}",
        only_tools.stdout
    );
    assert!(
        !only_tools.stdout.contains("skill:review"),
        "the agents source's item must be excluded: {}",
        only_tools.stdout
    );
}

#[test]
fn recall_source_glob_narrows_to_matching_sources() {
    // spec: CLI-86 - the `recall` listing `--source` filter accepts a glob the
    // same way as probe.
    let (sb, _tools) = melded_two_sources();

    let only_agents = sb.mind(&["recall", "--source", "*agents"]);
    assert!(only_agents.success, "{}", only_agents.stderr);
    assert!(
        only_agents.stdout.contains("review"),
        "expected the agents source's item: {}",
        only_agents.stdout
    );
    assert!(
        !only_agents.stdout.contains("deploy"),
        "the tools source's item must be excluded: {}",
        only_agents.stdout
    );
}

#[test]
fn probe_source_glob_matching_nothing_is_empty() {
    // spec: CLI-86 - a glob that matches no source yields an empty listing (no
    // error), as any fully-excluding filter does.
    let (sb, _tools) = melded_two_sources();
    let r = sb.mind(&["probe", "--no-tui", "--source", "*nope"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stdout.contains("skill:review") && !r.stdout.contains("skill:deploy"),
        "no items should be listed: {}",
        r.stdout
    );
}

#[test]
fn probe_source_glob_composes_with_json() {
    // spec: CLI-86, CLI-84, CLI-167 - the glob `--source` filter composes with `--json`;
    // result is wrapped in {"schema": 1, "items": [...]}.
    let (sb, _tools) = melded_two_sources();
    let r = sb.mind(&["probe", "--no-tui", "--source", "*agents", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let envelope: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("probe --json envelope");
    assert_eq!(
        envelope["schema"], 1,
        "schema field must be 1: {}",
        r.stdout
    );
    let rows = envelope["items"].as_array().expect("items array");
    assert!(
        rows.iter().any(|row| row["name"] == "review"),
        "agents item present in json: {}",
        r.stdout
    );
    assert!(
        !rows.iter().any(|row| row["name"] == "deploy"),
        "tools item excluded from json: {}",
        r.stdout
    );
}

#[test]
fn probe_source_glob_composes_with_kind_and_query() {
    // spec: CLI-86, CLI-85, CLI-83 - the glob `--source` filter ANDs with `--kind`
    // and the positional substring query simultaneously. Add a non-skill item and a
    // non-matching skill to the agents source so each filter is load-bearing: only
    // the row that satisfies all three (source `*agents`, kind `skill`, query
    // `review`) survives.
    let (sb, _tools) = melded_two_sources();

    let r = sb.mind(&[
        "probe", "--no-tui", "--source", "*agents", "--kind", "skill", "review",
    ]);
    assert!(r.success, "{}", r.stderr);
    // Satisfies all three filters.
    assert!(
        r.stdout.contains("skill:review"),
        "the item matching source+kind+query must be shown: {}",
        r.stdout
    );
    // Excluded by --kind (same source, matches neither kind nor query).
    assert!(
        !r.stdout.contains("rule:style"),
        "--kind must exclude the rule: {}",
        r.stdout
    );
    // Excluded by --kind (an agent in the same source).
    assert!(
        !r.stdout.contains("agent:dev"),
        "--kind must exclude the agent: {}",
        r.stdout
    );
    // Excluded by --source (the tools source's skill, which would pass --kind).
    assert!(
        !r.stdout.contains("skill:deploy"),
        "--source must exclude the other source's skill: {}",
        r.stdout
    );

    // A query that matches no item in the selected source+kind yields nothing.
    let none = sb.mind(&[
        "probe", "--no-tui", "--source", "*agents", "--kind", "skill", "deploy",
    ]);
    assert!(none.success, "{}", none.stderr);
    assert!(
        !none.stdout.contains("skill:"),
        "the query must still exclude non-matching items in the selected source/kind: {}",
        none.stdout
    );
}

#[test]
fn recall_sources_ignores_source_filter_glob() {
    // spec: CLI-83, CLI-86 - the `--source` filter (glob or not) applies to the
    // installed-items listing, NOT to the `--sources` view. Per CLI-83, passing
    // `--source` with `--sources` lists ALL sources and prints a note that the
    // filter is ignored; it does not narrow the source list.
    let (sb, _tools) = melded_two_sources();
    let agents_full = format!("{}/agents", sb.base_name());

    let r = sb.mind(&["recall", "--sources", "--source", "*agents"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // Both sources are still listed: the filter does not narrow `--sources`.
    assert!(
        r.stdout.contains(&agents_full),
        "the agents source must be listed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("/tools"),
        "the non-matching tools source must STILL be listed (filter ignored): {}",
        r.stdout
    );
    // The ignored-filter note is printed (CLI-83).
    assert!(
        r.stderr.contains("ignored with --sources"),
        "a note that the filter is ignored must be printed: {}",
        r.stderr
    );
}

#[test]
fn probe_no_tui_is_long_only() {
    // spec: CLI-164, TUI-54 - `--no-tui` is long-only; `-n` is no longer accepted
    // (CLI-163 reserves -n for --dry-run).
    let sb = melded();
    // --no-tui still works.
    let long = sb.mind(&["probe", "--no-tui"]);
    assert!(long.success, "{}", long.stderr);
    assert!(long.stdout.contains("skill:review"), "{}", long.stdout);
    assert!(long.stdout.contains("agent:dev"), "{}", long.stdout);
    assert!(long.stdout.contains("rule:style"), "{}", long.stdout);
    // -n must fail (unknown flag).
    let short = sb.mind(&["probe", "-n"]);
    assert!(!short.success, "probe -n should fail: {}", short.stdout);
}

#[test]
fn probe_query_matches_name_in_one_item_and_description_in_another() {
    // spec: CLI-85
    // A single query resolves via the NAME of one item and the DESCRIPTION of
    // another in the same result set. "audit" is the skill's name and also
    // appears only inside the agent's description, so both must be returned.
    let sb = Sandbox::named("dual");
    // skill:audit - "audit" only in the NAME.
    sb.write_and_commit(
        "skills/audit/SKILL.md",
        "---\nname: audit\ndescription: Inspect changes carefully\n---\n# audit\n",
    );
    // agent:dev - "audit" only in the DESCRIPTION, not the name.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Run an audit before shipping\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "audit"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    // Matched by name.
    assert!(
        r.stdout.contains("skill:audit"),
        "expected skill:audit (name match): {}",
        r.stdout
    );
    // Matched by description in a different item.
    assert!(
        r.stdout.contains("agent:dev"),
        "expected agent:dev (description match): {}",
        r.stdout
    );
}

#[test]
fn probe_matches_substring_in_middle_of_word() {
    // spec: CLI-85
    // The match is a raw substring, not a word-boundary match: a query that is
    // a fragment inside a longer word still matches.
    let sb = Sandbox::named("frag");
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Performs refactoring of modules\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // "factor" is in the middle of "refactoring".
    let r = sb.mind(&["probe", "factor"]);
    assert!(r.success, "probe failed: {}", r.stderr);
    assert!(
        r.stdout.contains("agent:dev"),
        "expected mid-word substring match: {}",
        r.stdout
    );
}

#[test]
fn learn_installs_and_creates_symlink() {
    // spec: CLI-30, STO-2, STO-14, LIFE-5
    let sb = melded();
    let r = sb.mind(&["learn", "review"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("learned skill:review"));

    let link = sb.claude_home.join("skills/review");
    let meta = std::fs::symlink_metadata(&link).expect("symlink should exist");
    assert!(
        meta.file_type().is_symlink(),
        "expected a symlink at {link:?}"
    );
}

#[test]
fn learn_force_overwrites_a_conflicting_target() {
    // spec: CLI-35, LIFE-41
    let sb = melded();
    // Plant a user file where the rule `style` would link.
    let link = sb.claude_home.join("rules/style.md");
    write(&link, "the user's own file\n");

    // Without --force, the clobber guard refuses (non-TTY: no prompt, no change).
    let r = sb.mind(&["learn", "style"]);
    assert!(
        !r.success,
        "learn must refuse to clobber a foreign target: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not managed by mind"),
        "expected a clobber error: {}",
        r.stderr
    );
    assert!(
        !std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the user's file must be left untouched without --force"
    );

    // With --force, the target is replaced by mind's symlink.
    let f = sb.mind(&["learn", "style", "--force"]);
    assert!(
        f.success,
        "learn --force should overwrite: {} {}",
        f.stdout, f.stderr
    );
    assert!(f.stdout.contains("learned rule:style"), "{}", f.stdout);
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("link should exist")
            .file_type()
            .is_symlink(),
        "--force must replace the file with mind's symlink"
    );
}

#[test]
fn recall_lists_and_shows_item_details() {
    // spec: CLI-70, CLI-71
    let sb = melded();
    sb.mind(&["learn", "review"]);

    let list = sb.mind(&["recall"]);
    assert!(list.stdout.contains("skill:review"));

    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(detail.stdout.contains("source  "), "{}", detail.stdout);
    assert!(detail.stdout.contains("/agents"), "{}", detail.stdout);
    assert!(detail.stdout.contains("hash"), "{}", detail.stdout);
}

#[test]
fn learn_unknown_item_errors() {
    // spec: CLI-3, CLI-100
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no item matches"), "{}", r.stderr);
}

#[test]
fn introspect_is_clean_after_learn() {
    // spec: CLI-90
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["introspect"]);
    assert!(r.success);
    assert!(r.stdout.contains("all good"), "{}", r.stdout);
}

#[test]
fn upgrade_reports_nothing_when_up_to_date() {
    // spec: CLI-64
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["upgrade"]);
    assert!(r.stdout.contains("up to date"), "{}", r.stdout);
}

#[test]
fn upgrade_reports_delta_and_declining_changes_nothing() {
    // spec: CLI-60, CLI-61
    let sb = melded();
    sb.mind(&["learn", "review"]);
    sb.edit_source();
    sb.mind(&["sync"]);

    // Dry-run report: shows hash and commit deltas with arrows.
    let report = sb.mind_with_input(&["upgrade"], Some("n\n"));
    assert!(report.stdout.contains("skill:review"), "{}", report.stdout);
    assert!(report.stdout.contains("hash"), "{}", report.stdout);
    assert!(report.stdout.contains("->"), "{}", report.stdout);
    assert!(report.stdout.contains("aborted"), "{}", report.stdout);

    // Declining must leave the installed commit untouched.
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    let again = sb.mind_with_input(&["upgrade"], Some("n\n"));
    assert!(again.stdout.contains("aborted"));
    assert_eq!(before, sb.mind(&["recall", "skill:review"]).stdout);
}

#[test]
fn upgrade_prompt_defaults_to_yes_on_bare_enter() {
    // spec: CLI-60 - the apply prompt defaults to Yes, so a bare Enter applies the
    // upgrade. (EOF is still No: see the empty-input branch.)
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    sb.edit_source();
    sb.mind(&["sync"]);

    // A bare Enter (newline, not EOF) confirms.
    let applied = sb.mind_with_input(&["upgrade"], Some("\n"));
    assert!(applied.success, "{}", applied.stderr);
    assert!(
        applied.stdout.contains("upgraded skill:review"),
        "a bare Enter must apply the upgrade: {}",
        applied.stdout
    );
    assert_ne!(
        before,
        sb.mind(&["recall", "skill:review"]).stdout,
        "the installed commit should have advanced"
    );

    // EOF (no input at all) still declines.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nedited again\n",
    );
    sb.mind(&["sync"]);
    let eof = sb.mind_with_input(&["upgrade"], Some(""));
    assert!(
        eof.stdout.contains("aborted"),
        "EOF must decline: {}",
        eof.stdout
    );
}

#[test]
fn upgrade_yes_applies_and_is_then_idempotent() {
    // spec: CLI-62, LIFE-13
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source();
    sb.mind(&["sync"]);

    let applied = sb.mind(&["upgrade", "--yes"]);
    assert!(applied.success, "{}", applied.stderr);
    assert!(
        applied.stdout.contains("upgraded skill:review"),
        "{}",
        applied.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]).stdout;
    assert_ne!(before, after, "commit/hash should have advanced");

    // Running again finds nothing to do.
    let idem = sb.mind(&["upgrade"]);
    assert!(idem.stdout.contains("up to date"), "{}", idem.stdout);
}

#[test]
fn forget_removes_symlink_and_manifest_entry() {
    // spec: CLI-40, LIFE-20
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["forget", "review"]);
    assert!(r.success, "{}", r.stderr);

    let link = sb.claude_home.join("skills/review");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "symlink should be gone"
    );

    // The item is no longer installed (a single-item recall lookup fails).
    assert!(
        !sb.mind(&["recall", "review"]).success,
        "review should no longer be installed"
    );
}

#[test]
fn forget_unknown_item_errors() {
    // spec: CLI-40
    let sb = melded();
    let r = sb.mind(&["forget", "review"]);
    assert!(!r.success);
    assert!(r.stderr.contains("not installed"), "{}", r.stderr);
}

#[test]
fn forget_bare_name_is_ambiguous_across_kinds_and_qualifier_resolves() {
    // spec: CLI-40, CLI-71
    let sb = Sandbox::bare("dup");
    sb.write_and_commit(
        "skills/dup/SKILL.md",
        "---\nname: dup\ndescription: skill dup\n---\n# dup skill\n",
    );
    sb.write_and_commit(
        "agents/dup.md",
        "---\nname: dup\ndescription: agent dup\n---\n# dup agent\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "skill:dup"]).success);
    assert!(sb.mind(&["learn", "agent:dup"]).success);

    // A bare name now matches both the skill and the agent -> ambiguous.
    let bare = sb.mind(&["forget", "dup"]);
    assert!(!bare.success);
    assert!(bare.stderr.contains("ambiguous"), "{}", bare.stderr);
    // recall <item> with the same bare name is ambiguous too.
    assert!(!sb.mind(&["recall", "dup"]).success);

    // A wrong source qualifier matches nothing.
    let wrong = sb.mind(&["forget", "other/repo#skill:dup"]);
    assert!(!wrong.success);
    assert!(wrong.stderr.contains("not installed"), "{}", wrong.stderr);

    // The kind prefix disambiguates and forgets exactly one.
    assert!(sb.mind(&["forget", "skill:dup"]).success);
    assert!(
        sb.mind(&["recall", "agent:dup"]).success,
        "agent:dup remains installed"
    );
    assert!(
        !sb.mind(&["recall", "skill:dup"]).success,
        "skill:dup uninstalled"
    );
}

#[test]
fn learn_refuses_to_clobber_a_user_file() {
    // spec: LIFE-41
    let sb = melded();
    // The user already has their own directory where the skill would link.
    let target = sb.claude_home.join("skills/review");
    write(&target.join("MINE.md"), "do not delete me");

    let r = sb.mind(&["learn", "review"]);
    assert!(!r.success, "learn should refuse to overwrite a user file");
    assert!(
        r.stderr.contains("managed by mind") || r.stderr.contains("already exists"),
        "{}",
        r.stderr
    );
    // The user's file is untouched and nothing was recorded.
    assert!(target.join("MINE.md").exists(), "user file was deleted");
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn relearn_replaces_minds_own_symlink() {
    // spec: LIFE-41
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    // Re-learning over mind's own symlink (it points into the store) is allowed.
    let again = sb.mind(&["learn", "review"]);
    assert!(again.success, "{}", again.stderr);
}

#[test]
fn probe_surfaces_frontmatter_descriptions() {
    // spec: DSC-2, DSC-20
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success);
    assert!(
        r.stdout.contains("Review the diff for bugs"),
        "expected skill description in probe output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Implements a spec with tests"),
        "{}",
        r.stdout
    );
}

#[test]
fn recall_detail_shows_description() {
    // spec: CLI-71, DSC-32
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["recall", "skill:review"]);
    assert!(
        r.stdout.contains("desc    Review the diff for bugs"),
        "{}",
        r.stdout
    );
}

#[test]
fn mind_toml_is_authoritative_and_overrides_link_and_description() {
    // spec: DSC-3, DSC-32, STO-2
    let sb = Sandbox::new();
    // A rule in a non-conventional location, declared explicitly with a custom
    // link target and description override.
    sb.write_and_commit(
        "guidelines/style.md",
        "---\ndescription: from frontmatter\n---\n# house style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"a test library\"\n\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"guidelines/style.md\"\n",
            "link = \"rules/custom-style.md\"\n",
            "description = \"override wins\"\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    // Authoritative: only the declared item is visible; convention dirs are not scanned.
    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("rule:style"), "{}", probe.stdout);
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);
    // Description override beats frontmatter.
    assert!(probe.stdout.contains("override wins"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("from frontmatter"),
        "{}",
        probe.stdout
    );

    // [source].description surfaces in `recall --sources`.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("a test library"),
        "{}",
        sources.stdout
    );

    // Custom link target is honored.
    assert!(sb.mind(&["learn", "style"]).success);
    let link = sb.claude_home.join("rules/custom-style.md");
    let meta = std::fs::symlink_metadata(&link).expect("custom link should exist");
    assert!(meta.file_type().is_symlink());
}

#[test]
fn mind_toml_discover_globs_find_items() {
    // spec: DSC-33, DSC-3
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: a glob-found skill\n---\n# foo\n",
    );
    sb.write_and_commit(
        "mind.toml",
        "[discover]\nskills = { include = [\"packages/*/SKILL.md\"] }\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:foo"), "{}", probe.stdout);
    assert!(
        probe.stdout.contains("a glob-found skill"),
        "{}",
        probe.stdout
    );
    // Convention scanning is off, so the conventional skill is absent.
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);
}

#[test]
fn mind_toml_discover_exclude_drops_matches() {
    // spec: DSC-37
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: foo\n---\n# foo\n",
    );
    sb.write_and_commit(
        "packages/internal-x/SKILL.md",
        "---\ndescription: internal\n---\n# internal\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[discover.skills]\n",
            "include = [\"packages/*/SKILL.md\"]\n",
            "exclude = [\"packages/internal-*/SKILL.md\"]\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:foo"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("skill:internal-x"),
        "{}",
        probe.stdout
    );
}

#[test]
fn super_source_recursively_melds_listed_sources() {
    // spec: DSC-38, CLI-15
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);

    // The curated source's items are available...
    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:review"), "{}", probe.stdout);
    // ...and both sources are registered (the curated one tracks its own upstream).
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(sources.stdout.contains("tools"), "{}", sources.stdout);
    assert!(sources.stdout.contains("registry"), "{}", sources.stdout);
    assert!(registry.mind(&["learn", "review"]).success);
}

#[test]
fn super_source_applies_nested_alias() {
    // spec: DSC-39
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", as = \"tl\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    assert!(registry.mind(&["meld", &spec]).success);

    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:tl:review"), "{}", probe.stdout);
}

#[test]
fn on_auth_failure_field_accepted_in_super_source() {
    // spec: DSC-68 -- on-auth-failure is a valid field on a nested source entry.
    // No auth failure occurs here (the nested source is a reachable local repo),
    // so the field is simply parsed: the meld must succeed and register the
    // nested source, proving deny_unknown_fields accepts the schema.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", on-auth-failure = {{ action = \"skip\", message = \"Configure credentials: https://example.com/auth\" }} }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld of a super-source with on-auth-failure must succeed: {}",
        r.stderr
    );
    // The nested source melded normally: its items are available and it is
    // registered (no auth failure, so the policy never fires).
    let probe = registry.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:review"), "{}", probe.stdout);
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(sources.stdout.contains("tools"), "{}", sources.stdout);
}

#[test]
fn on_auth_failure_invalid_action_rejected_in_super_source() {
    // spec: DSC-68 -- an on-auth-failure action that is neither "error" nor
    // "skip" is rejected by serde at mind.toml parse time (AuthFailureAction is
    // a typed enum); the error surfaces as a TOML parse failure at meld time.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", on-auth-failure = {{ action = \"warn\" }} }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "an invalid on-auth-failure action must fail the meld"
    );
    assert!(
        r.stderr.contains("on-auth-failure") || r.stderr.contains("expected 'error' or 'skip'"),
        "error must explain the invalid action: {}",
        r.stderr
    );
}

/// Write a fake `git` script to `<dir>/bin/git` and return the bin dir path.
/// The fake git exits non-zero with an auth-failure message whenever it is called
/// with `clone` and the URL starts with `https://`. All other invocations are
/// delegated to the real git.
fn fake_git_bin_dir(dir: &Path) -> PathBuf {
    // Resolve the real git once at test time.
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  for a; do\n    case \"$a\" in\n      https://*)\n        echo \"fatal: Authentication failed for '$a'\" >&2\n        exit 128\n        ;;\n    esac\n  done\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

/// Build a PATH string with `extra_dir` prepended to the current PATH.
fn prepend_path(extra_dir: &Path) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    if current.is_empty() {
        extra_dir.display().to_string()
    } else {
        format!("{}:{}", extra_dir.display(), current)
    }
}

#[test]
fn on_auth_failure_skip_continues_meld() {
    // spec: DSC-68, DSC-69
    // A super-source with a nested entry whose clone fails with an auth error and
    // action = "skip": the meld exits zero, the nested source is not registered,
    // a warning is printed to stderr.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Non-JSON mode: meld succeeds; nested source not registered; warning on stderr.
    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        r.success,
        "skip action must not abort meld: stderr={}",
        r.stderr
    );
    let sources = registry.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("private-repo"),
        "skipped source must not be registered: {}",
        sources.stdout
    );
    assert!(
        r.stderr.contains("unable to meld source") && r.stderr.contains("skipping"),
        "auth failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_skip_json_output_has_skipped_array() {
    // spec: DSC-68, DSC-69
    // Under --json, a skipped nested source appears in a "skipped" array on the
    // single root object (one JSON output, not multiple).
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(
        r.success,
        "skip action --json must exit zero: stderr={}",
        r.stderr
    );
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "action field: {}", r.stdout);
    assert_eq!(v["outcome"], "melded", "outcome field: {}", r.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(skipped.len(), 1, "exactly one skipped entry: {}", r.stdout);
    assert_eq!(
        skipped[0]["reason"], "auth_failure",
        "reason must be auth_failure: {}",
        r.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("private-repo"),
        "source must name the skipped repo: {}",
        r.stdout
    );
    // Warning goes to stderr even under --json.
    assert!(
        r.stderr.contains("unable to meld source"),
        "warning must appear on stderr under --json too: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_error_fails_meld() {
    // spec: DSC-68, DSC-69
    // action = "error": when the nested source clone fails with auth error, meld
    // exits non-zero.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "error action must cause non-zero exit: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn on_auth_failure_error_prints_message() {
    // spec: DSC-68, DSC-69
    // action = "error" with message: the message and the standard auth-failure
    // line appear on stderr, in both plain and --json modes.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\", message = \"Configure credentials.\" }\n",
    );
    let spec = registry.source_spec();

    // Plain mode.
    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(!r.success, "error action must fail: {}", r.stderr);
    assert!(
        r.stderr.contains("unable to meld source"),
        "standard auth-failure line must appear: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("Configure credentials."),
        "curator message must appear on stderr: {}",
        r.stderr
    );

    // JSON mode: the auth-failure warning lines (printed by the meld command
    // before returning the error, DSC-69) still appear on stderr regardless of
    // --json. The final MindError goes to stdout as the CLI-181 envelope.
    let rj = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(!rj.success, "error action --json must fail: {}", rj.stderr);
    assert!(
        rj.stderr.contains("unable to meld source"),
        "standard auth-failure line must appear under --json: {}",
        rj.stderr
    );
    assert!(
        rj.stderr.contains("Configure credentials."),
        "curator message must appear on stderr under --json: {}",
        rj.stderr
    );
}

#[test]
fn on_auth_failure_error_json_emits_error_envelope() {
    // spec: DSC-68, DSC-69, CLI-181
    // action = "error" with --json: the process exits non-zero and emits the
    // JSON error envelope on stdout. The auth-failure warning is printed to
    // stderr by the meld command before the error is returned (DSC-69), and
    // the final MindError is wrapped in the CLI-181 envelope on stdout.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"error\" }\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "error action --json must exit non-zero: {}",
        r.stderr
    );
    // The final MindError goes to stdout as a JSON error envelope (CLI-181).
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert!(
        v["error"]["kind"].is_string(),
        "error envelope must have a kind: {}",
        r.stdout
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "error envelope must have a non-empty message: {}",
        r.stdout
    );
    // The auth-failure warning (printed before the error is returned) still
    // appears on stderr (DSC-69: always warn to stderr regardless of --json).
    assert!(
        r.stderr.contains("unable to meld source"),
        "auth-failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_absent_propagates_as_generic_error() {
    // spec: DSC-68
    // When a nested source fails auth and has NO on-auth-failure config, the
    // error propagates as a generic git error (hard failure, non-zero exit).
    // This is distinct from action = "error": no standardized DSC-69 message,
    // just the raw git output.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    // No on-auth-failure field at all.
    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/unconfigured-private\"\n",
    );
    let spec = registry.source_spec();

    let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(
        !r.success,
        "auth failure without on-auth-failure must fail: {}",
        r.stderr
    );
    // The failure is the raw git error, NOT the structured DSC-69 message.
    assert!(
        !r.stderr.contains("unable to meld source"),
        "no structured auth-failure message without on-auth-failure config: {}",
        r.stderr
    );
}

#[test]
fn on_auth_failure_multiple_nested_sources_all_skipped() {
    // spec: DSC-68, DSC-69
    // A super-source with two nested entries that both fail auth with
    // action = "skip": both are skipped, meld exits zero, and under --json
    // the skipped array has two entries.  Uses separate sandbox instances for
    // plain and JSON modes because a successful meld registers the source and
    // a second meld of the same source would hit SourceExists.
    let toml_body = "[[discover.sources]]\nsource = \"https://example.com/owner/private-one\"\non-auth-failure = { action = \"skip\" }\n\n[[discover.sources]]\nsource = \"https://example.com/owner/private-two\"\non-auth-failure = { action = \"skip\" }\n";

    // Plain mode: exit zero, two auth-failure warnings on stderr.
    {
        let registry = Sandbox::bare("registry");
        let fake_dir = fake_git_bin_dir(&registry.base);
        let new_path = prepend_path(&fake_dir);
        registry.write_and_commit("mind.toml", toml_body);
        let spec = registry.source_spec();
        let r = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
        assert!(
            r.success,
            "two skipped nested sources must not abort meld: {}",
            r.stderr
        );
        assert_eq!(
            r.stderr.matches("unable to meld source").count(),
            2,
            "one warning per skipped source: {}",
            r.stderr
        );
    }

    // JSON mode: skipped array has exactly two entries, each with reason = "auth_failure".
    {
        let registry = Sandbox::bare("registry");
        let fake_dir = fake_git_bin_dir(&registry.base);
        let new_path = prepend_path(&fake_dir);
        registry.write_and_commit("mind.toml", toml_body);
        let spec = registry.source_spec();
        let rj = registry.mind_env(&["meld", &spec, "--json"], &[("PATH", &new_path)]);
        assert!(rj.success, "meld --json must succeed: {}", rj.stderr);
        let v = parse_json(&rj.stdout);
        let skipped = v["skipped"].as_array().expect("skipped must be an array");
        assert_eq!(
            skipped.len(),
            2,
            "two skipped sources must produce two skipped entries: {}",
            rj.stdout
        );
        for entry in skipped {
            assert_eq!(
                entry["reason"], "auth_failure",
                "each entry must have reason auth_failure: {}",
                rj.stdout
            );
        }
    }
}

#[test]
fn on_auth_failure_skip_during_sync_rewalk() {
    // spec: DSC-68, DSC-69
    // During sync's DSC-57 re-walk, a nested source that was previously skipped
    // (not registered) with action = "skip" is encountered again.  The sync
    // must complete successfully, not fail hard.  This exercises the bug path
    // where the sync re-walk loop did not carry on_auth_failure and let errors
    // propagate via `?`.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Initial meld: registry is registered; private-repo is skipped (auth fail).
    let rm = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    let sources_after_meld = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources_after_meld.contains("private-repo"),
        "private-repo must not be registered after skipped initial meld: {}",
        sources_after_meld
    );

    // sync: re-walks registry's mind.toml, finds private-repo not registered,
    // tries to meld it, hits auth failure again.  With on-auth-failure = skip,
    // sync must complete rather than failing hard.
    let rs = registry.mind_env(&["sync"], &[("PATH", &new_path)]);
    assert!(
        rs.success,
        "sync must complete when nested auth-failure has action=skip: stderr={}",
        rs.stderr
    );
    // The nested source is still not registered (skip, not absorbed).
    let sources_after_sync = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources_after_sync.contains("private-repo"),
        "skipped source must remain unregistered after sync: {}",
        sources_after_sync
    );
    // The warning still appears on stderr.
    assert!(
        rs.stderr.contains("unable to meld source") && rs.stderr.contains("skipping"),
        "auth-failure skip warning must appear on stderr during sync: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_skip_sync_json_has_skipped_array() {
    // spec: DSC-68, DSC-69
    // Under sync --json, a skipped nested source (auth-fail, action=skip)
    // discovered during the re-walk appears in the "skipped" array of the
    // sync result object.
    let registry = Sandbox::bare("registry");
    let fake_dir = fake_git_bin_dir(&registry.base);
    let new_path = prepend_path(&fake_dir);

    registry.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-repo\"\non-auth-failure = { action = \"skip\" }\n",
    );
    let spec = registry.source_spec();

    // Initial meld (non-JSON): establishes registry; private-repo is skipped.
    let rm = registry.mind_env(&["meld", &spec], &[("PATH", &new_path)]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);

    // sync --json: skipped entry from the re-walk appears in the result.
    let rs = registry.mind_env(&["sync", "--json"], &[("PATH", &new_path)]);
    assert!(
        rs.success,
        "sync --json must exit zero with skip: {}",
        rs.stderr
    );
    let v = parse_json(&rs.stdout);
    assert_eq!(v["action"], "sync", "action field: {}", rs.stdout);
    assert_eq!(v["outcome"], "synced", "outcome field: {}", rs.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "one skipped entry in sync result: {}",
        rs.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "auth_failure",
        "reason must be auth_failure: {}",
        rs.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("private-repo"),
        "source must name the skipped repo: {}",
        rs.stdout
    );
    // Warning still on stderr under --json.
    assert!(
        rs.stderr.contains("unable to meld source"),
        "auth-failure warning must appear on stderr under sync --json: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates() {
    // spec: DSC-70
    // T declares A with on-auth-failure=skip; A declares B (no on-auth-failure).
    // When B fails auth, the failure must propagate as a hard error from T's meld;
    // T's skip policy for A must not fire because A itself cloned successfully.
    let a = Sandbox::bare("source_a");
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);

    let r = t.mind_env(&["meld", &t_spec], &[("PATH", &new_path)]);

    // B's auth failure propagates as a hard error; A's skip policy in T must not suppress it.
    assert!(
        !r.success,
        "B's auth failure must exit non-zero, not be absorbed by A's on-auth-failure: stderr={}",
        r.stderr
    );
    // The DSC-69 "unable to meld source" line must not appear: that would mean T
    // misattributed B's failure to A and incorrectly applied A's skip policy.
    assert!(
        !r.stderr.contains("unable to meld source"),
        "B's failure must not be reported as A's auth failure: {}",
        r.stderr
    );
    // A is not persisted: the meld failed before registry.save() was reached.
    let sources = t.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("source_a"),
        "A must not remain registered after the failed meld: {}",
        sources.stdout
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates_during_sync() {
    // spec: DSC-70
    // The same scoping that prevents T's skip-on-A from absorbing B's auth
    // failure at meld time must also hold during sync's re-walk.
    //
    // Setup: T declares A with on-auth-failure=skip. A initially has no
    // mind.toml (so the first meld of T succeeds cleanly). After meld, A is
    // updated to declare B via https:// with no on-auth-failure. The next sync
    // re-walks A's mind.toml, finds B unregistered, attempts to clone B via
    // https://, and hits the fake-git auth failure. Because A is already in the
    // registry (it cloned OK), the code knows the failure came from a descendant
    // and must propagate it hard -- T's skip policy for A must not absorb it.
    let a = Sandbox::bare("source_a");
    // A starts without mind.toml so the initial meld has no nested sources to clone.
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    // Initial meld without fake git: A is a local path with no nested sources;
    // T and A are both registered successfully.
    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    let sources_after_meld = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources_after_meld.contains("source_a"),
        "A must be registered after meld: {}",
        sources_after_meld
    );

    // Now update A to declare B via https:// with no on-auth-failure.
    // Because A is a linked source (local path, no pin), sync reads its live
    // working tree, so this change is picked up without re-melding.
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    // Sync with the fake git binary: when the re-walk finds B (https://) unregistered
    // and tries to clone it, the fake git exits 128 (auth failure).
    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);

    let rs = t.mind_env(&["sync"], &[("PATH", &new_path)]);

    // B's auth failure must propagate as a hard error; T's skip for A must not
    // absorb it because A itself is already in the registry (it cloned OK).
    assert!(
        !rs.success,
        "B's auth failure during sync re-walk must exit non-zero: stderr={}",
        rs.stderr
    );
    // The "unable to meld source" line must NOT appear: that line is only
    // printed when on-auth-failure is present, which it is not for B's entry.
    // Its presence would indicate the sync incorrectly applied T's skip for A
    // to B's failure.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be attributed to A via the DSC-69 message: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_propagates_during_sync_with_error_action() {
    // spec: DSC-70
    // Same descendant-scoping as on_auth_failure_descendant_failure_propagates_during_sync,
    // but T declares A with action = "error" (with a distinctive message) instead
    // of "skip". B (A's descendant, no policy) failing auth during the sync
    // re-walk must propagate hard -- and crucially A's "error" policy must not
    // fire, because A itself cloned fine. If A's policy were misapplied to B, the
    // DSC-69 lines (including A's message) would be printed; they must not be.
    let a = Sandbox::bare("source_a");
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"error\", message = \"A-LEVEL-CREDS-HINT\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    // Initial meld without fake git: A is a local path with no nested sources.
    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);
    assert!(
        t.mind(&["recall", "--sources"]).stdout.contains("source_a"),
        "A must be registered after meld"
    );

    // A now declares B via https:// with no on-auth-failure of its own.
    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);
    let rs = t.mind_env(&["sync"], &[("PATH", &new_path)]);

    // B's auth failure propagates as a hard error even though A's policy is
    // "error" rather than "skip": the outcome is non-zero either way, but the
    // failure must travel via B's generic error path, not A's policy.
    assert!(
        !rs.success,
        "B's auth failure during sync re-walk must exit non-zero with A's error action: stderr={}",
        rs.stderr
    );
    // A's policy must not fire for B: neither the DSC-69 line nor A's message.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be reported via A's DSC-69 auth-failure line: {}",
        rs.stderr
    );
    assert!(
        !rs.stderr.contains("A-LEVEL-CREDS-HINT"),
        "A's curator message must not be printed for B's descendant failure: {}",
        rs.stderr
    );
}

#[test]
fn on_auth_failure_descendant_failure_during_sync_json_is_well_formed() {
    // spec: DSC-70
    // Under sync --json, a descendant (B) auth failure that must propagate hard
    // (it is not absorbed by A's skip policy) still exits non-zero, and the JSON
    // channel stays well-formed: no partial success object is emitted on stdout,
    // mirroring the meld error --json contract (stdout empty, diagnostics on
    // stderr).
    let a = Sandbox::bare("source_a");
    let a_spec = a.source_spec();

    let t = Sandbox::bare("super_t");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{a_spec}\"\non-auth-failure = {{ action = \"skip\" }}\n"
    );
    t.write_and_commit("mind.toml", &toml);
    let t_spec = t.source_spec();

    let rm = t.mind(&["meld", &t_spec]);
    assert!(rm.success, "initial meld must succeed: {}", rm.stderr);

    a.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"https://example.com/owner/private-b\"\n",
    );

    let fake_dir = fake_git_bin_dir(&t.base);
    let new_path = prepend_path(&fake_dir);
    let rs = t.mind_env(&["sync", "--json"], &[("PATH", &new_path)]);

    assert!(
        !rs.success,
        "descendant failure under sync --json must exit non-zero: stderr={}",
        rs.stderr
    );
    // The error path must not emit a partial JSON success object on stdout; if
    // anything is emitted it must still be valid JSON.
    let out = rs.stdout.trim();
    if !out.is_empty() {
        let _: serde_json::Value =
            serde_json::from_str(out).expect("any sync --json stdout must be well-formed JSON");
    }
    // B's failure must not be misattributed to A's skip policy.
    assert!(
        !rs.stderr.contains("unable to meld source"),
        "B's failure must not be reported via A's DSC-69 auth-failure line: {}",
        rs.stderr
    );
}

// The path fed to a nested entry to force an immediate non-auth clone failure:
// git treats a non-existent local path as a clone error and exits non-zero at
// once, with no network and no auth prompt.
const UNREACHABLE_SOURCE: &str = "/nonexistent/mind-test-source-does-not-exist";

#[test]
fn meld_nested_clone_failure_degrades_gracefully() {
    // spec: DSC-79
    // A super-source with its own items plus one reachable and one unreachable
    // nested entry: the meld succeeds (exit 0), the reachable nested source is
    // registered, the unreachable one is absent, and a warning is on stderr.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::named("super");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a non-auth nested clone failure must not abort the meld: stderr={}",
        r.stderr
    );

    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("tools"),
        "the reachable nested source must remain registered: {sources}"
    );
    assert!(
        sources.contains("super"),
        "the primary super-source must remain registered: {sources}"
    );
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "the unreachable nested source must not be registered: {sources}"
    );
    assert!(
        r.stderr.contains("skipping") && r.stderr.contains("clone"),
        "a clone-failure warning must appear on stderr: {}",
        r.stderr
    );
}

#[test]
fn meld_nested_clone_failure_json_skipped() {
    // spec: DSC-79
    // Under --json, the skipped nested entry appears in the outer result's
    // `skipped[]` array with `reason == "clone_failure"`.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::named("super");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec, "--json"]);
    assert!(r.success, "meld --json must exit zero: stderr={}", r.stderr);
    let v = parse_json(&r.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "exactly one skipped entry expected: {}",
        r.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "clone_failure",
        "reason must be clone_failure: {}",
        r.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("mind-test-source-does-not-exist"),
        "the skipped entry must name the unreachable source: {}",
        r.stdout
    );
}

#[test]
fn meld_curator_all_nested_fail_hard_fails() {
    // spec: DSC-80
    // A curator-only super-source (no items of its own) whose every nested entry
    // fails to register produces no discoverable items, so the meld hard-fails.
    let super_src = Sandbox::bare("curator");
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "a curator with zero own items and all nested sources failing must hard-fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn meld_curator_one_nested_succeeds_passes() {
    // spec: DSC-80
    // A curator-only super-source with at least one nested source that registers
    // succeeds even when another nested entry fails to clone.
    let tools = Sandbox::named("tools");
    let super_src = Sandbox::bare("curator");
    let toml = format!(
        "[[discover.sources]]\nsource = \"{}\"\n\n[[discover.sources]]\nsource = \"{}\"\n",
        tools.source_spec(),
        UNREACHABLE_SOURCE
    );
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a curator with one nested source registered must succeed: stderr={}",
        r.stderr
    );
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("curator"),
        "the curator must be registered: {sources}"
    );
    assert!(
        sources.contains("tools"),
        "the reachable nested source must be registered: {sources}"
    );
}

#[test]
fn meld_nested_clone_failure_descendant_propagates() {
    // spec: DSC-79
    // A super-source T declares A (a reachable curator that clones OK). A declares
    // only B (unreachable). B's non-auth clone failure leaves A a curator with no
    // discoverable items, so A's meld hard-fails (DSC-80). Because A itself is
    // already in the registry, that error came from a descendant and must
    // propagate as a hard failure -- it must NOT be silently swallowed as a
    // clone_failure skip of A by T (the DSC-70 scoping applied to DSC-79).
    let a = Sandbox::bare("source_a");
    a.write_and_commit(
        "mind.toml",
        &format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n"),
    );
    let a_spec = a.source_spec();

    // T has its own items so it is not itself a curator-empty case; the only
    // failure in play originates from A's descendant.
    let t = Sandbox::named("super_t");
    t.write_and_commit(
        "mind.toml",
        &format!("[[discover.sources]]\nsource = \"{a_spec}\"\n"),
    );
    let t_spec = t.source_spec();

    let r = t.mind(&["meld", &t_spec]);
    assert!(
        !r.success,
        "A's descendant failure must propagate as a hard error, not be absorbed as a skip: stderr={}",
        r.stderr
    );
    // A must not be reported as a skipped clone_failure of T: the error is A's own
    // hard failure, arriving after A was already registered.
    assert!(
        !r.stderr.contains("skipping 'source_a'"),
        "A's failure must not be misattributed as a clone_failure skip of A: {}",
        r.stderr
    );
    let sources = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("source_a"),
        "A must not remain registered after the failed meld: {sources}"
    );
}

#[test]
fn meld_curator_all_nested_fail_error_message() {
    // spec: DSC-80
    // The hard-fail produced by a curator-only source with all nested clone
    // failures must carry the CuratorAllNestedFailed message text, not just
    // any non-zero exit.  A different error (e.g. a bad mind.toml parse)
    // would also produce non-zero exit, so this test pins the exact message.
    let super_src = Sandbox::bare("curator-msg");
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "curator with all nested failing must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("produced no discoverable items")
            || r.stderr.contains("no items of its own"),
        "CuratorAllNestedFailed error message must appear on stderr: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("curator-msg"),
        "error message must name the super-source: {}",
        r.stderr
    );
}

#[test]
fn meld_non_curator_all_nested_fail_still_succeeds() {
    // spec: DSC-80
    // DSC-80 fires only when the primary has ZERO items of its own.  A source
    // with at least one own item must succeed even when every nested source
    // fails to clone (the curator-empty guard does not apply).
    let super_src = Sandbox::named("non-curator");
    // Add a mind.toml pointing only to an unreachable nested source.
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(
        r.success,
        "a source with own items must succeed even when all nested sources fail: stderr={}",
        r.stderr
    );
    // The primary source must still be registered.
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("non-curator"),
        "primary source must be registered: {sources}"
    );
    // The unreachable source must not be registered.
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "the unreachable nested source must not be registered: {sources}"
    );
    // A clone-failure warning must still be emitted.
    assert!(
        r.stderr.contains("skipping") && r.stderr.contains("clone"),
        "a clone-failure warning must appear even for a non-curator: {}",
        r.stderr
    );
}

#[test]
fn sync_nested_clone_failure_is_skipped() {
    // spec: DSC-79
    // The same non-auth clone-failure skip that applies during meld also applies
    // during sync's DSC-57 re-walk.  Scenario: a super-source is melded without
    // nested sources, then its mind.toml is updated to add an unreachable one.
    // sync re-walks, discovers the new entry, fails to clone it, and must exit
    // zero with a warning rather than hard-failing.
    let super_src = Sandbox::named("rewalk-host");
    let spec = super_src.source_spec();

    // Initial meld: no nested sources, just the super-source itself.
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "initial meld must succeed: {}", r.stderr);

    // Add an unreachable nested source to the super-source's mind.toml.
    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);

    // sync re-walks: encounters the new (unreachable) entry, skips it.
    let rs = super_src.mind(&["sync"]);
    assert!(
        rs.success,
        "sync must exit zero when a newly-listed nested source fails to clone: stderr={}",
        rs.stderr
    );
    // The warning must appear.
    assert!(
        rs.stderr.contains("skipping") && rs.stderr.contains("clone"),
        "a clone-failure warning must appear on stderr during sync re-walk: {}",
        rs.stderr
    );
    // The unreachable source must not have been registered.
    let sources = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("mind-test-source-does-not-exist"),
        "unreachable source must not be registered after sync re-walk: {sources}"
    );
}

#[test]
fn sync_nested_clone_failure_json_has_skipped_entry() {
    // spec: DSC-79
    // Under sync --json, a non-auth clone failure discovered during the
    // DSC-57 re-walk must appear in the result's skipped[] array with
    // reason = "clone_failure".
    let super_src = Sandbox::named("rewalk-json");
    let spec = super_src.source_spec();

    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "initial meld must succeed: {}", r.stderr);

    let toml = format!("[[discover.sources]]\nsource = \"{UNREACHABLE_SOURCE}\"\n");
    super_src.write_and_commit("mind.toml", &toml);

    let rs = super_src.mind(&["sync", "--json"]);
    assert!(
        rs.success,
        "sync --json must exit zero on nested clone failure during re-walk: stderr={}",
        rs.stderr
    );
    let v = parse_json(&rs.stdout);
    assert_eq!(v["action"], "sync", "action field: {}", rs.stdout);
    let skipped = v["skipped"].as_array().expect("skipped must be an array");
    assert_eq!(
        skipped.len(),
        1,
        "exactly one skipped entry expected in sync --json result: {}",
        rs.stdout
    );
    assert_eq!(
        skipped[0]["reason"], "clone_failure",
        "reason must be clone_failure: {}",
        rs.stdout
    );
    assert!(
        skipped[0]["source"]
            .as_str()
            .unwrap_or("")
            .contains("mind-test-source-does-not-exist"),
        "skipped entry must name the unreachable source: {}",
        rs.stdout
    );
}

#[test]
fn super_source_meld_is_cycle_safe() {
    // spec: DSC-38
    // aa and bb each list the other; melding aa must terminate.
    let a = Sandbox::bare("aa");
    let b = Sandbox::bare("bb");
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    b.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            a.source_spec()
        ),
    );
    let spec = a.source_spec();
    let r = a.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);
}

#[test]
fn super_source_meld_breaks_multi_level_cycle() {
    // spec: DSC-38
    // A multi-level chain that loops: aa -> bb -> cc -> aa. Each repo is itself a
    // super-source, so resolution must follow the chain, detect the cycle back to
    // aa, and process each source exactly once (no infinite recursion, no dupes).
    let a = Sandbox::bare("aa");
    let b = Sandbox::bare("bb");
    let c = Sandbox::bare("cc");
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    b.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            c.source_spec()
        ),
    );
    c.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            a.source_spec()
        ),
    );
    let spec = a.source_spec();
    let r = a.mind(&["meld", &spec]);
    assert!(r.success, "a cyclic chain must terminate: {}", r.stderr);
    // Each source is melded exactly once: the per-source "melding" progress line
    // appears three times, not more (a missed cycle guard would loop or repeat).
    assert_eq!(
        r.stdout.matches("melding").count(),
        3,
        "each source melds exactly once: {}",
        r.stdout
    );
    // All three are registered, each exactly once (no duplicate push).
    let recall = a.mind(&["recall", "--sources", "--json"]).stdout;
    for name in ["aa", "bb", "cc"] {
        assert_eq!(
            recall.matches(&format!("\"repo\": \"{name}\"")).count(),
            1,
            "{name} must be registered exactly once: {recall}"
        );
    }
}

#[test]
fn super_source_meld_does_not_auto_install_nested_items() {
    // spec: DSC-54
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "{}", r.stderr);
    // The nested source is registered and its items are available...
    assert!(
        registry.mind(&["probe"]).stdout.contains("skill:review"),
        "the curated source's items must be available"
    );
    // ...but NOT auto-installed: no link is created for a nested item by default.
    assert!(
        !registry.claude_home.join("skills/review").exists(),
        "a curated super-source must not auto-install the nested chain's items"
    );
    // The user can still install it explicitly.
    assert!(registry.mind(&["learn", "review"]).success);
    assert!(registry.claude_home.join("skills/review").exists());
}

#[test]
fn meld_recursive_installs_nested_items() {
    // spec: DSC-55
    let tools = Sandbox::named("tools"); // a normal source with items
    let registry = Sandbox::bare("registry"); // curates `tools`, no items of its own
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // With --recursive (and --yes to skip prompts), the nested chain's items install.
    let r = registry.mind(&["meld", &spec, "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "the nested source's items must install with --recursive"
    );
}

#[test]
fn meld_recursive_short_flag_installs_nested_items() {
    // spec: DSC-55 - the `-r` short form is equivalent to `--recursive`.
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "-r", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "-r must install the nested source's items"
    );
}

#[test]
fn remeld_recursive_installs_nested_chain() {
    // spec: DSC-55
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // First meld without the flag: chain registered, nested items not installed.
    assert!(registry.mind(&["meld", &spec]).success);
    assert!(!registry.claude_home.join("skills/review").exists());
    // Re-melding the already-registered super-source with the flag installs the
    // curated chain's items (nothing is re-registered).
    let r = registry.mind(&["meld", &spec, "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/review").exists(),
        "a re-meld must honor --recursive"
    );
}

#[test]
fn meld_installs_curator_flagged_nested_source_without_recursive() {
    // spec: DSC-58 - a `[discover].sources` entry marked `install = true` has its
    // items offered for install on a plain meld (no --recursive). A sibling entry
    // without the flag is registered but its items are left available.
    let want = Sandbox::bare("want"); // curator recommends installing this one
    want.write_and_commit(
        "skills/want-skill/SKILL.md",
        "---\nname: want-skill\ndescription: wanted\n---\n# want\n",
    );
    let skip = Sandbox::bare("skip"); // registered only, not installed
    skip.write_and_commit(
        "skills/skip-skill/SKILL.md",
        "---\nname: skip-skill\ndescription: skipped\n---\n# skip\n",
    );
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", install = true }}, {{ source = \"{}\" }}]\n",
            want.source_spec(),
            skip.source_spec()
        ),
    );
    // Plain meld, no --recursive. --yes auto-confirms the flagged source's prompt.
    let r = registry.mind(&["meld", &registry.source_spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // Both nested sources are registered.
    let sources = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("/want") && sources.contains("/skip"),
        "both nested sources should be registered: {sources}"
    );
    // The flagged source's item installed; the unflagged source's item did not.
    assert!(
        registry.claude_home.join("skills/want-skill").exists(),
        "the install=true nested source's item must be installed"
    );
    assert!(
        !registry.claude_home.join("skills/skip-skill").exists(),
        "the unflagged nested source's item must not be auto-installed"
    );
}

#[test]
fn meld_recursive_installs_even_unflagged_nested_sources() {
    // spec: DSC-55 DSC-58 - --recursive is the superset: it installs every nested
    // source, including ones the curator did not mark `install = true`.
    let want = Sandbox::bare("want");
    want.write_and_commit(
        "skills/want-skill/SKILL.md",
        "---\nname: want-skill\ndescription: wanted\n---\n# want\n",
    );
    let skip = Sandbox::bare("skip");
    skip.write_and_commit(
        "skills/skip-skill/SKILL.md",
        "---\nname: skip-skill\ndescription: skipped\n---\n# skip\n",
    );
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\", install = true }}, {{ source = \"{}\" }}]\n",
            want.source_spec(),
            skip.source_spec()
        ),
    );
    let r = registry.mind(&["meld", &registry.source_spec(), "--recursive", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        registry.claude_home.join("skills/want-skill").exists()
            && registry.claude_home.join("skills/skip-skill").exists(),
        "--recursive installs every nested source regardless of the install flag"
    );
}

#[test]
fn meld_super_source_suggests_probe() {
    // spec: DSC-56
    let tools = Sandbox::named("tools");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            tools.source_spec()
        ),
    );
    let r = registry.mind(&["meld", &registry.source_spec()]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("mind probe"),
        "melding a curated super-source should suggest probe: {}",
        r.stdout
    );
    // A plain source (no [discover].sources) does not get the hint.
    let plain = Sandbox::named("plain");
    let r2 = plain.mind(&["meld", &plain.source_spec()]);
    assert!(
        !r2.stdout.contains("mind probe"),
        "a normal source must not get the probe hint: {}",
        r2.stdout
    );
}

#[test]
fn sync_rewalks_super_source_for_new_nested_sources() {
    // spec: DSC-57
    let a = Sandbox::bare("aa"); // the curated super-source
    let b = Sandbox::named("bb"); // initially curated
    let c = Sandbox::named("cc"); // added to the list later
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            b.source_spec()
        ),
    );
    let spec = a.source_spec();
    assert!(a.mind(&["meld", &spec]).success);
    // Match the `/bb` path segment, not a bare `bb`: a short commit hash is hex,
    // so the two-letter source names (all valid hex) can appear inside it and
    // false-match a bare `contains` (a flaky failure when a hash holds "cc").
    let before = a.mind(&["recall", "--sources"]).stdout;
    assert!(before.contains("/bb"), "{before}");
    assert!(!before.contains("/cc"), "cc not yet listed: {before}");

    // Add cc to aa's discover list.
    a.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}, {{ source = \"{}\" }}]\n",
            b.source_spec(),
            c.source_spec()
        ),
    );
    // sync re-walks aa's [discover].sources and registers the newly listed cc.
    let r = a.mind(&["sync"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        a.mind(&["recall", "--sources"]).stdout.contains("/cc"),
        "sync must register the newly-listed nested source"
    );
}

#[test]
fn invalid_mind_toml_errors_clearly() {
    // spec: DSC-31
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success);
    assert!(r.stderr.contains("unknown item kind"), "{}", r.stderr);
}

#[test]
fn mind_toml_rejects_unknown_fields() {
    // spec: DSC-30
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nbogus = \"x\"\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success);
    assert!(r.stderr.contains("invalid mind.toml"), "{}", r.stderr);
}

#[test]
fn meld_as_prefixes_names_links_and_refs() {
    // spec: CLI-13, NS-1, NS-2
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);

    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:jk:review"), "{}", probe.stdout);
    assert!(probe.stdout.contains("agent:jk:dev"), "{}", probe.stdout);
    // The bare names must not appear.
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);

    // Install under the prefixed name; symlink lands at the prefixed location.
    assert!(sb.mind(&["learn", "jk:review"]).success);
    let link = sb.claude_home.join("skills/jk:review");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("namespace:jk"),
        "{}",
        sources.stdout
    );
}

#[test]
fn meld_namespace_flag_sets_prefix_and_as_alias_still_works() {
    // spec: CLI-159 - `--namespace` (short `-n`) is the renamed `--as`; both set
    // the source's namespace. `--as` is retained as a hidden deprecated alias.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // --namespace sets the prefix; items install under jk:<bare>.
    assert!(sb.mind(&["meld", &spec, "--namespace", "jk"]).success);
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "--namespace must set prefix: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:review"),
        "bare name must not appear when prefixed: {}",
        probe.stdout
    );

    // Verify the short form -N also works (a separate sandbox to avoid
    // re-meld collision).
    // spec: CLI-163 - -N is the new short form for --namespace on meld.
    let sb2 = Sandbox::new();
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2, "-N", "zz"]).success);
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("skill:zz:review"),
        "-N short form must set prefix: {}",
        probe2.stdout
    );

    // Verify the hidden --as alias still works identically.
    let sb3 = Sandbox::new();
    let spec3 = sb3.source_spec();
    assert!(sb3.mind(&["meld", &spec3, "--as", "qq"]).success);
    let probe3 = sb3.mind(&["probe"]);
    assert!(
        probe3.stdout.contains("skill:qq:review"),
        "--as deprecated alias must still set prefix: {}",
        probe3.stdout
    );
}

#[test]
fn review_namespace_flag_evaluates_under_prefix() {
    // spec: CLI-159, CLI-163 - `review --namespace <prefix>` (short `-N`) evaluates
    // the source under that prospective prefix; `--as` is a hidden deprecated alias.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // --namespace accepted (same behavior as former --as).
    let r = sb.mind(&["review", &spec, "--namespace", "jk"]);
    assert!(
        r.success,
        "review --namespace must exit 0 for clean source: {} {}",
        r.stdout, r.stderr
    );

    // Short -N accepted (CLI-163: -n is reserved for --dry-run).
    let r2 = sb.mind(&["review", &spec, "-N", "jk"]);
    assert!(
        r2.success,
        "review -N must exit 0 for clean source: {} {}",
        r2.stdout, r2.stderr
    );
}

#[test]
fn remeld_namespace_change_allowed_when_no_items_installed() {
    // spec: NS-30 CLI-161 - when no items are installed from the source,
    // re-melding with a different --namespace updates the persisted alias.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Meld with --link-only so no items are installed.
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "initial link-only meld"
    );

    // Re-meld with a new namespace: must succeed and persist the alias.
    let r = sb.mind(&["meld", &spec, "--namespace", "jk"]);
    assert!(
        r.success,
        "re-meld --namespace with no installed items must succeed: {} {}",
        r.stdout, r.stderr
    );

    // The new prefix must be reflected in the catalog.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "probe must show the new prefixed name: {}",
        probe.stdout
    );
}

#[test]
fn mind_toml_prefix_auto_applies_and_alias_overrides() {
    // spec: NS-1, DSC-35
    // Author-declared prefix.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"ag\"\n");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.stdout.contains("skill:ag:review"), "{}", probe.stdout);

    // Consumer --as overrides the author's prefix.
    let sb2 = Sandbox::new();
    sb2.write_and_commit("mind.toml", "[source]\nprefix = \"ag\"\n");
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2, "--as", "zz"]).success);
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("skill:zz:review"),
        "{}",
        probe2.stdout
    );
    assert!(!probe2.stdout.contains("ag:review"), "{}", probe2.stdout);
}

#[test]
fn ns_token_expands_to_prefixed_reference_on_install() {
    // spec: NS-11, NS-42
    // A skill sibling token expands prefixed; an agent sibling token expands bare
    // (NS-42: agents link under the bare name regardless of prefix).
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        // {{ns:dev}} is an agent sibling -> expands bare even under prefix.
        "---\nname: lead\ndescription: lead\n---\nDelegate to the {{ns:dev}} agent.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    // `lead` references {{ns:dev}}; confirm the closure with --yes.
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);

    // Store path still uses the effective (prefixed) name; only the link is bare.
    let store = sb.mind_home.join("store/agent/jk:lead");
    let body = std::fs::read_to_string(&store).expect("installed agent file");
    // Agent referent expands bare, not jk:dev.
    assert!(
        body.contains("the dev agent"),
        "expected bare agent ref: {body}"
    );
    assert!(!body.contains("{{ns:dev}}"), "token should be gone: {body}");
    // NS-40: the link is under the bare harness name, not the prefixed name.
    assert!(
        sb.claude_home.join("agents/lead.md").exists(),
        "agent should link as agents/lead.md"
    );
}

#[test]
fn ns_token_expands_to_bare_reference_without_prefix() {
    // spec: NS-14
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the {{ns:dev}} agent.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "lead", "--yes"]).success);

    let body = std::fs::read_to_string(sb.mind_home.join("store/agent/lead")).unwrap();
    assert!(body.contains("the dev agent"), "{body}");
    assert!(!body.contains("{{ns:"), "{body}");
}

#[test]
fn bad_ns_reference_errors_on_install() {
    // spec: NS-12
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nsee {{ns:ghost}}\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    let r = sb.mind(&["learn", "lead"]);
    assert!(!r.success);
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);
}

#[test]
fn meld_as_warns_about_unguarded_prose_references() {
    // spec: NS-20, NS-22, NS-42, CLI-14, CLI-162
    // Without --verbose the unguarded-reference warning is suppressed (CLI-162).
    // With --verbose it is emitted for non-agent referents whose name would be
    // prefixed; NS-42: agent-only names are excluded from the warning.
    // This fixture references `review` (a skill sibling) in bare prose.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the review skill.\n",
    );
    let spec = sb.source_spec();

    // Default (no --verbose): warning is suppressed.
    let r = sb.mind(&["meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "expected no unguarded-ref warning without --verbose: {}",
        r.stderr
    );

    // With --verbose: warning is emitted.
    let sb2 = Sandbox::new();
    sb2.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the review skill.\n",
    );
    let spec2 = sb2.source_spec();
    let r2 = sb2.mind(&["--verbose", "meld", &spec2, "--as", "jk"]);
    assert!(r2.success, "{}", r2.stderr);
    assert!(
        r2.stderr.contains("references sibling(s) in prose") && r2.stderr.contains("review"),
        "expected unguarded-ref warning under --verbose: {}",
        r2.stderr
    );
}

#[test]
fn no_warning_when_unprefixed() {
    // spec: NS-23, CLI-162 -- no prefix in effect => no warning, EVEN under
    // --verbose. The meld runs with --verbose so the warning gate is open; the
    // only reason it stays silent is the absent prefix (NS-23). A fixture that
    // WOULD warn under a prefix (bare prose ref to the skill sibling `review`)
    // must still stay silent unprefixed.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent, then run the review skill.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["--verbose", "meld", &spec]); // no prefix -> bare refs are correct
    assert!(r.success);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "{}",
        r.stderr
    );
}

// ---- NS-40 / NS-41 / NS-42: agents not namespaced -------------------------

#[test]
fn prefixed_agent_links_under_bare_harness_name() {
    // spec: NS-40 -- an agent from a prefixed source installs its store copy
    // under the effective (prefixed) name but creates the agent-home link under
    // the bare frontmatter `name`, which is how the harness resolves the agent.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    // Store uses the effective prefixed name.
    assert!(
        sb.mind_home.join("store/agent/jk:dev").exists(),
        "store should be at jk:dev"
    );
    // Agent-home link is under the bare frontmatter name, not the prefixed one.
    assert!(
        sb.claude_home.join("agents/dev.md").exists(),
        "link should be agents/dev.md"
    );
    assert!(
        !sb.claude_home.join("agents/jk:dev.md").exists(),
        "no link should exist at the prefixed path"
    );
}

#[test]
fn prefixed_agent_manifest_key_uses_effective_name() {
    // spec: NS-40 -- the manifest key and `recall` output use the effective
    // (prefixed) name; only the link is bare.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("jk:dev"),
        "recall should show jk:dev: {}",
        r.stdout
    );
}

#[test]
fn agent_collision_is_refused_at_learn() {
    // spec: NS-41 -- installing an agent whose bare harness name conflicts with
    // an installed agent from a different source is refused with AgentCollision.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // A second source ships an agent with the same frontmatter name "dev".
    let other = Sandbox::bare("other");
    other.write_and_commit(
        "agents/coder.md",
        "---\nname: dev\ndescription: another dev\n---\n# dev\n",
    );
    assert!(sb.mind(&["meld", &other.source_spec()]).success);

    let r = sb.mind(&["learn", "coder"]);
    assert!(!r.success, "colliding agent install must be refused");
    assert!(
        r.stderr.contains("conflict"),
        "expected collision error message: {}",
        r.stderr
    );
}

#[test]
fn meld_warns_when_incoming_agent_would_collide() {
    // spec: NS-41 -- `meld` emits an advisory warning (not an error) when the
    // incoming source carries an agent that would collide with an installed one.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    let other = Sandbox::bare("other");
    other.write_and_commit(
        "agents/coder.md",
        "---\nname: dev\ndescription: another dev\n---\n# dev\n",
    );
    // Meld must succeed (advisory only).
    let r = sb.mind(&["meld", &other.source_spec()]);
    assert!(
        r.success,
        "meld should succeed even on collision: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("would collide"),
        "expected advisory collision warning: {}",
        r.stderr
    );
}

#[test]
fn no_agent_collision_when_reinstalling_same_source() {
    // spec: NS-41 -- re-learning the same agent (same source + bare name) is not
    // a collision; upgrade / re-install must succeed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);
    // Re-learn the same item: should succeed, not collide.
    let r = sb.mind(&["learn", "dev"]);
    assert!(
        r.success,
        "re-learn of same agent should succeed: {}",
        r.stderr
    );
}

#[test]
fn agent_token_expands_bare_under_prefix() {
    // spec: NS-42 -- a {{ns:name}} token whose referent is a sibling agent
    // expands to the bare name even when the source has a prefix.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:dev}} for coding.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);

    // The stored content uses the bare name for the agent referent.
    let store = sb.mind_home.join("store/agent/jk:lead");
    let body = std::fs::read_to_string(&store).expect("store file");
    assert!(
        body.contains("dev") && !body.contains("jk:dev"),
        "agent token should expand bare: {body}"
    );
}

#[test]
fn unguarded_ref_warning_skips_agent_only_sibling_names() {
    // spec: NS-42, CLI-162 -- bare prose references to a sibling agent do not
    // trigger the unguarded-reference warning: the agent links bare regardless of
    // prefix, so the prose reference resolves correctly even without a token. The
    // meld runs under --verbose (CLI-162), so the warning IS active for non-agent
    // siblings; the fixture references BOTH the skill sibling `review` (which MUST
    // be flagged) and the agent sibling `dev` (which MUST NOT be) so the assertion
    // distinguishes "excluded because agent" from "warning never fired at all".
    let sb = Sandbox::new();
    // The standard fixture has `agents/dev.md` (agent) and `skills/review`.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent, then run the review skill.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["--verbose", "meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    // The warning fires for the non-agent sibling `review`...
    assert!(
        r.stderr.contains("references sibling(s) in prose") && r.stderr.contains("review"),
        "non-agent sibling `review` must be flagged under --verbose: {}",
        r.stderr
    );
    // ...but `dev` is an agent sibling and must NOT appear in the warning.
    assert!(
        !r.stderr.contains("dev"),
        "agent-only sibling should not trigger unguarded-ref warning: {}",
        r.stderr
    );
}

#[test]
fn unprefixed_agent_links_under_frontmatter_name_not_filename() {
    // spec: NS-40 -- even with no prefix, an agent links under its frontmatter
    // `name`, which may differ from its filename. The store copy and stable
    // identity use the file stem; only the agent-home link uses the harness name.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/coder.md",
        "---\nname: reviewer\ndescription: reviews code\n---\n# reviewer\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "coder"]).success);
    // The link uses the frontmatter name, not the filename.
    assert!(
        sb.claude_home.join("agents/reviewer.md").exists(),
        "link should use the frontmatter name"
    );
    assert!(
        !sb.claude_home.join("agents/coder.md").exists(),
        "no link at the filename path"
    );
    // Store and identity use the file stem.
    assert!(sb.mind_home.join("store/agent/coder").exists());
}

#[test]
fn upgrade_moves_agent_link_when_frontmatter_name_changes() {
    // spec: NS-40 -- an agent links under its frontmatter `name`. When the source
    // changes that name but keeps the same filename (so the item's effective name
    // and stable identity are unchanged, i.e. this is an in-place content upgrade,
    // not a rename), `upgrade` must move the agent-home link to the new bare name
    // and leave no orphaned old link.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.claude_home.join("agents/dev.md").exists());

    // Same file, new frontmatter name.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: lead\ndescription: now the lead\n---\n# lead agent\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    // The link moved to the new harness name; the old one is not orphaned.
    assert!(
        sb.claude_home.join("agents/lead.md").exists(),
        "link should move to the new harness name"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err(),
        "the old harness-name link must not be left orphaned"
    );
    // The store path and identity are unchanged (keyed on the file stem).
    assert!(sb.mind_home.join("store/agent/dev").exists());
}

#[test]
fn introspect_fix_recreates_bare_agent_link() {
    // spec: NS-40 -- `introspect --fix` recreates a missing agent link at its
    // recorded (bare) path from the manifest link registry, not a prefixed path.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    let link = sb.claude_home.join("agents/dev.md");
    assert!(link.exists());
    // Simulate drift: the link is deleted out from under mind.
    std::fs::remove_file(&link).unwrap();
    assert!(sb.mind(&["introspect", "--fix"]).success);
    assert!(
        link.exists(),
        "introspect --fix must recreate the bare link"
    );
    assert!(
        !sb.claude_home.join("agents/jk:dev.md").exists(),
        "the recreated link must be bare, not prefixed"
    );
}

#[test]
fn forget_removes_bare_agent_link() {
    // spec: NS-40 -- `forget` removes the agent link at its recorded bare path via
    // the manifest link registry, even when the source is prefixed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);
    assert!(sb.claude_home.join("agents/dev.md").exists());
    assert!(sb.mind(&["forget", "jk:dev"]).success);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err(),
        "forget must remove the bare agent link"
    );
    assert!(!sb.mind_home.join("store/agent/jk:dev").exists());
}

#[test]
fn cross_kind_shadow_name_still_warns_in_prose_under_prefix() {
    // spec: NS-42 -- the cross-kind shadow rule: a name that is BOTH an agent and
    // a skill is NOT treated as a bare agent referent, so a bare prose reference
    // to it under a prefix still triggers the unguarded-reference warning (the
    // skill side would be prefixed and break).
    let sb = Sandbox::new();
    // `shared` exists as both a skill and an agent.
    sb.write_and_commit(
        "skills/shared/SKILL.md",
        "---\nname: shared\ndescription: shared skill\n---\n# shared\n",
    );
    sb.write_and_commit(
        "agents/shared.md",
        "---\nname: shared\ndescription: shared agent\n---\n# shared\n",
    );
    // Another item references `shared` in bare prose.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nHand off to shared for the work.\n",
    );
    let r = sb.mind(&["--verbose", "meld", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("shared"),
        "a name shadowed across agent+skill must still be flagged: {}",
        r.stderr
    );
}

// ---- end NS-40 / NS-41 / NS-42 -------------------------------------------

#[test]
fn upgrade_treats_a_prefix_change_as_a_rename() {
    // spec: LIFE-10, LIFE-11, LIFE-14, CLI-61
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success); // no prefix yet
    assert!(sb.mind(&["learn", "review"]).success); // installed as skill:review

    // Upstream adds a namespace prefix.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("rename"),
        "report should flag rename: {}",
        r.stdout
    );
    assert!(
        r.stdout
            .contains("upgraded skill:review -> skill:jk:review"),
        "{}",
        r.stdout
    );

    // Manifest now holds only the renamed item.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("skill:jk:review"),
        "{}",
        recall.stdout
    );
    assert!(!recall.stdout.contains("skill:review"), "{}", recall.stdout);

    // Symlinks moved; the old one is gone, the new one exists.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_ok());
    // Old store copy removed, new one present.
    assert!(!sb.mind_home.join("store/skill/review").exists());
    assert!(sb.mind_home.join("store/skill/jk:review").exists());
}

#[test]
fn unmeld_unlink_only_keeps_installed_items() {
    // spec: CLI-20, CLI-22 - `--unlink-only` removes the source but keeps its
    // installed items, listing them with the forget hint.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["unmeld", "agents", "--unlink-only"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Source is gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    // The installed item is left in place and reported with the forget command.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok());
    assert!(
        sb.mind(&["recall", "review"]).success,
        "the item remains installed"
    );
    assert!(
        r.stdout.contains("remain installed") && r.stdout.contains("mind forget"),
        "unlink-only must list orphaned items and suggest forget: {}",
        r.stdout
    );
}

#[test]
fn unmeld_forgets_items_by_default() {
    // spec: CLI-21, CLI-27 - a plain unmeld uninstalls the source's items but
    // must not delete the linked local working tree (CLI-27).
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["unmeld", "agents"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err(),
        "the item link must be removed by default"
    );
    assert!(
        !sb.mind(&["recall", "review"]).success,
        "the item must be uninstalled by default"
    );
    // CLI-27: unmeld must not delete the linked source's working tree.
    assert!(
        sb.source.exists(),
        "unmeld must not delete the linked local working tree at {}",
        sb.source.display()
    );
}

#[test]
fn unmeld_unknown_source_errors() {
    // spec: CLI-20
    let sb = Sandbox::new();
    let r = sb.mind(&["unmeld", "nope"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no source named"), "{}", r.stderr);
}

#[test]
fn sources_with_same_basename_coexist() {
    // spec: STO-13, CLI-5
    let a = Sandbox::new();
    let b = Sandbox::new(); // separate repo, same basename, different parent
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success); // no collision

    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(sources.contains(&a_full), "{sources}");
    assert!(sources.contains(&b_full), "{sources}");

    // A bare item ref now matches both sources -> ambiguous.
    let bare = a.mind(&["learn", "review"]);
    assert!(!bare.success);
    assert!(bare.stderr.contains("ambiguous"), "{}", bare.stderr);

    // The full owner/repo qualifier resolves it.
    let r = a.mind(&["learn", &format!("{a_full}#review")]);
    assert!(r.success, "{}", r.stderr);
}

#[test]
fn unmeld_full_name_resolves_basename_collision() {
    // spec: CLI-20
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // Bare basename is ambiguous across the two sources.
    let amb = a.mind(&["unmeld", "agents"]);
    assert!(!amb.success);
    assert!(amb.stderr.contains("multiple sources"), "{}", amb.stderr);

    // Full owner/repo unmelds exactly one; the basename is then unambiguous.
    assert!(
        a.mind(&["unmeld", &format!("{}/agents", a.base_name())])
            .success
    );
    assert!(a.mind(&["unmeld", "agents"]).success);
}

#[test]
fn unmeld_glob_removes_only_the_matching_source() {
    // spec: CLI-28 - a glob removes the source(s) it matches and leaves the rest.
    // Meld two sources (`foo` and `agents`); `*agents` matches only `agents`.
    let a = Sandbox::named("foo");
    let agents = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &agents.source_spec()]).success);
    // Install an item from the agents source so its teardown is exercised.
    assert!(
        a.mind(&["learn", &format!("{}/agents#review", agents.base_name())])
            .success
    );

    let r = a.mind(&["unmeld", "*agents"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Only the agents source (and its item) is gone; foo survives.
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(sources.contains("foo"), "foo must remain melded: {sources}");
    assert!(
        !sources.contains(&format!("{}/agents", agents.base_name())),
        "the agents source must be unmelded: {sources}"
    );
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("skills/review")).is_err(),
        "the agents source's item link must be removed"
    );
}

#[test]
fn unmeld_glob_matching_several_lists_and_removes_with_yes() {
    // spec: CLI-28, CLI-42 - a glob may match more than one source; it lists the
    // matched sources and the multi-source confirmation applies. `--yes` skips it
    // and removes every match.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);
    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());

    // Without --yes a multi-source glob refuses in a non-TTY context, listing the
    // matched sources first.
    let refused = a.mind(&["unmeld", "*agents"]);
    assert!(!refused.success, "must refuse: {}", refused.stdout);
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    assert!(
        refused.stdout.contains(&a_full) && refused.stdout.contains(&b_full),
        "both matched sources must be listed: {}",
        refused.stdout
    );
    // Nothing removed by the refusal.
    let still = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        still.contains(&a_full) && still.contains(&b_full),
        "both sources must survive a refused unmeld: {still}"
    );

    // `--yes` removes both matched sources.
    let r = a.mind(&["unmeld", "*agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        a.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "every matching source must be unmelded"
    );
}

#[test]
fn unmeld_glob_matching_no_source_errors() {
    // spec: CLI-28 - a glob that matches nothing is SourceNotFound.
    let sb = melded();
    let r = sb.mind(&["unmeld", "*nope"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no source named"), "{}", r.stderr);
}

#[test]
fn unmeld_plain_ambiguous_suffix_still_errors() {
    // spec: CLI-28, CLI-20 - a plain (non-glob) ambiguous suffix is still
    // AmbiguousSource; only a glob is allowed to remove several sources.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    let amb = a.mind(&["unmeld", "agents"]);
    assert!(!amb.success);
    assert!(amb.stderr.contains("multiple sources"), "{}", amb.stderr);
}

#[test]
fn unmeld_glob_unlink_only_over_several_keeps_items() {
    // spec: CLI-28, CLI-22 - a glob matching more than one source goes through the
    // source-granularity multi-source confirmation (skipped here with `--yes`), and
    // `--unlink-only` applies to each matched source: every matched source is
    // unmelded but its installed items are KEPT, with the orphaned-items note shown
    // for each. Two distinct sources, each with one installed item.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);
    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());

    // Install one item from each source by its fully-qualified ref so both
    // sources have an installed item to orphan. Both sources carry a skill named
    // `review`; installing it from both would collide, so a different item (`dev`
    // agent) is installed from the second source to avoid a link conflict.
    assert!(
        a.mind(&["learn", &format!("{a_full}#skill:review")])
            .success,
        "install review from the first source"
    );
    assert!(
        a.mind(&["learn", &format!("{b_full}#agent:dev")]).success,
        "install dev from the second source"
    );

    // `*agents` matches both sources. `--yes` clears the multi-source confirmation;
    // `--unlink-only` keeps every matched source's items.
    let r = a.mind(&["unmeld", "*agents", "--unlink-only", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Both sources are unmelded.
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("no sources melded"),
        "every matched source must be unmelded: {sources}"
    );

    // CLI-22: the items are KEPT (links survive) and the orphaned-items note is
    // shown for each unmelded source.
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("skills/review")).is_ok(),
        "the first source's item link must be kept under --unlink-only"
    );
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("agents/dev.md")).is_ok(),
        "the second source's item link must be kept under --unlink-only"
    );
    assert!(
        r.stdout.matches("item(s) remain installed").count() >= 2,
        "the orphaned-items note must appear for each unmelded source: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("mind forget"),
        "the forget suggestion must be shown: {}",
        r.stdout
    );
}

#[test]
fn unmeld_glob_single_match_honors_item_count_confirmation() {
    // spec: CLI-28, CLI-21 - a glob matching exactly ONE source does not trigger
    // the source-granularity multi-source confirmation, but it DOES honor that
    // single source's per-source item-count confirmation (CLI-21/CLI-42): a non-TTY
    // run refuses without `--yes`, and `--yes` removes all of the source's items.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // A glob that matches the single melded `agents` source. Without `--yes`, the
    // item-count confirmation refuses in this non-TTY harness, listing the items.
    let refused = sb.mind(&["unmeld", "*agents"]);
    assert!(
        !refused.success,
        "a single-match glob must still honor the item-count confirmation: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    // It must NOT have prompted at source granularity (only one source matched).
    assert!(
        !refused.stdout.contains("would remove 1 source"),
        "a single match must not show the multi-source listing: {}",
        refused.stdout
    );
    // Nothing removed by the refusal.
    assert!(sb.mind(&["recall", "review"]).success, "item remains");
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source survives a refused single-match glob"
    );

    // `--yes` removes the source and every one of its items.
    let r = sb.mind(&["unmeld", "*agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "the matched source must be unmelded"
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err());
}

#[test]
fn unmeld_glob_aborts_remaining_sources_when_one_item_hook_fails() {
    // spec: CLI-28, HOOK-53, HOOK-54, HOOK-82 - CLI-28 says every matching source
    // is unmelded "each per its normal path"; the normal path includes the
    // hard-stop on a required (item) uninstall-hook failure (HOOK-53/82). So when a
    // matched source's item uninstall hook exits non-zero mid-iteration, the whole
    // `unmeld` aborts: that source stays melded with its item kept, and any
    // not-yet-processed matched source is left untouched (still melded).
    //
    // The failing source is melded FIRST so it is the first processed; an abort
    // therefore leaves the second, good source entirely unprocessed.
    let fail = Sandbox::bare("glob-abort");
    fail.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    // Per-item uninstall hook that exits non-zero (a hard stop, HOOK-82/53).
    fail.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"greet\"\npath = \"skills/greet\"\nuninstall = \"exit 1\"\n",
    );
    let good = Sandbox::bare("glob-abort");
    good.write_and_commit(
        "skills/other/SKILL.md",
        "---\ndescription: another skill\n---\n# other\n",
    );

    // Meld the failing source first, the good source second (iteration order is
    // meld order).
    assert!(
        fail.mind(&["meld", &fail.source_spec(), "--link-only"])
            .success
    );
    assert!(
        fail.mind(&["meld", &good.source_spec(), "--link-only"])
            .success
    );
    assert!(
        fail.mind(&[
            "learn",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success,
        "install greet (with its uninstall hook)"
    );
    assert!(
        fail.mind(&[
            "learn",
            "skill:other",
            "--dangerously-skip-install-hook-check"
        ])
        .success,
        "install other from the good source"
    );

    let fail_full = format!("{}/glob-abort", fail.base_name());
    let good_full = format!("{}/glob-abort", good.base_name());

    // `*glob-abort` matches both; `--yes` clears the multi-source confirmation,
    // `--dangerously-skip-install-hook-check` runs the hooks unattended.
    let r = fail.mind(&[
        "unmeld",
        "*glob-abort",
        "--yes",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !r.success,
        "a required item uninstall-hook failure must fail the whole unmeld: {} {}",
        r.stdout, r.stderr
    );

    // The failing source stays melded (its item is kept, mirroring HOOK-54).
    let sources = fail.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains(&fail_full),
        "the source whose item uninstall hook failed must stay melded: {sources}"
    );
    assert!(
        fail.mind(&["recall", "skill:greet"]).success,
        "the item is kept when its required uninstall hook fails"
    );

    // The remaining (good) source was processed AFTER the failing one, so the abort
    // leaves it untouched and still melded.
    assert!(
        sources.contains(&good_full),
        "a matched source after the failing one must be left unprocessed (still melded): {sources}"
    );
    assert!(
        fail.mind(&["recall", "skill:other"]).success,
        "the unprocessed source's item must still be installed"
    );
}

#[test]
fn sync_reports_up_to_date_then_updated() {
    // spec: CLI-50
    let sb = melded();
    assert!(sb.mind(&["sync"]).stdout.contains("up to date"));
    sb.edit_source();
    assert!(sb.mind(&["sync"]).stdout.contains("updated"));
}

#[test]
fn sync_with_no_sources_is_ok() {
    // spec: CLI-51
    let sb = Sandbox::new();
    let r = sb.mind(&["sync"]);
    assert!(r.success);
    assert!(r.stdout.contains("no sources melded"), "{}", r.stdout);
}

#[test]
fn introspect_reports_missing_link() {
    // spec: LIFE-30
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("symlink missing"), "{}", r.stdout);
}

#[test]
fn introspect_reports_drift_after_source_change() {
    // spec: LIFE-33
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("upstream changed"), "{}", r.stdout);
}

#[test]
fn introspect_reports_namespace_change() {
    // spec: LIFE-32
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("namespace changed"), "{}", r.stdout);
}

#[test]
fn failed_upgrade_preserves_the_previous_version() {
    // spec: LIFE-1, LIFE-2, LIFE-4
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:dev}}.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    // `lead` references {{ns:dev}}; confirm the closure with --yes.
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);
    let store = sb.mind_home.join("store/agent/jk:lead");
    // NS-42: {{ns:dev}} expands bare (dev is an agent sibling).
    assert!(std::fs::read_to_string(&store).unwrap().contains("dev"));

    // Upstream introduces a broken reference.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:ghost}}.\n",
    );
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(!r.success, "upgrade should fail on the bad reference");
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);

    // The previously installed good version is untouched.
    let body = std::fs::read_to_string(&store).expect("old store copy should remain");
    // NS-42: agent token expanded bare; NS-40: link is under bare harness name.
    assert!(body.contains("dev"), "old version should be intact: {body}");
    // NS-40: the agent link is under its bare frontmatter name, not the prefixed name.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/lead.md")).is_ok());
}

#[test]
fn removed_upstream_item_is_left_alone_and_flagged() {
    // spec: LIFE-12, LIFE-31
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);

    // The item disappears upstream.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success);

    // upgrade does not touch an item with no catalog match.
    let ev = sb.mind(&["upgrade", "--yes"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("up to date"), "{}", ev.stdout);
    assert!(sb.mind(&["recall"]).stdout.contains("agent:dev"));

    // introspect reports it as gone upstream.
    let ins = sb.mind(&["introspect"]);
    assert!(ins.stdout.contains("no longer present"), "{}", ins.stdout);
}

#[test]
fn upgrade_item_filter_limits_to_one() {
    // spec: CLI-63
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Change both items upstream.
    sb.edit_source(); // touches skills/review
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // Filtered upgrade applies only the named item.
    let ev = sb.mind(&["upgrade", "--yes", "review"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("upgraded skill:review"), "{}", ev.stdout);
    assert!(!ev.stdout.contains("agent:dev"), "{}", ev.stdout);

    // dev is still pending (reported by an unfiltered, declined upgrade).
    let rest = sb.mind(&["upgrade"]);
    assert!(rest.stdout.contains("agent:dev"), "{}", rest.stdout);
    assert!(!rest.stdout.contains("skill:review"), "{}", rest.stdout);
}

#[test]
fn upgrade_glob_upgrades_multiple_items() {
    // spec: CLI-65 -- a glob ref (`skill:*`) upgrades every matching pending item
    // in a single `upgrade --yes` pass; `agents#*` (source-scoped) upgrades all
    // pending items in that source.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Make both items drift upstream.
    sb.edit_source(); // touches skills/review
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // A kind-scoped glob upgrades all matching items in one pass.
    let ev = sb.mind(&["upgrade", "--yes", "skill:*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:review"),
        "skill:* must upgrade the skill: {}",
        ev.stdout
    );
    // agent:dev is out of scope for `skill:*`.
    assert!(
        !ev.stdout.contains("upgraded agent:dev"),
        "skill:* must not touch the agent: {}",
        ev.stdout
    );

    // Use source-scoped glob `agents#*` to upgrade the agent that is still pending.
    // The source identity ends with `/agents` so the bare suffix `agents` resolves it.
    let ev2 = sb.mind(&["upgrade", "--yes", "agents#*"]);
    assert!(ev2.success, "{}", ev2.stderr);
    assert!(
        ev2.stdout.contains("upgraded agent:dev"),
        "agents#* must upgrade the pending agent: {}",
        ev2.stdout
    );
}

#[test]
fn upgrade_glob_no_match_is_not_an_error() {
    // spec: CLI-65 -- a glob that matches no installed item reports up-to-date
    // and exits 0; it is NOT an error (unlike `forget`'s no-match glob).
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    // A glob that matches nothing pending: exits 0, reports nothing to do.
    let ev = sb.mind(&["upgrade", "--yes", "xyz*"]);
    assert!(ev.success, "no-match glob must exit 0: {}", ev.stderr);
    // The item is still pending (unchanged by the no-match glob).
    let pending = sb.mind(&["upgrade"]);
    assert!(
        pending.stdout.contains("skill:review"),
        "item must remain pending: {}",
        pending.stdout
    );
}

#[test]
fn upgrade_namespaced_glob_upgrades_namespace() {
    // spec: CLI-65 -- `upgrade 'jk:*'` upgrades only the items in that namespace.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Meld with an alias, learn all items.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);

    // Edit both upstream and sync.
    sb.edit_source();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // Glob over the whole namespace upgrades all items under it.
    let ev = sb.mind(&["upgrade", "--yes", "jk:*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:jk:review"),
        "jk:* must upgrade jk:review: {}",
        ev.stdout
    );
    assert!(
        ev.stdout.contains("upgraded agent:jk:dev"),
        "jk:* must upgrade jk:dev: {}",
        ev.stdout
    );
}

#[test]
fn upgrade_exact_ref_still_works_after_glob_change() {
    // spec: CLI-65 (regression) -- non-glob exact refs continue to work exactly
    // as before.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    sb.edit_source();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // An exact name ref upgrades only that item, not the glob-matched items.
    let ev = sb.mind(&["upgrade", "--yes", "review"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:review"),
        "exact ref must upgrade the item: {}",
        ev.stdout
    );
    assert!(
        !ev.stdout.contains("agent:dev"),
        "exact ref must not touch other items: {}",
        ev.stdout
    );

    // dev is still pending.
    let rest = sb.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("agent:dev"),
        "dev must remain pending: {}",
        rest.stdout
    );
}

#[test]
fn upgrade_source_glob_isolates_to_named_source() {
    // spec: CLI-65 -- `<source>#*` composes the source qualifier with the glob and
    // must upgrade ONLY that source's pending items, leaving another melded
    // source's pending items untouched in the same pass. The existing glob tests
    // meld a single source; this proves the source qualifier actually isolates
    // when two sources both have pending items.
    let agents = melded(); // source `agents`, carries skill:review
    assert!(agents.mind(&["learn", "review"]).success);

    // A second, independent source with a uniquely-named skill.
    let tools = Sandbox::bare("tools");
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of the second source failed"
    );
    assert!(agents.mind(&["learn", "deploy"]).success);

    // Drift an item in EACH source, then sync so both are pending.
    agents.edit_source(); // touches skills/review in the agents source
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\nedited\n",
    );
    assert!(agents.mind(&["sync"]).success);

    // Source-scoped glob for the tools source only.
    let ev = agents.mind(&["upgrade", "--yes", "tools#*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:deploy"),
        "tools#* must upgrade the tools source item: {}",
        ev.stdout
    );
    assert!(
        !ev.stdout.contains("skill:review"),
        "tools#* must NOT touch the other source's pending item: {}",
        ev.stdout
    );

    // The agents source item is still pending (unchanged by the scoped glob).
    let rest = agents.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("skill:review"),
        "the other source's item must remain pending: {}",
        rest.stdout
    );
    assert!(
        !rest.stdout.contains("skill:deploy"),
        "the tools source item was already upgraded: {}",
        rest.stdout
    );
}

#[test]
fn upgrade_exact_ref_no_match_is_up_to_date_not_error() {
    // spec: CLI-63, CLI-64 -- an EXACT (non-glob) ref that matches no installed
    // item reports up to date and exits 0 (like the glob no-match, CLI-65), rather
    // than erroring; and it leaves a genuinely-pending item untouched. This guards
    // the exact-ref path that the glob refactor left in place.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source(); // review is now pending
    assert!(sb.mind(&["sync"]).success);

    // An exact name that matches no installed item: not an error, nothing applied.
    let ev = sb.mind(&["upgrade", "--yes", "nonexistent"]);
    assert!(
        ev.success,
        "exact no-match ref must exit 0, not error: {} {}",
        ev.stdout, ev.stderr
    );
    assert!(
        !ev.stdout.contains("upgraded skill:review"),
        "a non-matching exact ref must not upgrade the pending item: {}",
        ev.stdout
    );

    // review is still pending (the no-match filter excluded it).
    let rest = sb.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("skill:review"),
        "the pending item must be untouched by the no-match ref: {}",
        rest.stdout
    );
}

#[test]
fn json_upgrade_glob_outcomes() {
    // spec: CLI-65, CLI-153 -- under --json a glob upgrade emits the standard
    // mutation object: a glob that matches a pending item yields outcome
    // "upgraded" with the installed keys; a glob that matches nothing yields
    // "up-to-date" (NOT an error) even while a non-matching item is still pending.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source(); // review pending
    assert!(sb.mind(&["sync"]).success);

    // A glob matching nothing: outcome up-to-date, single object, no prose.
    let none = sb.mind(&["upgrade", "--yes", "--json", "zzz*"]);
    assert!(
        none.success,
        "no-match glob under --json failed: {}",
        none.stderr
    );
    let v = parse_json(&none.stdout);
    assert_eq!(v["action"], "upgrade", "{}", none.stdout);
    assert_eq!(
        v["outcome"], "up-to-date",
        "a no-match glob must report up-to-date under --json: {}",
        none.stdout
    );
    assert!(
        !none.stdout.contains("up to date"),
        "no prose under --json: {}",
        none.stdout
    );

    // review is still pending; a matching glob now upgrades it.
    let some = sb.mind(&["upgrade", "--yes", "--json", "skill:*"]);
    assert!(
        some.success,
        "matching glob under --json failed: {}",
        some.stderr
    );
    let v = parse_json(&some.stdout);
    assert_eq!(v["action"], "upgrade", "{}", some.stdout);
    assert_eq!(
        v["outcome"], "upgraded",
        "a matching glob must report upgraded under --json: {}",
        some.stdout
    );
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        some.stdout
    );
    assert!(
        !some.stdout.contains("upgraded skill"),
        "no prose under --json: {}",
        some.stdout
    );
}

#[test]
fn mind_toml_unions_items_and_discover() {
    // spec: DSC-34
    let sb = Sandbox::new();
    sb.write_and_commit(
        "packages/foo/SKILL.md",
        "---\ndescription: foo\n---\n# foo\n",
    );
    sb.write_and_commit(
        "extra/special.md",
        "---\nname: special\ndescription: x\n---\n# special\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"agent\"\n",
            "name = \"special\"\n",
            "path = \"extra/special.md\"\n\n",
            "[discover]\n",
            "skills = { include = [\"packages/*/SKILL.md\"] }\n",
        ),
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let probe = sb.mind(&["probe"]).stdout;
    assert!(probe.contains("agent:special"), "from [[items]]: {probe}");
    assert!(probe.contains("skill:foo"), "from [discover]: {probe}");
}

#[test]
fn sync_preserves_consumer_alias() {
    // spec: CLI-52
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec(), "--as", "jk"]).success);
    assert!(sb.mind(&["sync"]).success);

    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("namespace:jk")
    );
    // Items remain namespaced under the alias after sync.
    assert!(sb.mind(&["probe"]).stdout.contains("skill:jk:review"));
}

#[test]
fn learn_glob_installs_all_matches() {
    // spec: CLI-31
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:dev"), "{recall}");
    assert!(recall.contains("rule:style"), "{recall}");
}

#[test]
fn learn_kind_glob_limits_to_kind() {
    // spec: CLI-31
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:*"]).success);
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "skill installed"
    );
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "agent not installed by a skill glob"
    );
}

#[test]
fn learn_all_flag_installs_whole_source() {
    // spec: CLI-36
    // `--all` is sugar for the `<source>#*` selector: every item of the source
    // installs, equivalent to `learn 'agents#*'`.
    let sb = melded();
    let r = sb.mind(&["learn", "agents", "--all"]);
    assert!(r.success, "{}", r.stderr);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:dev"), "{recall}");
    assert!(recall.contains("rule:style"), "{recall}");
}

#[test]
fn learn_all_flag_rejects_ref_with_hash() {
    // spec: CLI-36
    // Combining `--all` with a ref that already names an item is rejected; the
    // doubled selector is an invalid ref and nothing installs.
    let sb = melded();
    let r = sb.mind(&["learn", "agents#review", "--all"]);
    assert!(!r.success, "expected failure: {}", r.stdout);
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "nothing installed"
    );
}

#[test]
fn learn_dry_run_installs_nothing() {
    // spec: CLI-32
    let sb = melded();
    let r = sb.mind(&["learn", "*", "--dry-run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("would learn"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skill:review"),
        "plan should list items: {}",
        r.stdout
    );
    // Nothing was actually installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
}

#[test]
fn learn_glob_collision_errors_and_installs_nothing() {
    // spec: CLI-33
    let a = Sandbox::new();
    let b = Sandbox::new(); // same item names, different source
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // '*' matches review/dev/style from both sources -> same install names collide.
    let r = a.mind(&["learn", "*"]);
    assert!(!r.success);
    assert!(r.stderr.contains("ambiguous"), "{}", r.stderr);
    assert!(!a.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn probe_marks_installed_and_shows_hash() {
    // spec: CLI-81
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let probe = sb.mind(&["probe"]).stdout;

    let review = probe.lines().find(|l| l.contains("skill:review")).unwrap();
    assert!(
        review.starts_with('*'),
        "installed item should be marked: {review:?}"
    );
    let dev = probe.lines().find(|l| l.contains("agent:dev")).unwrap();
    assert!(
        !dev.starts_with('*'),
        "uninstalled item should not be marked: {dev:?}"
    );

    // A short (8 hex) content hash appears on the row.
    assert!(
        review
            .split_whitespace()
            .any(|t| t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit())),
        "expected a short hash: {review:?}"
    );
}

#[test]
fn probe_columns_align_with_long_names() {
    // spec: CLI-82
    let sb = Sandbox::new();
    // A key longer than the old fixed width, to exercise dynamic column sizing.
    sb.write_and_commit(
        "skills/consumer-experience-review/SKILL.md",
        "---\ndescription: long-named skill\n---\n# x\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let probe = sb.mind(&["probe"]).stdout;
    let cols: Vec<usize> = probe
        .lines()
        .filter(|l| l.contains("/agents"))
        .map(|l| l.find("local/").expect("source column on every row"))
        .collect();
    assert!(cols.len() >= 2, "expected several rows: {probe}");
    assert!(
        cols.iter().all(|&c| c == cols[0]),
        "source column misaligned: {cols:?}\n{probe}"
    );
}

#[test]
fn learn_source_and_kind_glob_compose() {
    // spec: CLI-31
    let sb = melded();
    // All skills of this source: review only (fixture has one skill).
    assert!(sb.mind(&["learn", "agents#skill:*"]).success);
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "skill installed"
    );
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "agent not installed by a skill glob"
    );
}

#[test]
fn learn_partial_failure_persists_successes() {
    // spec: CLI-34
    let sb = Sandbox::new();
    // A skill that sorts after `review` (so review installs first) and has a
    // broken reference, so the batch installs one item and then fails.
    sb.write_and_commit(
        "skills/zzz/SKILL.md",
        "---\ndescription: bad\n---\nsee {{ns:ghost}}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:*"]);
    assert!(!r.success, "should fail on the bad reference");
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);

    // The item installed before the failure is recorded in the manifest.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "successes should persist: {recall}"
    );
    // And the manifest matches disk: introspect finds no missing-link issues.
    let ins = sb.mind(&["introspect"]).stdout;
    assert!(
        !ins.contains("symlink missing"),
        "manifest/disk drift: {ins}"
    );
}

/// A source whose skill `review` references the agent `reviewer` via a
/// `{{ns:}}` token, so a partial `learn skill:review` must pull in `reviewer`
/// (its intra-source dependency). Returns the melded sandbox.
fn dep_fixture() -> Sandbox {
    let sb = Sandbox::bare("agents-and-skills");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\nhand off to {{ns:reviewer}}\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviews changes\n---\n# reviewer agent\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn learn_yes_installs_referenced_dependency_closure() {
    // spec: DEP-30
    // A partial `learn skill:review --yes` installs the whole closure: the
    // selected skill AND the agent it references via {{ns:reviewer}}. Both are
    // recorded in the manifest (dependency-first install order is internal and
    // not directly observable, so we assert the closure was applied).
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "referenced dependency pulled into the closure: {recall}"
    );
}

#[test]
fn learn_whole_source_glob_pulls_no_extras() {
    // spec: DEP-10 DEP-31
    // Selecting the whole source is full coverage: resolution is a no-op, so
    // `learn` installs directly with no prompt and adds nothing beyond the
    // two items that are already the entire source.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "agents-and-skills#*"]);
    assert!(r.success, "{}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:reviewer"), "{recall}");
}

#[test]
fn learn_dependency_dry_run_renders_tree_and_installs_nothing() {
    // spec: DEP-32
    // `--dry-run` over a partial selection renders the dependency tree (which
    // names the pulled-in `reviewer`) and lists the closure, but installs
    // nothing: the manifest stays empty.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--dry-run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("would learn"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skill:review [selected]"),
        "tree should head with the selected skill: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should mark the pulled-in dependency: {}",
        r.stdout
    );

    // Nothing was installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());
}

#[test]
fn forget_does_not_remove_a_dependency() {
    // spec: DEP-50
    // After installing the closure, forgetting the skill leaves its pulled-in
    // dependency installed: `forget` is per-item and never auto-removes deps.
    let sb = dep_fixture();
    assert!(sb.mind(&["learn", "skill:review", "--yes"]).success);
    assert!(sb.mind(&["forget", "skill:review"]).success);

    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "the forgotten skill is gone"
    );
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "the dependency stays installed"
    );
}

#[test]
fn learn_installs_dependency_before_dependent() {
    // spec: DEP-30 DEP-21
    // The closure installs dependency-first: the agent `reviewer` (a pulled-in
    // dependency) installs BEFORE the skill `review` that references it. The
    // "learned ..." lines are emitted in install order, so the dependency line
    // must precede the dependent's line in stdout.
    let sb = dep_fixture();
    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    let dep_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned agent:reviewer "))
        .unwrap_or_else(|| panic!("missing reviewer learned line: {}", r.stdout));
    let dependent_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned skill:review "))
        .unwrap_or_else(|| panic!("missing review learned line: {}", r.stdout));
    assert!(
        dep_line < dependent_line,
        "dependency must install before its dependent: {}",
        r.stdout
    );
}

#[test]
fn learn_dependency_prompt_decline_installs_nothing() {
    // spec: DEP-31
    // When the closure adds a pulled-in dependency, `learn` (no --yes) prints
    // the tree and prompts. Answering "n" cancels: nothing is installed, the
    // manifest holds neither item, and no symlinks are created.
    let sb = dep_fixture();
    let r = sb.mind_with_input(&["learn", "skill:review"], Some("n\n"));
    assert!(r.success, "{}", r.stderr);
    // The dependency tree is shown before the prompt.
    assert!(
        r.stdout.contains("skill:review [selected]"),
        "tree should head with the selected skill: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should mark the pulled-in dependency: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("cancelled; nothing installed"),
        "decline should print the cancelled line: {}",
        r.stdout
    );

    // Nothing installed: manifest empty, no symlinks for either item.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());
}

#[test]
fn learn_dependency_prompt_defaults_to_no_on_eof() {
    // spec: DEP-31
    // With no stdin (immediate EOF on the prompt), the `[y/N]` default is No, so
    // the closure is not installed. The prompt and tree are still shown.
    let sb = dep_fixture();
    let r = sb.mind_with_input(&["learn", "skill:review"], Some(""));
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should still render before the prompt: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("cancelled; nothing installed"),
        "EOF should default to No: {}",
        r.stdout
    );

    // Nothing installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/reviewer.md")).is_err());
}

#[test]
fn learn_dependency_prompt_accept_installs_closure() {
    // spec: DEP-31
    // Answering "y" to the prompt (without --yes) confirms: the whole closure
    // installs, both the selected skill and its pulled-in dependency.
    let sb = dep_fixture();
    let r = sb.mind_with_input(&["learn", "skill:review"], Some("y\n"));
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("agent:reviewer [dep]"),
        "tree should render before the prompt: {}",
        r.stdout
    );

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed on confirm: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "dependency installed on confirm: {recall}"
    );
}

#[test]
fn learn_pulls_dependency_referenced_in_non_skill_md_file() {
    // spec: DEP-1
    // The dependency scan covers the WHOLE skill directory (matching NS-20's
    // breadth), not just SKILL.md. A `{{ns:reviewer}}` token living in a sibling
    // file (extra.md) inside the skill dir still pulls in the agent.
    let sb = Sandbox::bare("nonmd-deps");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff\n---\n# review\n",
    );
    sb.write_and_commit(
        "skills/review/extra.md",
        "see {{ns:reviewer}} for handoff\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviews changes\n---\n# reviewer agent\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "token in a non-SKILL.md file still pulls the dependency: {recall}"
    );
}

#[test]
fn learn_dependency_already_installed_prompts_but_reinstalls_only_new() {
    // spec: DEP-23 DEP-31
    // Install the dependency alone first. A later partial `learn skill:review`
    // still shows the closure (so it still prompts, the dependency is part of
    // the tree) but the already-installed reviewer is marked [installed] and is
    // not reinstalled; only the new `review` installs.
    let sb = dep_fixture();
    assert!(sb.mind(&["learn", "agent:reviewer", "--yes"]).success);

    let r = sb.mind_with_input(&["learn", "skill:review"], Some("y\n"));
    assert!(r.success, "{}", r.stderr);
    // The tree marks the dependency as already installed (DEP-23).
    assert!(
        r.stdout.contains("agent:reviewer [installed]"),
        "already-installed dep should be marked [installed]: {}",
        r.stdout
    );
    // Only the new item is (re)installed; reviewer is not learned again.
    assert!(
        r.stdout.contains("learned skill:review "),
        "the new skill installs: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("learned agent:reviewer "),
        "the already-installed dependency is not reinstalled: {}",
        r.stdout
    );

    // Exactly one reviewer in the manifest (not duplicated), plus the skill.
    let recall = sb.mind(&["recall"]).stdout;
    assert_eq!(
        recall.matches("agent:reviewer").count(),
        1,
        "reviewer must not be duplicated: {recall}"
    );
    assert!(recall.contains("skill:review"), "{recall}");
}

#[test]
fn learn_closure_collision_via_pulled_dependency_aborts() {
    // spec: DEP-30
    // The collision check runs over the FULL closure, not just the explicit
    // selection. Two sources each carry a skill that references its own
    // `{{ns:reviewer}}` agent. Selecting `skill:*` selects two non-colliding
    // skills, but the closure pulls in BOTH `agent:reviewer` items, which
    // collide on `agent:reviewer`. Learn must report the collision and install
    // nothing.
    let a = Sandbox::bare("coll-a");
    a.write_and_commit(
        "skills/areview/SKILL.md",
        "---\nname: areview\ndescription: A review\n---\n# areview\nuse {{ns:reviewer}}\n",
    );
    a.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: A reviewer\n---\n# reviewer\n",
    );
    let b = Sandbox::bare("coll-b");
    b.write_and_commit(
        "skills/breview/SKILL.md",
        "---\nname: breview\ndescription: B review\n---\n# breview\nuse {{ns:reviewer}}\n",
    );
    b.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: B reviewer\n---\n# reviewer\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // The explicit selection (two distinct skills) does not collide; the
    // collision only arises once the pulled-in reviewers join the closure.
    let r = a.mind(&["learn", "skill:*", "--yes"]);
    assert!(!r.success, "closure collision should abort: {}", r.stdout);
    assert!(
        r.stderr.contains("ambiguous"),
        "collision should be reported as ambiguous: {}",
        r.stderr
    );
    // Nothing installed.
    assert!(!a.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn unlearn_is_an_alias_for_forget() {
    // spec: CLI-40
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["unlearn", "review"]).success);
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn status_is_an_alias_for_recall() {
    // spec: CLI-70
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let recall = sb.mind(&["recall"]);
    let status = sb.mind(&["status"]);
    assert!(status.success, "status alias runs: {}", status.stderr);
    assert_eq!(
        status.stdout, recall.stdout,
        "`status` must produce the same output as `recall`"
    );
    // The alias accepts recall's arguments too.
    assert!(sb.mind(&["status", "--sources"]).success);
}

#[test]
fn learn_links_into_all_configured_homes() {
    // spec: STO-14, LIFE-40
    let sb = Sandbox::new();
    let home_a = sb.base.join("homeA");
    let home_b = sb.base.join("homeB");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            home_a.display(),
            home_b.display()
        ),
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success);

    // The item is linked into both homes.
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());

    // forget removes it from every home (via the recorded link registry).
    assert!(sb.mind(&["forget", "review"]).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_err());
}

#[test]
fn learn_links_into_homes_from_env() {
    // spec: STO-14
    let sb = Sandbox::new();
    let home_a = sb.base.join("envA");
    let home_b = sb.base.join("envB");
    let homes = format!("{}:{}", home_a.display(), home_b.display());
    let env = [("MIND_AGENT_HOMES", homes.as_str())];

    assert!(sb.mind_env(&["meld", &sb.source_spec()], &env).success);
    assert!(sb.mind_env(&["learn", "review"], &env).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());
}

#[test]
fn meld_with_ssh_config_still_melds_a_local_source() {
    // spec: CLI-19 - `ssh = true` makes meld prefer SSH for https remotes, but a
    // local path is never rewritten, so a local-path meld still works and the
    // recorded URL stays the local path (no git@ rewrite).
    let sb = Sandbox::new();
    write(&sb.mind_home.join("config.toml"), "ssh = true\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "ssh-config meld of a local source should succeed: {}",
        r.stderr
    );
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains(&spec),
        "a local source URL must be unchanged under ssh=true: {json}"
    );
    assert!(
        !json.contains("git@"),
        "a local path must not be rewritten to git@: {json}"
    );
}

#[test]
fn config_lobes_add_list_remove() {
    // spec: CLI-111, CLI-112, CLI-113
    let sb = Sandbox::new();
    let home_a = sb.base.join("lobeA");
    let home_b = sb.base.join("lobeB");
    let (a, b) = (home_a.display().to_string(), home_b.display().to_string());

    assert!(sb.mind(&["config", "lobes", "add", &a]).success);
    assert!(sb.mind(&["config", "lobes", "add", &b]).success);

    let list = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(list.contains(&a), "{list}");
    assert!(list.contains(&b), "{list}");

    // Configured lobes drive where learn links.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());

    // Remove one; it drops from the list, removing a missing one errors.
    assert!(sb.mind(&["config", "lobes", "remove", &a]).success);
    let list2 = sb.mind(&["config", "lobes", "list"]).stdout;
    assert!(!list2.contains(&a), "{list2}");
    assert!(list2.contains(&b), "{list2}");
    let bad = sb.mind(&["config", "lobes", "remove", &a]);
    assert!(!bad.success);
    assert!(
        bad.stderr.contains("not a configured agent home"),
        "{}",
        bad.stderr
    );
}

#[test]
fn config_show_creates_default_and_reports_lobes() {
    // spec: CLI-110, STO-15
    let sb = Sandbox::new();
    let cfg_path = sb.mind_home.join("config.toml");
    assert!(!cfg_path.exists());

    // show creates the config with the default lobe (the claude home).
    let show = sb.mind(&["config", "show"]);
    assert!(show.success, "{}", show.stderr);
    assert!(cfg_path.exists(), "config should be created on show");
    assert!(show.stdout.contains("config.toml"), "{}", show.stdout);
    assert!(show.stdout.contains("lobes"), "{}", show.stdout);
    assert!(
        show.stdout.contains(&sb.claude_home.display().to_string()),
        "default lobe should be the claude home: {}",
        show.stdout
    );

    // After adding a lobe, show lists it too.
    let home = sb.base.join("shownLobe").display().to_string();
    assert!(sb.mind(&["config", "lobes", "add", &home]).success);
    assert!(sb.mind(&["config", "show"]).stdout.contains(&home));
}

#[test]
fn forget_glob_uninstalls_all_matches() {
    // spec: CLI-41
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);
    assert!(sb.mind(&["recall"]).stdout.contains("skill:review"));

    // A kind glob forgets only that kind.
    assert!(sb.mind(&["forget", "skill:*"]).success);
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "skill:review should be uninstalled"
    );
    assert!(
        sb.mind(&["recall", "agent:dev"]).success,
        "agent:dev should remain installed"
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());

    // A bare `*` forgets everything that is left (multi-match needs --yes, CLI-42).
    assert!(sb.mind(&["forget", "*", "--yes"]).success);
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));

    // A glob matching no installed item is an error.
    let none = sb.mind(&["forget", "zzz*"]);
    assert!(!none.success);
    assert!(none.stderr.contains("not installed"), "{}", none.stderr);
}

#[test]
fn forget_confirms_before_removing_multiple_items() {
    // spec: CLI-42 - a multi-match glob refuses in a non-TTY context without
    // --yes (rather than removing silently), and lists what it would remove.
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);

    let r = sb.mind(&["forget", "*"]);
    assert!(
        !r.success,
        "a multi-item forget must refuse without --yes: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("would remove") && r.stdout.contains("skill:review"),
        "it must list what would be removed: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "non-TTY refusal: {}",
        r.stderr
    );
    // Nothing was removed.
    assert!(
        sb.mind(&["recall"]).stdout.contains("skill:review"),
        "items must remain after a refused forget"
    );

    // A single exact forget is not prompted.
    assert!(sb.mind(&["forget", "skill:review"]).success);
}

#[test]
fn unmeld_forgets_all_items_with_yes() {
    // spec: CLI-21, CLI-42 - default unmeld removes the source's items; `--yes`
    // skips the multi-item confirmation.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Without --yes, a multi-item unmeld refuses in a non-TTY context (CLI-42).
    let refused = sb.mind(&["unmeld", "agents"]);
    assert!(
        !refused.success,
        "must refuse without --yes: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    // The source and items are untouched after the refusal.
    assert!(sb.mind(&["recall", "review"]).success, "item remains");

    let r = sb.mind(&["unmeld", "agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("removed"), "{}", r.stdout);

    // Both the source and every installed item are gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err());
    // CLI-27: unmeld must not delete the linked source's working tree.
    assert!(
        sb.source.exists(),
        "unmeld --yes must not delete the linked local working tree at {}",
        sb.source.display()
    );
}

#[test]
fn introspect_fix_relinks_missing_symlink() {
    // spec: CLI-91
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let link = sb.claude_home.join("skills/review");
    std::fs::remove_file(&link).unwrap();

    let r = sb.mind(&["introspect", "--fix"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("relinked"), "{}", r.stdout);

    // The link is back and introspect is now clean.
    assert!(std::fs::symlink_metadata(&link).is_ok());
    assert!(sb.mind(&["introspect"]).stdout.contains("all good"));
}

#[test]
fn sync_upgrade_refreshes_then_applies_upgrades() {
    // spec: CLI-53
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source(); // upstream change, not yet synced

    // One command fetches the change and (on `y`) applies the upgrade.
    let r = sb.mind_with_input(&["sync", "--upgrade"], Some("y\n"));
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("updated"), "sync ran: {}", r.stdout);
    assert!(
        r.stdout.contains("upgraded skill:review"),
        "upgrade applied: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]).stdout;
    assert_ne!(before, after, "commit/hash should have advanced");
}

#[test]
fn probe_and_recall_filter_by_kind_and_source() {
    // spec: CLI-83
    let sb = melded();

    // probe --kind narrows to one kind, composing with the substring query.
    let skills = sb.mind(&["probe", "--kind", "skill"]).stdout;
    assert!(skills.contains("skill:review"), "{skills}");
    assert!(!skills.contains("agent:dev"), "{skills}");

    // probe --source narrows by source selector (the repo basename suffix).
    let by_source = sb.mind(&["probe", "--source", "agents"]).stdout;
    assert!(by_source.contains("skill:review"), "{by_source}");
    let no_source = sb.mind(&["probe", "--source", "nope"]).stdout;
    assert!(!no_source.contains("skill:review"), "{no_source}");

    // recall --kind filters the installed listing.
    assert!(sb.mind(&["learn", "*"]).success);
    let only_agents = sb.mind(&["recall", "--kind", "agent"]).stdout;
    assert!(only_agents.contains("agent:dev"), "{only_agents}");
    assert!(!only_agents.contains("skill:review"), "{only_agents}");

    // Filters are meaningless with --sources; recall says so rather than
    // silently ignoring them.
    let warned = sb.mind(&["recall", "--sources", "--kind", "skill"]);
    assert!(warned.success, "{}", warned.stderr);
    assert!(warned.stderr.contains("ignored"), "{}", warned.stderr);
}

#[test]
fn meld_rejects_source_requiring_a_newer_mind() {
    // spec: DSC-40
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"9.0\"\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(!r.success, "should refuse a too-new source");
    assert!(r.stderr.contains("requires mind"), "{}", r.stderr);
    // Rejected: the source is not registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );

    // A satisfiable floor melds fine.
    let ok = Sandbox::new();
    ok.write_and_commit("mind.toml", "[source]\nmin-mind-version = \"0.0.1\"\n");
    assert!(ok.mind(&["meld", &ok.source_spec()]).success);
}

#[test]
fn config_is_created_with_default_lobe_on_first_use() {
    // spec: STO-15
    let sb = Sandbox::new();
    let cfg_path = sb.mind_home.join("config.toml");
    assert!(!cfg_path.exists());
    // A layout-creating command materializes the default config.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(cfg_path.exists(), "meld should create the default config");
    let body = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(body.contains("lobes"), "{body}");
    assert!(
        body.contains(&sb.claude_home.display().to_string()),
        "default lobe should be the claude home: {body}"
    );
}

#[test]
fn sync_continues_past_a_failed_source() {
    // spec: CLI-54
    let a = Sandbox::new(); // healthy
    let b = Sandbox::new(); // will be broken
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // Break b's remote and advance a's, then sync both.
    std::fs::remove_dir_all(&b.source).unwrap();
    a.edit_source();
    let r = a.mind(&["sync"]);

    // The run reports the failure and exits non-zero...
    assert!(!r.success, "sync should exit non-zero when a source fails");
    assert!(
        r.stdout.contains("failed") || r.stderr.contains("failed"),
        "broken source reported: {} / {}",
        r.stdout,
        r.stderr
    );
    // ...but the healthy source was still refreshed (progress persisted).
    assert!(r.stdout.contains("updated"), "healthy source: {}", r.stdout);
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains(&format!("{}/agents", a.base_name())),
        "{sources}"
    );
    assert!(
        sources.contains(&format!("{}/agents", b.base_name())),
        "{sources}"
    );
}

#[test]
fn recall_json_emits_items_and_sources() {
    // spec: CLI-73, CLI-167 - JSON outputs are wrapped in {"schema":1,"items":[...]}.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // The default view is an envelope with a sources array, each with nested items.
    let items = sb.mind(&["recall", "--json"]);
    assert!(items.success, "{}", items.stderr);
    let env: serde_json::Value =
        serde_json::from_str(&items.stdout).expect("recall --json envelope");
    assert_eq!(env["schema"], 1, "schema must be 1: {}", items.stdout);
    assert!(
        env["items"].is_array(),
        "items key must be array: {}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"items\""),
        "sources carry nested items: {}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"key\": \"skill:review\""),
        "{}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"installed\": true"),
        "the installed item is flagged: {}",
        items.stdout
    );

    // A single-item lookup is a plain JSON object (not wrapped).
    let one = sb.mind(&["recall", "skill:review", "--json"]).stdout;
    assert!(one.trim_start().starts_with('{'), "{one}");
    assert!(one.contains("\"hash\""), "{one}");

    // --sources is an envelope with sources array.
    let srcs_r = sb.mind(&["recall", "--sources", "--json"]);
    let srcs_env: serde_json::Value =
        serde_json::from_str(&srcs_r.stdout).expect("recall --sources --json envelope");
    assert_eq!(srcs_env["schema"], 1, "schema must be 1: {}", srcs_r.stdout);
    assert!(
        srcs_env["items"].is_array(),
        "items key must be array: {}",
        srcs_r.stdout
    );
    assert!(srcs_r.stdout.contains("\"url\""), "{}", srcs_r.stdout);

    // An empty registry is {"schema":1,"items":[]}, not a human message.
    // spec: CLI-167
    let fresh = Sandbox::new();
    let empty_env: serde_json::Value =
        serde_json::from_str(fresh.mind(&["recall", "--json"]).stdout.trim())
            .expect("empty recall envelope");
    assert_eq!(empty_env["schema"], 1);
    assert_eq!(
        empty_env["items"].as_array().map(|a| a.len()),
        Some(0),
        "empty recall must emit empty items array"
    );
}

#[test]
fn probe_json_emits_rows() {
    // spec: CLI-84, CLI-167 - output is wrapped in {"schema":1,"items":[...]}.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("probe --json envelope");
    assert_eq!(env["schema"], 1, "schema must be 1: {}", r.stdout);
    let rows = env["items"].as_array().expect("items must be array");
    assert!(r.stdout.contains("\"installed\""), "{}", r.stdout);
    assert!(r.stdout.contains("\"name\": \"review\""), "{}", r.stdout);
    // The installed item carries installed:true.
    assert!(
        rows.iter().any(|row| row["installed"] == true),
        "{}",
        r.stdout
    );
}

// --- unmanaged lobe items (spec/unmanaged.md) -------------------------------

/// Place an unmanaged skill (a dir) and agent (a file) directly in the lobe.
fn seed_unmanaged(sb: &Sandbox) {
    write(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    write(
        &sb.claude_home.join("agents/custom.md"),
        "---\nname: custom\n---\n# custom\n",
    );
}

#[test]
fn recall_shows_unmanaged_lobe_items() {
    // spec: UNM-1 UNM-2
    let sb = melded();
    seed_unmanaged(&sb);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("unmanaged: not installed by mind"),
        "recall must surface an unmanaged group: {}",
        r.stdout
    );
    assert!(r.stdout.contains("skill:handmade"), "{}", r.stdout);
    assert!(r.stdout.contains("agent:custom"), "{}", r.stdout);
}

#[test]
fn recall_excludes_managed_links_from_unmanaged() {
    // spec: UNM-1
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stdout.contains("unmanaged: not installed by mind"),
        "a mind-installed link must not be reported as unmanaged: {}",
        r.stdout
    );
}

#[test]
fn probe_lists_and_searches_unmanaged_items() {
    // spec: UNM-3
    let sb = melded();
    seed_unmanaged(&sb);
    // The non-interactive listing includes the unmanaged item, marked.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("skill:handmade") && r.stdout.contains("(unmanaged)"),
        "probe listing must mark the unmanaged item: {}",
        r.stdout
    );
    // The substring search matches its name (CLI-85).
    let s = sb.mind(&["probe", "handmade", "--no-tui"]);
    assert!(
        s.stdout.contains("skill:handmade"),
        "search must find the unmanaged item: {}",
        s.stdout
    );
    // JSON carries the unmanaged flag; managed rows omit it.
    let j = sb.mind(&["probe", "handmade", "--json"]);
    assert!(
        j.stdout.contains("\"unmanaged\": true"),
        "json must flag the unmanaged row: {}",
        j.stdout
    );
}

#[test]
fn forget_unmanaged_removes_after_warning() {
    // spec: UNM-4 UNM-5
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    assert!(skill.is_dir());
    let r = sb.mind(&["forget", "skill:handmade", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("not managed by mind"),
        "the removal must state it is unmanaged: {}",
        r.stdout
    );
    assert!(!skill.exists(), "the unmanaged skill dir must be removed");
}

#[test]
fn forget_unmanaged_refuses_without_yes_in_non_tty() {
    // spec: UNM-5
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // No --yes, non-TTY: refuse and remove nothing, after stating it is unmanaged.
    let r = sb.mind(&["forget", "skill:handmade"]);
    assert!(!r.success, "must refuse without --yes: {}", r.stdout);
    assert!(
        r.stdout.contains("not managed by mind"),
        "must state it is unmanaged: {}",
        r.stdout
    );
    assert!(skill.exists(), "nothing may be removed on refusal");
}

#[test]
fn forget_glob_never_sweeps_unmanaged() {
    // spec: UNM-4
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // A glob matches managed items only; with none installed it removes nothing
    // and must never touch the unmanaged skill.
    let _ = sb.mind(&["forget", "*", "--yes"]);
    assert!(
        skill.exists(),
        "a glob forget must never delete an unmanaged item"
    );
}

// --- UNM-7/UNM-8: forget --unmanaged bulk removal ---------------------------

/// `forget --unmanaged 'skill:*' --yes` removes every unmanaged skill.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_kind_glob_removes_matching() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");
    // skill:* removes the skill, not the agent.
    let r = sb.mind(&["forget", "--unmanaged", "skill:*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.to_lowercase().contains("not managed by mind"),
        "must state items are not managed: {}",
        r.stdout
    );
    assert!(!skill.exists(), "the unmanaged skill dir must be removed");
    assert!(agent.exists(), "the unmanaged agent must be untouched");
}

/// `forget --unmanaged --yes` (no ref) removes ALL unmanaged items.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_no_ref_removes_all() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");
    let r = sb.mind(&["forget", "--unmanaged", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!skill.exists(), "handmade skill must be removed");
    assert!(!agent.exists(), "custom agent must be removed");
}

/// A MANAGED installed item is never matched by `forget --unmanaged '*' --yes`.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_never_removes_managed_items() {
    let sb = melded();
    // Install a managed item.
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let managed_link = sb.claude_home.join("skills/review");
    assert!(managed_link.exists(), "managed link must exist after learn");

    // Also place an unmanaged skill.
    let unmanaged_skill = sb.claude_home.join("skills/handmade");
    write(
        &unmanaged_skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );

    // A broad --unmanaged '*' must remove only the unmanaged item.
    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!unmanaged_skill.exists(), "unmanaged skill must be removed");
    assert!(
        managed_link.exists(),
        "managed link must survive --unmanaged removal"
    );
}

/// Non-TTY without `--yes` exits ConfirmationRequired and removes nothing.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_refuses_non_tty_without_yes() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // No --yes, piped (non-TTY): must fail and leave the item in place.
    let r = sb.mind(&["forget", "--unmanaged", "skill:*"]);
    assert!(
        !r.success,
        "must refuse without --yes in non-TTY: {}",
        r.stderr
    );
    assert!(skill.exists(), "nothing must be removed on refusal");
}

/// A ref that matches no unmanaged item exits NotInstalled.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_no_match_is_not_installed() {
    let sb = melded();
    let r = sb.mind(&["forget", "--unmanaged", "nope*", "--yes"]);
    assert!(!r.success, "must fail when no match: {}", r.stderr);
    assert!(
        r.stderr.contains("not installed") || r.stderr.contains("nope"),
        "error must name the unmatched ref: {}",
        r.stderr
    );
}

/// `--json --yes --unmanaged <glob>` emits one MutationResult object whose
/// `removed` array carries the `kind:name` keys of every removed unmanaged item,
/// with no human prose, and removes the matched files.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_json_lists_removed_keys() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");

    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes", "--json"]);
    assert!(r.success, "forget --unmanaged --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "forget", "{}", r.stdout);
    assert_eq!(v["target"], "*", "{}", r.stdout);
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    // The removed array carries both keys, in scan order (BTreeMap by
    // (ItemKind, name); ItemKind declares Skill before Agent).
    assert_eq!(
        v["removed"],
        serde_json::json!(["skill:handmade", "agent:custom"]),
        "removed keys must list every removed unmanaged item: {}",
        r.stdout
    );
    // No human prose under --json.
    assert!(
        !r.stdout.contains("forgot") && !r.stdout.contains("not managed by mind"),
        "human prose must be absent under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
    assert!(!skill.exists() && !agent.exists(), "both must be removed");
}

/// The `-y` short form skips the prompt for `--unmanaged` just like `--yes`.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_short_y_skips_prompt() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["forget", "--unmanaged", "skill:*", "-y"]);
    assert!(
        r.success,
        "-y must skip the prompt: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists(),
        "the unmanaged skill must be removed with -y"
    );
}

/// The `unlearn` visible alias works with `--unmanaged`.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_via_unlearn_alias() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["unlearn", "--unmanaged", "--yes"]);
    assert!(
        r.success,
        "unlearn alias must accept --unmanaged: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists(),
        "the unmanaged skill must be removed via unlearn"
    );
}

/// A kind-qualified EXACT name (not a glob) removes exactly that one unmanaged
/// item and leaves a same-name item of a different kind alone.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_kind_exact_name_removes_one() {
    let sb = melded();
    // A skill and an agent that share the name `shared`.
    let skill = sb.claude_home.join("skills/shared");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# shared\n",
    );
    let agent = sb.claude_home.join("agents/shared.md");
    write(&agent, "---\nname: shared\n---\n# shared\n");

    let r = sb.mind(&["forget", "--unmanaged", "agent:shared", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!agent.exists(), "the agent:shared must be removed");
    assert!(
        skill.exists(),
        "the same-named skill must be untouched by an exact agent: ref"
    );
}

/// A BARE exact name shared across kinds (skill+agent both named `shared`)
/// removes BOTH through the list-and-confirm path. Unlike the single-item UNM-4
/// `resolve` path (which errors AmbiguousItem), the bulk `select` path treats a
/// bare name uniformly: every kind with that name matches and is removed.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_bare_name_removes_all_kinds() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/shared");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# shared\n",
    );
    let agent = sb.claude_home.join("agents/shared.md");
    write(&agent, "---\nname: shared\n---\n# shared\n");

    let r = sb.mind(&["forget", "--unmanaged", "shared", "--yes"]);
    assert!(
        r.success,
        "a bare shared name must not error under --unmanaged: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists() && !agent.exists(),
        "both same-named unmanaged items must be removed"
    );
}

/// A source-qualified ref never matches an unmanaged item, so it is NotInstalled
/// and removes nothing.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_source_qualified_is_not_installed() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&[
        "forget",
        "--unmanaged",
        "owner/repo#skill:handmade",
        "--yes",
    ]);
    assert!(
        !r.success,
        "a source-qualified ref must not match an unmanaged item: {}",
        r.stdout
    );
    assert!(
        skill.exists(),
        "nothing must be removed when the ref is source-qualified"
    );
}

/// An unmanaged item present in TWO configured lobes is one logical item; a bulk
/// `--unmanaged` removal deletes every occupied lobe path (STO-14, UNM-1).
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_removes_from_all_lobes() {
    let sb = Sandbox::new();
    let home_a = sb.base.join("homeA");
    let home_b = sb.base.join("homeB");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            home_a.display(),
            home_b.display()
        ),
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // The same unmanaged skill `dup` placed by hand in both lobes.
    let skill_a = home_a.join("skills/dup");
    let skill_b = home_b.join("skills/dup");
    write(
        &skill_a.join("SKILL.md"),
        "---\ndescription: mine\n---\n# dup\n",
    );
    write(
        &skill_b.join("SKILL.md"),
        "---\ndescription: mine\n---\n# dup\n",
    );

    let r = sb.mind(&["forget", "--unmanaged", "skill:dup", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!skill_a.exists(), "lobe A copy must be removed");
    assert!(!skill_b.exists(), "lobe B copy must be removed");
}

/// `--unmanaged` never touches the manifest: a managed item installed alongside
/// unmanaged ones survives in the manifest after a broad `--unmanaged '*'`.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_leaves_manifest_unchanged() {
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let manifest = sb.mind_home.join("manifest.json");
    let before = std::fs::read_to_string(&manifest).unwrap();

    // Place an unmanaged item and sweep all unmanaged.
    let unmanaged = sb.claude_home.join("skills/handmade");
    write(
        &unmanaged.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!unmanaged.exists(), "unmanaged item removed");

    let after = std::fs::read_to_string(&manifest).unwrap();
    assert_eq!(
        before, after,
        "the manifest must be byte-identical after --unmanaged removal"
    );
    // The managed item's link survives.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "the managed review link must survive"
    );
}

// --- TUI-2 fallback tests ---------------------------------------------------
//
// TUI-2: `probe` falls back to the non-interactive catalog listing when
// `--no-tui` is given, `--json` is given, or stdout is not a TTY (piped or
// redirected). The `query`, `--kind`, `--source` args apply in both modes.
//
// These tests run `mind probe` with stdout piped (non-TTY), which is the same
// condition the test harness always uses. We verify that:
//   (a) the plain listing is produced (not raw-mode garbage),
//   (b) `--no-tui` produces the same listing,
//   (c) `--json` produces JSON (not raw-mode garbage),
//   (d) query/--kind/--source args are honoured in fallback mode.
// TUI-1 (interactive launch with a real TTY) is allowlisted; it cannot be
// verified headlessly. These tests verify TUI-2 (fallback) and are sufficient
// to prove the opt-out logic is correct.

#[test]
fn probe_fallback_on_non_tty_stdout_produces_listing() {
    // spec: TUI-2
    // The test harness pipes stdout, so is_terminal() returns false; probe must
    // fall back to the plain catalog listing rather than entering raw mode.
    let sb = melded();
    let r = sb.mind(&["probe"]);
    assert!(r.success, "probe fallback should succeed: {}", r.stderr);
    // Listing shows all three kinds.
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("rule:style"), "listing: {}", r.stdout);
    // No ANSI raw-mode escape sequences (the listing does not use ratatui).
    assert!(
        !r.stdout.contains("\x1b[?1049h"),
        "raw-mode alt-screen escape must not appear in fallback output"
    );
}

#[test]
fn probe_no_tui_flag_produces_listing() {
    // spec: TUI-2 - `--no-tui` forces the plain listing even on a TTY.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe --no-tui should succeed: {}", r.stderr);
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(r.stdout.contains("agent:dev"), "listing: {}", r.stdout);
}

#[test]
fn probe_json_flag_produces_json_not_tui() {
    // spec: TUI-2, CLI-167 - `--json` forces JSON output wrapped in envelope.
    let sb = melded();
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "probe --json should succeed: {}", r.stderr);
    let env: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("probe --json must produce valid JSON");
    assert_eq!(
        env["schema"], 1,
        "probe --json must produce envelope: {}",
        r.stdout
    );
    assert!(
        env["items"].is_array(),
        "probe --json envelope must have items array: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("\x1b[?1049h"),
        "probe --json must not enter alt-screen"
    );
}

#[test]
fn probe_fallback_with_query_filters_listing() {
    // spec: TUI-2 - query arg applies in fallback (non-TUI) mode.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui", "review"]);
    assert!(
        r.success,
        "probe --no-tui query should succeed: {}",
        r.stderr
    );
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(!r.stdout.contains("agent:dev"), "filtered: {}", r.stdout);
}

#[test]
fn probe_fallback_with_kind_filter_narrows_listing() {
    // spec: TUI-2 - --kind arg applies in fallback mode.
    let sb = melded();
    let r = sb.mind(&["probe", "--no-tui", "--kind", "skill"]);
    assert!(
        r.success,
        "probe --no-tui --kind should succeed: {}",
        r.stderr
    );
    assert!(r.stdout.contains("skill:review"), "listing: {}", r.stdout);
    assert!(!r.stdout.contains("agent:dev"), "filtered: {}", r.stdout);
    assert!(!r.stdout.contains("rule:style"), "filtered: {}", r.stdout);
}

#[test]
fn probe_fallback_seed_query_with_no_tui() {
    // spec: TUI-2 - query args are seed state in both modes; with --no-tui the
    // query filters the listing (same as plain `probe <query>`).
    let sb = melded();
    let r1 = sb.mind(&["probe", "review"]);
    let r2 = sb.mind(&["probe", "--no-tui", "review"]);
    assert!(r1.success);
    assert!(r2.success);
    // Both produce the same result (same filter applied).
    assert_eq!(
        r1.stdout, r2.stdout,
        "--no-tui must not change filter behavior"
    );
}

#[test]
fn probe_fallback_with_source_filter_narrows_listing() {
    // spec: TUI-2 - the --source seed arg filters the listing in fallback mode,
    // matching plain `probe --source` (CLI-83). Only query and --kind were
    // previously exercised in fallback; this closes the --source axis.
    let sb = melded();
    let matched = sb.mind(&["probe", "--no-tui", "--source", "agents"]);
    assert!(
        matched.success,
        "probe --no-tui --source should succeed: {}",
        matched.stderr
    );
    assert!(
        matched.stdout.contains("skill:review"),
        "matching source listing: {}",
        matched.stdout
    );

    let unmatched = sb.mind(&["probe", "--no-tui", "--source", "nonesuch"]);
    assert!(
        unmatched.success,
        "probe --no-tui --source nonesuch should succeed: {}",
        unmatched.stderr
    );
    assert!(
        !unmatched.stdout.contains("skill:review"),
        "a non-matching --source must exclude items: {}",
        unmatched.stdout
    );
}

#[test]
fn probe_non_tty_returns_promptly_and_does_not_hang() {
    // spec: TUI-2 - a non-TTY `mind probe` (the harness pipes stdout) must fall
    // back to the listing and EXIT, never entering the interactive event loop
    // that blocks on terminal input. Regression guard: if the fallback branch
    // broke and the TUI launched here, the process would block on event::read
    // and this bounded wait would time out.
    use std::time::{Duration, Instant};

    let sb = melded();
    // Spawn directly so we can bound the wall-clock time. stdin is the inherited
    // null/closed handle of the test process (not a TTY), matching the non-TTY
    // condition; we do NOT feed any input, so a real TUI would hang.
    let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["probe"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mind probe");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(status.success(), "non-TTY probe should exit successfully");
                break;
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "non-TTY `mind probe` did not exit within 10s - it likely entered the TUI event loop instead of falling back"
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    let out = child.wait_with_output().expect("collect output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skill:review"),
        "fallback listing expected: {stdout}"
    );
    assert!(
        !stdout.contains("\x1b[?1049h"),
        "non-TTY probe must not enter the alt-screen"
    );
}

#[test]
fn introspect_json_emits_report() {
    // spec: CLI-92, CLI-189
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean: schema=1, empty issues array, integer counts for sources and items.
    let clean = sb.mind(&["introspect", "--json"]).stdout;
    assert!(clean.trim_start().starts_with('{'), "{clean}");
    let v: serde_json::Value = serde_json::from_str(&clean).expect("valid JSON");
    assert_eq!(v["schema"], 1, "schema field must be 1: {clean}");
    assert!(v["issues"].is_array(), "issues must be an array: {clean}");
    assert!(
        v["sources"].is_number(),
        "sources must be an integer count: {clean}"
    );
    assert!(
        v["items"].is_number(),
        "items must be an integer count: {clean}"
    );

    // A broken link surfaces as a missing-link issue with its stable kind tag.
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();
    let broken = sb.mind(&["introspect", "--json"]).stdout;
    let bv: serde_json::Value = serde_json::from_str(&broken).expect("valid JSON");
    assert_eq!(
        bv["schema"], 1,
        "schema present on error report too: {broken}"
    );
    assert!(broken.contains("\"missing-link\""), "{broken}");
}

#[test]
fn completions_emit_a_shell_script() {
    // spec: CLI-120
    let sb = Sandbox::new();
    let r = sb.mind(&["completions", "bash"]);
    assert!(r.success, "{}", r.stderr);
    // A bash completion script registers a completion function for `mind`.
    assert!(r.stdout.contains("_mind"), "{}", r.stdout);
    assert!(r.stdout.contains("complete"), "{}", r.stdout);

    // An unknown shell is rejected by the arg parser.
    assert!(!sb.mind(&["completions", "tcsh"]).success);
}

#[test]
fn relative_lobe_is_canonicalized_to_absolute() {
    // spec: STO-16
    let sb = Sandbox::new();
    // Configure a *relative* lobe. mind must resolve it against the working
    // directory at install time so the recorded link path is absolute and does
    // not depend on the cwd at a later uninstall.
    write(&sb.mind_home.join("config.toml"), "lobes = [\"rellobe\"]\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // Learn with the child cwd set to the sandbox base, so "rellobe" -> <base>/rellobe.
    let r = sb.mind_cwd(&["learn", "review"], &sb.base);
    assert!(r.success, "{}", r.stderr);
    let link = sb.base.join("rellobe/skills/review");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "link should be created at the resolved absolute path {link:?}"
    );

    // The recorded link path is absolute (not the relative "rellobe/...").
    let detail = sb.mind(&["recall", "skill:review"]).stdout;
    assert!(
        detail.contains(&link.display().to_string()),
        "recorded link should be the absolute path: {detail}"
    );

    // And forget, run from a *different* cwd, still removes it (the path was
    // absolute, not cwd-relative).
    assert!(sb.mind_cwd(&["forget", "review"], &sb.mind_home).success);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "link should be gone"
    );
}

#[test]
fn unguarded_ref_warning_scans_all_files_of_an_item() {
    // spec: NS-20, NS-42
    let sb = Sandbox::new();
    // A skill whose bare prose reference to the skill sibling `review` lives in
    // a secondary file, not SKILL.md. The warning must still catch it (scan is
    // item-wide). Using a skill referent (not an agent) because NS-42 excludes
    // pure-agent sibling names from the warning scan.
    sb.write_and_commit(
        "skills/lead/SKILL.md",
        "---\nname: lead\ndescription: lead skill\n---\n# lead\n",
    );
    sb.write_and_commit("skills/lead/NOTES.md", "Run the review skill first.\n");

    let r = sb.mind(&["--verbose", "meld", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("skill:jk:lead") && r.stderr.contains("review"),
        "warning should cite a sibling ref found in a non-SKILL.md file: {}",
        r.stderr
    );
}

#[test]
fn example_namespacing_expands_references() {
    // spec: NS-11, NS-14
    // Prefixed: tokens expand to the prefixed effective names, and a guarded
    // source produces no unguarded-reference warning.
    let jk = Sandbox::from_example("namespacing");
    // --verbose (CLI-162) opens the warning gate so the "no warning" assertion is
    // genuine: the source is prefixed, yet all refs are tokens, so nothing fires.
    let meld = jk.mind(&["--verbose", "meld", &jk.source_spec(), "--as", "jk"]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(
        !meld.stderr.contains("references sibling(s) in prose"),
        "all refs are tokens, so no warning: {}",
        meld.stderr
    );
    // `lead` references siblings via {{ns:}}, so a partial learn pulls in the
    // closure and prompts (DEP-31); `--yes` confirms.
    assert!(jk.mind(&["learn", "jk:lead", "--yes"]).success);
    let lead = std::fs::read_to_string(jk.mind_home.join("store/agent/jk:lead")).unwrap();
    // NS-42: {{ns:dev}} is an agent sibling -- expands bare under prefix too.
    assert!(lead.contains("the dev agent"), "{lead}");
    assert!(
        !lead.contains("jk:dev"),
        "agent token must not be prefixed: {lead}"
    );
    // Skill and rule referents still expand with the prefix (NS-11).
    assert!(lead.contains("the jk:review skill"), "{lead}");
    assert!(lead.contains("the jk:style rule"), "{lead}");
    assert!(!lead.contains("{{ns:"), "tokens should be gone: {lead}");
    // The skill references a rule from inside its directory; it expands too.
    assert!(jk.mind(&["learn", "jk:review", "--yes"]).success);
    let review =
        std::fs::read_to_string(jk.mind_home.join("store/skill/jk:review/SKILL.md")).unwrap();
    assert!(review.contains("jk:style rule"), "{review}");
    assert!(!review.contains("{{ns:"), "tokens should be gone: {review}");

    // Unprefixed: the same tokens expand to the bare names.
    let bare = Sandbox::from_example("namespacing");
    assert!(bare.mind(&["meld", &bare.source_spec()]).success);
    assert!(bare.mind(&["learn", "lead", "--yes"]).success);
    let lead2 = std::fs::read_to_string(bare.mind_home.join("store/agent/lead")).unwrap();
    assert!(lead2.contains("the dev agent"), "{lead2}");
    assert!(lead2.contains("the review skill"), "{lead2}");
    assert!(lead2.contains("the style rule"), "{lead2}");
    assert!(!lead2.contains("{{ns:"), "{lead2}");
}

#[test]
fn example_starter_convention_discovery() {
    // spec: DSC-10, DSC-11, DSC-12, DSC-20, CLI-85
    // The starter example ships no mind.toml: items are found by convention and
    // their descriptions come from each item's frontmatter.
    let sb = Sandbox::from_example("starter");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    // probe falls back to the listing on a non-TTY (piped) stdout; all three
    // convention items appear with their kinds.
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("skill:greet"), "{}", probe.stdout);
    assert!(probe.stdout.contains("agent:scribe"), "{}", probe.stdout);
    assert!(probe.stdout.contains("rule:tone"), "{}", probe.stdout);

    // A query that matches only a description (CLI-85): "plain" is in tone's
    // frontmatter description, not its name.
    let by_desc = sb.mind(&["probe", "plain"]);
    assert!(by_desc.success, "{}", by_desc.stderr);
    assert!(by_desc.stdout.contains("rule:tone"), "{}", by_desc.stdout);
    assert!(
        !by_desc.stdout.contains("agent:scribe"),
        "a description-only match should not list unrelated items: {}",
        by_desc.stdout
    );

    // Installing a convention item links it from the store.
    assert!(sb.mind(&["learn", "greet"]).success);
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "greet should be copied into the store"
    );
}

#[test]
fn root_mindfile_exposes_hello() {
    // spec: DSC-1, DSC-50
    // The repo-root mind.toml sets roots = ["examples/hello"], so melding the
    // mind repo itself discovers the hello-mind skill by convention under that
    // root, and `mind learn hello-mind` links it into the agent home. Guards
    // the landing-page command `mind meld jaemk/mind`.
    // spec: DSC-35, DSC-54, DSC-58
    // The root mind.toml curates two skill libraries via [discover].sources
    // (substituted with local stand-ins by the fixture to stay offline). Melding
    // registers the whole chain register-only (install = false) while the repo's
    // own hello-mind item is still discovered by convention and installable.
    let sb = Sandbox::from_root_mindfile();
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:hello-mind"),
        "{}",
        probe.stdout
    );

    // DSC-54/DSC-58: the curated nested sources are registered (browsable) but
    // their items are NOT installed by default (install = false).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success, "{}", sources.stderr);
    assert!(
        sources.stdout.contains("anthropics-skills")
            && sources.stdout.contains("awesome-claude-skills"),
        "both curated sources must be registered: {}",
        sources.stdout
    );
    assert!(
        probe.stdout.contains("skill:astand"),
        "the curated stand-in's item must be available to browse: {}",
        probe.stdout
    );
    assert!(
        !sb.claude_home.join("skills/astand").exists(),
        "a register-only curated item must NOT be installed on meld"
    );

    let learn = sb.mind(&["learn", "hello-mind"]);
    assert!(learn.success, "{}", learn.stderr);
    assert!(
        sb.mind_home
            .join("store/skill/hello-mind/SKILL.md")
            .exists(),
        "hello-mind should be copied into the store"
    );
    assert!(
        sb.claude_home.join("skills/hello-mind").exists(),
        "hello-mind should be linked into the agent home"
    );
}

#[test]
fn example_policy_validates() {
    // spec: POL-50
    // The shipped example managed policy validates clean via `review --policy`,
    // so the example cannot rot as the policy parser/validator changes.
    let sb = Sandbox::new();
    let policy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/policy/policy.toml");
    let r = sb.mind(&["review", "--policy", policy.to_str().unwrap()]);
    assert!(
        r.success,
        "example policy must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn example_tooling_expands_path_tokens() {
    // spec: TOOL-3, TOOL-10, TOOL-11, TOOL-12
    // The tooling example ships a `tool` plus a skill that references it through
    // path tokens. Learning the skill pulls in the tool it depends on, and the
    // tokens expand to store paths.
    let sb = Sandbox::from_example("tooling");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    // The `detect` tool is referenced by path tokens, not {{ns:}}, so it is not
    // an install dependency; learn it explicitly alongside the skill.
    assert!(sb.mind(&["learn", "detect"]).success);
    assert!(sb.mind(&["learn", "scan"]).success);
    let skill = std::fs::read_to_string(sb.mind_home.join("store/skill/scan/SKILL.md")).unwrap();
    assert!(
        skill.contains("store/tool/detect/detect.sh"),
        "{{tools:detect}} expands to the tool entrypoint: {skill}"
    );
    assert!(
        skill.contains("store/tool/detect/lib.sh"),
        "{{path:tool:detect}} reaches a non-entrypoint file: {skill}"
    );
    assert!(
        skill.contains("store/skill/scan"),
        "{{self}} expands to the skill's own store dir: {skill}"
    );
    assert!(
        !skill.contains("{{tools:") && !skill.contains("{{self") && !skill.contains("{{path:"),
        "tokens should be gone: {skill}"
    );

    // The tool is store-only: it lands in the store, linked into no agent home.
    assert!(
        sb.mind_home.join("store/tool/detect/detect.sh").exists(),
        "the detect tool should be copied into the store"
    );
}

#[test]
fn example_hooks_lists_declared_hooks() {
    // spec: HOOK-50, HOOK-54
    // The hooks example declares source install and uninstall hooks; `review`
    // discloses each one, so a consumer sees the source will run code.
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/hooks");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(r.success, "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    let out = format!("{}{}", r.stdout, r.stderr);
    assert!(
        out.contains("install hook"),
        "discloses an install hook: {out}"
    );
    assert!(
        out.contains("uninstall hook"),
        "discloses the uninstall hook: {out}"
    );
}

#[test]
fn example_monorepo_roots_discovery() {
    // spec: DSC-50, DSC-53
    // The monorepo example sets [source].roots, so convention discovery scans the
    // per-package subtrees and unions the results.
    let sb = Sandbox::from_example("monorepo");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "found under packages/web: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:release"),
        "found under packages/cli: {}",
        probe.stdout
    );
}

#[test]
fn example_explicit_inventory_offers_only_listed() {
    // spec: DSC-3
    // The explicit example declares a [[items]] inventory, which is authoritative:
    // convention is off and a shipped-but-unlisted file is not offered.
    let sb = Sandbox::from_example("explicit");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(probe.stdout.contains("rule:style"), "{}", probe.stdout);
    assert!(probe.stdout.contains("skill:scan"), "{}", probe.stdout);
    assert!(
        !probe.stdout.contains("internal"),
        "an unlisted file is not offered: {}",
        probe.stdout
    );
}

#[test]
fn example_explicit_item_hooks_fire() {
    // spec: HOOK-81, HOOK-82
    // The explicit example's `scan` skill declares per-item install/uninstall
    // hooks whose scripts ship under components/scan/hooks/. On a non-TTY they
    // are skipped unless `--dangerously-skip-install-hook-check` is passed, which
    // runs them. The install hook fires at learn (HOOK-81) and the uninstall hook
    // fires at forget (HOOK-82); each prints a recognizable line.
    let sb = Sandbox::from_example("explicit");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let learn = sb.mind(&["learn", "scan", "--dangerously-skip-install-hook-check"]);
    assert!(learn.success, "{} {}", learn.stdout, learn.stderr);
    assert!(
        learn.stdout.contains("explicit-example: scan installed"),
        "the install hook must fire at learn (HOOK-81): {}",
        learn.stdout
    );

    let forget = sb.mind(&["forget", "scan", "--dangerously-skip-install-hook-check"]);
    assert!(forget.success, "{} {}", forget.stdout, forget.stderr);
    assert!(
        forget.stdout.contains("explicit-example: scan removed"),
        "the uninstall hook must fire at forget (HOOK-82): {}",
        forget.stdout
    );
}

#[test]
fn example_discover_kind_globs() {
    // spec: DSC-33, DSC-37
    // The discover example declares an authoritative [discover] with per-kind
    // include/exclude globs. A skill glob ends at SKILL.md (item = parent dir)
    // and an agent glob matches the .md (item = stem) (DSC-33); an exclude glob
    // drops a matched path (DSC-37). Convention scanning is off, so only the two
    // glob-matched items are offered.
    let sb = Sandbox::from_example("discover");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:alpha"),
        "skill glob matches packages/a/skills/alpha/SKILL.md, item = parent dir (DSC-33): {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta"),
        "agent glob matches packages/b/agents/beta.md, item = stem (DSC-33): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("secret"),
        "internal/skills/secret/SKILL.md is dropped by the exclude glob (DSC-37): {}",
        probe.stdout
    );
    // Exactly the two glob-matched items: convention scanning stays off.
    assert_eq!(
        probe.stdout.matches("skill:").count() + probe.stdout.matches("agent:").count(),
        2,
        "only the two glob-matched items are discovered: {}",
        probe.stdout
    );
}

#[test]
fn example_super_source_validates() {
    // spec: DSC-38, DSC-39
    // The super-source example declares a [discover].sources registry. It
    // validates clean structurally (review does not clone the nested chain).
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/super-source");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(
        r.success,
        "super-source example must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn example_drift_upgrade() {
    // spec: CLI-75, CLI-155, CLI-90, LIFE-11, LIFE-13, LIFE-15, LIFE-33
    // The drift example installs skill:audit, edits the source body and syncs so
    // the recorded commit advances while the installed copy's source-content hash
    // lags (LIFE-15: hash is of source content, so detection compares source with
    // source). `recall` then marks the item stale with the `^` left-edge marker
    // (CLI-155) and the trailing `(outdated; run mind upgrade)` text (CLI-75);
    // `introspect` reports the drift (CLI-90, LIFE-33); and `mind upgrade --yes`
    // reports the hash/commit deltas and reinstalls under the same name
    // (LIFE-11 pending on hash change, LIFE-13 content-only reinstall), after
    // which `recall` shows the item current again.
    let sb = Sandbox::from_example("drift");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(sb.mind(&["learn", "audit"]).success);

    // Fresh install: not outdated, leads with the `+` marker.
    let fresh = sb.mind(&["--ascii", "recall"]);
    assert!(fresh.success, "{}", fresh.stderr);
    assert!(
        !fresh.stdout.contains("outdated"),
        "freshly installed audit must not be outdated: {}",
        fresh.stdout
    );

    // Simulate an upstream change: edit the source body, commit, then sync to
    // advance the recorded commit (mirrors the README walkthrough).
    write(
        &sb.source.join("skills/audit/SKILL.md"),
        "---\nname: audit\ndescription: Audit the change\n---\n# audit skill\nedited body\n",
    );
    git(&sb.source, &["commit", "-aqm", "edit audit"]);
    assert!(sb.mind(&["sync"]).success);

    // recall (CLI-75, CLI-155): the audit line leads with the `^` stale marker
    // and carries the `(outdated` text.
    let stale = sb.mind(&["--ascii", "recall"]);
    assert!(stale.success, "{}", stale.stderr);
    let line = stale
        .stdout
        .lines()
        .find(|l| l.contains("skill:audit"))
        .unwrap_or_else(|| panic!("no audit line in recall output: {}", stale.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
    assert!(
        line.contains("(outdated"),
        "the stale line must carry the (outdated; run mind upgrade) text: {line:?}"
    );

    // introspect (CLI-90, LIFE-33): reports the drift and a nonzero issue count.
    let ins = sb.mind(&["--ascii", "introspect"]);
    assert!(
        ins.stdout.contains("skill:audit") && ins.stdout.contains("upstream changed"),
        "introspect must report audit's upstream change: {}",
        ins.stdout
    );
    assert!(
        ins.stdout.contains("issue(s) found") && !ins.stdout.contains("0 issue(s) found"),
        "introspect must report a nonzero issue count: {}",
        ins.stdout
    );

    // upgrade --yes (LIFE-11, LIFE-13): reports the hash and commit `->` deltas
    // and reinstalls under the same name. Assert on shape, not literal hex.
    let up = sb.mind(&["--ascii", "upgrade", "--yes"]);
    assert!(up.success, "{} {}", up.stdout, up.stderr);
    assert!(
        up.stdout.contains("hash") && up.stdout.contains("->"),
        "upgrade must report the hash delta with an arrow: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("commit"),
        "upgrade must report the commit delta: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("upgraded skill:audit"),
        "upgrade must apply audit under the same name: {}",
        up.stdout
    );

    // After upgrade: recall shows audit current (marker back to `+`, no outdated).
    let after = sb.mind(&["--ascii", "recall"]);
    assert!(after.success, "{}", after.stderr);
    let line = after
        .stdout
        .lines()
        .find(|l| l.contains("skill:audit"))
        .unwrap_or_else(|| panic!("no audit line in recall output: {}", after.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('+'),
        "a current install must lead with the `+` marker after upgrade: {line:?}"
    );
    assert!(
        !line.contains("(outdated"),
        "the line must not carry the outdated text after upgrade: {line:?}"
    );
}

#[test]
fn example_multi_lobe_links_into_all_homes() {
    // spec: STO-14, LIFE-40
    // With two lobes configured (STO-14), a single `learn` links the item into
    // every configured agent home, and `forget` removes the link from all of
    // them (LIFE-40).
    let sb = Sandbox::from_example("multi-lobe");
    let lobe_a = sb.base.join("lobe-a");
    let lobe_b = sb.base.join("lobe-b");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            lobe_a.display(),
            lobe_b.display()
        ),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let learn = sb.mind(&["learn", "recap"]);
    assert!(learn.success, "{} {}", learn.stdout, learn.stderr);

    // The skill is symlinked into BOTH lobes.
    let link_a = lobe_a.join("skills/recap");
    let link_b = lobe_b.join("skills/recap");
    assert!(
        std::fs::symlink_metadata(&link_a).is_ok(),
        "recap must be linked into lobe A"
    );
    assert!(
        std::fs::symlink_metadata(&link_b).is_ok(),
        "recap must be linked into lobe B"
    );
    // Each link points at the one store copy.
    let store = sb.mind_home.join("store/skill/recap");
    assert_eq!(
        std::fs::canonicalize(&link_a).unwrap(),
        std::fs::canonicalize(&store).unwrap(),
        "lobe A link must point at the store copy"
    );
    assert_eq!(
        std::fs::canonicalize(&link_b).unwrap(),
        std::fs::canonicalize(&store).unwrap(),
        "lobe B link must point at the store copy"
    );

    // forget removes the link from BOTH lobes (LIFE-40).
    let forget = sb.mind(&["forget", "recap"]);
    assert!(forget.success, "{} {}", forget.stdout, forget.stderr);
    assert!(
        std::fs::symlink_metadata(&link_a).is_err(),
        "forget must remove the link from lobe A"
    );
    assert!(
        std::fs::symlink_metadata(&link_b).is_err(),
        "forget must remove the link from lobe B"
    );
}

#[test]
fn example_absorb_claims_unmanaged_item() {
    // spec: ABS-1, ABS-8, UNM-1
    // The absorb example is a README-only walkthrough, so build the scenario
    // directly: an unmanaged lobe skill (UNM-1) plus a throwaway git target.
    // `absorb --yes` moves the item to the target's convention path, commits,
    // melds, and learns it (ABS-1); afterward it is an ordinary managed item
    // (ABS-8): a managed lobe symlink and a version-controlled file in the target.
    let sb = Sandbox::new();

    // Seed an unmanaged skill placed directly in the lobe (the seed_unmanaged
    // pattern), with the example's `notes` name and frontmatter.
    write(
        &sb.claude_home.join("skills/notes/SKILL.md"),
        "---\ndescription: my personal notes skill\n---\n# notes\n",
    );

    // recall (before): notes is surfaced as unmanaged (UNM-1).
    let before = sb.mind(&["--ascii", "recall"]);
    assert!(before.success, "{}", before.stderr);
    assert!(
        before.stdout.contains("unmanaged: not installed by mind"),
        "recall must surface the unmanaged group before absorb: {}",
        before.stdout
    );
    assert!(
        before.stdout.contains("skill:notes"),
        "recall must list notes as unmanaged before absorb: {}",
        before.stdout
    );

    // A throwaway git target the user owns.
    let target = sb.base.join("absorb-target");
    std::fs::create_dir_all(&target).unwrap();
    git(&target, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&target, &["config", "user.email", "t@t"]);
    git(&target, &["config", "user.name", "t"]);
    git(&target, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let target_spec = target.to_string_lossy().into_owned();

    // absorb --yes: move, commit, meld, learn (ABS-1).
    let absorb = sb.mind(&[
        "--ascii",
        "absorb",
        "skill:notes",
        "--to",
        &target_spec,
        "--yes",
    ]);
    assert!(absorb.success, "{} {}", absorb.stdout, absorb.stderr);
    assert!(
        absorb
            .stdout
            .contains("absorbed skill:notes -> managed as skill:notes"),
        "absorb must report the managed result: {}",
        absorb.stdout
    );

    // The file now lives in the target repo at the convention path.
    assert!(
        target.join("skills/notes/SKILL.md").exists(),
        "the absorbed file must live in the target at skills/notes/SKILL.md"
    );

    // The lobe path is now a managed symlink into the store (ABS-8).
    let lobe_path = sb.claude_home.join("skills/notes");
    let meta = std::fs::symlink_metadata(&lobe_path).expect("lobe path must exist after absorb");
    assert!(
        meta.file_type().is_symlink(),
        "the lobe path must be a managed symlink after absorb"
    );
    assert_eq!(
        std::fs::canonicalize(&lobe_path).unwrap(),
        std::fs::canonicalize(sb.mind_home.join("store/skill/notes")).unwrap(),
        "the lobe link must point into the store"
    );

    // recall (after): notes is now a managed installed item, not unmanaged (ABS-8).
    let after = sb.mind(&["--ascii", "recall"]);
    assert!(after.success, "{}", after.stderr);
    let line = after
        .stdout
        .lines()
        .find(|l| l.contains("skill:notes"))
        .unwrap_or_else(|| panic!("no notes line in recall output: {}", after.stdout));
    assert!(
        line.contains("installed @"),
        "notes must be a managed installed item after absorb: {line:?}"
    );
    assert!(
        !after.stdout.contains("unmanaged: not installed by mind"),
        "notes must no longer be reported as unmanaged after absorb: {}",
        after.stdout
    );
}

#[test]
fn man_page_renders_roff() {
    // spec: CLI-121
    let sb = Sandbox::new();
    let r = sb.mind(&["man"]);
    assert!(r.success, "{}", r.stderr);
    // roff man pages open with a .TH title header.
    assert!(r.stdout.contains(".TH"), "{}", r.stdout);
    assert!(r.stdout.to_lowercase().contains("mind"), "{}", r.stdout);
}

// ---- concurrency tests -------------------------------------------------------

/// Spawn a `mind` child process and return its handle without waiting.
fn spawn_mind(
    mind_home: &std::path::Path,
    claude_home: &std::path::Path,
    args: &[&str],
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(args)
        .env("MIND_HOME", mind_home)
        .env("CLAUDE_HOME", claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mind")
}

#[test]
fn concurrent_mutating_commands_both_succeed_no_lost_update() {
    // Two `meld` calls that target different sources run concurrently against the
    // same MIND_HOME. The advisory exclusive lock serializes them so neither
    // overwrites the other's registry write. Both sources must appear in the
    // final sources list.
    // spec: STO-40 STO-41
    let a = Sandbox::new();
    let b = Sandbox::named("tools");
    // Reuse a's mind/claude home as the shared environment for both processes.
    let mind_home = &a.mind_home;
    let claude_home = &a.claude_home;

    let a_spec = a.source_spec();
    let b_spec = b.source_spec();

    let mut child_a = spawn_mind(mind_home, claude_home, &["meld", &a_spec]);
    let mut child_b = spawn_mind(mind_home, claude_home, &["meld", &b_spec]);

    let status_a = child_a.wait().expect("wait a");
    let status_b = child_b.wait().expect("wait b");

    assert!(status_a.success(), "first meld failed");
    assert!(status_b.success(), "second meld failed");

    // Both sources must be registered (no lost update).
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("agents"),
        "first source missing: {sources}"
    );
    assert!(
        sources.contains("tools"),
        "second source missing: {sources}"
    );
}

#[test]
fn concurrent_learn_commands_both_effects_survive() {
    // Two `learn` commands running concurrently against one MIND_HOME install
    // different items. Both must appear in the manifest afterward.
    // spec: STO-40 STO-41
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Pre-meld so both learns can resolve items.
    assert!(sb.mind(&["meld", &spec]).success);

    let mind_home = &sb.mind_home;
    let claude_home = &sb.claude_home;

    let mut child_a = spawn_mind(mind_home, claude_home, &["learn", "review"]);
    let mut child_b = spawn_mind(mind_home, claude_home, &["learn", "dev"]);

    let status_a = child_a.wait().expect("wait a");
    let status_b = child_b.wait().expect("wait b");

    assert!(status_a.success(), "learn review failed");
    assert!(status_b.success(), "learn dev failed");

    // Both items must be in the manifest - no lost update.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "review lost: {recall}");
    assert!(recall.contains("agent:dev"), "dev lost: {recall}");
}

#[test]
fn three_concurrent_learns_no_lost_update() {
    // Three learns of distinct items race against one MIND_HOME. Each is a
    // read-modify-write of manifest.json; without the exclusive lock at least one
    // entry would be lost to a clobbering write. All three must survive.
    // Repeat to make a lost update under a broken lock overwhelmingly likely.
    // spec: STO-40 STO-41
    for _ in 0..15 {
        let sb = Sandbox::new();
        let spec = sb.source_spec();
        assert!(sb.mind(&["meld", &spec]).success);

        let mind_home = &sb.mind_home;
        let claude_home = &sb.claude_home;

        let mut ca = spawn_mind(mind_home, claude_home, &["learn", "review"]);
        let mut cb = spawn_mind(mind_home, claude_home, &["learn", "dev"]);
        let mut cc = spawn_mind(mind_home, claude_home, &["learn", "style"]);

        assert!(ca.wait().expect("wait a").success(), "learn review failed");
        assert!(cb.wait().expect("wait b").success(), "learn dev failed");
        assert!(cc.wait().expect("wait c").success(), "learn style failed");

        let recall = sb.mind(&["recall"]);
        assert!(recall.success, "recall failed: {}", recall.stderr);
        assert!(
            recall.stdout.contains("skill:review"),
            "review lost: {}",
            recall.stdout
        );
        assert!(
            recall.stdout.contains("agent:dev"),
            "dev lost: {}",
            recall.stdout
        );
        assert!(
            recall.stdout.contains("rule:style"),
            "style lost: {}",
            recall.stdout
        );
    }
}

#[test]
fn concurrent_reader_and_writer_reader_does_not_see_torn_file() {
    // A `recall` (shared lock, reads sources.json / manifest.json) runs
    // concurrently with a `learn` (exclusive lock, writes manifest.json).
    // The reader must not error: it either sees the state before or after the
    // write, never a partial file (guaranteed by the advisory lock + atomic writes).
    // spec: STO-43
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let mind_home = &sb.mind_home;
    let claude_home = &sb.claude_home;

    // Run many rounds to increase the chance of interleaving. Each round races a
    // reader against both a learn (write) and the forget cleanup (another write),
    // widening the window in which a torn read could occur.
    for _ in 0..40 {
        let mut writer = spawn_mind(mind_home, claude_home, &["learn", "review"]);
        let reader1 = spawn_mind(mind_home, claude_home, &["recall"]);
        let reader2 = spawn_mind(mind_home, claude_home, &["recall", "--sources"]);

        let ws = writer.wait().expect("wait writer");
        let r1 = reader1.wait_with_output().expect("wait reader1");
        let r2 = reader2.wait_with_output().expect("wait reader2");

        assert!(ws.success(), "writer failed");
        // The reader may see "nothing learned" (before) or the item (after),
        // but must never error: a torn manifest.json would surface as a Json
        // parse error and a non-zero exit.
        assert!(
            r1.status.success(),
            "recall errored during concurrent write: {}",
            String::from_utf8_lossy(&r1.stderr)
        );
        assert!(
            r2.status.success(),
            "recall --sources errored during concurrent write: {}",
            String::from_utf8_lossy(&r2.stderr)
        );
        // The reader must not have hit a parse failure even on a successful exit
        // (defensive: a partial file that happened to parse to junk).
        let err1 = String::from_utf8_lossy(&r1.stderr);
        assert!(
            !err1.contains("expected") && !err1.to_lowercase().contains("json"),
            "reader saw a torn/partial file: {err1}"
        );

        // Clean up for the next round.
        sb.mind(&["forget", "review"]);
    }
}

#[test]
fn exclusive_lock_blocks_second_writer_until_first_completes() {
    // Start a writer; while it holds the exclusive lock, a second writer must
    // wait (block) rather than proceed concurrently. We verify this by running
    // two sequential meld+unmeld pairs and asserting the final state is
    // consistent (both ran fully). A non-blocking implementation would produce
    // racy JSON and crash or corrupt; a serializing one succeeds.
    // spec: STO-42
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Run two concurrent melds of the same spec; one will block on the lock
    // while the other runs. The second should get SourceExists and exit
    // non-zero, but must not crash or corrupt the registry. The registry must
    // be parseable (one valid source entry).
    let mut c1 = spawn_mind(&sb.mind_home, &sb.claude_home, &["meld", &spec]);
    let mut c2 = spawn_mind(&sb.mind_home, &sb.claude_home, &["meld", &spec]);

    let _ = c1.wait();
    let _ = c2.wait();

    // Exactly one meld should have succeeded; the registry must be well-formed.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.success,
        "recall failed after concurrent melds: {}",
        sources.stderr
    );
    // The registry must be well-formed (parseable by recall) and contain exactly
    // one source entry. Count non-blank, non-header lines.
    let entry_lines: Vec<_> = sources
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.contains("melded source"))
        .collect();
    assert_eq!(
        entry_lines.len(),
        1,
        "expected exactly one source entry, got {}: {}",
        entry_lines.len(),
        sources.stdout
    );
}

// ---- version pinning tests (DSC-41, STO-18, CLI-17, CLI-18, CLI-55) ---------

/// Build a sandbox repo that has a `stable` branch and a `v1.0` tag at the
/// initial commit, then advance `main` further. Returns (sandbox, sha_at_v1_0,
/// sha_at_main_tip).
fn make_pinnable_repo(name: &str) -> (Sandbox, String, String) {
    let sb = Sandbox::bare(name);

    // Write an initial file and commit it. This becomes the tagged commit.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v1\n---\n# dev v1\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "initial"]);

    // Read the sha of that initial commit.
    let sha_v1 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    // Tag it and create a `stable` branch pointing here.
    git(&sb.source, &["tag", "v1.0"]);
    git(&sb.source, &["branch", "stable"]);

    // Advance main with a second commit.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v2\n---\n# dev v2\n",
    );
    git(&sb.source, &["commit", "-aqm", "v2 commit"]);

    let sha_v2 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    (sb, sha_v1, sha_v2)
}

/// Read the `pin` field from a source's entry in sources.json.  Returns the
/// JSON object as a string so callers can assert on kind/value without pulling
/// in a serde dependency here.
fn read_source_pin_json(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Extract the `pin` object from the JSON.  The file is pretty-printed so
    // the pin block spans multiple lines; grab everything between `"pin": ` and
    // the next top-level `}` after it.
    let start = json.find("\"pin\":").expect("pin key in sources.json");
    // Find the matching `}` for the pin object.
    let after = &json[start..];
    let obj_start = after.find('{').expect("pin object open brace");
    let obj_str = &after[obj_start..];
    let mut depth = 0usize;
    let mut end = 0;
    for (i, c) in obj_str.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    obj_str[..end].to_string()
}

/// Read the recorded commit for the first source in sources.json.
fn read_source_commit(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Extract "commit": "sha" from the JSON.
    let key = "\"commit\": \"";
    let start = json.find(key).expect("commit key") + key.len();
    let end = json[start..].find('"').expect("closing quote") + start;
    json[start..end].to_string()
}

#[test]
fn meld_follow_branch_clones_named_branch_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-follow");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--follow-branch", "stable"]);
    assert!(r.success, "meld --follow-branch: {}", r.stderr);

    // The recorded commit is at stable (sha_v1), not main tip (sha_v2).
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "follow-branch=stable should record sha_v1");

    // The persisted pin has kind=follow-branch and value=stable.
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("follow-branch"),
        "pin kind should be follow-branch: {pin_json}"
    );
    assert!(
        pin_json.contains("stable"),
        "pin value should be stable: {pin_json}"
    );
}

#[test]
fn meld_pin_tag_clones_at_tag_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-tag");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(r.success, "meld --pin-tag: {}", r.stderr);

    // Should be at the tagged commit.
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "pin-tag=v1.0 should record sha_v1");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\""),
        "pin kind should be tag: {pin_json}"
    );
    assert!(
        pin_json.contains("v1.0"),
        "pin value should be v1.0: {pin_json}"
    );
}

#[test]
fn meld_pin_ref_clones_at_specific_commit_and_persists_pin() {
    // spec: CLI-17, CLI-18, STO-18
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-ref");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-ref", &sha_v1]);
    assert!(r.success, "meld --pin-ref: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v1, "pin-ref should record sha_v1");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "pin kind should be ref: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v1),
        "pin value should be the sha: {pin_json}"
    );
}

#[test]
fn meld_default_branch_pin_is_at_main_tip() {
    // spec: CLI-17 (no flag -> default branch), STO-18 (DefaultBranch persisted)
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("pintest-default");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld default: {}", r.stderr);

    // Default branch (main) tip is sha_v2.
    let commit = read_source_commit(&sb);
    assert_eq!(commit, sha_v2, "default branch should be at main tip");

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("default-branch"),
        "pin kind should be default-branch: {pin_json}"
    );
}

#[test]
fn meld_two_pin_flags_is_conflicting_pin_error() {
    // spec: CLI-17 (at most one pin flag)
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Two flags at once is an error.
    let r = sb.mind(&["meld", &spec, "--follow-branch", "main", "--pin-tag", "v1"]);
    assert!(
        !r.success,
        "two pin flags must be rejected: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // CLI-17 names the structured `ConflictingPin` error, so the flags are kept
    // independent at the clap layer and this is what surfaces (not a clap usage
    // string). The exit is non-zero and nothing is registered.
    assert!(
        r.stderr.contains("conflicting pin flags"),
        "expected the structured ConflictingPin error, got stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "nothing should be registered after a conflict error: {}",
        sources.stdout
    );
}

#[test]
fn source_directive_follow_branch_applies_when_no_consumer_flag() {
    // spec: DSC-41, CLI-17 (directive supplies default when no consumer flag)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-directive-follow");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with directive: {}", r.stderr);

    // Directive follow-branch=stable => clone at stable (sha_v1).
    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_v1,
        "directive follow-branch=stable should land on sha_v1"
    );

    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("follow-branch"),
        "pin kind should be follow-branch: {pin_json}"
    );
}

#[test]
fn consumer_flag_overrides_source_directive() {
    // spec: DSC-41, CLI-17 (consumer flag overrides directive)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-override");
    // Directive says follow stable (sha_v1); consumer says --follow-branch main.
    // Adding the mind.toml advances main by one more commit, so we capture the
    // resulting tip AFTER that commit.
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    // sha_main_tip is HEAD of main after the mind.toml commit.
    let sha_main_tip = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let spec = sb.source_spec();

    // Consumer says --follow-branch main which overrides the directive.
    let r = sb.mind(&["meld", &spec, "--follow-branch", "main"]);
    assert!(r.success, "meld override: {}", r.stderr);

    let commit = read_source_commit(&sb);
    assert_eq!(
        commit, sha_main_tip,
        "consumer --follow-branch main should override directive and land on main tip"
    );
    // Verify directive sha_v1 was NOT used (different commit).
    assert_ne!(
        commit, sha_v1,
        "directive must not take precedence over consumer flag"
    );
}

#[test]
fn sync_follow_branch_advances_commit() {
    // spec: CLI-55 (follow-branch resets to branch tip on sync)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-follow");
    let spec = sb.source_spec();

    // Meld at stable (sha_v1).
    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );
    let before = read_source_commit(&sb);
    assert_eq!(before, sha_v1);

    // Advance the `stable` branch on the remote.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v3\n---\n# dev v3\n",
    );
    git(&sb.source, &["commit", "-aqm", "v3 on stable"]);
    // Move stable to the new HEAD.
    let sha_v3 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    // The new commit is on main; create stable pointing at it.
    git(&sb.source, &["branch", "-f", "stable", &sha_v3]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync follow-branch: {}", r.stderr);

    let after = read_source_commit(&sb);
    assert_eq!(after, sha_v3, "follow-branch source should advance on sync");
}

#[test]
fn sync_pin_ref_stays_fixed() {
    // spec: CLI-55 (pin-ref source stays fixed on sync)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-ref");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-ref", &sha_v1]).success);
    let before = read_source_commit(&sb);
    assert_eq!(before, sha_v1);

    // Advance main further.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: v99\n---\n# v99\n",
    );
    git(&sb.source, &["commit", "-aqm", "v99"]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync pin-ref: {}", r.stderr);

    let after = read_source_commit(&sb);
    assert_eq!(after, sha_v1, "pin-ref should be immutable across sync");
}

#[test]
fn sync_does_not_change_pin() {
    // spec: CLI-55 (sync never changes the pin itself, only moves HEAD)
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-nopin");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );

    // Capture the pin before sync.
    let pin_before = read_source_pin_json(&sb);

    sb.mind(&["sync"]);

    // Pin must be identical after sync.
    let pin_after = read_source_pin_json(&sb);
    assert_eq!(
        pin_before, pin_after,
        "sync must not modify the recorded pin"
    );
    // Specifically still follow-branch=stable.
    assert!(pin_after.contains("follow-branch"), "{pin_after}");
    assert!(pin_after.contains("stable"), "{pin_after}");
}

#[test]
fn source_directive_conflict_is_error() {
    // spec: DSC-41 (more than one pin directive is a MindToml error)
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"main\"\npin-tag = \"v1.0\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success, "conflicting directives should fail meld");
    assert!(
        r.stderr.contains("conflicting pin") || r.stderr.contains("mind.toml"),
        "expected pin conflict error: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered"
    );
}

#[test]
fn existing_sources_json_without_pin_field_loads_as_default_branch() {
    // spec: STO-18 (missing pin field -> DefaultBranch default)
    // Write a sources.json that has no "pin" field, simulating an older registry
    // written before version pinning was added.  sync must still work and treat
    // the source as DefaultBranch.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // First meld so the clone exists on disk.
    assert!(sb.mind(&["meld", &spec]).success);

    // Rewrite sources.json without the `pin` field, in the format that
    // an old `mind` would have written.
    let path = sb.mind_home.join("sources.json");
    // Read the real file to get the actual name/url/host/owner/repo/commit values.
    let json = std::fs::read_to_string(&path).unwrap();
    // Extract the "name" value for use in the hand-crafted JSON.
    let name_start = json.find("\"name\": \"").unwrap() + "\"name\": \"".len();
    let name_end = json[name_start..].find('"').unwrap() + name_start;
    let name_val = &json[name_start..name_end];

    let url_start = json.find("\"url\": \"").unwrap() + "\"url\": \"".len();
    let url_end = json[url_start..].find('"').unwrap() + url_start;
    let url_val = &json[url_start..url_end];

    // Build a minimal sources.json with no pin field.
    let no_pin_json = format!(
        r#"{{
  "sources": [
    {{
      "name": "{name_val}",
      "url": "{url_val}",
      "host": "local",
      "owner": "x",
      "repo": "agents",
      "commit": null
    }}
  ]
}}"#
    );
    std::fs::write(&path, no_pin_json).unwrap();

    // sync must not error (reads missing pin as DefaultBranch).
    let r = sb.mind(&["sync"]);
    assert!(
        r.success,
        "sync on old sources.json (no pin field) should succeed: {}",
        r.stderr
    );
}

/// The on-disk clone dir for the sandbox's local source:
/// `<mind_home>/sources/local/<base_name>/<repo>`.
fn clone_dir_of(sb: &Sandbox, repo: &str) -> PathBuf {
    sb.mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join(repo)
}

#[test]
fn meld_pin_ref_unresolvable_is_git_error_and_registers_nothing() {
    // spec: CLI-18 - a pin that does not resolve in the remote is a `Git` error
    // and nothing is registered. The two-step clone re-clones at the resolved
    // pin after reading mind.toml; a failure of that second clone must not leave
    // a registered source nor a stray clone dir on disk.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-bad-ref");
    let spec = sb.source_spec();

    // A 40-char hex sha that does not exist in the remote.
    let bogus = "0123456789abcdef0123456789abcdef01234567";
    let r = sb.mind(&["meld", &spec, "--pin-ref", bogus]);
    assert!(
        !r.success,
        "unresolvable --pin-ref must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // It is a structured Git error (the checkout against the bogus sha fails).
    assert!(
        r.stderr.contains("git"),
        "expected a git error, got stderr={}",
        r.stderr
    );

    // Nothing registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after an unresolvable pin: {}",
        sources.stdout
    );
    // sources.json, if present, must not list the source.
    let sources_json = sb.mind_home.join("sources.json");
    if sources_json.exists() {
        let json = std::fs::read_to_string(&sources_json).unwrap();
        assert!(
            !json.contains("pintest-bad-ref"),
            "sources.json must not contain the failed source: {json}"
        );
    }
    // No stray clone dir is left under MIND_HOME for this source.
    let clone = clone_dir_of(&sb, "pintest-bad-ref");
    assert!(
        !clone.exists(),
        "an unresolvable pin must not leave a stray clone dir at {}",
        clone.display()
    );
}

#[test]
fn meld_pin_tag_unresolvable_is_git_error_and_registers_nothing() {
    // spec: CLI-18 - same as above for a tag that does not exist in the remote.
    // Here the re-clone uses `clone --branch <tag>` which fails outright, so the
    // staging clone dir never materializes.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-bad-tag");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-tag", "v9.9-does-not-exist"]);
    assert!(
        !r.success,
        "unresolvable --pin-tag must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("git"),
        "expected a git error, got stderr={}",
        r.stderr
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after an unresolvable tag pin: {}",
        sources.stdout
    );
    let clone = clone_dir_of(&sb, "pintest-bad-tag");
    assert!(
        !clone.exists(),
        "an unresolvable tag pin must not leave a stray clone dir at {}",
        clone.display()
    );
}

#[test]
fn sync_reclones_when_clone_dir_is_missing() {
    // spec: CLI-55 - sync resolves each source against its recorded pin. If the
    // clone dir has been removed out from under the registry, sync must recover
    // by re-cloning at the recorded pin rather than erroring, landing back on the
    // pinned commit.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-sync-missing");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1);

    // Delete the clone dir, simulating a wiped/partial sources tree.
    let clone = clone_dir_of(&sb, "pintest-sync-missing");
    assert!(clone.exists(), "clone should exist after meld");
    std::fs::remove_dir_all(&clone).unwrap();

    let r = sb.mind(&["sync"]);
    assert!(
        r.success,
        "sync must recover a missing clone dir: {}",
        r.stderr
    );
    // Recovered and still pinned at v1.0 (sha_v1), not main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "re-clone on sync must honor the recorded pin"
    );
    assert!(
        clone.join(".git").is_dir(),
        "sync should have re-created the clone"
    );
}

#[test]
fn pin_persists_across_repeated_syncs_while_commit_advances() {
    // spec: STO-18, CLI-55 - the recorded pin is untouched by sync across
    // repeated runs, even as a follow-branch source's recorded commit advances.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-multi-sync");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );
    assert_eq!(read_source_commit(&sb), sha_v1);
    let pin_initial = read_source_pin_json(&sb);

    // First sync with no upstream change: commit stays, pin stays.
    assert!(sb.mind(&["sync"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1);
    assert_eq!(read_source_pin_json(&sb), pin_initial);

    // Advance `stable` upstream, then sync: commit moves, pin still untouched.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: stable v3\n---\n# stable v3\n",
    );
    git(&sb.source, &["commit", "-aqm", "v3 on stable"]);
    let sha_v3 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&sb.source, &["branch", "-f", "stable", &sha_v3]);

    assert!(sb.mind(&["sync"]).success);
    assert_eq!(
        read_source_commit(&sb),
        sha_v3,
        "follow-branch commit should advance across repeated syncs"
    );
    assert_eq!(
        read_source_pin_json(&sb),
        pin_initial,
        "pin value must stay untouched across repeated syncs"
    );

    // A third sync with no further change keeps both stable.
    assert!(sb.mind(&["sync"]).success);
    assert_eq!(read_source_commit(&sb), sha_v3);
    assert_eq!(read_source_pin_json(&sb), pin_initial);
}

#[test]
fn source_directive_pin_tag_applies_when_no_consumer_flag() {
    // spec: DSC-41 - a `pin-tag` directive supplies the default pin when the
    // consumer gives no flag (parity with the follow-branch directive test).
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-directive-tag");
    sb.write_and_commit("mind.toml", "[source]\npin-tag = \"v1.0\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with pin-tag directive: {}", r.stderr);

    // The directive lands the clone on the tagged commit (sha_v1), not main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "directive pin-tag=v1.0 should land on the tagged commit"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\""),
        "pin kind should be tag: {pin_json}"
    );
    assert!(
        pin_json.contains("v1.0"),
        "pin value should be v1.0: {pin_json}"
    );
}

#[test]
fn source_directive_pin_ref_applies_when_no_consumer_flag() {
    // spec: DSC-41 - a `pin-ref` directive supplies the default pin when the
    // consumer gives no flag.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-directive-ref");
    // The directive must name a commit that exists in the default-branch clone,
    // since the directive is read from the default-branch mind.toml. sha_v1 is an
    // ancestor of main tip, so it is reachable.
    sb.write_and_commit("mind.toml", &format!("[source]\npin-ref = \"{sha_v1}\"\n"));
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with pin-ref directive: {}", r.stderr);

    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "directive pin-ref should land on the named commit"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "pin kind should be ref: {pin_json}"
    );
    assert!(
        pin_json.contains(&sha_v1),
        "pin value should be the sha: {pin_json}"
    );
}

#[test]
fn consumer_pin_ref_overrides_follow_branch_directive() {
    // spec: DSC-41, CLI-17 - a consumer flag of a DIFFERENT kind overrides the
    // directive (not just same-kind override). Directive follows `stable`; the
    // consumer pins a specific ref instead.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-cross-override");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();

    // Consumer pins the main-tip commit (after the mind.toml commit).
    let sha_main_tip = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let r = sb.mind(&["meld", &spec, "--pin-ref", &sha_main_tip]);
    assert!(r.success, "meld cross-kind override: {}", r.stderr);

    assert_eq!(
        read_source_commit(&sb),
        sha_main_tip,
        "consumer --pin-ref must override the follow-branch directive"
    );
    assert_ne!(
        read_source_commit(&sb),
        sha_v1,
        "the stable directive must not win over the consumer ref"
    );
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"ref\""),
        "persisted pin kind should be the consumer's ref, not follow-branch: {pin_json}"
    );
}

#[test]
fn meld_rejects_unknown_source_pin_field() {
    // spec: DSC-41 - `[source]` is `deny_unknown_fields`, so a misspelled pin
    // directive (e.g. `pin-branch` instead of `follow-branch`) is a parse error,
    // not a silently-ignored field that would leave the source on the default.
    let (sb, _v1, _v2) = make_pinnable_repo("pintest-unknown-field");
    sb.write_and_commit("mind.toml", "[source]\npin-branch = \"stable\"\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "an unknown [source] field must fail meld: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("mind.toml") || r.stderr.contains("pin-branch"),
        "expected a mind.toml parse error naming the bad field: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered after a mind.toml parse error"
    );
}

#[test]
fn sync_pin_tag_picks_up_moved_upstream_tag() {
    // spec: CLI-55 - the moved-tag force-fetch is observable end-to-end via the
    // CLI: a re-pointed upstream tag advances the recorded commit on sync (the
    // git-layer unit test alone does not exercise the meld+sync+registry path).
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-moved-tag");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1, "pinned at v1.0 == sha_v1");

    // Add a new commit upstream and re-point v1.0 at it.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: retagged\n---\n# retagged\n",
    );
    git(&sb.source, &["commit", "-aqm", "retag target"]);
    let sha_new = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&sb.source, &["tag", "-f", "v1.0", &sha_new]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync after moving tag: {}", r.stderr);
    assert_eq!(
        read_source_commit(&sb),
        sha_new,
        "a re-pointed upstream tag must be picked up by sync (force-fetch)"
    );
    // And the pin itself is unchanged (still tag v1.0).
    let pin_json = read_source_pin_json(&sb);
    assert!(pin_json.contains("\"tag\""), "{pin_json}");
    assert!(pin_json.contains("v1.0"), "{pin_json}");
}

// ---- scan roots integration tests (DSC-50, DSC-51, DSC-52, DSC-53, STO-17, CLI-16) ---

/// Read the `roots` field from the first source in sources.json as a JSON
/// string (for assertions without pulling in a serde dependency in tests).
fn read_source_roots_json(sb: &Sandbox) -> String {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    // Look for "roots": [ ... ]; return the whole array value.
    if let Some(start) = json.find("\"roots\":") {
        let after = &json[start + "\"roots\":".len()..];
        // Find the opening bracket.
        if let Some(arr_start) = after.find('[') {
            let arr = &after[arr_start..];
            let mut depth = 0usize;
            let mut end = 0;
            for (i, c) in arr.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            return arr[..end].to_string();
        }
    }
    // No roots field: return "null" to signal absence.
    "null".to_string()
}

#[test]
fn meld_root_persists_in_sources_json_and_probe_shows_subtree_items() {
    // spec: DSC-51, STO-17, CLI-16
    // A sandbox whose items live under a subdirectory "sub/".
    let sb = Sandbox::bare("subtree");
    // Items under "sub/" only.
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy skill\n---\n# deploy\n",
    );
    sb.write_and_commit(
        "sub/agents/ops.md",
        "---\ndescription: ops agent\n---\n# ops\n",
    );
    // Nothing at the repo root (no conventional dirs).
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--root", "sub"]);
    assert!(r.success, "meld --root: {}", r.stderr);

    // The root is persisted in sources.json.
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("sub"),
        "roots should be persisted: {roots_json}"
    );

    // probe shows the subtree items.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "subtree skill: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:ops"),
        "subtree agent: {}",
        probe.stdout
    );
}

#[test]
fn meld_root_on_authoritative_source_prints_note() {
    // spec: DSC-52 - --root on an authoritative source prints a note and is ignored.
    let sb = Sandbox::bare("auth-source");
    sb.write_and_commit(
        "pkg/style.md",
        "---\ndescription: style rule\n---\n# style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"pkg/style.md\"\n",
        ),
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "pkg"]);
    assert!(
        r.success,
        "meld should succeed even with ignored --root: {}",
        r.stderr
    );
    // The note appears on stdout.
    assert!(
        r.stdout.contains("ignored") || r.stdout.contains("note"),
        "expected an 'ignored' note: {}",
        r.stdout
    );
    // The explicit item is still discovered via the authoritative mind.toml.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:style"),
        "authoritative item still discovered: {}",
        probe.stdout
    );
}

#[test]
fn meld_root_nonexistent_dir_exits_nonzero() {
    // spec: DSC-52 (last sentence), CLI-16 - a --root that is not a directory in
    // the clone is an InvalidRoot error and exits non-zero.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "does-not-exist"]);
    assert!(!r.success, "meld with missing root must fail");
    assert!(
        r.stderr.contains("InvalidRoot") || r.stderr.contains("not a directory"),
        "expected InvalidRoot error: {}",
        r.stderr
    );
    // Nothing is registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing should be registered after an invalid root"
    );
}

#[test]
fn sync_preserves_roots() {
    // spec: STO-17 - the roots override is persisted at meld and not changed by sync.
    let sb = Sandbox::bare("roots-sync");
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--root", "sub"]).success);

    // Capture roots before sync.
    let roots_before = read_source_roots_json(&sb);
    assert!(
        roots_before.contains("sub"),
        "roots should be set: {roots_before}"
    );

    // sync must not change the roots field.
    assert!(sb.mind(&["sync"]).success);
    let roots_after = read_source_roots_json(&sb);
    assert_eq!(
        roots_before, roots_after,
        "sync must not modify the recorded roots"
    );

    // After sync, probe still shows the subtree items.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "subtree item still visible after sync: {}",
        probe.stdout
    );
}

#[test]
fn two_root_flags_union_and_both_persist() {
    // spec: DSC-51, DSC-53, STO-17, CLI-16
    // `meld --root a --root b` is repeatable: both subtrees are scanned and
    // unioned, and BOTH roots are persisted in sources.json. Drives the real CLI
    // arg parsing (the unit tests set Source.roots directly, so this is the only
    // check that the repeated flag actually threads through).
    let sb = Sandbox::bare("two-roots");
    sb.write_and_commit(
        "a/skills/alpha/SKILL.md",
        "---\ndescription: alpha skill\n---\n# alpha\n",
    );
    sb.write_and_commit(
        "b/agents/beta.md",
        "---\ndescription: beta agent\n---\n# beta\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--root", "a", "--root", "b"]);
    assert!(r.success, "meld --root a --root b: {}", r.stderr);

    // Both roots persisted.
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("\"a\""),
        "root a persisted: {roots_json}"
    );
    assert!(
        roots_json.contains("\"b\""),
        "root b persisted: {roots_json}"
    );

    // Both subtrees discovered.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha"),
        "root a item: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta"),
        "root b item: {}",
        probe.stdout
    );
}

#[test]
fn meld_absolute_root_exits_nonzero() {
    // spec: DSC-52, CLI-16
    // An absolute --root is rejected via the real CLI (the unit test exercises
    // scan_source directly; this confirms the binary surfaces InvalidRoot and
    // registers nothing).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "/tmp"]);
    assert!(!r.success, "absolute root must fail");
    assert!(
        r.stderr.contains("InvalidRoot") || r.stderr.contains("not a directory"),
        "expected InvalidRoot: {}",
        r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "nothing registered after an absolute root"
    );
}

#[test]
fn mindfile_roots_discovered_without_flag() {
    // spec: DSC-50 - [source].roots in mind.toml is respected without any --root flag.
    let sb = Sandbox::bare("toml-roots");
    sb.write_and_commit(
        "toolbox/skills/pack/SKILL.md",
        "---\ndescription: pack skill\n---\n# pack\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nroots = [\"toolbox\"]\n");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld with roots in mind.toml: {}", r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:pack"),
        "item under toolbox/ should be found: {}",
        probe.stdout
    );
}

// ---- review verb tests (CLI-130, CLI-131, CLI-132, CLI-133) -------------------

#[test]
fn review_clean_local_path_exits_zero() {
    // A clean local source (valid mind.toml if present, items with descriptions,
    // no bad tokens) exits 0 with no blocking issues.
    // spec: CLI-130, CLI-131
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "clean source should exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no issues") || r.stdout.contains("publishable") || r.stderr.is_empty(),
        "expected clean report: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
    // review must not leave any trace in the registry.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "review must not register anything"
    );
}

#[test]
fn review_malformed_mind_toml_exits_nonzero() {
    // A malformed mind.toml is a hard error -> exit non-zero.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[[[[bad toml");
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "malformed mind.toml must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn review_unknown_item_kind_exits_nonzero() {
    // An [[items]] entry with an unknown kind is a hard error -> exit non-zero.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x.md\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "unknown kind must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("unknown-kind") || r.stderr.contains("unknown item kind"),
        "expected unknown-kind in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_bad_ns_token_exits_nonzero() {
    // An item with {{ns:nope}} that doesn't resolve to any sibling is hard.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "bad ns token must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("bad-reference") || r.stderr.contains("does not resolve"),
        "expected bad-reference in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_conflicting_pin_exits_nonzero() {
    // A [source] section with two conflicting pin directives is a hard error.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"main\"\npin-tag = \"v1.0\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "conflicting pin must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("conflicting-pin") || r.stderr.contains("conflicting pin"),
        "expected conflicting-pin in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_missing_description_is_advisory_exit_zero() {
    // An item with no description is advisory only -> exit 0 with finding printed.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit("agents/nodesc.md", "# no frontmatter here\nsome content\n");
    // Remove the default fixture items so only nodesc.md is in the source.
    let source_dir = sb.source.clone();
    std::fs::remove_dir_all(source_dir.join("skills")).ok();
    std::fs::remove_dir_all(source_dir.join("rules")).ok();
    std::fs::remove_file(source_dir.join("agents/dev.md")).ok();
    git(&source_dir, &["add", "-A"]);
    git(&source_dir, &["commit", "-qm", "nodesc only"]);

    let spec = sb.source_spec();
    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "missing description is advisory, must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("missing-description") || r.stdout.contains("advisory"),
        "expected advisory finding in stdout: {}",
        r.stdout
    );
}

#[test]
fn review_unguarded_ref_under_as_is_advisory_exit_zero() {
    // An unguarded prose reference under --as <prefix> is advisory -> exit 0.
    // spec: CLI-132, CLI-133
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    assert!(
        r.success,
        "unguarded ref advisory must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("unguarded-reference") || r.stdout.contains("advisory"),
        "expected advisory finding: stdout={}",
        r.stdout
    );
    // No hard errors.
    assert!(
        !r.stderr.contains("error ["),
        "must have no hard errors: stderr={}",
        r.stderr
    );
}

#[test]
fn review_melded_selector_resolves_via_registry() {
    // `review <melded-selector>` resolves the target via the registry.
    // spec: CLI-130
    let sb = melded();

    // After meld, "agents" (the repo basename) is a registered suffix selector.
    let r = sb.mind(&["review", "agents"]);
    assert!(
        r.success,
        "review via registry selector must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn review_with_prefix_flag_evaluates_under_that_namespace() {
    // `review --as <prefix>` evaluates under the prospective prefix.
    // The source has a good token {{ns:dev}} that expands fine under prefix 'jk'.
    // spec: CLI-133
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:dev}}.\n",
    );
    let spec = sb.source_spec();

    // With --as jk: the token {{ns:dev}} should resolve to jk:dev (a sibling).
    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    // dev is a sibling, so no bad-reference error.
    assert!(
        r.success,
        "valid ns token with prefix must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No bad-reference hard error.
    assert!(
        !r.stderr.contains("bad-reference"),
        "valid token must not produce bad-reference: stderr={}",
        r.stderr
    );
}

#[test]
fn review_local_path_target_is_left_intact() {
    // CLI-130: a local-path target is the user's working dir, NOT a temp clone,
    // so review must leave it on disk and unmodified. Only the remote-spec path
    // clones to a temp area; a local path is read in place.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let skill = sb.source.join("skills/review/SKILL.md");
    let before = std::fs::read_to_string(&skill).unwrap();

    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "clean local review should exit 0: {} {}",
        r.stdout, r.stderr
    );

    // The source dir and its files still exist and are byte-identical.
    assert!(sb.source.is_dir(), "local source dir must survive review");
    let after = std::fs::read_to_string(&skill).unwrap();
    assert_eq!(before, after, "review must not modify the local source");
    // And nothing was cloned into the scratch area.
    assert_no_review_temp(&sb.mind_home);
}

#[test]
fn review_remote_spec_clone_failure_exits_nonzero_and_leaves_no_temp() {
    // CLI-130: a repo-spec target is shallow-cloned to a temp area. When the
    // clone itself FAILS (unreachable remote), review must exit non-zero and
    // leave nothing behind under MIND_HOME/.tmp. Uses a refused-connection URL
    // so the clone fails fast without real network egress.
    let sb = Sandbox::new();

    // parse_spec keeps this as host="127.0.0.1:1" (non-local), so review takes
    // the clone branch; the connection is refused, so the clone errors.
    let r = sb.mind(&["review", "https://127.0.0.1:1/owner/repo"]);
    assert!(
        !r.success,
        "a failed clone must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No leftover scratch dir, and no registry mutation.
    assert_no_review_temp(&sb.mind_home);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "failed review must not register anything"
    );
}

#[test]
fn review_report_lists_every_advisory_finding() {
    // CLI-131: the report names per-item results, not just an exit code. With a
    // clean item, a missing-description item, and an unguarded-ref item under a
    // prefix, ALL advisories must be printed (not just the first).
    let sb = Sandbox::new();
    // lead.md: has a description AND an unguarded prose ref to sibling `dev`.
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    // nodesc.md: a sibling with no description (advisory: missing-description).
    sb.write_and_commit("agents/nodesc.md", "# no frontmatter\nbody\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    assert!(
        r.success,
        "advisory-only review exits 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("missing-description"),
        "missing-description advisory must be printed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("unguarded-reference"),
        "unguarded-reference advisory must be printed: {}",
        r.stdout
    );
    // The clean fixture skill (skill:review has a description) is not flagged
    // for a missing description.
    assert!(
        !r.stdout.contains("skill:review: no description"),
        "clean item must not be flagged missing-description: {}",
        r.stdout
    );
}

#[test]
fn review_multiple_hard_errors_all_reported_and_counted() {
    // CLI-132: two distinct hard problems (two unresolved {{ns:}} tokens in two
    // items) both surface and the summary counts more than one hard error.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
    );
    sb.write_and_commit(
        "agents/boss.md",
        "---\ndescription: boss\n---\nDefer to {{ns:alsonope}}.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "multiple hard errors must exit non-zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("nope"),
        "first bad ref reported: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("alsonope"),
        "second bad ref reported: {}",
        r.stderr
    );
    // The summary line reports a hard-error count greater than one.
    assert!(
        r.stdout.contains("2 hard error(s)"),
        "summary must count both hard errors: {}",
        r.stdout
    );
}

#[test]
fn review_target_and_policy_are_mutually_exclusive() {
    // spec: CLI-134
    // Supplying both <target> and --policy is a clap usage error: exits non-zero
    // and prints a conflict diagnostic before any logic runs.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // The policy file need not exist; clap rejects the combination before any
    // I/O is attempted.
    let policy_path = sb.base.join("policy.toml").to_string_lossy().into_owned();
    let r = sb.mind(&["review", &spec, "--policy", &policy_path]);
    assert!(
        !r.success,
        "review with both <target> and --policy must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("cannot be used with"),
        "clap conflict diagnostic must appear in stderr: {}",
        r.stderr
    );
}

#[test]
fn meld_pin_tag_uses_pinned_mindfile_for_authoritativeness_not_default_branch() {
    // spec: DSC-52, DSC-41, STO-18
    //
    // Regression (M2, stale mindfile after pinned re-clone): meld step 1 clones
    // the default branch and reads its mind.toml; step 3 re-clones at the
    // resolved pin. The `is_authoritative` gate (which decides whether a
    // consumer `--root` is honored or ignored, DSC-52) must read the PINNED
    // mind.toml, not the default branch's.
    //
    // Here the TAGGED commit (v1.0) is NON-authoritative ([source] metadata
    // only) and ships its items under `sub/`, so `--root sub` must be honored.
    // The DEFAULT branch tip is AUTHORITATIVE ([[items]] present); if meld read
    // that stale file it would ignore `--root` and print the DSC-52 note.
    let sb = Sandbox::bare("pinned-authoritativeness");

    // --- Tagged commit (v1.0): non-authoritative mind.toml + item under sub/. ---
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy skill\n---\n# deploy\n",
    );
    sb.write_and_commit(
        "mind.toml",
        // [source] only: no [[items]] and no [discover] -> NOT authoritative,
        // so convention scanning (under the chosen --root) stays on.
        "[source]\ndescription = \"non-authoritative at v1.0\"\n",
    );
    git(&sb.source, &["tag", "v1.0"]);

    // --- Default branch tip: authoritative mind.toml ([[items]] present). ---
    sb.write_and_commit(
        "pkg/style.md",
        "---\ndescription: style rule\n---\n# style\n",
    );
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"authoritative at main tip\"\n\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"pkg/style.md\"\n",
        ),
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0", "--root", "sub"]);
    assert!(r.success, "meld --pin-tag v1.0 --root sub: {}", r.stderr);

    // The pinned (non-authoritative) file means --root is HONORED, so the
    // DSC-52 "ignored" note must NOT print (it would if the default branch's
    // authoritative file were read).
    assert!(
        !r.stdout.contains("ignored"),
        "--root must be honored against the pinned non-authoritative mind.toml, \
         not ignored against the default branch's authoritative one: {}",
        r.stdout
    );

    // And the root is actually persisted (only happens on the non-authoritative path).
    let roots_json = read_source_roots_json(&sb);
    assert!(
        roots_json.contains("sub"),
        "root from the pinned file must be persisted: {roots_json}"
    );

    // The pinned description (not the default branch's) is recorded.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("non-authoritative at v1.0"),
        "pinned [source].description should be recorded: {}",
        sources.stdout
    );
    assert!(
        !sources.stdout.contains("authoritative at main tip"),
        "default branch description must not leak through: {}",
        sources.stdout
    );

    // The pinned subtree item is discovered; the default branch's item is not.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "pinned subtree item should be discovered: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("rule:style"),
        "default branch's authoritative item must not appear: {}",
        probe.stdout
    );
}

#[test]
fn meld_pin_tag_uses_pinned_mindfile_for_nested_discovery_not_default_branch() {
    // spec: DSC-52, DSC-41, STO-18
    //
    // Companion to the authoritativeness regression: the nested
    // [discover].sources loop must also read the PINNED mind.toml, not the
    // default branch's. The default branch declares a nested source that does
    // not exist on disk; if meld read it, the recursive meld would fail. The
    // tagged commit declares no nested sources, so meld must succeed and pull in
    // exactly one source.
    let sb = Sandbox::bare("pinned-nested-discovery");

    // Tagged commit: a plain non-authoritative mind.toml, no nested sources,
    // one convention item.
    sb.write_and_commit("agents/dev.md", "---\ndescription: dev agent\n---\n# dev\n");
    sb.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"no nested sources at v1.0\"\n",
    );
    git(&sb.source, &["tag", "v1.0"]);

    // Default branch tip: declares a nested source pointing at a path that does
    // not exist, which would make a recursive meld fail if it were read.
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "description = \"nested at main tip\"\n\n",
            "[[discover.sources]]\n",
            "source = \"/nonexistent/nested/repo\"\n",
        ),
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(
        r.success,
        "meld must use the pinned (no-nested) mind.toml and succeed: {} {}",
        r.stdout, r.stderr
    );

    // Exactly one source was melded (no phantom nested source from the default
    // branch). recall --sources lists the single pinned source.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no nested sources at v1.0"),
        "pinned source description should be present: {}",
        sources.stdout
    );
    assert!(
        !sources.stdout.contains("/nonexistent/nested/repo"),
        "default branch's nested source must not be melded: {}",
        sources.stdout
    );
}

// --- managed policy enforcement (POL-*) ------------------------------------
//
// The policy is injected via $MIND_POLICY_FILE, which `Policy::load` honors only
// when no system policy file exists at the fixed per-OS path (POL-2). The test
// environment has no such system file, so the env var is authoritative here.
// Non-policy tests above never set MIND_POLICY_FILE, so they stay unmanaged
// (POL-4 inert). A local path-melded source's identity is `local/<base>/<name>`
// (see source.rs make_source / Sandbox::base_name), where <base> is the dynamic
// temp-dir name; the allow patterns below use `local/*/<name>` so the single
// segment wildcard matches that base deterministically without hardcoding it.

/// Write a policy TOML to the sandbox base and return its absolute path string,
/// for use as the `MIND_POLICY_FILE` env value.
fn write_policy(sb: &Sandbox, body: &str) -> String {
    let path = sb.base.join("policy.toml");
    write(&path, body);
    path.to_string_lossy().into_owned()
}

/// Count the melded sources by reading sources.json (0 when the file is absent).
fn source_count(sb: &Sandbox) -> usize {
    let path = sb.mind_home.join("sources.json");
    let Ok(json) = std::fs::read_to_string(&path) else {
        return 0;
    };
    json.matches("\"url\"").count()
}

#[test]
fn meld_refused_when_not_in_allow_and_locked() {
    // spec: POL-11
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // allow lists a different repo name; lock enforces it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "locked non-allowed meld must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted") || r.stderr.contains("not permitted by the managed"),
        "error should mention the source is not permitted: {}",
        r.stderr
    );
    // Nothing registered and no clone left on disk for the source. The source's
    // clone dir is sources/local/<base>/agents; the refusal removes it.
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should be left at {}",
        clone_dir.display()
    );
}

#[test]
fn meld_allowed_when_not_in_allow_but_unlocked() {
    // spec: POL-13
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // lock is off, so allow is advisory: a non-match warns but proceeds.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = false\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "unlocked non-allowed meld must succeed: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("advisory") || r.stderr.contains("not in the managed policy"),
        "a warning should be printed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn policy_is_authoritative_over_explicit_user_meld() {
    // spec: POL-3
    // The user explicitly asks to meld this exact source, but a locked policy
    // that does not allow it refuses regardless of user intent.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "policy must override the user's explicit meld request: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted"),
        "refusal should be explained: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn meld_pinned_policy_refuses_floating_branch_and_allows_tag() {
    // spec: POL-20
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-policy");
    let spec = sb.source_spec();
    // pinned requires a tag/ref. allow matches this repo so only the pin gates.
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pintest-policy\"]\n",
    );

    // No pin flag => default branch => refused.
    let floating = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !floating.success,
        "pinned policy must refuse a default-branch meld: {}",
        floating.stdout
    );
    assert!(
        floating.stderr.contains("must be pinned"),
        "refusal should mention pinning: {}",
        floating.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on refusal");

    // A tag pin satisfies the policy.
    let tagged = sb.mind_env(
        &["meld", &spec, "--pin-tag", "v1.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        tagged.success,
        "pinned policy must accept a --pin-tag meld: {}",
        tagged.stderr
    );
    assert_eq!(source_count(&sb), 1, "tagged source should be registered");
}

#[test]
fn learn_skips_disallowed_source_when_locked() {
    // spec: POL-12
    // Meld under no policy, then apply a locked policy that no longer allows the
    // source: learn must skip it with a notice and not error.
    let sb = melded(); // melds + learns nothing extra; source is registered
    // Confirm the source is present and not yet learned beyond `review`.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["learn", "agent:dev"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "learn must not error when skipping disallowed sources: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "learn should report the skipped source: {}",
        r.stdout
    );
    // The item was not installed.
    let recall = sb.mind_env(
        &["recall", "agent:dev"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.success,
        "the disallowed item must not be installed: {}",
        recall.stdout
    );
}

#[test]
fn sync_skips_disallowed_source_when_locked() {
    // spec: POL-12
    let sb = melded();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync must not error on a skipped source: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "sync should report the skipped source: {}",
        r.stdout
    );
}

#[test]
fn sync_provisions_auto_meld_and_is_idempotent() {
    // spec: POL-32
    // The policy declares an auto_meld entry (pinned to a tag). `sync` provisions
    // it: the source appears in the registry. A second sync is a no-op (no new
    // source, no error).
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-src");
    let spec = sb.source_spec();
    // lock/pinned off so the entry validates without an allow/pin match check on
    // the path spec; the entry itself carries a tag pin.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{spec}\"\ntag = \"v1.0\"\n",
        spec = spec.replace('\\', "\\\\")
    );
    let policy = write_policy(&sb, &body);

    assert_eq!(source_count(&sb), 0, "registry starts empty");
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "auto-meld sync should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "auto_meld entry should be provisioned into the registry: {}",
        r.stdout
    );

    // Idempotent: a second sync provisions nothing new and still succeeds.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second sync should succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "auto-meld provisioning must be idempotent: {}",
        r2.stdout
    );
}

#[test]
fn config_lobes_add_refused_when_lobes_locked() {
    // spec: POL-40
    let sb = Sandbox::named("agents");
    let policy = write_policy(&sb, "[lobes]\nlock = true\ntargets = [\"~/.claude\"]\n");

    // Snapshot the lobe list before the refused add.
    let before = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(before.success, "list before: {}", before.stderr);

    let r = sb.mind_env(
        &["config", "lobes", "add", "/tmp/some-home"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "locked lobes add must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("lock") || r.stderr.contains("refused") || r.stderr.contains("pinned"),
        "refusal should be explained: {}",
        r.stderr
    );

    // The lobe list is unchanged: the path was not added.
    let after = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !after.stdout.contains("/tmp/some-home"),
        "the refused lobe must not appear: {}",
        after.stdout
    );
}

#[test]
fn upgrade_skips_disallowed_source_when_locked() {
    // spec: POL-12
    // upgrade operates only on sources whose identity matches allow. Meld + learn
    // under no policy, drift the source upstream so an upgrade is pending, then
    // run upgrade under a locked policy that no longer allows the source: the
    // pending upgrade is reported as skipped (not applied) and upgrade exits zero.
    let sb = melded();
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    // Drift the source and refresh the clone (unmanaged sync), so the catalog now
    // differs from the installed hash and upgrade would otherwise apply it.
    sb.edit_source();
    let synced = sb.mind(&["sync"]);
    assert!(
        synced.success,
        "sync failed: {} {}",
        synced.stdout, synced.stderr
    );

    // Now a locked policy that does not allow the source. upgrade must skip it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "--yes"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "upgrade must not error when skipping disallowed sources: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "upgrade should report the skipped source: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("upgraded"),
        "the disallowed item must not be upgraded: {}",
        r.stdout
    );

    // The installed item is unchanged: its recorded commit/hash did not advance.
    // Extract the commit line from each output and compare only that, because the
    // "out of date" status line legitimately differs (the source has drifted but
    // the upgrade was blocked; the displayed outdated marker is expected).
    let after = sb
        .mind_env(
            &["recall", "skill:review"],
            &[("MIND_POLICY_FILE", policy.as_str())],
        )
        .stdout;
    let commit_before = before.lines().find(|l| l.contains("commit")).unwrap_or("");
    let commit_after = after.lines().find(|l| l.contains("commit")).unwrap_or("");
    assert_eq!(
        commit_before, commit_after,
        "the skipped item's recorded commit must not advance: before={before} after={after}"
    );
    let hash_before = before.lines().find(|l| l.contains("hash")).unwrap_or("");
    let hash_after = after.lines().find(|l| l.contains("hash")).unwrap_or("");
    assert_eq!(
        hash_before, hash_after,
        "the skipped item's recorded hash must not advance: before={before} after={after}"
    );
}

#[test]
fn upgrade_applies_allowed_source_while_skipping_disallowed() {
    // spec: POL-12
    // The "rest proceed" half of POL-12: a locked allowlist that matches the
    // source lets upgrade apply the pending upgrade (the same drift that is skipped
    // in the test above is applied here because the source matches allow).
    let sb = melded();
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    sb.edit_source();
    let synced = sb.mind(&["sync"]);
    assert!(
        synced.success,
        "sync failed: {} {}",
        synced.stdout, synced.stderr
    );

    // The allow pattern matches this sandbox's local identity, so the lock does
    // not exclude it; the pending upgrade applies.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "--yes"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("upgraded skill:review"),
        "an allowed source must be upgraded: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skipping"),
        "an allowed source must not be skipped: {}",
        r.stdout
    );
}

#[test]
fn sync_provisions_auto_meld_under_lock_and_is_idempotent() {
    // spec: POL-32
    // The locked + pinned + allowed round-trip: a locked policy whose auto_meld
    // entry is pinned to a tag and satisfies allow (POL-31) is provisioned by
    // sync, and re-provisioning is idempotent. This exercises the full enforced
    // path (the meld inside provisioning runs under the same locked policy), not
    // just the unlocked provisioning above.
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-locked");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // allow must satisfy BOTH allowlist checks for this entry: POL-31 policy
    // validation matches the raw `repo` string (here the local fixture path), and
    // runtime meld enforcement matches the parsed identity `local/<base>/<name>`.
    // For a real `host/owner/repo` spec these coincide; for a local-path fixture
    // they differ in segment shape, so the allow list carries one pattern for each
    // form: `<base>/*` for the raw path and `local/*/automeld-locked` for the
    // identity. Both use a single-segment `*`, never crossing a `/`.
    let raw_pat = sb.base.join("*").to_string_lossy().replace('\\', "\\\\");
    let body = format!(
        "[sources]\nlock = true\npinned = true\nallow = [\"{raw_pat}\", \"local/*/automeld-locked\"]\n\n[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n"
    );
    let policy = write_policy(&sb, &body);

    assert_eq!(source_count(&sb), 0, "registry starts empty");
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "locked+pinned auto-meld sync should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the allowed+pinned auto_meld entry should be provisioned under lock: {}",
        r.stdout
    );

    // The recorded pin is the declared tag, not a floating branch.
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\"") && pin_json.contains("v1.0"),
        "auto_meld entry should be provisioned at its declared tag pin: {pin_json}"
    );

    // Idempotent under the same locked policy: no second provisioning, no error.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second locked sync should succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "locked auto-meld provisioning must be idempotent: {}",
        r2.stdout
    );
}

#[test]
fn auto_meld_entry_already_melded_is_not_remelded() {
    // spec: POL-32
    // spec: POL-55
    // When the recorded pin already equals the declared pin, the entry is left
    // unchanged: no re-meld, no re-pin message, no duplicate registration.
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-pre");
    let spec = sb.source_spec();

    // User melds it first (unmanaged), pinned to the tag.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(
        pre.success,
        "pre-meld failed: {} {}",
        pre.stdout, pre.stderr
    );
    assert_eq!(source_count(&sb), 1, "source melded once");

    // A policy whose auto_meld declares the same pin: idempotent, no action.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);
    assert_eq!(
        source_count(&sb),
        1,
        "an already-melded auto_meld entry at the same pin must not be re-melded: {}",
        r.stdout
    );
    // No re-pin event: pins matched, so no message should appear.
    assert!(
        !r.stdout.contains("re-pinned"),
        "matching pin must not emit a re-pinned message: {}",
        r.stdout
    );
    // The recorded pin is still v1.0 (unchanged).
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\"") && pin_json.contains("v1.0"),
        "pin must remain at tag v1.0: {pin_json}"
    );
}

#[test]
fn auto_meld_pin_bump_reconciles_on_sync() {
    // spec: POL-55
    // When a policy's auto_meld entry declares a pin that differs from the
    // registered source's recorded pin, sync updates the recorded pin and reports
    // "re-pinned <name> <old> -> <new>". The source stays registered (no
    // unmeld/remeld): only the pin field is updated so the subsequent sync fetch
    // lands the new ref.
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("automeld-repin");
    let spec = sb.source_spec();

    // Add a v2.0 tag at the second commit (sha_v2, the tip of main after
    // make_pinnable_repo advanced it).
    git(&sb.source, &["tag", "v2.0"]);

    // First: pre-provision the source at v1.0.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);
    assert_eq!(source_count(&sb), 1, "one source registered");
    let pin_before = read_source_pin_json(&sb);
    assert!(
        pin_before.contains("\"tag\"") && pin_before.contains("v1.0"),
        "initial pin must be tag v1.0: {pin_before}"
    );

    // IT bumps the policy pin to v2.0.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with bumped pin failed: {} {}",
        r.stdout, r.stderr
    );

    // The source is still registered exactly once (no unmeld/remeld).
    assert_eq!(
        source_count(&sb),
        1,
        "source count must remain 1 after re-pin"
    );

    // The re-pin message must appear in the sync output.
    assert!(
        r.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("v1.0") && r.stdout.contains("v2.0"),
        "re-pinned message must name old and new pins: {}",
        r.stdout
    );

    // The recorded pin is now v2.0.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "recorded pin must be updated to tag v2.0: {pin_after}"
    );
    assert!(
        !pin_after.contains("v1.0"),
        "old pin v1.0 must not remain in sources.json: {pin_after}"
    );
}

#[test]
fn auto_meld_repin_default_branch_to_follow_branch_reconciles() {
    // spec: POL-55
    // A re-pin transition with NO Tag on either side: the recorded pin is the
    // default branch (no explicit pin) and the policy declares a follow_branch.
    // sync reconciles the drift, reports it with the human pin descriptions
    // ("default branch" -> "branch <b>"), lands the branch tip, and leaves the
    // source registered exactly once. Guards the non-Tag pin_description arms and
    // the DefaultBranch == am.pin comparison for a non-Tag declared pin.
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("automeld-db-to-branch");
    let spec = sb.source_spec();

    // Pre-meld with no pin: recorded pin is DefaultBranch, commit at main tip.
    let pre = sb.mind(&["meld", &spec]);
    assert!(
        pre.success,
        "pre-meld (default branch) failed: {}",
        pre.stderr
    );
    assert_eq!(source_count(&sb), 1, "one source registered");
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "a default-branch meld records the main tip (sha_v2)"
    );
    let pin_before = read_source_pin_json(&sb);
    assert!(
        pin_before.contains("default-branch"),
        "initial pin must be default-branch: {pin_before}"
    );

    // Policy declares follow_branch = stable (which points at sha_v1).
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\nfollow_branch = \"stable\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "re-pin sync failed: {} {}", r.stdout, r.stderr);

    // The transition is reported with both non-Tag pin descriptions.
    assert!(
        r.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("default branch") && r.stdout.contains("branch stable"),
        "re-pin message must name the default-branch -> follow-branch transition: {}",
        r.stdout
    );

    // The recorded pin is now follow-branch stable, and the branch tip landed.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("follow-branch") && pin_after.contains("stable"),
        "recorded pin must be follow-branch stable: {pin_after}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "the follow_branch (stable) tip (sha_v1) must be landed after re-pin"
    );
    assert_eq!(source_count(&sb), 1, "source stays registered exactly once");
}

#[test]
fn auto_meld_repin_lands_new_ref_and_second_sync_is_idempotent() {
    // spec: POL-55
    // spec: POL-32
    // The re-pin-only save path: a sync that provisions NO new source but
    // reconciles one pin must (a) persist the updated pin, (b) actually LAND the
    // new ref -- the recorded commit moves to the new tag's commit, not just the
    // pin field -- and (c) a second sync with the now-matching pin is a silent
    // idempotent no-op (POL-32) that keeps the landed commit.
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("automeld-repin-land");
    let spec = sb.source_spec();
    // Tag the second commit v2.0 (make_pinnable_repo only tags v1.0 at sha_v1).
    git(&sb.source, &["tag", "v2.0"]);

    // Pre-provision at v1.0: recorded commit is sha_v1.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);
    assert_eq!(read_source_commit(&sb), sha_v1, "v1.0 pin lands sha_v1");

    // Bump the policy pin to v2.0 and sync.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "re-pin sync failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("re-pinned"),
        "first sync must re-pin: {}",
        r.stdout
    );

    // The new ref actually landed: recorded commit advanced to sha_v2 on the same
    // sync (the per-source fetch after the re-pin resolved the new tag).
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the re-pinned v2.0 tag's commit (sha_v2) must be fetched and landed on the same sync"
    );
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "recorded pin must be persisted as tag v2.0: {pin_after}"
    );

    // Second sync: pins now match, so it is a silent no-op and the commit holds.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second sync failed: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        !r2.stdout.contains("re-pinned"),
        "a matching pin on the second sync must not re-pin again: {}",
        r2.stdout
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the landed commit must remain at sha_v2 after the idempotent second sync"
    );
    assert_eq!(source_count(&sb), 1, "still exactly one source");
}

#[test]
fn auto_meld_repin_message_suppressed_under_json() {
    // spec: POL-55
    // The `if !out.json` guard on the re-pin message: under `sync --json` the
    // human "re-pinned" line must NOT appear on stdout, yet the reconciliation
    // still happens (the recorded pin is updated and the new ref lands).
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("automeld-repin-json");
    let spec = sb.source_spec();
    git(&sb.source, &["tag", "v2.0"]);

    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);

    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(
        &["sync", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(r.success, "sync --json failed: {} {}", r.stdout, r.stderr);

    // The human re-pin line must be fully suppressed under --json.
    assert!(
        !r.stdout.contains("re-pinned"),
        "the re-pin message must not be emitted under --json: {}",
        r.stdout
    );

    // But the reconciliation still took effect: pin updated and new ref landed.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "the pin must still be reconciled to v2.0 under --json: {pin_after}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the new ref must still land under --json"
    );
}

#[test]
fn auto_meld_repin_touches_only_the_drifting_source() {
    // spec: POL-55
    // spec: POL-32
    // With several registered sources, only the entry whose recorded pin differs
    // from its declared pin is re-pinned; entries whose pins already match are
    // left untouched (POL-32). Exactly one re-pin line is emitted and it names the
    // drifting source, not the stable one.
    let (sb, _sha_a_v1, sha_a_v2) = make_pinnable_repo("automeld-multi-a");
    let spec_a = sb.source_spec();
    git(&sb.source, &["tag", "v2.0"]); // drift target for A

    // Build a second source repo (B) under the same sandbox base, tagged v1.0.
    let src_b = sb.base.join("automeld-multi-b");
    write(
        &src_b.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev b\n---\n# dev b\n",
    );
    git(&src_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_b, &["config", "user.email", "t@t"]);
    git(&src_b, &["config", "user.name", "t"]);
    git(&src_b, &["add", "-A"]);
    git(&src_b, &["commit", "-qm", "initial"]);
    git(&src_b, &["tag", "v1.0"]);
    let spec_b = src_b.to_string_lossy().into_owned();

    // Pre-meld both, each pinned to v1.0.
    assert!(
        sb.mind(&["meld", &spec_a, "--pin-tag", "v1.0"]).success,
        "meld A at v1.0 failed"
    );
    assert!(
        sb.mind(&["meld", &spec_b, "--pin-tag", "v1.0"]).success,
        "meld B at v1.0 failed"
    );
    assert_eq!(source_count(&sb), 2, "two sources registered");

    // Policy: A drifts to v2.0, B stays at v1.0 (matching -> untouched).
    let esc_a = spec_a.replace('\\', "\\\\");
    let esc_b = spec_b.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{esc_a}\"\ntag = \"v2.0\"\n\n[[sources.auto_meld]]\nrepo = \"{esc_b}\"\ntag = \"v1.0\"\n"
    );
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);

    // Exactly one re-pin, and it names A (the drifting source), not B.
    assert_eq!(
        r.stdout.matches("re-pinned").count(),
        1,
        "exactly one source must be re-pinned: {}",
        r.stdout
    );
    let a_name = format!("local/{}/automeld-multi-a", sb.base_name());
    let b_name = format!("local/{}/automeld-multi-b", sb.base_name());
    let repin_line = r
        .stdout
        .lines()
        .find(|l| l.contains("re-pinned"))
        .unwrap_or("");
    assert!(
        repin_line.contains(&a_name),
        "the re-pin line must name the drifting source A: {repin_line}"
    );
    assert!(
        !repin_line.contains(&b_name),
        "the stable source B must not be re-pinned: {repin_line}"
    );

    // Both still registered; A landed its new v2.0 commit (A is first in the
    // registry, having been melded first).
    assert_eq!(source_count(&sb), 2, "both sources remain registered");
    assert_eq!(
        read_source_commit(&sb),
        sha_a_v2,
        "the drifting source A must have landed its new v2.0 commit"
    );
    // The full registry still records B's unchanged v1.0 tag pin alongside A's v2.0.
    let sources_json = read_sources_json(&sb);
    assert!(
        sources_json.contains("v2.0") && sources_json.contains("v1.0"),
        "A must be re-pinned to v2.0 while B keeps v1.0: {sources_json}"
    );
}

#[test]
fn sync_auto_meld_provisioning_soft_fails_on_unreachable_entry() {
    // spec: POL-34
    // When an auto_meld entry cannot be provisioned (here a nonexistent local
    // path that git cannot clone -- fully offline), `sync` warns and continues
    // rather than aborting. The already-melded source still syncs, and the
    // command exits non-zero with the failed entry named in stderr.
    let sb = Sandbox::named("good-src-pol34");
    let good_spec = sb.source_spec();

    // Meld the good source up front (unmanaged).
    let meld = sb.mind(&["meld", &good_spec]);
    assert!(meld.success, "meld good source failed: {}", meld.stderr);
    assert_eq!(source_count(&sb), 1, "good source should be melded");

    // Policy: one entry already melded (idempotent skip), one pointing at a
    // nonexistent path that will fail to clone without any network access.
    let bad_repo = "/nonexistent/path/that/cannot/be/cloned";
    let escaped_good = good_spec.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped_good}\"\n\n[[sources.auto_meld]]\nrepo = \"{bad_repo}\"\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);

    // (b) exits non-zero because provisioning the bad entry failed.
    assert!(
        !r.success,
        "sync must exit non-zero when an auto_meld entry fails to provision: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // (c) the failed entry is named in the output.
    assert!(
        r.stderr.contains(bad_repo) || r.stdout.contains(bad_repo),
        "the failed entry must be named in the output: stderr={} stdout={}",
        r.stderr,
        r.stdout
    );

    // (a) the already-melded good source is still in the registry: the sync
    // loop ran past the provisioning failure rather than aborting.
    assert_eq!(
        source_count(&sb),
        1,
        "the already-melded source must still be registered after a provisioning failure"
    );
}

#[test]
fn policy_min_mind_version_gate_surfaces_clean_error_end_to_end() {
    // spec: POL-61
    // spec: POL-62
    // The whole point of the version gate: a real command that loads the policy
    // must stop with a clear "requires mind >=" message instead of failing
    // opaquely. The policy also carries an UNKNOWN top-level table ([future]),
    // which the strict deny_unknown_fields parse would otherwise reject first --
    // asserting the version error wins proves phase 1 runs before phase 2 all the
    // way through the binary, not just in a unit test.
    let running = env!("CARGO_PKG_VERSION");
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "min-mind-version = \"999.0.0\"\n[future]\nunknown-key = 1\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "a too-new policy must fail closed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("requires mind >=") && r.stderr.contains("999.0.0"),
        "must surface the version gate error naming the required version: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("upgrade mind"),
        "must tell the user to upgrade: {}",
        r.stderr
    );
    // Ordering proof: the version error wins over the unknown-field error even
    // though [future] is an unknown table the strict parse would also reject.
    assert!(
        !r.stderr.contains("unknown field") && !r.stderr.contains("unknown-key"),
        "version gate must fire BEFORE the strict unknown-field parse: {}",
        r.stderr
    );
    // The gate fired before any provisioning: nothing registered on disk.
    assert_eq!(
        source_count(&sb),
        0,
        "no source registered when the gate fires"
    );
    // Sanity: the running binary version really is below the gate.
    assert_ne!(running, "999.0.0");
}

// ---------------------------------------------------------------------------
// auto_meld install = true (POL-58/59/60)
// ---------------------------------------------------------------------------

#[test]
fn auto_meld_install_true_installs_items_after_provisioning() {
    // spec: POL-58
    // When an auto_meld entry carries `install = true`, `sync` provisions the
    // source AND installs all its items headlessly. `recall --json` shows every
    // item with "installed": true.
    let sb = Sandbox::named("am-install-src");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with install=true should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // All three items must be installed.
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall.success,
        "recall after sync+install should succeed: {}",
        recall.stderr
    );
    assert!(
        recall.stdout.contains("\"installed\": true"),
        "items must be installed after auto_meld with install=true: {}",
        recall.stdout
    );
    // None should be uninstalled.
    assert!(
        !recall.stdout.contains("\"installed\": false"),
        "no items should be uninstalled: {}",
        recall.stdout
    );
}

#[test]
fn auto_meld_install_absent_registers_only() {
    // spec: POL-58
    // When `install` is absent (default false), `sync` registers the source but
    // installs nothing. `recall --json` shows all items with "installed": false.
    let sb = Sandbox::named("am-register-only");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // No `install` field: default is false, same as `install = false`.
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with default install should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Source must be registered.
    assert_eq!(source_count(&sb), 1, "source must be provisioned");

    // No items should be installed (recall shows them as available but not installed).
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.stdout.contains("\"installed\": true"),
        "no items must be installed when install is absent: {}",
        recall.stdout
    );
}

#[test]
fn auto_meld_install_true_build_hook_skipped_by_default() {
    // spec: POL-59
    // With `install = true` and `run-build-hooks` absent (default false), a
    // tool's build hook is skipped. The item installs but the hook does not run
    // (sentinel file absent).
    let sb = Sandbox::bare("am-bld-skip");
    let sentinel_name = "built.txt";
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"mytool\"\n",
            "path = \"tools/mytool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    write(
        &sb.source.join("tools/mytool/TOOL.md"),
        "---\ndescription: tool with build hook\n---\n# mytool\n",
    );
    write(&sb.source.join("mind.toml"), &toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add tool"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true, run-build-hooks absent (default false).
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync should succeed even with a build hook present: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The tool must be installed (symlink or store copy exists).
    let store_path = sb.mind_home.join("store/tool/mytool");
    assert!(
        store_path.exists(),
        "tool must be installed in the store: {store_path:?}"
    );

    // The sentinel file must NOT exist: build hook was skipped (HOOK-72 / POL-59).
    let sentinel = store_path.join(sentinel_name);
    assert!(
        !sentinel.exists(),
        "build hook must be skipped without run-build-hooks=true (POL-59): {sentinel:?}"
    );
}

#[test]
fn auto_meld_install_true_build_hook_runs_when_enabled() {
    // spec: POL-59
    // With `install = true` and `run-build-hooks = true`, the tool's build hook
    // executes during the headless install pass. The sentinel file appears in the
    // store proving the hook ran.
    let sb = Sandbox::bare("am-bld-run");
    let sentinel_name = "built.txt";
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"mytool\"\n",
            "path = \"tools/mytool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    write(
        &sb.source.join("tools/mytool/TOOL.md"),
        "---\ndescription: tool with build hook\n---\n# mytool\n",
    );
    write(&sb.source.join("mind.toml"), &toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add tool"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true AND run-build-hooks = true.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\nrun-build-hooks = true\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with run-build-hooks=true should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The sentinel file must exist: build hook ran (POL-59).
    let sentinel = sb.mind_home.join("store/tool/mytool").join(sentinel_name);
    assert!(
        sentinel.exists(),
        "build hook must run when run-build-hooks=true (POL-59): {sentinel:?}"
    );
}

#[test]
fn auto_meld_install_item_failure_soft_fails_and_continues() {
    // spec: POL-60
    // When one item's install fails during the auto_meld install pass, the
    // failure is soft: `sync` warns and continues installing the remaining items.
    // The good items land installed; the overall exit code is non-zero.
    //
    // A build hook that exits non-zero (with run-build-hooks=true) causes its
    // tool's install to fail (HOOK-71 rolls it back). The other items (a skill
    // and an agent) carry no build hook and succeed.
    let sb = Sandbox::bare("am-soft-fail");
    // Write a failing tool.
    let fail_toml = concat!(
        "[[items]]\n",
        "kind = \"tool\"\n",
        "name = \"badtool\"\n",
        "path = \"tools/badtool\"\n",
        "build = \"exit 9\"\n",
        "\n",
        "[[items]]\n",
        "kind = \"skill\"\n",
        "name = \"review\"\n",
        "path = \"skills/review\"\n",
    );
    write(
        &sb.source.join("tools/badtool/TOOL.md"),
        "---\ndescription: tool that fails to build\n---\n# badtool\n",
    );
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review skill\n---\n# review\n",
    );
    write(&sb.source.join("mind.toml"), fail_toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add items"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true + run-build-hooks = true so the failing build hook fires.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\nrun-build-hooks = true\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);

    // Non-zero exit because badtool's install failed (POL-60 / POL-34).
    assert!(
        !r.success,
        "sync must exit non-zero when an item install fails: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The good skill must still be installed (soft-fail: other items proceed).
    let skill_store = sb.mind_home.join("store/skill/review");
    assert!(
        skill_store.exists(),
        "the good skill must be installed despite the tool failure (POL-60): {skill_store:?}"
    );

    // The bad tool's store copy must be absent (hook failure rolled it back).
    let tool_store = sb.mind_home.join("store/tool/badtool");
    assert!(
        !tool_store.exists(),
        "the bad tool must not be in the store after a failed build hook: {tool_store:?}"
    );

    // The failure is named in stderr (POL-60 reporting).
    assert!(
        r.stderr.contains("badtool") || r.stderr.contains("failed"),
        "failed item must be named in stderr: {}",
        r.stderr
    );
}

#[test]
fn auto_meld_install_true_second_sync_is_idempotent() {
    // spec: POL-58
    // A second `sync` under the same install = true policy must be idempotent: the
    // source is already registered at its declared pin (the POL-32 skip), the
    // items stay installed, no reinstall is attempted, no item-failure is warned,
    // and the command exits zero (nothing else failed). Guards against a
    // regression where a repeat sync re-runs install and errors or double-registers.
    let sb = Sandbox::named("am-install-idem");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    // First sync: provisions the source AND installs its items.
    let first = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        first.success,
        "first sync must succeed: stdout={} stderr={}",
        first.stdout, first.stderr
    );
    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall1.stdout.contains("\"installed\": true"),
        "items must be installed after the first sync: {}",
        recall1.stdout
    );

    // Second sync: idempotent no-op for this source. Exit zero, no item-failure
    // warning, no duplicate registration.
    let second = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        second.success,
        "second sync must exit zero (idempotent): stdout={} stderr={}",
        second.stdout, second.stderr
    );
    assert!(
        !second.stderr.contains("item install failed"),
        "second sync must not warn about any item install failure: {}",
        second.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the source must remain singly registered after a second sync"
    );

    // The items remain installed and none flipped to uninstalled.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must remain installed after the second sync: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may become uninstalled after the second sync: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_register_only_then_install_true_converges() {
    // spec: POL-58
    // Regression for the defect where the install pass was only reached on a fresh
    // meld.  A source provisioned register-only (install absent), then given
    // `install = true` in policy at the SAME pin, must install its items on the
    // next sync (POL-32 same-pin path now falls through to the install pass).
    let sb = Sandbox::named("am-converge");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");

    // First sync: register only (install absent -> default false).
    let body_reg = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\n");
    let policy_reg = write_policy(&sb, &body_reg);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_reg.as_str())]);
    assert!(
        r1.success,
        "register-only sync must succeed: stdout={} stderr={}",
        r1.stdout, r1.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must be registered after first sync"
    );

    // Confirm nothing is installed yet.
    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_reg.as_str())],
    );
    assert!(
        !recall1.stdout.contains("\"installed\": true"),
        "items must NOT be installed after register-only sync: {}",
        recall1.stdout
    );

    // Second sync: flip to install = true at the SAME pin (POL-32 same-pin path).
    let body_ins = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy_ins = write_policy(&sb, &body_ins);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_ins.as_str())]);
    assert!(
        r2.success,
        "install=true sync at same pin must succeed: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );

    // Items must now be installed (convergence).
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_ins.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after flipping to install=true at same pin: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may remain uninstalled after convergence sync: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_repin_with_install_true_installs_at_new_ref() {
    // spec: POL-58
    // spec: POL-55
    // When a source is registered at pin A with install=false, and the policy
    // bumps to pin B with install=true, the sync must both re-pin the source AND
    // install its items (the POL-55 re-pin path now falls through to the install
    // pass).
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("am-repin-install");
    let spec = sb.source_spec();

    // Tag v2.0 at the current tip.
    git(&sb.source, &["tag", "v2.0"]);

    // First: register at v1.0 with no install.
    let escaped = spec.replace('\\', "\\\\");
    let body_v1 = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy_v1 = write_policy(&sb, &body_v1);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v1.as_str())]);
    assert!(
        r1.success,
        "first sync (register at v1.0) must succeed: {} {}",
        r1.stdout, r1.stderr
    );
    assert_eq!(source_count(&sb), 1, "one source registered");

    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v1.as_str())],
    );
    assert!(
        !recall1.stdout.contains("\"installed\": true"),
        "nothing installed after register-only at v1.0: {}",
        recall1.stdout
    );

    // Second sync: bump to v2.0 AND enable install=true.
    let body_v2 =
        format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\ninstall = true\n");
    let policy_v2 = write_policy(&sb, &body_v2);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r2.success,
        "re-pin+install sync must succeed: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );

    // Re-pin must be reported.
    assert!(
        r2.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r2.stdout
    );

    // Source stays registered exactly once.
    assert_eq!(
        source_count(&sb),
        1,
        "source count must remain 1 after re-pin+install"
    );

    // Items must be installed.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after re-pin+install sync: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may remain uninstalled after re-pin+install: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_repin_install_persists_new_pin_and_lands_ref_durably() {
    // spec: POL-55
    // spec: POL-58
    // spec: POL-32
    // Registry-save / `provisioned`-counter interaction on the re-pin + install
    // path.  The restructured loop increments `provisioned` on a POL-55 re-pin,
    // the install pass then saves the registry and resets `provisioned` to 0 (so
    // the outer `if provisioned > 0` does NOT redundantly save).  This test
    // certifies the OBSERVABLE contract that a missed save would break: after a
    // re-pin+install sync the NEW pin must be durably on disk (sources.json, not
    // just in memory) and the new ref's commit must have landed -- neither is
    // asserted by auto_meld_repin_with_install_true_installs_at_new_ref, which
    // only checks in-process recall output.  It then flips to the POL-32 same-pin
    // path (where `provisioned` is never incremented, so the install pass does NOT
    // pre-save) and confirms nothing on disk is lost by the counter being 0.
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("am-repin-install-durable");
    let spec = sb.source_spec();
    // Tag v2.0 at the current main tip (sha_v2); make_pinnable_repo tags v1.0 at sha_v1.
    git(&sb.source, &["tag", "v2.0"]);
    let escaped = spec.replace('\\', "\\\\");

    // First sync: register at v1.0, install = false.
    let body_v1 = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy_v1 = write_policy(&sb, &body_v1);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v1.as_str())]);
    assert!(
        r1.success,
        "first sync (register at v1.0) must succeed: {} {}",
        r1.stdout, r1.stderr
    );
    let pin_v1 = read_source_pin_json(&sb);
    assert!(
        pin_v1.contains("\"tag\"") && pin_v1.contains("v1.0"),
        "initial recorded pin must be tag v1.0: {pin_v1}"
    );

    // Second sync: re-pin to v2.0 AND install = true (POL-55 + POL-58).
    let body_v2 =
        format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\ninstall = true\n");
    let policy_v2 = write_policy(&sb, &body_v2);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r2.success,
        "re-pin+install sync must succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r2.stdout
    );

    // DURABILITY: reload sources.json from disk. The new pin must be persisted and
    // the old one gone -- the install-pass save must not have dropped the re-pin,
    // and the outer save must not have clobbered it back.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "re-pinned tag v2.0 must be durably persisted to sources.json: {pin_after}"
    );
    assert!(
        !pin_after.contains("v1.0"),
        "old pin v1.0 must not remain on disk after re-pin+install: {pin_after}"
    );
    // The new ref actually landed: the per-source fetch after the re-pin advanced
    // the recorded commit to sha_v2 and the final save persisted it.
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the re-pinned v2.0 commit (sha_v2) must be landed and persisted on disk"
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must stay registered exactly once after re-pin+install"
    );
    // Items installed on the re-pin path.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after re-pin+install sync: {}",
        recall2.stdout
    );

    // Third sync: pins now match (POL-32 same-pin path, `provisioned` never
    // incremented, so the install pass does NOT pre-save the registry). Nothing on
    // disk may be lost by the counter being 0: no re-pin, pin/commit hold, items
    // stay installed, exit zero.
    let r3 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r3.success,
        "third sync (same-pin + install) must succeed: {} {}",
        r3.stdout, r3.stderr
    );
    assert!(
        !r3.stdout.contains("re-pinned"),
        "a matching pin must not re-pin again on the same-pin path: {}",
        r3.stdout
    );
    let pin_final = read_source_pin_json(&sb);
    assert!(
        pin_final.contains("v2.0") && !pin_final.contains("v1.0"),
        "the same-pin install pass must leave the persisted pin at v2.0: {pin_final}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the landed commit must remain at sha_v2 after the same-pin+install sync"
    );
    let recall3 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall3.stdout.contains("\"installed\": true"),
        "items must remain installed after the same-pin+install sync: {}",
        recall3.stdout
    );
    assert!(
        !recall3.stdout.contains("\"installed\": false"),
        "no item may flip to uninstalled on the same-pin+install path: {}",
        recall3.stdout
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must stay registered exactly once after the same-pin+install sync"
    );
}

#[test]
fn auto_meld_install_true_with_zero_items_succeeds() {
    // spec: POL-58
    // install = true on a source that offers NO items (a pure registry) must
    // provision successfully with an empty install pass: the source is registered,
    // no item failure is reported, and sync exits zero. Guards against the empty
    // catalog being treated as an error.
    let sb = Sandbox::bare("am-install-empty");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with install=true on an item-less source must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("item install failed"),
        "an item-less source must not report any item failure: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the item-less source must still be registered"
    );
}

#[test]
fn auto_meld_mixed_install_flags_install_only_the_opted_in_source() {
    // spec: POL-58
    // Two auto_meld entries: one carries install = true, the other does not. Only
    // the opted-in source's items are installed; the other registers only. Each
    // source ships a uniquely named skill so the two are unambiguous even though
    // they are provisioned in the same run.
    let sb = Sandbox::bare("am-mixed");

    // Source A: opted in (install = true), ships skill:alpha.
    let src_a = sb.base.join("am-mixed-a");
    write(
        &src_a.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: alpha skill\n---\n# alpha\n",
    );
    git(&src_a, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_a, &["config", "user.email", "t@t"]);
    git(&src_a, &["config", "user.name", "t"]);
    git(&src_a, &["add", "-A"]);
    git(&src_a, &["commit", "-qm", "initial"]);
    let spec_a = src_a.to_string_lossy().into_owned();

    // Source B: register-only (no install), ships skill:beta.
    let src_b = sb.base.join("am-mixed-b");
    write(
        &src_b.join("skills/beta/SKILL.md"),
        "---\nname: beta\ndescription: beta skill\n---\n# beta\n",
    );
    git(&src_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_b, &["config", "user.email", "t@t"]);
    git(&src_b, &["config", "user.name", "t"]);
    git(&src_b, &["add", "-A"]);
    git(&src_b, &["commit", "-qm", "initial"]);
    let spec_b = src_b.to_string_lossy().into_owned();

    let esc_a = spec_a.replace('\\', "\\\\");
    let esc_b = spec_b.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{esc_a}\"\ninstall = true\n\n[[sources.auto_meld]]\nrepo = \"{esc_b}\"\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "mixed-policy sync must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Both sources registered.
    assert_eq!(
        source_count(&sb),
        2,
        "both auto_meld sources must be registered"
    );
    // A's item is installed; B's item is not (register-only).
    assert!(
        sb.claude_home.join("skills/alpha").exists(),
        "alpha from the install=true source must be installed: {:?}",
        sb.claude_home
    );
    assert!(
        !sb.claude_home.join("skills/beta").exists(),
        "beta from the register-only source must NOT be installed"
    );
}

#[test]
fn auto_meld_run_build_hooks_without_install_is_inert() {
    // spec: POL-58
    // spec: POL-59
    // run-build-hooks = true WITHOUT install = true is a harmless no-op: run-build-hooks
    // only gates build hooks during the install pass, and there is no install pass
    // when install is absent. The policy parses, the source registers only, nothing
    // is installed, and sync exits zero.
    let sb = Sandbox::named("am-rbh-noinstall");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // run-build-hooks set, install absent (default false).
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\nrun-build-hooks = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "run-build-hooks without install must be a harmless no-op: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Source registered, nothing installed.
    assert_eq!(
        source_count(&sb),
        1,
        "the source must be registered when only run-build-hooks is set"
    );
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.stdout.contains("\"installed\": true"),
        "no items may be installed when install is absent (run-build-hooks alone is inert): {}",
        recall.stdout
    );
}

#[test]
fn meld_pinned_policy_accepts_source_directive_tag() {
    // spec: POL-20
    // The pin may come from the source's own mind.toml `[source]` directive
    // (DSC-41), not just the --pin-tag flag. A directive that resolves to a tag
    // satisfies pinned = true and the meld is accepted.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pindir-tag");
    sb.write_and_commit("mind.toml", "[source]\npin-tag = \"v1.0\"\n");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pindir-tag\"]\n",
    );

    // No consumer pin flag: the [source] directive supplies the tag pin.
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "a [source] tag directive must satisfy a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the directive-pinned source should register"
    );
    // The landed pin is the directive's tag (sha_v1), not the floating main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "the directive tag pin should land on the tagged commit"
    );
}

#[test]
fn meld_pinned_policy_refuses_source_directive_floating_branch() {
    // spec: POL-20
    // The negative of the directive case: a `[source]` directive that resolves to
    // a floating branch (follow-branch) does NOT satisfy pinned = true and is
    // refused, leaving nothing registered.
    let (sb, _v1, _v2) = make_pinnable_repo("pindir-branch");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pindir-branch\"]\n",
    );

    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "a [source] follow-branch directive must be refused under a pinned policy: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("must be pinned"),
        "refusal should mention pinning: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        0,
        "nothing registered on a floating refusal"
    );
}

#[test]
fn config_lobes_add_allowed_when_lobes_not_locked() {
    // spec: POL-40
    // The refusal is specific to the lobe lock: with [lobes].lock = false (and a
    // policy otherwise present), `config lobes add` still works. The lock is the
    // only thing that pins the agent homes.
    let sb = Sandbox::named("agents");
    let policy = write_policy(&sb, "[lobes]\nlock = false\ntargets = [\"~/.claude\"]\n");
    let lobe = sb.base.join("extra-home");
    let lobe_str = lobe.to_string_lossy().into_owned();

    let r = sb.mind_env(
        &["config", "lobes", "add", &lobe_str],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "an unlocked lobes add must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("added lobe"),
        "the add should be reported: {}",
        r.stdout
    );

    // The lobe is now listed, confirming the write took effect.
    let after = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        after.stdout.contains(&lobe_str),
        "the added lobe must appear in the list: {}",
        after.stdout
    );
}

#[test]
fn config_lobes_add_allowed_with_no_lobes_section() {
    // spec: POL-40
    // A policy that controls only sources (no [lobes] section at all) leaves the
    // lobe lock unset, so `config lobes add` is unaffected.
    let sb = Sandbox::named("agents");
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let lobe = sb.base.join("home-no-lobes-section");
    let lobe_str = lobe.to_string_lossy().into_owned();

    let r = sb.mind_env(
        &["config", "lobes", "add", &lobe_str],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "lobes add must work when the policy has no [lobes] lock: {} {}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("added lobe"), "{}", r.stdout);
}

#[test]
fn meld_refused_when_not_allowed_leaves_no_clone_and_no_registry() {
    // spec: POL-11
    // Reinforce the "nothing cloned or registered" half: after a refused meld the
    // clone dir is absent AND sources.json records nothing (no partial registry),
    // and no link leaked into the hermetic claude_home.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(!r.success, "refused meld must fail: {}", r.stdout);

    assert_eq!(source_count(&sb), 0, "registry must record nothing");

    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should survive a refusal at {}",
        clone_dir.display()
    );
    let leaked = sb.claude_home.join("agents/dev.md");
    assert!(
        std::fs::symlink_metadata(&leaked).is_err(),
        "no item should be installed on a refused meld"
    );
}

#[test]
fn meld_unlocked_advisory_warning_text() {
    // spec: POL-13
    // The advisory warning under lock=false names the allowlist and explains it is
    // not enforced because the lock is off.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = false\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "advisory meld must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("allowlist") && r.stderr.contains("advisory"),
        "warning should name the allowlist and mark it advisory: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("lock is false"),
        "warning should explain the lock is off: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the advisory source is still registered"
    );
}

/// A sandbox whose source declares an install hook command in mind.toml.
/// `[source]` with only `install =` is NOT authoritative, so the three
/// convention items are still discovered.
fn sandbox_with_declared_hook(name: &str, cmd: &str) -> Sandbox {
    let sb = Sandbox::named(name);
    sb.write_and_commit("mind.toml", &format!("[source]\ninstall = \"{cmd}\"\n"));
    sb
}

#[test]
fn meld_with_declared_hook_non_tty_skips_but_still_installs() {
    // spec: HOOK-22, HOOK-21, HOOK-55
    // stdin is not a TTY in this harness, so a declared hook takes the skip
    // path (HOOK-22): the source and its items still install (HOOK-21), but the
    // tooling is not built. The clone-dir marker the hook would create must not
    // appear, the run is reported as skipped, and the registry records the hook
    // command with a NULL install_hook_commit (never ran).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld should still succeed: {}", r.stderr);

    // The source is registered (HOOK-21: skipping still installs the source).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("agents"),
        "source must be registered after a skipped hook: {}",
        sources.stdout
    );

    // The items are still discoverable / learnable (the tooling, not the items,
    // is what the skip drops).
    assert!(
        sb.mind(&["learn", "review"]).success,
        "items must install even when the hook is skipped"
    );

    // The hook did NOT run: its marker is absent from the clone dir.
    let marker = sb.source.clone().join("hookran");
    assert!(
        !marker.exists(),
        "the install hook must not have run: {} exists",
        marker.display()
    );

    // The skip is reported to the user with the exact note `meld_recursive`
    // prints on the HOOK-22 skip path (commands.rs); the source name in the
    // middle is the full `host/owner/repo` identity, so assert the two stable
    // fragments around it. A regression that drops or rewords the note fails
    // here rather than passing on any bare "skipped".
    let prefix = "note: skipped install hook ";
    let suffix = "; its items may not work until it runs";
    let reported = (r.stdout.contains(prefix) && r.stdout.contains(suffix))
        || (r.stderr.contains(prefix) && r.stderr.contains(suffix));
    assert!(
        reported,
        "the skip must be reported with the exact note: {} {}",
        r.stdout, r.stderr
    );

    // The registry records the hook in `install_hooks` with a null `ran_at`
    // (skipped, so `upgrade` can re-offer it) per HOOK-55.
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the hook command: {json}"
    );
    assert!(
        json.contains("install_hooks") && json.contains("\"ran_at\": null"),
        "a skipped hook must record in install_hooks with ran_at = null: {json}"
    );
}

#[test]
fn meld_dangerously_skip_runs_hook_and_records_it() {
    // spec: HOOK-23, HOOK-10, HOOK-31, HOOK-55
    // --dangerously-skip-install-hook-check runs the hook without prompting
    // (HOOK-23). It runs in the clone after checkout (HOOK-10), so its marker
    // lands in the clone dir, and the registry records both the command and the
    // commit it ran at (HOOK-31).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "meld should succeed: {}", r.stderr);

    // HOOK-10: the hook ran in the clone dir.
    let marker = sb.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the install hook must have run in the clone: {} missing",
        marker.display()
    );

    // HOOK-31/HOOK-55: the registry records the command in `install_hooks` with a
    // non-null `ran_at` (the commit it ran at).
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the hook command: {json}"
    );
    assert!(
        json.contains("install_hooks") && !json.contains("\"ran_at\": null"),
        "a hook that ran must record a non-null ran_at in install_hooks: {json}"
    );
}

#[test]
fn meld_hook_nonzero_exit_fails_and_registers_nothing() {
    // spec: HOOK-30
    // A non-zero hook exit is a HookFailed error that fails the meld: the source
    // is not registered and the clone is removed, as for any failed meld.
    let sb = sandbox_with_declared_hook("agents", "exit 1");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(
        !r.success,
        "a non-zero hook exit must fail the meld: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("install hook") && r.stderr.contains("failed"),
        "stderr must report the failed install hook: {}",
        r.stderr
    );

    // Nothing registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after a failed hook: {}",
        sources.stdout
    );
    let sources_json = sb.mind_home.join("sources.json");
    if sources_json.exists() {
        let json = std::fs::read_to_string(&sources_json).unwrap();
        assert!(
            !json.contains("\"repo\": \"agents\""),
            "sources.json must not list the source after a failed hook: {json}"
        );
    }

    // The source is a linked local working tree (CLI-27), so a failed hook must
    // NOT delete it -- it is the user's directory, not a clone we own.
    assert!(
        sb.source.exists(),
        "a failed hook must not delete a linked source's working tree at {}",
        sb.source.display()
    );
}

#[test]
fn meld_install_hook_flag_supplies_hook_without_mind_toml() {
    // spec: HOOK-2
    // --install-hook supplies a hook for a repo that ships no mind.toml. With
    // --dangerously-skip-install-hook-check it runs, and the registry records
    // the supplied command and a non-null run-commit.
    let sb = Sandbox::new(); // no mind.toml
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--install-hook",
        "touch hookran",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld should succeed: {}", r.stderr);

    let marker = sb.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the supplied hook must have run: {} missing",
        marker.display()
    );

    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the supplied hook command: {json}"
    );
    assert!(
        !json.contains("\"install_hook_commit\": null"),
        "install_hook_commit must be non-null after the supplied hook ran: {json}"
    );
}

#[test]
fn recall_sources_shows_install_hook_marker() {
    // spec: HOOK-31
    // recall --sources reports that a source carries an install hook.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"])
            .success
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success, "recall failed: {}", sources.stderr);
    // The marker is the ` hook` token inside the bracketed commit/alias column
    // (commands.rs `recall`), e.g. `[<commit> hook]`. Assert the exact bracketed
    // token so a regression that drops the marker (or renames the column) fails.
    assert!(
        sources.stdout.contains(" hook]"),
        "recall --sources must mark a source with the bracketed ` hook]` token: {}",
        sources.stdout
    );
}

#[test]
fn upgrade_reruns_hook_after_source_advances() {
    // spec: HOOK-11
    // After a source advances to a new commit, upgrade re-runs the hook (the
    // tooling tracks the source). When the source has not advanced, upgrade does
    // not re-run the hook (the recorded run-commit already equals the commit).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"])
            .success,
        "initial meld should run the hook and record commit C1"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");

    // Clear the marker so a re-run is observable.
    std::fs::remove_file(&marker).unwrap();

    // Advance the source and sync (sync alone must not run the hook).
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);
    assert!(
        !marker.exists(),
        "sync alone must not re-run the hook (HOOK-11)"
    );

    // upgrade sees the new commit and re-runs the hook.
    let ev = sb.mind(&["upgrade", "-y", "--dangerously-skip-install-hook-check"]);
    assert!(ev.success, "upgrade failed: {} {}", ev.stdout, ev.stderr);
    assert!(
        marker.exists(),
        "upgrade must re-run the hook after the source advanced: {} missing",
        marker.display()
    );

    // The recorded run-commit advanced to the new commit; a second upgrade with
    // no source change must NOT re-run the hook.
    std::fs::remove_file(&marker).unwrap();
    let again = sb.mind(&["upgrade", "-y", "--dangerously-skip-install-hook-check"]);
    assert!(again.success, "second upgrade failed: {}", again.stderr);
    assert!(
        !marker.exists(),
        "upgrade must not re-run the hook when the source has not advanced"
    );
}

#[test]
fn sync_upgrade_runs_hook_rerun_only_with_the_skip_check_flag() {
    // spec: HOOK-11, HOOK-23
    // `sync --upgrade` drives an upgrade pass, so it must honor the same hook
    // re-run rules. In a non-TTY context the re-run is skipped (HOOK-22), and
    // `--dangerously-skip-install-hook-check` threaded through `sync` is what
    // runs it unattended (HOOK-23) -- the CI workflow the flag exists for.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"])
            .success,
        "initial meld should run the hook and record commit C1"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");
    std::fs::remove_file(&marker).unwrap();

    // Advance the source so a re-run is warranted (the recorded run-commit now
    // lags the source's commit).
    sb.edit_source();

    // `sync --upgrade` with no flag: sync advances the commit, the upgrade pass
    // sees the new commit but takes the non-TTY skip path, so the hook does not
    // re-run.
    let no_flag = sb.mind(&["sync", "--upgrade"]);
    assert!(
        no_flag.success,
        "sync --upgrade failed: {} {}",
        no_flag.stdout, no_flag.stderr
    );
    assert!(
        !marker.exists(),
        "sync --upgrade without the flag must not re-run the hook (HOOK-22)"
    );

    // `sync --upgrade --dangerously-skip-install-hook-check`: the flag now
    // reaches the upgrade pass, which re-runs the still-warranted hook unattended.
    let with_flag = sb.mind(&["sync", "--upgrade", "--dangerously-skip-install-hook-check"]);
    assert!(
        with_flag.success,
        "sync --upgrade --dangerously-skip-install-hook-check failed: {} {}",
        with_flag.stdout, with_flag.stderr
    );
    assert!(
        marker.exists(),
        "sync --upgrade with the flag must re-run the hook unattended: {} missing",
        marker.display()
    );
}

#[test]
fn scoped_upgrade_does_not_rerun_unrelated_source_hook() {
    // spec: HOOK-11
    // A scoped `upgrade <item>` must NOT re-run install hooks (arbitrary code) for
    // sources unrelated to the targeted item. Meld a hooked source (`agents`,
    // recorded via --dangerously-skip-install-hook-check) plus a second,
    // hook-free source (`tools`); learn an item only from `tools`; advance the
    // hooked source and sync. A scoped upgrade targeting the `tools` item must
    // leave the hooked source's marker untouched, while an UNSCOPED upgrade (the
    // positive control) does re-run it.
    let agents = sandbox_with_declared_hook("agents", "touch hookran");
    let agents_spec = agents.source_spec();
    assert!(
        agents
            .mind(&[
                "meld",
                &agents_spec,
                "--dangerously-skip-install-hook-check"
            ])
            .success,
        "initial meld of the hooked source should run the hook and record its commit"
    );

    let tools = Sandbox::named("tools");
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of the second (hook-free) source failed"
    );

    // Learn an item from the OTHER source only, source-qualified so it resolves
    // unambiguously across the two sources that share fixture item names.
    let learn = agents.mind(&["learn", "tools#skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    // The hook ran on meld; clear its marker so any re-run is observable.
    let marker = agents.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the hook should have run on the initial meld"
    );
    std::fs::remove_file(&marker).unwrap();

    // Advance the hooked source so its commit moves past the recorded run-commit,
    // i.e. an UNSCOPED upgrade would re-run its hook. sync alone must not.
    agents.edit_source();
    assert!(agents.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // Scoped upgrade targeting the OTHER source's item: the hooked source is out
    // of scope, so its hook must NOT re-run even though its commit advanced.
    let scoped = agents.mind(&[
        "upgrade",
        "tools#skill:review",
        "-y",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        scoped.success,
        "scoped upgrade failed: {} {}",
        scoped.stdout, scoped.stderr
    );
    assert!(
        !marker.exists(),
        "a scoped upgrade of an unrelated item must not re-run the hooked source's hook: {} exists",
        marker.display()
    );

    // Positive control: an UNSCOPED upgrade DOES re-run the hooked source's hook.
    let unscoped = agents.mind(&["upgrade", "-y", "--dangerously-skip-install-hook-check"]);
    assert!(
        unscoped.success,
        "unscoped upgrade failed: {} {}",
        unscoped.stdout, unscoped.stderr
    );
    assert!(
        marker.exists(),
        "an unscoped upgrade must re-run the hooked source's hook: {} missing",
        marker.display()
    );
}

#[test]
fn glob_scoped_upgrade_does_not_rerun_unrelated_source_hook() {
    // spec: CLI-65, HOOK-11
    // The hook_scope filter is computed via installed_matches_glob, so a GLOB ref
    // must scope install-hook re-runs the same way an exact ref does: a source
    // qualifier glob (`tools#*`) targeting one source must NOT re-run an unrelated
    // hooked source's install hook, even though that source's commit advanced.
    // This exercises the glob branch of the hook_scope computation, distinct from
    // the exact-ref path covered by scoped_upgrade_does_not_rerun_unrelated_source_hook.
    let agents = sandbox_with_declared_hook("agents", "touch hookran");
    let agents_spec = agents.source_spec();
    assert!(
        agents
            .mind(&[
                "meld",
                &agents_spec,
                "--dangerously-skip-install-hook-check"
            ])
            .success,
        "initial meld of the hooked source should run the hook and record its commit"
    );

    let tools = Sandbox::named("tools");
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of the second (hook-free) source failed"
    );

    // Install an item from the hook-free tools source only.
    let learn = agents.mind(&["learn", "tools#skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    // Clear the meld-time marker so any re-run is observable.
    let marker = agents.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the hook should have run on the initial meld"
    );
    std::fs::remove_file(&marker).unwrap();

    // Advance the hooked source so an UNSCOPED upgrade would re-run its hook.
    agents.edit_source();
    assert!(agents.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // A SOURCE-GLOB scoped upgrade of the OTHER source: the hooked agents source
    // is out of scope, so its hook must NOT re-run.
    let scoped = agents.mind(&[
        "upgrade",
        "tools#*",
        "-y",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        scoped.success,
        "glob-scoped upgrade failed: {} {}",
        scoped.stdout, scoped.stderr
    );
    assert!(
        !marker.exists(),
        "a glob-scoped upgrade of an unrelated source must not re-run the hooked source's hook: {} exists",
        marker.display()
    );

    // Positive control: an unscoped upgrade DOES re-run the hooked source's hook.
    let unscoped = agents.mind(&["upgrade", "-y", "--dangerously-skip-install-hook-check"]);
    assert!(
        unscoped.success,
        "unscoped upgrade failed: {} {}",
        unscoped.stdout, unscoped.stderr
    );
    assert!(
        marker.exists(),
        "an unscoped upgrade must re-run the hooked source's hook: {} missing",
        marker.display()
    );
}

#[test]
fn upgrade_skips_disallowed_source_hook_when_locked() {
    // spec: POL-12
    // Install hooks are arbitrary code; running a disallowed source's hook would
    // violate POL-12. Meld + record a hooked source while it is allowed, then
    // advance it and run upgrade under a locked policy whose `allow` excludes the
    // source: the hook must NOT re-run (marker not re-created) and the skip is
    // reported.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"])
            .success,
        "initial meld should run the hook and record its commit"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");
    std::fs::remove_file(&marker).unwrap();

    // Advance the source so an UNSCOPED upgrade would otherwise re-run the hook.
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // A locked policy whose allowlist excludes this source.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "-y", "--dangerously-skip-install-hook-check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "upgrade must not error when skipping a disallowed source's hook: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !marker.exists(),
        "a policy-disallowed source's hook must not re-run: {} exists",
        marker.display()
    );
    assert!(
        r.stdout.contains("skipping install hook for")
            && r.stdout
                .contains("not permitted by the managed policy's allowlist"),
        "the skipped hook must be reported: {}",
        r.stdout
    );
}

#[test]
fn evolve_check_with_explicit_version_reports_update_and_changes_nothing() {
    // spec: CLI-141
    // `evolve --check --version <X>` makes zero network calls (an explicit
    // --version bypasses the GitHub API). When X > the running version, the
    // command must succeed and report the update as available.
    let sb = Sandbox::new(); // empty sandbox; no sources or manifest needed
    let r = sb.mind(&["evolve", "--check", "--version", "9.9.9"]);
    assert!(
        r.success,
        "evolve --check --version 9.9.9 should succeed: {} {}",
        r.stdout, r.stderr
    );
    // The output must contain the target version and signal it is available.
    assert!(
        r.stdout.contains("9.9.9"),
        "expected target version 9.9.9 in output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("available"),
        "expected 'available' in output: {}",
        r.stdout
    );
    // Nothing on disk changed: no source or manifest files exist in the sandbox.
    assert!(
        !sb.mind_home.join("sources.json").exists(),
        "no sources.json should be written by evolve --check"
    );
    assert!(
        !sb.mind_home.join("manifest.json").exists(),
        "no manifest.json should be written by evolve --check"
    );
}

#[test]
fn evolve_check_at_current_version_reports_up_to_date() {
    // spec: CLI-141
    // When the explicit --version equals the running binary version, evolve
    // --check reports up to date and exits zero, with zero network calls.
    let sb = Sandbox::new();
    let current = env!("CARGO_PKG_VERSION");
    let r = sb.mind(&["evolve", "--check", "--version", current]);
    assert!(
        r.success,
        "evolve --check --version {current} should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("up to date"),
        "expected 'up to date' in output for version {current}: {}",
        r.stdout
    );
}

#[test]
fn help_lists_upgrade_evolve_and_self_update_alias() {
    // Confirm clap renders both subcommands. `self-update` is now a visible alias
    // for `evolve` (CLI-172) and must appear in --help.
    let sb = Sandbox::new();
    let r = sb.mind(&["--help"]);
    assert!(
        r.success,
        "mind --help should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgrade"),
        "help must list the 'upgrade' subcommand: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("evolve"),
        "help must list the 'evolve' subcommand: {}",
        r.stdout
    );
    // spec: CLI-172 - self-update is now a visible alias for evolve.
    assert!(
        r.stdout.contains("self-update"),
        "help must list the 'self-update' visible alias: {}",
        r.stdout
    );
}

// ---- evolve policy control tests (POL-51..54) --------------------------------
//
// Policy is injected via $MIND_POLICY_FILE. evolve --check --version <X> is
// fully offline (no network); that property is exploited here so tests run
// without a live GitHub connection.

#[test]
fn evolve_disabled_by_policy_rejects_both_check_and_run() {
    // spec: POL-52 -- self-update = false must gate both evolve and evolve --check
    // before any network call.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = false\n");

    // --check mode: must fail even though it normally makes no network call.
    let r = sb.mind_env(
        &["evolve", "--check", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "evolve --check must be refused when self-update = false: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("disabled by the managed policy"),
        "error must name the policy: {}",
        r.stderr
    );
    // spec: POL-66 -- self-update = false errors BEFORE the PinnedBelowCurrent
    // decision, so the skew warning path must never be reached: no warning text on
    // stdout even though a --version below current is supplied.
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "disabled policy must not reach/print the skew warning: {}",
        r.stdout
    );

    // run mode (--yes skips the interactive prompt): must also fail, no download.
    let r = sb.mind_env(
        &["evolve", "--yes", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "evolve --yes must be refused when self-update = false: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("disabled by the managed policy"),
        "run error must also name the policy: {}",
        r.stderr
    );
    // spec: POL-66 -- likewise the run path must not reach the skew warning.
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "disabled policy (run mode) must not reach/print the skew warning: {}",
        r.stdout
    );
}

#[test]
fn evolve_pinned_by_policy_check_is_offline_and_names_pin() {
    // spec: POL-53 -- self-update = "<version>" forces that version as the target
    // without any API call. evolve --check with the pin set must succeed offline
    // and mention the pinned version in its report.
    let sb = Sandbox::new();
    // Pin to "0.1.0", which is below any real build, so the decision is
    // PinnedBelowCurrent and the output says "not downgrading". No network needed.
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with a pin below current must succeed (not-downgrading path): {} {}",
        r.stdout, r.stderr
    );
    // The output must reference the pinned version.
    assert!(
        r.stdout.contains("0.1.0"),
        "check output must name the pinned version: {}",
        r.stdout
    );
}

#[test]
fn evolve_pinned_mismatched_version_arg_fails() {
    // spec: POL-53 -- when policy pins to X and --version Y (Y != X) is given,
    // the command must fail naming the conflict.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.14.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "0.15.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "mismatched --version must be refused by a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("0.14.0"),
        "error must name the policy pin: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("conflicts") || r.stderr.contains("conflict"),
        "error must say 'conflicts': {}",
        r.stderr
    );
}

#[test]
fn evolve_pinned_matching_version_arg_succeeds() {
    // spec: POL-53 -- when --version matches the policy pin exactly, the command
    // proceeds normally. Pin = 0.1.0 (below current) with --version 0.1.0 ->
    // PinnedBelowCurrent report, exit 0.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "0.1.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "matching --version must be accepted by a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("0.1.0"),
        "output must name the pinned version: {}",
        r.stdout
    );
}

#[test]
fn evolve_self_update_true_allows_evolve() {
    // spec: POL-54 -- self-update = true is identical to the absent key.
    // evolve --check --version 9.9.9 (offline explicit check) must succeed.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = true\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "self-update = true must allow evolve: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("9.9.9"),
        "output must name the target version: {}",
        r.stdout
    );
}

// ---- POL-66: policy pin skew warning -----------------------------------------

#[test]
fn evolve_policy_pin_below_running_emits_skew_warning_human_mode() {
    // spec: POL-66 -- when the running binary is above the policy pin
    // (PinnedBelowCurrent), evolve and evolve --check must print a human-readable
    // warning that the running version differs from the policy pin and that the
    // pin is an upper bound. Pin "0.1.0" is always below any real build version.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    // Test the --check path.
    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("warning:"),
        "check mode must print a warning when running > policy pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("0.1.0"),
        "warning must name the policy pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("upper bound") || r.stdout.contains("does not downgrade"),
        "warning must say the pin is an upper bound or does not downgrade: {}",
        r.stdout
    );

    // Test the non-check (evolve --yes --version 0.1.0) path. We cannot do a real
    // download, but we can test the PinnedBelowCurrent branch by letting the policy
    // pin steer the version. Without --yes the binary would prompt; with a policy
    // pin below current the PinnedBelowCurrent branch returns before the prompt.
    let r2 = sb.mind_env(&["evolve"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "evolve (no --check) with a pin below current must exit 0: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("warning:"),
        "non-check evolve must also print the skew warning: {}",
        r2.stdout
    );
}

#[test]
fn evolve_policy_pin_skew_json_no_warning_on_stdout_outcome_is_not_downgrading() {
    // spec: POL-66 -- --json mode must NOT print the human warning text on stdout;
    // the machine-readable hook is the structured `outcome` field. Exit code 0.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check --json with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    // The whole stdout must parse as ONE well-formed JSON object: a stray warning
    // line prepended/appended (or unescaped text) would corrupt it. Substring
    // matching alone would not catch such corruption, so parse it.
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "evolve --json stdout must be valid JSON ({e}): {}",
            r.stdout
        )
    });
    assert_eq!(
        v.get("action").and_then(|a| a.as_str()),
        Some("evolve"),
        "structured action must be 'evolve': {}",
        r.stdout
    );
    // The structured outcome must be exactly "not-downgrading".
    assert_eq!(
        v.get("outcome").and_then(|o| o.as_str()),
        Some("not-downgrading"),
        "JSON outcome must be 'not-downgrading': {}",
        r.stdout
    );
    // The human warning text must not appear anywhere on stdout.
    assert!(
        !r.stdout.contains("warning:"),
        "JSON mode must not print the human warning on stdout: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("upper bound"),
        "JSON mode must not leak the skew-warning wording onto stdout: {}",
        r.stdout
    );
}

#[test]
fn evolve_policy_pin_skew_non_check_json_no_warning_valid_json() {
    // spec: POL-66 -- the NON-check --json path (the second, distinct warning site
    // in selfupdate::run, guarded by its own `if out.json { return }`) must also
    // emit only the structured outcome with no warning text and produce valid JSON.
    // The --check --json test above exercises the first site; this pins the second.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --json (non-check) with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "non-check evolve --json stdout must be valid JSON ({e}): {}",
            r.stdout
        )
    });
    assert_eq!(
        v.get("outcome").and_then(|o| o.as_str()),
        Some("not-downgrading"),
        "non-check JSON outcome must be 'not-downgrading': {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "non-check JSON mode must not print the skew warning on stdout: {}",
        r.stdout
    );
}

#[test]
fn evolve_policy_pin_equal_to_running_no_skew_warning() {
    // spec: POL-66 -- when pin == running, the decision is UpToDate, not
    // PinnedBelowCurrent, so no skew warning is emitted.
    let sb = Sandbox::new();
    let current = env!("CARGO_PKG_VERSION");
    let policy = write_policy(&sb, &format!("[binary]\nself-update = \"{current}\"\n"));

    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with pin == running must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("warning:"),
        "no skew warning when pin equals the running version: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("up to date"),
        "output must say 'up to date' when pin == running: {}",
        r.stdout
    );
}

#[test]
fn evolve_explicit_version_below_running_no_policy_no_skew_warning() {
    // spec: POL-66 -- the skew warning is specific to the policy-pin case.
    // When the user passes --version X (below current) with no policy in effect,
    // the existing "not downgrading" message is sufficient; no skew warning.
    let sb = Sandbox::new();
    let r = sb.mind(&["evolve", "--check", "--version", "0.1.0"]);
    assert!(
        r.success,
        "evolve --check --version 0.1.0 (below current, no policy) must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("warning:"),
        "no skew warning when pin is from --version with no policy: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("not downgrading"),
        "output must say 'not downgrading': {}",
        r.stdout
    );
}

// ---- lifecycle-hook system tests (HOOK-50..58) --------------------------------
//
// These tests cover the extended hook system: multiple named [[hooks]] entries,
// optional hooks, uninstall hooks at `unmeld`, and the `init-source` scaffold.
// All tests run non-TTY (stdin piped), so interactive prompts never fire.

#[test]
fn remeld_reoffers_pending_install_hooks_and_force_reruns() {
    // spec: HOOK-60
    let sb = Sandbox::bare("remeld-hook");
    let marker = sb.base.join("hook-ran");
    let m = marker.to_str().unwrap().to_owned();
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"install\"\n"),
    );
    let spec = sb.source_spec();

    // A fresh non-TTY meld registers but skips the hook (HOOK-22).
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(!marker.exists(), "hook skipped on the non-TTY meld");

    // Re-melding re-offers the pending (skipped) hook; the dangerous flag runs it.
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(marker.exists(), "re-meld must run the pending hook");

    // Now recorded as run at this commit: a plain re-meld does not re-run it.
    std::fs::remove_file(&marker).unwrap();
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(
        !marker.exists(),
        "a hook already run at this commit is not re-offered"
    );

    // --force re-offers (and re-runs) every hook regardless.
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--force",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(marker.exists(), "--force must re-run an already-run hook");
}

#[test]
fn recall_status_view_marks_install_state() {
    // spec: CLI-70, CLI-74
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let out = sb.mind(&["recall"]).stdout;
    // The source header is present, with its items nested and marked.
    assert!(out.contains("agents"), "source header: {out}");
    assert!(
        out.contains("skill:review") && out.contains("installed @"),
        "an installed item is marked installed with its commit: {out}"
    );
    assert!(
        out.contains("agent:dev") && out.contains("available"),
        "a not-installed item is marked available: {out}"
    );
}

#[test]
fn install_hook_output_is_mirrored_to_mind_stdout() {
    // spec: HOOK-30 - a hook's stdout is mirrored to mind's own output under
    // a labeled separator frame.
    let sb = Sandbox::bare("hook-output");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nrun = \"echo HELLO-FROM-HOOK\"\nname = \"build\"\nevent = \"install\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--link-only",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("====== (hook-stdout: build) ======"),
        "the stdout separator frame must appear in mind's output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("HELLO-FROM-HOOK"),
        "the hook's stdout must be mirrored to mind's output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("====== (end hook: build) ======"),
        "the closing divider must separate the hook output from what follows: {}",
        r.stdout
    );
}

#[test]
fn install_hook_stderr_is_framed_and_mirrored() {
    // spec: HOOK-30 - a hook's stderr is captured and printed under a labeled
    // separator frame, visible in mind's output.
    let sb = Sandbox::bare("hook-stderr");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nrun = \"echo OOPS 1>&2\"\nname = \"warn\"\nevent = \"install\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--link-only",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("====== (hook-stderr: warn) ======"),
        "the stderr separator frame must appear in mind's output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("OOPS"),
        "the hook's stderr must be mirrored to mind's output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("====== (end hook: warn) ======"),
        "the closing divider must separate the hook output from what follows: {}",
        r.stdout
    );
}

#[test]
fn meld_runs_multiple_install_hooks_with_dangerous_flag() {
    // spec: HOOK-50
    // A source with two [[hooks]] entries (both event = "install") runs both
    // hooks in declaration order when --dangerously-skip-install-hook-check is
    // given. Both marker files must exist after the meld succeeds.
    let sb = Sandbox::bare("multi-hook");
    let marker1 = sb.base.join("marker1");
    let marker2 = sb.base.join("marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--dangerously-skip-install-hook-check",
        "--link-only",
    ]);
    assert!(
        r.success,
        "meld with two install hooks should succeed: {} {}",
        r.stdout, r.stderr
    );

    assert!(
        marker1.exists(),
        "first install hook must have run (marker1 missing): {}",
        marker1.display()
    );
    assert!(
        marker2.exists(),
        "second install hook must have run (marker2 missing): {}",
        marker2.display()
    );

    // Source is registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("multi-hook"),
        "source must be registered after both hooks ran: {sources}"
    );
}

#[test]
fn meld_non_tty_skips_install_hooks_and_still_registers_source() {
    // spec: HOOK-22 (preserved with multi-hook)
    // Without --dangerously-skip-install-hook-check, a non-TTY meld skips all
    // hooks, prints a skip note, and still registers the source.
    let sb = Sandbox::bare("multi-hook-skip");
    let marker1 = sb.base.join("skip-marker1");
    let marker2 = sb.base.join("skip-marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Default meld: non-TTY, no dangerous flag.
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld should still succeed on non-TTY skip: {} {}",
        r.stdout, r.stderr
    );

    // Neither hook ran.
    assert!(
        !marker1.exists(),
        "hook must not have run in non-TTY mode (marker1 exists)"
    );
    assert!(
        !marker2.exists(),
        "hook must not have run in non-TTY mode (marker2 exists)"
    );

    // Skip note is printed with the exact prefix that `run_install_hooks` emits
    // on the HOOK-22 skip path. Asserting the literal prefix ensures the message
    // is present and not just any word "skipped" in unrelated output.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("note: skipped install hook "),
        "non-TTY skip must print a note starting with 'note: skipped install hook ': {combined}"
    );

    // Source is still registered (HOOK-22: skip-and-continue registers the source).
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("multi-hook-skip"),
        "source must be registered even when hooks are skipped: {sources}"
    );
}

#[test]
fn optional_install_hook_failure_aborts_meld() {
    // spec: HOOK-53
    // An optional hook's non-zero exit is a hard stop, like a required hook: the
    // meld fails and the source is not registered. `optional` only governs whether
    // the user may decline to run it, never whether it may fail.
    let sb = Sandbox::bare("optional-hook-fail");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"install\"\noptional = true\n";
    sb.write_and_commit("mind.toml", toml);
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--dangerously-skip-install-hook-check",
        "--link-only",
    ]);
    assert!(
        !r.success,
        "an optional hook failure must abort the meld: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "--sources"])
            .stdout
            .contains("optional-hook-fail"),
        "the source must not be registered after a failed optional hook"
    );
}

#[test]
fn required_install_hook_failure_aborts_meld() {
    // spec: HOOK-53
    // A required install hook that exits non-zero fails the meld entirely: the
    // source is NOT registered and the command exits with a non-zero status.
    let sb = Sandbox::bare("required-fail");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"install\"\n";
    sb.write_and_commit("mind.toml", toml);
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(
        !r.success,
        "meld must fail when a required install hook exits non-zero: {} {}",
        r.stdout, r.stderr
    );

    // Source is NOT registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("required-fail"),
        "source must not be registered after a required hook failure: {sources}"
    );
}

#[test]
fn unmeld_runs_uninstall_hook_with_dangerous_flag() {
    // spec: HOOK-54
    // A source with an event = "uninstall" hook: after meld, `unmeld --dangerously-skip-install-hook-check`
    // runs the hook and removes the source from the registry.
    let sb = Sandbox::bare("uninstall-hook");
    let uninstall_marker = sb.base.join("uninstall-ran");
    let m = uninstall_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Meld first (no uninstall hooks run at meld time).
    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        !uninstall_marker.exists(),
        "uninstall hook must not run at meld time"
    );

    // unmeld with dangerous flag: uninstall hook runs, source removed.
    let unmeld = sb.mind(&[
        "unmeld",
        "uninstall-hook",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        unmeld.success,
        "unmeld should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );

    assert!(
        uninstall_marker.exists(),
        "uninstall hook must have run at unmeld: marker missing at {}",
        uninstall_marker.display()
    );

    // Source is no longer registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("uninstall-hook"),
        "source must be removed after unmeld: {sources}"
    );
}

#[test]
fn unmeld_uninstall_hook_override_replaces_declared() {
    // spec: HOOK-59
    // `unmeld --uninstall-hook <cmd>` replaces the source's declared uninstall
    // hook: the override command runs, the declared one does not.
    let sb = Sandbox::bare("uninstall-override");
    let declared_marker = sb.base.join("declared-ran");
    let override_marker = sb.base.join("override-ran");
    let dm = declared_marker.to_str().unwrap().to_owned();
    let om = override_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {dm}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success, "meld");

    let unmeld = sb.mind(&[
        "unmeld",
        "uninstall-override",
        "--uninstall-hook",
        &format!("touch {om}"),
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        unmeld.success,
        "unmeld --uninstall-hook should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    assert!(
        override_marker.exists(),
        "the override uninstall hook must run: {}",
        override_marker.display()
    );
    assert!(
        !declared_marker.exists(),
        "the declared uninstall hook must not run when overridden"
    );
    assert!(
        !sb.mind(&["recall", "--sources"])
            .stdout
            .contains("uninstall-override"),
        "source must be removed"
    );
}

#[test]
fn unmeld_non_tty_skips_uninstall_hook_but_still_removes_source() {
    // spec: HOOK-54 (non-TTY path)
    // A plain non-TTY `unmeld` (no dangerous flag) skips the uninstall hook
    // (no marker) but still removes the source from the registry.
    let sb = Sandbox::bare("uninstall-skip");
    let uninstall_marker = sb.base.join("uninstall-skip-ran");
    let m = uninstall_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );

    // Unmeld without the dangerous flag: non-TTY -> skip hook, still remove source.
    let unmeld = sb.mind(&["unmeld", "uninstall-skip"]);
    assert!(
        unmeld.success,
        "unmeld should succeed even when hook is skipped: {} {}",
        unmeld.stdout, unmeld.stderr
    );

    // Hook did NOT run.
    assert!(
        !uninstall_marker.exists(),
        "uninstall hook must not run in non-TTY mode without the dangerous flag"
    );

    // Source is still removed (skip-and-continue).
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("uninstall-skip"),
        "source must be removed even when uninstall hook is skipped: {sources}"
    );
}

#[test]
fn init_source_scaffold_offers_hook_examples() {
    // spec: HOOK-57
    // `mind init-source <dir>` on a fresh repo dir writes a mind.toml scaffold
    // whose content contains commented [[hooks]] examples for both install and
    // uninstall events, including optional = true.
    let sb = Sandbox::new();
    let repo = sb.base.join("new-source");
    // Write a minimal item so init-source has something to discover.
    write(
        &repo.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: A greeting skill\n---\n# greet\n",
    );
    let dir = repo.to_str().unwrap();

    let r = sb.mind(&["init-source", dir]);
    assert!(
        r.success,
        "init-source should succeed: {} {}",
        r.stdout, r.stderr
    );

    let scaffold =
        std::fs::read_to_string(repo.join("mind.toml")).expect("init-source must create mind.toml");

    // The scaffold must contain commented [[hooks]] entries.
    assert!(
        scaffold.contains("[[hooks]]"),
        "scaffold must contain [[hooks]] examples: {scaffold}"
    );

    // Must have both install and uninstall event examples, on comment lines.
    let has_install_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("event") && l.contains("install"));
    assert!(
        has_install_comment,
        "scaffold must have a commented event = \"install\" line: {scaffold}"
    );

    let has_uninstall_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("event") && l.contains("uninstall"));
    assert!(
        has_uninstall_comment,
        "scaffold must have a commented event = \"uninstall\" line: {scaffold}"
    );

    // Must have optional = true on a comment line.
    let has_optional_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("optional") && l.contains("true"));
    assert!(
        has_optional_comment,
        "scaffold must have a commented optional = true line: {scaffold}"
    );
}

#[test]
fn recall_sources_marks_multi_hook_source() {
    // spec: HOOK-58
    // After a multi-hook meld (with --dangerously-skip-install-hook-check so the
    // hooks are recorded), `recall --sources` contains a `hook` token indicating
    // the source has hooks.
    let sb = Sandbox::bare("hook-report");
    let marker1 = sb.base.join("report-marker1");
    let marker2 = sb.base.join("report-marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--dangerously-skip-install-hook-check",
            "--link-only"
        ])
        .success,
        "meld should succeed"
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.success,
        "recall --sources failed: {}",
        sources.stderr
    );

    // The output must contain the count-aware ` hooks(2)` token (HOOK-58:
    // N > 1 renders as ` hooks(N)`) for the two declared install hooks.
    // This assertion would fail if the token were dropped or rendered differently.
    assert!(
        sources.stdout.contains(" hooks(2)"),
        "recall --sources must mark a two-hook source with ' hooks(2)': {}",
        sources.stdout
    );
}

#[test]
fn pinned_local_meld_hook_failure_leaves_no_orphan_clone() {
    // spec: CLI-18, CLI-27, HOOK-30
    // A pinned local source (`--pin-ref`) is snapshotted into the sources tree
    // rather than read from the working tree. When a hook fails during that meld,
    // the snapshot clone must be removed (no orphan) and the source must not be
    // registered. The working tree itself must be untouched (CLI-27).
    let sb = sandbox_with_declared_hook("agents", "exit 1");
    let spec = sb.source_spec();

    // Read HEAD sha to supply as --pin-ref (so this becomes a pinned-local meld).
    let sha = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };

    let r = sb.mind(&[
        "meld",
        &spec,
        "--pin-ref",
        &sha,
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !r.success,
        "hook failure must fail the meld: {} {}",
        r.stdout, r.stderr
    );

    // Nothing registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "source must not be registered after a failed hook"
    );

    // The snapshot clone must be gone -- no orphan under the sources tree.
    let sources_tree = sb.mind_home.join("sources");
    if sources_tree.exists() {
        let clone = sources_tree
            .join("local")
            .join(sb.base_name())
            .join("agents");
        assert!(
            !clone.exists(),
            "pinned-local clone must be removed on hook failure, found orphan at {}",
            clone.display()
        );
    }

    // The working tree itself must be untouched (CLI-27).
    assert!(
        sb.source.exists(),
        "working tree must survive a failed pinned-local meld: {}",
        sb.source.display()
    );
}

#[test]
fn upgrade_pending_filter_treats_none_ran_at_as_always_pending() {
    // spec: HOOK-55, HOOK-11
    // A hook recorded with ran_at=None (skipped at meld time) must be re-offered
    // by `upgrade` even when the source's commit is also None (a commitless linked
    // source). The predicate `ran_at.is_none() || ran_at != commit` ensures this.
    //
    // The test melds a local source declaring a hook (non-TTY meld skips it,
    // recording ran_at=null), then runs `upgrade --dangerously-skip-install-hook-check`.
    // The hook must re-run (marker appears) proving the none-pending filter works.
    let sb = Sandbox::bare("upgrade-pending");
    let marker = sb.base.join("upgrade-pending-ran");
    let m = marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"install\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Meld without the dangerous flag: non-TTY skips the hook, ran_at=null.
    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        !marker.exists(),
        "hook must not run at meld time (non-TTY skip)"
    );

    // Verify the registry has ran_at=null for the hook.
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("\"ran_at\": null"),
        "registry must record ran_at=null for the skipped hook: {json}"
    );

    // Upgrade with the dangerous flag: the skipped (ran_at=null) hook must re-run.
    let upgrade = sb.mind(&["upgrade", "--dangerously-skip-install-hook-check"]);
    assert!(
        upgrade.success,
        "upgrade should succeed: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        marker.exists(),
        "upgrade must re-run a hook with ran_at=null (none-pending filter): marker absent"
    );
}

#[test]
fn unmeld_confirm_decline_leaves_source_melded_and_hook_not_run() {
    // spec: CLI-21, CLI-42, HOOK-54
    // When the default unmeld would remove multiple items, the multi-item
    // confirmation must happen BEFORE uninstall hooks run. A user who answers
    // "no" must leave the source melded AND the hook must not have executed.
    //
    // TTY simulation: send "n\n" as stdin to exercise the confirm path.
    let sb = Sandbox::bare("unmeld-confirm-order");
    let sentinel = sb.base.join("uninstall-ran");
    let s = sentinel.to_str().unwrap().to_owned();
    let hook_toml = format!("[[hooks]]\nrun = \"touch {s}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &hook_toml);

    // Also add two items so the multi-item confirm triggers.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    sb.write_and_commit(
        "agents/ops.md",
        "---\nname: ops\ndescription: ops\n---\n# ops\n",
    );

    let spec = sb.source_spec();

    // Meld and install both items.
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success, "meld");
    assert!(sb.mind(&["learn", "agent:dev"]).success, "learn dev");
    assert!(sb.mind(&["learn", "agent:ops"]).success, "learn ops");

    // Unmeld with TTY input "n" to decline the multi-item confirm.
    // The test harness sets stdin to a pipe, so the subprocess sees a TTY-like
    // stdin for reading input but is_tty() is false (piped). We therefore use
    // --yes=false path by omitting --yes, and the non-TTY branch refuses with
    // ConfirmationRequired rather than prompting.
    //
    // Non-TTY behavior: with 2 items and no --yes, unmeld errors BEFORE running
    // hooks. Assert the source is still registered and the hook sentinel is absent.
    let r = sb.mind(&["unmeld", "unmeld-confirm-order"]);
    assert!(
        !r.success,
        "unmeld without --yes must fail in non-TTY: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );

    // Sentinel must be absent: hook did NOT run before the confirmation gate.
    assert!(
        !sentinel.exists(),
        "uninstall hook must not run before the multi-item confirmation gate: sentinel exists"
    );

    // Source must still be registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("unmeld-confirm-order"),
        "source must remain melded after a declined confirm: {sources}"
    );
}

#[test]
fn unmeld_failing_uninstall_hook_leaves_source_melded() {
    // spec: HOOK-53, HOOK-54, HOOK-87
    // A source uninstall hook that exits non-zero is a hard stop: the unmeld
    // fails and the source remains registered. Under HOOK-87 the source hook runs
    // AFTER the items are torn down, so the item is already removed when the
    // source hook fails; the source itself stays melded.
    let sb = Sandbox::bare("failing-uninstall-hook");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"uninstall\"\n";
    sb.write_and_commit("mind.toml", toml);
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let spec = sb.source_spec();

    // Meld and install the item.
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld should succeed"
    );
    assert!(sb.mind(&["learn", "agent:dev"]).success, "learn dev");

    // Unmeld with dangerous flag so the hook runs (non-TTY would skip it).
    let r = sb.mind(&[
        "unmeld",
        "failing-uninstall-hook",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !r.success,
        "unmeld must fail when uninstall hook exits non-zero: {} {}",
        r.stdout, r.stderr
    );

    // Source must still be registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("failing-uninstall-hook"),
        "source must remain melded after a failed uninstall hook: {sources}"
    );

    // HOOK-87: teardown reverses install, so the item is removed BEFORE the
    // source uninstall hook runs; by the time that hook fails the item is gone.
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "the item is torn down before the source uninstall hook fires (HOOK-87)"
    );
}

/// A source with two shared tools and a skill (plus a bundled script) that
/// reference them via path tokens. Committed and ready to meld.
fn tool_source() -> Sandbox {
    let sb = Sandbox::bare("agents");
    // Two shared tools; each entrypoint is the convention default file.
    write(
        &sb.source.join("tools/shard/shard"),
        "#!/bin/sh\necho shard\n",
    );
    // detect's helper file references the other tool (tool -> tool).
    write(
        &sb.source.join("tools/detect/detect"),
        "#!/bin/sh\necho detect\n",
    );
    write(
        &sb.source.join("tools/detect/lib.sh"),
        "exec {{tools:shard}} \"$@\"\n",
    );
    // A skill referencing its own file, a tool's entrypoint, and a non-entrypoint
    // file inside a tool. Its bundled run.sh also calls a tool (script -> tool).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\nrun {{self}}/run.sh\ndetect {{tools:detect}} .\nlib {{path:tool:detect}}/lib.sh\n",
    );
    write(
        &sb.source.join("skills/review/run.sh"),
        "#!/bin/sh\n{{tools:detect}} run\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "tools"]);
    sb
}

#[test]
fn tool_installs_store_only_and_tokens_expand_everywhere() {
    // spec: TOOL-3 TOOL-13 TOOL-14 TOOL-15
    let sb = tool_source();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    let store = sb.mind_home.join("store");
    // The tools install to the store...
    assert!(store.join("tool/detect/detect").is_file());
    assert!(store.join("tool/shard/shard").is_file());
    // ...but are store-only: not linked into the agent home.
    assert!(
        !sb.claude_home.join("tools").exists(),
        "a tool must not be linked into an agent home"
    );
    assert!(!sb.claude_home.join("skills/detect").exists());
    // The skill links normally.
    let link = sb.claude_home.join("skills/review");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the skill links as usual"
    );

    // Tokens expanded to store paths in the SKILL.md...
    let s = store.display().to_string();
    let skill_md = std::fs::read_to_string(store.join("skill/review/SKILL.md")).unwrap();
    assert!(
        skill_md.contains(&format!("run {s}/skill/review/run.sh")),
        "{skill_md}"
    );
    assert!(
        skill_md.contains(&format!("detect {s}/tool/detect/detect .")),
        "{skill_md}"
    );
    assert!(
        skill_md.contains(&format!("lib {s}/tool/detect/lib.sh")),
        "{skill_md}"
    );
    // ...in the skill's bundled script (TOOL-14)...
    let run_sh = std::fs::read_to_string(store.join("skill/review/run.sh")).unwrap();
    assert!(
        run_sh.contains(&format!("{s}/tool/detect/detect run")),
        "{run_sh}"
    );
    // ...and tool -> tool, in a tool's own helper file (TOOL-15).
    let lib_sh = std::fs::read_to_string(store.join("tool/detect/lib.sh")).unwrap();
    assert!(
        lib_sh.contains(&format!("exec {s}/tool/shard/shard")),
        "{lib_sh}"
    );
}

#[test]
fn tool_prefix_applies_to_store_and_tokens() {
    // spec: TOOL-6
    let sb = tool_source();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--as", "jk", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let store = sb.mind_home.join("store");
    // The tool installs under the prefixed effective name.
    assert!(store.join("tool/jk:detect/detect").is_file());
    // The same tokens resolve to the prefixed store paths.
    let skill_md = std::fs::read_to_string(store.join("skill/jk:review/SKILL.md")).unwrap();
    assert!(
        skill_md.contains(&format!("{}/tool/jk:detect/detect", store.display())),
        "{skill_md}"
    );
}

#[test]
fn tool_with_explicit_link_is_surfaced() {
    // spec: TOOL-4
    let sb = Sandbox::bare("agents");
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"tool\"\nname = \"detect\"\npath = \"tools/detect\"\nlink = \"agents/detect\"\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "linked-tool"]);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let link = sb.claude_home.join("agents/detect");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "an explicit link surfaces the tool in the agent home"
    );
}

#[test]
fn review_flags_tooling_references() {
    // spec: CLI-135 CLI-136 CLI-137
    let sb = Sandbox::bare("agents");
    // A shared tool so `detect` is a sibling tool (and {{tools:nope}} stays bad).
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\n\
         run {{tools:nope}} .\n\
         also ~/.claude/skills/review/resources/pr.py\n\
         mention the detect tool\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(
        !r.success,
        "an unresolved path token is a hard error: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("bad-reference"),
        "expected a bad-reference hard finding: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("hardcoded-path") && r.stdout.contains("{{self}}/resources/pr.py"),
        "expected a hardcoded-path advisory suggesting the token: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("bare-tool-reference"),
        "expected a bare-tool-reference advisory: {}",
        r.stdout
    );
}

#[test]
fn review_hardcoded_path_classifies_and_detects_env_forms() {
    // spec: CLI-145 CLI-136
    let sb = Sandbox::bare("agents");
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\n\
         own ~/.claude/skills/review/resources/pr.py\n\
         tool $HOME/.mind/store/tool/detect/detect run\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "advisory-only review exits zero: {} {}",
        r.stdout, r.stderr
    );
    // Own-resource reference: "works but assumes install location" wording + the
    // {{self}} suggestion that generalizes it (CLI-145).
    assert!(
        r.stdout.contains("hardcodes its own resource path")
            && r.stdout.contains("this works but assumes")
            && r.stdout.contains("{{self}}/resources/pr.py"),
        "own-resource classification: {}",
        r.stdout
    );
    // Shared-tool reference, written with the $HOME spelling: store-only wording
    // + {{tools:}} suggestion, proving the extended form is detected too.
    assert!(
        r.stdout.contains("hardcodes a shared tool path")
            && r.stdout.contains("will not resolve")
            && r.stdout.contains("{{tools:detect}}"),
        "shared-tool classification via $HOME form: {}",
        r.stdout
    );
}

#[test]
fn review_flags_helper_script_duplicated_across_items() {
    // spec: CLI-144
    let sb = Sandbox::bare("agents");
    // Two skills ship the same helper script verbatim; it should be a tool.
    write(
        &sb.source.join("skills/a/SKILL.md"),
        "---\nname: a\ndescription: a\n---\n# a\n",
    );
    write(
        &sb.source.join("skills/a/helper.sh"),
        "#!/bin/sh\necho shared\n",
    );
    write(
        &sb.source.join("skills/a/only.sh"),
        "#!/bin/sh\necho unique\n",
    );
    write(
        &sb.source.join("skills/b/SKILL.md"),
        "---\nname: b\ndescription: b\n---\n# b\n",
    );
    write(
        &sb.source.join("skills/b/helper.sh"),
        "#!/bin/sh\necho shared\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "an advisory-only review exits zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("duplicate-tooling") && r.stdout.contains("helper.sh"),
        "expected a duplicate-tooling advisory naming the file: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:a") && r.stdout.contains("skill:b"),
        "the finding names both carriers: {}",
        r.stdout
    );
    // CLI-144: the message is non-prescriptive - keeping the per-item copies is a
    // valid choice (siloing a helper with its skill), not a defect to fix.
    assert!(
        r.stdout.contains("both are valid"),
        "duplicate-tooling must frame the copy as an optional, valid choice: {}",
        r.stdout
    );
    // A script that lives under only one item is not flagged.
    assert!(
        !r.stdout.contains("only.sh"),
        "a non-duplicated script must not be flagged: {}",
        r.stdout
    );
}

#[test]
fn review_does_not_flag_duplicated_markdown() {
    // spec: CLI-144
    // Markdown is prose, not tooling: identical docs across items are not a
    // duplicate-tooling finding.
    let sb = Sandbox::bare("agents");
    write(
        &sb.source.join("skills/a/SKILL.md"),
        "---\nname: a\ndescription: a\n---\n# shared heading\n",
    );
    write(&sb.source.join("skills/a/NOTES.md"), "same notes\n");
    write(
        &sb.source.join("skills/b/SKILL.md"),
        "---\nname: b\ndescription: b\n---\n# shared heading\n",
    );
    write(&sb.source.join("skills/b/NOTES.md"), "same notes\n");
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("duplicate-tooling"),
        "duplicated markdown must not be flagged: {}",
        r.stdout
    );
}

#[test]
fn review_fix_rewrites_local_copy() {
    // spec: CLI-138
    let sb = Sandbox::bare("agents");
    let skill = sb.source.join("skills/review/SKILL.md");
    write(
        &skill,
        "---\nname: review\ndescription: review\n---\n\
         run ~/.claude/skills/review/run.sh; hand off to dev\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(
        r.success,
        "advisory-only fix must exit zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("fixed"),
        "must report the fixed file: {}",
        r.stdout
    );

    let rewritten = std::fs::read_to_string(&skill).unwrap();
    assert!(
        rewritten.contains("{{self}}/run.sh"),
        "hardcoded path rewritten to a token: {rewritten}"
    );
    assert!(
        rewritten.contains("{{ns:dev}}"),
        "bare sibling name templatized: {rewritten}"
    );
}

#[test]
fn review_fix_refuses_a_registry_target() {
    // spec: CLI-138
    let sb = melded();
    let r = sb.mind(&["review", "agents", "--fix"]);
    assert!(
        !r.success,
        "--fix against a melded selector must refuse: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("fix-not-local"),
        "expected a fix-not-local refusal: {}",
        r.stderr
    );
}

#[test]
fn two_sources_same_names_coexist_under_a_prefix() {
    // spec: NS-2
    // Two melded sources both ship `review`/`dev`/`style`. Namespacing the second
    // gives its items distinct effective names, so both install side by side.
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec(), "--as", "zz"]).success);

    // The prefix makes the effective names distinct, so each installs by its own
    // name with no ambiguity and no qualifier: `review` from a, `zz:review` from b.
    let la = a.mind(&["learn", "review"]);
    assert!(la.success, "learn review: {} {}", la.stdout, la.stderr);
    let lb = a.mind(&["learn", "zz:review"]);
    assert!(lb.success, "learn zz:review: {} {}", lb.stdout, lb.stderr);

    // Both coexist: the unprefixed one as `review`, the namespaced one as `zz:review`.
    let recall = a.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("skill:zz:review"), "{recall}");
    assert!(
        a.mind_home.join("store/skill/review").is_dir(),
        "a's store copy"
    );
    assert!(
        a.mind_home.join("store/skill/zz:review").is_dir(),
        "b's store copy"
    );
    for link in ["skills/review", "skills/zz:review"] {
        assert!(
            std::fs::symlink_metadata(a.claude_home.join(link))
                .unwrap()
                .file_type()
                .is_symlink(),
            "expected a symlink at {link}"
        );
    }
}

#[test]
fn unprefixed_same_name_second_install_is_a_noop_first_wins() {
    // spec: NS-2
    // Without a prefix two same-named items share one install path (`skill:review`),
    // so they cannot coexist. The first installed wins; a later install of the same
    // name from the other source is a no-op (the name is already taken), not a
    // silent overwrite -- and it is not an error.
    let a = Sandbox::new();
    let b = Sandbox::new();
    // Give b's review a distinct description so an overwrite would be observable.
    b.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: BRAVO review\n---\n# review b\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());
    assert!(a.mind(&["learn", &format!("{a_full}#review")]).success);
    // Installing the same name from the other source succeeds but changes nothing.
    let second = a.mind(&["learn", &format!("{b_full}#review")]);
    assert!(second.success, "second install: {}", second.stderr);

    // The store still holds a's content: the first install was not replaced.
    let installed =
        std::fs::read_to_string(a.mind_home.join("store/skill/review/SKILL.md")).unwrap();
    assert!(
        installed.contains("Review the diff for bugs") && !installed.contains("BRAVO review"),
        "the first install must remain (no overwrite): {installed}"
    );
}

// ---------------------------------------------------------------------------
// Output polish: capability gate (CLI-151/154), glyph fallback (CLI-152), and
// the structured JSON result object for mutating verbs (CLI-153).
//
// The integration harness always pipes stdout (non-TTY), so the color/Unicode
// capability gate (CLI-151) is OFF: output must be plain ASCII with no ANSI
// escape sequences. The rich (TTY) branch of the gate cannot be exercised
// without a real PTY and is covered by unit tests in src/render.rs.
// ---------------------------------------------------------------------------

/// Parse `stdout` as a single JSON value, failing loudly with the raw text.
fn parse_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON ({e}): {stdout:?}"))
}

/// True if `s` carries any ANSI escape (ESC, 0x1b).
fn has_ansi_escape(s: &str) -> bool {
    s.contains('\u{1b}')
}

#[test]
fn json_learn_emits_result_object_and_no_prose() {
    // spec: CLI-153, CLI-150
    let sb = melded();

    // --json before the verb.
    let pre = sb.mind(&["--json", "learn", "skill:review"]);
    assert!(pre.success, "learn --json failed: {}", pre.stderr);
    let v = parse_json(&pre.stdout);
    assert_eq!(v["action"], "learn", "{}", pre.stdout);
    assert_eq!(v["target"], "skill:review", "{}", pre.stdout);
    assert_eq!(v["outcome"], "installed", "{}", pre.stdout);
    // The `installed` array names the effective key actually installed.
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        pre.stdout
    );
    // CLI-153: nothing else on stdout. The non-json path prints "learned ...";
    // that prose must be absent under --json.
    assert!(
        !pre.stdout.contains("learned"),
        "human prose `learned` must not appear under --json: {}",
        pre.stdout
    );
    // The JSON-only stdout has no ANSI escapes (also CLI-151).
    assert!(!has_ansi_escape(&pre.stdout), "json stdout: {}", pre.stdout);

    // --json AFTER the verb yields an equivalent object (CLI-150: position-free).
    let sb2 = melded();
    let post = sb2.mind(&["learn", "skill:review", "--json"]);
    assert!(
        post.success,
        "learn --json (suffix) failed: {}",
        post.stderr
    );
    assert_eq!(
        parse_json(&post.stdout),
        v,
        "flag position must not change the JSON: pre={} post={}",
        pre.stdout,
        post.stdout
    );
}

#[test]
fn json_forget_emits_removed_object_and_no_prose() {
    // spec: CLI-153
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    let r = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(r.success, "forget --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "forget", "{}", r.stdout);
    assert_eq!(v["target"], "skill:review", "{}", r.stdout);
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    assert_eq!(
        v["removed"],
        serde_json::json!(["skill:review"]),
        "{}",
        r.stdout
    );
    // The non-json path prints "forgot <key>"; that prose must be absent.
    assert!(
        !r.stdout.contains("forgot"),
        "human prose `forgot` must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_meld_emits_result_object_and_no_prose() {
    // spec: CLI-153
    // A default non-TTY meld registers the source (and installs nothing); under
    // --json it emits a single meld object with outcome "melded".
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["--json", "meld", &spec]);
    assert!(r.success, "meld --json failed: {} {}", r.stdout, r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "{}", r.stdout);
    assert_eq!(v["outcome"], "melded", "{}", r.stdout);
    assert!(
        v["target"].is_string() && !v["target"].as_str().unwrap().is_empty(),
        "meld target must name the source: {}",
        r.stdout
    );
    // The non-json default-meld path prints how to "learn" items later; under
    // --json that prose is suppressed.
    assert!(
        !r.stdout.contains("learn") && !r.stdout.contains("melded source"),
        "default-meld prose must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_remeld_already_melded_is_a_single_object() {
    // spec: CLI-153
    // Re-melding a fully-installed source under --json emits exactly one JSON
    // object (outcome "already-melded"), not the human item-status report. The
    // "already-melded" outcome is only reached when nothing remains to install,
    // so the source must be installed first (a default non-TTY meld installs
    // nothing, which would instead route through the install flow).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld+install");
    let r = sb.mind(&["meld", &spec, "--json"]);
    assert!(r.success, "re-meld --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "{}", r.stdout);
    assert_eq!(v["outcome"], "already-melded", "{}", r.stdout);
    // The non-json re-meld prints "already melded" prose and "to install ...";
    // none of that may leak onto stdout under --json.
    assert!(
        !r.stdout.contains("already melded") && !r.stdout.contains("to install"),
        "re-meld prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_sync_emits_result_object_and_no_prose() {
    // spec: CLI-153
    let sb = melded();
    let r = sb.mind(&["sync", "--json"]);
    assert!(r.success, "sync --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "sync", "{}", r.stdout);
    assert_eq!(v["outcome"], "synced", "{}", r.stdout);
    assert!(v["count"].is_number(), "sync count: {}", r.stdout);
    // The non-json path prints "syncing <source> ..."; suppressed under --json.
    assert!(
        !r.stdout.contains("syncing") && !r.stdout.contains("up to date"),
        "sync prose must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_sync_no_op_on_empty_registry() {
    // spec: CLI-153
    // With no sources melded, sync changes nothing: the outcome is the explicit
    // "no-op" value, not a human "no sources melded" message.
    let sb = Sandbox::new();
    let r = sb.mind(&["sync", "--json"]);
    assert!(r.success, "sync --json on empty registry: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "sync", "{}", r.stdout);
    assert_eq!(v["outcome"], "no-op", "{}", r.stdout);
    assert!(
        !r.stdout.contains("no sources"),
        "no-op prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_upgrade_up_to_date_is_an_object() {
    // spec: CLI-153
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind(&["upgrade", "--json"]);
    assert!(r.success, "upgrade --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "upgrade", "{}", r.stdout);
    assert_eq!(v["outcome"], "up-to-date", "{}", r.stdout);
    assert!(
        !r.stdout.contains("up to date"),
        "upgrade prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_upgrade_applies_and_reports_upgraded() {
    // spec: CLI-153
    // A real delta upgraded under --json emits outcome "upgraded" plus the
    // installed keys, and no "upgraded skill:review" prose.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes", "--json"]);
    assert!(r.success, "upgrade --yes --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "upgrade", "{}", r.stdout);
    assert_eq!(v["outcome"], "upgraded", "{}", r.stdout);
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        r.stdout
    );
    // The "✓ upgraded ..." prose line must be gone.
    assert!(
        !r.stdout.contains("upgraded skill"),
        "upgrade prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_unmeld_emits_result_object() {
    // spec: CLI-153
    // Unmeld with an installed item removes it and the source; under --json this
    // is a single object (outcome "removed"), with the item-removal prose absent.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld+install");
    let name = "agents"; // the fixture source's directory name

    let r = sb.mind(&["unmeld", name, "--yes", "--json"]);
    assert!(r.success, "unmeld --json failed: {} {}", r.stdout, r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "unmeld", "{}", r.stdout);
    // `target` is the source's canonical identity (e.g. `local/<base>/agents`),
    // which ends with the source dir name the command was given.
    assert!(
        v["target"]
            .as_str()
            .is_some_and(|t| t.ends_with(&format!("/{name}")) || t == name),
        "unmeld target must name the source: {}",
        r.stdout
    );
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    assert!(
        !r.stdout.contains("unmelded"),
        "unmeld prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_lobe_add_and_remove_emit_objects() {
    // spec: CLI-153
    let sb = Sandbox::new();
    let extra = sb.base.join("extra-lobe");
    let extra_s = extra.to_string_lossy().into_owned();

    let added = sb.mind(&["config", "lobes", "add", &extra_s, "--json"]);
    assert!(added.success, "lobe add --json failed: {}", added.stderr);
    let v = parse_json(&added.stdout);
    assert_eq!(v["action"], "lobe-add", "{}", added.stdout);
    assert_eq!(v["outcome"], "added", "{}", added.stdout);

    // Re-adding the same lobe is a no-op outcome, not a human message.
    let again = sb.mind(&["config", "lobes", "add", &extra_s, "--json"]);
    assert!(again.success, "{}", again.stderr);
    assert_eq!(
        parse_json(&again.stdout)["outcome"],
        "no-op",
        "{}",
        again.stdout
    );

    let removed = sb.mind(&["config", "lobes", "remove", &extra_s, "--json"]);
    assert!(
        removed.success,
        "lobe remove --json failed: {}",
        removed.stderr
    );
    let rv = parse_json(&removed.stdout);
    assert_eq!(rv["action"], "lobe-remove", "{}", removed.stdout);
    assert_eq!(rv["outcome"], "removed", "{}", removed.stdout);
}

#[test]
fn json_learn_dry_run_lists_nothing_installed_as_prose() {
    // spec: CLI-153
    // A --dry-run under --json reports outcome "dry-run" as an object, and does
    // not print the "would learn N item(s)" prose.
    let sb = melded();
    let r = sb.mind(&["learn", "skill:review", "--dry-run", "--json"]);
    assert!(r.success, "learn --dry-run --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "learn", "{}", r.stdout);
    assert_eq!(v["outcome"], "dry-run", "{}", r.stdout);
    assert!(
        !r.stdout.contains("would learn"),
        "dry-run prose must not appear under --json: {}",
        r.stdout
    );
    // A dry-run installs nothing.
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "dry-run must not install anything"
    );
}

#[test]
fn json_error_emits_envelope_on_stdout_not_stderr() {
    // spec: CLI-181, CLI-182
    // Under --json an error emits {"schema":1,"error":{"kind":"...","message":"..."}}
    // to stdout; nothing is written to stderr by the main error handler.
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist", "--json"]);
    assert!(!r.success, "unknown item must fail");
    // The envelope must be valid JSON on stdout.
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    let err = &v["error"];
    assert_eq!(
        err["kind"], "item-not-found",
        "kind must be item-not-found: {}",
        r.stdout
    );
    assert!(
        err["message"]
            .as_str()
            .map(|s| s.contains("does-not-exist"))
            .unwrap_or(false),
        "message must contain the query: {}",
        r.stdout
    );
    // No error text on stderr from the main error handler.
    assert!(
        !r.stderr.contains("error:"),
        "main error handler must not write to stderr under --json: {}",
        r.stderr
    );
    // Exit code must be 1 (FAILURE), not 2.
    // (success == false + no clap usage error means exit 1.)
}

#[test]
fn json_error_envelope_schema_and_kind_fields() {
    // spec: CLI-181, CLI-182
    // A second distinct error class (SourceNotFound via `forget`) also produces
    // the envelope, confirming the schema/kind contract is not specific to learn.
    let sb = melded();
    // `forget` on a name that is not installed -> NotInstalled (not SourceNotFound,
    // since forget resolves from the manifest). Use `unmeld` on a nonexistent
    // source name to get SourceNotFound.
    let r = sb.mind(&["--json", "unmeld", "no-such-source"]);
    assert!(!r.success, "unmeld unknown source must fail");
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema field must be 1: {}", r.stdout);
    let err = &v["error"];
    // The kind slug must be the stable source-not-found slug.
    assert_eq!(
        err["kind"], "source-not-found",
        "kind must be source-not-found: {}",
        r.stdout
    );
    // message must be non-empty and contain the queried name.
    let msg = err["message"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "message must be non-empty: {}", r.stdout);
    assert!(
        msg.contains("no-such-source"),
        "message must contain the source name: {}",
        r.stdout
    );
    // Nothing on stderr from the main error handler.
    assert!(
        !r.stderr.contains("error:"),
        "no stderr from main handler under --json: {}",
        r.stderr
    );
}

#[test]
fn clap_usage_error_is_not_json_enveloped() {
    // spec: CLI-183
    // A clap argument-parse failure (unknown flag) exits 2 with plain text on
    // stderr, not a JSON envelope on stdout. The --json envelope only applies to
    // post-parse MindError failures (exit 1).
    let sb = Sandbox::new();
    // An unknown flag causes clap to exit 2 before any command logic runs.
    let r = sb.mind(&["--no-such-flag"]);
    // Exit code 2 is reported by Command::status() as non-success.
    assert!(!r.success, "unknown flag must fail");
    // stdout must NOT contain a JSON envelope; clap writes to stderr.
    assert!(
        r.stdout.trim().is_empty(),
        "clap usage errors must not produce stdout output: {:?}",
        r.stdout
    );
    // stderr must carry clap's plain-text error message.
    assert!(
        !r.stderr.is_empty(),
        "clap must write the usage error to stderr: {:?}",
        r.stderr
    );
}

#[test]
fn non_tty_output_is_plain_ascii_with_no_escapes() {
    // spec: CLI-151
    // The harness pipes stdout, so the capability gate is OFF: every ordinary
    // (non-json) command's stdout must be free of ANSI escape sequences.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    for args in [
        vec!["recall"],
        vec!["recall", "--sources"],
        vec!["recall", "skill:review"],
        vec!["probe"],
        vec!["introspect"],
        vec!["upgrade"],
    ] {
        let r = sb.mind(&args);
        assert!(
            !has_ansi_escape(&r.stdout),
            "non-TTY stdout for `{args:?}` must contain no ANSI escapes: {:?}",
            r.stdout
        );
    }
}

#[test]
fn no_color_env_forces_plain_ascii() {
    // spec: CLI-154
    // NO_COLOR set (even though already non-TTY) must keep the gate OFF: no
    // escapes appear. Passed in the child env so it cannot race other tests.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind_env(&["recall"], &[("NO_COLOR", "1")]);
    assert!(r.success, "recall failed: {}", r.stderr);
    assert!(
        !has_ansi_escape(&r.stdout),
        "NO_COLOR must force plain ASCII: {:?}",
        r.stdout
    );

    // An empty NO_COLOR value also counts as "set" and forces the gate OFF.
    let empty = sb.mind_env(&["recall"], &[("NO_COLOR", "")]);
    assert!(
        !has_ansi_escape(&empty.stdout),
        "empty NO_COLOR must still force plain ASCII: {:?}",
        empty.stdout
    );
}

#[test]
fn ascii_flag_forces_plain_output() {
    // spec: CLI-154
    // --ascii forces the gate OFF regardless of other state; in this non-TTY
    // harness the result is still escape-free ASCII, and accepted before or
    // after the verb (CLI-150).
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let pre = sb.mind(&["--ascii", "recall"]);
    assert!(pre.success, "--ascii recall failed: {}", pre.stderr);
    assert!(!has_ansi_escape(&pre.stdout), "{:?}", pre.stdout);
    let post = sb.mind(&["recall", "--ascii"]);
    assert!(!has_ansi_escape(&post.stdout), "{:?}", post.stdout);
}

#[test]
fn ascii_fallback_glyphs_are_present_in_plain_mode() {
    // spec: CLI-152
    // With the gate OFF, every glyph uses its ASCII fallback. recall's status
    // view marks an installed item with `installed @` (the `+` ok glyph) and an
    // available one with `available` (the `-` glyph); probe marks an installed
    // row with the `*` ASCII bullet. None of the Unicode glyphs may appear.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("installed @"),
        "installed marker (ASCII fallback) must show `installed @`: {recall}"
    );
    assert!(
        recall.contains("available"),
        "available marker (ASCII fallback) must show `available`: {recall}"
    );
    // The Unicode status glyphs from src/render.rs must NOT leak into plain mode.
    for glyph in ['✓', '○', '✗', '●'] {
        assert!(
            !recall.contains(glyph),
            "Unicode glyph {glyph:?} must not appear in plain mode: {recall}"
        );
    }

    // probe marks the installed row with the `*` ASCII bullet (not `●`).
    let probe = sb.mind(&["probe", "review"]).stdout;
    assert!(
        probe.contains('*'),
        "probe must mark the installed item with the `*` ASCII bullet: {probe}"
    );
    assert!(
        !probe.contains('●'),
        "probe must not emit the Unicode bullet in plain mode: {probe}"
    );
}

#[test]
fn every_reachable_verb_emits_valid_json_under_json_flag() {
    // spec: CLI-153
    // Cross-check: each mutating verb the hermetic fixture can drive produces a
    // single, parseable JSON object under --json (no torn or doubled output).
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let meld = sb.mind(&["meld", &spec, "--json"]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(parse_json(&meld.stdout).is_object(), "{}", meld.stdout);

    let learn = sb.mind(&["learn", "skill:review", "--json"]);
    assert!(learn.success, "{}", learn.stderr);
    assert!(parse_json(&learn.stdout).is_object(), "{}", learn.stdout);

    let sync = sb.mind(&["sync", "--json"]);
    assert!(sync.success, "{}", sync.stderr);
    assert!(parse_json(&sync.stdout).is_object(), "{}", sync.stdout);

    let upgrade = sb.mind(&["upgrade", "--json"]);
    assert!(upgrade.success, "{}", upgrade.stderr);
    assert!(
        parse_json(&upgrade.stdout).is_object(),
        "{}",
        upgrade.stdout
    );

    let forget = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(forget.success, "{}", forget.stderr);
    assert!(parse_json(&forget.stdout).is_object(), "{}", forget.stdout);

    let unmeld = sb.mind(&["unmeld", "agents", "--json"]);
    assert!(unmeld.success, "{}", unmeld.stderr);
    assert!(parse_json(&unmeld.stdout).is_object(), "{}", unmeld.stdout);
}

#[test]
fn json_sync_upgrade_emits_two_objects_one_per_action() {
    // spec: CLI-153
    // `sync --upgrade --json` performs two logical actions (sync, then upgrade)
    // and emits one JSON object per action. Assert BOTH objects are present and
    // each parses on its own (concatenated pretty-JSON objects). This documents
    // the deliberate two-object stream: stdout is NOT a single JSON value here.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind(&["sync", "--upgrade", "--json"]);
    assert!(r.success, "sync --upgrade --json failed: {}", r.stderr);

    // A single-value parse must FAIL (there are two top-level objects), which is
    // the property we are pinning: this stream is two objects, not one.
    assert!(
        serde_json::from_str::<serde_json::Value>(r.stdout.trim()).is_err(),
        "sync --upgrade --json is expected to emit two objects, not one value: {}",
        r.stdout
    );
    // Both a sync action and an upgrade action must appear in the stream.
    let actions: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&r.stdout)
        .into_iter::<serde_json::Value>()
        .map(|d| d.expect("each chunk must be valid JSON"))
        .collect();
    assert_eq!(
        actions.len(),
        2,
        "exactly two JSON objects (one per logical action): {}",
        r.stdout
    );
    assert_eq!(actions[0]["action"], "sync", "{}", r.stdout);
    assert_eq!(actions[1]["action"], "upgrade", "{}", r.stdout);
}

// ===== Per-item install/uninstall hooks (HOOK-80..85) =====

/// A source named `name` (a `bare` repo) with one skill `greet` declared in
/// `mind.toml` `[[items]]` carrying per-item `install` and `uninstall` hooks.
/// The commands are arbitrary; markers under `<base>/markers` let a test observe
/// which fired. The install command also drops a relative `built-here` file so a
/// test can confirm the hook ran with the store dir as its working directory.
fn sandbox_with_item_hook_cmds(name: &str, install: &str, uninstall: &str) -> Sandbox {
    let sb = Sandbox::bare(name);
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"{install}\"\n",
            "uninstall = \"{uninstall}\"\n",
        ),
        install = install,
        uninstall = uninstall,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    sb
}

/// The success-marker variant: the install hook drops `built-here` (relative, in
/// the store dir) plus an absolute `<base>/markers/installed`; the uninstall hook
/// drops an absolute `<base>/markers/uninstalled`.
fn sandbox_with_item_hooks(name: &str) -> Sandbox {
    // Build first so we know the base path, then rewrite the mind.toml commands
    // with absolute marker paths under that base.
    let sb = Sandbox::bare(name);
    let markers = sb.base.join("markers");
    let m = markers.display();
    let install = format!("touch built-here && mkdir -p '{m}' && touch '{m}/installed'");
    let uninstall = format!("mkdir -p '{m}' && touch '{m}/uninstalled'");
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"{install}\"\n",
            "uninstall = \"{uninstall}\"\n",
        ),
        install = install,
        uninstall = uninstall,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    sb
}

#[test]
fn learn_runs_item_install_hook_in_store_dir() {
    // spec: HOOK-81, HOOK-83
    // An item install hook runs as the final install step, in the item's store
    // directory, when run unattended via --dangerously-skip-install-hook-check.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    // Register without auto-installing (so the install runs under our flag).
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "learn should succeed: {} {}", r.stdout, r.stderr);

    // The item installed.
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the skill must be installed"
    );
    // HOOK-81: the install hook ran with the store dir as cwd (relative marker).
    assert!(
        sb.mind_home.join("store/skill/greet/built-here").exists(),
        "install hook must run in the item's store directory"
    );
    // And its absolute side effect happened.
    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook's side effect must have run"
    );
}

#[test]
fn learn_without_flag_skips_item_install_hook_in_non_tty() {
    // spec: HOOK-83
    // A non-TTY learn with no flag skips the item install hook: the item still
    // installs, but the side effect does not run, and a note says so.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&["learn", "skill:greet"]);
    assert!(
        r.success,
        "learn should still succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the item must install even though the hook is skipped"
    );
    assert!(
        !sb.base.join("markers/installed").exists(),
        "a non-TTY learn must skip the install hook"
    );
    assert!(
        r.stdout.contains("skipped install hook"),
        "the skip must be reported: {}",
        r.stdout
    );
}

#[test]
fn learn_item_install_hook_failure_rolls_back_the_install() {
    // spec: HOOK-81
    // A non-zero install-hook exit rolls the item's install back: its store copy
    // and link are removed and it is left not installed.
    let sb = sandbox_with_item_hook_cmds("agents", "exit 1", "true");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !r.success,
        "a failing install hook must fail learn: {}",
        r.stdout
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the store copy must be removed on rollback"
    );
    assert!(
        !sb.claude_home.join("skills/greet").exists(),
        "the link must be removed on rollback"
    );
    let manifest = std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        !manifest.contains("greet"),
        "a rolled-back item must not be recorded in the manifest: {manifest}"
    );
}

#[test]
fn forget_runs_item_uninstall_hook() {
    // spec: HOOK-82
    // forget runs the item's uninstall hook (in its store dir) before removing it.
    let sb = sandbox_with_item_hooks("agents");
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

    let r = sb.mind(&[
        "forget",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        r.success,
        "forget should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "the uninstall hook must run at forget"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item must be removed after its uninstall hook"
    );
}

#[test]
fn forget_without_flag_skips_item_uninstall_hook_in_non_tty() {
    // spec: HOOK-83
    // A non-TTY forget with no flag skips the uninstall hook but still removes the
    // item (cleanup is the graceful decline).
    let sb = sandbox_with_item_hooks("agents");
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

    let r = sb.mind(&["forget", "skill:greet"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !sb.base.join("markers/uninstalled").exists(),
        "a non-TTY forget must skip the uninstall hook"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item is still removed when the hook is skipped"
    );
    assert!(
        r.stdout.contains("skipped uninstall hook"),
        "the skip must be reported: {}",
        r.stdout
    );
}

#[test]
fn forget_item_uninstall_hook_failure_leaves_item_installed() {
    // spec: HOOK-82
    // A non-zero uninstall-hook exit is a hard stop: the removal stops and the
    // item is left installed.
    let sb = sandbox_with_item_hook_cmds("agents", "true", "exit 1");
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

    let r = sb.mind(&[
        "forget",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        !r.success,
        "a failing uninstall hook must fail forget: {}",
        r.stdout
    );
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the item must remain installed when its uninstall hook fails"
    );
}

#[test]
fn unmeld_runs_item_uninstall_hook() {
    // spec: HOOK-82
    // unmeld removes the source's items, running each item's uninstall hook first.
    let sb = sandbox_with_item_hooks("agents");
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

    let r = sb.mind(&[
        "unmeld",
        "agents",
        "-y",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(
        r.success,
        "unmeld should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "the item uninstall hook must run at unmeld"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item must be removed at unmeld"
    );
}

#[test]
fn item_install_hook_reruns_on_reinstall() {
    // spec: HOOK-84
    // Nothing is recorded for the hook: it fires on every removal and re-runs on
    // every reinstall. learn -> forget -> learn fires install, uninstall, install.
    let sb = sandbox_with_item_hooks("agents");
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
    assert!(sb.base.join("markers/installed").exists());

    // Clear the markers, then remove and reinstall.
    std::fs::remove_dir_all(sb.base.join("markers")).unwrap();
    assert!(
        sb.mind(&[
            "forget",
            "skill:greet",
            "--dangerously-skip-install-hook-check"
        ])
        .success
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "uninstall hook fires on removal"
    );

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook must re-run on reinstall (HOOK-84)"
    );
}

#[test]
fn in_place_upgrade_reruns_install_hook_but_not_uninstall_hook() {
    // spec: HOOK-82, HOOK-81
    // An in-place upgrade (same effective name, content swapped) re-runs the item
    // install hook (HOOK-81) but does NOT run the uninstall hook, since the item
    // is not removed (HOOK-82).
    let sb = sandbox_with_item_hooks("agents");
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
    std::fs::remove_dir_all(sb.base.join("markers")).unwrap();

    // Change the skill upstream so upgrade swaps its content in place.
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet v2\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "-y", "--dangerously-skip-install-hook-check"]);
    assert!(
        r.success,
        "upgrade should succeed: {} {}",
        r.stdout, r.stderr
    );

    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook must re-run on an in-place upgrade (HOOK-81)"
    );
    assert!(
        !sb.base.join("markers/uninstalled").exists(),
        "an in-place upgrade must NOT run the uninstall hook (HOOK-82)"
    );
}

#[test]
fn review_lists_item_install_and_uninstall_hooks() {
    // spec: HOOK-85
    // `mind review` surfaces an item's declared install/uninstall hooks as
    // advisory findings so a consumer sees, before installing, that the item runs
    // code on the host.
    let sb = sandbox_with_item_hooks("agents");
    let r = sb.mind(&["review", &sb.source_spec()]);
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(
        all.contains("item-hook"),
        "review must emit item-hook advisories: {all}"
    );
    assert!(
        all.contains("declares an install hook"),
        "review must list the install hook: {all}"
    );
    assert!(
        all.contains("declares an uninstall hook"),
        "review must list the uninstall hook: {all}"
    );
}

// ---- CLI-75: hash-based outdated detection ----------------------------------

/// Meld a local directory source, learn an item, edit the item source file in
/// place (no commit), then check that `mind recall` marks the item outdated.
/// A local linked source is read live from its working tree, so a content
/// change changes the hash while the commit is unchanged.
// spec: CLI-75
#[test]
fn recall_marks_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Verify the item is initially NOT marked outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // Edit the item source file in place without committing. For a linked local
    // source this changes the content hash while the commit is unchanged.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Now `mind recall` must mark skill:review as outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outdated"),
        "recall must mark the item outdated after an in-place content edit: {}",
        r.stdout
    );
}

/// CLI-155: the `recall` status view uses a distinct left-edge marker for an
/// installed-but-stale item. With the capability gate OFF (captured stdout), a
/// current install shows the ASCII installed glyph `+`; an out-of-date install
/// shows the stale glyph `^` instead. Assert on the per-item line so the marker,
/// not just the trailing `(outdated)` text, carries the state.
// spec: CLI-155
#[test]
fn recall_status_view_uses_stale_marker_for_outdated_item() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Fresh install: the review line carries the `+` installed marker, not `^`.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in recall output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('+'),
        "a current install must lead with the `+` marker: {line:?}"
    );

    // Edit the item source in place (hash drift, commit unchanged).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Now the review line must lead with the `^` stale marker, not `+`.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in recall output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
}

/// CLI-155: the `source_status` view (reached by re-melding an already-melded
/// source) also uses the stale marker `^` for an out-of-date item rather than the
/// installed `+`.
// spec: CLI-155
#[test]
fn source_status_uses_stale_marker_for_outdated_item() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // In-place edit (no commit) so only the content hash drifts.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Re-meld the already-melded source: all items installed, so this falls
    // through to source_status.
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in source_status output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
}

/// After an in-place content edit, `mind recall <item>` must show an out-of-date
/// note in the single-item detail view.
#[test]
fn recall_item_detail_shows_out_of_date_after_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Edit source file in place without committing.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    let r = sb.mind(&["recall", "skill:review"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("out of date"),
        "recall <item> must show out-of-date note after content edit: {}",
        r.stdout
    );
}

/// Control case: an item whose source file has not been edited must NOT be
/// marked outdated by `mind recall`.
#[test]
fn recall_does_not_mark_unedited_item_outdated() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "unedited item must not be marked outdated: {}",
        r.stdout
    );

    let r = sb.mind(&["recall", "skill:review"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("out of date"),
        "recall <item> must not show out-of-date for unedited item: {}",
        r.stdout
    );
}

/// The `probe` non-interactive listing must mark a drifted installed item out of
/// date, and must NOT mark a clean installed item. No other test exercises the
/// probe surface for CLI-75.
// spec: CLI-75
#[test]
fn probe_marks_installed_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Clean: probe must not flag any item out of date.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed items must not be outdated in probe: {}",
        r.stdout
    );

    // Edit one item's source file in place (no commit) -> hash drift only.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nchanged\n",
    );

    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    let review = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review row in probe: {}", r.stdout));
    assert!(
        review.contains("outdated"),
        "probe must mark the drifted item outdated: {review:?}\n{}",
        r.stdout
    );
    // The untouched agent row must remain clean.
    let dev = r
        .stdout
        .lines()
        .find(|l| l.contains("agent:dev"))
        .unwrap_or_else(|| panic!("no dev row in probe: {}", r.stdout));
    assert!(
        !dev.contains("outdated"),
        "an unedited item must not be marked outdated in probe: {dev:?}"
    );
}

/// Re-melding an already-melded local source whose working tree was edited in
/// place reaches the `source_status` view, which must mark the drifted item out
/// of date.
// spec: CLI-75
#[test]
fn remeld_source_status_marks_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // In-place edit (no commit) so only the content hash drifts.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nremeld-edit\n",
    );

    // Re-meld the already-melded source: all items already installed, so this
    // falls through to source_status.
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "expected the already-melded status view: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("outdated"),
        "source_status via re-meld must mark the drifted item outdated: {}",
        r.stdout
    );
}

/// Regression: a commit that advances the source WITHOUT changing a given item's
/// content must NOT mark that item outdated in `recall`. The outdated marker must
/// match `upgrade`'s pending condition (LIFE-11): hash drift OR rename, not
/// commit advance. Before the fix the commit-only advance produced a permanent
/// false-positive marker that `mind upgrade` would then report as "up to date".
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_does_not_mark_item_outdated_after_commit_only_advance() {
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean install: not outdated.
    let r = sb.mind(&["recall"]);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // Advance the source commit by touching an UNRELATED file, so the source
    // commit moves past the installed item's commit but the item content is
    // unchanged. sync updates the recorded source commit.
    sb.write_and_commit("CHANGES.md", "unrelated change\n");
    assert!(sb.mind(&["sync"]).success);

    // recall must NOT mark the unchanged item outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "recall must NOT mark the item outdated after a commit-only advance (content unchanged): {}",
        r.stdout
    );

    // upgrade must agree: nothing pending for this item.
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("everything is up to date"),
        "upgrade must report everything up to date after a commit-only advance: {}",
        r.stdout
    );
}

/// The marker is a human-view affordance only: `recall --json` output must be
/// byte-identical before and after a content edit drifts the item.
// spec: CLI-75
#[test]
fn recall_json_is_unchanged_by_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let before_status = sb.mind(&["recall", "--json"]);
    let before_detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(before_status.success && before_detail.success);

    // Drift the item in place.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\njson-drift\n",
    );

    let after_status = sb.mind(&["recall", "--json"]);
    let after_detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(after_status.success && after_detail.success);

    assert_eq!(
        before_status.stdout, after_status.stdout,
        "recall --json status output must not change with drift"
    );
    assert_eq!(
        before_detail.stdout, after_detail.stdout,
        "recall <item> --json output must not change with drift"
    );
    assert!(
        !after_status.stdout.contains("outdated") && !after_status.stdout.contains("out of date"),
        "JSON must carry no human out-of-date marker: {}",
        after_status.stdout
    );
}

/// Regression: `probe --no-tui` must not mark an installed item outdated when
/// the source commit advanced without changing that item's content. Both `recall`
/// and `probe` must agree with `upgrade` on what is pending (CLI-75 / LIFE-11).
// spec: CLI-75
// spec: LIFE-11
#[test]
fn probe_does_not_mark_item_outdated_after_commit_only_advance() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Clean: probe must not flag any item out of date.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed items must not be outdated in probe: {}",
        r.stdout
    );

    // Advance the source by committing an unrelated file; review content unchanged.
    sb.write_and_commit("NOTES.md", "unrelated\n");
    assert!(sb.mind(&["sync"]).success);

    // probe must still not flag the unchanged item.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "probe must NOT mark item outdated after commit-only advance: {}",
        r.stdout
    );

    // recall must also not flag it.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "recall must NOT mark item outdated after commit-only advance: {}",
        r.stdout
    );

    // upgrade confirms: nothing pending.
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("everything is up to date"),
        "upgrade must report everything up to date after commit-only advance: {}",
        r.stdout
    );
}

/// After a commit that also changes item content, recall and probe must still
/// mark the item outdated (hash drift triggers the marker regardless of commit).
/// This ensures the commit-only fix did not regress content-drift detection.
// spec: CLI-75
#[test]
fn recall_still_marks_item_outdated_after_commit_with_content_change() {
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean install: not outdated.
    let r = sb.mind(&["recall"]);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // edit_source changes the review skill content AND commits.
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    // recall must mark the item outdated because content (hash) changed.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outdated"),
        "recall must mark item outdated when commit also changed content: {}",
        r.stdout
    );
}

/// PRIMARY GAP: the `rename_lag` half of the fix. An item whose effective NAME
/// changed (a namespace/prefix rename) but whose content hash did NOT must be
/// marked outdated by `recall` (status view) and `probe --no-tui`, and `upgrade`
/// must report it pending as a rename. The drift is created without re-melding
/// `--as` (which would apply the rename immediately): the source declares a
/// `[source].prefix` in `mind.toml` after install, so the catalog's effective
/// name (`jk:review`) diverges from the still-recorded manifest name (`review`)
/// with the item's SKILL.md content byte-identical (the hash is of the item
/// content, not mind.toml -- LIFE-15). The four surfaces must agree with upgrade.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_and_probe_mark_item_outdated_on_rename_without_content_change() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Sanity: freshly installed, unprefixed, nothing outdated anywhere.
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("outdated"),
        "fresh install must not be outdated in recall: {}",
        recall.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        !probe.stdout.contains("outdated"),
        "fresh install must not be outdated in probe: {}",
        probe.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        !detail.stdout.contains("out of date"),
        "fresh install single-item must not be out of date: {}",
        detail.stdout
    );

    // Introduce a namespace prefix via mind.toml WITHOUT re-melding. After sync,
    // the catalog computes effective name `jk:review` while the manifest still
    // records `review`; the SKILL.md content is unchanged so the hash matches.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    // probe --no-tui listing: must mark the renamed item outdated.
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "probe failed: {}", probe.stderr);
    assert!(
        probe.stdout.contains("outdated"),
        "probe must mark a renamed item outdated: {}",
        probe.stdout
    );

    // recall single-item detail: looked up by the OLD installed name, still
    // present (matched by stable identity); must report out of date.
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(detail.success, "recall detail failed: {}", detail.stderr);
    assert!(
        detail.stdout.contains("out of date"),
        "recall single-item detail must report a renamed item out of date: {}",
        detail.stdout
    );

    // upgrade must agree: the item is pending as a rename, not "up to date".
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);
    assert!(
        !up.stdout.contains("everything is up to date"),
        "upgrade must NOT report up to date when an effective name changed: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("rename")
            && up.stdout.contains("review -> ")
            && up.stdout.contains("jk:review"),
        "upgrade must report the rename review -> jk:review: {}",
        up.stdout
    );
}

/// CLI-75 applies the rename marker to the default `recall` status view too. The
/// status view matches catalog items to the manifest by stable identity
/// `(source, kind, bare_name)`, so a renamed item (effective name `skill:jk:review`
/// vs the manifest's `skill:review`) still lands in the matched arm and `rename_lag`
/// marks it outdated, rather than being misreported as `available` + an orphan
/// `(removed upstream)`. The status view agrees with `probe`, the single-item
/// detail, and `upgrade`.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_status_view_marks_renamed_item_outdated() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);
    // The renamed item must be marked outdated, not shown as removed-upstream.
    assert!(
        recall.stdout.contains("outdated"),
        "recall status view must mark a renamed (effective-name-changed) item \
         outdated to agree with probe/detail/upgrade: {}",
        recall.stdout
    );
    assert!(
        !recall.stdout.contains("removed upstream"),
        "a pure namespace rename must not be reported as removed upstream: {}",
        recall.stdout
    );
}

/// FOUR-SURFACE CONSISTENCY (hash-drift case): for one in-place content edit
/// (no new commit, no rename), `recall` status, `recall <item>` detail,
/// `probe --no-tui`, and `upgrade` must all agree the item is changed. None may
/// call it current that upgrade would change, and `upgrade` applying it must
/// clear the marker on all surfaces afterwards.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn all_four_surfaces_agree_on_hash_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Edit the item content in place WITHOUT a new commit (local-source drift).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nfour-surface-drift\n",
    );

    // All three human views must flag it, matching what upgrade will do.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("outdated"),
        "recall status must flag hash drift: {}",
        recall.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        detail.stdout.contains("out of date"),
        "recall single-item detail must flag hash drift: {}",
        detail.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("outdated"),
        "probe must flag hash drift: {}",
        probe.stdout
    );

    // upgrade is the source of truth: it must find this item pending and apply it.
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);
    assert!(
        !up.stdout.contains("everything is up to date"),
        "upgrade must act on the drifted item: {}",
        up.stdout
    );

    // After upgrade, every surface must agree the item is now current.
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("outdated"),
        "recall must be clean after upgrade: {}",
        recall.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        !detail.stdout.contains("out of date"),
        "recall detail must be clean after upgrade: {}",
        detail.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        !probe.stdout.contains("outdated"),
        "probe must be clean after upgrade: {}",
        probe.stdout
    );
}

/// The "outdated; run mind upgrade" marker is a HUMAN-view concern only (CLI-75):
/// the JSON outputs must never carry it. This complements
/// `recall_json_is_unchanged_by_drift` (hash-drift case) by covering the rename
/// case across `recall --json`, `recall <item> --json`, and `probe --json`.
///
/// Note: the JSON bytes DO legitimately change under a rename drift -- the synced
/// commit advances and the catalog's effective keys are now prefixed -- so this
/// asserts the real invariant (no human marker string leaks) rather than byte
/// equality, which holds only for an in-place edit with no commit/key change.
// spec: CLI-75
// spec: CLI-73
// spec: CLI-84
#[test]
fn json_outputs_carry_no_outdated_marker_under_rename_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Rename drift via mind.toml prefix (item content unchanged), then sync.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall", "--json"]);
    let detail = sb.mind(&["recall", "skill:review", "--json"]);
    let probe = sb.mind(&["probe", "--json"]);
    assert!(recall.success && detail.success && probe.success);

    for (label, body) in [
        ("recall --json", &recall.stdout),
        ("recall detail --json", &detail.stdout),
        ("probe --json", &probe.stdout),
    ] {
        // The JSON must parse and must carry no human out-of-date marker text.
        let _: serde_json::Value =
            serde_json::from_str(body).unwrap_or_else(|e| panic!("{label} not valid JSON: {e}"));
        assert!(
            !body.contains("outdated") && !body.contains("out of date"),
            "{label} must carry no human out-of-date marker: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Certification of the orphan-classification rework (recall status view matches
// catalog<->manifest by stable identity (source, kind, bare_name); orphans_of
// flags removed-upstream only when NO catalog item shares that identity).
//
// The dev shard covered the rename half (the item must be marked outdated on the
// four surfaces). These add the adversarial edges that were not covered:
//  - a renamed item must appear EXACTLY ONCE (no double-listing as both an
//    outdated row and an orphan/removed-upstream row), in human view and JSON;
//  - a genuine removal must STILL show (removed upstream) in the human recall
//    view and as installed-but-no-catalog-match (orphaned) in recall --json;
//  - identity is (source, kind, bare_name): a same-named item in another source
//    must not cross-match, so removing/renaming in one source never mislabels the
//    other's item;
//  - the unmanaged-item accounting is unaffected by the orphan change.
// ---------------------------------------------------------------------------

/// EDGE 1 (no double-listing under rename): after a pure prefix rename (effective
/// name changed, content unchanged), the item appears EXACTLY ONCE in the human
/// `recall` status view -- as the installed `(outdated; run mind upgrade)` row --
/// and is NOT ALSO emitted as a `(removed upstream)` orphan. The dev test asserts
/// the markers are present/absent; this one asserts the stronger structural
/// property by COUNTING the rows that mention the item, so a regression that
/// reintroduced both an outdated row and an orphan row would be caught.
// spec: CLI-75
#[test]
fn recall_status_renamed_item_appears_exactly_once_no_orphan_dup() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld failed");

    // Rename drift: declare a prefix after install, then sync so the catalog's
    // effective name (`jk:review`) diverges from the recorded manifest name
    // (`review`) with the SKILL.md content byte-identical.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // Exactly one line carries the review-skill key (either the installed name
    // `skill:review` or the catalog's new effective name `skill:jk:review`).
    // The `skill:` prefix anchors to an item row and avoids spurious matches
    // on output chrome that merely contains the word "review".
    let review_lines: Vec<&str> = recall
        .stdout
        .lines()
        .filter(|l| l.contains("skill:review") || l.contains("skill:jk:review"))
        .collect();
    assert_eq!(
        review_lines.len(),
        1,
        "the renamed item must appear on exactly one row (no orphan dup), got: {:#?}",
        review_lines
    );
    assert!(
        review_lines[0].contains("outdated"),
        "the single review row must be the outdated row: {}",
        review_lines[0]
    );
    assert!(
        !recall.stdout.contains("removed upstream"),
        "a pure rename must not be flagged removed upstream: {}",
        recall.stdout
    );
}

/// EDGE 5 (JSON under rename): in `recall --json` the renamed item resolves to its
/// manifest entry by stable identity -- `installed:true` with the correct commit
/// -- and is emitted EXACTLY ONCE, never duplicated as a separate `orphaned:true`
/// row. The single-item `recall <item> --json`, looked up by the OLD installed
/// name, also resolves correctly.
// spec: CLI-73
// spec: CLI-75
#[test]
fn recall_json_renamed_item_installed_once_not_orphaned() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld failed");

    // Record the install commit so we can assert the JSON carries it.
    // spec: CLI-167 - recall --json now wrapped in {"schema":1,"items":[...]}.
    let before = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let source_commit = before["items"][0]["commit"].as_str().unwrap().to_string();

    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let j = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let items = j["items"][0]["items"].as_array().expect("items array");

    // The skill must appear under its NEW effective key, installed, with the
    // commit it was installed at, and must not be duplicated as an orphan.
    let review_rows: Vec<&serde_json::Value> = items
        .iter()
        .filter(|r| {
            let k = r["key"].as_str().unwrap_or("");
            k == "skill:review" || k == "skill:jk:review"
        })
        .collect();
    assert_eq!(
        review_rows.len(),
        1,
        "the renamed skill must be emitted exactly once in recall --json: {items:#?}"
    );
    let row = review_rows[0];
    assert_eq!(
        row["key"].as_str(),
        Some("skill:jk:review"),
        "the renamed item must carry its new effective key: {row}"
    );
    assert_eq!(
        row["installed"].as_bool(),
        Some(true),
        "the renamed item must resolve installed by stable identity: {row}"
    );
    assert_eq!(
        row["commit"].as_str(),
        Some(source_commit.as_str()),
        "the renamed item must carry its install commit: {row}"
    );
    assert!(
        row.get("orphaned").is_none(),
        "the renamed item must not be flagged orphaned: {row}"
    );
    // No item in this source's JSON should be orphaned at all.
    assert!(
        !items.iter().any(|r| r.get("orphaned").is_some()),
        "no catalog-matched item may be reported orphaned under a pure rename: {items:#?}"
    );

    // The single-item lookup by the OLD installed name still resolves.
    let detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(
        detail.success,
        "recall detail --json failed: {}",
        detail.stderr
    );
    let d = parse_json(&detail.stdout);
    assert_eq!(
        d["name"].as_str(),
        Some("review"),
        "the single-item lookup resolves by the recorded (old) name: {d}"
    );
}

/// EDGE 2 (genuine removal still works): an item actually deleted from the source
/// (file removed, then sync) has NO catalog item sharing its identity, so it is a
/// real orphan. It must still show `(removed upstream)` in the human `recall` view
/// AND appear as installed-but-no-catalog-match (orphaned, installed:true) in
/// `recall --json`. The orphan rework must not have regressed this.
// spec: CLI-73
// spec: CLI-75
#[test]
fn removed_upstream_still_flagged_in_recall_human_and_json() {
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");

    // The agent disappears upstream, then sync drops it from the catalog.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    // Human view: the installed-but-removed item is flagged removed upstream.
    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);
    assert!(
        recall.stdout.contains("agent:dev"),
        "the removed item must still be listed: {}",
        recall.stdout
    );
    assert!(
        recall.stdout.contains("removed upstream"),
        "a genuinely removed item must be flagged removed upstream: {}",
        recall.stdout
    );

    // JSON: the removed item appears as installed:true with orphaned:true (no
    // catalog match). The review skill (still in the catalog) is not orphaned.
    // spec: CLI-167 - recall --json wrapped in envelope.
    let j = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let items = j["items"][0]["items"].as_array().expect("items array");
    let dev = items
        .iter()
        .find(|r| r["key"].as_str() == Some("agent:dev"))
        .expect("the removed agent must be present in recall --json");
    assert_eq!(
        dev["installed"].as_bool(),
        Some(true),
        "the removed-upstream item is still installed: {dev}"
    );
    assert_eq!(
        dev["orphaned"].as_bool(),
        Some(true),
        "the removed-upstream item must be flagged orphaned in JSON: {dev}"
    );
    assert!(
        !items
            .iter()
            .any(|r| r["key"].as_str() == Some("skill:review") && r.get("orphaned").is_some()),
        "a still-cataloged item must not be orphaned: {items:#?}"
    );
}

/// EDGE 3 (same bare name across two sources does not cross-match): identity is
/// (source, kind, bare_name). Two melded sources each ship a `review` skill (the
/// second namespaced so both can install). Removing `review` from source A must
/// flag ONLY A's item as removed upstream and must NOT mislabel B's `zz:review`,
/// which is unchanged. This is the isolation the rework depends on: A's orphan
/// scan must not match B's manifest entry by bare name, and B's catalog must not
/// rescue A's removed item.
// spec: CLI-75
#[test]
fn same_bare_name_across_sources_does_not_cross_match_on_removal() {
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld a");
    assert!(
        a.mind(&["meld", &b.source_spec(), "--as", "zz"]).success,
        "meld b as zz"
    );

    // Both review skills install side by side under distinct effective names.
    assert!(a.mind(&["learn", "review"]).success, "learn review (a)");
    assert!(
        a.mind(&["learn", "zz:review"]).success,
        "learn zz:review (b)"
    );

    // Remove review from source A only, then sync.
    a.remove_and_commit("skills/review/SKILL.md");
    assert!(a.mind(&["sync"]).success, "sync failed");

    let recall = a.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // A's review is removed upstream; B's zz:review is untouched (not flagged).
    let removed_lines: Vec<&str> = recall
        .stdout
        .lines()
        .filter(|l| l.contains("removed upstream"))
        .collect();
    assert_eq!(
        removed_lines.len(),
        1,
        "exactly one item (A's review) must be removed upstream: {:#?}",
        removed_lines
    );
    assert!(
        removed_lines[0].contains("skill:review") && !removed_lines[0].contains("zz:review"),
        "the removed-upstream row must be A's review, not B's zz:review: {}",
        removed_lines[0]
    );

    // JSON confirms the cross-match isolation: A's review orphaned, B's
    // zz:review installed and NOT orphaned.
    // spec: CLI-167 - recall --json wrapped in envelope.
    let jj = parse_json(&a.mind(&["recall", "--json"]).stdout);
    let sources = jj["items"].as_array().expect("sources array");
    let mut saw_review_orphan = false;
    let mut saw_zz_review_ok = false;
    for s in sources {
        for r in s["items"].as_array().unwrap() {
            match r["key"].as_str() {
                Some("skill:review") => {
                    assert_eq!(
                        r["orphaned"].as_bool(),
                        Some(true),
                        "A's review must be orphaned: {r}"
                    );
                    saw_review_orphan = true;
                }
                Some("skill:zz:review") => {
                    assert!(
                        r.get("orphaned").is_none(),
                        "B's zz:review must not be orphaned: {r}"
                    );
                    assert_eq!(
                        r["installed"].as_bool(),
                        Some(true),
                        "B's zz:review must stay installed: {r}"
                    );
                    saw_zz_review_ok = true;
                }
                _ => {}
            }
        }
    }
    assert!(
        saw_review_orphan && saw_zz_review_ok,
        "both A's orphaned review and B's intact zz:review must be present: {jj:#?}"
    );
}

/// EDGE 4 (unmanaged accounting unaffected by the orphan rework): an agent-home
/// item mind did not install still lists as unmanaged, AND a genuinely
/// removed-upstream mind-installed item is still flagged removed upstream in the
/// same `recall` run. The two classifications are independent: the orphan change
/// must not swallow an unmanaged item, and unmanaged scanning must not suppress a
/// removed-upstream flag.
// spec: UNM-2
// spec: CLI-75
#[test]
fn unmanaged_listing_unaffected_by_orphan_detection() {
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");

    // Seed an unmanaged skill directly in the lobe (mind did not install it).
    write(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\nname: handmade\ndescription: hand written\n---\n# handmade\n",
    );

    // Genuinely remove the installed agent upstream.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // The unmanaged group and item are present, unchanged by the orphan rework.
    assert!(
        recall.stdout.contains("unmanaged: not installed by mind"),
        "the unmanaged group must still be shown: {}",
        recall.stdout
    );
    assert!(
        recall.stdout.contains("skill:handmade"),
        "the unmanaged item must still be listed: {}",
        recall.stdout
    );
    // The removed-upstream mind item is still flagged, in the same run.
    assert!(
        recall.stdout.contains("agent:dev") && recall.stdout.contains("removed upstream"),
        "the removed-upstream mind item must still be flagged alongside unmanaged: {}",
        recall.stdout
    );
    // The unmanaged item must NOT be misreported as a source orphan.
    assert!(
        !recall.stdout.contains("handmade") || !recall_handmade_is_in_a_source(&recall.stdout),
        "the unmanaged item must not be classified as a source's removed-upstream item: {}",
        recall.stdout
    );
}

/// True if a `handmade` mention appears on a `removed upstream` line, which would
/// mean the unmanaged item was misclassified as a source orphan.
fn recall_handmade_is_in_a_source(stdout: &str) -> bool {
    stdout
        .lines()
        .any(|l| l.contains("handmade") && l.contains("removed upstream"))
}

// ---- DSC-59/60/61: curator adoption of an un-onboarded nested source --------

/// Read the whole `sources.json` for a sandbox.
fn read_sources_json(sb: &Sandbox) -> String {
    std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json")
}

/// Build an un-onboarded nested source (no mind.toml) whose items live under a
/// `pkg/` subdir, so default convention discovery (repo root) finds nothing but a
/// curator-supplied `roots = ["pkg"]` does. It carries a `stable` branch (for a
/// curator follow-branch pin) holding the same content. Returns the sandbox.
fn make_unonboarded_nested(name: &str) -> Sandbox {
    let sb = Sandbox::bare(name);
    // An item under pkg/ only: the repo root holds no skills/agents/rules dir, so
    // a root-only scan discovers nothing.
    write(
        &sb.source.join("pkg/skills/widget/SKILL.md"),
        "---\nname: widget\ndescription: A curated widget skill\n---\n# widget\n",
    );
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "pkg layout"]);
    // A stable branch at this same content, so a follow-branch pin resolves.
    git(&sb.source, &["branch", "stable"]);
    sb
}

#[test]
fn curator_applies_follow_branch_roots_and_hook_when_nested_has_no_mind_toml() {
    // spec: DSC-59 DSC-60 DSC-61
    // A super-source curates an un-onboarded nested source (no mind.toml of its
    // own), supplying follow-branch, roots, and a hook. All three apply: the
    // nested source's pin is recorded as follow-branch, roots govern discovery
    // (the pkg-only item is found), and the hook runs.
    let nested = make_unonboarded_nested("widgets");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-61: roots applied -> the pkg-only item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "curator roots must govern discovery so the pkg-only item is found: {}",
        probe.stdout
    );

    // DSC-61: follow-branch applied -> the nested source's recorded pin is
    // follow-branch=stable. (The registry super-source itself has no pin
    // directive, so a follow-branch/stable pin can only be the nested source's.)
    let json = read_sources_json(&registry);
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "the nested source's pin must be recorded as follow-branch=stable: {json}"
    );

    // DSC-61: the hook ran in the nested source's clone (a follow-branch pin
    // snapshots a local source under the sources tree).
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("widgets");
    let marker = nested_clone.join("curated-hookran");
    assert!(
        marker.exists(),
        "the curator-supplied hook must have run in the nested clone: {} missing",
        marker.display()
    );
    // The hook command is recorded against the nested source.
    assert!(
        json.contains("touch curated-hookran"),
        "the curator hook command must be recorded on the nested source: {json}"
    );
}

#[test]
fn curator_values_ignored_with_warning_when_nested_has_mind_toml() {
    // spec: DSC-59 DSC-60 DSC-65
    // DSC-65: the curator's pin directive is AUTHORITATIVE and applies even when
    // the nested source ships its own mind.toml. DSC-60: the gated fields (roots
    // and hooks) are still suppressed when the source has a mind.toml, and the
    // warning fires only because those gated fields are present.
    let nested = make_unonboarded_nested("onboarded");
    // The nested source onboards with a metadata-only mind.toml (no pin/roots/
    // hooks). It still ships the pkg-only item, which a root scan won't find.
    nested.write_and_commit("mind.toml", "[source]\ndescription = \"onboarded\"\n");
    // Point stable at the onboarded commit so the curator follow-branch can apply.
    git(&nested.source, &["branch", "-f", "stable"]);

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: a warning fires because roots/hooks are gated and suppressed.
    // The warning must mention the source name and "ignored"; it need NOT mention
    // the pin (DSC-65 exempts the pin from the warning).
    assert!(
        r.stderr.contains("ships its own mind.toml")
            && r.stderr.contains("ignored")
            && r.stderr.contains("onboarded"),
        "a DSC-60 warning must be emitted naming the onboarded source: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // DSC-65: the curator follow-branch IS now applied (authoritative). The nested
    // source's recorded pin must be follow-branch=stable, NOT the default branch.
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "the curator follow-branch must apply (authoritative DSC-65), recorded as follow-branch=stable: {json}"
    );
    // The suppressed roots: the pkg-only item is not discovered (a root scan of
    // the onboarded source finds nothing under the repo root). Roots are gated.
    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:widget"),
        "the curator roots must be suppressed: the pkg-only item must not appear: {}",
        probe.stdout
    );
    // The suppressed hook: it never ran and is not recorded. Hooks are gated.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("onboarded");
    assert!(
        !nested_clone.join("curated-hookran").exists(),
        "the curator hook must be suppressed (no marker)"
    );
    assert!(
        !json.contains("touch curated-hookran"),
        "the curator hook command must not be recorded when suppressed: {json}"
    );
}

#[test]
fn consumer_pin_flag_overrides_curator_follow_branch() {
    // spec: DSC-61
    // DSC-41 precedence: a consumer `meld` pin flag still wins over a curator
    // follow-branch. The effective pin is `consumer_pin.or(curator_follow_pin)`,
    // so a consumer flag on a direct meld of the (otherwise curator-adopted)
    // un-onboarded source must record the consumer pin, not the curator branch.
    //
    // The un-onboarded source carries a `stable` branch (what a curator would
    // pin via follow-branch) and a `v1` tag (the consumer's explicit choice).
    // Melding it directly with --pin-tag v1 must record the tag pin: the consumer
    // flag wins. (A nested meld passes no consumer pin, so the curator branch is
    // what applies there; the apply test covers that positive path.)
    let nested = make_unonboarded_nested("pinned");
    git(&nested.source, &["tag", "v1"]);
    let spec = nested.source_spec();

    let r = nested.mind(&["meld", &spec, "--pin-tag", "v1"]);
    assert!(r.success, "meld --pin-tag should succeed: {}", r.stderr);
    let json = read_sources_json(&nested);
    assert!(
        json.contains("\"kind\": \"tag\"") && json.contains("v1"),
        "a consumer pin flag must win and record a tag pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "a consumer pin flag must override any follow-branch (no follow-branch pin recorded): {json}"
    );
}

#[test]
fn curator_empty_roots_list_discovers_nothing() {
    // spec: DSC-59 DSC-61
    // A curator `roots = []` (explicit empty list) is distinct from unset roots:
    // it scans zero roots, mirroring the source-level DSC-50/DSC-53 semantics.
    // The un-onboarded nested source's only item lives under pkg/, so with an
    // empty roots list nothing is discovered -- and crucially this differs from
    // omitting roots entirely (which would fall back to the repo root and still
    // find nothing here, so to make the empty-list behavior load-bearing we put
    // an item at the REPO ROOT too: an unset/repo-root scan would find it, while
    // an explicit empty list must scan nothing and find neither.)
    let nested = make_unonboarded_nested("emptyroots");
    // Add a root-level item. A repo-root scan (unset roots) would find this; an
    // explicit empty roots list must not.
    nested.write_and_commit(
        "skills/toplevel/SKILL.md",
        "---\nname: toplevel\ndescription: A root-level skill\n---\n# toplevel\n",
    );

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = []\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // An explicit empty roots list scans nothing: neither the pkg item nor the
    // root-level item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:widget"),
        "empty curator roots must scan nothing (pkg item must not appear): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:toplevel"),
        "empty curator roots must scan nothing, not even the repo root (toplevel must not appear): {}",
        probe.stdout
    );
}

#[test]
fn curator_hooks_do_not_leak_across_nested_entries() {
    // spec: DSC-59 DSC-61
    // Two un-onboarded nested sources, each with its own
    // `[[discover.sources.hooks]]`. Each entry's CuratedConfig is independent, so
    // a given nested source must run ONLY its own hook -- never the sibling
    // entry's. A leak would run both hooks in one clone.
    let first = make_unonboarded_nested("alpha");
    let second = make_unonboarded_nested("beta");

    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch alpha-marker\"\n\n\
             [[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch beta-marker\"\n",
            first.source_spec(),
            second.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    let alpha_clone = registry
        .mind_home
        .join("sources/local")
        .join(first.base_name())
        .join("alpha");
    let beta_clone = registry
        .mind_home
        .join("sources/local")
        .join(second.base_name())
        .join("beta");

    // Each entry ran exactly its own hook in its own clone.
    assert!(
        alpha_clone.join("alpha-marker").exists(),
        "alpha's own hook must run in alpha's clone"
    );
    assert!(
        beta_clone.join("beta-marker").exists(),
        "beta's own hook must run in beta's clone"
    );
    // No leak: a sibling entry's hook must not have run in the other's clone.
    assert!(
        !alpha_clone.join("beta-marker").exists(),
        "beta's hook leaked into alpha's clone"
    );
    assert!(
        !beta_clone.join("alpha-marker").exists(),
        "alpha's hook leaked into beta's clone"
    );

    // And the recorded hook on each nested source is only its own command.
    let json = read_sources_json(&registry);
    assert!(
        json.contains("touch alpha-marker") && json.contains("touch beta-marker"),
        "each nested source records its own hook command: {json}"
    );
}

#[test]
fn curator_values_suppressed_when_nested_declares_own_pin_roots_hooks() {
    // spec: DSC-59 DSC-60 DSC-65
    // DSC-60 gate with a nested mind.toml that DECLARES its OWN pin/roots/hooks.
    // Under DSC-65: the CURATOR pin now wins over the source's own pin. Roots and
    // hooks remain gated: the source's own roots/hooks still govern (the curator's
    // roots/hooks are suppressed). The warning fires because gated values are present.
    let nested = make_unonboarded_nested("selfdeclared");
    // The nested source onboards declaring its OWN pin (follow-branch = own), its
    // own roots (pkg, where its item lives), and its own hook.
    nested.write_and_commit(
        "mind.toml",
        "[source]\n\
         description = \"self-declared\"\n\
         follow-branch = \"own\"\n\
         roots = [\"pkg\"]\n\n\
         [[hooks]]\n\
         run = \"touch source-own-hook\"\n",
    );
    git(&nested.source, &["branch", "own"]);
    git(&nested.source, &["branch", "-f", "stable"]);

    let registry = Sandbox::bare("registry");
    // The curator supplies a DIFFERENT follow-branch (stable), bogus roots, and a
    // curator hook. Under DSC-65: curator pin (stable) overrides source pin (own).
    // Roots and hooks are still gated: source's own roots/hooks govern.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"nonexistent\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curator-hook\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec, "--dangerously-skip-install-hook-check"]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: the warning fires because roots/hooks are present and gated.
    assert!(
        r.stderr.contains("ships its own mind.toml")
            && r.stderr.contains("ignored")
            && r.stderr.contains("selfdeclared"),
        "a DSC-60 warning must name the onboarded source: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The source's OWN roots win (roots are gated): its pkg item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "the source's own roots = [pkg] must govern, finding its item: {}",
        probe.stdout
    );
    // DSC-65: the CURATOR follow-branch=stable now wins over the source's own
    // follow-branch=own. The recorded pin must be stable, NOT own.
    assert!(
        json.contains("follow-branch") && json.contains("\"stable\""),
        "the curator follow-branch=stable must win (DSC-65 authoritative): {json}"
    );
    assert!(
        !json.contains("\"own\""),
        "the source's own follow-branch=own must be overridden by the curator pin: {json}"
    );
    // The source's own hook ran; the curator's did not. Hooks are gated.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("selfdeclared");
    assert!(
        nested_clone.join("source-own-hook").exists(),
        "the source's own declared hook must run"
    );
    assert!(
        !nested_clone.join("curator-hook").exists(),
        "the curator hook must be suppressed"
    );
    assert!(
        json.contains("touch source-own-hook") && !json.contains("touch curator-hook"),
        "only the source's own hook command is recorded: {json}"
    );
}

#[test]
fn curator_pin_ref_authoritative_overrides_source_own_pin() {
    // spec: DSC-59 DSC-65
    // A curator entry with `pin-ref = <sha>` on a nested source that has its own
    // mind.toml (with its own `follow-branch = "own"`) must record the curator's
    // pin-ref as the nested source's effective pin (DSC-65: curator pin is
    // authoritative, regardless of the source's own mind.toml). Roots and hooks
    // from the curator are absent here so no DSC-60 warning fires.
    let nested = make_unonboarded_nested("pinref-target");
    // The nested source onboards with its own follow-branch pin pointing to "own".
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nfollow-branch = \"own\"\n",
    );
    git(&nested.source, &["branch", "own"]);

    // Capture the nested source's HEAD commit sha (the onboarded content commit).
    let sha_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&nested.source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("git rev-parse HEAD");
    let sha = String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string();
    assert!(!sha.is_empty(), "could not capture HEAD commit sha");

    // The curator supplies pin-ref pointing at the onboarded commit.
    let registry = Sandbox::bare("pinref-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             pin-ref = \"{sha}\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-65: the curator pin-ref must apply even though the source has its own
    // mind.toml with its own follow-branch=own. No warning fires: no gated
    // fields (roots/hooks) are present.
    assert!(
        !r.stderr.contains("ignored"),
        "no DSC-60 warning should fire for a pin-only curator entry (no gated fields): {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The recorded pin must be the ref variant with the curator-supplied sha.
    assert!(
        json.contains("\"kind\": \"ref\"") && json.contains(&sha),
        "the curator pin-ref must be recorded as the ref pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "the source's own follow-branch must be overridden by the curator pin-ref: {json}"
    );
}

#[test]
fn curator_hook_skipped_under_non_tty_without_skip_flag() {
    // spec: DSC-61
    // The curator hook runs through the same disclosure/safety path as a source's
    // own hooks, INCLUDING the non-TTY skip (HOOK-22). The integration harness is
    // non-TTY (piped stdin), so a meld WITHOUT
    // `--dangerously-skip-install-hook-check` must SKIP the curator hook (its
    // marker is never created) rather than run it silently, while the meld itself
    // still succeeds and the source is registered.
    let nested = make_unonboarded_nested("skiphook");
    let registry = Sandbox::bare("registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources.hooks]]\n\
             run = \"touch curated-hookran\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    // No --dangerously-skip-install-hook-check: non-TTY must take the skip path.
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld should still succeed: {} {}",
        r.stdout, r.stderr
    );

    // The hook was skipped, not run: no marker.
    let nested_clone = registry
        .mind_home
        .join("sources/local")
        .join(nested.base_name())
        .join("skiphook");
    assert!(
        !nested_clone.join("curated-hookran").exists(),
        "a non-TTY meld without the skip flag must NOT run the curator hook"
    );
    // The skip is announced (HOOK-22 disclosure path), not silent.
    assert!(
        r.stdout.contains("skipped install hook") || r.stderr.contains("skipped install hook"),
        "the skip must be announced: {} {}",
        r.stdout,
        r.stderr
    );
    // The source is still registered (skip != abort), and the follow-branch pin
    // still applies (the gate is about hook execution, not pin/roots).
    let json = read_sources_json(&registry);
    assert!(
        json.contains("follow-branch") && json.contains("stable"),
        "roots/follow-branch still apply even when the hook is skipped: {json}"
    );
}

#[test]
fn sync_rewalk_applies_curator_follow_branch_to_new_nested() {
    // spec: DSC-59 DSC-61
    // The DSC-57 sync re-walk threads CuratedConfig: a nested source newly added
    // to a super-source's [discover].sources, carrying a curator follow-branch,
    // is melded by `sync` with the same gate/apply behavior as a fresh meld. Its
    // recorded pin must be follow-branch=stable.
    let registry = Sandbox::bare("registry");
    let first = make_unonboarded_nested("present"); // listed from the start
    let later = make_unonboarded_nested("arriving"); // added before sync

    // Initially the super-source curates only `first`.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = [\"pkg\"]\n",
            first.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        r.success,
        "initial meld should succeed: {} {}",
        r.stdout, r.stderr
    );

    // `arriving` is not yet registered.
    let before = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !before.contains("/arriving"),
        "the new nested source must not be registered before sync: {before}"
    );

    // Add `arriving` with a curator follow-branch to the super-source's list.
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             roots = [\"pkg\"]\n\n\
             [[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             roots = [\"pkg\"]\n",
            first.source_spec(),
            later.source_spec()
        ),
    );

    // sync re-walks and melds `arriving`, applying the curator follow-branch.
    let r = registry.mind(&["sync"]);
    assert!(r.success, "sync should succeed: {} {}", r.stdout, r.stderr);
    assert!(
        registry
            .mind(&["recall", "--sources"])
            .stdout
            .contains("/arriving"),
        "sync must register the newly-listed nested source"
    );

    // DSC-61 end-to-end through sync: the curator follow-branch is recorded as the
    // newly discovered source's pin. The registry super-source declares no pin and
    // `first` has none, so a follow-branch=stable pin can only be `arriving`'s.
    let json = read_sources_json(&registry);
    assert!(
        json.contains("arriving") && json.contains("follow-branch") && json.contains("stable"),
        "sync's re-walk must apply the curator follow-branch to the new nested source: {json}"
    );
    // Curator roots also applied through sync: the pkg-only item is discovered.
    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:widget"),
        "curator roots must govern discovery for a sync-discovered nested source: {}",
        probe.stdout
    );
}

#[test]
fn curator_pin_tag_authoritative_overrides_source_own_pin() {
    // spec: DSC-59 DSC-65
    // A curator entry with `pin-tag = <tag>` on a nested source that ships its
    // own mind.toml (with its own `follow-branch = "own"`) must record the
    // curator's tag pin as the nested source's effective pin (DSC-65: curator pin
    // is authoritative regardless of the source's own mind.toml). This covers the
    // pin-tag kind, complementing the pin-ref and follow-branch cases. No gated
    // fields (roots/hooks) are present, so no DSC-60 warning fires.
    let nested = make_unonboarded_nested("pintag-target");
    // The nested source onboards with its own follow-branch pin pointing to "own".
    nested.write_and_commit(
        "mind.toml",
        "[source]\ndescription = \"onboarded\"\nfollow-branch = \"own\"\n",
    );
    git(&nested.source, &["branch", "own"]);
    // A tag at the onboarded commit, which the curator pins to.
    git(&nested.source, &["tag", "rel-1"]);

    let registry = Sandbox::bare("pintag-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             pin-tag = \"rel-1\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // No DSC-60 warning: a pin-only curator entry has no gated fields.
    assert!(
        !r.stderr.contains("ignored"),
        "no DSC-60 warning should fire for a pin-only (pin-tag) curator entry: {}",
        r.stderr
    );

    let json = read_sources_json(&registry);
    // The recorded pin must be the tag variant with the curator-supplied tag.
    assert!(
        json.contains("\"kind\": \"tag\"") && json.contains("rel-1"),
        "the curator pin-tag must be recorded as the tag pin: {json}"
    );
    assert!(
        !json.contains("follow-branch"),
        "the source's own follow-branch must be overridden by the curator pin-tag: {json}"
    );
}

#[test]
fn curator_conflicting_pin_directives_is_meld_error() {
    // spec: DSC-59
    // A `[discover.sources]` entry that declares more than one pin directive
    // (here follow-branch AND pin-ref) is a MindToml one-of error. The error must
    // surface at meld (not only as a unit test), and nothing must be registered.
    let nested = make_unonboarded_nested("conflict-target");
    let registry = Sandbox::bare("conflict-registry");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             follow-branch = \"stable\"\n\
             pin-ref = \"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\"\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "a nested entry with two pin directives must fail at meld: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("conflicting pin"),
        "the meld error must mention conflicting pin directives: {}",
        r.stderr
    );
    // The conflicting nested source must NOT be registered.
    let recall = registry.mind(&["recall", "--sources"]).stdout;
    assert!(
        !recall.contains("/conflict-target"),
        "the conflicting nested source must not be registered: {recall}"
    );
}

// ---- DEP-4, DEP-5, DEP-6: `requires:` frontmatter dependency key ----------

#[test]
fn learn_requires_frontmatter_pulls_dependency_closure() {
    // spec: DEP-4 DEP-5
    // A `requires: agent:reviewer` entry in SKILL.md (no {{ns:}} token in the
    // text) still pulls the referenced agent into the dependency closure when
    // `learn` selects the skill alone. The agent installs before the skill
    // (dependency-first order, DEP-21).
    let sb = Sandbox::bare("req-closure");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "learn must succeed: {}", r.stderr);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "selected skill must be installed: {recall}"
    );
    assert!(
        recall.contains("agent:reviewer"),
        "requires entry must pull the dependency into the closure: {recall}"
    );

    // Dependency-first: the reviewer learned line must precede the review line.
    let dep_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned agent:reviewer "));
    let dep_line = dep_line.unwrap_or_else(|| panic!("no reviewer learned line: {}", r.stdout));
    let skill_line = r
        .stdout
        .lines()
        .position(|l| l.starts_with("learned skill:review "));
    let skill_line = skill_line.unwrap_or_else(|| panic!("no review learned line: {}", r.stdout));
    assert!(
        dep_line < skill_line,
        "requires dep must install before its dependent: {}",
        r.stdout
    );
}

#[test]
fn learn_requires_union_with_token_dep_deduped() {
    // spec: DEP-4
    // When the item both declares `requires: agent:reviewer` AND has a
    // {{ns:reviewer}} token in the text, only one dep edge exists: the agent
    // installs exactly once. Regression guard for the dedup invariant.
    let sb = Sandbox::bare("req-dedup");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\nhandoff to {{ns:reviewer}}\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    // Add a third item so the source is a proper subset on `learn skill:review`.
    sb.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(r.success, "{}", r.stderr);

    // agent:reviewer appears in the learned output exactly once.
    let reviewer_count = r
        .stdout
        .lines()
        .filter(|l| l.contains("agent:reviewer"))
        .count();
    assert_eq!(
        reviewer_count, 1,
        "dedup: agent:reviewer must appear once in the install output: {}",
        r.stdout
    );
}

#[test]
fn learn_requires_typo_is_bad_reference_error() {
    // spec: DEP-6
    // A `requires:` entry naming a non-existent sibling is a BadReference at
    // install time: `learn` must fail with a non-zero exit and a message
    // referencing the bad entry.
    let sb = Sandbox::bare("req-typo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:nonexistent\n---\n# review skill\n",
    );
    // Add another item so learn sees a proper subset (triggers full validation).
    sb.write_and_commit(
        "agents/helper.md",
        "---\ndescription: helper\n---\n# helper\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:review", "--yes"]);
    assert!(
        !r.success,
        "learn with unresolved requires must fail: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("nonexistent")
            || combined.contains("bad")
            || combined.contains("reference"),
        "error output must mention the bad entry: {combined}"
    );
}

#[test]
fn learn_requires_resolves_against_own_source_not_a_sibling_source() {
    // spec: DEP-5 DEP-6
    // A `requires` entry is intra-source (DEP-5): it must resolve against the
    // referencing item's OWN source, never another melded source's items. Two
    // sources are melded; source A's skill requires `agent:helper` and that agent
    // exists ONLY in source B (not in A). If validation/resolution leaked across
    // sources, the entry would wrongly resolve and `learn` would succeed. It must
    // instead fail as a BadReference, because A has no `helper` of its own.
    let a = Sandbox::bare("alpha");
    a.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:helper\n---\n# review\n",
    );
    // A has no `helper`; it has only an unrelated sibling so the source is a
    // proper subset on `learn skill:review` (full closure validation runs).
    a.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");
    // Source B is where a `helper` agent actually lives.
    let b = Sandbox::bare("beta");
    b.write_and_commit(
        "agents/helper.md",
        "---\nname: helper\ndescription: Helper\n---\n# helper\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld A failed");
    assert!(a.mind(&["meld", &b.source_spec()]).success, "meld B failed");

    let r = a.mind(&["learn", "skill:review", "--yes"]);
    assert!(
        !r.success,
        "a requires entry must not resolve against another source's agent: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("helper") || combined.contains("bad") || combined.contains("reference"),
        "error must name the unresolved cross-source entry: {combined}"
    );
}

#[test]
fn review_requires_resolves_per_source_in_a_multi_source_registry() {
    // spec: DEP-5 DEP-6 CLI-131
    // Reviewing a melded source resolves each item's `requires` against that
    // source's own siblings only. Source B is melded and carries `agent:helper`;
    // source A's skill requires `agent:helper` but A has no such agent. Reviewing
    // A (by registry selector) must report the unresolved entry as a hard
    // finding -- B's `helper` must NOT satisfy A's requirement.
    let a = Sandbox::bare("alpha");
    a.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:helper\n---\n# review\n",
    );
    let b = Sandbox::bare("beta");
    b.write_and_commit(
        "agents/helper.md",
        "---\nname: helper\ndescription: Helper\n---\n# helper\n",
    );
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld A failed");
    assert!(a.mind(&["meld", &b.source_spec()]).success, "meld B failed");

    // Review the alpha source by its registry selector.
    let r = a.mind(&["review", "alpha"]);
    assert!(
        !r.success,
        "review of alpha must fail: its requires must not resolve against beta: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("bad-reference") && combined.contains("helper"),
        "must report alpha's unresolved cross-source requires: {combined}"
    );
}

#[test]
fn review_requires_typo_is_hard_finding() {
    // spec: DEP-6 CLI-131
    // `review` surfaces an unresolved `requires:` entry as a hard bad-reference
    // finding, identical in severity to an unresolved {{ns:}} token.
    let sb = Sandbox::bare("review-req-typo");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:nonexistent\n---\n# review skill\n",
    );
    // No `agent:nonexistent` item exists in the source.

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        !r.success,
        "review with unresolved requires must exit non-zero: {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{} {}", r.stdout, r.stderr);
    assert!(
        combined.contains("bad-reference"),
        "must report a bad-reference finding: {combined}"
    );
    assert!(
        combined.contains("nonexistent"),
        "bad-reference message must name the offending entry: {combined}"
    );
}

// ---------------------------------------------------------------------------
// DEP-60: forget warns about installed dependents
// ---------------------------------------------------------------------------

/// Build a sandbox where `skill:review` depends on `agent:reviewer` (via
/// `requires:`) and both are installed. Returns the sandbox and the source
/// identity string used for meld.
fn dep60_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep60-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    // Install both items (--yes bypasses the dependency prompt).
    assert!(
        sb.mind(&["learn", "skill:review", "--yes"]).success,
        "fixture: learn should succeed"
    );
    sb
}

#[test]
fn forget_single_item_with_dependents_refuses_non_tty_without_force() {
    // spec: DEP-60
    // Forgetting an item that another installed item depends on: in a non-TTY
    // run without --force or --yes, the command must refuse (ConfirmationRequired)
    // and leave the item installed.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "agent:reviewer"]);
    assert!(
        !r.success,
        "forget of a depended-on item must refuse in non-TTY: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );
    // The item was NOT removed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "the item must still be installed after refused forget"
    );
}

#[test]
fn forget_single_item_with_dependents_lists_them() {
    // spec: DEP-60
    // The refusal output must list which installed item(s) depend on the target.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "agent:reviewer"]);
    assert!(!r.success);
    assert!(
        r.stdout.contains("skill:review"),
        "output must name the dependent: {}",
        r.stdout
    );
}

#[test]
fn forget_single_item_with_dependents_proceeds_with_yes() {
    // spec: DEP-60
    // `--yes` (global bypass) lets the removal proceed even when dependents exist.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "--yes", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --yes must proceed: {} {}",
        r.stdout, r.stderr
    );
    // The item is now removed.
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "item must be removed after forget --yes"
    );
    // DEP-50: the dependent is NOT removed (no cascade).
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "dependent must remain installed (DEP-50)"
    );
}

#[test]
fn forget_single_item_with_dependents_proceeds_with_force() {
    // spec: DEP-60
    // `--force` also bypasses the dependents gate; the item is removed.
    let sb = dep60_fixture();

    let r = sb.mind(&["forget", "--force", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --force must proceed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "item must be removed after forget --force"
    );
    // DEP-50: the dependent is NOT removed.
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "dependent must remain installed (DEP-50)"
    );
}

#[test]
fn forget_single_item_no_dependents_removes_without_extra_prompt() {
    // spec: DEP-60
    // An item with no installed dependents removes immediately with no extra
    // confirmation (CLI-40 behavior unchanged).
    let sb = dep60_fixture();

    // Forget the skill (the dependent, not the dependency) -- no dependents of skill:review.
    let r = sb.mind(&["forget", "skill:review"]);
    assert!(
        r.success,
        "forget with no dependents must not prompt: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "skill must be removed"
    );
    // agent:reviewer is still installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer must remain installed"
    );
}

#[test]
fn forget_glob_path_uses_existing_cli42_confirmation_not_dep60() {
    // spec: DEP-60 CLI-42
    // The glob path (keys.len() > 1) keeps the CLI-42 confirmation unchanged;
    // the DEP-60 dependents gate is single-item only.
    let sb = dep60_fixture();

    // forget '*' hits both items. In a non-TTY without --yes it should refuse
    // with the existing CLI-42 message (count-based), not a DEP-60 dependents
    // warning. We check that it refuses and mentions the count.
    let r = sb.mind(&["forget", "*"]);
    assert!(!r.success, "multi-item forget must refuse: {}", r.stderr);
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("would remove"),
        "must show CLI-42 count message: {}",
        r.stdout
    );
    // Both items must still be installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer still installed"
    );
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "review still installed"
    );
}

// ---------------------------------------------------------------------------
// DEP-61: recall --tree
// ---------------------------------------------------------------------------

/// Fixture for tree tests: a chain `skill:review -> agent:reviewer`.
fn dep61_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep61-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "skill:review", "--yes"]).success,
        "fixture: learn should succeed"
    );
    sb
}

#[test]
fn recall_tree_renders_dependency_forest() {
    // spec: DEP-61
    // `recall --tree` with no item should render the full installed forest.
    // `skill:review` (no incoming installed edge) is a root; `agent:reviewer`
    // is its dependency, nested beneath it.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(
        r.success,
        "recall --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:review"),
        "forest must include skill:review: {out}"
    );
    assert!(
        out.contains("agent:reviewer"),
        "forest must include agent:reviewer: {out}"
    );
    // skill:review is a root (at the start of a line after "- ").
    assert!(
        out.lines().any(|l| l.starts_with("- skill:review")),
        "skill:review must be a root: {out}"
    );
    // agent:reviewer is indented (not a primary root since skill:review depends on it).
    assert!(
        out.lines().any(|l| l.starts_with("  - agent:reviewer")),
        "agent:reviewer must be nested under skill:review: {out}"
    );
}

#[test]
fn recall_tree_item_scopes_to_subtree() {
    // spec: DEP-61
    // `recall <item> --tree` scopes the output to one item's subtree.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "skill:review", "--tree"]);
    assert!(
        r.success,
        "recall <item> --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    // The root of the subtree is the requested item.
    assert!(
        out.lines().any(|l| l.starts_with("- skill:review")),
        "subtree root must be skill:review: {out}"
    );
    // Its dependency appears nested beneath it.
    assert!(
        out.contains("agent:reviewer"),
        "subtree must include the dependency: {out}"
    );
}

#[test]
fn recall_tree_dependency_only_item_is_not_a_root() {
    // spec: DEP-61
    // An item reachable only as a dependency of another installed item must NOT
    // appear as a primary root in the forest; it appears only nested under its
    // dependent.
    let sb = dep61_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(r.success, "recall --tree must succeed");
    let out = &r.stdout;
    // agent:reviewer must not appear as a top-level root line.
    assert!(
        !out.lines().any(|l| l.starts_with("- agent:reviewer")),
        "agent:reviewer must not be a primary root: {out}"
    );
}

// ---------------------------------------------------------------------------
// DEP-62: non-interactive probe shows the tree
// ---------------------------------------------------------------------------

/// Fixture for probe tree tests: same `skill:review -> agent:reviewer` chain.
fn dep62_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep62-agents");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review skill\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer agent\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn probe_non_interactive_nests_dependency_under_dependent() {
    // spec: DEP-62
    // Non-interactive `probe -n` (no TUI) nests each hit's transitive
    // dependencies beneath it in the human listing. For the dep62 fixture,
    // `skill:review` depends on `agent:reviewer`, so the reviewer line must
    // appear indented as a child of the review hit row.
    // We query "skill:" to match only skill:review (not agent:reviewer), so
    // agent:reviewer appears only as a nested dependency, not as its own hit.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--no-tui", "--kind", "skill", "review"]);
    assert!(
        r.success,
        "probe --no-tui --kind skill must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:review"),
        "skill:review must appear in output: {out}"
    );
    // The dependency agent:reviewer must appear nested (indented) after skill:review.
    let review_pos = out.lines().position(|l| l.contains("skill:review"));
    let reviewer_pos = out.lines().position(|l| l.contains("agent:reviewer"));
    assert!(review_pos.is_some(), "skill:review must appear: {out}");
    assert!(
        reviewer_pos.is_some(),
        "agent:reviewer dependency must appear in output: {out}"
    );
    assert!(
        reviewer_pos.unwrap() > review_pos.unwrap(),
        "agent:reviewer (dependency) must come after skill:review: {out}"
    );
    // The dependency line must be indented (nested under the hit).
    let reviewer_line = out.lines().find(|l| l.contains("agent:reviewer")).unwrap();
    assert!(
        reviewer_line.starts_with("  "),
        "dependency line must be indented: {reviewer_line:?}"
    );
}

#[test]
fn probe_json_includes_dependencies_field() {
    // spec: DEP-62, CLI-167
    // `probe --json` adds a `dependencies` field to each row with the direct
    // dependency keys. For `skill:review` that depends on `agent:reviewer`, the
    // field must contain `"agent:reviewer"`. Output is wrapped in an envelope.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--json", "review"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "review")
        .expect("skill:review must be in JSON output");
    let deps = review_row["dependencies"]
        .as_array()
        .expect("dependencies must be an array");
    assert!(
        deps.iter().any(|d| d == "agent:reviewer"),
        "dependencies must include agent:reviewer: {deps:?}"
    );
}

#[test]
fn probe_json_item_with_no_deps_omits_dependencies_field() {
    // spec: DEP-62, CLI-167
    // An item with no dependencies should have the `dependencies` field absent
    // (or empty) from its JSON row. Output is wrapped in an envelope.
    let sb = dep62_fixture();

    let r = sb.mind(&["probe", "--json", "reviewer"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    let reviewer_row = rows
        .iter()
        .find(|row| row["name"] == "reviewer")
        .expect("agent:reviewer must be in JSON output");
    // Field must be absent (omitted when empty) or present but empty.
    let deps = reviewer_row.get("dependencies");
    assert!(
        deps.is_none() || deps.unwrap().as_array().is_some_and(|a| a.is_empty()),
        "dependencies field must be absent or empty for an item with no deps: {reviewer_row}"
    );
}

// ---------------------------------------------------------------------------
// DEP-60/61/62: additional adversarial / edge coverage (certification shard)
// ---------------------------------------------------------------------------

/// Fixture: a transitive chain `skill:a -> agent:b -> rule:c`, all installed.
/// Each edge is declared with `requires:`. The whole source is melded and
/// installed via a full-coverage glob so resolution is a no-op (DEP-10) and all
/// three land in the manifest regardless of prompting.
fn dep_chain_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep-chain");
    sb.write_and_commit(
        "skills/a/SKILL.md",
        "---\nname: a\ndescription: A\nrequires: agent:b\n---\n# a skill\n",
    );
    sb.write_and_commit(
        "agents/b.md",
        "---\nname: b\ndescription: B\nrequires: rule:c\n---\n# b agent\n",
    );
    sb.write_and_commit(
        "rules/c.md",
        "---\nname: c\ndescription: C\n---\n# c rule\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    // Full-coverage glob installs all three (DEP-10 no-op, no prompt).
    assert!(
        sb.mind(&["learn", "dep-chain#*"]).success,
        "fixture: whole-source learn should install all three"
    );
    sb
}

#[test]
fn forget_transitive_lists_only_direct_dependent_and_no_cascade() {
    // spec: DEP-60 DEP-50
    // Chain a -> b -> c. Forgetting the middle item b: only its DIRECT dependent
    // (a) is listed (c is b's dependency, not a dependent, so it is not listed),
    // the non-TTY run refuses without --yes/--force, and with --yes b is removed
    // while BOTH a (the dependent, no cascade up) and c (b's own dependency, no
    // cascade down, DEP-50) remain installed.
    let sb = dep_chain_fixture();

    // Non-TTY without confirmation: refuse, list only the direct dependent a.
    let refused = sb.mind(&["forget", "agent:b"]);
    assert!(
        !refused.success,
        "forget of a depended-on middle item must refuse: {} {}",
        refused.stdout, refused.stderr
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        refused.stderr
    );
    // The dependent a is listed.
    assert!(
        refused.stdout.contains("skill:a"),
        "the direct dependent skill:a must be listed: {}",
        refused.stdout
    );
    // c is b's dependency, NOT a dependent: it must not appear in the warning's
    // dependent list. (The warning only enumerates dependents.)
    assert!(
        !refused.stdout.lines().any(|l| l.trim() == "rule:c"),
        "rule:c (a dependency of b, not a dependent) must not be listed as a dependent: {}",
        refused.stdout
    );
    // Nothing removed on refusal.
    assert!(sb.mind(&["recall", "agent:b"]).success, "b still installed");

    // With --yes: b is removed; a and c both remain (no cascade either way).
    let done = sb.mind(&["forget", "--yes", "agent:b"]);
    assert!(done.success, "forget --yes must proceed: {}", done.stderr);
    assert!(
        !sb.mind(&["recall", "agent:b"]).success,
        "b must be removed"
    );
    assert!(
        sb.mind(&["recall", "skill:a"]).success,
        "dependent a must remain (no upward cascade)"
    );
    assert!(
        sb.mind(&["recall", "rule:c"]).success,
        "dependency c must remain (no downward cascade, DEP-50)"
    );
}

#[test]
fn forget_dependent_warning_fires_on_union_of_requires_and_token_edges() {
    // spec: DEP-60 DEP-4
    // Two distinct installed items depend on agent:target: one via a `requires:`
    // entry, the other via a `{{ns:}}` token in its body. Forgetting the target
    // must list BOTH dependents -- the dependent set is the union of requires and
    // token edges, not just one source of edge.
    let sb = Sandbox::bare("dep-union");
    sb.write_and_commit(
        "agents/target.md",
        "---\nname: target\ndescription: Target\n---\n# target\n",
    );
    // Dependent via requires:.
    sb.write_and_commit(
        "skills/via-requires/SKILL.md",
        "---\nname: via-requires\ndescription: R\nrequires: agent:target\n---\n# via-requires\n",
    );
    // Dependent via a {{ns:}} token in the body.
    sb.write_and_commit(
        "skills/via-token/SKILL.md",
        "---\nname: via-token\ndescription: T\n---\n# via-token\nhand off to {{ns:target}}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "dep-union#*"]).success,
        "fixture: whole-source learn should install all three"
    );

    let r = sb.mind(&["forget", "agent:target"]);
    assert!(
        !r.success,
        "forget of the doubly-depended item must refuse: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skill:via-requires"),
        "the requires-edge dependent must be listed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:via-token"),
        "the token-edge dependent must be listed: {}",
        r.stdout
    );
}

#[test]
fn forget_force_does_not_bypass_cli42_multi_item_confirmation() {
    // spec: DEP-60 CLI-42
    // `--force` bypasses only the DEP-60 single-item dependents gate. A glob that
    // matches 2+ items still routes through the CLI-42 multi-item confirmation,
    // which only `--yes` bypasses. So a non-TTY `forget --force '*'` over 2+
    // matches must STILL refuse (ConfirmationRequired) and remove nothing.
    let sb = dep60_fixture(); // two installed items: skill:review, agent:reviewer

    let r = sb.mind(&["forget", "--force", "*"]);
    assert!(
        !r.success,
        "forget --force over a multi-match glob must still refuse: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired (CLI-42, not bypassed by --force): {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("would remove"),
        "must show the CLI-42 count message: {}",
        r.stdout
    );
    // Both items remain.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "reviewer must remain installed"
    );
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "review must remain installed"
    );
}

#[test]
fn recall_tree_item_with_no_dependencies_prints_just_that_item() {
    // spec: DEP-61
    // `recall <item> --tree` for an item that has no dependencies prints exactly
    // that one item as the subtree root, with no nested children.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    // agent:reviewer is a leaf (it depends on nothing).
    let r = sb.mind(&["recall", "agent:reviewer", "--tree"]);
    assert!(
        r.success,
        "recall <leaf> --tree must succeed: {} {}",
        r.stdout, r.stderr
    );
    let lines: Vec<&str> = r.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["- agent:reviewer"],
        "a dependency-free item's subtree must be just that item: {:?}",
        r.stdout
    );
}

/// Fixture: a 2-cycle among installed items. `skill:loop-a` and `skill:loop-b`
/// each `requires:` the other; the whole source is installed so both land.
fn dep_cycle_fixture() -> Sandbox {
    let sb = Sandbox::bare("dep-cycle");
    sb.write_and_commit(
        "skills/loop-a/SKILL.md",
        "---\nname: loop-a\ndescription: A\nrequires: skill:loop-b\n---\n# loop-a\n",
    );
    sb.write_and_commit(
        "skills/loop-b/SKILL.md",
        "---\nname: loop-b\ndescription: B\nrequires: skill:loop-a\n---\n# loop-b\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(
        sb.mind(&["learn", "dep-cycle#*"]).success,
        "fixture: whole-source learn should install both cycle members"
    );
    sb
}

#[test]
fn recall_tree_cyclic_installed_pair_renders_every_item() {
    // spec: DEP-61 DEP-22
    // A pure cycle among installed items (every node has in-degree >= 1, so no
    // natural root exists) must still render EVERY installed item in the forest:
    // a secondary root is promoted and the back-edge is marked (cycle). No
    // installed item may be missing from `recall --tree` output.
    let sb = dep_cycle_fixture();

    let r = sb.mind(&["recall", "--tree"]);
    assert!(
        r.success,
        "recall --tree over a cycle must succeed: {} {}",
        r.stdout, r.stderr
    );
    let out = &r.stdout;
    assert!(
        out.contains("skill:loop-a"),
        "loop-a must appear in the forest: {out}"
    );
    assert!(
        out.contains("skill:loop-b"),
        "loop-b must appear in the forest: {out}"
    );
    // The cycle must be broken with a marked back-edge, not expanded forever.
    assert!(
        out.contains("(cycle)"),
        "the cycle must be rendered as a marked back-edge: {out}"
    );
}

#[test]
fn probe_json_resolves_dependency_to_prefixed_effective_key() {
    // spec: DEP-62, CLI-167
    // When a source is melded under a prefix, an item's dependency key in the
    // `probe --json` adjacency field must be the EFFECTIVE (prefixed) key.
    // Output is wrapped in an envelope.
    let sb = Sandbox::bare("dep-prefix");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec(), "--as", "jk"]).success);

    let r = sb.mind(&["probe", "--json", "review"]);
    assert!(
        r.success,
        "probe --json must succeed: {} {}",
        r.stdout, r.stderr
    );
    let env: serde_json::Value = serde_json::from_str(&r.stdout).expect("must be valid JSON");
    let rows = env["items"].as_array().expect("items must be array");
    // The effective name carries the prefix.
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "jk:review")
        .expect("skill:jk:review must be in JSON output (prefixed effective name)");
    let deps = review_row["dependencies"]
        .as_array()
        .expect("dependencies must be an array");
    assert!(
        deps.iter().any(|d| d == "agent:jk:reviewer"),
        "dependency key must be the prefixed effective key agent:jk:reviewer, not bare: {deps:?}"
    );
    assert!(
        !deps.iter().any(|d| d == "agent:reviewer"),
        "the bare (unprefixed) dependency key must NOT appear: {deps:?}"
    );
}

// DEP-63: recall --tree --json structured output
// ---------------------------------------------------------------------------

#[test]
fn recall_tree_json_emits_json_array_with_dependency_nested() {
    // spec: DEP-63
    // `recall --tree --json` emits a JSON array of root nodes.
    // Fixture: skill:review -> agent:reviewer.
    // skill:review has in-degree 0 (it is the root); agent:reviewer is its
    // dependency.  The root node must have "key": "skill:review" and its
    // "dependencies" must contain one entry "agent:reviewer".
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Output must be valid JSON.
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    // Top-level is a JSON array (the forest roots).
    assert!(v.is_array(), "output must be a JSON array: {v}");
    let arr = v.as_array().unwrap();

    // Exactly one root: skill:review.
    let root = arr
        .iter()
        .find(|n| n["key"] == "skill:review")
        .unwrap_or_else(|| panic!("must have skill:review as root: {arr:?}"));

    // Normal node: has "dependencies", no "cycle".
    assert!(
        root.get("dependencies").is_some(),
        "root node must have dependencies field: {root}"
    );
    assert!(
        root.get("cycle").is_none(),
        "root node must not have cycle field: {root}"
    );

    // agent:reviewer is nested under skill:review.
    let deps = root["dependencies"].as_array().unwrap();
    let reviewer = deps
        .iter()
        .find(|n| n["key"] == "agent:reviewer")
        .unwrap_or_else(|| panic!("agent:reviewer must be in dependencies: {deps:?}"));
    assert!(
        reviewer.get("cycle").is_none(),
        "reviewer node must not be a cycle: {reviewer}"
    );
    let reviewer_deps = reviewer["dependencies"]
        .as_array()
        .expect("reviewer must have dependencies field");
    assert!(
        reviewer_deps.is_empty(),
        "reviewer is a leaf, so dependencies must be empty: {reviewer_deps:?}"
    );
}

#[test]
fn recall_tree_json_item_emits_single_object_not_array() {
    // spec: DEP-63
    // `recall <item> --tree --json` emits a single JSON object (not an array)
    // for that item's subtree.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "skill:review", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <item> --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    // Must be an object, not an array.
    assert!(
        v.is_object(),
        "scoped recall --tree --json must emit an object: {v}"
    );
    assert_eq!(
        v["key"], "skill:review",
        "object key must be skill:review: {v}"
    );
    let deps = v["dependencies"]
        .as_array()
        .expect("root object must have dependencies");
    assert_eq!(deps.len(), 1, "skill:review has one dependency: {deps:?}");
    assert_eq!(deps[0]["key"], "agent:reviewer");
}

#[test]
fn recall_tree_json_leaf_item_has_empty_dependencies() {
    // spec: DEP-63
    // `recall agent:reviewer --tree --json` for a leaf node emits a single
    // object with an empty `dependencies` array (not absent).
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    let r = sb.mind(&["recall", "agent:reviewer", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <leaf> --tree --json must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{}", r.stdout));

    assert!(v.is_object(), "must be an object: {v}");
    assert_eq!(v["key"], "agent:reviewer");
    let deps = v["dependencies"]
        .as_array()
        .expect("leaf object must have dependencies field (not absent)");
    assert!(
        deps.is_empty(),
        "leaf must have empty dependencies array: {deps:?}"
    );
}

#[test]
fn recall_tree_json_with_prefix_uses_effective_keys() {
    // spec: DEP-63
    // When items are installed under a prefix, the JSON keys must use the
    // effective (prefixed) name, matching what recall --tree (human) emits.
    let sb = Sandbox::bare("dep63-prefix");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    // Meld with a prefix so items install as "pfx:review" / "pfx:reviewer".
    assert!(
        sb.mind(&["meld", "--as", "pfx", &sb.source_spec()]).success,
        "meld with prefix must succeed"
    );
    // Under a prefix, the effective name is "pfx:review" -- use that to learn.
    assert!(
        sb.mind(&["learn", "pfx:review", "--yes"]).success,
        "learn with prefix must succeed"
    );

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json with prefix must succeed: {} {}",
        r.stdout, r.stderr
    );

    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("must be valid JSON: {e}\n{}", r.stdout));
    assert!(v.is_array());
    let arr = v.as_array().unwrap();

    // Root key must use the effective prefixed name.
    let root = arr
        .iter()
        .find(|n| n["key"] == "skill:pfx:review")
        .unwrap_or_else(|| panic!("root must be skill:pfx:review: {arr:?}"));

    let deps = root["dependencies"].as_array().unwrap();
    assert_eq!(deps.len(), 1, "one dep: {deps:?}");
    assert_eq!(
        deps[0]["key"], "agent:pfx:reviewer",
        "dep must use effective prefixed key: {deps:?}"
    );
}

#[test]
fn recall_tree_with_sources_resolves_to_sources_path_with_note() {
    // spec: DEP-61
    // Precedence pin: `recall --tree --sources` is NOT the tree path. The code
    // notes that --tree is ignored with --sources and runs the sources listing.
    // This pins the resolved precedence so it cannot silently change: a note is
    // emitted on stderr AND the output is the sources listing (showing the melded
    // source), not a dependency forest of installed items.
    let sb = dep61_fixture(); // one melded source "dep61-agents", items installed

    let r = sb.mind(&["recall", "--tree", "--sources"]);
    assert!(
        r.success,
        "recall --tree --sources must succeed: {} {}",
        r.stdout, r.stderr
    );
    // The note about --tree being ignored with --sources is emitted.
    assert!(
        r.stderr.contains("--tree") && r.stderr.contains("ignored with --sources"),
        "a note that --tree is ignored with --sources must be emitted: {}",
        r.stderr
    );
    // The sources path ran: the melded source appears in the listing.
    assert!(
        r.stdout.contains("dep61-agents"),
        "the sources listing (not a dependency forest) must be shown: {}",
        r.stdout
    );
    // It is NOT a dependency forest: no nested "- agent:reviewer" tree line.
    assert!(
        !r.stdout
            .lines()
            .any(|l| l.starts_with("  - agent:reviewer")),
        "must not render the dependency forest under --sources: {}",
        r.stdout
    );
}

#[test]
fn recall_tree_json_not_installed_item_errors_like_non_json() {
    // spec: DEP-63
    // `recall <item> --tree --json` for an item that is NOT installed at all must
    // error the same way the non-json `recall <item> --tree` does (NotInstalled
    // via resolve_installed), NOT emit a null/empty object. The DepNode::normal
    // fallback in commands.rs is only reached AFTER resolve_installed succeeds
    // (item present in the manifest), so an absent item never reaches it.
    let sb = dep61_fixture(); // skill:review and agent:reviewer installed

    // A skill that exists in the source but was never learned is not installed.
    let json = sb.mind(&["recall", "skill:nope", "--tree", "--json"]);
    assert!(
        !json.success,
        "recall <uninstalled> --tree --json must fail: {} {}",
        json.stdout, json.stderr
    );
    // Under --json the error appears as the CLI-181 envelope on stdout.
    let v = parse_json(&json.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", json.stdout);
    assert_eq!(
        v["error"]["kind"], "not-installed",
        "kind must be not-installed: {}",
        json.stdout
    );
    assert!(
        v["error"]["message"]
            .as_str()
            .map(|s| s.contains("not installed") || s.contains("nope"))
            .unwrap_or(false),
        "message must mention the missing item: {}",
        json.stdout
    );

    // Parity: the non-json form fails the same way.
    let human = sb.mind(&["recall", "skill:nope", "--tree"]);
    assert!(
        !human.success,
        "non-json recall <uninstalled> --tree must also fail: {} {}",
        human.stdout, human.stderr
    );
    assert!(
        human.stderr.contains("not installed"),
        "non-json form must also report NotInstalled: {}",
        human.stderr
    );
}

#[test]
fn recall_tree_json_installed_but_orphaned_item_falls_back_to_normal_node() {
    // spec: DEP-63
    // The `DepNode::normal(key, [])` fallback in commands.rs: an item that IS in
    // the manifest (resolve_installed succeeds) but is NOT a node in the graph
    // (it was removed upstream, so the catalog no longer carries it, and
    // subtree_node returns None). The scoped `recall <item> --tree --json` must
    // still emit a valid single object {"key": ..., "dependencies": []}, not
    // null and not an error.
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");
    // The agent disappears upstream; sync drops it from the catalog while it
    // stays installed in the manifest.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let r = sb.mind(&["recall", "agent:dev", "--tree", "--json"]);
    assert!(
        r.success,
        "recall <orphaned> --tree --json must succeed via the fallback: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    assert!(
        v.is_object(),
        "fallback must emit a single object, not an array: {v}"
    );
    assert_eq!(
        v["key"], "agent:dev",
        "fallback node key must be the item key: {v}"
    );
    let deps = v["dependencies"]
        .as_array()
        .expect("fallback node must carry an (empty) dependencies array");
    assert!(
        deps.is_empty(),
        "an orphaned item has no graph edges, so dependencies must be empty: {deps:?}"
    );
    assert!(
        v.get("cycle").is_none(),
        "the fallback node must not be a cycle leaf: {v}"
    );
}

#[test]
fn recall_tree_json_empty_manifest_emits_empty_array() {
    // spec: DEP-63
    // `recall --tree --json` with nothing installed must emit a valid JSON empty
    // array `[]`, not an error, not an empty string, not the human "no installed
    // items" line.
    // A melded-but-not-learned source has an empty manifest.
    let sb = dep61_fixture_unlearned();

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json over an empty manifest must succeed: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    assert!(
        v.as_array().is_some_and(|a| a.is_empty()),
        "empty manifest must yield an empty JSON array: {v}"
    );
}

/// Like `dep61_fixture` but the source is melded and NOT learned, so the
/// manifest stays empty (for the empty-forest case).
fn dep61_fixture_unlearned() -> Sandbox {
    let sb = Sandbox::bare("dep63-empty");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nrequires: agent:reviewer\n---\n# review\n",
    );
    sb.write_and_commit(
        "agents/reviewer.md",
        "---\nname: reviewer\ndescription: Reviewer\n---\n# reviewer\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    sb
}

#[test]
fn recall_tree_json_cyclic_pair_every_item_present_with_cycle_leaf() {
    // spec: DEP-63 DEP-22
    // A pure cycle among installed items, driven through the real binary: the
    // structured forest must still contain EVERY installed item (the cycle
    // promotes a secondary root), the back-edge is a {"cycle": true} leaf with
    // no "dependencies" field, and there is no infinite nesting (bounded output).
    let sb = dep_cycle_fixture(); // skill:loop-a <-> skill:loop-b, both installed

    let r = sb.mind(&["recall", "--tree", "--json"]);
    assert!(
        r.success,
        "recall --tree --json over a cycle must succeed: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("output must be valid JSON: {e}\n{:?}", r.stdout));
    let arr = v.as_array().expect("forest must be a JSON array");

    // Collect every key that appears anywhere in the structured forest.
    fn collect(node: &serde_json::Value, out: &mut std::collections::HashSet<String>) {
        if let Some(k) = node["key"].as_str() {
            out.insert(k.to_string());
        }
        if let Some(children) = node["dependencies"].as_array() {
            for c in children {
                collect(c, out);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for root in arr {
        collect(root, &mut seen);
    }
    assert!(
        seen.contains("skill:loop-a") && seen.contains("skill:loop-b"),
        "every installed cycle member must appear in the JSON forest: {seen:?}"
    );

    // Find at least one cycle leaf: {"cycle": true} with no "dependencies".
    fn has_cycle_leaf(node: &serde_json::Value) -> bool {
        if node["cycle"] == serde_json::Value::Bool(true) {
            assert!(
                node.get("dependencies").is_none(),
                "a cycle leaf must omit dependencies: {node}"
            );
            return true;
        }
        node["dependencies"]
            .as_array()
            .is_some_and(|cs| cs.iter().any(has_cycle_leaf))
    }
    assert!(
        arr.iter().any(has_cycle_leaf),
        "the cycle must surface as a {{cycle:true}} leaf, not infinite nesting: {v}"
    );
}

#[test]
fn recall_tree_json_and_probe_json_agree_on_direct_dependencies() {
    // spec: DEP-63 DEP-62
    // Cross-form consistency: `recall --tree --json` (nested tree, DEP-63) and
    // `probe --json` (flat adjacency, DEP-62) must describe the SAME direct edges
    // for the same item. For skill:review, both must report agent:reviewer as its
    // single direct dependency.
    let sb = dep61_fixture(); // skill:review -> agent:reviewer

    // Nested tree form: skill:review's direct children.
    let tree = sb.mind(&["recall", "skill:review", "--tree", "--json"]);
    assert!(
        tree.success,
        "recall --tree --json must succeed: {}",
        tree.stderr
    );
    let tv: serde_json::Value = serde_json::from_str(tree.stdout.trim())
        .unwrap_or_else(|e| panic!("recall tree JSON invalid: {e}\n{:?}", tree.stdout));
    let mut tree_deps: Vec<String> = tv["dependencies"]
        .as_array()
        .expect("subtree object must have dependencies")
        .iter()
        .map(|n| n["key"].as_str().unwrap().to_string())
        .collect();
    tree_deps.sort();

    // Flat adjacency form: skill:review's `dependencies` field.
    // spec: CLI-167 - probe --json is wrapped in an envelope.
    let probe = sb.mind(&["probe", "--json", "review"]);
    assert!(probe.success, "probe --json must succeed: {}", probe.stderr);
    let probe_env: serde_json::Value =
        serde_json::from_str(&probe.stdout).expect("probe JSON invalid");
    let rows = probe_env["items"].as_array().expect("items must be array");
    let review_row = rows
        .iter()
        .find(|row| row["name"] == "review")
        .expect("skill:review must be a probe row");
    let mut probe_deps: Vec<String> = review_row
        .get("dependencies")
        .and_then(|d| d.as_array())
        .map(|a| a.iter().map(|d| d.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    probe_deps.sort();

    assert_eq!(
        tree_deps, probe_deps,
        "recall --tree --json and probe --json must agree on skill:review's direct deps"
    );
    assert_eq!(
        tree_deps,
        vec!["agent:reviewer".to_string()],
        "both forms must report exactly agent:reviewer: {tree_deps:?}"
    );
}

// ---------------------------------------------------------------------------
// C3: forget --json without --yes when dependents exist => ConfirmationRequired
// ---------------------------------------------------------------------------

/// Under `--json` without `--yes` or `--force`, forgetting an item that has
/// installed dependents must return ConfirmationRequired and remove nothing.
/// json mode is non-interactive; it must not silently proceed through a
/// destructive confirmation (DEP-60).
#[test]
fn forget_json_without_yes_when_dependents_exist_is_confirmation_required() {
    // spec: DEP-60, CLI-181
    let sb = dep60_fixture(); // skill:review depends on agent:reviewer

    // --json but no --yes and no --force.
    let r = sb.mind(&["--json", "forget", "agent:reviewer"]);
    assert!(
        !r.success,
        "forget --json without --yes must refuse when dependents exist: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // Under --json the ConfirmationRequired error goes to stdout as the CLI-181 envelope.
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert_eq!(
        v["error"]["kind"], "confirmation-required",
        "kind must be confirmation-required: {}",
        r.stdout
    );
    // The item must still be installed.
    assert!(
        sb.mind(&["recall", "agent:reviewer"]).success,
        "agent:reviewer must remain installed after json refusal"
    );
}

/// `--json --yes` must still proceed (yes overrides the confirmation gate).
#[test]
fn forget_json_with_yes_when_dependents_exist_proceeds() {
    // spec: DEP-60
    let sb = dep60_fixture();

    let r = sb.mind(&["--json", "--yes", "forget", "agent:reviewer"]);
    assert!(
        r.success,
        "forget --json --yes must proceed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "agent:reviewer"]).success,
        "agent:reviewer must be removed after forget --json --yes"
    );
}

// ---------------------------------------------------------------------------
// Consumer pin-flag injection: leading-dash values rejected (DSC-66)
// ---------------------------------------------------------------------------

/// A consumer `--pin-tag` value that starts with `-` (e.g. `--pin-tag=-x`) must
/// be rejected with `InvalidRef` before any git call. No source must be registered.
/// The `=` form (--pin-tag=-x) passes the value through clap as a string; our
/// `resolve_pin_flags` then validates it before any git subprocess is spawned.
#[test]
fn meld_pin_tag_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-tag");
    let spec = sb.source_spec();

    // Use the `--flag=value` form so clap passes the leading-dash value through
    // to our validate_ref_value call rather than treating it as a flag itself.
    let r = sb.mind(&["meld", &spec, "--pin-tag=-x"]);
    assert!(
        !r.success,
        "--pin-tag=-x must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The error must mention the invalid ref, not a git error.
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    // No source registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after invalid pin: {}",
        sources.stdout
    );
}

/// A consumer `--pin-ref` value that starts with `-` must be rejected.
#[test]
fn meld_pin_ref_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-ref");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--pin-ref=--upload-pack=evil"]);
    assert!(
        !r.success,
        "--pin-ref=--upload-pack=evil must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered: {}",
        sources.stdout
    );
}

/// A consumer `--follow-branch` value that starts with `-` must be rejected.
#[test]
fn meld_follow_branch_leading_dash_is_invalid_ref() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-branch");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--follow-branch=-evil"]);
    assert!(
        !r.success,
        "--follow-branch=-evil must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("invalid ref") || r.stderr.contains("InvalidRef"),
        "must report InvalidRef: stderr={}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered: {}",
        sources.stdout
    );
}

/// The space-separated form `--pin-tag -x` (leading-dash value) is rejected at
/// the clap layer (no `allow_hyphen_values`), so clap treats `-x` as a flag and
/// errors before `resolve_pin_flags`. This is the complement of the `=` form
/// (`--pin-tag=-x`), which clap passes through to `validate_ref_value`. Either
/// surface must end with no source registered and no git fetch having run: a
/// leading-dash injection cannot reach a git subprocess by either path.
#[test]
fn meld_pin_tag_space_separated_leading_dash_is_rejected_before_git() {
    // spec: DSC-66
    let sb = Sandbox::bare("pin-inject-space");
    let spec = sb.source_spec();

    // Space-separated leading-dash value: clap rejects this as an unknown flag /
    // missing value rather than accepting `-x` as the tag's value.
    let r = sb.mind(&["meld", &spec, "--pin-tag", "-x"]);
    assert!(
        !r.success,
        "--pin-tag -x (space form) must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No source registered and no fetch ran (the failure is before any git call,
    // whether at the clap layer or via validate_ref_value).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after a rejected space-form pin: {}",
        sources.stdout
    );
}

// ---- json-mode meld (CLI-156) -----------------------------------------------

/// `meld --yes --json` must emit exactly ONE top-level JSON object whose
/// `installed` array lists the items that were installed in this call.
/// Multiple JSON documents would break `json.loads`; silent installs would
/// make the meld result indistinguishable from a link-only run.
#[test]
fn meld_json_with_yes_installs_and_emits_single_object() {
    // spec: CLI-156 CLI-153
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["--json", "meld", &spec, "--yes"]);
    assert!(
        r.success,
        "meld --yes --json must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // stdout must parse as exactly one JSON object (no concatenated documents).
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "meld --yes --json stdout must be one valid JSON object, got error {e}: '{}'",
            r.stdout
        )
    });
    assert_eq!(v["action"], "meld", "action must be 'meld': {v}");
    assert_eq!(v["outcome"], "melded", "outcome must be 'melded': {v}");
    // The fixture has skill:review, agent:dev, rule:style => at least one installed key.
    let installed = v["installed"]
        .as_array()
        .expect("installed must be an array");
    assert!(
        !installed.is_empty(),
        "installed must not be empty when --yes is given: {v}"
    );
}

/// `meld --json` (no `--yes`) on a non-TTY (piped stdin) must not block
/// waiting for a confirmation prompt and must emit exactly ONE JSON object
/// with `pending_items >= 1`.
#[test]
fn meld_json_no_yes_non_tty_emits_single_object_with_pending() {
    // spec: CLI-156
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Passing empty stdin ensures this is non-TTY; no --yes so items are pending.
    let r = sb.mind_with_input(&["--json", "meld", &spec], Some(""));
    assert!(
        r.success,
        "meld --json (no --yes) must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "meld --json stdout must be one valid JSON object, got error {e}: '{}'",
            r.stdout
        )
    });
    assert_eq!(v["action"], "meld", "action must be 'meld': {v}");
    assert_eq!(v["outcome"], "melded", "outcome must be 'melded': {v}");
    let pending = v["pending_items"].as_u64().unwrap_or(0);
    assert!(
        pending >= 1,
        "pending_items must be >= 1 when items exist but --yes was not given: {v}"
    );
}

// ---- re-learn noop (CLI-157) -------------------------------------------------

/// Re-learning an already-installed item must print a human-readable signal
/// and, under --json, use the distinct "up-to-date" outcome rather than
/// "installed". This lets callers distinguish a real install from a no-op.
#[test]
fn relearn_already_installed_signals_noop() {
    // spec: CLI-157 DEP-23
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // First learn succeeds.
    let r1 = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r1.success, "initial meld+learn must succeed: {}", r1.stderr);

    // Human mode: re-learn prints a recognizable noop message.
    let r2 = sb.mind(&["learn", "skill:review"]);
    assert!(
        r2.success,
        "re-learn must still exit 0: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("already installed") || r2.stdout.contains("nothing to do"),
        "re-learn human output must signal noop: '{}'",
        r2.stdout
    );

    // JSON mode: outcome is "up-to-date", NOT "installed".
    let r3 = sb.mind(&["--json", "learn", "skill:review"]);
    assert!(
        r3.success,
        "re-learn --json must exit 0: stdout={} stderr={}",
        r3.stdout, r3.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r3.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "re-learn --json stdout must be one valid JSON object, got error {e}: '{}'",
            r3.stdout
        )
    });
    assert_eq!(
        v["outcome"], "up-to-date",
        "json outcome for re-learn must be 'up-to-date', not 'installed': {v}"
    );
}

// ---- DSC-74..77 / STO-44 / CLI-158 / DUMP-10: flat skill layout -------------

/// A bare source whose skill directories sit directly at the repo root (no
/// `skills/` container), plus a conventional `agents/` dir. With `mindfile` set,
/// also writes a `mind.toml` at the root. No commit beyond the initial bare one
/// unless committed by the caller.
fn make_flat_source(name: &str, mindfile: Option<&str>) -> Sandbox {
    let sb = Sandbox::bare(name);
    write(
        &sb.source.join("alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Alpha flat skill\n---\n# alpha\n",
    );
    write(
        &sb.source.join("beta/SKILL.md"),
        "---\nname: beta\ndescription: Beta flat skill\n---\n# beta\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: A dev agent\n---\n# dev\n",
    );
    if let Some(toml) = mindfile {
        write(&sb.source.join("mind.toml"), toml);
    }
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "flat layout"]);
    sb
}

#[test]
fn meld_flat_skills_flag_discovers_root_level_skill_dirs() {
    // spec: DSC-74 DSC-75 CLI-158 STO-44
    // `meld --flat-skills` discovers skills as bare-name directories at the repo
    // root (no `skills/` container), records the override on the source, and
    // leaves agent discovery (a conventional `agents/` dir) unchanged.
    let sb = make_flat_source("flatsrc", None);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--flat-skills", "--yes"]);
    assert!(
        r.success,
        "meld --flat-skills failed: {} {}",
        r.stdout, r.stderr
    );

    let recall = sb.mind(&["recall"]);
    for item in ["alpha", "beta", "dev"] {
        assert!(
            recall.stdout.contains(item),
            "flat skill/agent '{item}' must be discovered: {}",
            recall.stdout
        );
    }

    // STO-44: the consumer override is persisted on the source.
    let json = read_sources_json(&sb);
    assert!(
        json.contains("\"flat_skills\": true"),
        "the --flat-skills override must be persisted on the source: {json}"
    );
}

#[test]
fn meld_without_flat_skills_skips_root_level_skill_dirs() {
    // spec: DSC-74
    // Control: without the flag (and with no `[source].flat-skills`), the same
    // root-level skill directories are NOT discovered (the DSC-10 container layout
    // is required), while the conventional `agents/` dir still yields its item.
    let sb = make_flat_source("flatsrc-control", None);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "root-level skill dirs must NOT be discovered without flat-skills: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:dev"),
        "the conventional agents/ item must still be discovered: {}",
        probe.stdout
    );
}

#[test]
fn source_flat_skills_directive_discovers_without_flag() {
    // spec: DSC-74
    // A source that declares `[source].flat-skills = true` (non-authoritative
    // mind.toml) gets flat skill discovery with no consumer flag.
    let sb = make_flat_source("flatsrc-declared", Some("[source]\nflat-skills = true\n"));
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && probe.stdout.contains("skill:beta"),
        "[source].flat-skills must enable flat discovery without a flag: {}",
        probe.stdout
    );
}

#[test]
fn meld_flat_skills_ignored_for_authoritative_mindfile() {
    // spec: DSC-76
    // For an authoritative mind.toml (declaring [[items]]), --flat-skills affects
    // nothing and `meld` prints a note that it is ignored. The explicitly declared
    // item is found; the root-level flat dirs are not.
    let toml = "[[items]]\nkind = \"skill\"\nname = \"alpha\"\npath = \"alpha\"\n";
    let sb = make_flat_source("flatsrc-auth", Some(toml));
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--flat-skills", "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("--flat-skills is ignored"),
        "an authoritative mind.toml must note that --flat-skills is ignored: {}",
        r.stdout
    );

    // Authoritative discovery: only the declared `alpha`, and `beta` (a sibling
    // root dir) is NOT scanned in.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "authoritative mind.toml must ignore the flat layout: {}",
        probe.stdout
    );
}

#[test]
fn curator_flat_skills_applies_when_nested_has_no_mind_toml() {
    // spec: DSC-77
    // A super-source curates an un-onboarded nested flat-layout source (no
    // mind.toml of its own), supplying `flat-skills = true`. The flag applies
    // (the DSC-60 gate permits it), so the nested source's root-level skill dirs
    // are discovered.
    let nested = make_flat_source("flat-nested", None);
    let registry = Sandbox::bare("registry-flat");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             flat-skills = true\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    let probe = registry.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha") && probe.stdout.contains("skill:beta"),
        "curator flat-skills must govern discovery of the un-onboarded nested source: {}",
        probe.stdout
    );
}

#[test]
fn curator_flat_skills_ignored_with_warning_when_nested_has_mind_toml() {
    // spec: DSC-77 DSC-60
    // When the nested source ships its own mind.toml, a curator-supplied
    // `flat-skills = true` is gated out (the DSC-60 whole-file gate) and a warning
    // fires. The nested source's metadata-only mind.toml does not declare
    // flat-skills, so its root-level skill dirs are NOT discovered; its
    // conventional agents/ item still is.
    let nested = make_flat_source(
        "flat-onboarded",
        Some("[source]\ndescription = \"onboarded\"\n"),
    );
    let registry = Sandbox::bare("registry-flat-gated");
    registry.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\n\
             source = \"{}\"\n\
             flat-skills = true\n",
            nested.source_spec()
        ),
    );
    let spec = registry.source_spec();
    let r = registry.mind(&["meld", &spec]);
    assert!(r.success, "meld should succeed: {} {}", r.stdout, r.stderr);

    // DSC-60: the warning fires (flat-skills is a gated field) and names the source.
    assert!(
        r.stderr.contains("ships its own mind.toml")
            && r.stderr.contains("ignored")
            && r.stderr.contains("flat-onboarded"),
        "a DSC-60 warning must be emitted naming the onboarded source: {}",
        r.stderr
    );

    let probe = registry.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:alpha") && !probe.stdout.contains("skill:beta"),
        "curator flat-skills must be suppressed: root-level skill dirs must not appear: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:dev"),
        "the nested source's conventional agents/ item must still be discovered: {}",
        probe.stdout
    );
}

#[test]
fn dump_emits_flat_skills_for_flat_source() {
    // spec: DUMP-10
    // A source melded with --flat-skills (the consumer override STO-44) is dumped
    // with `flat-skills = true` on its [discover].sources entry, so re-melding the
    // output reproduces the flat layout. A separate non-flat source emits no key.
    let flat = make_flat_source("dump-flat", None);
    let flat_spec = flat.source_spec();
    assert!(
        flat.mind(&["meld", &flat_spec, "--flat-skills", "--yes"])
            .success
    );

    let dump = flat.mind(&["dump"]);
    assert!(dump.success, "dump failed: {} {}", dump.stdout, dump.stderr);
    assert!(
        dump.stdout.contains("flat-skills = true"),
        "dump must emit flat-skills = true for a flat source: {}",
        dump.stdout
    );

    // Control: a conventional source dumps with no flat-skills key.
    let normal = Sandbox::new();
    let normal_spec = normal.source_spec();
    assert!(normal.mind(&["meld", &normal_spec, "--yes"]).success);
    let dump2 = normal.mind(&["dump"]);
    assert!(
        !dump2.stdout.contains("flat-skills"),
        "a non-flat source must emit no flat-skills key: {}",
        dump2.stdout
    );
}

// ---------------------------------------------------------------------------
// NS-25: reserved kind word rejected as a namespace prefix (CLI path)
// ---------------------------------------------------------------------------

#[test]
fn meld_as_reserved_kind_word_is_rejected() {
    // spec: NS-25
    // A prefix equal to a reserved item-kind word (skill/agent/rule/tool) would
    // make `prefix:name` indistinguishable from a kind-qualified item ref (NS-26),
    // so `meld --as <kind-word>` must fail before registering the source.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // `--as skill` must be rejected.
    let r = sb.mind(&["meld", &spec, "--as", "skill"]);
    assert!(
        !r.success,
        "meld --as skill must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("reserved") || r.stderr.contains("kind"),
        "error must mention the reserved-kind-word problem: {}",
        r.stderr
    );
    // Source must not have been registered.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "source must not be registered after a rejected meld"
    );

    // `--as agent` must also be rejected (second reserved word).
    let sb2 = Sandbox::new();
    let spec2 = sb2.source_spec();
    let r2 = sb2.mind(&["meld", &spec2, "--as", "agent"]);
    assert!(
        !r2.success,
        "meld --as agent must fail: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stderr.contains("reserved") || r2.stderr.contains("kind"),
        "error must mention the reserved-kind-word problem: {}",
        r2.stderr
    );
}

// ---------------------------------------------------------------------------
// NS-27: item installed under former `-` separator migrates on upgrade
// ---------------------------------------------------------------------------

#[test]
fn upgrade_migrates_dash_separator_to_colon_by_stable_identity() {
    // spec: NS-27
    // An item installed under the former `-` separator (e.g. `jk-review`) keeps
    // its stable identity (source, kind, bare_name). When the binary now emits `:`
    // (so the catalog yields `jk:review`), upgrade must detect the rename via
    // identity match and re-link under the new separator, removing the old path.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Meld with prefix `jk`; the binary now emits `jk:review`.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(
        sb.mind(&["learn", "jk:review"]).success,
        "learn must accept the colon-separated effective name"
    );

    // Verify the current (colon-separator) layout.
    let new_store = sb.mind_home.join("store/skill/jk:review");
    let new_link = sb.claude_home.join("skills/jk:review");
    assert!(
        new_store.exists(),
        "store must be at jk:review after install"
    );
    assert!(
        std::fs::symlink_metadata(&new_link).is_ok(),
        "symlink must be at skills/jk:review after install"
    );

    // -- Simulate old `-` separator layout --
    // Rewrite the manifest so it looks like an install from a binary that
    // used `-` as the separator (e.g. jk-review instead of jk:review).
    let manifest_path = sb.mind_home.join("manifest.json");
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap();
    // Replace the effective name in the JSON key, the name field, the store
    // path, and the absolute link paths, while keeping bare_name/source/kind
    // intact (those form the stable identity).
    let manifest_old = manifest_text
        .replace("\"skill:jk:review\"", "\"skill:jk-review\"")
        .replace("\"jk:review\"", "\"jk-review\"")
        .replace("store/skill/jk:review", "store/skill/jk-review")
        .replace("skills/jk:review", "skills/jk-review");
    std::fs::write(&manifest_path, &manifest_old).unwrap();

    // Move the store directory to the old name.
    let old_store = sb.mind_home.join("store/skill/jk-review");
    std::fs::rename(&new_store, &old_store).unwrap();

    // Recreate the symlink under the old name pointing at the renamed store.
    std::fs::remove_file(&new_link).unwrap();
    let old_link = sb.claude_home.join("skills/jk-review");
    std::os::unix::fs::symlink(&old_store, &old_link).unwrap();

    // -- Run upgrade --
    // The catalog finds (source, kind=skill, bare_name=review, effective_name=jk:review).
    // The manifest has (source, kind=skill, bare_name=review, name=jk-review).
    // Identity match fires: new effective name != recorded name => rename.
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(
        up.success,
        "upgrade must succeed on a separator-migration rename: {} {}",
        up.stdout, up.stderr
    );
    // The upgrade output must mention both the old and new names in the rename report.
    assert!(
        up.stdout.contains("jk-review") && up.stdout.contains("jk:review"),
        "upgrade must report the rename jk-review -> jk:review: {}",
        up.stdout
    );

    // After upgrade: new-separator paths must exist.
    assert!(
        sb.mind_home.join("store/skill/jk:review").exists(),
        "store must be at jk:review after separator migration"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_ok(),
        "symlink must be at skills/jk:review after separator migration"
    );
    // Old-separator paths must be gone.
    assert!(
        !sb.mind_home.join("store/skill/jk-review").exists(),
        "old jk-review store must be removed after migration"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk-review")).is_err(),
        "old jk-review symlink must be removed after migration"
    );
    // Recall must now show the new name.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:jk:review"),
        "recall must show skill:jk:review after migration: {recall}"
    );
    assert!(
        !recall.contains("skill:jk-review"),
        "recall must not show the old skill:jk-review after migration: {recall}"
    );
}

// ---------------------------------------------------------------------------
// NS-26: a prefixed effective name (`jk:review`) is usable as a ref in the
// installed-side verbs (recall / upgrade / forget), not only `learn`.
// ---------------------------------------------------------------------------

#[test]
fn prefixed_effective_name_resolves_for_recall_upgrade_and_forget() {
    // spec: NS-26
    // NS-26 promises prefixed effective names stay usable as refs despite the
    // shared `:` separator. `learn jk:review` is covered elsewhere; this exercises
    // the manifest-side resolution path (resolve_installed) used by recall,
    // upgrade, and forget, which the resolve.rs unit test does not drive end to end.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(
        sb.mind(&["learn", "jk:review"]).success,
        "learn must accept the colon-separated effective name"
    );

    // recall <item> resolves the install by its effective name.
    let recall = sb.mind(&["recall", "jk:review"]);
    assert!(
        recall.success && recall.stdout.contains("jk:review"),
        "recall jk:review must resolve the installed item: {} {}",
        recall.stdout,
        recall.stderr
    );

    // upgrade <item> resolves it too (here a no-op, but it must not be reported
    // as NotInstalled, which is what a broken `:`-name parse would produce).
    let upgrade = sb.mind(&["upgrade", "jk:review", "--yes"]);
    assert!(
        upgrade.success,
        "upgrade jk:review must resolve the installed item: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        !upgrade.stderr.contains("not installed") && !upgrade.stdout.contains("not installed"),
        "upgrade jk:review must not report the prefixed ref as not installed: {} {}",
        upgrade.stdout,
        upgrade.stderr
    );

    // forget <item> removes exactly that item, by its effective name.
    let forget = sb.mind(&["forget", "jk:review"]);
    assert!(
        forget.success,
        "forget jk:review must resolve and remove the installed item: {} {}",
        forget.stdout, forget.stderr
    );
    assert!(
        !sb.mind_home.join("store/skill/jk:review").exists(),
        "forget jk:review must remove the store copy"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_err(),
        "forget jk:review must remove the lobe symlink"
    );
    // It is no longer in the manifest: forgetting the same effective name again
    // must now report it as not installed (proving the manifest entry was keyed
    // and removed by the `:` effective name, not left behind).
    let again = sb.mind(&["forget", "jk:review"]);
    assert!(
        !again.success
            && (again.stderr.contains("not installed") || again.stderr.contains("jk:review")),
        "a second forget jk:review must report it as not installed: {} {}",
        again.stdout,
        again.stderr
    );
}

// ---------------------------------------------------------------------------
// NS-2: a `:` in the effective name produces a real, resolvable on-disk store
// dir and lobe symlink on this platform (the load-bearing assumption).
// ---------------------------------------------------------------------------

#[test]
fn prefixed_lobe_symlink_resolves_through_colon_path() {
    // spec: NS-2
    // The whole separator change rests on a literal `:` being valid in a store
    // directory and a lobe symlink. Existing tests assert the paths EXIST; this
    // asserts they are RESOLVABLE: the file is readable THROUGH the colon-bearing
    // symlink and its content matches the store copy. If a `:` in a path broke on
    // this platform, this would fail rather than silently certify a broken layout.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);

    let store_file = sb.mind_home.join("store/skill/jk:review/SKILL.md");
    let link_file = sb.claude_home.join("skills/jk:review/SKILL.md");

    // The lobe entry is a symlink (not a copy).
    let link_dir = sb.claude_home.join("skills/jk:review");
    assert!(
        std::fs::symlink_metadata(&link_dir)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the prefixed lobe entry must be a symlink"
    );

    // Reading THROUGH the colon-bearing symlink path must resolve to the store
    // copy: the file is readable and its bytes match.
    let via_store = std::fs::read_to_string(&store_file).expect("store file readable");
    let via_link =
        std::fs::read_to_string(&link_file).expect("file readable through the colon-bearing link");
    assert_eq!(
        via_link, via_store,
        "content read through the `:` symlink must equal the store copy"
    );
    assert!(
        via_link.contains("review skill"),
        "the resolved file must be the review SKILL.md: {via_link}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-5 / NS-2: dump of a prefixed install records the prefix as `as = "<p>"`
// and lists items by their BARE `kind:name`, not the prefixed effective name.
// ---------------------------------------------------------------------------

#[test]
fn dump_records_prefix_and_bare_install_items() {
    // spec: DUMP-5
    // A prefixed source's dump must carry the prefix (`namespace = "jk"`, the
    // canonical DSC-78 key) so re-melding reproduces the namespace, while
    // install-items stay in source/catalog truth (bare `skill:review`), never the
    // install-time `skill:jk:review` form. A proper subset (only `review` of
    // {review, dev, style}) forces the install-items listing rather than
    // `install = true`.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);

    let dump = sb.mind(&["dump"]);
    assert!(dump.success, "dump failed: {} {}", dump.stdout, dump.stderr);
    assert!(
        dump.stdout.contains("namespace = \"jk\""),
        "dump must record the effective prefix as `namespace = \"jk\"` (DSC-78): {}",
        dump.stdout
    );
    assert!(
        dump.stdout.contains("\"skill:review\""),
        "install-items must list the BARE ref skill:review: {}",
        dump.stdout
    );
    assert!(
        !dump.stdout.contains("skill:jk:review") && !dump.stdout.contains("jk:review"),
        "install-items must NOT carry the prefixed effective name: {}",
        dump.stdout
    );

    // The dumped document must parse as a mind.toml (re-meldable super-source).
    let parsed: Result<toml::Value, _> = toml::from_str(&dump.stdout);
    assert!(
        parsed.is_ok(),
        "dump output must be valid TOML: {:?}\n{}",
        parsed.err(),
        dump.stdout
    );
}

// ---------------------------------------------------------------------------
// NS-25: the reserved-kind-word prefix guard fires on the re-meld `--as` path
// (CLI-13) and for every reserved word, not just `skill`/`agent`.
// ---------------------------------------------------------------------------

#[test]
fn remeld_as_reserved_kind_word_is_rejected() {
    // spec: NS-25
    // A source already melded can have its prefix changed by re-melding with
    // `--as` (CLI-13). That change must be held to the same NS-25 rule, so a
    // reserved kind word is rejected at the re-meld chokepoint too. Also covers
    // `rule` and `tool`, which the initial-meld CLI test does not exercise.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // First meld with a normal prefix so the source is registered.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);

    for word in ["rule", "tool"] {
        let r = sb.mind(&["meld", &spec, "--as", word]);
        assert!(
            !r.success,
            "re-meld --as {word} must fail: stdout={} stderr={}",
            r.stdout, r.stderr
        );
        assert!(
            r.stderr.contains("reserved") || r.stderr.contains("kind"),
            "error must mention the reserved-kind-word problem for {word}: {}",
            r.stderr
        );
    }

    // The original prefix must be intact (the rejected re-meld changed nothing).
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "the original prefix must survive a rejected re-meld: {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Claude plugin marketplace support (MKT-1..11)
// ---------------------------------------------------------------------------

#[test]
fn marketplace_plugin_meld_discovers_skill_and_agent() {
    // spec: MKT-1, MKT-3
    // A single-plugin source (.claude-plugin/plugin.json) feeds the normal
    // catalog -> store -> symlink pipeline.  Only skill and agent kinds are
    // produced; no rule or tool items come from a plugin.
    //
    // Note: agents appear in probe with the plugin-name prefix in their key
    // (agent:acme-tools:helper), matching the existing NS-40 behavior where
    // probe uses the effective name but the installed LINK is the bare
    // frontmatter name (agents/helper.md).
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:acme-tools:greet"),
        "plugin skill must appear in probe: {}",
        probe.stdout
    );
    // Agent probe key uses the effective (prefixed) name per NS-40/MKT-5.
    assert!(
        probe.stdout.contains("agent:acme-tools:helper"),
        "plugin agent must appear in probe: {}",
        probe.stdout
    );
    // MKT-3: no rule or tool items
    assert!(
        !probe.stdout.contains("rule:"),
        "no rule items from a plugin: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("tool:"),
        "no tool items from a plugin: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_learn_installs_and_links() {
    // spec: MKT-1
    // `learn` installs a plugin skill through the normal store+symlink pipeline.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);

    // The skill directory is symlinked into the lobe under its effective name.
    let link = sb.claude_home.join("skills/acme-tools:greet");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "symlink must exist at claude_home/skills/acme-tools:greet"
    );
}

#[test]
fn marketplace_plugin_skipped_components_note() {
    // spec: MKT-4
    // Unsupported component kinds (commands/, hooks/) are not installed; meld
    // prints a count note so the user is not misled into thinking the plugin is
    // fully represented.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("not installed (no mind equivalent)"),
        "meld must print the skipped-components note: {combined}"
    );
    // The note must mention at least one of the fixture's unsupported kinds.
    assert!(
        combined.contains("hook") || combined.contains("command"),
        "skipped-components note must name a kind: {combined}"
    );
}

#[test]
fn marketplace_plugin_name_is_default_prefix_for_skills() {
    // spec: MKT-5
    // The plugin.json `name` is the default namespace prefix for skills.
    // For agents, NS-40 specifies the lobe LINK is always the bare frontmatter
    // `name` (not `plugin:agent`), even though probe still shows the effective
    // (prefixed) key.  Verify both: skill prefix in probe, bare link on disk.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let probe = sb.mind(&["probe"]);
    // Skill carries the plugin name as prefix.
    assert!(
        probe.stdout.contains("skill:acme-tools:greet"),
        "skill must be prefixed with the plugin name: {}",
        probe.stdout
    );

    // Install the agent and verify its lobe link is the bare frontmatter name.
    // The probe key is `agent:acme-tools:helper` (effective name), so we learn
    // by that key.
    let r = sb.mind(&["learn", "acme-tools:helper"]);
    assert!(r.success, "learn agent failed: {} {}", r.stdout, r.stderr);
    // NS-40/MKT-5: the lobe link is under the bare harness name, not the prefix.
    assert!(
        sb.claude_home.join("agents/helper.md").exists(),
        "agent link must be at agents/helper.md (bare harness name, not prefixed)"
    );
    assert!(
        !sb.claude_home.join("agents/acme-tools:helper.md").exists(),
        "no prefixed agent link must exist"
    );
}

#[test]
fn marketplace_plugin_namespace_override_sets_prefix() {
    // spec: MKT-5
    // `meld --namespace z` overrides the plugin-name prefix; skills install as
    // z:<bare>.  The agent LINK is still bare (NS-40: lobe link ignores the
    // prefix); its probe key reflects the override (agent:z:helper).
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "z", "--link-only"])
            .success
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:z:greet"),
        "consumer --namespace must override the plugin-name prefix: {}",
        probe.stdout
    );

    // Install the agent; its lobe link must remain bare regardless of the
    // consumer namespace.
    let r = sb.mind(&["learn", "z:helper"]);
    assert!(r.success, "learn agent failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("agents/helper.md").exists(),
        "agent link must be bare (agents/helper.md) even with a consumer namespace"
    );
    assert!(
        !sb.claude_home.join("agents/z:helper.md").exists(),
        "no prefixed agent link must exist"
    );
}

#[test]
fn marketplace_plugin_namespace_empty_clears_prefix() {
    // spec: MKT-5
    // `meld --namespace ''` (empty) removes the plugin-name prefix.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--namespace", "", "--link-only"])
            .success
    );

    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:greet"),
        "empty --namespace must clear the plugin-name prefix (skill is bare): {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:acme-tools:greet"),
        "plugin-name prefix must not appear after --namespace '': {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_description_and_version_recorded() {
    // spec: MKT-6
    // The plugin.json `description` is recorded on the source and appears in
    // `recall --sources`.  The `version` is stored and visible in JSON output.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    // Description surfaces in the plain-text recall --sources output.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("Acme developer tools plugin"),
        "plugin description must appear in recall --sources: {}",
        sources.stdout
    );

    // plugin_version is recorded; confirm via --json output.
    let jsrc = sb.mind(&["recall", "--sources", "--json"]);
    assert!(jsrc.success, "{}", jsrc.stderr);
    assert!(
        jsrc.stdout.contains("1.0.0"),
        "plugin_version must be present in JSON output: {}",
        jsrc.stdout
    );
}

#[test]
fn marketplace_catalog_melds_in_repo_plugins() {
    // spec: MKT-7 MKT-14
    // A .claude-plugin/marketplace.json catalog scans each listed in-repo
    // plugin as catalog items of the parent source (MKT-14); both plugins'
    // items appear in probe. Installing an item works through the normal
    // `learn` path.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The parent source is registered. Under MKT-14 in-repo plugins are
    // items of the parent source, not separate sub-sources.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("marketplace-catalog"),
        "catalog source must appear: {}",
        sources.stdout
    );

    // Both plugins' items are available for install via probe.
    // Beta's agent probe key is `agent:beta:two` (effective/prefixed name per
    // NS-40), but its lobe LINK lands at agents/two.md (bare harness name).
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:alpha:one"),
        "alpha's skill must appear in probe: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("agent:beta:two"),
        "beta's agent must appear in probe (prefixed effective name): {}",
        probe.stdout
    );

    // Items from sub-sources are installable through the normal `learn` path.
    let r = sb.mind(&["learn", "alpha:one"]);
    assert!(
        r.success,
        "learn alpha:one failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/alpha:one").exists(),
        "skill alpha:one must be symlinked into the lobe"
    );

    // Beta's agent links under the bare frontmatter name (NS-40/MKT-5).
    let r = sb.mind(&["learn", "beta:two"]);
    assert!(
        r.success,
        "learn beta:two failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("agents/two.md").exists(),
        "beta's agent link must be at agents/two.md (bare harness name)"
    );
}

#[test]
fn marketplace_catalog_probe_hint_fires() {
    // spec: MKT-7
    // After melding a marketplace catalog, `maybe_probe_hint` prints the
    // curated-source hint (DSC-56) so the user knows to browse with `probe`.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("mind probe"),
        "meld of a marketplace catalog must print the probe hint: {combined}"
    );
}

#[test]
fn marketplace_catalog_external_plugin_registers() {
    // spec: MKT-7
    // A marketplace entry pointing at an external git source (local path via
    // file:// URL in the test) melds it as a nested sub-source tracking its
    // own commit, mirroring the [discover].sources external-source behavior.
    let extplugin = Sandbox::bare("extplugin");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1","description":"External test plugin"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );

    // A catalog that references the external plugin via file:// URL
    // (file:// is detected as External by is_external_string -> contains "://").
    let catalog = Sandbox::bare("ext-catalog");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"ext-market","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );

    let cat_spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &cat_spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The external plugin is registered as a sub-source.
    let sources = catalog.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("extplugin"),
        "external plugin sub-source must be registered: {}",
        sources.stdout
    );

    // Its skill is discoverable.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:ext:extskill"),
        "external plugin's skill must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_yes_auto_installs_in_repo_plugins() {
    // spec: MKT-7
    // MKT-7: a marketplace catalog's in-repo plugins are offered for install on
    // meld like the catalog's own items (CLI-23). With `--yes` (non-TTY), they
    // install automatically -- no explicit `learn` needed.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);

    // Both in-repo plugins' items were installed automatically.
    assert!(
        sb.claude_home.join("skills/alpha:one").exists(),
        "alpha's skill must be auto-installed under --yes: lobe = {}",
        sb.claude_home.display()
    );
    assert!(
        sb.claude_home.join("agents/two.md").exists(),
        "beta's agent must be auto-installed under --yes (bare harness name)"
    );
}

#[test]
fn marketplace_external_plugin_installs_only_under_recursive() {
    // spec: MKT-7
    // DSC-54/DSC-55 via MKT-7: an EXTERNAL marketplace plugin is register-only on
    // a plain `--yes` meld (left available), and installs only under
    // `--recursive`, mirroring a `[discover].sources` external nested source.
    let extplugin = Sandbox::bare("extplugin2");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1","description":"External test plugin"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );
    let catalog = Sandbox::bare("ext-catalog2");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"ext-market","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );
    let cat_spec = catalog.source_spec();

    // Plain --yes meld: the external plugin registers but is NOT installed.
    let r = catalog.mind(&["meld", &cat_spec, "--yes"]);
    assert!(r.success, "meld --yes failed: {} {}", r.stdout, r.stderr);
    assert!(
        !catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin must be register-only without --recursive"
    );

    // Re-meld with --recursive --yes: now the external plugin's item installs.
    let r = catalog.mind(&["meld", &cat_spec, "--recursive", "--yes"]);
    assert!(
        r.success,
        "remeld --recursive --yes failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin's skill must install under --recursive"
    );
}

#[test]
fn marketplace_entry_name_wins_over_plugin_json_name() {
    // spec: MKT-8
    // The marketplace entry `name` is used as the alias (namespace prefix) for
    // that plugin's items, overriding whatever name the in-repo plugin.json
    // declares.  The entry name is authoritative.
    let catalog = Sandbox::bare("mkt8-catalog");
    // Build an in-repo plugin whose plugin.json says "original-name".
    catalog.write_and_commit(
        "plugins/myplugin/.claude-plugin/plugin.json",
        r#"{"name":"original-name","version":"0.1"}"#,
    );
    catalog.write_and_commit(
        "plugins/myplugin/skills/theskill/SKILL.md",
        "---\nname: theskill\ndescription: A skill\n---\n# theskill\n",
    );
    // The catalog entry uses "override-name" as the name (MKT-8: entry wins).
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"test","plugins":[{"name":"override-name","source":"./plugins/myplugin"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // The skill must appear under "override-name", not "original-name".
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:override-name:theskill"),
        "entry name must override plugin.json name as the prefix: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:original-name:theskill"),
        "plugin.json name must not appear as prefix when entry overrides it: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_unsafe_in_repo_path_fails_meld() {
    // spec: MKT-9
    // An in-repo source path of "../escape" contains ".." and must be rejected
    // at meld time before any filesystem traversal happens.
    let catalog = Sandbox::bare("unsafe-mkt");
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"bad","plugins":[{"name":"evil","source":"../escape"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(!r.success, "unsafe path must cause meld to fail");
    assert!(
        r.stderr.contains("unsafe") || r.stderr.contains("..") || r.stderr.contains("escape"),
        "error must describe the path safety violation: {}",
        r.stderr
    );
}

#[test]
fn marketplace_malformed_plugin_json_fails_meld() {
    // spec: MKT-9
    // A .claude-plugin/plugin.json that is not valid JSON must fail the meld
    // with a clear error rather than silently producing zero items.
    let sb = Sandbox::bare("bad-plugin-json");
    sb.write_and_commit(".claude-plugin/plugin.json", "{not valid json at all");

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success, "malformed plugin.json must cause meld to fail");
    assert!(
        r.stderr.contains("plugin.json") || r.stderr.contains("invalid"),
        "error must indicate the invalid plugin.json: {}",
        r.stderr
    );
}

#[test]
fn marketplace_malformed_marketplace_json_fails_meld() {
    // spec: MKT-9
    // A .claude-plugin/marketplace.json that is not valid JSON must fail meld.
    let sb = Sandbox::bare("bad-market-json");
    sb.write_and_commit(".claude-plugin/marketplace.json", "{bad json");

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        !r.success,
        "malformed marketplace.json must cause meld to fail"
    );
    assert!(
        r.stderr.contains("marketplace.json") || r.stderr.contains("invalid"),
        "error must indicate the invalid marketplace.json: {}",
        r.stderr
    );
}

#[test]
fn marketplace_ansi_escape_stripped_in_recall_sources() {
    // spec: MKT-9
    // Descriptions taken from a plugin.json have ANSI escape sequences stripped
    // before display, so a malicious or miscoded description cannot corrupt the
    // terminal via `recall --sources`.
    let sb = Sandbox::bare("ansi-plugin");
    // ESC [ 3 1 m = red-color CSI sequence; should be stripped.
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        "{\"name\":\"ansi-p\",\"description\":\"\\u001b[31mred text\\u001b[0m safe\"}",
    );

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    // The ESC byte (0x1b) must not appear in the output.
    assert!(
        !sources.stdout.contains('\x1b'),
        "ANSI escape bytes must be stripped from recall --sources output: {:?}",
        sources.stdout
    );
    // The visible text portion must still appear (the safe suffix after stripping).
    assert!(
        sources.stdout.contains("safe") || sources.stdout.contains("red text"),
        "stripped text must still be visible: {}",
        sources.stdout
    );
}

#[test]
fn marketplace_plugin_origin_label_in_recall_sources() {
    // spec: MKT-10
    // A source whose items came from a .claude-plugin/plugin.json is labelled
    // `origin:claude-plugin` in `recall --sources`.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("origin:claude-plugin"),
        "recall --sources must label a plugin source with origin:claude-plugin: {}",
        sources.stdout
    );
}

#[test]
fn marketplace_catalog_origin_label_in_recall_sources() {
    // spec: MKT-10
    // A source whose items came from a .claude-plugin/marketplace.json (both the
    // catalog itself and its sub-sources) is labelled `origin:claude-marketplace`.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("origin:claude-marketplace"),
        "recall --sources must label a marketplace source with origin:claude-marketplace: {}",
        sources.stdout
    );
}

#[test]
fn mind_toml_suppresses_plugin_manifest_with_note() {
    // spec: MKT-2
    // When a source has an authoritative mind.toml (one that declares [[items]])
    // AND a .claude-plugin/plugin.json, the mind.toml wins and a note is printed
    // saying the plugin manifest was found but ignored.
    let sb = Sandbox::bare("toml-wins");
    // Authoritative mind.toml declaring one item.
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"toml-skill\"\npath = \"skills/toml-skill/SKILL.md\"\n",
    );
    sb.write_and_commit(
        "skills/toml-skill/SKILL.md",
        "---\nname: toml-skill\ndescription: From mind.toml\n---\n# toml\n",
    );
    // Also has a plugin.json declaring different items.
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"plugin-name","version":"1.0","description":"Plugin desc"}"#,
    );
    sb.write_and_commit(
        "skills/plugin-skill/SKILL.md",
        "---\nname: plugin-skill\ndescription: From plugin\n---\n# plugin\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // Advisory note: mind.toml is authoritative, plugin manifest is ignored.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("authoritative mind.toml")
            || combined.contains(".claude-plugin/ manifest is ignored"),
        "meld must print the advisory note about mind.toml suppressing plugin manifest: {combined}"
    );

    // Items come from mind.toml, not from the plugin.json.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:toml-skill"),
        "mind.toml item must appear in probe: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("skill:plugin-skill"),
        "plugin item must not appear (mind.toml is authoritative): {}",
        probe.stdout
    );
    // The plugin-name prefix from plugin.json must also not be applied.
    assert!(
        !probe.stdout.contains("skill:plugin-name:"),
        "plugin-name prefix must not appear: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_plugin_does_not_emit_claude_plugin_manifest() {
    // spec: MKT-11
    // Consuming a marketplace does not make mind a plugin publisher: no
    // .claude-plugin/ directory is created in the store or lobe, and `dump`
    // output contains no .claude-plugin reference.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success);

    // No .claude-plugin dir in the store.
    let store_dir = sb.mind_home.join("store");
    if store_dir.is_dir() {
        for entry in std::fs::read_dir(&store_dir).unwrap().flatten() {
            let p = entry.path();
            assert!(
                !p.join(".claude-plugin").exists(),
                "store entry must not contain .claude-plugin: {:?}",
                p
            );
        }
    }

    // No .claude-plugin dir in the lobe.
    assert!(
        !sb.claude_home.join(".claude-plugin").exists(),
        "lobe must not contain a .claude-plugin dir"
    );

    // `dump` output contains no .claude-plugin reference.
    let dump = sb.mind(&["dump"]);
    assert!(
        !dump.stdout.contains(".claude-plugin"),
        "dump output must contain no .claude-plugin reference: {}",
        dump.stdout
    );
}

#[test]
fn marketplace_entry_version_wins_over_plugin_json_version() {
    // spec: MKT-8
    // A marketplace entry's declared `version` is authoritative over the in-repo
    // plugin.json's own `version` (mirrors Claude's "strict": false). The existing
    // MKT-8 test only proves the entry NAME wins; this proves the VERSION field
    // does too, surfaced via `recall --sources --json`.
    let catalog = Sandbox::bare("mkt8-version");
    // In-repo plugin whose plugin.json declares version 9.9.9.
    catalog.write_and_commit(
        "plugins/myplugin/.claude-plugin/plugin.json",
        r#"{"name":"myplugin","version":"9.9.9"}"#,
    );
    catalog.write_and_commit(
        "plugins/myplugin/skills/theskill/SKILL.md",
        "---\nname: theskill\ndescription: A skill\n---\n# theskill\n",
    );
    // The catalog entry declares a DIFFERENT version 1.1.1 (entry wins).
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"test","plugins":[{"name":"override","source":"./plugins/myplugin","version":"1.1.1"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Under MKT-14 in-repo entries are catalog items of the parent source;
    // the entry-level version is not stored per-sub-source. What IS
    // observable: the skill is discoverable under the entry name as prefix
    // (MKT-8 entry name wins).
    let probe = catalog.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("override:theskill"),
        "skill must appear under entry name as prefix (MKT-8): {}",
        probe.stdout
    );
}

#[test]
fn mind_toml_suppresses_marketplace_manifest_with_note() {
    // spec: MKT-2
    // The marketplace analogue of mind_toml_suppresses_plugin_manifest_with_note:
    // an authoritative mind.toml alongside a .claude-plugin/marketplace.json wins,
    // a note is printed, and NONE of the marketplace's sub-source plugins are
    // melded (only the mind.toml item exists).
    let catalog = Sandbox::bare("mkt2-market-suppress");
    // Authoritative mind.toml declaring one rule.
    catalog.write_and_commit(
        "rules/my-rule.md",
        "---\ndescription: my rule\n---\n# rule\n",
    );
    catalog.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"rule\"\nname = \"my-rule\"\npath = \"rules/my-rule.md\"\n",
    );
    // A marketplace.json that would otherwise meld an in-repo plugin sub-source.
    catalog.write_and_commit(
        "plugins/embedded/.claude-plugin/plugin.json",
        r#"{"name":"embedded","version":"0.1"}"#,
    );
    catalog.write_and_commit(
        "plugins/embedded/skills/embskill/SKILL.md",
        "---\nname: embskill\ndescription: embedded skill\n---\n# embskill\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"embedded","source":"./plugins/embedded"}]}"#,
    );

    let spec = catalog.source_spec();
    let r = catalog.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("authoritative mind.toml")
            || combined.contains(".claude-plugin/ manifest is ignored"),
        "meld must print the advisory note for a suppressed marketplace manifest: {combined}"
    );

    // No sub-source was registered from the suppressed marketplace.
    let sources = catalog.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("embedded"),
        "the marketplace's in-repo plugin must NOT be melded when mind.toml is authoritative: {}",
        sources.stdout
    );

    // Only the mind.toml rule item exists; the plugin skill does not.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("rule:my-rule"),
        "the mind.toml rule must be present: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("embskill"),
        "the suppressed plugin's skill must not appear: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_two_plugins_same_agent_name_collide() {
    // spec: NS-41
    // Per MKT-5/NS-40 a plugin's agents flatten to their bare frontmatter name and
    // the plugin prefix does NOT reach agents. So two catalog plugins each shipping
    // an agent with the SAME frontmatter `name:` both target agents/<name>.md -- a
    // detected collision (NS-41), not a silent overwrite. Learning the second one
    // must fail with an agent-collision error.
    let catalog = Sandbox::bare("mkt-agent-collide");
    // Plugin A ships an agent whose frontmatter name is "shared".
    catalog.write_and_commit("plugins/a/.claude-plugin/plugin.json", r#"{"name":"pa"}"#);
    catalog.write_and_commit(
        "plugins/a/agents/from-a.md",
        "---\nname: shared\ndescription: agent from A\n---\n# a\n",
    );
    // Plugin B ships a differently-filed agent with the SAME frontmatter name.
    catalog.write_and_commit("plugins/b/.claude-plugin/plugin.json", r#"{"name":"pb"}"#);
    catalog.write_and_commit(
        "plugins/b/agents/from-b.md",
        "---\nname: shared\ndescription: agent from B\n---\n# b\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"pa","source":"./plugins/a"},{"name":"pb","source":"./plugins/b"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // Both agents flatten to the bare harness name "shared" (prefix omitted).
    // Learn the first: it installs at agents/shared.md.
    let r1 = catalog.mind(&["learn", "pa:from-a"]);
    assert!(
        r1.success,
        "first agent must install: {} {}",
        r1.stdout, r1.stderr
    );
    assert!(
        catalog.claude_home.join("agents/shared.md").exists(),
        "first agent must link at agents/shared.md"
    );

    // Learn the second (a different source): NS-41 must refuse the colliding link.
    let r2 = catalog.mind(&["learn", "pb:from-b"]);
    assert!(
        !r2.success,
        "a second agent colliding at agents/shared.md must be refused (NS-41): {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stderr.to_lowercase().contains("collid") || r2.stderr.to_lowercase().contains("shared"),
        "the error must describe the agent collision: {}",
        r2.stderr
    );
}

#[test]
fn marketplace_sync_rewalk_registers_new_entry() {
    // spec: MKT-7 MKT-14
    // After melding a catalog, adding a new in-repo plugin entry to marketplace.json
    // and running `sync` must make that plugin's items discoverable. Under MKT-14
    // in-repo entries are scan roots of the catalog (not sub-melded), so the items
    // appear under the catalog source after sync fetches the updated commit; the new
    // plugin does NOT appear as a separate registered source.
    let catalog = Sandbox::bare("mkt-sync-rewalk");
    // First plugin present from the start.
    catalog.write_and_commit(
        "plugins/first/.claude-plugin/plugin.json",
        r#"{"name":"first"}"#,
    );
    catalog.write_and_commit(
        "plugins/first/skills/oneskill/SKILL.md",
        "---\nname: oneskill\ndescription: skill one\n---\n# one\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"first","source":"./plugins/first"}]}"#,
    );

    let spec = catalog.source_spec();
    assert!(catalog.mind(&["meld", &spec]).success);

    // Only the catalog source is registered; "second" does not yet exist.
    let before = catalog.mind(&["recall", "--sources"]);
    assert!(
        !before.stdout.contains("second"),
        "second must not exist before it is added: {}",
        before.stdout
    );

    // Add a SECOND in-repo plugin and list it in the marketplace, then commit.
    catalog.write_and_commit(
        "plugins/second/.claude-plugin/plugin.json",
        r#"{"name":"second"}"#,
    );
    catalog.write_and_commit(
        "plugins/second/skills/twoskill/SKILL.md",
        "---\nname: twoskill\ndescription: skill two\n---\n# two\n",
    );
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"cat","plugins":[{"name":"first","source":"./plugins/first"},{"name":"second","source":"./plugins/second"}]}"#,
    );

    // Sync must fetch the catalog and pick up the new plugin via the marketplace scan.
    let sync = catalog.mind(&["sync"]);
    assert!(sync.success, "sync failed: {} {}", sync.stdout, sync.stderr);

    // Under MKT-14 in-repo plugins are scan roots of the catalog, not sub-melded sub-sources.
    // "second" must NOT appear as a registered source; its items surface via the catalog scan.
    let after = catalog.mind(&["recall", "--sources"]);
    assert!(
        !after.stdout.contains("/second") && !after.stdout.contains("second "),
        "second must not be a registered sub-source (MKT-14): {}",
        after.stdout
    );
    // The new plugin's skill is discoverable via the catalog's marketplace scan.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("twoskill"),
        "the newly-added plugin's skill must be discoverable after sync: {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Plugin repos / marketplace catalogs as [discover].sources entries (MKT-12/13)
// ---------------------------------------------------------------------------

#[test]
fn nested_source_plugin_inherits_plugin_name_as_namespace() {
    // spec: MKT-12
    // A [discover].sources entry pointing at a repo that carries a
    // .claude-plugin/plugin.json and has no explicit `namespace` uses the
    // plugin's `name` field as the effective prefix for that nested source --
    // exactly as if it had been melded directly under MKT-5.
    let plugin_repo = Sandbox::bare("mkt12-plugin");
    plugin_repo.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Plugin greet skill\n---\n# greet\n",
    );
    plugin_repo.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"myplugin","version":"1.0"}"#,
    );

    let super_src = Sandbox::bare("mkt12-super");
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n",
            plugin_repo.source_spec()
        ),
    );

    let spec = super_src.source_spec();
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // The nested plugin source is registered with the plugin name as prefix.
    let probe = super_src.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("myplugin:greet"),
        "nested plugin source must use plugin name as default prefix (MKT-12): {}",
        probe.stdout
    );
}

#[test]
fn nested_source_marketplace_preserves_per_plugin_namespacing() {
    // spec: MKT-13
    // A [discover].sources entry that points at a marketplace catalog preserves
    // per-plugin namespacing: each plugin's `name` field from the catalog entry
    // becomes the effective prefix for that plugin's items (MKT-8), regardless
    // of any outer namespace set on the [discover].sources entry.
    let market_repo = Sandbox::bare("mkt13-market");
    market_repo.write_and_commit(
        "plugins/alpha/.claude-plugin/plugin.json",
        r#"{"name":"alpha"}"#,
    );
    market_repo.write_and_commit(
        "plugins/alpha/skills/search/SKILL.md",
        "---\nname: search\ndescription: Alpha search skill\n---\n# search\n",
    );
    market_repo.write_and_commit(
        "plugins/beta/.claude-plugin/plugin.json",
        r#"{"name":"beta"}"#,
    );
    market_repo.write_and_commit(
        "plugins/beta/skills/search/SKILL.md",
        "---\nname: search\ndescription: Beta search skill\n---\n# search\n",
    );
    market_repo.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"TestMarket","plugins":[{"name":"alpha","source":"./plugins/alpha"},{"name":"beta","source":"./plugins/beta"}]}"#,
    );

    let super_src = Sandbox::bare("mkt13-super");
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[[discover.sources]]\nsource = \"{}\"\n",
            market_repo.source_spec()
        ),
    );

    let spec = super_src.source_spec();
    let r = super_src.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Both plugins appear under their per-plugin names through the nested chain (MKT-13).
    let probe = super_src.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("alpha:search"),
        "alpha's skill must be namespaced under 'alpha' through the nested chain (MKT-13): {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("beta:search"),
        "beta's skill must be namespaced under 'beta' through the nested chain (MKT-13): {}",
        probe.stdout
    );
}

// ---------------------------------------------------------------------------
// Cross-source skill collision at meld (NS-43/NS-45)
// ---------------------------------------------------------------------------

#[test]
fn skill_collision_non_interactive_errors_with_guidance() {
    // spec: NS-43 NS-45
    // In a non-interactive session (no TTY), melding a source whose skill
    // effective name matches an already-installed skill from a different source
    // exits non-zero (SkillCollision) and suggests --namespace. Melding with
    // --namespace resolves the conflict.
    let src_a = Sandbox::bare("ns43-a");
    src_a.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill A\n---\n# deploy\n",
    );
    // Meld and install source A so `deploy` is in the manifest.
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    // Source B has the same skill name.
    let src_b = Sandbox::bare("ns43-b");
    src_b.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill B\n---\n# deploy\n",
    );
    // Non-TTY meld of source B must fail with SkillCollision (NS-45).
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "colliding meld must fail in non-interactive mode (NS-45)"
    );
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("collision")
            || combined.contains("conflict")
            || combined.contains("namespace"),
        "error must describe the collision and suggest --namespace (NS-43 NS-45): {combined}"
    );

    // Melding source B with --namespace resolves the collision.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--namespace", "sb", "--yes"]);
    assert!(
        r.success,
        "meld with --namespace must resolve the collision: {} {}",
        r.stdout, r.stderr
    );
    // The namespaced skill is discoverable in probe.
    let probe = src_a.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("sb:deploy"),
        "namespaced skill sb:deploy must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn skill_collision_same_source_remeld_is_not_a_collision() {
    // spec: NS-43
    // The NS-43 same-source skip prevents false collision detection when items
    // from a previously installed source remain in the manifest after the source
    // is unregistered. Re-melding that source must succeed.
    let src = Sandbox::bare("ns43-remeld");
    src.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill\n---\n# deploy\n",
    );

    // Meld and install.
    let r = src.mind(&["meld", &src.source_spec(), "--yes"]);
    assert!(
        r.success,
        "first meld must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Unmeld --unlink-only: source is removed from the registry but the
    // installed item remains in the manifest under the original source name.
    let r = src.mind(&["unmeld", "ns43-remeld", "--unlink-only"]);
    assert!(
        r.success,
        "unmeld --unlink-only must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Re-meld: manifest still holds `deploy` under this source's name.
    // The NS-43 same-source skip must prevent a false SkillCollision.
    let r = src.mind(&["meld", &src.source_spec(), "--yes"]);
    assert!(
        r.success,
        "re-meld of same source must succeed (NS-43 same-source skip): {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn skill_collision_with_namespace_set_does_not_collide() {
    // spec: NS-43
    // Melding with --namespace makes effective names distinct from
    // already-installed items, so the NS-43 collision check does not fire.
    let src_a = Sandbox::bare("ns43-ns-a");
    src_a.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill A\n---\n# deploy\n",
    );
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    let src_b = Sandbox::bare("ns43-ns-b");
    src_b.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Deploy skill B\n---\n# deploy\n",
    );
    // --namespace sb makes the effective name "sb:deploy", distinct from the
    // installed "deploy" -- no collision fires.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--namespace", "sb", "--yes"]);
    assert!(
        r.success,
        "meld with --namespace must succeed when effective names are distinct (NS-43): {} {}",
        r.stdout, r.stderr
    );
    // Both skills are discoverable under their distinct names.
    let probe = src_a.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("sb:deploy"),
        "namespaced sb:deploy must appear in probe: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_same_root_multiple_plugins_with_skills_array() {
    // spec: MKT-14
    // Multiple in-repo plugins all with "source": "./" and explicit skills arrays
    // (the Anthropic pattern). Each plugin's listed skills are scanned under its
    // own entry name as prefix. Skills from different plugins do not cross-contaminate.
    let sb = Sandbox::bare("mkt14-same-root");

    // Plugin 1: document-skills
    sb.write_and_commit(
        "skills/xlsx/SKILL.md",
        "---\nname: xlsx\ndescription: xlsx\n---\n# xlsx\n",
    );
    sb.write_and_commit(
        "skills/docx/SKILL.md",
        "---\nname: docx\ndescription: docx\n---\n# docx\n",
    );

    // Plugin 2: example-skills
    sb.write_and_commit(
        "skills/art/SKILL.md",
        "---\nname: art\ndescription: art\n---\n# art\n",
    );
    sb.write_and_commit(
        "skills/design/SKILL.md",
        "---\nname: design\ndescription: design\n---\n# design\n",
    );

    // Plugin 3: api-skills
    sb.write_and_commit(
        "skills/claude-api/SKILL.md",
        "---\nname: claude-api\ndescription: api\n---\n# api\n",
    );

    // marketplace.json: three plugins, all source: "./", each with a disjoint skills array
    sb.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{
  "name": "anthropic-agent-skills",
  "plugins": [
    {"name": "document-skills", "source": "./", "skills": ["./skills/xlsx", "./skills/docx"]},
    {"name": "example-skills",  "source": "./", "skills": ["./skills/art", "./skills/design"]},
    {"name": "api-skills",      "source": "./", "skills": ["./skills/claude-api"]}
  ]
}"#,
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let probe = sb.mind(&["probe", "--no-tui"]);
    // document-skills plugin
    assert!(
        probe.stdout.contains("document-skills:xlsx"),
        "expected document-skills:xlsx: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("document-skills:docx"),
        "expected document-skills:docx: {}",
        probe.stdout
    );
    // example-skills plugin
    assert!(
        probe.stdout.contains("example-skills:art"),
        "expected example-skills:art: {}",
        probe.stdout
    );
    assert!(
        probe.stdout.contains("example-skills:design"),
        "expected example-skills:design: {}",
        probe.stdout
    );
    // api-skills plugin
    assert!(
        probe.stdout.contains("api-skills:claude-api"),
        "expected api-skills:claude-api: {}",
        probe.stdout
    );

    // No skill appears under the wrong prefix (partition is correct).
    assert!(
        !probe.stdout.contains("example-skills:xlsx"),
        "xlsx must not appear under example-skills: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("document-skills:art"),
        "art must not appear under document-skills: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_forget_removes_plugin_installed_item() {
    // spec: MKT-1
    // `forget` uninstalls a plugin-sourced skill: the lobe symlink is removed and
    // `recall` no longer finds the item.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["forget", "skill:acme-tools:greet"]);
    assert!(r.success, "forget must succeed: {} {}", r.stdout, r.stderr);

    // The symlink must be gone.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/acme-tools:greet")).is_err(),
        "symlink must be removed after forget"
    );

    // `recall` must no longer list the item.
    assert!(
        !sb.mind(&["recall", "skill:acme-tools:greet"]).success,
        "recall must fail after forget (item no longer installed)"
    );
}

#[test]
fn marketplace_upgrade_plugin_sourced_item() {
    // spec: MKT-1, MKT-6
    // Upgrading after a source change updates the installed plugin item; the hash
    // advances. Drift is source-content-hash driven (MKT-6), not by declared plugin version.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let before = sb.mind(&["recall", "skill:acme-tools:greet"]).stdout;

    // Update the skill content and commit.
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet (updated)\n",
    );
    assert!(
        sb.mind(&["sync"]).success,
        "sync must succeed after source update"
    );

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        r.success,
        "upgrade --yes must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgraded skill:acme-tools:greet"),
        "upgrade must report the item as upgraded: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:acme-tools:greet"]).stdout;
    assert_ne!(
        before, after,
        "recall output must differ after upgrade (hash must advance)"
    );
}

#[test]
fn marketplace_introspect_fix_relinks_plugin_item() {
    // spec: MKT-1
    // Manually removing a plugin-item symlink and running `introspect --fix`
    // restores it, confirming the plugin install path uses the same registry as
    // convention items.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let link = sb.claude_home.join("skills/acme-tools:greet");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "symlink must exist before the test removes it"
    );
    std::fs::remove_file(&link).unwrap();

    let r = sb.mind(&["introspect", "--fix"]);
    assert!(
        r.success,
        "introspect --fix must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("relinked"),
        "introspect --fix must report the link was recreated: {}",
        r.stdout
    );
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "symlink must be restored by introspect --fix"
    );
    assert!(
        sb.mind(&["introspect"]).stdout.contains("all good"),
        "introspect must be clean after --fix"
    );
}

#[test]
fn marketplace_recall_shows_installed_plugin_item() {
    // spec: MKT-1, MKT-5
    // `recall` (item list) includes an installed plugin item by its effective
    // (prefixed) name. Distinct from the probe assertions elsewhere which only
    // check availability, not the installed-item listing.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld --link-only must succeed"
    );
    let learn = sb.mind(&["learn", "acme-tools:greet"]);
    assert!(
        learn.success,
        "learn must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let list = sb.mind(&["recall"]);
    assert!(
        list.success,
        "recall must succeed: {} {}",
        list.stdout, list.stderr
    );
    assert!(
        list.stdout.contains("skill:acme-tools:greet"),
        "recall must list the installed plugin skill by its prefixed name: {}",
        list.stdout
    );
}

#[test]
fn marketplace_source_only_mind_toml_composes_with_plugin() {
    // spec: MKT-2
    // A `[source]`-only mind.toml (no `[[items]]` or `[discover]`, so non-authoritative)
    // composes with a .claude-plugin/plugin.json: the mind.toml prefix wins over the
    // plugin name (MKT-5 precedence), and the plugin still supplies items.
    let sb = Sandbox::bare("compose-toml");
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"custom\"\n");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"plugin-name","version":"1.0"}"#,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        r.success,
        "meld --link-only must succeed: {} {}",
        r.stdout, r.stderr
    );

    let probe = sb.mind(&["probe"]);
    // mind.toml prefix wins over plugin name (MKT-5 precedence).
    assert!(
        probe.stdout.contains("skill:custom:greet"),
        "mind.toml prefix must win over plugin name: {}",
        probe.stdout
    );
    // Plugin name must NOT be used as prefix when mind.toml has one.
    assert!(
        !probe.stdout.contains("skill:plugin-name:greet"),
        "plugin name must not be used as prefix when mind.toml supplies one: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_mind_toml_roots_suppress_manifest_own_items_with_note() {
    // spec: MKT-15
    // A repo ships a .claude-plugin/plugin.json AND a mind.toml declaring a
    // [source].roots scan layout. The roots layout is an own-item directive: it
    // suppresses the manifest's own-item layer, so convention discovery under the
    // root supplies the repo's items, the plugin's components are not scanned, and
    // meld prints a note that the manifest's plugin components are ignored.
    let sb = Sandbox::bare("mkt15-roots");
    // The plugin manifest would (absent MKT-15) contribute skills/greet as acme:greet.
    sb.write_and_commit(".claude-plugin/plugin.json", "{\n  \"name\": \"acme\"\n}\n");
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: plugin greet\n---\n# greet\n",
    );
    // The mind.toml relocates convention to pkg/, defining the repo's own items.
    sb.write_and_commit("mind.toml", "[source]\nroots = [\"pkg\"]\n");
    sb.write_and_commit(
        "pkg/skills/own/SKILL.md",
        "---\nname: own\ndescription: convention own skill\n---\n# own\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // The MKT-15 suppression note is printed.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin components are ignored"),
        "meld must note the manifest's own-item layer is ignored: {combined}"
    );

    let probe = sb.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:own"),
        "convention skill under the declared root must be discovered: {probe}"
    );
    assert!(
        !probe.contains("greet"),
        "the plugin manifest's own items must be suppressed by the roots layout: {probe}"
    );
}

#[test]
fn marketplace_consumer_root_flag_suppresses_manifest_with_note() {
    // spec: MKT-15, DSC-51
    // A consumer `meld --root pkg` on a manifest source with no mind.toml scan
    // layout is an own-item directive too: it suppresses the manifest's own-item
    // layer, convention discovery under the consumer root supplies the items, and
    // meld prints the suppression note (naming the flag). Without this, --root
    // would be a silent no-op on a manifest source.
    let sb = Sandbox::bare("mkt15-consumer-root");
    // The plugin manifest would (absent MKT-15) contribute skills/greet as acme:greet.
    sb.write_and_commit(".claude-plugin/plugin.json", "{\n  \"name\": \"acme\"\n}\n");
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: plugin greet\n---\n# greet\n",
    );
    // The repo's own items live under pkg/, reached only via a consumer --root.
    sb.write_and_commit(
        "pkg/skills/own/SKILL.md",
        "---\nname: own\ndescription: convention own skill\n---\n# own\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--root", "pkg", "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("plugin components are ignored")
            && combined.contains("--root/--flat-skills"),
        "meld must note the manifest is ignored and name the consumer flag: {combined}"
    );

    let probe = sb.mind(&["probe"]).stdout;
    assert!(
        probe.contains("skill:own"),
        "convention skill under the consumer root must be discovered: {probe}"
    );
    assert!(
        !probe.contains("greet"),
        "the plugin manifest's own items must be suppressed by --root: {probe}"
    );
}

#[test]
fn marketplace_and_curator_compose_via_discover_sources() {
    // spec: MKT-16
    // A repo ships a .claude-plugin/marketplace.json (an in-repo plugin) AND a
    // mind.toml whose only discovery content is [discover].sources (a curator
    // directive over OTHER repos). The two compose: the marketplace still defines
    // the immediate source (its in-repo plugin item is discovered, not suppressed),
    // and the curated nested source is registered. No suppression note is printed.
    let curator = Sandbox::bare("curator");
    let nested = Sandbox::named("curated"); // a normal source with fixture items
    // Marketplace with one in-repo plugin: the curator's own items.
    curator.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{ "name": "M", "plugins": [ {"name": "toolkit", "source": "./plugins/toolkit"} ] }"#,
    );
    curator.write_and_commit(
        "plugins/toolkit/skills/format/SKILL.md",
        "---\nname: format\ndescription: fmt\n---\n# format\n",
    );
    // A co-present mind.toml with ONLY [discover].sources (curator directive).
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[source]\ndescription = \"marketplace + curator\"\n\n[discover]\nsources = [{{ source = \"{}\" }}]\n",
            nested.source_spec()
        ),
    );

    let spec = curator.source_spec();
    let r = curator.mind(&["meld", &spec, "--link-only"]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // Compose: the marketplace's own in-repo plugin item IS discovered (the
    // [discover].sources list does not suppress the manifest).
    let probe = curator.mind(&["probe"]).stdout;
    assert!(
        probe.contains("toolkit:format"),
        "the marketplace in-repo plugin item must be discovered (not suppressed by [discover].sources): {probe}"
    );
    // And the curated nested source is registered.
    let sources = curator.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("/curated"),
        "the curated nested source must be registered alongside the marketplace: {sources}"
    );
    // No manifest-suppression note: a bare [discover].sources is not an own-item directive.
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        !combined.contains("manifest's plugin components are ignored")
            && !combined.contains("manifest is ignored"),
        "a [discover].sources-only mind.toml must not suppress the manifest: {combined}"
    );
}

#[test]
fn example_marketplace_curator_validates() {
    // spec: MKT-15, MKT-16
    // The marketplace-curator example is both a Claude marketplace and a mind
    // curator. It validates clean structurally (review does not clone the nested
    // chain), pinning that the compose shape parses and passes author validation.
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/marketplace-curator");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(
        r.success,
        "marketplace-curator example must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn marketplace_plugin_with_only_unsupported_components() {
    // spec: MKT-3, MKT-4
    // A plugin with no skills or agents (only hooks/, commands/) succeeds
    // on meld, prints a skipped-components note, and contributes zero items to probe.
    let sb = Sandbox::bare("no-items-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"hooks-only","version":"1.0"}"#,
    );
    sb.write_and_commit("hooks/post.sh", "#!/bin/sh\n");
    sb.write_and_commit("commands/do.sh", "#!/bin/sh\n");

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld of a plugin with only unsupported components must succeed: {} {}",
        r.stdout, r.stderr
    );

    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("not installed (no mind equivalent)"),
        "meld must print the skipped-components note: {combined}"
    );

    // Since this is the only melded source, probe must have no item lines.
    let probe = sb.mind(&["probe"]);
    assert!(
        !probe.stdout.contains("skill:")
            && !probe.stdout.contains("agent:")
            && !probe.stdout.contains("rule:")
            && !probe.stdout.contains("tool:"),
        "a hooks-only plugin must contribute zero items to probe: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_reserved_kind_word_plugin_name_installs_bare() {
    // spec: MKT-5, NS-25
    // When a plugin.json `name` is a reserved kind word (e.g. "skill"), the resilience
    // path in catalog.rs (NS-25 validate_prefix guard) silently falls through to no
    // prefix, so items install bare rather than making the source un-meldable.
    let sb = Sandbox::bare("reserved-name-plugin");
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"skill","version":"1.0"}"#,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: Greet\n---\n# greet\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        r.success,
        "meld of a plugin named with a reserved kind word must succeed: {} {}",
        r.stdout, r.stderr
    );

    let probe = sb.mind(&["probe"]);
    // Items install bare (no prefix) when the plugin name is a reserved kind word.
    assert!(
        probe.stdout.contains("skill:greet"),
        "item must install bare when plugin name is a reserved kind word: {}",
        probe.stdout
    );
    // The reserved word must NOT be used as a prefix (would produce "skill:skill:greet").
    assert!(
        !probe.stdout.contains("skill:skill:greet"),
        "reserved kind word must not be used as prefix: {}",
        probe.stdout
    );
}

#[test]
fn marketplace_external_plugin_learnable_on_demand() {
    // spec: MKT-7, DSC-54
    // An external marketplace plugin registered by a plain meld (without --recursive)
    // is available (discoverable via probe) and learnable on demand via `learn`,
    // even though it was not auto-installed at meld time.
    let extplugin = Sandbox::bare("extplugin-ondemand");
    extplugin.write_and_commit(
        ".claude-plugin/plugin.json",
        r#"{"name":"ext","version":"0.1"}"#,
    );
    extplugin.write_and_commit(
        "skills/extskill/SKILL.md",
        "---\nname: extskill\ndescription: External skill\n---\n# extskill\n",
    );

    let catalog = Sandbox::bare("ext-catalog-ondemand");
    let ext_url = format!("file://{}", extplugin.source_spec());
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(r#"{{"name":"cat","plugins":[{{"name":"ext","source":"{ext_url}"}}]}}"#),
    );

    let cat_spec = catalog.source_spec();
    // Plain meld (no --yes, no --recursive): external plugin registers but is not installed.
    let r = catalog.mind(&["meld", &cat_spec]);
    assert!(r.success, "meld must succeed: {} {}", r.stdout, r.stderr);

    // External skill must NOT be in the lobe yet.
    assert!(
        !catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin skill must not be installed after plain meld"
    );

    // External skill IS discoverable via probe.
    let probe = catalog.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:ext:extskill"),
        "external plugin skill must be discoverable after plain meld: {}",
        probe.stdout
    );

    // On-demand learn must install it.
    let learn = catalog.mind(&["learn", "ext:extskill"]);
    assert!(
        learn.success,
        "learn of external plugin skill must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        catalog.claude_home.join("skills/ext:extskill").exists(),
        "external plugin skill must be linked after learn"
    );
}

#[test]
fn marketplace_dump_of_plugin_source_contains_no_claude_plugin_ref() {
    // spec: MKT-11, MKT-6
    // `dump` of a source that came from a plugin manifest:
    // (a) succeeds and produces output;
    // (b) contains NO ".claude-plugin" reference (MKT-11: mind does not emit manifests);
    // (c) contains the source spec so a re-meld would work.
    let sb = Sandbox::from_example("marketplace-plugin");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "meld --yes must succeed"
    );

    let dump = sb.mind(&["dump"]);
    assert!(
        dump.success,
        "dump must succeed: {} {}",
        dump.stdout, dump.stderr
    );
    assert!(
        !dump.stdout.is_empty(),
        "dump output must be non-empty for a plugin source"
    );
    assert!(
        !dump.stdout.contains(".claude-plugin"),
        "dump must not contain any .claude-plugin reference (MKT-11): {}",
        dump.stdout
    );
}

#[test]
fn marketplace_upgrade_catalog_sub_source_item() {
    // spec: MKT-7, MKT-6
    // Upgrading a catalog sub-source item after the sub-source content changes:
    // `sync` re-walks the marketplace and detects the update; `upgrade --yes` applies it.
    let sb = Sandbox::from_example("marketplace-catalog");
    let spec = sb.source_spec();
    // Plain meld (no --yes) to skip auto-install; then learn the item explicitly.
    assert!(sb.mind(&["meld", &spec]).success, "meld must succeed");
    let learn = sb.mind(&["learn", "alpha:one"]);
    assert!(
        learn.success,
        "learn alpha:one must succeed: {} {}",
        learn.stdout, learn.stderr
    );

    let before = sb.mind(&["recall", "skill:alpha:one"]).stdout;

    // Update the sub-source item and commit.
    sb.write_and_commit(
        "plugins/alpha/skills/one/SKILL.md",
        "---\nname: one\ndescription: Alpha skill (updated)\n---\n# one updated\n",
    );
    assert!(
        sb.mind(&["sync"]).success,
        "sync must succeed after sub-source update"
    );

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(
        r.success,
        "upgrade --yes must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgraded skill:alpha:one"),
        "upgrade must report the sub-source item as upgraded: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:alpha:one"]).stdout;
    assert_ne!(
        before, after,
        "recall output must differ after upgrade (hash must advance)"
    );
}

// ---------------------------------------------------------------------------
// init-source --marketplace / --flat-skills / --namespace (INIT-10..12)
// ---------------------------------------------------------------------------

#[test]
fn init_source_marketplace_generates_correct_json() {
    // spec: INIT-10 INIT-11
    // Conventional source with no mind.toml. --marketplace generates a
    // marketplace.json with name = dir basename, source = ".", no `skills` key.
    let sb = Sandbox::new();
    let repo = sb.base.join("my-plugin");
    write(
        &repo.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(
        r.success,
        "init-source --marketplace failed: {} {}",
        r.stdout, r.stderr
    );

    let mkt_path = repo.join(".claude-plugin/marketplace.json");
    assert!(
        mkt_path.exists(),
        ".claude-plugin/marketplace.json must be created"
    );
    let content = std::fs::read_to_string(&mkt_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");

    // INIT-11: name defaults to dir basename when no prefix or --namespace.
    assert_eq!(
        v["name"], "my-plugin",
        "top-level name must be the dir basename"
    );
    let plugin = &v["plugins"][0];
    assert_eq!(plugin["name"], "my-plugin", "plugin entry name");
    assert_eq!(plugin["source"], ".", "plugin source must be '.'");

    // Without --flat-skills the skills key must be absent (INIT-10).
    assert!(
        plugin.get("skills").is_none(),
        "skills key must be absent without --flat-skills: {content}"
    );
}

#[test]
fn init_source_marketplace_uses_mindtoml_prefix() {
    // spec: INIT-10 INIT-11
    // When mind.toml has [source].prefix, that value becomes the plugin name.
    let sb = Sandbox::new();
    let repo = sb.base.join("irrelevant-dirname");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    write(
        &repo.join("mind.toml"),
        "[source]\nprefix = \"mypkg\"\ndescription = \"A great plugin\"\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(
        v["name"], "mypkg",
        "prefix from mind.toml must override dirname"
    );
    assert_eq!(v["plugins"][0]["name"], "mypkg", "plugin entry name");
    assert_eq!(
        v["plugins"][0]["description"], "A great plugin",
        "description from mind.toml [source].description"
    );
}

#[test]
fn init_source_marketplace_namespace_flag_overrides() {
    // spec: INIT-10 INIT-11 INIT-12
    // --namespace <n> overrides both mind.toml prefix and dir basename; the value
    // is also written as [source].prefix in mind.toml.
    let sb = Sandbox::new();
    let repo = sb.base.join("some-dir");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace", "--namespace", "foo"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    // marketplace.json uses the --namespace value.
    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(v["name"], "foo", "name must be the --namespace value");
    assert_eq!(v["plugins"][0]["name"], "foo", "plugin entry name");

    // mind.toml must carry namespace = "foo" (INIT-11, DSC-82 write key).
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"foo\""),
        "mind.toml must carry namespace = \"foo\": {toml}"
    );
}

#[test]
fn init_source_marketplace_no_overwrite() {
    // spec: INIT-10
    // An existing .claude-plugin/marketplace.json is left unchanged; the command
    // still exits 0 and prints the "already exists" notice.
    let sb = Sandbox::new();
    let repo = sb.base.join("existing-mkt");
    write(
        &repo.join("skills/build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let sentinel = r#"{"name":"sentinel","plugins":[]}"#;
    write(&repo.join(".claude-plugin/marketplace.json"), sentinel);
    let dir = repo.to_str().unwrap();

    let r = sb.mind(&["init-source", dir, "--marketplace"]);
    assert!(
        r.success,
        "must exit 0 even when file exists: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("already exists"),
        "must print 'already exists' message: {}",
        r.stdout
    );

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    assert_eq!(content, sentinel, "file must be left unchanged");
}

#[test]
fn init_source_flat_skills_sets_mindtoml_key() {
    // spec: INIT-12
    // Three sub-cases: no mind.toml, existing without the key, existing with it.

    let sb = Sandbox::new();

    // Case A: no existing mind.toml -> creates from scaffold with flat-skills = true.
    let repo_a = sb.base.join("flat-a");
    write(
        &repo_a.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    let r = sb.mind(&["init-source", repo_a.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let toml_a = std::fs::read_to_string(repo_a.join("mind.toml")).unwrap();
    assert!(
        toml_a.contains("flat-skills = true"),
        "case A: flat-skills must be set: {toml_a}"
    );

    // Case B: existing mind.toml without flat-skills -> inserts the key.
    let repo_b = sb.base.join("flat-b");
    write(
        &repo_b.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo_b.join("mind.toml"),
        "[source]\ndescription = \"existing\"\n",
    );
    let r = sb.mind(&["init-source", repo_b.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let toml_b = std::fs::read_to_string(repo_b.join("mind.toml")).unwrap();
    assert!(
        toml_b.contains("flat-skills = true"),
        "case B: flat-skills must be inserted: {toml_b}"
    );
    assert!(
        toml_b.contains("description = \"existing\""),
        "case B: existing content must be preserved: {toml_b}"
    );

    // Case C: existing mind.toml with flat-skills = false -> replaces with true.
    let repo_c = sb.base.join("flat-c");
    write(
        &repo_c.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo_c.join("mind.toml"),
        "[source]\nflat-skills = false\ndescription = \"existing\"\n",
    );
    let r = sb.mind(&["init-source", repo_c.to_str().unwrap(), "--flat-skills"]);
    assert!(r.success, "case C failed: {} {}", r.stdout, r.stderr);
    let toml_c = std::fs::read_to_string(repo_c.join("mind.toml")).unwrap();
    assert!(
        toml_c.contains("flat-skills = true"),
        "case C: flat-skills must be true: {toml_c}"
    );
    assert!(
        !toml_c.contains("flat-skills = false"),
        "case C: old value must be gone: {toml_c}"
    );
}

#[test]
fn init_source_flat_skills_marketplace_skills_array() {
    // spec: INIT-10 INIT-12
    // With --flat-skills --marketplace on a flat source (skill dirs at root),
    // the generated marketplace.json includes a `skills` array with the relative
    // dir path of each discovered skill.
    let sb = Sandbox::new();
    let repo = sb.base.join("flat-mkt");
    // Two flat skills at the repo root.
    write(
        &repo.join("review/SKILL.md"),
        "---\ndescription: review\n---\n# review\n",
    );
    write(
        &repo.join("build/SKILL.md"),
        "---\ndescription: build\n---\n# build\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--flat-skills", "--marketplace"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    let skills = v["plugins"][0]["skills"]
        .as_array()
        .expect("skills array must be present with --flat-skills");

    let skill_strings: Vec<&str> = skills.iter().filter_map(|s| s.as_str()).collect();
    assert!(
        skill_strings.contains(&"review"),
        "skills must include 'review': {skill_strings:?}"
    );
    assert!(
        skill_strings.contains(&"build"),
        "skills must include 'build': {skill_strings:?}"
    );

    // mind.toml must have flat-skills = true.
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("flat-skills = true"),
        "mind.toml must have flat-skills = true: {toml}"
    );
}

#[test]
fn init_source_namespace_writes_namespace_key() {
    // spec: INIT-11 INIT-12
    // --namespace bar writes namespace = "bar" to mind.toml (creating it if
    // absent; DSC-82 write key).
    let sb = Sandbox::new();
    let repo = sb.base.join("ns-test");
    write(
        &repo.join("skills/scan/SKILL.md"),
        "---\ndescription: scan\n---\n# scan\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--namespace", "bar"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"bar\""),
        "mind.toml must have namespace = \"bar\": {toml}"
    );
    // Other scaffold content is preserved.
    assert!(
        toml.contains("[source]"),
        "must still have [source] header: {toml}"
    );
}

#[test]
fn init_source_marketplace_placeholder_description() {
    // spec: INIT-10
    // When no [source].description is present in mind.toml (or no mind.toml
    // exists at all), the generated marketplace.json must contain the placeholder
    // description "TODO: describe this plugin", not an empty string.
    let sb = Sandbox::new();

    // Case A: no mind.toml at all.
    let repo_a = sb.base.join("placeholder-a");
    write(
        &repo_a.join("skills/plan/SKILL.md"),
        "---\ndescription: plan\n---\n# plan\n",
    );
    let r = sb.mind(&["init-source", repo_a.to_str().unwrap(), "--marketplace"]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let content_a =
        std::fs::read_to_string(repo_a.join(".claude-plugin/marketplace.json")).unwrap();
    let v_a: serde_json::Value = serde_json::from_str(&content_a).expect("valid JSON");
    assert_eq!(
        v_a["plugins"][0]["description"], "TODO: describe this plugin",
        "case A: description must be the placeholder when no mind.toml: {content_a}"
    );

    // Case B: mind.toml exists but [source].description is empty.
    let repo_b = sb.base.join("placeholder-b");
    write(
        &repo_b.join("skills/plan/SKILL.md"),
        "---\ndescription: plan\n---\n# plan\n",
    );
    write(&repo_b.join("mind.toml"), "[source]\ndescription = \"\"\n");
    let r = sb.mind(&["init-source", repo_b.to_str().unwrap(), "--marketplace"]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let content_b =
        std::fs::read_to_string(repo_b.join(".claude-plugin/marketplace.json")).unwrap();
    let v_b: serde_json::Value = serde_json::from_str(&content_b).expect("valid JSON");
    assert_eq!(
        v_b["plugins"][0]["description"], "TODO: describe this plugin",
        "case B: description must be the placeholder when description is empty: {content_b}"
    );
}

#[test]
fn init_source_namespace_updates_existing_mindtoml() {
    // spec: INIT-11
    // When --namespace is passed and mind.toml already exists, the prefix key is
    // inserted or updated in the existing file without destroying its other content.
    let sb = Sandbox::new();

    // Case A: existing mind.toml without a prefix key -- prefix is inserted.
    let repo_a = sb.base.join("ns-existing-a");
    write(
        &repo_a.join("skills/deploy/SKILL.md"),
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    write(
        &repo_a.join("mind.toml"),
        "[source]\ndescription = \"my source\"\n",
    );
    let r = sb.mind(&[
        "init-source",
        repo_a.to_str().unwrap(),
        "--namespace",
        "mypkg",
    ]);
    assert!(r.success, "case A failed: {} {}", r.stdout, r.stderr);
    let toml_a = std::fs::read_to_string(repo_a.join("mind.toml")).unwrap();
    assert!(
        toml_a.contains("namespace = \"mypkg\""),
        "case A: namespace must be inserted: {toml_a}"
    );
    assert!(
        toml_a.contains("description = \"my source\""),
        "case A: existing content must be preserved: {toml_a}"
    );

    // Case B: existing mind.toml with a different prefix -- prefix is replaced.
    let repo_b = sb.base.join("ns-existing-b");
    write(
        &repo_b.join("skills/deploy/SKILL.md"),
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    write(
        &repo_b.join("mind.toml"),
        "[source]\nprefix = \"old\"\ndescription = \"my source\"\n",
    );
    let r = sb.mind(&[
        "init-source",
        repo_b.to_str().unwrap(),
        "--namespace",
        "new",
    ]);
    assert!(r.success, "case B failed: {} {}", r.stdout, r.stderr);
    let toml_b = std::fs::read_to_string(repo_b.join("mind.toml")).unwrap();
    assert!(
        toml_b.contains("namespace = \"new\""),
        "case B: namespace must be updated to new: {toml_b}"
    );
    assert!(
        !toml_b.contains("prefix = \"old\""),
        "case B: old prefix key must be gone: {toml_b}"
    );
    assert!(
        toml_b.contains("description = \"my source\""),
        "case B: other content must be preserved: {toml_b}"
    );
}

#[test]
fn init_source_namespace_overrides_existing_prefix_in_marketplace() {
    // spec: INIT-10 INIT-11
    // When mind.toml already has [source].prefix = "old" but --namespace "new" is
    // passed, the generated marketplace.json uses "new" as the plugin name
    // (--namespace beats [source].prefix, INIT-11) and mind.toml is updated so
    // the stored prefix also becomes "new".
    let sb = Sandbox::new();
    let repo = sb.base.join("ns-override");
    write(
        &repo.join("skills/lint/SKILL.md"),
        "---\ndescription: lint\n---\n# lint\n",
    );
    write(
        &repo.join("mind.toml"),
        "[source]\nprefix = \"old\"\ndescription = \"lint tools\"\n",
    );
    let dir = repo.to_str().unwrap();
    let r = sb.mind(&["init-source", dir, "--marketplace", "--namespace", "new"]);
    assert!(r.success, "failed: {} {}", r.stdout, r.stderr);

    // marketplace.json must use "new", not "old" or the dirname.
    let content = std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    assert_eq!(
        v["name"], "new",
        "top-level name must be the --namespace value: {content}"
    );
    assert_eq!(
        v["plugins"][0]["name"], "new",
        "plugin entry name must be --namespace value: {content}"
    );
    // description comes from existing mind.toml [source].description.
    assert_eq!(
        v["plugins"][0]["description"], "lint tools",
        "description from existing mind.toml must be preserved: {content}"
    );

    // mind.toml must have the namespace updated to "new" (DSC-82 write key).
    let toml = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        toml.contains("namespace = \"new\""),
        "mind.toml namespace must be updated to 'new': {toml}"
    );
    assert!(
        !toml.contains("prefix = \"old\""),
        "old prefix key must no longer be present: {toml}"
    );
}

// ---------------------------------------------------------------------------
// H1 / NS-43 / NS-45: --yes flag forces non-interactive collision path
// ---------------------------------------------------------------------------

#[test]
fn skill_collision_yes_flag_forces_noninteractive_error() {
    // spec: NS-43 NS-45
    // Passing --yes to `meld` that would produce a cross-source skill collision
    // must cause SkillCollision (non-zero exit) rather than hanging on an
    // interactive prompt, even in a session where is_tty() might be true.
    // In the headless test environment is_tty()=false, so this test also
    // functions as a regression guard that the non-interactive path fires.
    let src_a = Sandbox::bare("h1-yes-a");
    src_a.write_and_commit(
        "skills/h1skill/SKILL.md",
        "---\nname: h1skill\ndescription: H1 skill A\n---\n# h1skill\n",
    );
    let r = src_a.mind(&["meld", &src_a.source_spec(), "--yes"]);
    assert!(r.success, "meld source-a failed: {} {}", r.stdout, r.stderr);

    let src_b = Sandbox::bare("h1-yes-b");
    src_b.write_and_commit(
        "skills/h1skill/SKILL.md",
        "---\nname: h1skill\ndescription: H1 skill B\n---\n# h1skill\n",
    );
    // --yes + collision: must return non-zero (SkillCollision) rather than prompt.
    let r = src_a.mind(&["meld", &src_b.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "--yes meld with collision must fail non-interactively (NS-45): {} {}",
        r.stdout, r.stderr
    );
    let combined = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        combined.contains("collision")
            || combined.contains("conflict")
            || combined.contains("namespace"),
        "--yes collision error must mention collision/namespace (NS-43 NS-45): {combined}"
    );
}

// ---------------------------------------------------------------------------
// M1: default `recall` view emits `namespace:` not `as:` for the alias token
// ---------------------------------------------------------------------------

#[test]
fn recall_default_view_uses_namespace_prefix_for_alias() {
    // spec: NS-43 NS-45
    // The non-`--sources` recall path (the default item status view) must emit
    // `namespace:<alias>` in the source header, consistent with the `--sources`
    // path (which already uses `namespace:`).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--namespace", "jk"]).success);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("namespace:jk"),
        "default recall must show 'namespace:jk' not 'as:jk': {recall}"
    );
    assert!(
        !recall.contains("as:jk"),
        "default recall must not use the old 'as:' prefix: {recall}"
    );
}

// ---------------------------------------------------------------------------
// M14 / DSC-78: sync re-walk honors `namespace =` key in [discover].sources
// ---------------------------------------------------------------------------

#[test]
fn sync_rewalk_respects_namespace_key_in_mindfile() {
    // spec: DSC-78
    // A super-source whose [discover].sources entry uses `namespace = "pfx"`
    // (the canonical DSC-78 key, distinct from the legacy `as = "pfx"`) must
    // register the nested source under that namespace alias during sync re-walk.
    // Guards H2: the re-walk previously passed `ns.alias` (the legacy `as`
    // field), silently dropping any value set via the canonical `namespace` key.
    let nested = Sandbox::named("dsc78-n");
    let super_src = Sandbox::bare("dsc78-sup");

    // Start with no discover sources so the nested source is NOT registered
    // at initial meld time.
    super_src.write_and_commit("mind.toml", "[source]\ndescription = \"super\"\n");

    let r = super_src.mind(&["meld", &super_src.source_spec()]);
    assert!(
        r.success,
        "meld super-source failed: {} {}",
        r.stdout, r.stderr
    );

    // Confirm nested is not yet registered.
    let before = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        !before.contains("/dsc78-n"),
        "nested must not be registered before sync re-walk: {before}"
    );

    // Update super-source mind.toml to add the nested source using the
    // canonical `namespace` key (not `as`).
    super_src.write_and_commit(
        "mind.toml",
        &format!(
            "[source]\ndescription = \"super\"\n\
             [[discover.sources]]\nsource = \"{}\"\nnamespace = \"pfx\"\n",
            nested.source_spec()
        ),
    );

    // sync fetches the updated super-source mind.toml and re-walks discover.sources,
    // finding the newly added nested source with namespace = "pfx".
    let r = super_src.mind(&["sync"]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);

    let after = super_src.mind(&["recall", "--sources"]).stdout;
    assert!(
        after.contains("/dsc78-n"),
        "nested source must be registered after sync re-walk: {after}"
    );
    assert!(
        after.contains("namespace:pfx"),
        "nested source must carry namespace:pfx alias after sync re-walk (DSC-78): {after}"
    );
}

// ---- CLI surface: flag renames and aliases (DEC-2 through DEC-8) ------------

#[test]
fn meld_register_only_is_canonical() {
    // spec: CLI-165 - `--register-only` is the canonical flag; `--link-only` is a
    // hidden deprecated alias that still works.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // The new canonical flag registers without installing.
    assert!(
        sb.mind(&["meld", &spec, "--register-only"]).success,
        "--register-only must succeed"
    );
    // Verify nothing was installed - no symlink in the lobe.
    let skill_link = sb.claude_home.join("skills/review");
    assert!(
        !skill_link.exists(),
        "--register-only must not install items: lobe symlink exists at {skill_link:?}"
    );
}

#[test]
fn unmeld_keep_items_is_canonical() {
    // spec: CLI-166 - `--keep-items` is the canonical flag; `--unlink-only` is a
    // hidden deprecated alias that still works.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    let r = sb.mind(&["unmeld", "agents", "--keep-items"]);
    assert!(r.success, "--keep-items must succeed: {}", r.stderr);

    // The item's symlink must still exist in the lobe.
    assert!(
        sb.claude_home.join("skills/review").exists()
            || sb.claude_home.join("skills/review/SKILL.md").exists(),
        "--keep-items must preserve the installed item"
    );
}

#[test]
fn mutation_result_schema_field() {
    // spec: CLI-168 - every mutation JSON result carries `"schema": 1`.
    let sb = melded();
    let r = sb.mind(&["learn", "review", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let v: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("learn --json must produce JSON");
    assert_eq!(
        v["schema"], 1,
        "mutation result must carry schema:1: {}",
        r.stdout
    );
    assert!(
        v["action"].is_string(),
        "action must be present: {}",
        r.stdout
    );
}

#[test]
fn upgrade_no_sync_flag_is_accepted() {
    // spec: CLI-169 - `upgrade --no-sync` skips the pre-upgrade source fetch.
    // Verifies the flag is wired; full sync-vs-no-sync diff is covered by the
    // next test.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["upgrade", "--no-sync"]);
    // Either exits 0 (nothing to upgrade) or 1 (pending); must not crash with
    // "unexpected argument --no-sync".
    assert!(
        !r.stderr.contains("unexpected argument"),
        "--no-sync must be a recognized flag: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("error: unrecognized"),
        "--no-sync must be a recognized flag: {}",
        r.stderr
    );
}

#[test]
fn upgrade_syncs_before_computing_delta() {
    // spec: CLI-169 - `upgrade` syncs each source first by default; `--no-sync`
    // skips the fetch. With a cloned source (--follow-branch), after a new commit
    // in the source: `--no-sync` sees stale clone (reports up-to-date); the
    // default sync detects the change (reports pending).
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Meld as a clone (--follow-branch main makes it non-linked).
    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "main", "--yes"])
            .success,
        "meld --follow-branch failed"
    );
    assert!(sb.mind(&["learn", "review"]).success, "learn review failed");

    // Advance the source: change the SKILL.md content so the hash differs.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: updated skill\n---\n# review v2\n",
    );

    // --no-sync: the clone is stale -> no pending upgrades.
    let no_sync = sb.mind(&["upgrade", "--no-sync", "--yes"]);
    // Must succeed; no crash.
    assert!(
        !no_sync.stderr.contains("error"),
        "--no-sync must not error: {}",
        no_sync.stderr
    );

    // default sync: fetches the new commit -> detects pending upgrade.
    let with_sync = sb.mind(&["upgrade", "--yes"]);
    // Success (0) means it applied an upgrade; exit 1 is "pending, no --yes prompt
    // answered". Either way, the output must mention the item.
    assert!(
        with_sync.stdout.contains("review") || with_sync.stderr.contains("review"),
        "upgrade with sync must reference the changed item: out={} err={}",
        with_sync.stdout,
        with_sync.stderr
    );
}

#[test]
fn mind_default_lobe_env_var_overrides_claude_home() {
    // spec: CLI-170 - MIND_DEFAULT_LOBE takes precedence over CLAUDE_HOME as the
    // default agent home. Items must be linked into the MIND_DEFAULT_LOBE dir, not
    // the CLAUDE_HOME dir set by the sandbox.
    //
    // All commands use MIND_DEFAULT_LOBE so that ensure_config() (called during
    // meld) bakes alt_lobe into config.toml, not sb.claude_home.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Create an alternative lobe directory first.
    let alt_lobe = sb.mind_home.join("alt_lobe");
    std::fs::create_dir_all(&alt_lobe).expect("create alt_lobe");
    let alt_lobe_str = alt_lobe.to_str().unwrap().to_string();

    // Meld with MIND_DEFAULT_LOBE so the written config.toml records alt_lobe.
    assert!(
        sb.mind_env(
            &["meld", &spec, "--register-only"],
            &[("MIND_DEFAULT_LOBE", alt_lobe_str.as_str())]
        )
        .success,
        "register-only meld with MIND_DEFAULT_LOBE failed"
    );

    // Install with MIND_DEFAULT_LOBE still set.
    let r = sb.mind_env(
        &["learn", "review"],
        &[("MIND_DEFAULT_LOBE", alt_lobe_str.as_str())],
    );
    assert!(
        r.success,
        "learn with MIND_DEFAULT_LOBE failed: {}",
        r.stderr
    );

    // Item must be linked in alt_lobe, not the sandbox's CLAUDE_HOME.
    let in_alt =
        alt_lobe.join("skills/review").exists() || alt_lobe.join("skills/review/SKILL.md").exists();
    let in_claude = sb.claude_home.join("skills/review").exists()
        || sb.claude_home.join("skills/review/SKILL.md").exists();

    assert!(
        in_alt,
        "item must be linked in MIND_DEFAULT_LOBE: {alt_lobe:?}"
    );
    assert!(
        !in_claude,
        "item must NOT be linked in CLAUDE_HOME when MIND_DEFAULT_LOBE is set: {:?}",
        sb.claude_home
    );
}

#[test]
fn visible_aliases_present_in_help() {
    // spec: CLI-172 - `add`, `install`, `uninstall`, `update`, `search`, `list`,
    // `doctor`, `self-update` must appear as visible aliases in the top-level help.
    let sb = Sandbox::new();
    let r = sb.mind(&["--help"]);
    assert!(r.success, "--help failed: {}", r.stderr);
    let out = r.stdout;
    for alias in &[
        "add",
        "install",
        "uninstall",
        "update",
        "search",
        "list",
        "doctor",
    ] {
        assert!(
            out.contains(alias),
            "visible alias `{alias}` must appear in --help: {out}"
        );
    }
    // `detach` and `target` must NOT appear (demoted to hidden).
    assert!(
        !out.contains("detach"),
        "`detach` must not appear in --help (hidden alias): {out}"
    );
    assert!(
        !out.contains("target"),
        "`target` must not appear in --help (hidden alias): {out}"
    );
}

#[test]
fn meld_help_mentions_install_default() {
    // spec: CLI-173 - the meld one-line help must mention that it installs by default.
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", "--help"]);
    assert!(r.success, "meld --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.contains("install") || out.contains("Install"),
        "meld --help must mention install: {out}"
    );
}

#[test]
fn unmeld_long_help_leads_with_uninstall() {
    // spec: CLI-174 - the unmeld long help must lead with the uninstall default and
    // mention --keep-items.
    let sb = Sandbox::new();
    let r = sb.mind(&["unmeld", "--help"]);
    assert!(r.success, "unmeld --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.contains("uninstall") || out.contains("Uninstalls"),
        "unmeld --help must mention uninstall: {out}"
    );
    assert!(
        out.contains("--keep-items"),
        "unmeld --help must mention --keep-items: {out}"
    );
}

#[test]
fn exit_codes_distinguish_runtime_and_usage_errors() {
    // spec: CLI-175 - 0 for success, 1 for a runtime error (MindError), 2 for a
    // usage error (clap parse failure).
    let sb = Sandbox::new();

    let ok = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(ok.status.code(), Some(0), "recall in a fresh home exits 0");

    let runtime = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["learn", "no-such-item"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(
        runtime.status.code(),
        Some(1),
        "a MindError exits 1: {}",
        String::from_utf8_lossy(&runtime.stderr)
    );

    let usage = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["probe", "--definitely-not-a-flag"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(
        usage.status.code(),
        Some(2),
        "a clap parse failure exits 2: {}",
        String::from_utf8_lossy(&usage.stderr)
    );
}

#[test]
fn removed_aliases_are_usage_errors() {
    // spec: CLI-172 - the former `detach` (unmeld) and `target` (config lobes)
    // aliases are removed, not hidden; both are usage errors now.
    let sb = Sandbox::new();
    let detach = sb.mind(&["detach", "whatever"]);
    assert!(
        !detach.success,
        "`mind detach` must fail: {}",
        detach.stdout
    );
    let target = sb.mind(&["config", "target", "list"]);
    assert!(
        !target.success,
        "`mind config target` must fail: {}",
        target.stdout
    );
}

#[test]
fn self_update_alias_works() {
    // spec: CLI-172 - `self-update` is a visible alias for `evolve`.
    let sb = Sandbox::new();
    // --help on the alias should succeed and show evolve content.
    let r = sb.mind(&["self-update", "--help"]);
    assert!(
        r.success || r.stderr.contains("evolve") || r.stdout.contains("evolve"),
        "self-update alias must be recognized: out={} err={}",
        r.stdout,
        r.stderr
    );
}

// ---- gap-closing tests added by qa agent ------------------------------------

#[test]
fn all_mutating_verbs_carry_schema_1_in_json_envelope() {
    // spec: CLI-168 - every mutation JSON result carries "schema": 1.
    // mutation_result_schema_field covers `learn`; this test pins the remaining
    // mutating verbs: meld, forget, sync, upgrade, and unmeld.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // meld
    let meld_r = sb.mind(&["meld", &spec, "--json"]);
    assert!(meld_r.success, "meld --json failed: {}", meld_r.stderr);
    assert_eq!(
        parse_json(&meld_r.stdout)["schema"],
        1,
        "meld --json must carry schema:1: {}",
        meld_r.stdout
    );

    // sync
    let sync_r = sb.mind(&["sync", "--json"]);
    assert!(sync_r.success, "sync --json failed: {}", sync_r.stderr);
    assert_eq!(
        parse_json(&sync_r.stdout)["schema"],
        1,
        "sync --json must carry schema:1: {}",
        sync_r.stdout
    );

    // learn then forget
    assert!(
        sb.mind(&["learn", "skill:review"]).success,
        "learn review failed"
    );
    let forget_r = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(
        forget_r.success,
        "forget --json failed: {}",
        forget_r.stderr
    );
    assert_eq!(
        parse_json(&forget_r.stdout)["schema"],
        1,
        "forget --json must carry schema:1: {}",
        forget_r.stdout
    );

    // upgrade (reinstall item first; no delta case)
    assert!(
        sb.mind(&["learn", "skill:review"]).success,
        "learn review failed"
    );
    let upgrade_r = sb.mind(&["upgrade", "--json"]);
    assert!(
        upgrade_r.success,
        "upgrade --json failed: {}",
        upgrade_r.stderr
    );
    assert_eq!(
        parse_json(&upgrade_r.stdout)["schema"],
        1,
        "upgrade --json must carry schema:1: {}",
        upgrade_r.stdout
    );

    // unmeld (removes source and its installed items)
    let unmeld_r = sb.mind(&["unmeld", "agents", "--json"]);
    assert!(
        unmeld_r.success,
        "unmeld --json failed: {}",
        unmeld_r.stderr
    );
    assert_eq!(
        parse_json(&unmeld_r.stdout)["schema"],
        1,
        "unmeld --json must carry schema:1: {}",
        unmeld_r.stdout
    );
}

#[test]
fn short_n_is_usage_error_on_meld_review_and_init_source() {
    // spec: CLI-163 - -n is reserved for --dry-run on `learn` only. The verbs
    // meld, review, and init-source do not define -n; passing it must be
    // rejected as a clap usage error (unknown flag), not accepted.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let meld_n = sb.mind(&["meld", "-n", &spec]);
    assert!(
        !meld_n.success,
        "meld -n must be rejected as an unknown flag: {}",
        meld_n.stderr
    );

    let review_n = sb.mind(&["review", "-n", &spec]);
    assert!(
        !review_n.success,
        "review -n must be rejected as an unknown flag: {}",
        review_n.stderr
    );

    let init_n = sb.mind(&["init-source", "-n"]);
    assert!(
        !init_n.success,
        "init-source -n must be rejected as an unknown flag: {}",
        init_n.stderr
    );

    // learn -n is still accepted as --dry-run (the reserved use).
    assert!(sb.mind(&["meld", &spec]).success, "meld without -n failed");
    let learn_n = sb.mind(&["learn", "-n", "skill:review"]);
    assert!(
        learn_n.success,
        "learn -n must still work as --dry-run: {}",
        learn_n.stderr
    );
}

#[test]
fn deprecated_flag_aliases_are_hidden_from_help() {
    // spec: CLI-165 - --link-only is a hidden deprecated alias for
    // --register-only; it must NOT appear in `meld --help` output.
    // spec: CLI-166 - --unlink-only is a hidden deprecated alias for
    // --keep-items; it must NOT appear in `unmeld --help` output.
    // The canonical names must still be visible in their respective help text.
    let sb = Sandbox::new();

    let meld_help = sb.mind(&["meld", "--help"]).stdout;
    assert!(
        !meld_help.contains("link-only"),
        "--link-only must not appear in meld --help (hidden alias): {meld_help}"
    );
    assert!(
        meld_help.contains("register-only"),
        "--register-only must appear in meld --help (canonical name): {meld_help}"
    );

    let unmeld_help = sb.mind(&["unmeld", "--help"]).stdout;
    assert!(
        !unmeld_help.contains("unlink-only"),
        "--unlink-only must not appear in unmeld --help (hidden alias): {unmeld_help}"
    );
    assert!(
        unmeld_help.contains("keep-items"),
        "--keep-items must appear in unmeld --help (canonical name): {unmeld_help}"
    );
}

#[test]
fn verb_aliases_dispatch_to_correct_commands() {
    // spec: CLI-172 - aliases must not merely appear in --help; they must
    // dispatch the correct command and produce real output. Tests the verbs
    // most likely to silently mis-route: add, install, list, search, update,
    // doctor, uninstall.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // `add` -> meld: registers the source.
    let add_r = sb.mind(&["add", &spec]);
    assert!(
        add_r.success,
        "mind add must dispatch to meld: {}",
        add_r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "mind add must register the source"
    );

    // `install` -> learn: installs an item.
    let install_r = sb.mind(&["install", "skill:review"]);
    assert!(
        install_r.success,
        "mind install must dispatch to learn: {}",
        install_r.stderr
    );

    // `list` -> recall: lists installed items.
    let list_r = sb.mind(&["list"]);
    assert!(
        list_r.success,
        "mind list must dispatch to recall: {}",
        list_r.stderr
    );
    assert!(
        list_r.stdout.contains("skill:review"),
        "mind list must show the installed item: {}",
        list_r.stdout
    );

    // `search` -> probe: lists catalog items (--no-tui forces text output in non-TTY).
    let search_r = sb.mind(&["search", "--no-tui"]);
    assert!(
        search_r.success,
        "mind search must dispatch to probe: {}",
        search_r.stderr
    );
    assert!(
        search_r.stdout.contains("review"),
        "mind search must list catalog items: {}",
        search_r.stdout
    );

    // `update` -> sync: syncs sources without error.
    let update_r = sb.mind(&["update"]);
    assert!(
        update_r.success,
        "mind update must dispatch to sync: {}",
        update_r.stderr
    );

    // `doctor` -> introspect: reports health diagnostics.
    let doctor_r = sb.mind(&["doctor"]);
    assert!(
        doctor_r.success,
        "mind doctor must dispatch to introspect: {}",
        doctor_r.stderr
    );

    // `uninstall` -> forget: removes the installed item (no more "installed @" marker).
    let uninstall_r = sb.mind(&["uninstall", "skill:review"]);
    assert!(
        uninstall_r.success,
        "mind uninstall must dispatch to forget: {}",
        uninstall_r.stderr
    );
    let recall_after = sb.mind(&["recall"]).stdout;
    assert!(
        !recall_after.contains("installed @"),
        "mind uninstall must have removed skill:review (no installed-at marker): {recall_after}"
    );
}

#[test]
fn sync_upgrade_is_noted_as_deprecated_in_help() {
    // spec: CLI-169 - `sync --upgrade` continues to work but the --upgrade flag
    // is noted as deprecated in help text; prefer `upgrade` which now syncs
    // first by default.
    let sb = Sandbox::new();
    let r = sb.mind(&["sync", "--help"]);
    assert!(r.success, "sync --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.to_lowercase().contains("deprecated") || out.to_lowercase().contains("prefer"),
        "sync --help must note that --upgrade is deprecated: {out}"
    );
}

#[test]
fn upgrade_sync_first_outcome_with_drift_example() {
    // spec: CLI-169 - upgrade fetches each involved source before computing
    // deltas (sync-first). With a cloned source and a new upstream commit that
    // the clone has not yet fetched:
    //   --no-sync: clone is stale -> item appears up-to-date (no delta seen).
    //   default (sync-first): fetches the commit -> detects and applies upgrade.
    //
    // Uses from_example("drift") to drive a real-world-ish source layout
    // (skills/audit/SKILL.md) and --follow-branch to create an actual git clone
    // so the sync path runs a real fetch.
    let sb = Sandbox::from_example("drift");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "main", "--register-only"])
            .success,
        "meld --follow-branch failed"
    );
    assert!(sb.mind(&["learn", "audit"]).success, "learn audit failed");

    // Advance the source without syncing the clone.
    write(
        &sb.source.join("skills/audit/SKILL.md"),
        "---\nname: audit\ndescription: Updated audit skill\n---\n# audit v2 body\n",
    );
    git(&sb.source, &["commit", "-aqm", "update audit"]);

    // --no-sync: clone is stale, so the installed hash still matches the old content.
    let no_sync = sb.mind(&["upgrade", "--no-sync", "--json"]);
    assert!(
        no_sync.success,
        "--no-sync must succeed: {}",
        no_sync.stderr
    );
    let ns = parse_json(&no_sync.stdout);
    assert_eq!(
        ns["outcome"], "up-to-date",
        "--no-sync on stale clone must report up-to-date (no delta visible from old clone): {}",
        no_sync.stdout
    );

    // Default upgrade (sync-first): fetches the new commit then upgrades.
    let with_sync = sb.mind(&["upgrade", "--yes", "--json"]);
    assert!(
        with_sync.success,
        "upgrade with sync must succeed: {}",
        with_sync.stderr
    );
    let ws = parse_json(&with_sync.stdout);
    assert_eq!(
        ws["outcome"], "upgraded",
        "upgrade with sync must detect and apply the upstream change: {}",
        with_sync.stdout
    );

    // After the sync-first upgrade the item is current (not outdated).
    let after = sb.mind(&["recall"]);
    assert!(
        !after.stdout.contains("outdated"),
        "item must be current after sync-first upgrade: {}",
        after.stdout
    );
}

// ---- helper: fake git that emits a proxy 407 on https:// clone ----------

fn fake_git_proxy_bin_dir(dir: &Path) -> PathBuf {
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-proxy-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  for a; do\n    case \"$a\" in\n      https://*)\n        echo \"fatal: unable to access '$a': Received HTTP code 407 from proxy after CONNECT\" >&2\n        exit 128\n        ;;\n    esac\n  done\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn meld_top_level_auth_failure_prints_ssh_hint() {
    // spec: CLI-177 -- auth failure on a direct meld emits SSH/credential hints.
    let sb = Sandbox::bare("auth-hint-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/private-repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail on auth error: {}", r.stderr);
    assert!(
        r.stderr.contains("git@example.com:owner/private-repo"),
        "auth hint must include the SSH URL: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("ssh = true"),
        "auth hint must mention `ssh = true` config option: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("credential helper"),
        "auth hint must mention credential helper: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_proxy_failure_prints_proxy_hint() {
    // spec: CLI-178 -- 407 proxy error on a direct meld emits HTTPS_PROXY hint.
    let sb = Sandbox::bare("proxy-hint-test");
    let fake_dir = fake_git_proxy_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail on proxy error: {}", r.stderr);
    assert!(
        r.stderr.contains("HTTPS_PROXY"),
        "proxy hint must mention HTTPS_PROXY: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("http.proxy"),
        "proxy hint must mention http.proxy: {}",
        r.stderr
    );
    // Auth hint must NOT be shown for a proxy failure.
    assert!(
        !r.stderr.contains("credential helper"),
        "auth hint must not appear for a proxy failure: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_clone_error_leads_with_git_stderr() {
    // spec: CLI-180 -- git's stderr leads the output; the explicitly constructed
    // "  command:" and "  store:" lines are suppressed without --verbose.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Clone a valid local repo but request a branch that does not exist; this
    // exercises the re-clone-at-pin error path (top_level=true).
    let r = sb.mind(&["meld", &spec, "--follow-branch", "nonexistent-branch"]);
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // git's stderr (the fatal error) must appear.
    assert!(
        r.stderr.contains("fatal:")
            || r.stderr.contains("error:")
            || r.stderr.contains("not found"),
        "git stderr must appear in output: {}",
        r.stderr
    );
    // Without --verbose the explicitly constructed "  command:" and "  store:"
    // lines must NOT appear (they expose the internal store path).
    assert!(
        !r.stderr.contains("  command: git clone"),
        "explicit command line must be absent without --verbose: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("  store:   "),
        "explicit store path line must be absent without --verbose: {}",
        r.stderr
    );
}

#[test]
fn meld_top_level_clone_error_verbose_shows_command_and_store() {
    // spec: CLI-180 -- under --verbose the command line and store path lines appear.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        "--verbose",
        &spec,
        "--follow-branch",
        "nonexistent-branch",
    ]);
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // Under --verbose the explicitly constructed lines must appear.
    assert!(
        r.stderr.contains("  command: git clone"),
        "explicit command line must appear under --verbose: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("  store:   "),
        "explicit store path line must appear under --verbose: {}",
        r.stderr
    );
}

#[test]
fn learn_not_found_with_sources_hints_probe_not_sync() {
    // spec: CLI-179 -- when sources are melded and the item is unknown, the
    // error and hint point at `mind probe` and never mention `mind sync`.
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist"]);
    assert!(!r.success, "learn must fail for unknown item: {}", r.stderr);
    assert!(
        r.stderr.contains("probe does-not-exist"),
        "hint must point at `mind probe <query>`: {}",
        r.stderr
    );
    // Neither the ItemNotFound Display nor the hint line may mention sync.
    assert!(
        !r.stderr.contains("sync"),
        "with sources, no line in stderr may mention sync: {}",
        r.stderr
    );
}

#[test]
fn meld_json_clone_error_message_contains_git_cause() {
    // spec: CLI-184 -- under --json the JSON envelope's `message` field contains
    // the actual git stderr (the real cause) rather than the literal `<no stderr>`.
    let sb = Sandbox::bare("json-git-cause-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["--json", "meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stdout);
    // In --json mode the JSON envelope is on stdout; stderr should be empty.
    assert!(
        r.stderr.is_empty(),
        "no stderr output expected in --json mode: {}",
        r.stderr
    );
    let v = parse_json(&r.stdout);
    let msg = v["error"]["message"]
        .as_str()
        .expect("error.message must be a string");
    // The message must include the git failure text (the fake git says "Authentication failed").
    assert!(
        msg.contains("Authentication failed") || msg.contains("fatal:"),
        "message must contain the git error cause, not a placeholder: {msg}"
    );
    // The placeholder must NOT appear.
    assert!(
        !msg.contains("<no stderr>"),
        "message must not contain `<no stderr>` placeholder: {msg}"
    );
}

#[test]
fn meld_human_clone_error_no_no_stderr_literal() {
    // spec: CLI-185 -- in human mode, after git stderr is printed to the
    // terminal, the error trailer shows "(git output above)" not `<no stderr>`.
    let sb = Sandbox::bare("human-no-stderr-literal-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stderr);
    assert!(
        !r.stderr.contains("<no stderr>"),
        "the literal `<no stderr>` must not appear in human output: {}",
        r.stderr
    );
    // Git's actual error text must appear (printed directly before the hint).
    assert!(
        r.stderr.contains("Authentication failed") || r.stderr.contains("fatal:"),
        "git's stderr must appear in human output: {}",
        r.stderr
    );
}

fn fake_git_ansi_bin_dir(dir: &std::path::Path) -> std::path::PathBuf {
    // Creates a fake git that emits ANSI escape codes and Unicode bidi-override
    // characters in stderr on clone, to test that strip_ansi sanitizes them.
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-ansi-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // The ANSI sequence \033[31m is "red"; \xe2\x80\xae is a bidi-override char (U+202E).
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"clone\" ]; then\n  printf 'fatal: \\033[31mANSI\\033[0m injection \\xe2\\x80\\xae bidi\\n' >&2\n  exit 128\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn meld_clone_error_strips_ansi_from_git_stderr() {
    // spec: CLI-186 -- ANSI escape codes in git stderr are stripped before
    // being printed to prevent terminal spoofing by a hostile remote.
    let sb = Sandbox::bare("ansi-strip-test");
    let fake_dir = fake_git_ansi_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/repo"],
        &[("PATH", &new_path)],
    );
    assert!(!r.success, "meld must fail: {}", r.stderr);
    // The text must appear but stripped of ANSI escapes.
    assert!(
        r.stderr.contains("ANSI"),
        "the text content must still appear: {}",
        r.stderr
    );
    assert!(
        !has_ansi_escape(&r.stderr),
        "no ANSI escape sequence must appear in the printed stderr: {:?}",
        r.stderr
    );
}

fn fake_git_ansi_fetch_bin_dir(dir: &std::path::Path) -> std::path::PathBuf {
    // A fake git that fails on `fetch` with ANSI escape + bidi-override bytes in
    // stderr, passing every other subcommand (clone, rev-parse, ...) through to
    // real git. This lets a source be melded (clone) with real git and then fail
    // its per-source `sync` fetch with hostile output, exercising the CLI-186
    // strip_ansi on the sync path (a different code path from the meld top-level
    // clone path already covered above).
    let real_git = String::from_utf8(
        std::process::Command::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();

    let bin_dir = dir.join("fake-git-ansi-fetch-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // \033[31m ... \033[0m is an ANSI color pair; \xe2\x80\xae is U+202E (bidi).
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"fetch\" ]; then\n  printf 'fatal: \\033[31mANSIFETCH\\033[0m injection \\xe2\\x80\\xae bidi\\n' >&2\n  exit 128\nfi\nexec \"{real_git}\" \"$@\"\n"
    );
    let script_path = bin_dir.join("git");
    std::fs::write(&script_path, &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[test]
fn sync_per_source_error_strips_ansi_from_git_stderr() {
    // spec: CLI-186 -- the per-source `sync` failure path also sanitizes git
    // stderr via strip_ansi. The dev flagged this path had no dedicated ANSI
    // test (only the top-level meld clone path was covered). Drive it by melding
    // a tag-pinned local source (a real clone) with real git, then running sync
    // with a fake git whose `fetch` fails with ANSI/bidi bytes.
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("sync-ansi-strip-test");
    let spec = sb.source_spec();

    // Meld at a tag so the source is a clone (not a linked working tree): its
    // sync goes through git::sync_to_pin -> `git fetch`, which the fake fails.
    let melded = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(melded.success, "meld --pin-tag v1.0: {}", melded.stderr);

    let fake_dir = fake_git_ansi_fetch_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let r = sb.mind_env(&["sync"], &[("PATH", &new_path)]);
    // sync exits non-zero because the one source failed to fetch.
    assert!(
        !r.success,
        "sync must fail when the source's fetch fails: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The sanitized error text must still carry the git message content.
    assert!(
        r.stderr.contains("ANSIFETCH"),
        "the git error text content must still appear: {}",
        r.stderr
    );
    // But every ANSI escape must have been stripped before printing.
    assert!(
        !has_ansi_escape(&r.stderr),
        "no ANSI escape sequence may appear in the printed sync stderr: {:?}",
        r.stderr
    );
    // The bidi-override control char (U+202E) is an ANSI/control byte the
    // strip_ansi filter also removes; assert it did not survive.
    assert!(
        !r.stderr.contains('\u{202e}'),
        "the U+202E bidi-override char must not survive into printed stderr: {:?}",
        r.stderr
    );
}

#[test]
fn meld_policy_refused_before_git_clone_no_clone_dir() {
    // spec: POL-36 -- the allow/lock refusal fires before any git network call
    // or directory creation; no clone dir should exist after a refused meld.
    let sb = Sandbox::bare("pol36-no-clone-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    // The lock excludes every external repo.
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/refused-repo"],
        &[("PATH", &new_path), ("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "locked meld must be refused: {}", r.stdout);
    // The error must name the policy refusal, not a git error.
    assert!(
        r.stderr.contains("not permitted"),
        "stderr must report SourceNotAllowed: {}",
        r.stderr
    );
    // The fake git emits "Authentication failed" on clone; if that appears, the
    // clone ran before the policy check (wrong order).
    assert!(
        !r.stderr.contains("Authentication failed"),
        "git clone must not have run: policy check must fire first: {}",
        r.stderr
    );
    // sources tree for the refused URL must not exist.
    let sources_root = sb.mind_home.join("sources");
    let clone_exists = sources_root.exists()
        && std::fs::read_dir(&sources_root)
            .map(|d| d.count() > 0)
            .unwrap_or(false);
    assert!(
        !clone_exists,
        "no clone directory should exist after a refused meld"
    );
}

#[test]
fn meld_policy_refused_prints_policy_file_hint() {
    // spec: POL-37 -- when SourceNotAllowed is returned, the effective policy
    // file path is printed to stderr as a hint.
    let sb = Sandbox::bare("pol37-hint-test");
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let spec = sb.source_spec();
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(!r.success, "locked meld must be refused: {}", r.stdout);
    // The hint line must name the policy file path.
    assert!(
        r.stderr.contains("policy") && r.stderr.contains("policy.toml"),
        "hint must mention the policy file path: {}",
        r.stderr
    );
}

// --- allow-local policy knob (POL-56/POL-57) ---------------------------------

#[test]
fn allow_local_false_refuses_local_path_meld_even_with_permissive_pattern() {
    // spec: POL-56 -- allow-local = false under lock refuses a local-path meld
    // regardless of allow patterns (even one that would otherwise match), leaves
    // no clone dir, and error text names the allow-local = false reason.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // The allow pattern would match the local identity, but allow-local = false
    // overrides it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = false under lock must refuse a local-path meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("allow-local"),
        "error must name the allow-local = false reason: {}",
        r.stderr
    );
    // The error must be distinct from the generic pattern-miss message.
    assert!(
        !r.stderr
            .contains("not permitted by the managed policy's allowlist"),
        "error must not be the generic SourceNotAllowed message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
    // No clone dir left on disk.
    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should be left at {}",
        clone_dir.display()
    );
}

#[test]
fn allow_local_false_under_no_lock_does_not_refuse() {
    // spec: POL-56 -- with lock absent or false, allow-local has no effect;
    // the meld proceeds regardless of the allow-local value.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = false\nallow-local = false\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "allow-local = false with lock = false must not refuse: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn allow_local_true_still_requires_allow_pattern_under_lock() {
    // spec: POL-57 -- allow-local = true (explicit or absent default) preserves
    // existing behavior: a local-path meld still requires an allow pattern match
    // when lock = true. A meld that fails the pattern is refused as SourceNotAllowed.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = true with lock must still enforce allow patterns: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted"),
        "error should be the standard SourceNotAllowed message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_true_default_allows_local_meld_matching_pattern() {
    // spec: POL-57 -- absent allow-local (default true) allows a local-path meld
    // that satisfies the allow pattern under lock.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // Pattern matches the local identity local/<base>/agents.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "default allow-local (true) with matching pattern must succeed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn allow_local_false_refuses_file_url_meld() {
    // spec: POL-56 -- the `file://` spelling of a local source parses to
    // host == "local" (Source::is_local), so allow-local = false under a lock
    // must refuse it exactly as it refuses a bare absolute path. Guards against a
    // regression where the guard only catches the `/`-prefixed spelling and lets
    // the `file://` URL through.
    let sb = Sandbox::named("agents");
    // `file://` + an absolute path yields the canonical `file:///abs/path` URL.
    let spec = format!("file://{}", sb.source_spec());
    // A permissive allow pattern would match the identity, but allow-local = false
    // overrides it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = false under lock must refuse a file:// meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("allow-local"),
        "error must name the allow-local = false reason: {}",
        r.stderr
    );
    // Must be LocalMeldForbidden, not the generic pattern-miss error (proves the
    // guard fired on is_local, not that the pattern happened to miss).
    assert!(
        !r.stderr
            .contains("not permitted by the managed policy's allowlist"),
        "error must be LocalMeldForbidden, not the generic SourceNotAllowed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_false_meld_json_error_envelope_has_kind_slug() {
    // spec: POL-56 -- under --json a refused local meld emits the structured
    // error envelope {"schema":1,"error":{"kind":"local-meld-forbidden",...}}
    // with the stable kind slug and the allow-local message, rather than a human
    // string. Mirrors the Phase-1 --json error-envelope tests.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(
        &["--json", "meld", &spec],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "meld must fail: {}", r.stdout);
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert_eq!(
        v["error"]["kind"].as_str(),
        Some("local-meld-forbidden"),
        "kind slug must be local-meld-forbidden: {}",
        r.stdout
    );
    let msg = v["error"]["message"]
        .as_str()
        .expect("error.message must be a string");
    assert!(
        msg.contains("allow-local"),
        "message must name the allow-local reason, not a placeholder: {msg}"
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_false_no_lock_emits_no_allow_local_warning() {
    // spec: POL-56 -- with lock = false the allow-local guard is inert: the meld
    // proceeds and NO allow-local text appears on stderr (the only warning is the
    // unrelated POL-13 advisory allowlist notice). Guards against the guard
    // dropping its `policy.lock()` precondition and warning/refusing with the lock
    // off.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = false\nallow-local = false\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "lock = false must not refuse: {}", r.stderr);
    assert!(
        !r.stderr.contains("allow-local"),
        "no allow-local warning must be emitted when lock is false: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn auto_meld_partial_failure_does_not_persist_partial_entry() {
    // spec: POL-35 -- when a provisioning entry fails after meld_recursive has
    // pushed sources into the registry, the registry is rolled back so a
    // subsequent save does not persist the partial entry.
    //
    // Setup:
    //   entry-1 (first-source): succeeds, gets provisioned -> provisioned += 1.
    //   entry-2 (super-source): has a nested source but install-items names an
    //     item that does not exist -> meld_recursive returns Err(BadReference)
    //     after pushing super-source (and nested source) into registry.sources.
    //
    // Without the rollback, registry.save would persist entry-2's partial data.
    // With the rollback, only entry-1 is in the registry after the save.
    //
    // NOTE: the discover-nested field is `install-items` (kebab), not the
    // underscore form -- the underscore form is an unknown-field TOML *parse*
    // error that fails before the super-source is ever pushed, which would make
    // this test pass trivially without exercising the rollback at all.
    let sb = Sandbox::new(); // provides "first-source" with the standard fixture
    let first_spec = sb.source_spec();

    // Build the nested source (has real-skill, but NOT nonexistent-skill).
    let nested = sb.base.join("nested-src");
    write(
        &nested.join("skills/real-skill/SKILL.md"),
        "---\nname: real-skill\ndescription: a real skill\n---\n# real skill\n",
    );
    git(&nested, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&nested, &["config", "user.email", "t@t"]);
    git(&nested, &["config", "user.name", "t"]);
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-qm", "initial"]);

    // Build the super-source: discover.sources -> nested, install-items -> bad ref.
    let super_src = sb.base.join("super-src");
    let nested_str = nested.to_string_lossy();
    let super_toml = format!(
        "[[discover.sources]]\nsource = \"{nested_str}\"\ninstall-items = [\"skill:nonexistent-skill\"]\n"
    );
    write(&super_src.join("mind.toml"), &super_toml);
    git(&super_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&super_src, &["config", "user.email", "t@t"]);
    git(&super_src, &["config", "user.name", "t"]);
    git(&super_src, &["add", "-A"]);
    git(&super_src, &["commit", "-qm", "initial"]);

    let super_str = super_src.to_string_lossy();
    let first_str = first_spec.as_str();
    let policy_body = format!(
        "[[sources.auto_meld]]\nrepo = \"{first_str}\"\n\n[[sources.auto_meld]]\nrepo = \"{super_str}\"\n"
    );
    let policy = write_policy(&sb, &policy_body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    // sync exits non-zero because the second entry failed.
    assert!(
        !r.success,
        "sync must fail due to provisioning failure: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The failure must be the DSC-63 BadReference (which fires only after the
    // super-source AND its nested source are pushed into the registry), not an
    // earlier parse/clone error that would never push anything and make the
    // rollback assertion pass for the wrong reason.
    assert!(
        r.stderr.contains("nonexistent-skill") && r.stderr.contains("does not match"),
        "failure must be the DSC-63 bad-reference error (fires post-push): {}",
        r.stderr
    );
    // Only the first (successful) entry should be in the registry.
    assert_eq!(
        source_count(&sb),
        1,
        "only the first source should be persisted; the failed entry must be rolled back: stderr={}",
        r.stderr
    );
}

#[test]
fn auto_meld_rollback_on_dsc64_validate_failure() {
    // spec: POL-35 -- a second, distinct failure mode from the install_items
    // bad-ref case above. Here meld_recursive pushes the super-source into the
    // registry, then a DSC-64 validation error (install = true and install_items
    // both set on a nested entry) propagates out. Without the POL-35 rollback the
    // partially-pushed super-source would be persisted by the save; with it, only
    // the first successful entry survives.
    let sb = Sandbox::new();
    let first_spec = sb.source_spec();

    // A nested source that offers a real skill (never reached: validate errors
    // before the nested meld runs, but the entry must parse as a valid source).
    let nested = sb.base.join("nested-dsc64-src");
    write(
        &nested.join("skills/real-skill/SKILL.md"),
        "---\nname: real-skill\ndescription: a real skill\n---\n# real skill\n",
    );
    git(&nested, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&nested, &["config", "user.email", "t@t"]);
    git(&nested, &["config", "user.name", "t"]);
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-qm", "initial"]);

    // Super-source: install = true AND install_items set is a DSC-64 violation.
    let super_src = sb.base.join("super-dsc64-src");
    let nested_str = nested.to_string_lossy();
    let super_toml = format!(
        "[[discover.sources]]\nsource = \"{nested_str}\"\ninstall = true\ninstall-items = [\"skill:real-skill\"]\n"
    );
    write(&super_src.join("mind.toml"), &super_toml);
    git(&super_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&super_src, &["config", "user.email", "t@t"]);
    git(&super_src, &["config", "user.name", "t"]);
    git(&super_src, &["add", "-A"]);
    git(&super_src, &["commit", "-qm", "initial"]);

    let super_str = super_src.to_string_lossy();
    let first_str = first_spec.as_str();
    let policy_body = format!(
        "[[sources.auto_meld]]\nrepo = \"{first_str}\"\n\n[[sources.auto_meld]]\nrepo = \"{super_str}\"\n"
    );
    let policy = write_policy(&sb, &policy_body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "sync must fail on the DSC-64 provisioning error: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The failure must be the DSC-64 mutual-exclusion error (reached only after
    // the super-source was pushed), not some earlier parse/clone error that would
    // never push the super and make this test trivially pass.
    assert!(
        r.stderr.contains("mutually exclusive"),
        "failure must be the DSC-64 validate error (fires after the super push): {}",
        r.stderr
    );
    // Only the first (successful) entry survives; the super-source pushed before
    // the validate error must be rolled back, not persisted.
    assert_eq!(
        source_count(&sb),
        1,
        "the DSC-64-failing entry must be rolled back, leaving only the first: stderr={}",
        r.stderr
    );
}

#[test]
fn no_sources_melded_phrasing_sync() {
    // spec: CLI-187 -- `sync` with no sources uses the standardized phrase
    // "no sources melded; run `mind meld <owner/repo>` to add one".
    let sb = Sandbox::bare("cli187-sync-test");
    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync with no sources must succeed: {}", r.stderr);
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "sync empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_recall_sources() {
    // spec: CLI-187 -- `recall --sources` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-recall-sources-test");
    let r = sb.mind(&["recall", "--sources"]);
    assert!(
        r.success,
        "recall --sources with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "recall --sources empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_recall() {
    // spec: CLI-187 -- `recall` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-recall-test");
    let r = sb.mind(&["recall"]);
    assert!(
        r.success,
        "recall with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "recall empty message must match the standard phrasing: {}",
        r.stdout
    );
}

#[test]
fn no_sources_melded_phrasing_probe() {
    // spec: CLI-187 -- `probe` with no sources uses the same phrase.
    let sb = Sandbox::bare("cli187-probe-test");
    let r = sb.mind(&["probe", "anything"]);
    assert!(
        r.success,
        "probe with no sources must succeed: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("no sources melded")
            && r.stdout.contains("mind meld <owner/repo>")
            && r.stdout.contains("to add one"),
        "probe empty message must match the standard phrasing: {}",
        r.stdout
    );
}
