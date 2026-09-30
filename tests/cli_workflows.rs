//! The `workflow` item kind (spec/workflows.md): end-to-end tests that
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
//!   WF-24: the divergence warning, and that its remedy token installs
//!   WF-50: the kind-generic machinery (upgrade, forget, unmanaged) covers it
//!   WF-51: `probe` shows `whenToUse` beside the description
//!   WF-56: mind's own metadata cap is reported as mind's, not as the harness's
//!   WF-58: a `meta.name` with an invisible character is unusable, and said so
//!   WF-60: `recall <item> --json` carries the harness name and its findings,
//!     including the WF-30 unloadable reasons
//!   WF-61: an unmanaged lobe workflow is a WF-29 collision claimant
//!   WF-62: `--json` carries `description` and `when_to_use` separately

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU32 = AtomicU32::new(0);
/// Names the capture files of a single [`Sandbox::mind_bounded`] run.
static BOUNDED_COUNTER: AtomicU32 = AtomicU32::new(0);

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
            // The cap tests below pass `--max-metadata-size` explicitly, and
            // the rest depend on the default; a developer with
            // MIND_MAX_METADATA_SIZE exported would otherwise see both sets
            // fail for a reason that has nothing to do with the code.
            .env_remove("MIND_MAX_METADATA_SIZE")
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

    /// `mind` under a deadline, so a scan that stops making progress fails the
    /// test instead of hanging the suite. Output goes to files rather than
    /// pipes: a child that has to be killed cannot then deadlock the reader.
    fn mind_bounded(&self, args: &[&str]) -> Run {
        // One pair of capture files per call, not per sandbox: two bounded runs
        // sharing a sandbox (a test that drives several verbs, or two threads
        // inside one) would otherwise truncate each other's output and the
        // assertions would read whichever won.
        let n = BOUNDED_COUNTER.fetch_add(1, Ordering::SeqCst);
        let out_path = self.base.join(format!("bounded-{n}.out"));
        let err_path = self.base.join(format!("bounded-{n}.err"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_MAX_METADATA_SIZE")
            .stdout(Stdio::from(File::create(&out_path).unwrap()))
            .stderr(Stdio::from(File::create(&err_path).unwrap()))
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn mind");
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            match child.try_wait().expect("wait for mind") {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("`mind {}` did not finish within 60s", args.join(" "));
                }
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        };
        Run {
            stdout: String::from_utf8_lossy(&std::fs::read(&out_path).unwrap()).into_owned(),
            stderr: String::from_utf8_lossy(&std::fs::read(&err_path).unwrap()).into_owned(),
            success: status.success(),
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

    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout
            .contains("Review changed files - before opening a PR"),
        "the workflow row must read `<description> - <whenToUse>`: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Review the diff") && !r.stdout.contains("Review the diff -"),
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
    // spec: WF-3 -- the extension is compared case-SENSITIVELY, as the harness
    // compares it, so an uppercase spelling is not a workflow. Without a
    // fixture here an `eq_ignore_ascii_case` regression would discover (and
    // install) a file the harness would never load.
    sb.write_and_commit("workflows/LOUD.JS", REVIEW_JS);
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:review-changes"),
        "the flat `.js` workflow is still found: {}",
        r.stdout
    );
    for missed in ["deep", "modern", "typed", "notes", "LOUD"] {
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
    // The run has to have SUCCEEDED and reached the token checks, or the
    // absence of `inert-token` below means only that review printed nothing.
    assert!(
        r.success,
        "review must succeed on a source whose tokens all resolve: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("[workflow-content]") && r.stdout.contains("workflow:review-changes"),
        "review must have reached the workflow checks at all: {}\n{}",
        r.stdout,
        r.stderr
    );
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
    // A hard finding fails the run and prints as `error [kind]:` on stderr;
    // an advisory would leave the run green and print `advisory [kind]:`.
    assert!(
        !r.success,
        "a hard bad-reference must fail the review: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr
            .lines()
            .any(|l| l.starts_with("error [bad-reference]:") && l.contains("nonesuch")),
        "an unresolvable token in a workflow is a hard defect: {all}"
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

    let r = sb.mind(&["meld", &sb.source_spec(), "--json"]);
    assert!(
        !r.success,
        "a rule linking into workflows/ must be refused: {}\n{}",
        r.stdout, r.stderr
    );
    // Refused for the RIGHT reason: a meld can fail for a dozen unrelated
    // causes, and this test is about the confined-link-target rule.
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document: {e}\n{}", r.stdout));
    assert_eq!(
        doc["error"]["kind"].as_str(),
        Some("mind-toml"),
        "the refusal must come from validating the declared link, not from some \
         unrelated failure: {}",
        r.stdout
    );
    let message = format!("{}{}", doc["error"]["message"], r.stderr);
    assert!(
        message.contains("workflows/deploy.js") && message.contains("link"),
        "and must name the offending link target: {message}"
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
    // 524288 bytes is the cap and is NOT an overage; the fixture is exactly one
    // byte past it, so a `>=` comparison in place of the `>` would fail the
    // `at the cap` half of this test rather than pass it silently.
    let sized = |name: &str, bytes: usize| -> String {
        let header =
            format!("export const meta = {{\n  name: '{name}',\n  description: 'Big',\n}}\n// ");
        let mut s = String::with_capacity(bytes);
        s.push_str(&header);
        s.push_str(&"x".repeat(bytes - header.len()));
        assert_eq!(s.len(), bytes, "the fixture size must be exact");
        s
    };
    sb.write_and_commit("workflows/at-cap.js", &sized("at-cap", 524_288));
    sb.write_and_commit("workflows/huge.js", &sized("huge", 524_289));

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
        joined.contains("workflow:no-meta")
            && joined.contains("mind read no `name`, `description`, or `whenToUse`"),
        "an unreadable meta must be reported, in terms of what mind read: {joined}"
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
    // spec: WF-7 -- the cap itself is not an overage: the file exactly AT it
    // draws no finding, which is what pins the comparison as `>`.
    assert!(
        !joined.contains("workflow:at-cap"),
        "a file exactly at the cap is not over it: {joined}"
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
    let r = sb.mind(&["review", &sb.source_spec(), "--namespace", "jk"]);
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

/// The divergence warning's remedy is FOLLOWABLE: writing the exact
/// `{{ns:<name>}}` token it prints into `meta.name` clears the warning and the
/// install succeeds.
///
/// The remedy has to name the BARE name. Under a prefix, `{{ns:}}` resolves
/// only against bare sibling names (NS-11), so a `{{ns:jk:review-changes}}`
/// token names no sibling and install aborts with a hard `bad-reference`
/// (NS-12) -- mind's own advice would have traded an advisory warning for a
/// failed install. This test takes the token straight out of the message rather
/// than restating it, so it cannot pass against a message that suggests
/// something else.
// spec: WF-24 WF-23 NS-11
#[test]
fn the_divergence_remedy_token_is_one_that_actually_installs() {
    let sb = Sandbox::new();
    // A literal `meta.name` under a prefix: the item installs as
    // `jk:review-changes` and the harness answers to `review-changes`.
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'review-changes',\n  description: 'Review changed files',\n}\n",
    );

    let r = sb.mind(&["review", &sb.source_spec(), "--namespace", "jk"]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    let finding = r
        .stdout
        .lines()
        .find(|l| l.contains("[workflow-name]"))
        .unwrap_or_else(|| panic!("review must report the divergence: {}", r.stdout))
        .to_string();
    assert!(
        finding.contains("resolves it as 'review-changes'")
            && finding.contains("not 'jk:review-changes'"),
        "the divergence names both spellings: {finding}"
    );

    // Pull the suggested token out of the message and write exactly that.
    let start = finding
        .find("{{ns:")
        .unwrap_or_else(|| panic!("the finding must suggest a token: {finding}"));
    let end = finding[start..]
        .find("}}")
        .map(|i| start + i + 2)
        .unwrap_or_else(|| panic!("the suggested token must be terminated: {finding}"));
    let token = &finding[start..end];
    assert_eq!(
        token, "{{ns:review-changes}}",
        "the remedy must name the BARE name, the only spelling `{{{{ns:}}}}` resolves: {finding}"
    );
    sb.write_and_commit(
        "workflows/review-changes.js",
        &format!(
            "export const meta = {{\n  name: '{token}',\n  description: 'Review changed files',\n}}\n"
        ),
    );

    // The advice taken: review is quiet and the install succeeds.
    let r = sb.mind(&["review", &sb.source_spec(), "--namespace", "jk"]);
    assert!(
        r.success,
        "review after the fix: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("[workflow-name]"),
        "following the advice must clear the divergence: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("bad-reference") && !r.stderr.contains("bad-reference"),
        "the suggested token must resolve: {}\n{}",
        r.stdout,
        r.stderr
    );

    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--namespace", "jk"])
            .success
    );
    let r = sb.mind(&["learn", "workflow:jk:review-changes"]);
    assert!(
        r.success,
        "the install must succeed with the suggested token: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("resolves it as"),
        "and draw no divergence warning: {}",
        r.stderr
    );
    let installed = std::fs::read_to_string(sb.link("jk:review-changes.js")).unwrap();
    assert!(
        installed.contains("name: 'jk:review-changes'"),
        "the token expanded to the effective name: {installed}"
    );
}

/// A `meta.name` carrying an invisible code point is not a usable harness name:
/// it is reported as its own defect rather than silently accepted, and it draws
/// neither a divergence (which would read "resolves it as 'review', not
/// 'review'") nor a collision against the real `review`.
// spec: WF-58
#[test]
fn an_invisible_character_in_a_meta_name_is_reported_not_accepted() {
    let sb = Sandbox::new();
    // `deploy` is an ordinary workflow claiming `deploy`. `staging` carries a
    // zero-width space in its name, so it PRINTS as `deploy` too: accepted, it
    // would report a collision against a name that looks identical and a
    // divergence reading "resolves it as 'deploy', not 'staging'".
    sb.write_and_commit(
        "workflows/deploy.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Deploy it',\n}\n",
    );
    sb.write_and_commit(
        "workflows/staging.js",
        "export const meta = {\n  name: 'dep\u{200B}loy',\n  description: 'Deploy to staging',\n}\n",
    );

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    let unloadable: Vec<&str> = r
        .stdout
        .lines()
        .filter(|l| l.contains("[workflow-unloadable]"))
        .collect();
    let joined = unloadable.join("\n");
    assert!(
        joined.contains("workflow:staging") && joined.contains("control or invisible character"),
        "an invisible character in `meta.name` must be reported: {}",
        r.stdout
    );
    assert!(
        !joined.contains("workflow:deploy:"),
        "the ordinary sibling draws nothing: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("[workflow-name-collision]"),
        "an unusable name claims nothing, so it collides with nothing: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("resolves it as"),
        "and draws no divergence, which would compare a name with itself: {}",
        r.stdout
    );

    // spec: WF-31 -- reported, never enforced: it still installs.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:staging"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("control or invisible character"),
        "learn warns on the same terms: {}",
        r.stderr
    );
    assert!(
        sb.link("staging.js").symlink_metadata().is_ok(),
        "the workflow installs anyway"
    );
}

/// The other side of WF-58: a `meta.name` written as a template literal
/// spanning lines carries a leading and trailing newline, which is a control
/// character the WF-58 test would flag if it ran before the trim. It must not:
/// the name mind uses is the trimmed one, so the workflow is loadable, agrees
/// with its file name, and draws no finding at all.
// spec: WF-58 WF-24
#[test]
fn a_meta_name_padded_by_a_multiline_template_is_usable_not_a_defect() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/release.js",
        "export const meta = {\n  name: `\n    release\n  `,\n  \
         description: 'Cut a release',\n}\n",
    );

    let r = sb.mind(&["review", &sb.source_spec()]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("control or invisible character"),
        "padding is not a character defect: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("[workflow-unloadable]"),
        "a padded name is a name: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("resolves it as"),
        "the trimmed name is the item's own name, so nothing diverges: {}",
        r.stdout
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:release"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("control or invisible character")
            && !r.stderr.contains("resolves it as"),
        "learn warns about nothing either: {}",
        r.stderr
    );

    // And the name mind reports using is the trimmed one, not the padded text.
    let r = sb.mind(&["recall", "workflow:release", "--json"]);
    assert!(r.success, "recall --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one JSON document");
    assert_eq!(
        doc["harness_name"].as_str(),
        Some("release"),
        "the harness name is the trimmed one: {}",
        r.stdout
    );
    assert_eq!(
        doc["workflow_findings"].as_array().map(Vec::len),
        Some(0),
        "and there is nothing to report: {}",
        r.stdout
    );
}

