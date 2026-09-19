//! Integration tests for `mind review`'s workflow reporting.
//!
//! Covers:
//!   - WF-53: every workflow item draws a `workflow-content` disclosure
//!   - WF-54: a workflow never draws the generic `missing-description` finding
//!   - NS-42: a `{{ns:}}` in `meta.name` naming a sibling AGENT is predicted
//!     bare, the way install expands it, so the WF-24 divergence and the WF-29
//!     collision report the name the harness will really answer to
//!
//! Each test drives the real `mind` binary against a hermetic fixture source
//! directory (local path, no network), using isolated MIND_HOME / CLAUDE_HOME
//! temp dirs, exactly as tests/review_hooks.rs does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Minimal fixture harness (mirrors tests/review_hooks.rs)
// ---------------------------------------------------------------------------

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
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-rw-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
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

    fn review(&self) -> Run {
        let target = self.source.to_string_lossy().into_owned();
        self.mind(&["review", &target])
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

/// Every advisory line carrying `tag`.
fn findings<'a>(stdout: &'a str, tag: &str) -> Vec<&'a str> {
    let needle = format!("advisory [{tag}]:");
    stdout
        .lines()
        .filter(|l| l.contains(&needle))
        .collect::<Vec<_>>()
}

// ---------------------------------------------------------------------------
// WF-53: the workflow payload disclosure
// ---------------------------------------------------------------------------

/// Every workflow item draws exactly one `workflow-content` advisory, and no
/// other kind draws one. The finding is unconditional (nothing in these files
/// is suspicious) and advisory (review still exits 0).
// spec: WF-53
#[test]
fn every_workflow_draws_exactly_one_workflow_content_advisory() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("workflows/deploy.js"),
        "export const meta = { name: 'deploy', description: 'Deploy it' }\n\
         export default async function () {}\n",
    );
    write(
        &sb.source.join("workflows/audit.js"),
        "export const meta = { name: 'audit', description: 'Audit it' }\n\
         export default async function () {}\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );

    let r = sb.review();
    assert!(
        r.success,
        "workflow-content is advisory; review must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    let lines = findings(&r.stdout, "workflow-content");
    assert_eq!(
        lines.len(),
        2,
        "one disclosure per workflow, none for the agent: {}",
        r.stdout
    );
    assert!(
        lines.iter().any(|l| l.contains("workflow:deploy:")),
        "must name the deploy workflow: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("workflow:audit:")),
        "must name the audit workflow: {lines:?}"
    );
    // The disclosure says what the file IS and what mind does not do with it.
    for line in &lines {
        assert!(
            line.contains("JavaScript") && line.contains("harness"),
            "must disclose that the harness evaluates the file: {line}"
        );
        assert!(
            line.contains("neither reads nor validates"),
            "must disclose that mind does not read the body: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// WF-54: no generic missing-description for a workflow
// ---------------------------------------------------------------------------

/// A workflow whose `meta` has no `description` draws `workflow-unloadable` and
/// NOT `missing-description`: one defect, one report. The generic finding still
/// fires for a kind that does have frontmatter.
// spec: WF-54
#[test]
fn a_workflow_with_no_meta_description_draws_only_workflow_unloadable() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("workflows/deploy.js"),
        "export const meta = { name: 'deploy' }\nexport default async function () {}\n",
    );
    // A rule with no frontmatter description: the generic check must still fire
    // here, so the workflow's absence is the exemption and not a dead check.
    write(&sb.source.join("rules/style.md"), "# style\n");

    let r = sb.review();
    assert!(
        r.success,
        "both findings are advisory; review must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    let unloadable = findings(&r.stdout, "workflow-unloadable");
    assert_eq!(
        unloadable.len(),
        1,
        "the missing description is reported once, as unloadable: {}",
        r.stdout
    );
    assert!(
        unloadable[0].contains("workflow:deploy:")
            && unloadable[0].contains("`meta.description` is missing"),
        "unloadable finding must name the missing meta key: {}",
        unloadable[0]
    );

    let missing = findings(&r.stdout, "missing-description");
    assert!(
        missing.iter().all(|l| !l.contains("workflow:")),
        "a workflow must never draw missing-description: {missing:?}"
    );
    assert!(
        missing.iter().any(|l| l.contains("rule:style")),
        "the generic check must still fire for a frontmatter kind: {}",
        r.stdout
    );
}

/// The exemption is not "workflows are never checked for a description": a
/// workflow with a complete `meta` draws neither finding.
// spec: WF-54
#[test]
fn a_workflow_with_a_meta_description_draws_neither_description_finding() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("workflows/deploy.js"),
        "export const meta = { name: 'deploy', description: 'Deploy it' }\n",
    );

    let r = sb.review();
    assert!(r.success, "stdout={} stderr={}", r.stdout, r.stderr);
    assert!(
        findings(&r.stdout, "workflow-unloadable").is_empty(),
        "a complete meta is not unloadable: {}",
        r.stdout
    );
    assert!(
        findings(&r.stdout, "missing-description").is_empty(),
        "no generic finding either: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// NS-42: an agent referent in `meta.name` expands bare
// ---------------------------------------------------------------------------

/// Under a prefix, a `{{ns:}}` naming a sibling AGENT expands to the BARE name
/// at install (NS-42), so `review` must predict the bare name too: the WF-24
/// divergence names `dev`, never `jk:dev`, and the workflow collides (WF-29)
/// with the sibling workflow that spells the same name literally.
// spec: NS-42 WF-24 WF-29
#[test]
fn a_meta_name_naming_a_sibling_agent_is_predicted_bare_under_a_prefix() {
    let sb = Sandbox::new("wf");
    write(&sb.source.join("mind.toml"), "[source]\nprefix = \"jk\"\n");
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent\n---\n# dev\n",
    );
    write(
        &sb.source.join("workflows/tokenized.js"),
        "export const meta = { name: '{{ns:dev}}', description: 'Drive the dev agent' }\n",
    );
    write(
        &sb.source.join("workflows/literal.js"),
        "export const meta = { name: 'dev', description: 'Drive the dev agent' }\n",
    );

    let r = sb.review();
    assert!(
        r.success,
        "every workflow finding is advisory: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    let divergences = findings(&r.stdout, "workflow-name");
    let tokenized = divergences
        .iter()
        .find(|l| l.contains("workflow:jk:tokenized:"))
        .unwrap_or_else(|| {
            panic!(
                "expected a divergence for the tokenized workflow: {}",
                r.stdout
            )
        });
    assert!(
        tokenized.contains("resolves it as 'dev'"),
        "the token names an agent, so install writes the bare name: {tokenized}"
    );
    assert!(
        !tokenized.contains("jk:dev"),
        "predicting a prefixed agent name is the NS-42 bug: {tokenized}"
    );

    // Both workflows answer to `dev`, which only holds once the tokenized one
    // is predicted bare.
    let collisions = findings(&r.stdout, "workflow-name-collision");
    assert_eq!(
        collisions.len(),
        2,
        "both claimants are reported: {}",
        r.stdout
    );
    assert!(
        collisions.iter().all(|l| l.contains("harness name 'dev'")),
        "the shared name is the bare agent name: {collisions:?}"
    );
}
