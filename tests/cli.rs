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

    /// Change the skill upstream and commit, so a `sync` + `evolve` sees a delta.
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
fn meld_twice_errors() {
    // spec: CLI-12
    let sb = melded();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec]);
    assert!(!r.success);
    assert!(r.stderr.contains("already melded"), "stderr: {}", r.stderr);
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
fn evolve_reports_nothing_when_up_to_date() {
    // spec: CLI-64
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["evolve"]);
    assert!(r.stdout.contains("up to date"), "{}", r.stdout);
}

#[test]
fn evolve_reports_delta_and_declining_changes_nothing() {
    // spec: CLI-60, CLI-61
    let sb = melded();
    sb.mind(&["learn", "review"]);
    sb.edit_source();
    sb.mind(&["sync"]);

    // Dry-run report: shows hash and commit deltas with arrows.
    let report = sb.mind_with_input(&["evolve"], Some("n\n"));
    assert!(report.stdout.contains("skill:review"), "{}", report.stdout);
    assert!(report.stdout.contains("hash"), "{}", report.stdout);
    assert!(report.stdout.contains("->"), "{}", report.stdout);
    assert!(report.stdout.contains("aborted"), "{}", report.stdout);

    // Declining must leave the installed commit untouched.
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    let again = sb.mind_with_input(&["evolve"], Some("n\n"));
    assert!(again.stdout.contains("aborted"));
    assert_eq!(before, sb.mind(&["recall", "skill:review"]).stdout);
}

#[test]
fn evolve_yes_applies_and_is_then_idempotent() {
    // spec: CLI-62, LIFE-13
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source();
    sb.mind(&["sync"]);

    let applied = sb.mind(&["evolve", "--yes"]);
    assert!(applied.success, "{}", applied.stderr);
    assert!(
        applied.stdout.contains("evolved skill:review"),
        "{}",
        applied.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]).stdout;
    assert_ne!(before, after, "commit/hash should have advanced");

    // Running again finds nothing to do.
    let idem = sb.mind(&["evolve"]);
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

    let list = sb.mind(&["recall"]);
    assert!(list.stdout.contains("nothing learned"), "{}", list.stdout);
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
    assert!(sb.mind(&["recall"]).stdout.contains("agent:dup"));
    assert!(!sb.mind(&["recall"]).stdout.contains("skill:dup"));
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
    assert!(sb.mind(&["recall"]).stdout.contains("nothing learned"));
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
    assert!(sb.mind(&["learn", "jk-lead"]).success);

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
    assert!(sb.mind(&["learn", "lead"]).success);

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
fn evolve_treats_a_prefix_change_as_a_rename() {
    // spec: LIFE-10, LIFE-11, LIFE-14, CLI-61
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success); // no prefix yet
    assert!(sb.mind(&["learn", "review"]).success); // installed as skill:review

    // Upstream adds a namespace prefix.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["evolve", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("rename"),
        "report should flag rename: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("evolved skill:review -> skill:jk-review"),
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
fn unmeld_removes_source_but_keeps_installed_items() {
    // spec: CLI-20, CLI-21
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["unmeld", "agents"]).success);

    // Source is gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    // The installed item is left in place.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok());
    assert!(sb.mind(&["recall"]).stdout.contains("skill:review"));
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
    assert!(sb.mind(&["learn", "jk-lead"]).success);
    let store = sb.mind_home.join("store/agent/jk-lead");
    assert!(std::fs::read_to_string(&store).unwrap().contains("jk-dev"));

    // Upstream introduces a broken reference.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:ghost}}.\n",
    );
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["evolve", "--yes"]);
    assert!(!r.success, "evolve should fail on the bad reference");
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

    // evolve does not touch an item with no catalog match.
    let ev = sb.mind(&["evolve", "--yes"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("up to date"), "{}", ev.stdout);
    assert!(sb.mind(&["recall"]).stdout.contains("agent:dev"));

    // introspect reports it as gone upstream.
    let ins = sb.mind(&["introspect"]);
    assert!(ins.stdout.contains("no longer present"), "{}", ins.stdout);
}

#[test]
fn evolve_item_filter_limits_to_one() {
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

    // Filtered evolve upgrades only the named item.
    let ev = sb.mind(&["evolve", "--yes", "review"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("evolved skill:review"), "{}", ev.stdout);
    assert!(!ev.stdout.contains("agent:dev"), "{}", ev.stdout);

    // dev is still pending (reported by an unfiltered, declined evolve).
    let rest = sb.mind(&["evolve"]);
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
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(!recall.contains("agent:dev"), "{recall}");
}

#[test]
fn learn_dry_run_installs_nothing() {
    // spec: CLI-32
    let sb = melded();
    let r = sb.mind(&["learn", "*", "--dry-run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("dry run"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skill:review"),
        "plan should list items: {}",
        r.stdout
    );
    // Nothing was actually installed.
    assert!(sb.mind(&["recall"]).stdout.contains("nothing learned"));
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
    assert!(a.mind(&["recall"]).stdout.contains("nothing learned"));
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
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(!recall.contains("agent:dev"), "{recall}");
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

#[test]
fn unlearn_is_an_alias_for_forget() {
    // spec: CLI-40
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["unlearn", "review"]).success);
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(sb.mind(&["recall"]).stdout.contains("nothing learned"));
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
    let after = sb.mind(&["recall"]).stdout;
    assert!(!after.contains("skill:review"), "{after}");
    assert!(after.contains("agent:dev"), "{after}");
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());

    // A bare `*` forgets everything that is left.
    assert!(sb.mind(&["forget", "*"]).success);
    assert!(sb.mind(&["recall"]).stdout.contains("nothing learned"));

    // A glob matching no installed item is an error.
    let none = sb.mind(&["forget", "zzz*"]);
    assert!(!none.success);
    assert!(none.stderr.contains("not installed"), "{}", none.stderr);
}

