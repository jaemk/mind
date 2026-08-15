//! Integration tests for `mind review --fix` and the `{{ns:}}` context
//! classifier (spec/namespacing.md NS-46 through NS-51).
//!
//! `--fix` is the one path that rewrites a user's files, and it un-wraps a token
//! it believes sits in code. These tests drive the real binary against hermetic
//! fixture sources (local path, no network) with isolated MIND_HOME /
//! CLAUDE_HOME temp dirs, and assert both directions: a prose token survives the
//! rewrite byte for byte, and a token genuinely in code is still un-wrapped.
//!
//! The classifier reads the document as CommonMark (`pulldown-cmark`), so each
//! case below is stated as what a renderer does with the document rather than as
//! what the scan's own rules would make of it.

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
        let base = std::env::temp_dir().join(format!("mind-rfn-{}-{n}", std::process::id()));
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

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
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

/// A prefixed source with the two referenced siblings the fixtures point at.
fn fixture() -> Sandbox {
    let sb = Sandbox::new("cos");
    write(&sb.source.join("mind.toml"), "[source]\nprefix = \"cos\"\n");
    write(
        &sb.source.join("skills/cos-spec/SKILL.md"),
        "---\nname: cos-spec\ndescription: EARS patterns\n---\n# spec\n",
    );
    write(
        &sb.source.join("skills/cos-cert-setup/SKILL.md"),
        "---\nname: cos-cert-setup\ndescription: cert setup\n---\n# cert\n",
    );
    sb
}

// ---------------------------------------------------------------------------
// NS-46 / NS-48: a prose token after a multi-line code span survives --fix
// ---------------------------------------------------------------------------

/// The reported repro: an inline code span opened on one line and closed on the
/// next, then a `{{ns:}}` token in the same paragraph. The token is prose, so
/// `--fix` must leave the file byte for byte identical and must not report a
/// misplaced-reference advisory.
/// spec: NS-46 NS-48
#[test]
fn fix_leaves_a_prose_token_after_a_multiline_code_span_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    let original = "---\nname: cos-http\ndescription: http client\n---\n\
                    # HTTP\n\n\
                    Read the token with `gh auth\n\
                    token --hostname github.com` and the CA bundle callback \
                    (via {{ns:cos-cert-setup}}).\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "a prose token must not be flagged misplaced: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must not rewrite a file whose only token is prose"
    );

    // Idempotent: a second --fix still changes nothing.
    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must not re-dirty the file on a second run"
    );
}

/// The other reported repro: a four-backtick fence quoting fence delimiters. A
/// run shorter than the opener is block content, so the outer block ends at its
/// own closer and the prose after it is prose. A toggle that flips on any
/// line-initial triple backtick desynchronizes here (an odd number of flips) and
/// reads the rest of the file as a code block.
/// spec: NS-47 NS-48
#[test]
fn fix_leaves_a_prose_token_after_a_nested_fence_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-handoff/SKILL.md");
    let original = "---\nname: cos-handoff\ndescription: handoff\n---\n\
                    # Handoff\n\n\
                    Wrap the transcript in a fence, like this:\n\n\
                    ````markdown\n\
                    Reply with a fenced block opened by\n\
                    ```\n\
                    and nothing else on the line.\n\
                    ````\n\n\
                    Verify the stated behaviors. See the {{ns:cos-spec}} skill for the \
                    EARS pattern reference and REQ-ID semantics.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "a prose token after a nested fence must not be flagged: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must not rewrite a file whose only token is prose"
    );
}

/// A four-space-indented lone fence delimiter. In CommonMark that is an
/// indented code block showing the delimiter as literal text, so it opens no
/// fence; reading it as an opener leaves a block nothing closes and classifies
/// every token below it as code, which `--fix` then de-tokenizes. Same symptom
/// and same destructive path as the filed report.
/// spec: NS-49
#[test]
fn fix_leaves_a_prose_token_after_an_indented_code_block_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-handoff/SKILL.md");
    let original = "---\nname: cos-handoff\ndescription: handoff\n---\n\
                    # Handoff\n\n\
                    Wrap the reply in a fence, opened by a line holding only:\n\n\
                    \x20   ```\n\n\
                    Verify the stated behaviors. See the {{ns:cos-spec}} skill for the \
                    EARS pattern reference.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "a prose token after an indented code block must not be flagged: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must leave the file byte for byte identical"
    );
}

/// A backslash-escaped backtick is a literal backtick in CommonMark, never a
/// span delimiter. Counting it as one pairs it with the opener of the next real
/// span, so the prose between them reads as code and `--fix` de-tokenizes it.
/// spec: NS-50
#[test]
fn fix_leaves_a_prose_token_after_an_escaped_backtick_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    let original = "---\nname: cos-http\ndescription: http client\n---\n\
                    # HTTP\n\n\
                    Escape a backtick as \\` when quoting it in prose. Then read the \
                    {{ns:cos-cert-setup}} skill and run `mind sync`.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "a prose token after an escaped backtick must not be flagged: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must leave the file byte for byte identical"
    );
}

/// The other side of NS-49: a fence nested in a list item is indented, and is
/// still a fence, so the token inside it is still un-wrapped. This is what rules
/// out closing the indented-code defect by refusing to look past indentation.
/// spec: NS-49
#[test]
fn fix_still_unwraps_a_token_inside_a_fence_nested_in_a_list_item() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\nname: cos-run\ndescription: runner\n---\n\
         1.  Install it:\n\n\
         \x20   ```sh\n\
         \x20   mind learn {{ns:cos-spec}}\n\
         \x20   ```\n\n\
         \x20   Then read the {{ns:cos-cert-setup}} skill.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);

    let fixed = std::fs::read_to_string(&skill).unwrap();
    assert!(
        fixed.contains("    mind learn cos-spec\n"),
        "the token in the nested fence must be un-wrapped: {fixed}"
    );
    assert!(
        fixed.contains("    Then read the {{ns:cos-cert-setup}} skill."),
        "prose written at the item's own content column is still prose, so its \
         token must survive: {fixed}"
    );
}

// ---------------------------------------------------------------------------
// NS-46 / NS-47: the true positives still fire
// ---------------------------------------------------------------------------

/// A token genuinely inside a fenced block, inside a code span that wraps across
/// a line break, and adjacent to a path is still misplaced: `--fix` un-wraps all
/// three. The prose token in the same file is left alone.
/// spec: NS-46 NS-47
#[test]
fn fix_still_unwraps_tokens_that_really_are_in_code() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    ```sh\n\
                    mind learn {{ns:cos-spec}}\n\
                    ```\n\n\
                    Run `mind learn\n\
                    {{ns:cos-spec}}` from ~/{{ns:cos-spec}} first.\n\n\
                    Then read the {{ns:cos-spec}} skill.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);

    let fixed = std::fs::read_to_string(&skill).unwrap();
    assert!(
        fixed.contains("mind learn cos-spec\n```"),
        "the fenced token must be un-wrapped: {fixed}"
    );
    assert!(
        fixed.contains("Run `mind learn\ncos-spec` from ~/cos-spec first."),
        "the multi-line span and path tokens must be un-wrapped: {fixed}"
    );
    assert!(
        fixed.contains("Then read the {{ns:cos-spec}} skill."),
        "the prose token must survive: {fixed}"
    );
}

// ---------------------------------------------------------------------------
// NS-48: the class-level guard over the fixed corpus
// ---------------------------------------------------------------------------

