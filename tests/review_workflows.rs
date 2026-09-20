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

    /// `review <target>` with extra flags (`--json`, `--fix`).
    fn review_with(&self, extra: &[&str]) -> Run {
        let target = self.source.to_string_lossy().into_owned();
        let mut args = vec!["review", target.as_str()];
        args.extend_from_slice(extra);
        self.mind(&args)
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

fn write_bytes(path: &Path, contents: &[u8]) {
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

// ---------------------------------------------------------------------------
// WF-53 under `--json` (CLI-218/CLI-219): the finding has a machine shape too
// ---------------------------------------------------------------------------

/// The `workflow-content` disclosure is a first-class finding, so a `--json`
/// consumer sees it as one: `kind: "workflow-content"` in the `advisory`
/// array, one per workflow, with the item key in its message. The text-mode
/// tests above assert on a rendered line, which would still pass if the
/// finding were printed ad hoc instead of pushed onto `advisory`.
// spec: WF-53 CLI-218 CLI-219
#[test]
fn the_workflow_content_finding_has_a_json_shape() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("workflows/deploy.js"),
        "export const meta = { name: 'deploy', description: 'Deploy it' }\n",
    );
    write(
        &sb.source.join("skills/widget/SKILL.md"),
        "---\ndescription: a widget\n---\n# widget\n",
    );

    let r = sb.review_with(&["--json"]);
    assert!(
        r.success,
        "an advisory-only review exits 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document: {e}\n{}", r.stdout));
    assert_eq!(v["action"], "review", "{v}");
    // spec: WF-53 -- a source shipping a workflow is never "clean".
    assert_eq!(
        v["outcome"], "advisory",
        "a workflow alone must move the outcome off `clean`: {v}"
    );
    assert_eq!(v["hard"], serde_json::json!([]), "{v}");

    let advisory = v["advisory"]
        .as_array()
        .unwrap_or_else(|| panic!("advisory must be an array: {v}"));
    let disclosures: Vec<&serde_json::Value> = advisory
        .iter()
        .filter(|f| f["kind"] == "workflow-content")
        .collect();
    assert_eq!(
        disclosures.len(),
        1,
        "exactly one disclosure, for the one workflow: {v}"
    );
    let msg = disclosures[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the finding must carry a message: {v}"));
    assert!(
        msg.contains("workflow:deploy"),
        "the message must name the item: {msg}"
    );
    // spec: WF-54 -- and the skill's presence proves the run reached the other
    // checks, which found nothing to say about either item.
    assert!(
        !advisory.iter().any(|f| f["kind"] == "missing-description"
            && f["message"]
                .as_str()
                .is_some_and(|m| m.contains("workflow:"))),
        "no missing-description for a workflow, in JSON either: {v}"
    );
}

// ---------------------------------------------------------------------------
// The disclosure is unconditional: it does not merge with, or defer to, the
// other workflow findings
// ---------------------------------------------------------------------------

