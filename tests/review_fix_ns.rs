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

/// A `.markdown` file inside an item is markdown, and `install` expands the
/// `{{ns:}}` tokens in *every* UTF-8 file it copies, so those tokens are live
/// references. `--fix` decides prose-versus-code by an exact `.md` extension
/// test, so it treats the file as all code and strips every token in it,
/// leaving bare names that no longer resolve under a prefix. Same destructive
/// outcome as a misclassified fence, reached through the file filter instead.
/// spec: NS-24 NS-48
#[test]
#[ignore = "defect: `--fix` un-wraps every token in a `.markdown` (or `.txt`, \
            or uppercase `.MD`) file because the prose test is an exact `.md` \
            extension match, while `install` still expands them"]
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

/// A non-markdown file inside an item is all code, so `--fix` un-wraps every
/// `{{ns:}}` token in it and never wraps a bare name there. Characterization of
/// one incompleteness: the structure map is still a markdown map, so a script
/// line that happens to look like a fence delimiter is read as structure and a
/// token on it is neither reported nor un-wrapped. Not destructive (`install`
/// still expands it), but the cleanup is partial, so it is pinned rather than
/// left to be discovered.
/// spec: NS-24 NS-47
#[test]
fn fix_treats_a_non_markdown_file_as_all_code() {
    let sb = fixture();
    write(
        &sb.source.join("skills/cos-run/SKILL.md"),
        "---\nname: cos-run\ndescription: runner\n---\n# Runner\n",
    );
    let script = sb.source.join("skills/cos-run/run.sh");
    write(
        &script,
        "#!/bin/sh\n\
         # hand off to {{ns:cos-spec}}\n\
         echo cos-cert-setup\n\
         cat <<'EOF'\n\
         ```{{ns:cos-spec}}\n\
         EOF\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        std::fs::read_to_string(&script).unwrap(),
        "#!/bin/sh\n\
         # hand off to cos-spec\n\
         echo cos-cert-setup\n\
         cat <<'EOF'\n\
         ```{{ns:cos-spec}}\n\
         EOF\n",
        "the token in the script is un-wrapped, the bare name is never wrapped, \
         and the one on a fence-shaped line is left behind"
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
/// wrapping must never touch.
/// spec: NS-24 NS-47
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
        original,
        "frontmatter must survive a BOM"
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
/// untouchable. Driven through the binary, so discovery reading the item and the
/// classifier reading the same file have to agree about where its frontmatter is.
/// spec: NS-47 NS-24
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
        "\u{feff}---\r\nname: cos-spec\r\ndescription: hands off to cos-cert-setup\r\n\
         ---\r\n# spec\r\n\r\nSee the {{ns:cos-cert-setup}} skill.\r\n",
        "the frontmatter survives the BOM and only the body mention is wrapped"
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
        "discovery reads the BOM-prefixed frontmatter: {}",
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
        installed.contains("See the cos:cos-cert-setup skill."),
        "and the body token the fix created expands: {installed}"
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
