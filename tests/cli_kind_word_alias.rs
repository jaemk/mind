//! NS-74: a kind-qualified ref with no source selector is ALSO read as the
//! effective name `<kind>:<name>` (a legacy prefix equal to a kind word,
//! DSC-112). When both readings match different items, single-item
//! resolution is ambiguous; when only the effective-name reading matches, it
//! resolves to that item.
//!
//! Drives the real `mind` binary against hermetic local git repos with
//! `MIND_HOME`/`CLAUDE_HOME` in a temp dir. No network.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    skill_src: PathBuf,
    workflow_src: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    /// Source `skills-src` holds skill `review`; source `wf-src` holds workflow
    /// `review`. Both committed, nothing melded yet.
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-kind-alias-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sb = Sandbox {
            skill_src: base.join("skills-src"),
            workflow_src: base.join("wf-src"),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        };
        write(
            &sb.skill_src.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\n",
        );
        write(
            &sb.workflow_src.join("workflows/review.js"),
            "export const meta = { name: 'review', description: 'Review workflow' };\n",
        );
        for repo in [&sb.skill_src, &sb.workflow_src] {
            git(repo, &["-c", "init.defaultBranch=main", "init", "-q"]);
            git(repo, &["config", "user.email", "t@t"]);
            git(repo, &["config", "user.name", "t"]);
            git(repo, &["add", "-A"]);
            git(repo, &["commit", "-qm", "initial"]);
        }
        std::fs::create_dir_all(&sb.mind_home).unwrap();
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_POLICY_FILE")
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

    fn spec(path: &Path) -> String {
        path.to_string_lossy().into_owned()
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

/// Meld the skill source under `acme`, rewrite the recorded prefix to the
/// (now reserved) `workflow`, and install its skill as `skill:workflow:review`;
/// then meld the workflow source and install `workflow:review` from it.
fn setup_both_readings(sb: &Sandbox) {
    let skill = Sandbox::spec(&sb.skill_src);
    let meld = sb.mind(&[
        "meld",
        &skill,
        "--namespace",
        "acme",
        "--register-only",
        "--yes",
    ]);
    assert!(
        meld.success,
        "setup meld A: {} {}",
        meld.stdout, meld.stderr
    );

    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).expect("read sources.json");
    let recorded = "\"alias\": \"acme\"";
    assert!(text.contains(recorded), "{text}");
    std::fs::write(&registry, text.replace(recorded, "\"alias\": \"workflow\"")).unwrap();

    let learn_a = sb.mind(&["learn", "skill:workflow:review", "--yes"]);
    assert!(
        learn_a.success,
        "learn the aliased skill: {} {}",
        learn_a.stdout, learn_a.stderr
    );

    let wf = Sandbox::spec(&sb.workflow_src);
    let meld_b = sb.mind(&["meld", &wf, "--register-only", "--yes"]);
    assert!(meld_b.success, "setup meld B: {}", meld_b.stderr);
    let learn_b = sb.mind(&["learn", "wf-src#workflow:review", "--yes"]);
    assert!(
        learn_b.success,
        "learn the workflow: {} {}",
        learn_b.stdout, learn_b.stderr
    );
}