/// Three workflows, three different states -- clean, unloadable, and colliding
/// -- draw three `workflow-content` disclosures and one per item, in item
/// order. A disclosure suppressed once another finding fired for the same file
/// would be the easy mistake, and the two-clean-workflows test above cannot
/// catch it.
// spec: WF-53 WF-30
#[test]
fn every_workflow_is_disclosed_whatever_else_was_found_about_it() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("workflows/aclean.js"),
        "export const meta = { name: 'aclean', description: 'Clean' }\n",
    );
    // Unloadable: no description.
    write(
        &sb.source.join("workflows/bbroken.js"),
        "export const meta = { name: 'bbroken' }\n",
    );
    // Colliding: two files claiming `dup`.
    write(
        &sb.source.join("workflows/cdup.js"),
        "export const meta = { name: 'dup', description: 'One' }\n",
    );
    write(
        &sb.source.join("workflows/ddup.js"),
        "export const meta = { name: 'dup', description: 'Two' }\n",
    );

    let r = sb.review();
    assert!(r.success, "stdout={} stderr={}", r.stdout, r.stderr);

    let lines = findings(&r.stdout, "workflow-content");
    assert_eq!(lines.len(), 4, "one disclosure per workflow: {}", r.stdout);
    for key in [
        "workflow:aclean:",
        "workflow:bbroken:",
        "workflow:cdup:",
        "workflow:ddup:",
    ] {
        assert_eq!(
            lines.iter().filter(|l| l.contains(key)).count(),
            1,
            "{key} must be disclosed exactly once: {}",
            r.stdout
        );
    }
    // The other findings are still there, unmerged.
    assert_eq!(
        findings(&r.stdout, "workflow-unloadable").len(),
        1,
        "{}",
        r.stdout
    );
    assert_eq!(
        findings(&r.stdout, "workflow-name-collision").len(),
        2,
        "both claimants: {}",
        r.stdout
    );
    // spec: WF-24 -- `cdup`/`ddup` diverge from their file names as well, and
    // that is a separate finding from the collision.
    let names = findings(&r.stdout, "workflow-name");
    assert_eq!(
        names.len(),
        2,
        "the two `dup` claimants each diverge from their item name: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// A workflow the convention scan would never find
// ---------------------------------------------------------------------------

/// A `[[items]]`-declared workflow at a non-convention path (WF-8) draws the
/// same findings: the checks read `item.path`, so an authoritative `mind.toml`
/// cannot move a workflow out of the reviewer's sight. Its `mind.toml`
/// `description` must NOT suppress the WF-30 report of a missing
/// `meta.description` either: the two describe different things -- what mind
/// displays versus what the harness needs to load the file -- and the harness
/// never reads `mind.toml`.
// spec: WF-8 WF-53 WF-30 WF-54
#[test]
fn a_declared_workflow_at_an_odd_path_is_disclosed_and_checked() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ndescription = \"odd layout\"\n\n\
         [[items]]\nkind = \"workflow\"\nname = \"deploy\"\n\
         path = \"packages/deploy/flow.js\"\n\
         description = \"Deploy, as mind.toml says\"\n",
    );
    write(
        &sb.source.join("packages/deploy/flow.js"),
        "export const meta = { name: 'deploy' }\n",
    );
    // A decoy at the convention path that the authoritative mind.toml excludes:
    // if a check scanned `workflows/` instead of the declared inventory, the
    // counts below would move.
    write(
        &sb.source.join("workflows/decoy.js"),
        "export const meta = { name: 'decoy', description: 'Not in the inventory' }\n",
    );

    let r = sb.review();
    assert!(
        r.success,
        "every workflow finding is advisory: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    let disclosures = findings(&r.stdout, "workflow-content");
    assert_eq!(
        disclosures.len(),
        1,
        "only the declared workflow is an item: {}",
        r.stdout
    );
    assert!(
        disclosures[0].contains("workflow:deploy:"),
        "the declared workflow must be disclosed: {}",
        disclosures[0]
    );

    // spec: WF-30 -- read from the declared path, and not suppressed by the
    // `mind.toml` description.
    let unloadable = findings(&r.stdout, "workflow-unloadable");
    assert_eq!(unloadable.len(), 1, "{}", r.stdout);
    assert!(
        unloadable[0].contains("workflow:deploy:")
            && unloadable[0].contains("`meta.description` is missing"),
        "a mind.toml description must not answer for the harness's: {}",
        unloadable[0]
    );
    // spec: WF-54 -- and the generic check stays off the kind regardless.
    assert!(
        findings(&r.stdout, "missing-description").is_empty(),
        "{}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// Files mind's reader cannot read
// ---------------------------------------------------------------------------

/// A workflow whose bytes are not UTF-8 is still disclosed (WF-53) and is
/// reported as unloadable (WF-30), and neither is a hard failure: mind does not
/// gatekeep workflow content. The disclosure is the load-bearing half -- it
/// comes from a read that can fail, and swallowing the failure silently would
/// hide exactly the file a reviewer most wants flagged.
// spec: WF-53 WF-30 WF-31
#[test]
fn a_non_utf8_workflow_is_still_disclosed_and_reported_unloadable() {
    let sb = Sandbox::new("wf");
    // Valid JS shape, invalid UTF-8 in the middle (a lone 0x80 continuation).
    let mut bytes: Vec<u8> = b"export const meta = { name: 'deploy', description: '".to_vec();
    bytes.extend_from_slice(&[0x80, 0xff, 0xfe]);
    bytes.extend_from_slice(b"' }\n");
    write_bytes(&sb.source.join("workflows/deploy.js"), &bytes);

    let r = sb.review();
    assert!(
        r.success,
        "an unreadable workflow body is advisory, never hard: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let disclosures = findings(&r.stdout, "workflow-content");
    assert_eq!(
        disclosures.len(),
        1,
        "a file mind cannot decode is still JavaScript the harness will run: {}",
        r.stdout
    );
    assert!(
        disclosures[0].contains("workflow:deploy:"),
        "{}",
        disclosures[0]
    );
    let unloadable = findings(&r.stdout, "workflow-unloadable");
    assert_eq!(unloadable.len(), 1, "{}", r.stdout);
    assert!(
        unloadable[0].contains("it declares no `meta` object mind can read"),
        "an undecodable file reads as no readable meta: {}",
        unloadable[0]
    );
    assert!(
        !r.stderr.contains("error ["),
        "nothing about a workflow's body is a hard finding: {}",
        r.stderr
    );
}

/// A workflow over the harness's 524288-byte cap (WF-7) is reported as
/// unloadable and nothing else changes: mind reads its `meta` fine, discloses
/// it, and exits 0. The cap is the harness's, and mind reports it rather than
/// enforcing a cap of its own (WF-32).
// spec: WF-7 WF-30 WF-32
#[test]
fn an_over_cap_workflow_is_reported_and_review_still_exits_zero() {
    let sb = Sandbox::new("wf");
    let mut body = String::from("export const meta = { name: 'big', description: 'Big' }\n// ");
    body.push_str(&"p".repeat(524_288));
    body.push('\n');
    write(&sb.source.join("workflows/big.js"), &body);

    let r = sb.review();
    assert!(
        r.success,
        "the harness's cap is reported, not enforced: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let unloadable = findings(&r.stdout, "workflow-unloadable");
    assert_eq!(
        unloadable.len(),
        1,
        "the overage is the only complaint -- the `meta` itself is complete: {}",
        r.stdout
    );
    assert!(
        unloadable[0].contains("over the harness's 524288-byte cap"),
        "{}",
        unloadable[0]
    );
    assert_eq!(
        findings(&r.stdout, "workflow-content").len(),
        1,
        "an over-cap file is still disclosed: {}",
        r.stdout
    );
    assert!(
        !r.stderr.contains("metadata-too-large"),
        "the harness cap is far below mind's metadata cap; this file trips only \
         the former: {}",
        r.stderr
    );
}

/// A workflow past mind's own metadata read cap (DSC-91, 8 MiB) reads as NO
/// readable `meta` instead of failing the scan (WF-55). The scan is what the
/// cap used to take down: `catalog.rs` reads every item's metadata through one
/// capped read, and for a workflow that text IS the whole `.js`, so an oversized
/// one aborted the scan of the SOURCE before any item existed.
///
/// So the assertions come in two halves: the healthy sibling is scanned,
/// disclosed, and checked as if the oversized file were not there, and the
/// oversized file is reported through the ordinary WF-30 path (no readable
/// `meta`, plus the WF-7 overage) with no hard finding anywhere.
// spec: WF-55 WF-30 WF-32 WF-53 DSC-91
#[test]
fn a_workflow_past_minds_own_read_cap_reads_as_no_meta_and_spares_the_scan() {
    let sb = Sandbox::new("wf");
    // A perfectly ordinary second workflow: it is the blast radius the old
    // scan failure took with it.
    write(
        &sb.source.join("workflows/fine.js"),
        "export const meta = { name: 'fine', description: 'Fine' }\n",
    );
    let path = sb.source.join("workflows/huge.js");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Sparse: the test must not allocate 8 MiB of its own.
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(8 * 1024 * 1024 + 1).unwrap();
    drop(f);

    let r = sb.review();
    assert!(
        r.success,
        "an over-cap workflow is advisory, not a hard finding: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("error ["),
        "nothing here is hard -- no scan-error, no metadata-too-large: {}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("metadata-too-large") && !r.stderr.contains("metadata-too-large"),
        "the cap is mind's own read bound, not a finding about the source: {} {}",
        r.stdout,
        r.stderr
    );

    // Half one: the healthy sibling survived. Both workflows are disclosed...
    let disclosures = findings(&r.stdout, "workflow-content");
    assert_eq!(
        disclosures.len(),
        2,
        "both workflows are scanned and disclosed: {}",
        r.stdout
    );
    assert!(
        disclosures.iter().any(|d| d.contains("workflow:fine")),
        "the healthy sibling is disclosed: {}",
        r.stdout
    );

    // Half two: ...and only the oversized one is reported unloadable, for the
    // two reasons mind can see -- it read no `meta`, and the file is over the
    // harness's own cap (WF-7).
    let unloadable = findings(&r.stdout, "workflow-unloadable");
    assert!(
        unloadable.iter().all(|u| u.contains("workflow:huge")),
        "the healthy sibling draws no unloadable finding: {}",
        r.stdout
    );
    assert!(
        unloadable
            .iter()
            .any(|u| u.contains("it declares no `meta` object mind can read")),
        "an over-cap file is the WF-5 'yields nothing' case: {}",
        r.stdout
    );
    assert!(
        unloadable
            .iter()
            .any(|u| u.contains("over the harness's 524288-byte cap")),
        "and the WF-7 overage is reported beside it: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// `--fix` and a workflow file
// ---------------------------------------------------------------------------

/// `--fix` never rewrites a workflow's `.js` (NS-54: markdown only), so a
/// reviewed workflow comes back byte-identical even when the rewrite passes
/// would have something to say about it. The sibling markdown item in the same
/// source IS rewritten, so this is the extension rule and not a `--fix` that
/// did nothing at all.
///
/// Note for the spec's benefit: NS-54 justifies the markdown-only rule with
/// "a token in a non-markdown file never expands", which WF-25 makes untrue
/// for a workflow -- install DOES expand tokens in a `.js`. The behavior pinned
/// here is today's; the rationale is the part that no longer covers this kind.
// spec: NS-54 WF-25
#[test]
fn fix_leaves_a_workflow_js_untouched() {
    let sb = Sandbox::new("wf");
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent\n---\n# dev\n",
    );
    // A bare sibling mention in a workflow body: `templatize` would wrap this
    // as `{{ns:dev}}` in a markdown file.
    let js = "export const meta = { name: 'drive', description: 'Drive dev' }\n\
              const done = await agent('dev', 'Do the work.')\n";
    write(&sb.source.join("workflows/drive.js"), js);
    // The markdown control, carrying the same bare mention.
    let md = "---\ndescription: Calls the dev agent\n---\n# helper\n\nHand off to dev.\n";
    write(&sb.source.join("skills/helper/SKILL.md"), md);

    let r = sb.review_with(&["--fix"]);
    assert!(
        r.success,
        "--fix on a local source must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    let after_js = std::fs::read_to_string(sb.source.join("workflows/drive.js")).unwrap();
    assert_eq!(
        after_js, js,
        "--fix must leave a workflow's .js byte-identical (NS-54)"
    );
    let after_md = std::fs::read_to_string(sb.source.join("skills/helper/SKILL.md")).unwrap();
    assert_ne!(
        after_md, md,
        "the markdown control must have been rewritten, or this test proves \
         only that --fix did nothing: {after_md}"
    );
    // spec: WF-53 -- and the disclosure still fires on a `--fix` run.
    assert_eq!(
        findings(&r.stdout, "workflow-content").len(),
        1,
        "{}",
        r.stdout
    );
}