/// After `--fix`, no item file carries a bare sibling name in prose: whatever
/// `--fix` un-wraps or leaves bare must not be something mind's own
/// unguarded-reference check would then flag. A bare prose mention after a
/// multi-line code span is wrapped rather than left bare.
/// spec: NS-48
#[test]
fn fix_leaves_no_unguarded_reference_in_prose() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    write(
        &skill,
        "---\nname: cos-http\ndescription: http client\n---\n\
         Read the token with `gh auth\n\
         token --hostname github.com` then see the cos-spec skill.\n\n\
         Set up certs with {{ns:cos-cert-setup}}.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);

    let fixed = std::fs::read_to_string(&skill).unwrap();
    assert!(
        fixed.contains("see the {{ns:cos-spec}} skill"),
        "the bare prose mention must be templatized, not left unguarded: {fixed}"
    );
    assert!(
        fixed.contains("{{ns:cos-cert-setup}}"),
        "the prose token must survive: {fixed}"
    );
    // The guard: re-reviewing the fixed source reports no unguarded reference
    // and no misplaced reference, so the fix is a fixed point.
    let r2 = sb.mind(&["review", &target]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert!(
        !r2.stdout.contains("misplaced-reference"),
        "the fixed source must be clean: {}",
        r2.stdout
    );
}

// ---------------------------------------------------------------------------
// Idempotence of the rewrite itself, in both directions
// ---------------------------------------------------------------------------

/// `--fix` must be a fixed point on a file whose tokens are all in code, not
/// only on the prose case. Un-wrapping leaves bare sibling names sitting inside
/// a fence, inside a code span, and after a `~/`; if wrapping disagreed with
/// un-wrapping about any of those, the second run would put the tokens back and
/// the file would flip forever. The second run must change nothing and must not
/// report the file as fixed.
/// spec: NS-46 NS-47
#[test]
fn fix_is_idempotent_on_a_file_whose_tokens_are_all_in_code() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\nname: cos-run\ndescription: runner\n---\n\
         ````md\n\
         ```sh\n\
         mind learn {{ns:cos-spec}}\n\
         ```\n\
         ````\n\n\
         Run `mind learn\n\
         {{ns:cos-spec}}` from ~/{{ns:cos-spec}} first.\n\n\
         Then read the {{ns:cos-spec}} skill.\n",
    );

    let target = sb.source_spec();
    let r1 = sb.mind(&["review", &target, "--fix"]);
    assert!(r1.success, "{} {}", r1.stdout, r1.stderr);
    assert!(
        r1.stdout.contains("fixed"),
        "the first run rewrites the file: {}",
        r1.stdout
    );
    let after_first = std::fs::read_to_string(&skill).unwrap();

    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        after_first,
        "--fix must not re-dirty a file whose bare names all sit in code"
    );
    assert!(
        !r2.stdout.contains("fixed"),
        "the second run must report nothing to fix: {}",
        r2.stdout
    );
}

/// The other direction: a run that really does rewrite (wrapping a bare prose
/// mention) must also settle after one pass.
/// spec: NS-46 NS-48
#[test]
fn fix_settles_after_one_pass_when_it_wraps_a_bare_prose_mention() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    write(
        &skill,
        "---\nname: cos-http\ndescription: http client\n---\n\
         Read it with `gh auth\n\
         token --hostname github.com` then see the cos-spec skill.\n",
    );

    let target = sb.source_spec();
    let r1 = sb.mind(&["review", &target, "--fix"]);
    assert!(r1.success, "{} {}", r1.stdout, r1.stderr);
    let after_first = std::fs::read_to_string(&skill).unwrap();
    assert!(
        after_first.contains("see the {{ns:cos-spec}} skill"),
        "the bare mention after a multi-line span must be wrapped: {after_first}"
    );

    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        after_first,
        "--fix must settle after one pass"
    );
    assert!(
        !r2.stdout.contains("fixed"),
        "the second run must report nothing to fix: {}",
        r2.stdout
    );
}

/// The third fence shape that desynchronized the old toggle: a tilde fence
/// quoting a backtick delimiter. One line-initial triple backtick is an odd
/// count, so the old toggle read the rest of the file as a code block and
/// `--fix` deleted the prose token; the run-length-and-character rule keeps the
/// inner delimiter as content.
/// spec: NS-47
#[test]
fn fix_survives_a_tilde_fence_quoting_a_backtick_delimiter() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-handoff/SKILL.md");
    let original = "---\nname: cos-handoff\ndescription: handoff\n---\n\
                    Reply with a fenced block opened by:\n\n\
                    ~~~text\n\
                    ```\n\
                    ~~~\n\n\
                    Then see the {{ns:cos-spec}} skill.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "a prose token after a tilde fence must not be flagged: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must leave the file byte for byte identical"
    );
}

// ---------------------------------------------------------------------------
// INIT-5: the other consumer of the same structure map
// ---------------------------------------------------------------------------

/// `init-source --template` wraps through the same `templatize`, so the two
/// structural defects were reachable from it too, in the opposite direction:
/// line-local backtick parity made it *skip* a prose mention that followed a
/// code span closing on a continuation line, and the naive fence toggle made it
/// *wrap* a name inside a nested example fence, corrupting the fenced sample.
/// Both are file rewrites in the author's working tree.
/// spec: INIT-5 NS-46 NS-47
#[test]
fn init_source_template_reads_the_document_structure() {
    let sb = Sandbox::new("kit");
    write(&sb.source.join("mind.toml"), "[source]\nprefix = \"kit\"\n");
    write(
        &sb.source.join("skills/beta/SKILL.md"),
        "---\nname: beta\ndescription: beta\n---\n# beta\n",
    );
    let alpha = sb.source.join("skills/alpha/SKILL.md");
    write(
        &alpha,
        "---\nname: alpha\ndescription: alpha\n---\n\
         # Alpha\n\n\
         Read it with `gh auth\n\
         token --hostname github.com` then hand off to beta.\n\n\
         Quote the reply like this:\n\n\
         ````markdown\n\
         ```\n\
         hand off to beta\n\
         ```\n\
         ````\n\n\
         - run `gh auth\n\
         \x20 token` then beta\n\
         - and `beta` inline\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["init-source", &target, "--template"]);
    assert!(
        r.success,
        "init-source must exit 0: {} {}",
        r.stdout, r.stderr
    );

    let out = std::fs::read_to_string(&alpha).unwrap();
    assert!(
        out.contains("then hand off to {{ns:beta}}."),
        "a prose mention after a code span that closed on the next line must be \
         wrapped: {out}"
    );
    assert!(
        out.contains("```\nhand off to beta\n```\n"),
        "a mention inside a nested example fence must stay bare: {out}"
    );
    assert!(
        out.contains("token` then {{ns:beta}}\n"),
        "a span may cross a line break inside one list item: {out}"
    );
    assert!(
        out.contains("- and `beta` inline\n"),
        "a mention inside a single-line span must stay bare: {out}"
    );
    assert!(
        out.contains("name: alpha"),
        "frontmatter is never rewritten: {out}"
    );

    // Idempotent: a second --template finds nothing left to wrap.
    let r2 = sb.mind(&["init-source", &target, "--template"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert!(
        r2.stdout.contains("no bare references to template"),
        "the second run must find nothing: {}",
        r2.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&alpha).unwrap(),
        out,
        "the second run must not change the file"
    );
}

// ---------------------------------------------------------------------------
// Outside-in: the whole rewrite, pinned byte for byte over a mixed document
// ---------------------------------------------------------------------------

/// One file carrying every structural shape the classifier decides between, with
/// the post-`--fix` bytes written out in full. The per-shape tests assert one
/// property each and would each survive a change that broke a different shape;
/// this one fails on any change to any of them, including one that only moves a
/// byte. It is also the only assertion that covers the three passes composed in
/// their real order (`rewrite_hardcoded_paths`, then un-wrap, then wrap).
/// spec: NS-46 NS-47 NS-49 NS-50
#[test]
fn fix_rewrites_a_mixed_document_exactly() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\nname: cos-run\ndescription: runner\n---\n\
         # Runner\n\n\
         Read it with `gh auth\n\
         token --hostname github.com` and see cos-spec.\n\n\
         ````md\n\
         ```sh\n\
         mind learn cos-spec\n\
         ```\n\
         ````\n\n\
         1.  Install:\n\n\
         \x20   ```sh\n\
         \x20   mind learn {{ns:cos-spec}}\n\
         \x20   ```\n\n\
         \x20   Then see the {{ns:cos-cert-setup}} skill.\n\n\
         Escape a backtick as \\` then see cos-cert-setup.\n\n\
         Show a bare delimiter with:\n\n\
         \x20   ```\n\n\
         Wrap up with {{ns:cos-spec}}, ~/{{ns:cos-spec}}, and `{{ns:cos-spec}}`.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);

    let expected = "---\nname: cos-run\ndescription: runner\n---\n\
         # Runner\n\n\
         Read it with `gh auth\n\
         token --hostname github.com` and see {{ns:cos-spec}}.\n\n\
         ````md\n\
         ```sh\n\
         mind learn cos-spec\n\
         ```\n\
         ````\n\n\
         1.  Install:\n\n\
         \x20   ```sh\n\
         \x20   mind learn cos-spec\n\
         \x20   ```\n\n\
         \x20   Then see the {{ns:cos-cert-setup}} skill.\n\n\
         Escape a backtick as \\` then see {{ns:cos-cert-setup}}.\n\n\
         Show a bare delimiter with:\n\n\
         \x20   ```\n\n\
         Wrap up with {{ns:cos-spec}}, ~/cos-spec, and `cos-spec`.\n";
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), expected);

    // And it settles: a second run finds nothing to change.
    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        expected,
        "--fix must not re-dirty the file"
    );
}

