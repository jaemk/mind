//! The `workflow` item kind (spec/workflows.md, WF-1..52): end-to-end tests that
//! drive the real `mind` binary against a hermetic, network-free fixture (a
//! local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   WF-1/WF-2/WF-3: `workflows/<name>.js` is discovered, flatly, `.js` only
//!   WF-4: the description comes from the `meta` object, not frontmatter
//!   WF-6: `--kind workflow` and the `workflow:<name>` ref select it
//!   WF-10: it stores at `store/workflow/<name>` and links at `workflows/<name>.js`
//!   WF-22: a namespace prefix gives `workflows/<prefix>:<name>.js`
//!   WF-23/WF-25: `{{ns:}}` in `meta.name` expands at install
//!   WF-50: the kind-generic machinery (upgrade, forget, unmanaged) covers it
//!   WF-51: `probe` shows `whenToUse` beside the description

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A workflow whose `meta` carries all three keys the reader extracts.
const REVIEW_JS: &str = r#"export const meta = {
  name: 'review-changes',
  description: 'Review changed files',
  whenToUse: 'before opening a PR',
}
phase('Review')
"#;

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
    /// A source repo with one workflow (`review-changes`) and one skill
    /// (`review`), so kind filters have something to exclude.
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-wf-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("agents");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(&source.join("workflows/review-changes.js"), REVIEW_JS);
        write(
            &source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff\n---\n# review\n",
        );
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        std::fs::create_dir_all(&sb.mind_home).unwrap();
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, None)
    }

    /// `mind` with `$HOME` pointed at the sandbox, so the store root really is
    /// under home and TOOL-16's `~` rendering applies. The default runner leaves
    /// `$HOME` alone (the store is then a temp path outside it, and every path
    /// token renders absolute whatever the kind).
    fn mind_under_home(&self, args: &[&str]) -> Run {
        self.mind_env(args, Some(&self.base))
    }

    fn mind_env(&self, args: &[&str], home: Option<&Path>) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if let Some(home) = home {
            cmd.env("HOME", home);
        }
        let out = cmd.output().expect("run mind");
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

    /// The lobe path a workflow links to.
    fn link(&self, name: &str) -> PathBuf {
        self.claude_home.join("workflows").join(name)
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

fn git(repo: &Path, args: &[&str]) {
    std::fs::create_dir_all(repo).unwrap();
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A workflow is discovered, offered by `probe` with the description from its
/// `meta` object, installed to the store, and linked into the lobe at
/// `workflows/<name>.js`.
#[test]
fn workflow_installs_to_the_store_and_links_into_the_lobe() {
    // spec: WF-1 WF-4 WF-6 WF-10
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["probe", "--no-tui", "--kind", "workflow"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:review-changes"),
        "--kind workflow must select the workflow: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Review changed files"),
        "the description must come from the `meta` object: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skill:review"),
        "--kind workflow must exclude other kinds: {}",
        r.stdout
    );

    let r = sb.mind(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let link = sb.link("review-changes.js");
    assert!(
        link.symlink_metadata().is_ok(),
        "the workflow must be linked at workflows/review-changes.js"
    );
    let target = std::fs::read_link(&link).expect("the lobe entry is a symlink");
    assert!(
        target.ends_with("store/workflow/review-changes"),
        "the link must point at store/workflow/review-changes, got {target:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&link).unwrap(),
        REVIEW_JS,
        "the installed workflow is the source file"
    );

    let r = sb.mind(&["recall", "--kind", "workflow"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:review-changes"),
        "recall must report the installed workflow: {}",
        r.stdout
    );
}

/// `probe` appends `whenToUse` to the description, the way the harness's own
/// workflow list renders the pair. An item with no `whenToUse` is unaffected.
#[test]
fn probe_shows_when_to_use_beside_the_description() {
    // spec: WF-51
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "--no-tui", "--json"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout
            .contains("Review changed files - before opening a PR"),
        "the workflow row must read `<description> - <whenToUse>`: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("\"Review the diff\""),
        "a skill's description must be untouched: {}",
        r.stdout
    );

    // The composed form is what a query matches against, so a `whenToUse`
    // phrase finds the workflow.
    let r = sb.mind(&["probe", "--no-tui", "opening a PR"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:review-changes"),
        "a whenToUse phrase must match the workflow: {}",
        r.stdout
    );
}

/// The scan is flat and the extension is exactly `.js`: a nested workflow and a
/// `.mjs`/`.ts`/`.md` sibling are all invisible, matching what the harness's own
/// loader would accept.
#[test]
fn the_scan_is_flat_and_js_only() {
    // spec: WF-2 WF-3
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/nested/deep.js", REVIEW_JS);
    sb.write_and_commit("workflows/modern.mjs", REVIEW_JS);
    sb.write_and_commit("workflows/typed.ts", REVIEW_JS);
    sb.write_and_commit("workflows/notes.md", "---\ndescription: notes\n---\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:review-changes"),
        "the flat `.js` workflow is still found: {}",
        r.stdout
    );
    for missed in ["deep", "modern", "typed", "notes"] {
        assert!(
            !r.stdout.contains(missed),
            "'{missed}' must not be discovered as an item: {}",
            r.stdout
        );
    }
}