/// One shared harness name is one warning, and its subject is an item the run
/// touched -- not whichever claimant happens to sort first. `alpha` is installed
/// by an earlier run and sorts before `zeta`; the run that installs `zeta` is the
/// one that has something to say, so the warning is about `zeta`.
// spec: WF-29 WF-59
#[test]
fn a_collision_warning_is_subjected_to_the_item_the_run_touched() {
    let sb = Sandbox::new();
    for name in ["alpha", "zeta"] {
        sb.write_and_commit(
            &format!("workflows/{name}.js"),
            &format!(
                "export const meta = {{\n  name: 'deploy',\n  \
                 description: 'Deploy from {name}',\n}}\n"
            ),
        );
    }
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let first = sb.mind(&["learn", "workflow:alpha"]);
    assert!(
        first.success,
        "learn alpha: {}\n{}",
        first.stdout, first.stderr
    );
    assert!(
        !first.stderr.contains("harness name 'deploy'"),
        "one claimant is no collision: {}",
        first.stderr
    );

    let second = sb.mind(&["learn", "workflow:zeta"]);
    assert!(
        second.success,
        "learn zeta: {}\n{}",
        second.stdout, second.stderr
    );
    let lines: Vec<&str> = second
        .stderr
        .lines()
        .filter(|l| l.contains("harness name 'deploy'"))
        .collect();
    assert_eq!(lines.len(), 1, "one name, one warning: {lines:?}");
    assert!(
        lines[0].contains("warning: workflow:zeta:"),
        "the subject is the item this run installed, not the alphabetically first \
         claimant: {lines:?}"
    );
    assert!(
        lines[0].contains("workflow:alpha also claims"),
        "and the untouched claimant is named as the other: {lines:?}"
    );

    assert!(
        !lines[0].contains("workflow:zeta also"),
        "no claimant is ever reported against itself: {lines:?}"
    );

    // The other ordering: one run that touches BOTH claimants still warns once,
    // subjected to the first of them, and still lists only the other.
    let sb = Sandbox::new();
    for name in ["alpha", "zeta"] {
        sb.write_and_commit(
            &format!("workflows/{name}.js"),
            &format!(
                "export const meta = {{\n  name: 'deploy',\n  \
                 description: 'Deploy from {name}',\n}}\n"
            ),
        );
    }
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let both = sb.mind(&["learn", "--all", "agents"]);
    assert!(both.success, "learn both: {}\n{}", both.stdout, both.stderr);
    let lines: Vec<&str> = both
        .stderr
        .lines()
        .filter(|l| l.contains("harness name 'deploy'"))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "one name is one warning however many of its claimants a run touched: {lines:?}"
    );
    assert!(
        lines[0].contains("warning: workflow:alpha:") && lines[0].contains("workflow:zeta also"),
        "the first claimant is the subject when the run touched both: {lines:?}"
    );
    assert!(
        !lines[0].contains("workflow:alpha also"),
        "and it is not listed among the others: {lines:?}"
    );
}