/// CRLF, end to end through the binary. Every line ending has to survive the
/// three passes byte for byte, and the carriage return must not break fence
/// matching (the delimiter run is followed by `\r`, not by end of line) or the
/// span match that crosses the line break.
/// spec: NS-46 NS-47
#[test]
fn fix_preserves_crlf_line_endings() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\r\nname: cos-run\r\ndescription: runner\r\n---\r\n\
         ````md\r\n\
         ```sh\r\n\
         mind learn cos-spec\r\n\
         ```\r\n\
         ````\r\n\r\n\
         Read it with `gh auth\r\n\
         token` then see cos-spec and {{ns:cos-cert-setup}}.\r\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "---\r\nname: cos-run\r\ndescription: runner\r\n---\r\n\
         ````md\r\n\
         ```sh\r\n\
         mind learn cos-spec\r\n\
         ```\r\n\
         ````\r\n\r\n\
         Read it with `gh auth\r\n\
         token` then see {{ns:cos-spec}} and {{ns:cos-cert-setup}}.\r\n",
        "CRLF must round-trip and the fenced sample must stay bare"
    );
}

/// A fence left unclosed inside a list item ends with the item, end to end. An
/// author who forgets the closing delimiter of a nested fence would otherwise
/// hand `--fix` a code block covering the entire rest of the file, and every
/// token below it -- the whole document, in a file where the mistake is near the
/// top -- gets un-wrapped in one run. The dedent bound is what keeps the damage
/// inside the item, so it is asserted through the binary and not only in a unit.
/// spec: NS-49
#[test]
fn fix_confines_an_unclosed_nested_fence_to_its_list_item() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    Steps:\n\n\
                    - Install it:\n\n\
                    \x20 ```sh\n\
                    \x20 mind sync\n\n\
                    Then see the {{ns:cos-cert-setup}} skill, and the cos-spec skill.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original.replace("the cos-spec skill", "the {{ns:cos-spec}} skill"),
        "the dedent ends the item and its fence, so the paragraph below is prose"
    );
}

/// The other half of NS-49, end to end: an indented code block cannot interrupt
/// a paragraph, so an over-indented continuation of a wrapped prose line is
/// prose. Reading it as code is the same destructive rewrite as a false fence
/// opener, pointed the other way -- the token on the continuation line is what
/// `--fix` deletes. Asserted in both directions, since a classifier that called
/// the line code would also decline to wrap the bare mention on it.
/// spec: NS-49
#[test]
fn fix_treats_an_over_indented_continuation_line_as_prose() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    The certificate bundle callback is documented at length in\n\
                    \x20       the {{ns:cos-cert-setup}} skill, which also covers renewal.\n\n\
                    Rotation is covered in\n\
                    \x20       the cos-spec skill instead.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original.replace("the cos-spec skill", "the {{ns:cos-spec}} skill"),
        "the token on the continuation line must survive and the bare mention \
         on the other one must be wrapped"
    );
}

/// A `.markdown` file inside an item is markdown ([`namespace::is_markdown`],
/// NS-53), and `install` expands `{{ns:}}` tokens in every markdown file it
/// copies, so those tokens are live references there too. `--fix` recognizes
/// the extension through the same predicate `install` uses (rather than an
/// exact `.md` test), so it treats the file as prose and leaves the token
/// alone instead of stripping it into a bare name that no longer resolves
/// under a prefix.
/// spec: NS-24 NS-48 NS-53
#[test]
fn fix_keeps_prose_tokens_in_a_markdown_file_that_is_not_dot_md() {
    let sb = fixture();
    let doc = sb.source.join("skills/cos-run/REFERENCE.markdown");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let original = "# Reference\n\nHand off to {{ns:cos-spec}} when done.\n";
    write(&doc, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&doc).unwrap(),
        original,
        "a prose token in a markdown file must survive whatever the extension"
    );
}

/// The companion the test above cannot be: "the file came back unchanged" is
/// also what a file `--fix` skipped entirely looks like (NS-54), so it passes
/// just as well if `is_markdown` stops recognizing the extension. This one
/// distinguishes the two by asking for a rewrite that only happens *inside*
/// markdown: a bare prose mention in a `.markdown` / `.mkd` / `.mdown` /
/// uppercase `.MD` file must be wrapped, while the same text in a file the
/// predicate rejects (`.mdx`, `.txt`, no extension at all) must not be.
/// spec: NS-53 NS-54 NS-24
#[test]
fn fix_wraps_a_bare_mention_in_every_markdown_extension_and_no_other() {
    let sb = fixture();
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let bare = "Hand off to cos-spec when done.\n";
    let wrapped = "Hand off to {{ns:cos-spec}} when done.\n";
    for name in ["A.markdown", "B.mkd", "C.mdown", "D.MD", "E.Markdown"] {
        write(&sb.source.join("skills/cos-run").join(name), bare);
    }
    // Rejected by the predicate: a near-miss extension, an unrelated text
    // extension, a bare extensionless name, and a dotfile whose leading dot is
    // not an extension.
    for name in ["F.mdx", "G.txt", "NOTES", ".mkd"] {
        write(&sb.source.join("skills/cos-run").join(name), bare);
    }

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    for name in ["A.markdown", "B.mkd", "C.mdown", "D.MD", "E.Markdown"] {
        assert_eq!(
            std::fs::read_to_string(sb.source.join("skills/cos-run").join(name)).unwrap(),
            wrapped,
            "{name} is markdown, so its bare mention must be wrapped"
        );
    }
    for name in ["F.mdx", "G.txt", "NOTES", ".mkd"] {
        assert_eq!(
            std::fs::read_to_string(sb.source.join("skills/cos-run").join(name)).unwrap(),
            bare,
            "{name} is not markdown, so --fix must not rewrite it (NS-54)"
        );
    }
}

/// A fenced block may open on the same line as a list marker. The hand-rolled
/// classifier only looked for a delimiter at the start of a line, so it read the
/// opener as prose and the real closer as an opener: the rest of the item became
/// a code block and every token below it was un-wrapped, the filed bug's exact
/// symptom. A CommonMark parse sees the block where the marker puts it.
/// spec: NS-47 NS-49
#[test]
fn fix_leaves_a_prose_token_after_a_marker_line_fence_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    Setup:\n\n\
                    - ```sh\n\
                    \x20 mind sync\n\
                    \x20 ```\n\n\
                    \x20 Then see the {{ns:cos-spec}} skill.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must leave the item's trailing prose token alone"
    );
}

/// A fence inside a blockquote is a fence, end to end. The quoted delimiter used
/// to be invisible, so the whole quoted block read as prose and wrapping
/// rewrote the bare sibling name inside the quoted sample: the sample then reads
/// wrong and expands to a prefixed name at install. Quoted code is code for both
/// passes, so the sample survives while the quoted prose around it is still
/// wrapped.
/// spec: NS-47
#[test]
fn fix_leaves_a_bare_name_inside_a_blockquoted_code_sample_alone() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\nname: cos-run\ndescription: runner\n---\n\
         Quoting the install step:\n\n\
         > ```sh\n\
         > mind learn cos-spec\n\
         > ```\n\
         >\n\
         > Then read the cos-cert-setup skill.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "---\nname: cos-run\ndescription: runner\n---\n\
         Quoting the install step:\n\n\
         > ```sh\n\
         > mind learn cos-spec\n\
         > ```\n\
         >\n\
         > Then read the {{ns:cos-cert-setup}} skill.\n",
        "the quoted code sample keeps its bare name; the quoted prose is wrapped"
    );
}

/// A `{{ns:}}` token written across a line break is a live reference at install
/// time (`expand` reads the file as a whole). The line-by-line wrapper swallowed
/// the opening line looking for `}}`, then wrapped the name on the next line
/// *inside* the token, producing `{{ns:\n{{ns:cos-spec}} }}`, which `install`
/// rejects as a bad reference: the source stops installing entirely. Asserted
/// through both binaries' worth of behavior -- the rewrite, and then a `learn`
/// of the rewritten source.
/// spec: NS-51
#[test]
fn fix_does_not_nest_a_token_inside_one_that_spans_a_line_break() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    Hand off to {{ns:\n\
                    cos-spec }} when the run finishes.\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let fixed = std::fs::read_to_string(&skill).unwrap();
    assert_eq!(
        fixed, original,
        "a token that spans a line break is copied verbatim, not wrapped into"
    );

    // The proof that the corruption mattered: the rewritten source still
    // installs, and the token expands to the prefixed name.
    let m = sb.mind(&["meld", &target, "--yes"]);
    assert!(m.success, "meld must succeed: {} {}", m.stdout, m.stderr);
    let l = sb.mind(&["learn", "cos:cos-run", "--yes"]);
    assert!(l.success, "learn must succeed: {} {}", l.stdout, l.stderr);
    let installed = std::fs::read_to_string(sb.mind_home.join("store/skill/cos:cos-run/SKILL.md"))
        .expect("installed skill");
    assert!(
        installed.contains("cos:cos-spec"),
        "the split token must still expand: {installed}"
    );
}

// ---------------------------------------------------------------------------
// What else the same rewrite pass does to prose it decides is prose
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Outside-in third pass: constructs the parser swap did not exercise
// ---------------------------------------------------------------------------

/// One file carrying the constructs no earlier test drove -- an HTML block, a
/// table, an autolink, a nested blockquote, a heading with a closing hash
/// sequence, a hard line break, and a tight list -- each with a prose token
/// beside it. None of them is code, so `--fix` must return the file byte for
/// byte and must flag nothing. A single misread of any one of them deletes the
/// token next to it, which is the filed bug's exact symptom reached through a
/// construct nobody tested.
/// spec: NS-46 NS-47 NS-48 NS-49
#[test]
fn fix_leaves_prose_tokens_beside_untested_constructs_untouched() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\ndescription: runner\n---\n\
                    ## Runner ##\n\n\
                    <div align=\"center\">\n\
                    Set up with {{ns:cos-cert-setup}}.\n\
                    </div>\n\n\
                    | step | note |\n\
                    |---|---|\n\
                    | one | see {{ns:cos-spec}} |\n\
                    | two | and `mind sync` |\n\n\
                    Docs live at <https://example.com/docs>, see {{ns:cos-spec}}.\n\n\
                    > > ```sh\n\
                    > > mind learn cos-spec\n\
                    > > ```\n\
                    >\n\
                    > Then read {{ns:cos-cert-setup}}.\n\n\
                    A wrapped line\\\n\
                    continues into {{ns:cos-spec}} here.\n\n\
                    - tight {{ns:cos-spec}}\n\
                    - items {{ns:cos-cert-setup}}\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("misplaced-reference"),
        "no token here is misplaced: {}",
        r.stdout
    );
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "--fix must leave the file byte for byte identical"
    );
}

/// The wrapping direction over the same constructs: a bare sibling name inside
/// a blockquoted fence and inside a table cell's code span stays bare, while the
/// mentions in the surrounding prose are wrapped. Pinned byte for byte, because
/// a wrap in a code sample corrupts the sample and then expands to a prefixed
/// name at install.
/// spec: INIT-5 NS-46 NS-47
#[test]
fn fix_wraps_prose_beside_those_constructs_without_touching_their_code() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    write(
        &skill,
        "---\nname: cos-run\ndescription: runner\n---\n\
         | step | note |\n\
         |---|---|\n\
         | one | see cos-spec |\n\
         | two | run `mind learn cos-spec` |\n\n\
         > > ```sh\n\
         > > mind learn cos-spec\n\
         > > ```\n\
         >\n\
         > Then read cos-cert-setup.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "---\nname: cos-run\ndescription: runner\n---\n\
         | step | note |\n\
         |---|---|\n\
         | one | see {{ns:cos-spec}} |\n\
         | two | run `mind learn cos-spec` |\n\n\
         > > ```sh\n\
         > > mind learn cos-spec\n\
         > > ```\n\
         >\n\
         > Then read {{ns:cos-cert-setup}}.\n",
        "the cell's prose and the quoted prose are wrapped; both code samples \
         keep their bare names"
    );
}

/// A non-markdown file's tokens never expand (NS-53), so `--fix` reports them
/// rather than rewriting (NS-54) -- the reverse of the old behavior, which
/// treated the file as all code and silently un-wrapped every token in it. The
/// file must come back byte for byte identical, and the token the structure
/// map can see (the one not sitting on what a CommonMark parse reads as a
/// fence-open line -- a pre-existing, unrelated limitation of that map) is
/// reported with the literal "will not expand" wording rather than the
/// misplaced-in-code wording used inside a markdown file.
/// spec: NS-24 NS-47 NS-53 NS-54
#[test]
fn fix_never_rewrites_a_non_markdown_file() {
    let sb = fixture();
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    let original = "#!/bin/sh\n\
                    # hand off to {{ns:cos-spec}}\n\
                    echo cos-cert-setup\n\
                    cat <<'EOF'\n\
                    ```{{ns:cos-spec}}\n\
                    EOF\n";
    write(&script, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&script).unwrap(),
        original,
        "--fix must never rewrite a non-markdown file (NS-54)"
    );
    assert!(
        r.stdout
            .contains("will not expand here; tokens expand in markdown only"),
        "the token must be reported, not silently fixed: {}",
        r.stdout
    );
}