/// A namespace prefix renames a workflow to `<prefix>:<name>`, and a `{{ns:}}`
/// token in `meta.name` expands to that same effective name, so the harness
/// answers to the name mind installed.
#[test]
fn a_prefixed_workflow_links_and_expands_under_its_namespaced_name() {
    // spec: WF-22 WF-23 WF-25
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: '{{ns:review-changes}}',\n  description: 'Review changed files',\n}\n",
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--namespace", "jk"]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "workflow:jk:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let link = sb.link("jk:review-changes.js");
    assert!(
        link.symlink_metadata().is_ok(),
        "a prefixed workflow links at workflows/jk:review-changes.js"
    );
    assert!(
        sb.link("review-changes.js").symlink_metadata().is_err(),
        "the bare name must not also be linked"
    );
    assert!(
        sb.mind_home
            .join("store/workflow/jk:review-changes")
            .exists(),
        "the store copy uses the effective name"
    );

    // spec: WF-23 WF-25 -- the token expanded in the `.js` file itself, so the
    // stored workflow carries a plain string literal that matches the link name.
    let installed = std::fs::read_to_string(&link).unwrap();
    assert!(
        installed.contains("name: 'jk:review-changes'"),
        "the `{{{{ns:}}}}` token in meta.name must expand to the effective name: {installed}"
    );
    assert!(
        !installed.contains("{{ns:"),
        "no token may survive into the installed file: {installed}"
    );
}

/// An unprefixed source's `{{ns:}}` token expands to the bare name, so the same
/// file works either way (WF-23).
#[test]
fn an_unprefixed_workflow_expands_its_token_to_the_bare_name() {
    // spec: WF-23 WF-25
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: '{{ns:review-changes}}',\n  description: 'Review changed files',\n}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let installed = std::fs::read_to_string(sb.link("review-changes.js")).unwrap();
    assert!(
        installed.contains("name: 'review-changes'"),
        "an unprefixed token expands to the bare name: {installed}"
    );
}

/// `upgrade` picks up an edited workflow, and `forget` removes both the link and
/// the store copy: the kind rides the generic lifecycle machinery.
#[test]
fn workflow_upgrades_and_forgets_like_any_item() {
    // spec: WF-50
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "workflow:review-changes"]).success);

    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'review-changes',\n  description: 'Review changed files v2',\n}\n",
    );
    assert!(sb.mind(&["sync"]).success, "sync must succeed");
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("upgraded workflow:review-changes"),
        "the workflow must upgrade: {}",
        r.stdout
    );
    assert!(
        std::fs::read_to_string(sb.link("review-changes.js"))
            .unwrap()
            .contains("Review changed files v2"),
        "the linked file must show the new content"
    );

    let r = sb.mind(&["forget", "workflow:review-changes", "--yes"]);
    assert!(r.success, "forget: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("review-changes.js").symlink_metadata().is_err(),
        "forget removes the lobe link"
    );
    assert!(
        !sb.mind_home.join("store/workflow/review-changes").exists(),
        "forget removes the store copy"
    );
}