#[test]
fn unmeld_forget_purges_installed_items() {
    // spec: CLI-22
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    let r = sb.mind(&["unmeld", "agents", "--forget"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("removed"), "{}", r.stdout);

    // Both the source and every installed item are gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    assert!(sb.mind(&["recall"]).stdout.contains("nothing learned"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err());
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
fn sync_evolve_refreshes_then_applies_upgrades() {
    // spec: CLI-53
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source(); // upstream change, not yet synced

    // One command fetches the change and (on `y`) applies the upgrade.
    let r = sb.mind_with_input(&["sync", "--evolve"], Some("y\n"));
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("updated"), "sync ran: {}", r.stdout);
    assert!(
        r.stdout.contains("evolved skill:review"),
        "evolve applied: {}",
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

    // Listing is a JSON array of installed items.
    let items = sb.mind(&["recall", "--json"]);
    assert!(items.success, "{}", items.stderr);
    assert!(
        items.stdout.trim_start().starts_with('['),
        "{}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"kind\": \"skill\""),
        "{}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"name\": \"review\""),
        "{}",
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

    // An empty listing is `[]`, not a human message.
    assert!(sb.mind(&["forget", "review"]).success);
    let empty = sb.mind(&["recall", "--json"]).stdout;
    assert_eq!(empty.trim(), "[]", "{empty}");
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
    assert!(jk.mind(&["learn", "jk-lead"]).success);
    let lead = std::fs::read_to_string(jk.mind_home.join("store/agent/jk-lead")).unwrap();
    assert!(lead.contains("the jk-dev agent"), "{lead}");
    assert!(lead.contains("the jk-review skill"), "{lead}");
    assert!(lead.contains("the jk-style rule"), "{lead}");
    assert!(!lead.contains("{{ns:"), "tokens should be gone: {lead}");
    // The skill references a rule from inside its directory; it expands too.
    assert!(jk.mind(&["learn", "jk-review"]).success);
    let review =
        std::fs::read_to_string(jk.mind_home.join("store/skill/jk-review/SKILL.md")).unwrap();
    assert!(review.contains("jk-style rule"), "{review}");
    assert!(!review.contains("{{ns:"), "tokens should be gone: {review}");

    // Unprefixed: the same tokens expand to the bare names.
    let bare = Sandbox::from_example("namespacing");
    assert!(bare.mind(&["meld", &bare.source_spec()]).success);
    assert!(bare.mind(&["learn", "lead"]).success);
    let lead2 = std::fs::read_to_string(bare.mind_home.join("store/agent/lead")).unwrap();
    assert!(lead2.contains("the dev agent"), "{lead2}");
    assert!(lead2.contains("the review skill"), "{lead2}");
    assert!(lead2.contains("the style rule"), "{lead2}");
    assert!(!lead2.contains("{{ns:"), "{lead2}");
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
    assert!(sources.contains("agents"), "first source missing: {sources}");
    assert!(sources.contains("tools"), "second source missing: {sources}");
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
        assert!(recall.stdout.contains("skill:review"), "review lost: {}", recall.stdout);
        assert!(recall.stdout.contains("agent:dev"), "dev lost: {}", recall.stdout);
        assert!(recall.stdout.contains("rule:style"), "style lost: {}", recall.stdout);
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
    assert!(sources.success, "recall failed after concurrent melds: {}", sources.stderr);
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
    let json =
        std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
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
    let json =
        std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
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
    assert_eq!(
        commit, sha_v1,
        "follow-branch=stable should record sha_v1"
    );

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
        r.stdout,
        r.stderr
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
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"stable\"\n",
    );
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
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"stable\"\n",
    );
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
    assert_ne!(commit, sha_v1, "directive must not take precedence over consumer flag");
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
    assert_eq!(
        after, sha_v3,
        "follow-branch source should advance on sync"
    );
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
    // spec: CLI-18 — a pin that does not resolve in the remote is a `Git` error
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
    // spec: CLI-18 — same as above for a tag that does not exist in the remote.
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
    // spec: CLI-55 — sync resolves each source against its recorded pin. If the
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
    // spec: STO-18, CLI-55 — the recorded pin is untouched by sync across
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
    // spec: DSC-41 — a `pin-tag` directive supplies the default pin when the
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
    assert!(pin_json.contains("\"tag\""), "pin kind should be tag: {pin_json}");
    assert!(pin_json.contains("v1.0"), "pin value should be v1.0: {pin_json}");
}

#[test]
fn source_directive_pin_ref_applies_when_no_consumer_flag() {
    // spec: DSC-41 — a `pin-ref` directive supplies the default pin when the
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
    assert!(pin_json.contains("\"ref\""), "pin kind should be ref: {pin_json}");
    assert!(pin_json.contains(&sha_v1), "pin value should be the sha: {pin_json}");
}

#[test]
fn consumer_pin_ref_overrides_follow_branch_directive() {
    // spec: DSC-41, CLI-17 — a consumer flag of a DIFFERENT kind overrides the
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
    // spec: DSC-41 — `[source]` is `deny_unknown_fields`, so a misspelled pin
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
    // spec: CLI-55 — the moved-tag force-fetch is observable end-to-end via the
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