/// A link reference definition and a reference label are markdown syntax, not
/// prose, but the structure map calls everything that is not a code block or a
/// code span prose, so wrapping rewrites them. The rewritten file no longer
/// resolves the reference and renders a literal `[{{ns:name}}]`, which is a
/// destructive rewrite of the author's working tree.
/// spec: NS-24 NS-52 INIT-5
#[test]
fn fix_leaves_link_reference_syntax_alone() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    let original = "---\nname: cos-http\ndescription: http client\n---\n\
                    Read the [certificate notes][cos-spec] first.\n\n\
                    [cos-spec]: https://example.com/notes\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        original,
        "a link label is syntax, not prose"
    );
}

/// `frontmatter.rs` strips a UTF-8 BOM before its delimiter check (DSC-23), so
/// a BOM-prefixed SKILL.md is a valid item mind reads normally. The `--fix`
/// classifier's own frontmatter pre-pass does not strip it, so the whole file
/// parses as markdown and the frontmatter becomes an ordinary block whose text
/// wrapping rewrites -- including the `name:` field NS-24 names as the one place
/// wrapping must never touch. `description:` is not that field, so its sibling
/// mention IS wrapped (NS-56) -- the BOM strip must still hold up under that.
/// spec: NS-24 NS-47 NS-56
#[test]
fn fix_keeps_the_frontmatter_of_a_bom_prefixed_file() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-spec/SKILL.md");
    let original = "\u{feff}---\nname: cos-spec\ndescription: hands off to cos-cert-setup\n\
                    ---\n# spec\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "\u{feff}---\nname: cos-spec\ndescription: hands off to {{ns:cos-cert-setup}}\n\
         ---\n# spec\n",
        "name: survives the BOM; description: is wrapped"
    );
}