/// A hand-written workflow in a lobe is reported as an unmanaged item; a nested
/// one is not seen at all, matching the flat convention scan (WF-2).
#[test]
fn a_hand_written_workflow_is_reported_as_unmanaged() {
    // spec: WF-50
    let sb = Sandbox::new();
    write(&sb.claude_home.join("workflows/mine.js"), REVIEW_JS);
    write(&sb.claude_home.join("workflows/nested/deep.js"), REVIEW_JS);
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("unmanaged") && r.stdout.contains("workflow:mine"),
        "a hand-written workflow must be listed as unmanaged: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("deep"),
        "a nested workflow must not be surfaced as unmanaged: {}",
        r.stdout
    );
}

/// A `mind.toml` may declare a workflow at a non-convention path and discover
/// others by glob; the glob matches the workflow FILE.
#[test]
fn a_mindfile_declares_and_globs_workflows() {
    // spec: WF-6 WF-8
    let sb = Sandbox::new();
    sb.write_and_commit("orchestration/audit.js", REVIEW_JS);
    sb.write_and_commit("packages/one/workflows/ship.js", REVIEW_JS);
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"workflow\"\nname = \"audit\"\npath = \"orchestration/audit.js\"\n\n\
         [discover]\nworkflows = { include = [\"packages/*/workflows/*.js\"] }\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "--no-tui", "--kind", "workflow"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:audit"),
        "the declared workflow must be offered: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("workflow:ship"),
        "the globbed workflow must be offered: {}",
        r.stdout
    );
    // The mind.toml is authoritative, so the convention `workflows/` directory
    // is no longer scanned (DSC-3).
    assert!(
        !r.stdout.contains("review-changes"),
        "an authoritative mind.toml turns off the convention scan: {}",
        r.stdout
    );

    let r = sb.mind(&["learn", "workflow:audit"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("audit.js").symlink_metadata().is_ok(),
        "a declared workflow still links at workflows/<name>.js"
    );
}

/// A lobe whose `kinds` filter excludes workflows gets no workflow link. The
/// harness presets' own skills-only lists are the WF-12 case; a hand-written
/// filter here exercises the generic mechanism they ride on.
#[test]
fn a_skills_only_lobe_admits_no_workflows() {
    // spec: WF-12
    let sb = Sandbox::new();
    let other = sb.base.join("gemini");
    std::fs::create_dir_all(&other).unwrap();
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "[[lobes]]\npath = \"{}\"\n\n[[lobes]]\npath = \"{}\"\nkinds = [\"skill\"]\n",
            sb.claude_home.display(),
            other.display(),
        ),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    assert!(
        sb.link("review-changes.js").symlink_metadata().is_ok(),
        "the all-kinds lobe gets the workflow"
    );
    assert!(
        !other.join("workflows/review-changes.js").exists(),
        "a skills-only lobe must not receive a workflow link"
    );
}

/// A path token in a workflow renders in the `~` home form, not the absolute
/// form an NS-57 `expand:`-listed script gets: a workflow never executes a path,
/// its strings are prompts read by an agent.
#[test]
fn a_path_token_in_a_workflow_renders_in_the_home_form() {
    // spec: WF-25 WF-26
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'review-changes',\n  \
         description: 'Review changed files',\n}\n\
         agent(`read {{path:skill:review}}/SKILL.md`)\n",
    );
    assert!(sb.mind_under_home(&["meld", &sb.source_spec()]).success);
    let r = sb.mind_under_home(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let installed = std::fs::read_to_string(sb.link("review-changes.js")).unwrap();
    assert!(
        installed.contains("~/mind/store/skill/review/SKILL.md"),
        "the path token must render in the `~` home form, not an absolute path \
         (the NS-57 expand-listed form): {installed}"
    );
    assert!(
        !installed.contains("{{path:"),
        "no token may survive into the installed file: {installed}"
    );
}