// spec: NS-74
#[test]
fn a_kind_word_ref_matching_two_items_is_ambiguous_and_the_full_key_is_not() {
    let sb = Sandbox::new();
    setup_both_readings(&sb);

    let forget = sb.mind(&["forget", "workflow:review", "--yes"]);
    assert!(
        !forget.success,
        "both readings match: forget must refuse: {} {}",
        forget.stdout, forget.stderr
    );
    assert!(
        forget.stderr.contains("workflow:review") && forget.stderr.contains("skills-src"),
        "the ambiguity must name the aliased skill: {}",
        forget.stderr
    );
    assert!(
        forget.stderr.contains("wf-src"),
        "the ambiguity must name the workflow: {}",
        forget.stderr
    );
    assert!(
        sb.claude_home.join("workflows/review.js").exists(),
        "the workflow must remain linked"
    );
    assert!(
        sb.claude_home.join("skills/workflow:review").exists(),
        "the skill must remain linked"
    );

    // The unambiguous full key removes only the skill.
    let forget_skill = sb.mind(&["forget", "skill:workflow:review", "--yes"]);
    assert!(
        forget_skill.success,
        "the kind-qualified full key is unambiguous: {} {}",
        forget_skill.stdout, forget_skill.stderr
    );
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/workflow:review")).is_err(),
        "the skill link must be gone"
    );
    assert!(
        sb.claude_home.join("workflows/review.js").exists(),
        "the workflow must be untouched"
    );
}

// spec: NS-74
#[test]
fn a_kind_word_ref_matching_only_the_effective_name_resolves_to_it() {
    let sb = Sandbox::new();
    let skill = Sandbox::spec(&sb.skill_src);
    let meld = sb.mind(&[
        "meld",
        &skill,
        "--namespace",
        "acme",
        "--register-only",
        "--yes",
    ]);
    assert!(meld.success, "{}", meld.stderr);
    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).unwrap();
    std::fs::write(
        &registry,
        text.replace("\"alias\": \"acme\"", "\"alias\": \"workflow\""),
    )
    .unwrap();
    let learn = sb.mind(&["learn", "skill:workflow:review", "--yes"]);
    assert!(learn.success, "{} {}", learn.stdout, learn.stderr);

    // `workflow:review` names no workflow here; only the effective-name
    // reading (the aliased skill) matches, so recall resolves to it.
    let recall = sb.mind(&["recall", "workflow:review"]);
    assert!(
        recall.success,
        "the effective-name reading must resolve: {} {}",
        recall.stdout, recall.stderr
    );
    assert!(
        recall.stdout.contains("workflow:review"),
        "recall must show the aliased skill: {}",
        recall.stdout
    );
}

/// Meld both sources (skill source under the rewritten `workflow` alias) but
/// install nothing.
fn meld_both_readings(sb: &Sandbox) {
    meld_aliased_skill_source(sb);
    let wf = Sandbox::spec(&sb.workflow_src);
    let meld_b = sb.mind(&["meld", &wf, "--register-only", "--yes"]);
    assert!(meld_b.success, "meld B: {}", meld_b.stderr);
}

fn meld_aliased_skill_source(sb: &Sandbox) {
    let skill = Sandbox::spec(&sb.skill_src);
    let meld = sb.mind(&[
        "meld",
        &skill,
        "--namespace",
        "acme",
        "--register-only",
        "--yes",
    ]);
    assert!(meld.success, "meld A: {}", meld.stderr);
    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).unwrap();
    std::fs::write(
        &registry,
        text.replace("\"alias\": \"acme\"", "\"alias\": \"workflow\""),
    )
    .unwrap();
}

fn linked(sb: &Sandbox, rel: &str) -> bool {
    std::fs::symlink_metadata(sb.claude_home.join(rel)).is_ok()
}

// spec: NS-74
#[test]
fn learn_of_a_kind_word_ref_matching_two_catalog_items_is_ambiguous() {
    let sb = Sandbox::new();
    meld_both_readings(&sb);

    let r = sb.mind(&["learn", "workflow:review", "--yes"]);
    assert!(!r.success, "both readings match: {} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("skills-src") && r.stderr.contains("wf-src"),
        "the ambiguity must name both candidates: {}",
        r.stderr
    );
    assert!(
        !linked(&sb, "workflows/review.js") && !linked(&sb, "skills/workflow:review"),
        "nothing may install on an ambiguous learn"
    );
}