/// NS-56 makes every frontmatter field other than `name:` wrappable. But
/// `description:` is the only frontmatter field that is free prose; the others
/// mind itself reads are *structured* and are parsed from the SOURCE file, not
/// from the expanded store copy:
///
/// * `requires:` is a list of item refs (DEP-4/DEP-5) `catalog.rs` reads from
///   the source frontmatter and `install.rs` validates before staging.
/// * `build:`/`install:` are shell commands run verbatim (HOOK/TOOL) taken from
///   the same source frontmatter.
///
/// Wrapping a sibling name inside any of those writes a token into a field
/// nothing ever expands, which is destruction, not templating: the dependency
/// stops resolving and the hook runs a literal `{{ns:...}}`. `--fix` must leave
/// them alone.
/// spec: NS-56 NS-24 DEP-4
#[test]
fn fix_never_wraps_a_structured_frontmatter_field() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-run/SKILL.md");
    let original = "---\nname: cos-run\n\
                    description: runner\n\
                    requires: skill:cos-spec\n\
                    build: make cos-cert-setup\n\
                    ---\n# Runner\n";
    write(&skill, original);

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let after = std::fs::read_to_string(&skill).unwrap();

    // Stated as the consequence first: whatever `--fix` wrote, the source it
    // just rewrote must still install. A token in `requires:` is read straight
    // out of the source frontmatter and never expanded, so wrapping one turns a
    // resolvable dependency into a `BadReference`.
    let m = sb.mind(&["meld", &target, "--yes"]);
    assert!(
        m.success,
        "the source `--fix` just rewrote must still meld: {} {} (file is now:\n{after})",
        m.stdout, m.stderr
    );
    let l = sb.mind(&["learn", "cos:cos-run", "--yes"]);
    assert!(
        l.success,
        "the `requires:` entry must still resolve after --fix: {} {} (file is now:\n{after})",
        l.stdout, l.stderr
    );

    assert!(
        after.contains("requires: skill:cos-spec\n"),
        "a `requires:` entry is an item ref, not prose: {after}"
    );
    assert!(
        after.contains("build: make cos-cert-setup\n"),
        "a `build:` value is a shell command, not prose: {after}"
    );
}

/// NS-52 through `review`'s own output. `Cell::LinkSyntax` deliberately has no
/// `NsContext` of its own -- it maps onto `Path`, so the advisory reads "in a
/// path" -- and nothing but this asserts that the mapping reaches the user.
/// Every token here sits in a different part of a link (destination, reference
/// label, definition label, definition title), and `--fix` must take all four
/// back out without the wrapping pass putting any of them back.
/// spec: NS-52 NS-24
#[test]
fn review_reports_a_token_in_link_syntax_as_a_path_and_fix_removes_it() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    write(
        &skill,
        "---\nname: cos-http\ndescription: http client\n---\n\
         Read [the notes]({{ns:cos-spec}}.md) and [more][{{ns:cos-spec}}].\n\n\
         [{{ns:cos-spec}}]: https://example.com/x \"{{ns:cos-cert-setup}} notes\"\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "review must exit 0: {} {}", r.stdout, r.stderr);
    assert_eq!(
        r.stdout.matches("in a path").count(),
        4,
        "every part of a link but its text reports as a path: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("{{ns:cos-spec}} in a path"),
        "the destination and the labels: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("{{ns:cos-cert-setup}} in a path"),
        "the definition's title too: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("in a code span") && !r.stdout.contains("in a code block"),
        "link syntax is not code: {}",
        r.stdout
    );

    let f = sb.mind(&["review", &target, "--fix"]);
    assert!(f.success, "{} {}", f.stdout, f.stderr);
    let fixed = "---\nname: cos-http\ndescription: http client\n---\n\
                 Read [the notes](cos-spec.md) and [more][cos-spec].\n\n\
                 [cos-spec]: https://example.com/x \"cos-cert-setup notes\"\n";
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        fixed,
        "--fix un-wraps link syntax and wrapping must not put it back"
    );
    let f2 = sb.mind(&["review", &target, "--fix"]);
    assert!(f2.success, "{} {}", f2.stdout, f2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        fixed,
        "--fix must settle after one pass"
    );
}

/// The wrapping direction of NS-52 over one document carrying every link
/// spelling at once, pinned byte for byte. A link's visible text is the prose
/// wrapping is meant to reach; everything else it is made of is syntax, and a
/// rewrite there breaks the link in the author's working tree. The shapes the
/// unit tests could not compose -- a link in a table cell, a link in a
/// blockquote, an angle-bracket destination, an image, a code span inside link
/// text -- are all here together, so an interaction between them fails this.
/// spec: NS-52 INIT-5
#[test]
fn fix_wraps_only_the_visible_text_of_every_link_spelling() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    write(
        &skill,
        "---\nname: cos-http\ndescription: http client\n---\n\
         See [the cos-spec notes](cos-spec.md) and ![a shot](<a cos-spec shot.png>).\n\n\
         | step | link |\n\
         |---|---|\n\
         | one | [the cos-spec guide](cos-spec.md) |\n\n\
         > Quoted [cos-spec] shortcut, and [the cos-spec page][cos-spec].\n\n\
         Mail <cos-spec@example.com> or read [the `cos-spec` command](x.md).\n\n\
         [cos-spec]: https://example.com/x \"the cos-cert-setup notes\"\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let expected = "---\nname: cos-http\ndescription: http client\n---\n\
         See [the {{ns:cos-spec}} notes](cos-spec.md) and ![a shot](<a cos-spec shot.png>).\n\n\
         | step | link |\n\
         |---|---|\n\
         | one | [the {{ns:cos-spec}} guide](cos-spec.md) |\n\n\
         > Quoted [cos-spec] shortcut, and [the {{ns:cos-spec}} page][cos-spec].\n\n\
         Mail <cos-spec@example.com> or read [the `cos-spec` command](x.md).\n\n\
         [cos-spec]: https://example.com/x \"the cos-cert-setup notes\"\n";
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), expected);
    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        expected,
        "--fix must not re-dirty the file"
    );
}