/// `review` does not report a workflow's tokens as inert: they expand (WF-25),
/// so the finding would be false. It still reports one that does not resolve,
/// as a hard failure rather than the advisory dead text gets.
#[test]
fn review_does_not_call_a_workflows_tokens_inert() {
    // spec: WF-27
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: '{{ns:review-changes}}',\n  \
         description: 'Review changed files',\n}\n\
         agent(`read {{path:skill:review}}/SKILL.md`)\n",
    );

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        !r.stdout.contains("inert-token") && !r.stderr.contains("inert-token"),
        "a workflow's resolving tokens are not inert: {}\n{}",
        r.stdout,
        r.stderr
    );

    // The same file with a token that names no sibling is a hard bad-reference,
    // the treatment a markdown file's would get, not a downgraded advisory.
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'review-changes',\n  \
         description: 'Review changed files',\n}\n\
         agent(`hand off to {{ns:nonesuch}}`)\n",
    );
    let r = sb.mind(&["review", &sb.source_spec()]);
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(
        all.contains("bad-reference") && all.contains("nonesuch"),
        "an unresolvable token in a workflow is a real defect: {all}"
    );
    assert!(
        !all.contains("dead text"),
        "it must not be downgraded to the dead-text advisory: {all}"
    );
}

/// Only a workflow may `link` into `workflows/`: the directory carries DSC-97's
/// extra condition, since a file there is something the harness runs.
#[test]
fn only_a_workflow_may_link_into_the_workflows_directory() {
    // spec: WF-8
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"rule\"\nname = \"style\"\npath = \"rules/style.md\"\n\
         link = \"workflows/deploy.js\"\n",
    );
    sb.write_and_commit("rules/style.md", "---\ndescription: style\n---\n# style\n");

    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(
        !r.success,
        "a rule linking into workflows/ must be refused: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.link("deploy.js").symlink_metadata().is_ok(),
        "nothing may be linked into workflows/ by a non-workflow item"
    );
}

/// `review` reports every workflow the harness would skip -- an unreadable
/// `meta`, a missing or empty `name`/`description`, and a file over the size cap
/// -- as advisory findings that do not fail the run.
#[test]
fn review_reports_a_workflow_the_harness_will_not_load() {
    // spec: WF-7 WF-30 WF-32
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/no-meta.js", "phase('Review')\nagent('go')\n");
    sb.write_and_commit(
        "workflows/blank-name.js",
        "export const meta = {\n  name: '   ',\n  description: 'Review the diff',\n}\n",
    );
    sb.write_and_commit(
        "workflows/no-description.js",
        "export const meta = {\n  name: 'no-description',\n}\n",
    );
    // 524288 bytes is the cap; one byte past it is an overage.
    let padded = format!(
        "export const meta = {{\n  name: 'huge',\n  description: 'Huge',\n}}\n// {}\n",
        "x".repeat(524_289)
    );
    sb.write_and_commit("workflows/huge.js", &padded);

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        r.success,
        "an unloadable workflow is advisory, never a failure: {}\n{}",
        r.stdout, r.stderr
    );
    let findings: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| l.contains("[workflow-unloadable]"))
        .collect();
    let joined = findings.join("\n");
    assert!(
        joined.contains("workflow:no-meta") && joined.contains("no `meta` object"),
        "an unreadable meta must be reported: {joined}"
    );
    assert!(
        joined.contains("workflow:blank-name") && joined.contains("`meta.name` is empty"),
        "an empty name must be reported: {joined}"
    );
    assert!(
        joined.contains("workflow:no-description")
            && joined.contains("`meta.description` is missing"),
        "a missing description must be reported: {joined}"
    );
    assert!(
        joined.contains("workflow:huge") && joined.contains("524288-byte cap"),
        "a file over the cap must be reported: {joined}"
    );
    assert!(
        !joined.contains("workflow:review-changes"),
        "a complete workflow must not be reported: {joined}"
    );

    // spec: WF-32 -- reported, not enforced: the over-cap file still installs.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:huge"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("huge.js").symlink_metadata().is_ok(),
        "the over-cap workflow is installed anyway"
    );
}

/// `learn` warns about a workflow the harness would skip and installs it: mind's
/// reader disagreeing with the harness's must not decide an install.
#[test]
fn learn_warns_about_an_unloadable_workflow_and_installs_it() {
    // spec: WF-30 WF-31
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/no-meta.js", "phase('Review')\nagent('go')\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "workflow:no-meta"]);
    assert!(
        r.success,
        "the install must succeed: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("the harness will not load this workflow")
            && r.stderr.contains("installed anyway"),
        "learn must warn and say it installed anyway: {}",
        r.stderr
    );
    assert!(
        sb.link("no-meta.js").symlink_metadata().is_ok(),
        "the workflow is installed despite the warning"
    );

    // A complete workflow in the same source draws no warning.
    let r = sb.mind(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("will not load"),
        "a loadable workflow must be silent: {}",
        r.stderr
    );
}

