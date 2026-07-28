//! Integration tests for `mind review --fix` and the `{{ns:}}` context
//! classifier (spec/namespacing.md NS-46, NS-47, NS-48).
//!
//! `--fix` is the one path that rewrites a user's files, and it un-wraps a token
//! it believes sits in code. These tests drive the real binary against hermetic
//! fixture sources (local path, no network) with isolated MIND_HOME /
//! CLAUDE_HOME temp dirs, and assert both directions: a prose token survives the
//! rewrite byte for byte, and a token genuinely in code is still un-wrapped.

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

/// A fenced block may open on the same line as a list marker. `--fix` reads the
/// opener as prose and the real closer as an opener, so the rest of the item is
/// a code block: every token below it is un-wrapped, which is the filed bug's
/// exact symptom in a document that no existing test covers.
/// spec: NS-47 NS-49
#[test]
#[ignore = "defect: a fence opened on a list-marker line inverts the block, so \
            `--fix` deletes the tokens in the rest of the item"]
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

// ---------------------------------------------------------------------------
// What else the same rewrite pass does to prose it decides is prose
// ---------------------------------------------------------------------------

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