/// The BOM fix in the combination a Windows editor actually produces: a BOM and
/// CRLF together. The frontmatter read has to strip three bytes and then track
/// one extra byte per line, and the field it protects is the `name:` NS-24 calls
/// untouchable; `description:` is not, so its sibling mention is wrapped too
/// (NS-56). Driven through the binary, so discovery reading the item and the
/// classifier reading the same file have to agree about where its frontmatter
/// is, and so the display-vs-store ordering (NS-56) is proven end to end:
/// `--fix` wraps `description:`, `probe` still shows the flattened bare name
/// (`namespace::flatten_display`), and the installed store copy has the fully
/// expanded (prefixed) name.
/// spec: NS-47 NS-24 NS-56
#[test]
fn fix_keeps_the_frontmatter_of_a_bom_prefixed_crlf_file() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-spec/SKILL.md");
    write(
        &skill,
        "\u{feff}---\r\nname: cos-spec\r\ndescription: hands off to cos-cert-setup\r\n\
         ---\r\n# spec\r\n\r\nSee the cos-cert-setup skill.\r\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        "\u{feff}---\r\nname: cos-spec\r\ndescription: hands off to {{ns:cos-cert-setup}}\r\n\
         ---\r\n# spec\r\n\r\nSee the {{ns:cos-cert-setup}} skill.\r\n",
        "name: survives the BOM; description: and the body mention are both wrapped"
    );
    // The premise of the fix, asserted rather than assumed: `frontmatter.rs`
    // strips the same BOM (DSC-23), so this file really is an item mind
    // discovers, describes and installs. If it were not, protecting its
    // frontmatter would be protecting nothing.
    let m = sb.mind(&["meld", &target, "--yes"]);
    assert!(m.success, "meld must succeed: {} {}", m.stdout, m.stderr);
    let p = sb.mind(&["probe", "cos-spec", "--no-tui"]);
    assert!(p.success, "{} {}", p.stdout, p.stderr);
    assert!(
        p.stdout.contains("hands off to cos-cert-setup"),
        "discovery reads the BOM-prefixed frontmatter, and the token `--fix` \
         wrapped it into is flattened back to the bare name for display: {}",
        p.stdout
    );
    assert!(
        !p.stdout.contains("{{ns:"),
        "the raw token must never leak into the catalog display: {}",
        p.stdout
    );
    let l = sb.mind(&["learn", "cos:cos-spec", "--yes"]);
    assert!(l.success, "learn must succeed: {} {}", l.stdout, l.stderr);
    let installed = std::fs::read_to_string(sb.mind_home.join("store/skill/cos:cos-spec/SKILL.md"))
        .expect("installed skill");
    assert!(
        installed.contains("name: cos-spec\r\n"),
        "the declared name is installed unprefixed and untouched: {installed}"
    );
    assert!(
        installed.contains("description: hands off to cos:cos-cert-setup\r\n"),
        "the description token expands fully (prefixed) in the installed copy: {installed}"
    );
    assert!(
        installed.contains("See the cos:cos-cert-setup skill."),
        "and the body token the fix created expands: {installed}"
    );
    // `probe` above is one display surface; `recall` is the other. It reads the
    // description back out of the manifest (recorded at install from the
    // catalog) rather than out of the file, so it is a second, independent read
    // that would leak the raw token if the flatten sat only on the path `probe`
    // happens to take. (`dump` emits no item description at all, so it is not a
    // third surface however NS-56 once read it.)
    let rc = sb.mind(&["recall", "cos:cos-spec"]);
    assert!(rc.success, "{} {}", rc.stdout, rc.stderr);
    assert!(
        rc.stdout.contains("hands off to cos-cert-setup") && !rc.stdout.contains("{{ns:"),
        "recall must show the flattened description: {}",
        rc.stdout
    );
}

/// Characterization, not endorsement: `review --fix` passes the *whole* sibling
/// set to the wrapper, including the item's own name, so an item that names
/// itself in its own prose (a `# name` heading is the common shape) has that
/// mention wrapped. `init-source --template` removes the item's own name first
/// (`commands.rs`), so the two wrappers disagree about self-mentions. This is
/// pre-existing and not part of the NS-46/NS-47 change, but it is the same file
/// rewrite, so it is pinned here rather than left undetected.
/// spec: NS-24
#[test]
fn fix_wraps_an_items_own_name_in_its_own_prose() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-http/SKILL.md");
    write(
        &skill,
        "---\nname: cos-http\ndescription: http client\n---\n\
         # cos-http\n\n\
         Body text.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let fixed = std::fs::read_to_string(&skill).unwrap();
    assert!(
        fixed.contains("# {{ns:cos-http}}"),
        "the heading is rewritten to a token: {fixed}"
    );
    assert!(
        fixed.contains("name: cos-http"),
        "frontmatter is still untouched: {fixed}"
    );
}

// ---------------------------------------------------------------------------
// CLI-223: a `{{...}}` token that RESOLVES in a non-markdown item file is
// still inert (it never expands there), and review must not stay silent
// about it the way it used to.
// ---------------------------------------------------------------------------

/// A `{{tools:detect}}` token that names a real sibling tool -- i.e. one that
/// `expand_paths` resolves successfully -- but sits in a bundled `.sh` file
/// gets an `inert-token` advisory: the token would resolve if the file were
/// markdown, but no token family expands outside markdown (NS-53), so it is
/// left literal at install and silently breaks the script at runtime. Before
/// this check, review only flagged an UNRESOLVED token here (`bad-reference`)
/// and said nothing about a resolvable one, which is the exact case this
/// advisory exists to catch.
/// spec: CLI-223
#[test]
fn inert_token_in_a_non_markdown_file_is_advisory() {
    let sb = fixture();
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    write(&script, "#!/bin/sh\n{{tools:detect}} --scan\n");

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "advisory-only run must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("[inert-token]"),
        "a resolvable token in a non-markdown file must still be flagged \
         inert: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("run.sh"),
        "the advisory must name the file: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("{{tools:detect}}"),
        "the advisory must name the token: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("markdown only"),
        "the advisory must say tokens expand in markdown only: {}",
        r.stdout
    );
    // The script itself is never rewritten (NS-54): `--fix` only reports here.
    let r2 = sb.mind(&["review", &target, "--fix"]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert_eq!(
        std::fs::read_to_string(&script).unwrap(),
        "#!/bin/sh\n{{tools:detect}} --scan\n",
        "--fix must never rewrite a non-markdown file"
    );
}

/// The identical resolvable token, in a markdown file, expands normally and is
/// not flagged `inert-token`: the whole point of the advisory is that the
/// token never reaches install here, so it must not fire where the token
/// actually does expand.
/// spec: CLI-223
#[test]
fn inert_token_in_a_markdown_file_is_not_flagged() {
    let sb = fixture();
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n\
         Run {{tools:detect}} --scan.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("[inert-token]"),
        "the same token in a markdown file expands there, so it must not be \
         flagged inert: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// CLI-224: finding messages are sanitized before they reach either output
// path (stderr text, `--json`'s `details`/document).
// ---------------------------------------------------------------------------

/// The sibling of the advisory test for the HARD finding path (CLI-224 covers
/// both constructors). A `{{ns:}}` token in the frontmatter `name:` field is a
/// hard `misplaced-reference` finding, and its message embeds the token name
/// verbatim -- source-controlled text. A bidi override there must be stripped
/// from the stderr text (hard findings print to stderr) and from the `--json`
/// `hard` array, exactly as the advisory path is. Without this, the
/// `Finding::hard` sanitize call could regress to identity undetected: no other
/// test drives a hard finding through the sanitize boundary.
/// spec: CLI-224
#[test]
fn hard_finding_message_strips_a_bidi_override_in_text_and_json_output() {
    let sb = fixture();
    // A `{{ns:}}` token in the frontmatter `name:` field -> hard
    // misplaced-reference; the token name (with the bidi override) is embedded
    // in the message verbatim.
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: {{ns:evil\u{202E}x}}\ndescription: runner\n---\n# Runner\n",
    );

    let target = sb.source_spec();

    // Text mode: a hard finding fails the run and prints to stderr.
    let text = sb.mind(&["review", &target]);
    assert!(
        text.stderr.contains("[misplaced-reference]"),
        "the hard finding must fire on stderr: {} {}",
        text.stdout,
        text.stderr
    );
    assert!(
        !text.stderr.contains('\u{202E}') && !text.stdout.contains('\u{202E}'),
        "bidi override must be stripped from the hard finding output: {} {}",
        text.stdout,
        text.stderr
    );

    // `--json`: the hard array carries the same sanitized message.
    let json = sb.mind(&["review", &target, "--json"]);
    assert!(
        !json.stdout.contains('\u{202E}'),
        "bidi override must be stripped from the --json output: {}",
        json.stdout
    );
    // Hard findings fail the run, so under `--json` the review document is the
    // CLI-181 error envelope's `details` member (CLI-221), not the top level.
    let doc: serde_json::Value =
        serde_json::from_str(&json.stdout).expect("--json must emit one JSON document");
    let hard = doc["details"]["hard"]
        .as_array()
        .expect("hard array present under details");
    let msg = hard
        .iter()
        .find(|f| f["kind"] == "misplaced-reference")
        .expect("a misplaced-reference hard finding is present")["message"]
        .as_str()
        .expect("message is a string");
    assert!(
        !msg.contains('\u{202E}'),
        "the parsed JSON hard message field must not carry the bidi override: {msg}"
    );
}