/// A `meta.name` that differs from the item's effective name is reported at
/// `learn` and by `review`, and stays visible in the `recall <item>` detail.
#[test]
fn a_diverging_meta_name_is_reported_at_learn_review_and_recall() {
    // spec: WF-24
    let sb = Sandbox::new();
    // The file stem is `review-changes`; the harness will answer to `deploy`.
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Review changed files',\n}\n",
    );

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("[workflow-name]")
            && r.stdout.contains("resolves it as 'deploy'")
            && r.stdout.contains("not 'review-changes'"),
        "review must report the divergence: {}",
        r.stdout
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("resolves it as 'deploy'"),
        "learn must warn about the divergence: {}",
        r.stderr
    );

    let r = sb.mind(&["recall", "workflow:review-changes"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("resolves it as 'deploy'"),
        "the detail view must keep reporting it: {}",
        r.stdout
    );

    // spec: WF-24 -- absent from the listing and from introspect, where it would
    // be unactionable noise.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("resolves it as"),
        "the listing must stay quiet: {}",
        r.stdout
    );
    let r = sb.mind(&["introspect"]);
    assert!(
        !r.stdout.contains("resolves it as") && !r.stderr.contains("resolves it as"),
        "introspect must stay quiet: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// A prefixed source whose `meta.name` is tokenized does NOT diverge: the token
/// expands to the effective name, which is the whole point of WF-23.
#[test]
fn a_tokenized_meta_name_draws_no_divergence_warning() {
    // spec: WF-23 WF-24
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: '{{ns:review-changes}}',\n  description: 'Review changed files',\n}\n",
    );
    let r = sb.mind(&["review", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("[workflow-name]"),
        "a tokenized meta.name matches the prefixed effective name: {}",
        r.stdout
    );

    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--namespace", "jk"])
            .success
    );
    let r = sb.mind(&["learn", "workflow:jk:review-changes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("resolves it as"),
        "learn must stay quiet: {}",
        r.stderr
    );
}

/// Two workflows whose `meta.name` agree are one workflow to the harness. mind
/// reports it at `learn`, in `review`, and in the `recall <item>` detail, and
/// installs both.
#[test]
fn two_workflows_sharing_a_meta_name_are_reported() {
    // spec: WF-29
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/deploy-staging.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Deploy to staging',\n}\n",
    );
    sb.write_and_commit(
        "workflows/deploy-prod.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Deploy to prod',\n}\n",
    );

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(
        r.success,
        "a name collision is advisory, never a failure: {}\n{}",
        r.stdout, r.stderr
    );
    let findings: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| l.contains("[workflow-name-collision]"))
        .collect();
    assert_eq!(
        findings.len(),
        2,
        "both sides of the collision are reported: {:?}",
        findings
    );
    assert!(
        findings.iter().all(|f| f.contains("harness name 'deploy'")),
        "the finding names the shared harness name: {findings:?}"
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "--all", "agents"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("harness name 'deploy'"),
        "learn must warn about the shared name: {}",
        r.stderr
    );
    // spec: WF-29 -- both install; nothing is blocked.
    assert!(sb.link("deploy-staging.js").symlink_metadata().is_ok());
    assert!(sb.link("deploy-prod.js").symlink_metadata().is_ok());

    let r = sb.mind(&["recall", "workflow:deploy-prod"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:deploy-staging"),
        "the detail view names the other claimant: {}",
        r.stdout
    );
}

/// The unguarded-reference scan covers a workflow file, as it covers every text
/// file of an item.
#[test]
fn the_unguarded_reference_scan_covers_a_workflow_file() {
    // spec: WF-28
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: '{{ns:review-changes}}',\n  description: 'Review changed files',\n}\n\
         agent('follow the review skill')\n",
    );

    let r = sb.mind(&["review", &sb.source_spec(), "--as", "jk"]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("[unguarded-reference]")
            && r.stdout.contains("workflow:jk:review-changes"),
        "a bare sibling name in a workflow's prompt must be reported: {}",
        r.stdout
    );
}