/// `recall <item> --json` carries the harness-facing name and the findings about
/// it, so a scripted consumer sees the divergence and the collision the text
/// view prints.
// spec: WF-60
#[test]
fn recall_json_carries_the_harness_name_and_its_findings() {
    let sb = Sandbox::new();
    // `deploy-staging` answers to `deploy` (a WF-24 divergence) and shares that
    // name with `deploy-prod` (a WF-29 collision).
    sb.write_and_commit(
        "workflows/deploy-staging.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Deploy to staging',\n}\n",
    );
    sb.write_and_commit(
        "workflows/deploy-prod.js",
        "export const meta = {\n  name: 'deploy',\n  description: 'Deploy to prod',\n}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "workflow:deploy-staging"]).success);
    assert!(sb.mind(&["learn", "workflow:deploy-prod"]).success);

    let r = sb.mind(&["recall", "workflow:deploy-staging", "--json"]);
    assert!(r.success, "recall --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document: {e}\n{}", r.stdout));
    assert_eq!(
        doc["harness_name"].as_str(),
        Some("deploy"),
        "the document must carry the name the harness answers to: {}",
        r.stdout
    );
    let findings: Vec<&str> = doc["workflow_findings"]
        .as_array()
        .unwrap_or_else(|| panic!("workflow_findings must be an array: {}", r.stdout))
        .iter()
        .map(|f| f.as_str().expect("each finding is a string"))
        .collect();
    assert!(
        findings
            .iter()
            .any(|f| f.contains("resolves it as 'deploy'") && f.contains("not 'deploy-staging'")),
        "the WF-24 divergence must ride the document: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.contains("harness name 'deploy'") && f.contains("workflow:deploy-prod")),
        "so must the WF-29 collision: {findings:?}"
    );
    // The same strings the text view prints, so the two cannot drift.
    let text = sb.mind(&["recall", "workflow:deploy-staging"]);
    assert!(text.success, "recall: {}\n{}", text.stdout, text.stderr);
    for finding in &findings {
        assert!(
            text.stdout.contains(finding),
            "every JSON finding must be one the text view prints: {finding}\n{}",
            text.stdout
        );
    }

    // A non-workflow item's document is unchanged: the fields are the kind's.
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(r.success, "recall skill --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one JSON document");
    assert!(
        doc.get("harness_name").is_none() && doc.get("workflow_findings").is_none(),
        "only a workflow carries the workflow fields: {}",
        r.stdout
    );
}