/// A token whose inner text carries a bidi-override character (U+202E) is
/// source-controlled, and it flows verbatim into the advisory finding's
/// message. Both the human-readable text output and the `--json` document
/// must have it stripped, not just serde-escaped: a bidi override renders
/// visually even when it round-trips through valid JSON.
///
/// The fixture still writes a sibling tool dir carrying the same bidi-laced
/// name, but DSC-96 now rejects that name at scan, so the sibling never enters
/// the catalog and the token no longer resolves: the advisory that fires is
/// `bad-reference`, not `inert-token`. The sanitize boundary under test is the
/// same either way -- the finding message is source-controlled text on its way
/// to the terminal -- so this asserts on that, not on which check fired.
/// spec: CLI-224, DSC-96
#[test]
fn finding_message_strips_a_bidi_override_in_text_and_json_output() {
    let sb = fixture();
    write(
        &sb.source.join("tools/evil\u{202E}here/evil\u{202E}here"),
        "#!/bin/sh\n",
    );
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    write(&script, "#!/bin/sh\n{{tools:evil\u{202E}here}} --scan\n");

    let target = sb.source_spec();

    // Human text output: the raw bidi-override byte sequence must not survive.
    let text = sb.mind(&["review", &target]);
    assert!(text.success, "{} {}", text.stdout, text.stderr);
    assert!(
        !text.stdout.contains('\u{202E}'),
        "bidi override must be stripped from the human-readable finding: {}",
        text.stdout
    );
    assert!(
        text.stdout.contains("[bad-reference]"),
        "the finding must still fire (only the character is stripped): {}",
        text.stdout
    );

    // `--json`: the raw character must not survive inside the message field
    // either, even though serde keeps the document structurally valid JSON.
    let json = sb.mind(&["review", &target, "--json"]);
    assert!(json.success, "{} {}", json.stdout, json.stderr);
    assert!(
        !json.stdout.contains('\u{202E}'),
        "bidi override must be stripped from the --json output: {}",
        json.stdout
    );
    let doc: serde_json::Value =
        serde_json::from_str(&json.stdout).expect("--json must emit one JSON document");
    let advisory = doc["advisory"].as_array().expect("advisory array present");
    let msg = advisory
        .iter()
        .find(|f| f["kind"] == "bad-reference")
        .expect("a bad-reference advisory is present")["message"]
        .as_str()
        .expect("message is a string");
    assert!(
        !msg.contains('\u{202E}'),
        "the parsed JSON message field must not carry the bidi override: {msg}"
    );
}

/// The suppression side of CLI-223: a single RESOLVABLE `{{ns:}}` token in a
/// non-markdown file used to draw BOTH `misplaced-reference` (Check 11) and
/// `inert-token` (Check 14) for the same span -- one broken/misplaced
/// reference reported as two findings. Check 11 unconditionally reports every
/// `{{ns:}}` token in a non-markdown file (misplaced by construction there),
/// so it already names this span; the generic `inert-token` net now excludes
/// any `{{ns:...}}` token for that reason, leaving exactly one finding.
/// spec: CLI-223
#[test]
fn resolvable_ns_token_in_a_script_draws_only_misplaced_not_inert() {
    let sb = fixture();
    // cos-spec is a real sibling, so Check 5 (ns resolution) does NOT hard-fail;
    // the token simply sits in a script where it will never expand.
    let script = sb.source.join("skills/cos-spec/run.sh");
    write(&script, "#!/bin/sh\n# see {{ns:cos-cert-setup}}\n");

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "a resolvable token must not hard-fail review: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("[misplaced-reference]"),
        "Check 11 still fires for a non-markdown ns token: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("[inert-token]"),
        "Check 14 must not re-report the same {{{{ns:}}}} span Check 11 already \
         named, or a single broken/misplaced reference reads as two findings: {}",
        r.stdout
    );
}

/// An UNRESOLVABLE `{{ns:}}` token in a NON-markdown file must not hard-fail
/// review: install expands `{{ns:}}` in markdown only (NS-53), so the token is
/// dead text that can never become a `BadReference` at install. Check 5 mirrors
/// Check 8's non-markdown downgrade (CLI-132, CLI-135) and reports the miss as
/// an advisory instead. Check 11 also fires (every `{{ns:}}` token in a
/// non-markdown file is misplaced there, whether or not it resolves), but the
/// generic `inert-token` net (Check 14) does NOT re-report the same span a
/// third time: it excludes every `{{ns:...}}` token in a non-markdown file,
/// since Check 11 already names all of them unconditionally (CLI-223).
/// spec: CLI-132 CLI-223
#[test]
fn unresolvable_ns_token_in_a_script_is_advisory_not_hard() {
    let sb = fixture();
    let script = sb.source.join("skills/cos-spec/run.sh");
    write(&script, "#!/bin/sh\n# call {{ns:nosuchsibling}} here\n");

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "an unresolved ns token in a non-markdown file must not hard-fail review: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("[bad-reference]"),
        "Check 5 must not emit a hard bad-reference for a non-markdown file: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        r.stdout.contains("[bad-reference]"),
        "the unresolved token is still surfaced, just as an advisory: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("[misplaced-reference]"),
        "Check 11 still fires for the same non-markdown ns token: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("[inert-token]"),
        "the generic inert-token net must not re-report the same {{{{ns:}}}} \
         span Check 5 and Check 11 already named, or one broken reference \
         reads as three findings: {}",
        r.stdout
    );
}

/// A resolvable `{{tools:detect}}` token in a `.sh` -- the case `inert-token`
/// exists for (CLI-223): no other check has any reason to mention it, since it
/// resolves fine and is not an `{{ns:}}` token -- draws exactly ONE
/// `inert-token` advisory, not a duplicate. Counts the `--json` advisory array
/// rather than a substring `contains`, so a regression that emitted the same
/// finding twice would be caught even though both copies read identically.
/// spec: CLI-223
#[test]
fn resolvable_tools_token_in_a_script_yields_exactly_one_inert_token_advisory() {
    let sb = fixture();
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    write(&script, "#!/bin/sh\n{{tools:detect}} --scan\n");

    let target = sb.source_spec();
    let json = sb.mind(&["review", &target, "--json"]);
    assert!(json.success, "{} {}", json.stdout, json.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&json.stdout).expect("--json must emit one JSON document");
    let inert: Vec<&serde_json::Value> = doc["advisory"]
        .as_array()
        .expect("advisory array present")
        .iter()
        .filter(|f| f["kind"] == "inert-token")
        .collect();
    assert_eq!(
        inert.len(),
        1,
        "a resolvable token that no other check mentions must draw exactly one \
         inert-token advisory: {inert:?}"
    );
}