// spec: NS-74
#[test]
fn a_source_selector_or_the_full_key_disambiguates_learn() {
    let sb = Sandbox::new();
    meld_both_readings(&sb);

    // The source selector suppresses the alternate reading: only the workflow.
    let wf = sb.mind(&["learn", "wf-src#workflow:review", "--yes"]);
    assert!(wf.success, "{} {}", wf.stdout, wf.stderr);
    assert!(linked(&sb, "workflows/review.js"));
    assert!(
        !linked(&sb, "skills/workflow:review"),
        "the selector must not also install the aliased skill"
    );

    // The full key names the aliased skill alone.
    let skill = sb.mind(&["learn", "skill:workflow:review", "--yes"]);
    assert!(skill.success, "{} {}", skill.stdout, skill.stderr);
    assert!(linked(&sb, "skills/workflow:review"));
}

// spec: NS-74
#[test]
fn learn_resolves_to_the_aliased_skill_when_no_workflow_shares_the_name() {
    let sb = Sandbox::new();
    meld_aliased_skill_source(&sb);

    let r = sb.mind(&["learn", "workflow:review", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(linked(&sb, "skills/workflow:review"));
}

// spec: NS-74
#[test]
fn recall_of_an_ambiguous_kind_word_ref_names_both_and_full_key_resolves() {
    let sb = Sandbox::new();
    setup_both_readings(&sb);

    let r = sb.mind(&["recall", "workflow:review"]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("skills-src") && r.stderr.contains("wf-src"),
        "{}",
        r.stderr
    );

    let scoped = sb.mind(&["recall", "wf-src#workflow:review"]);
    assert!(scoped.success, "{} {}", scoped.stdout, scoped.stderr);

    let full = sb.mind(&["recall", "skill:workflow:review"]);
    assert!(full.success, "{} {}", full.stdout, full.stderr);

    // `--tree <item>` goes through the same resolver.
    let tree = sb.mind(&["recall", "--tree", "workflow:review"]);
    assert!(!tree.success, "{} {}", tree.stdout, tree.stderr);
    assert!(tree.stderr.contains("wf-src"), "{}", tree.stderr);
}

// spec: NS-74
#[test]
fn forget_glob_refs_stay_kind_qualified_and_never_sweep_the_aliased_skill() {
    let sb = Sandbox::new();
    setup_both_readings(&sb);

    // Globs are multi-selectors and keep the kind filter: `workflow:rev*` is a
    // workflow glob, not the skill whose effective name starts with `workflow:`.
    let r = sb.mind(&["forget", "workflow:rev*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!linked(&sb, "workflows/review.js"), "the workflow is gone");
    assert!(
        linked(&sb, "skills/workflow:review"),
        "the aliased skill must survive a kind-qualified glob"
    );
}

// spec: NS-74
#[test]
fn upgrade_of_a_kind_word_ref_is_a_kind_filter_and_leaves_the_aliased_skill() {
    let sb = Sandbox::new();
    setup_both_readings(&sb);

    write(
        &sb.skill_src.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill v2\n",
    );
    write(
        &sb.workflow_src.join("workflows/review.js"),
        "export const meta = { name: 'review', description: 'Review workflow v2' };\n",
    );
    for repo in [&sb.skill_src, &sb.workflow_src] {
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", "v2"]);
    }

    // `upgrade` takes a glob-capable filter, not the single-item resolver, so a
    // kind-qualified ref stays a kind filter and is not ambiguous.
    let r = sb.mind(&["upgrade", "workflow:review", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    let wf_store =
        std::fs::read_to_string(sb.mind_home.join("store/workflow/review")).expect("wf store");
    assert!(
        wf_store.contains("v2"),
        "the workflow must upgrade (guards a vacuous pass): {wf_store}"
    );

    let skill_store = sb.mind_home.join("store/skill/workflow:review/SKILL.md");
    let after = std::fs::read_to_string(&skill_store).expect("skill store copy");
    assert!(
        !after.contains("v2"),
        "the aliased skill must not be swept by `upgrade workflow:review`: {after}"
    );
}