/// `recall <item>` of a workflow whose store copy mind cannot read says nothing
/// about the harness rather than erroring: `harness_name` is null, the findings
/// are empty, and the text view prints no `harness` line. The read is the one
/// thing in the detail view that touches the file system, so a store copy
/// deleted by hand, replaced by a directory, or holding bytes that are not UTF-8
/// must not be able to fail the command that is meant to report on it.
///
/// WF-56 is not in play for any of these: mind's own cap is about a file too
/// large to read, not one it could not read at all, so no cap notice may appear.
/// An EMPTY store copy is not in this list: mind reads it fine and it declares
/// no `meta`, which is a WF-30 finding (see
/// `recall_reports_why_the_harness_will_not_load_an_installed_workflow`).
// spec: WF-60 WF-5 WF-56
#[test]
fn recall_of_a_workflow_whose_store_copy_is_unreadable_reports_no_harness_name() {
    for (what, break_it) in [("deleted", 0u8), ("a directory", 1), ("not UTF-8", 2)] {
        let sb = Sandbox::new();
        assert!(sb.mind(&["meld", &sb.source_spec()]).success);
        assert!(sb.mind(&["learn", "workflow:review-changes"]).success);
        let store = sb.mind_home.join("store/workflow/review-changes");
        assert!(store.is_file(), "the store copy is a file to begin with");
        match break_it {
            0 => std::fs::remove_file(&store).unwrap(),
            1 => {
                std::fs::remove_file(&store).unwrap();
                std::fs::create_dir(&store).unwrap();
            }
            _ => std::fs::write(&store, [0xff, 0xfe, 0x00, 0x80]).unwrap(),
        }

        let r = sb.mind(&["recall", "workflow:review-changes", "--json"]);
        assert!(
            r.success,
            "recall --json with a {what} store copy must still succeed: {}\n{}",
            r.stdout, r.stderr
        );
        let doc: serde_json::Value = serde_json::from_str(r.stdout.trim())
            .unwrap_or_else(|e| panic!("one JSON document ({what}): {e}\n{}", r.stdout));
        assert!(
            doc["harness_name"].is_null(),
            "a {what} store copy yields no harness name: {}",
            r.stdout
        );
        assert_eq!(
            doc["workflow_findings"].as_array().map(Vec::len),
            Some(0),
            "and no findings, since mind knows nothing about the file ({what}): {}",
            r.stdout
        );
        assert!(
            !r.stdout.contains("max-metadata-size"),
            "an unreadable file is not an over-cap one ({what}): {}",
            r.stdout
        );

        let text = sb.mind(&["recall", "workflow:review-changes"]);
        assert!(
            text.success,
            "the text view too ({what}): {}\n{}",
            text.stdout, text.stderr
        );
        assert!(
            !text.stdout.contains("harness "),
            "nothing to say about the harness ({what}): {}",
            text.stdout
        );
    }
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
    // spec: WF-59 -- one shared name is ONE finding, naming the claimants.
    assert_eq!(
        findings.len(),
        1,
        "one finding for the one shared name: {:?}",
        findings
    );
    assert!(
        findings[0].contains("harness name 'deploy'"),
        "the finding names the shared harness name: {findings:?}"
    );
    assert!(
        findings[0].contains("workflow:deploy-prod")
            && findings[0].contains("workflow:deploy-staging"),
        "and names both claimants: {findings:?}"
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

/// A workflow whose `meta` holds a shape the reader does not model - a regex
/// literal carrying an unbalanced `)` - is read, not stalled on. The `meta`
/// reader runs on every discovered workflow in the ordinary catalog scan while
/// the process lock is held, so a cursor that stops advancing hangs `meld`,
/// `probe`, `learn`, and every other verb, and blocks every other `mind`
/// process behind the lock. Each run here is bounded, so a regression fails.
#[test]
fn a_workflow_meta_the_reader_cannot_model_does_not_stall_a_scan() {
    // spec: WF-5
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/deploy.js",
        "export const meta = {\n  name: 'deploy',\n  tagPattern: /release\\)/,\n  \
         description: 'Deploy a release',\n}\nphase('Deploy')\n",
    );

    let r = sb.mind_bounded(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["probe", "--no-tui", "--kind", "workflow"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("Deploy a release"),
        "the keys around the unreadable one still read: {}",
        r.stdout
    );

    let r = sb.mind_bounded(&["learn", "workflow:deploy"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("deploy.js").symlink_metadata().is_ok(),
        "the workflow installs: the reader yields less, it never fails an install"
    );
}

/// A `meta` whose every entry is a shape the reader does not model: an
/// unbalanced `)` inside a regex literal, an arrow body that closes one paren
/// too many, a computed key holding a closer, and a nested array that does the
/// same. Each one leaves a character in the entry position that neither the key
/// reader nor the value skipper will consume - the no-progress case - and the
/// two string keys on the ends prove the reader still read around all of it.
const PATHOLOGICAL_JS: &str = "export const meta = {\n  name: 'deploy',\n  \
     tagPattern: /rel)ease/,\n  arrow: () => x),\n  [)]: 1,\n  \
     phases: [{ title: 'a' ) }],\n  description: 'Deploy a release',\n  \
     whenToUse: 'when shipping',\n}\nphase('Deploy')\n";

/// The `meta` reader runs on every discovered workflow in the ordinary catalog
/// scan, and again on the installed store copy for the WF-24/WF-29/WF-30
/// reports, so a cursor that stops advancing hangs whatever verb touched it
/// while that verb holds the process lock - blocking every other `mind` process
/// too. `meld`, `probe`, and `learn` are covered above; these are the rest of
/// the verbs that reach the same reader. Every run is bounded, so a regression
/// fails the test instead of hanging the suite.
#[test]
fn a_pathological_workflow_meta_stalls_no_verb_that_scans() {
    // spec: WF-5
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/deploy.js", PATHOLOGICAL_JS);
    let r = sb.mind_bounded(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind_bounded(&["learn", "workflow:deploy"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    // `sync` rescans the source; `introspect` and `dump` walk the installed set
    // and the catalog behind it.
    for args in [
        vec!["sync"],
        vec!["introspect"],
        vec!["dump"],
        vec!["recall", "--sources"],
    ] {
        let r = sb.mind_bounded(&args);
        assert!(
            r.success,
            "`mind {}` failed: {}\n{}",
            args.join(" "),
            r.stdout,
            r.stderr
        );
    }

    // `recall` reads the STORE copy through `workflow_check`, a second reach
    // into the same reader that the source-side scan does not cover.
    let r = sb.mind_bounded(&["recall"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:deploy"),
        "the item is listed as installed: {}",
        r.stdout
    );
    let r = sb.mind_bounded(&["recall", "workflow:deploy"]);
    assert!(r.success, "recall detail: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("Deploy a release"),
        "the keys around the unreadable ones still describe the item: {}",
        r.stdout
    );

    // `review` reads every item file of the source through the same check.
    let r = sb.mind_bounded(&["review", &sb.source_spec()]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);

    // And the item still upgrades and forgets: the reader yields less on a
    // shape it cannot model, it never fails an operation.
    sb.write_and_commit(
        "workflows/deploy.js",
        &PATHOLOGICAL_JS.replace("Deploy a release", "Deploy a release v2"),
    );
    let r = sb.mind_bounded(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(
        std::fs::read_to_string(sb.link("deploy.js"))
            .unwrap()
            .contains("Deploy a release v2"),
        "the new content is linked: {}",
        r.stdout
    );
    let r = sb.mind_bounded(&["forget", "workflow:deploy", "--yes"]);
    assert!(r.success, "forget: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("deploy.js").symlink_metadata().is_err(),
        "forget removes the lobe link"
    );
}

/// The same reader on the curate path: a curator that lists the source pulls
/// its catalog through the identical scan, with `curate` holding the lock for
/// the whole reconcile.
#[test]
fn curate_does_not_stall_on_a_pathological_workflow_meta() {
    // spec: WF-5
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/deploy.js", PATHOLOGICAL_JS);

    let curator = sb.base.join("curator");
    write(
        &curator.join("mind.toml"),
        &format!(
            "[discover]\nsources = [\n  {{ source = \"{}\", install = true }}\n]\n",
            sb.source_spec()
        ),
    );
    git(&curator, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&curator, &["config", "user.email", "t@t"]);
    git(&curator, &["config", "user.name", "t"]);
    git(&curator, &["add", "-A"]);
    git(&curator, &["commit", "-qm", "curator"]);

    let spec = curator.to_string_lossy().into_owned();
    let r = sb.mind_bounded(&["meld", &spec, "--register-only"]);
    assert!(r.success, "meld curator: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["curate", "--check"]);
    assert!(r.success, "curate --check: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["curate", "--yes"]);
    assert!(r.success, "curate --yes: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("deploy.js").symlink_metadata().is_ok(),
        "the curated source's workflow installs: {}\n{}",
        r.stdout,
        r.stderr
    );
}

// ---- WF-55: an over-cap workflow, wherever it is declared -------------------
//
// The cap these use is a lowered one (`--max-metadata-size`, DSC-103) rather
// than a real 8 MiB file: the relaxation keys on the item's KIND, not on how
// many bytes tripped it, so the behavior is the same and the fixture costs a
// few hundred bytes instead of eight megabytes. The default cap against a real
// over-sized file is covered in cli_workflows_upgrade.rs. Every run is bounded,
// since these also walk the `meta` reader.

/// A workflow small enough to sit under the lowered cap, so a test can tell an
/// over-cap sibling apart from a source-wide failure.
const TINY_JS: &str = "export const meta={name:'fine',description:'Fine'}\n";

/// Bytes of padding that put a workflow past the lowered cap without putting a
/// `mind.toml` or a plugin manifest past it.
fn padded_js(name: &str) -> String {
    format!(
        "export const meta = {{ name: '{name}', description: 'Ship it' }}\n// {}\n",
        "p".repeat(2600)
    )
}

/// An over-cap workflow leaves every catalog-scanning verb working, and the
/// healthy sibling of that source keeps its description everywhere. This is the
/// blast radius WF-55 closed: the scan builds a source's whole item list in one
/// pass, so one unreadable file used to take the rest of the source with it.
// spec: WF-55 WF-30
#[test]
fn an_over_cap_workflow_leaves_every_scanning_verb_working() {
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/fine.js", TINY_JS);
    sb.write_and_commit("workflows/huge.js", &padded_js("huge"));
    let cap = ["--max-metadata-size", "2KiB"];

    let mut meld = vec!["meld", "--register-only"];
    let spec = sb.source_spec();
    meld.push(&spec);
    meld.extend_from_slice(&cap);
    let r = sb.mind_bounded(&meld);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    // probe sees BOTH: the over-cap one as an item with no description, the
    // sibling with the description the cap never touched.
    let mut probe = vec!["probe", "--no-tui"];
    probe.extend_from_slice(&cap);
    let r = sb.mind_bounded(&probe);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("workflow:huge") && r.stdout.contains("workflow:fine"),
        "both workflows must be offered: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Fine"),
        "the sibling keeps its description: {}",
        r.stdout
    );

    for (name, unloadable) in [("workflow:fine", false), ("workflow:huge", true)] {
        let mut learn = vec!["learn", name];
        learn.extend_from_slice(&cap);
        let r = sb.mind_bounded(&learn);
        assert!(r.success, "learn {name}: {}\n{}", r.stdout, r.stderr);
        // The warning is what proves the fixture bites: the over-cap file
        // reaches mind as a workflow mind read nothing of, and the sibling
        // under the cap is untouched.
        //
        // spec: WF-56 -- and the warning names MIND's cap and the flag that
        // raises it, rather than claiming the harness will not load a file
        // mind never read. The cap here is a lowered one, far below the
        // harness's own 524288 bytes, so this workflow is one the harness
        // loads perfectly well.
        assert_eq!(
            r.stderr.contains("over mind's own 2 KiB metadata read cap"),
            unloadable,
            "learn {name} reported the wrong loadability: {}",
            r.stderr
        );
        assert_eq!(
            r.stderr.contains("--max-metadata-size"),
            unloadable,
            "learn {name} must name the flag that raises the cap: {}",
            r.stderr
        );
        assert!(
            !r.stderr.contains("the harness will not load this workflow"),
            "mind's own cap says nothing about the harness: {}",
            r.stderr
        );
    }
    assert!(
        sb.link("huge.js").symlink_metadata().is_ok(),
        "the over-cap workflow installs like any other unloadable one (WF-31)"
    );

    for args in [
        vec!["sync"],
        vec!["introspect"],
        vec!["dump"],
        vec!["recall"],
        vec!["recall", "--sources"],
        vec!["recall", "workflow:huge"],
        vec!["upgrade", "--yes"],
        vec!["review", &spec],
    ] {
        let mut full = args.clone();
        full.extend_from_slice(&cap);
        let r = sb.mind_bounded(&full);
        assert!(
            r.success,
            "`mind {}` failed: {}\n{}",
            args.join(" "),
            r.stdout,
            r.stderr
        );
        assert!(
            !format!("{}{}", r.stdout, r.stderr).contains("size cap"),
            "`mind {}` must not report the cap as a failure: {}\n{}",
            args.join(" "),
            r.stdout,
            r.stderr
        );
    }
}

/// The same through `curate`, which pulls the catalog while holding the lock
/// for a whole reconcile, so a failure there blocks more than one verb.
// spec: WF-55
#[test]
fn curate_installs_a_source_carrying_an_over_cap_workflow() {
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/huge.js", &padded_js("huge"));

    let curator = sb.base.join("curator");
    write(
        &curator.join("mind.toml"),
        &format!(
            "[discover]\nsources = [\n  {{ source = \"{}\", install = true }}\n]\n",
            sb.source_spec()
        ),
    );
    git(&curator, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&curator, &["config", "user.email", "t@t"]);
    git(&curator, &["config", "user.name", "t"]);
    git(&curator, &["add", "-A"]);
    git(&curator, &["commit", "-qm", "curator"]);

    let spec = curator.to_string_lossy().into_owned();
    let r = sb.mind_bounded(&[
        "meld",
        &spec,
        "--register-only",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(r.success, "meld curator: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["curate", "--yes", "--max-metadata-size", "2KiB"]);
    assert!(r.success, "curate: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("huge.js").symlink_metadata().is_ok(),
        "the curated source's over-cap workflow installs: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// The relaxation is not tied to the convention scan: a workflow declared by an
/// authoritative `mind.toml` `[[items]]` entry, at a path the scan would never
/// look at, behaves the same. The `mind.toml` itself is ordinary metadata and
/// stays under the same cap, so this also pins that the relaxation is scoped to
/// the item and does not leak to the file that declared it.
// spec: WF-55 WF-8
#[test]
fn an_over_cap_workflow_declared_in_mind_toml_is_catalogued() {
    let sb = Sandbox::new();
    write(&sb.source.join("flows/huge.js"), &padded_js("huge"));
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"workflow\"\nname = \"huge\"\npath = \"flows/huge.js\"\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind_bounded(&[
        "meld",
        &spec,
        "--register-only",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["learn", "workflow:huge", "--max-metadata-size", "2KiB"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("huge.js").symlink_metadata().is_ok(),
        "a declared over-cap workflow installs: {}\n{}",
        r.stdout,
        r.stderr
    );
    // spec: WF-56 -- reported as mind's own unread file, not as a read failure
    // and not as a verdict about the harness.
    assert!(
        r.stderr.contains("over mind's own 2 KiB metadata read cap"),
        "it is reported as mind's cap, not as a read failure: {}",
        r.stderr
    );
}

/// And through a plugin manifest, the third declaration site: a plugin's
/// `workflows/` maps to the kind (WF-40), so the same relaxation has to hold
/// for an item the manifest arm discovered.
// spec: WF-55 WF-40
#[test]
fn an_over_cap_workflow_in_a_plugin_is_catalogued() {
    let sb = Sandbox::new();
    write(&sb.source.join("workflows/fine.js"), TINY_JS);
    write(&sb.source.join("workflows/huge.js"), &padded_js("huge"));
    sb.write_and_commit(
        ".claude-plugin/plugin.json",
        "{\"name\":\"acme-tools\",\"description\":\"Tools\"}\n",
    );

    let spec = sb.source_spec();
    let r = sb.mind_bounded(&[
        "meld",
        &spec,
        "--register-only",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_bounded(&["probe", "--no-tui", "--max-metadata-size", "2KiB"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("acme-tools:huge") && r.stdout.contains("acme-tools:fine"),
        "both of the plugin's workflows must be offered under the plugin's \
         namespace: {}",
        r.stdout
    );
}

/// The relaxation is scoped to the workflow kind. Every other kind keeps
/// DSC-91's hard refusal, which is the property that makes WF-55 a narrow
/// exception rather than a weakening of the cap.
// spec: WF-55 DSC-91
#[test]
fn an_over_cap_skill_still_fails_the_scan() {
    let sb = Sandbox::new();
    sb.write_and_commit("workflows/huge.js", &padded_js("huge"));
    sb.write_and_commit(
        "skills/review/SKILL.md",
        &format!("---\ndescription: Review\n---\n{}\n", "p".repeat(2600)),
    );

    let r = sb.mind_bounded(&[
        "meld",
        &sb.source_spec(),
        "--register-only",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(
        !r.success,
        "an over-cap SKILL.md must still fail the scan: {}\n{}",
        r.stdout, r.stderr
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("SKILL.md") && combined.contains("size cap"),
        "and must fail naming the file and the cap: {combined}"
    );
}

/// The gate that grants a workflow's `.js` token expansion (WF-25) is the same
/// one that grants an NS-57 `expand:`-listed file, and the dependency scan reads
/// it: a `{{ns:}}` in an expand-listed script is an edge install really will
/// expand, so the closure has to bring its referent in.
///
/// This lives beside the workflow tests because it pins the SHARED gate: the two
/// ways into token expansion have to be one question with one answer, or a
/// caller that asks the narrower one silently drops real edges (the dependency
/// scan did exactly that).
// spec: NS-57 WF-25 DEP-1
#[test]
fn a_token_in_an_expand_listed_file_is_a_dependency_edge() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/deployer/SKILL.md",
        "---\nname: deployer\ndescription: Deploy things\nexpand: run.sh\n---\n# deployer\n",
    );
    // The only reference to `review` is in the expand-listed script, where
    // install DOES expand it, so it is a real edge and not dead text.
    sb.write_and_commit("skills/deployer/run.sh", "#!/bin/sh\n# see {{ns:review}}\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:deployer", "--yes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the token's referent must come with the closure: {}\n{}",
        r.stdout,
        r.stderr
    );
    // And the token really was expanded in the installed copy, which is what
    // makes the edge real rather than a guess.
    let installed = std::fs::read_to_string(sb.claude_home.join("skills/deployer/run.sh")).unwrap();
    assert!(
        installed.contains("# see review") && !installed.contains("{{ns:"),
        "the expand-listed file must have been expanded: {installed}"
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

    let r = sb.mind(&["review", &sb.source_spec(), "--namespace", "jk"]);
    assert!(r.success, "review: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("[unguarded-reference]")
            && r.stdout.contains("workflow:jk:review-changes"),
        "a bare sibling name in a workflow's prompt must be reported: {}",
        r.stdout
    );
}

/// A `{{ns:}}` token in a workflow's body is a dependency edge: learning the
/// workflow brings the skill it names.
// spec: DEP-1 WF-25
#[test]
fn a_token_in_a_workflow_body_is_a_dependency_edge() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/review-changes.js",
        "export const meta = {\n  name: 'review-changes',\n  description: 'Review changed files',\n}\n\
         agent(`run {{ns:review}} on the diff`)\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:review-changes", "--yes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the token's referent must come with the closure: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// The same edge from a workflow past a lowered metadata cap: the dependency
/// scan reads the whole file, like install does (DSC-90), so mind's own cap on
/// the `meta` read (WF-55) must not drop the edge.
// spec: DEP-1 WF-55
#[test]
fn an_over_cap_workflows_token_is_still_a_dependency_edge() {
    let sb = Sandbox::new();
    let body = format!(
        "{}agent(`run {{{{ns:review}}}} on the diff`)\n",
        padded_js("review-changes")
    );
    sb.write_and_commit("workflows/review-changes.js", &body);
    let spec = sb.source_spec();
    let r = sb.mind_bounded(&[
        "meld",
        &spec,
        "--register-only",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind_bounded(&[
        "learn",
        "workflow:review-changes",
        "--yes",
        "--max-metadata-size",
        "2KiB",
    ]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("over mind's own 2 KiB metadata read cap"),
        "the fixture must really be over the cap: {}",
        r.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "an over-cap workflow's token must still pull its referent: {}\n{}",
        r.stdout,
        r.stderr
    );
}

/// An UNMANAGED lobe workflow claims its harness name like any other: `learn`
/// warns once about the shared name, naming the unmanaged file as one of the
/// others (never as the subject), and `recall <item>` (text and `--json`)
/// reports it too. The item still installs.
// spec: WF-61 WF-29
#[test]
fn an_unmanaged_workflow_is_a_collision_claimant() {
    let sb = Sandbox::new();
    write(
        &sb.claude_home.join("workflows/hand.js"),
        "export const meta = { name: 'deploy', description: 'Hand-written deploy' }\n",
    );
    sb.write_and_commit(
        "workflows/deploy.js",
        "export const meta = { name: 'deploy', description: 'Deploy it' }\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "workflow:deploy", "--yes"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("deploy.js").symlink_metadata().is_ok(),
        "the item still installs"
    );
    let warnings: Vec<&str> = r
        .stderr
        .lines()
        .filter(|l| l.contains("workflow:hand (unmanaged)"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "exactly one collision warning names the unmanaged claimant: {}",
        r.stderr
    );
    assert!(
        warnings[0].starts_with("warning: workflow:deploy:")
            && warnings[0].contains("harness name 'deploy'"),
        "the installed item is the subject: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("warning: workflow:hand"),
        "an unmanaged claimant is never the subject: {}",
        r.stderr
    );

    let text = sb.mind(&["recall", "workflow:deploy"]);
    assert!(text.success, "recall: {}\n{}", text.stdout, text.stderr);
    assert!(
        text.stdout.contains("workflow:hand (unmanaged)"),
        "recall <item> must report the unmanaged claimant: {}",
        text.stdout
    );
    let json = sb.mind(&["recall", "workflow:deploy", "--json"]);
    assert!(
        json.success,
        "recall --json: {}\n{}",
        json.stdout, json.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(json.stdout.trim()).expect("one document");
    let findings = doc["workflow_findings"].as_array().expect("findings array");
    assert!(
        findings.iter().any(|f| f
            .as_str()
            .is_some_and(|s| s.contains("workflow:hand (unmanaged)"))),
        "the --json findings carry it too: {doc}"
    );
}

/// `--json` carries a workflow's `description` unchanged and its `whenToUse`
/// as a separate `when_to_use` key, in `probe --json` and `recall --json`
/// (list and item). Other kinds never carry the key. The joined form stays
/// human output (`probe --no-tui`).
// spec: WF-62 WF-51
#[test]
fn json_carries_when_to_use_separately_from_the_description() {
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["probe", "--no-tui", "--json"]);
    assert!(r.success, "probe --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one document");
    let rows = doc["items"].as_array().expect("items array");
    let wf = rows
        .iter()
        .find(|row| row["kind"] == "workflow" && row["name"] == "review-changes")
        .unwrap_or_else(|| panic!("a workflow row: {doc}"));
    assert_eq!(wf["description"], "Review changed files", "{wf}");
    assert_eq!(wf["when_to_use"], "before opening a PR", "{wf}");
    let skill = rows
        .iter()
        .find(|row| row["kind"] == "skill" && row["name"] == "review")
        .unwrap_or_else(|| panic!("a skill row: {doc}"));
    assert!(
        skill.get("when_to_use").is_none(),
        "a skill has no key: {skill}"
    );

    let human = sb.mind(&["probe", "--no-tui"]);
    assert!(
        human
            .stdout
            .contains("Review changed files - before opening a PR"),
        "human output keeps the join: {}",
        human.stdout
    );

    assert!(
        sb.mind(&["learn", "workflow:review-changes", "--yes"])
            .success
    );
    assert!(sb.mind(&["learn", "skill:review", "--yes"]).success);

    let r = sb.mind(&["recall", "workflow:review-changes", "--json"]);
    assert!(r.success, "recall item --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one document");
    assert_eq!(doc["description"], "Review changed files", "{doc}");
    assert_eq!(doc["when_to_use"], "before opening a PR", "{doc}");
    let r = sb.mind(&["recall", "skill:review", "--json"]);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one document");
    assert!(
        doc.get("when_to_use").is_none(),
        "a skill has no key: {doc}"
    );

    let r = sb.mind(&["recall", "--json"]);
    assert!(r.success, "recall --json: {}\n{}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).expect("one document");
    let rows: Vec<&serde_json::Value> = doc["items"]
        .as_array()
        .expect("sources array")
        .iter()
        .flat_map(|s| s["items"].as_array().expect("item rows").iter())
        .collect();
    let wf = rows
        .iter()
        .find(|row| row["key"] == "workflow:review-changes")
        .unwrap_or_else(|| panic!("a workflow row: {doc}"));
    assert_eq!(wf["when_to_use"], "before opening a PR", "{wf}");
    let skill = rows
        .iter()
        .find(|row| row["key"] == "skill:review")
        .unwrap_or_else(|| panic!("a skill row: {doc}"));
    assert!(
        skill.get("when_to_use").is_none(),
        "a skill has no key: {skill}"
    );
}

/// `recall <item>` reports why the harness would not load an installed
/// workflow, the same WF-30 reasons `learn` warns with, in the text view and
/// the `--json` findings: a `meta` missing its description, and an empty store
/// copy (which mind reads fine and finds no `meta` in). A store copy mind
/// cannot read at all stays finding-free (see
/// `recall_of_a_workflow_whose_store_copy_is_unreadable_reports_no_harness_name`).
// spec: WF-30 WF-60
#[test]
fn recall_reports_why_the_harness_will_not_load_an_installed_workflow() {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "workflows/bare.js",
        "export const meta = { name: 'bare' }\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "workflow:bare", "--yes"]).success);

    let text = sb.mind(&["recall", "workflow:bare"]);
    assert!(text.success, "recall: {}\n{}", text.stdout, text.stderr);
    assert!(
        text.stdout.lines().any(|l| l.contains("harness")
            && l.contains("the harness will not load this workflow")
            && l.contains("`meta.description` is missing")),
        "the text view must print the unloadable reason: {}",
        text.stdout
    );
    let json = sb.mind(&["recall", "workflow:bare", "--json"]);
    let doc: serde_json::Value = serde_json::from_str(json.stdout.trim()).expect("one document");
    let findings: Vec<&str> = doc["workflow_findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert!(
        findings
            .iter()
            .any(|f| f.contains("`meta.description` is missing")),
        "the --json findings carry it: {doc}"
    );

    // An empty store copy is readable and declares nothing.
    assert!(
        sb.mind(&["learn", "workflow:review-changes", "--yes"])
            .success
    );
    std::fs::write(sb.mind_home.join("store/workflow/review-changes"), "").unwrap();
    let json = sb.mind(&["recall", "workflow:review-changes", "--json"]);
    assert!(
        json.success,
        "recall --json: {}\n{}",
        json.stdout, json.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(json.stdout.trim()).expect("one document");
    assert!(doc["harness_name"].is_null(), "{doc}");
    assert!(
        doc["workflow_findings"]
            .as_array()
            .expect("findings array")
            .iter()
            .any(|f| f
                .as_str()
                .is_some_and(|s| s.contains("mind read no `name`"))),
        "an empty store copy is a WF-30 finding: {doc}"
    );
}