/// The suppression is per-TOKEN, not per-file: an unresolved path token draws a
/// `bad-reference` (Check 8, which stops at the first miss) and is excluded
/// from `inert-token`, but a second, unrelated, resolvable token in the SAME
/// file is not swept up by that exclusion and still gets its own
/// `inert-token` advisory. Guards against an implementation that suppresses
/// the whole file's `inert-token` finding rather than just the one span
/// another check already reported.
/// spec: CLI-223
#[test]
fn bad_reference_suppression_does_not_swallow_an_unrelated_token_in_the_same_file() {
    let sb = fixture();
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    // {{tools:nosuch}} does not resolve (Check 8: bad-reference, advisory in a
    // non-markdown file); {{tools:detect}} does resolve and is unrelated.
    write(
        &script,
        "#!/bin/sh\n{{tools:nosuch}}\n{{tools:detect}} --scan\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("[bad-reference]") && r.stdout.contains("{{tools:nosuch}}"),
        "the unresolved token is still reported by Check 8: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("[inert-token]") && r.stdout.contains("{{tools:detect}}"),
        "the unrelated resolvable token in the same file must still be named \
         by inert-token: {}",
        r.stdout
    );
    // The bad token itself must not ALSO show up inside the inert-token
    // finding's own token list -- that would be the duplicate this test guards
    // against.
    let inert_line = r
        .stdout
        .lines()
        .find(|l| l.contains("[inert-token]"))
        .expect("an inert-token line is present");
    assert!(
        !inert_line.contains("{{tools:nosuch}}"),
        "the token Check 8 already reported must not also appear in the \
         inert-token finding's list: {inert_line}"
    );
}

/// P1(c), the `{{ns:}}`-exclusion path (distinct from the check8 path the test
/// above covers): a script holding TWO distinct tokens -- one `{{ns:}}` token
/// that Check 11 already reports as misplaced, and one resolvable `{{tools:}}`
/// token that only the generic net catches -- must report BOTH. The `ns:`
/// exclusion in Check 14 keys on the token's inner text, so a mutation that
/// widened it to drop the whole file (or the sibling tools token) would leave
/// the resolvable token unreported. Guards that the exclusion removes only the
/// ns span, never the unrelated tools span in the same file.
/// spec: CLI-223
#[test]
fn ns_token_and_resolvable_tools_token_in_same_script_are_both_reported() {
    let sb = fixture();
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    // {{ns:cos-cert-setup}} resolves (a real sibling) but is misplaced in a
    // script -> Check 11. {{tools:detect}} resolves -> only Check 14.
    write(
        &script,
        "#!/bin/sh\n# see {{ns:cos-cert-setup}}\n{{tools:detect}} --scan\n",
    );

    let target = sb.source_spec();
    let json = sb.mind(&["review", &target, "--json"]);
    assert!(json.success, "{} {}", json.stdout, json.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&json.stdout).expect("--json must emit one JSON document");
    let advisory = doc["advisory"].as_array().expect("advisory array present");

    // Check 11 names the ns token as misplaced.
    let misplaced: Vec<&serde_json::Value> = advisory
        .iter()
        .filter(|f| f["kind"] == "misplaced-reference")
        .collect();
    assert_eq!(
        misplaced.len(),
        1,
        "the ns token draws exactly one misplaced-reference: {advisory:#?}"
    );

    // Check 14 names the tools token exactly once, and its token list must NOT
    // contain the ns token (that one is excluded, reported by Check 11 instead).
    let inert: Vec<&serde_json::Value> = advisory
        .iter()
        .filter(|f| f["kind"] == "inert-token")
        .collect();
    assert_eq!(
        inert.len(),
        1,
        "the resolvable tools token draws exactly one inert-token: {advisory:#?}"
    );
    let inert_msg = inert[0]["message"].as_str().unwrap();
    assert!(
        inert_msg.contains("{{tools:detect}}"),
        "inert-token must name the tools token: {inert_msg}"
    );
    assert!(
        !inert_msg.contains("cos-cert-setup"),
        "inert-token must NOT re-list the ns token Check 11 already named: {inert_msg}"
    );
}

/// Known-divergence pin (P1d): the Check 14 `ns:` exclusion keys on the token's
/// trimmed inner text starting with `ns:`, but Check 11 (`scan_ns_refs`) only
/// recognizes the literal `{{ns:` open delimiter. A token with whitespace
/// BETWEEN the braces and `ns:` -- `{{ ns:foo }}` -- is therefore excluded by
/// Check 14 (its inner trims to `ns:foo`) yet NOT reported by Check 11 (the
/// `{{ns:` scan does not match the space), so it is reported by nobody. It also
/// never expands anywhere (install's `expand` needs the same literal `{{ns:`),
/// so it is genuinely dead text either way -- the divergence is an
/// under-report of an already-broken token, not a mis-expansion. This pins the
/// CURRENT behavior so the divergence is visible and a future change to close
/// it is a deliberate, test-observed decision rather than a silent drift.
/// spec: CLI-223
#[test]
fn spaced_ns_token_in_a_script_falls_through_both_checks_current_behavior() {
    let sb = fixture();
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    // Note the space between `{{` and `ns:`.
    write(&script, "#!/bin/sh\n# see {{ ns:cos-cert-setup }}\n");

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    // Current behavior: neither Check 11 nor Check 14 reports this token.
    assert!(
        !r.stdout.contains("[misplaced-reference]"),
        "Check 11 does not recognize a spaced `{{{{ ns:` token: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("[inert-token]"),
        "the `ns:` exclusion drops the spaced token even though Check 11 does \
         not report it -- a known under-report divergence pinned here: {}",
        r.stdout
    );
}

/// The same unresolvable `{{ns:}}` token in a MARKDOWN item file is unchanged:
/// install WILL try to expand it there, so an unresolved token is a genuine
/// install-blocking defect and Check 5 still hard-fails.
/// spec: CLI-132
#[test]
fn unresolvable_ns_token_in_markdown_still_hard_fails() {
    let sb = fixture();
    let skill = sb.source.join("skills/cos-spec/SKILL.md");
    write(
        &skill,
        "---\nname: cos-spec\ndescription: EARS patterns\n---\n\
         # spec\n\nSee {{ns:nosuchsibling}} for details.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        !r.success,
        "an unresolved ns token in a markdown file must still hard-fail review: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("[bad-reference]"),
        "Check 5 must still emit a hard bad-reference for a markdown file: {} {}",
        r.stdout,
        r.stderr
    );
}

/// The sanitize boundary must strip only ANSI/control/bidi, never legitimate
/// printable non-ASCII. A sibling-token name carrying an accented letter
/// (U+00E9, not a bidi/control code point) must survive verbatim in the finding
/// message, in both text and `--json`. Guards against the boundary regressing
/// to an over-aggressive ASCII-only filter that would silently corrupt an
/// international curator's name or a non-English token.
/// spec: CLI-224
#[test]
fn finding_message_preserves_legitimate_non_ascii() {
    let sb = fixture();
    // The token names a real sibling tool, so it resolves and Check 8 leaves
    // it to `inert-token` alone (CLI-223's suppression only excludes a token
    // another check already reported).
    write(
        &sb.source.join("tools/caf\u{00e9}/caf\u{00e9}"),
        "#!/bin/sh\n",
    );
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    // An accented letter inside the token; backticks/braces are ordinary
    // printable ASCII and must survive too.
    write(&script, "#!/bin/sh\n{{tools:caf\u{00e9}}} --scan\n");

    let target = sb.source_spec();
    let text = sb.mind(&["review", &target]);
    assert!(text.success, "{} {}", text.stdout, text.stderr);
    assert!(
        text.stdout.contains("{{tools:caf\u{00e9}}}"),
        "the accented token must survive verbatim in the text finding: {}",
        text.stdout
    );

    let json = sb.mind(&["review", &target, "--json"]);
    assert!(json.success, "{} {}", json.stdout, json.stderr);
    let doc: serde_json::Value =
        serde_json::from_str(&json.stdout).expect("--json must emit one JSON document");
    let msg = doc["advisory"]
        .as_array()
        .expect("advisory array present")
        .iter()
        .find(|f| f["kind"] == "inert-token")
        .expect("an inert-token advisory is present")["message"]
        .as_str()
        .expect("message is a string");
    assert!(
        msg.contains("caf\u{00e9}"),
        "the accented letter must survive the sanitize boundary in --json: {msg}"
    );
}
