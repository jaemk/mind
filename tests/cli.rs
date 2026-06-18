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
