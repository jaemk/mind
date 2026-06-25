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
}

#[test]
fn meld_uses_declared_prefix_when_installing() {
    // spec: CLI-24 - a non-interactive meld accepts a source's declared
    // `[source].prefix`; installed items are namespaced `<prefix>-<name>`.
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "meld of a prefixed source should succeed"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("jk-review"),
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
        !recall.contains("jk-"),
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
    assert!(
        r.stdout.contains("advisory [unguarded-reference]"),
        "the bare `dev` mention must be flagged as an unguarded-reference advisory, \
         in the same format as `review`: {}",
        r.stdout
    );
    // INIT-3: a mind.toml is scaffolded when absent, with a `[source]` table and
    // a commented-out generic prefix example whose value matches its comment.
    let scaffold = std::fs::read_to_string(repo.join("mind.toml")).unwrap();
    assert!(
        scaffold.contains("[source]") && scaffold.contains("# prefix = \"prefix\""),
        "scaffold must carry a [source] table and a generic commented prefix: {scaffold}"
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
fn remeld_with_as_reprefixes_installed_items() {
    // spec: CLI-13 - a re-meld with --as changes the prefix and renames the
    // installed items (and their links) to the new effective names.
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

    let r = sb.mind(&["meld", &spec, "--as", "jk", "--yes"]);
    assert!(r.success, "re-meld --as failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("renamed skill:review -> skill:jk-review"),
        "re-meld --as must rename installed items: {}",
        r.stdout
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/jk-review")).is_ok(),
        "the prefixed link must exist"
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err(),
        "the old unprefixed link must be gone"
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("jk-review"),
        "recall must show the re-prefixed name: {recall}"
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
    assert!(probe.stdout.contains("skill:tl-review"), "{}", probe.stdout);
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
    assert!(probe.stdout.contains("skill:jk-review"), "{}", probe.stdout);
    assert!(probe.stdout.contains("agent:jk-dev"), "{}", probe.stdout);
    // The bare names must not appear.
    assert!(!probe.stdout.contains("skill:review"), "{}", probe.stdout);

    // Install under the prefixed name; symlink lands at the prefixed location.
    assert!(sb.mind(&["learn", "jk-review"]).success);
    let link = sb.claude_home.join("skills/jk-review");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.stdout.contains("as:jk"), "{}", sources.stdout);
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
    assert!(probe.stdout.contains("skill:ag-review"), "{}", probe.stdout);

    // Consumer --as overrides the author's prefix.
    let sb2 = Sandbox::new();
    sb2.write_and_commit("mind.toml", "[source]\nprefix = \"ag\"\n");
    let spec2 = sb2.source_spec();
    assert!(sb2.mind(&["meld", &spec2, "--as", "zz"]).success);
    let probe2 = sb2.mind(&["probe"]);
    assert!(
        probe2.stdout.contains("skill:zz-review"),
        "{}",
        probe2.stdout
    );
    assert!(!probe2.stdout.contains("ag-review"), "{}", probe2.stdout);
}

#[test]
fn ns_token_expands_to_prefixed_reference_on_install() {
    // spec: NS-11
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the {{ns:dev}} agent.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    // `lead` references {{ns:dev}}; the closure prompt is confirmed with --yes.
    assert!(sb.mind(&["learn", "jk-lead", "--yes"]).success);

    let store = sb.mind_home.join("store/agent/jk-lead");
    let body = std::fs::read_to_string(&store).expect("installed agent file");
    assert!(
        body.contains("the jk-dev agent"),
        "expected expanded ref: {body}"
    );
    assert!(!body.contains("{{ns:dev}}"), "token should be gone: {body}");
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
    // spec: NS-20, NS-22, CLI-14
    let sb = Sandbox::new();
    // Bare prose reference to sibling `dev`, no token.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("references sibling(s) in prose") && r.stderr.contains("dev"),
        "expected unguarded-ref warning: {}",
        r.stderr
    );
}

#[test]
fn no_warning_when_unprefixed() {
    // spec: NS-23
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]); // no prefix -> bare refs are correct
    assert!(r.success);
    assert!(
        !r.stderr.contains("references sibling(s) in prose"),
        "{}",
        r.stderr
    );
}

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
            .contains("upgraded skill:review -> skill:jk-review"),
        "{}",
        r.stdout
    );

    // Manifest now holds only the renamed item.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("skill:jk-review"),
        "{}",
        recall.stdout
    );
    assert!(!recall.stdout.contains("skill:review"), "{}", recall.stdout);

    // Symlinks moved; the old one is gone, the new one exists.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/jk-review")).is_ok());
    // Old store copy removed, new one present.
    assert!(!sb.mind_home.join("store/skill/review").exists());
    assert!(sb.mind_home.join("store/skill/jk-review").exists());
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
    assert!(sb.mind(&["learn", "jk-lead", "--yes"]).success);
    let store = sb.mind_home.join("store/agent/jk-lead");
    assert!(std::fs::read_to_string(&store).unwrap().contains("jk-dev"));

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
    assert!(
        body.contains("jk-dev"),
        "old version should be intact: {body}"
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/jk-lead.md")).is_ok());
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

    assert!(sb.mind(&["recall", "--sources"]).stdout.contains("as:jk"));
    // Items remain namespaced under the alias after sync.
    assert!(sb.mind(&["probe"]).stdout.contains("skill:jk-review"));
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
fn detach_is_an_alias_for_unmeld() {
    // spec: CLI-20
    let sb = melded();
    assert!(sb.mind(&["detach", "agents"]).success);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
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
fn config_target_is_an_alias_for_lobes() {
    // spec: CLI-111
    let sb = Sandbox::new();
    let home = sb.base.join("viaTarget").display().to_string();
    assert!(sb.mind(&["config", "target", "add", &home]).success);
    assert!(
        sb.mind(&["config", "target", "list"])
            .stdout
            .contains(&home)
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
    // spec: CLI-73
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // The default view is a JSON array of sources, each with nested items that
    // carry their install state (CLI-73).
    let items = sb.mind(&["recall", "--json"]);
    assert!(items.success, "{}", items.stderr);
    assert!(
        items.stdout.trim_start().starts_with('['),
        "{}",
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

    // A single-item lookup is a JSON object.
    let one = sb.mind(&["recall", "skill:review", "--json"]).stdout;
    assert!(one.trim_start().starts_with('{'), "{one}");
    assert!(one.contains("\"hash\""), "{one}");

    // --sources is a JSON array of sources.
    let srcs = sb.mind(&["recall", "--sources", "--json"]).stdout;
    assert!(srcs.trim_start().starts_with('['), "{srcs}");
    assert!(srcs.contains("\"url\""), "{srcs}");

    // An empty registry is `[]`, not a human message.
    let fresh = Sandbox::new();
    assert_eq!(
        fresh.mind(&["recall", "--json"]).stdout.trim(),
        "[]",
        "an empty registry must emit []"
    );
}

#[test]
fn probe_json_emits_rows() {
    // spec: CLI-84
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.trim_start().starts_with('['), "{}", r.stdout);
    assert!(r.stdout.contains("\"installed\""), "{}", r.stdout);
    assert!(r.stdout.contains("\"name\": \"review\""), "{}", r.stdout);
    // The installed item carries installed:true.
    assert!(r.stdout.contains("true"), "{}", r.stdout);
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
    // spec: TUI-2 - `--json` forces JSON output; must not enter raw mode.
    let sb = melded();
    let r = sb.mind(&["probe", "--json"]);
    assert!(r.success, "probe --json should succeed: {}", r.stderr);
    assert!(
        r.stdout.trim_start().starts_with('['),
        "probe --json must produce a JSON array: {}",
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
    // spec: CLI-92
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean: an object with an (empty) issues array and counts.
    let clean = sb.mind(&["introspect", "--json"]).stdout;
    assert!(clean.trim_start().starts_with('{'), "{clean}");
    assert!(clean.contains("\"issues\""), "{clean}");
    assert!(clean.contains("\"items\""), "{clean}");

    // A broken link surfaces as a missing-link issue with its stable kind tag.
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();
    let broken = sb.mind(&["introspect", "--json"]).stdout;
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
    // spec: NS-20
    let sb = Sandbox::new();
    // A skill whose bare prose reference to sibling `dev` lives in a secondary
    // file, not SKILL.md. The warning must still catch it (scan is item-wide).
    sb.write_and_commit(
        "skills/lead/SKILL.md",
        "---\nname: lead\ndescription: lead skill\n---\n# lead\n",
    );
    sb.write_and_commit("skills/lead/NOTES.md", "Delegate to the dev agent.\n");

    let r = sb.mind(&["meld", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stderr.contains("skill:jk-lead") && r.stderr.contains("dev"),
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
    let meld = jk.mind(&["meld", &jk.source_spec(), "--as", "jk"]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(
        !meld.stderr.contains("references sibling(s) in prose"),
        "all refs are tokens, so no warning: {}",
        meld.stderr
    );
    // `lead` references siblings via {{ns:}}, so a partial learn pulls in the
    // closure and prompts (DEP-31); `--yes` confirms.
    assert!(jk.mind(&["learn", "jk-lead", "--yes"]).success);
    let lead = std::fs::read_to_string(jk.mind_home.join("store/agent/jk-lead")).unwrap();
    assert!(lead.contains("the jk-dev agent"), "{lead}");
    assert!(lead.contains("the jk-review skill"), "{lead}");
    assert!(lead.contains("the jk-style rule"), "{lead}");
    assert!(!lead.contains("{{ns:"), "tokens should be gone: {lead}");
    // The skill references a rule from inside its directory; it expands too.
    assert!(jk.mind(&["learn", "jk-review", "--yes"]).success);
    let review =
        std::fs::read_to_string(jk.mind_home.join("store/skill/jk-review/SKILL.md")).unwrap();
    assert!(review.contains("jk-style rule"), "{review}");
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

    // With --as jk: the token {{ns:dev}} should resolve to jk-dev (a sibling).
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
    // Idempotency at the entry level: a source already present in the registry
    // (here melded by the user before any policy applied) is left unchanged when
    // an auto_meld entry names the same identity. No duplicate, no error.
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

    // Now a policy whose auto_meld names the same source.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);
    assert_eq!(
        source_count(&sb),
        1,
        "an already-melded auto_meld entry must not be re-melded: {}",
        r.stdout
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
fn help_lists_upgrade_and_evolve_not_self_update() {
    // Confirm clap renders both subcommands with the right names.
    // No spec cite needed; this is a structural smoke test.
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
    assert!(
        !r.stdout.contains("self-update"),
        "help must NOT contain 'self-update': {}",
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
    // spec: HOOK-53, HOOK-54
    // An uninstall hook that exits non-zero is a hard stop: the unmeld fails
    // and the source remains registered. Items must also remain installed.
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

    // Items must still be installed.
    assert!(
        sb.mind(&["recall", "agent:dev"]).success,
        "items must remain installed after a failed uninstall hook"
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
    assert!(store.join("tool/jk-detect/detect").is_file());
    // The same tokens resolve to the prefixed store paths.
    let skill_md = std::fs::read_to_string(store.join("skill/jk-review/SKILL.md")).unwrap();
    assert!(
        skill_md.contains(&format!("{}/tool/jk-detect/detect", store.display())),
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
    // name with no ambiguity and no qualifier: `review` from a, `zz-review` from b.
    let la = a.mind(&["learn", "review"]);
    assert!(la.success, "learn review: {} {}", la.stdout, la.stderr);
    let lb = a.mind(&["learn", "zz-review"]);
    assert!(lb.success, "learn zz-review: {} {}", lb.stdout, lb.stderr);

    // Both coexist: the unprefixed one as `review`, the namespaced one as `zz-review`.
    let recall = a.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("skill:zz-review"), "{recall}");
    assert!(
        a.mind_home.join("store/skill/review").is_dir(),
        "a's store copy"
    );
    assert!(
        a.mind_home.join("store/skill/zz-review").is_dir(),
        "b's store copy"
    );
    for link in ["skills/review", "skills/zz-review"] {
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
fn json_error_goes_to_stderr_and_stdout_stays_clean() {
    // spec: CLI-153
    // An error under --json must not corrupt stdout: the failure is reported on
    // stderr and stdout carries no half-written JSON object.
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist", "--json"]);
    assert!(!r.success, "unknown item must fail");
    assert!(
        r.stderr.contains("no item matches"),
        "error must go to stderr: {}",
        r.stderr
    );
    assert!(
        r.stdout.trim().is_empty(),
        "no JSON (or prose) must be written to stdout on error: {:?}",
        r.stdout
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
    let install = format!("touch built-here && mkdir -p {m} && touch {m}/installed");
    let uninstall = format!("mkdir -p {m} && touch {m}/uninstalled");
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

/// A commit advance (the source moves to a new commit past the installed one)
/// must mark the item out of date in `recall`, even without a hash check
/// mismatch path that differs from content drift.
// spec: CLI-75
#[test]
fn recall_marks_item_outdated_after_commit_advance() {
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean install: not outdated.
    let r = sb.mind(&["recall"]);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // Advance the source by committing an edit, then sync so the recorded source
    // commit moves past the installed item's commit.
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outdated"),
        "recall must mark the item outdated after a commit advance: {}",
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
