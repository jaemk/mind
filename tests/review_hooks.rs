//! Integration tests for `mind review` install-hook advisory findings.
//!
//! Covers:
//!   - HOOK-90: [source].install deprecated-field advisory
//!   - CLI-146: install-hook-safe wording in hardcoded-path (OtherItem)
//!     and bare-tool-reference advisories
//!
//! Each test drives the real `mind` binary against a hermetic fixture source
//! directory (local path, no network), using isolated MIND_HOME / CLAUDE_HOME
//! temp dirs, exactly as tests/cli.rs does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Minimal fixture harness (mirrors tests/cli.rs)
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
        let base = std::env::temp_dir().join(format!("mind-rh-{}-{n}", std::process::id()));
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

// ---------------------------------------------------------------------------
// HOOK-90: deprecated-field advisory for [source].install
// ---------------------------------------------------------------------------

/// `mind review` on a source whose mind.toml has [source].install prints a
/// `deprecated-field` advisory and exits 0 (advisory, not hard).
/// spec: HOOK-90
#[test]
fn review_source_install_emits_deprecated_field_advisory() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ninstall = \"make build\"\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(
        r.success,
        "deprecated-field is advisory; review must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("deprecated-field"),
        "expected deprecated-field advisory in stdout: {}",
        r.stdout
    );
    // Must name the [[hooks]] equivalent form.
    assert!(
        r.stdout.contains("[[hooks]]"),
        "deprecated-field advisory must mention [[hooks]]: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("event = \"install\""),
        "deprecated-field advisory must mention event = \"install\": {}",
        r.stdout
    );
    // Must echo the declared command so the maintainer can verify.
    assert!(
        r.stdout.contains("make build"),
        "deprecated-field advisory must echo the command: {}",
        r.stdout
    );
}

/// The deprecated-field advisory is emitted ALONGSIDE the install-hook
/// advisory (both must appear when [source].install is declared).
/// spec: HOOK-90
#[test]
fn review_source_install_emits_both_install_hook_and_deprecated_field() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ninstall = \"npm install\"\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only review exits 0: {}", r.stdout);
    assert!(
        r.stdout.contains("install-hook"),
        "install-hook advisory must still be present: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("deprecated-field"),
        "deprecated-field advisory must also be present: {}",
        r.stdout
    );
}

/// A source that uses only [[hooks]] (no legacy [source].install) produces
/// no deprecated-field advisory.
/// spec: HOOK-90
#[test]
fn review_hooks_table_only_no_deprecated_field() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("mind.toml"),
        "[[hooks]]\nrun = \"npm install\"\nevent = \"install\"\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only review exits 0: {}", r.stdout);
    assert!(
        !r.stdout.contains("deprecated-field"),
        "[[hooks]]-only source must not emit deprecated-field: {}",
        r.stdout
    );
}

/// A whitespace-only [source].install is treated as absent (HOOK-3), so it
/// yields NO deprecated-field advisory (and runs no hook).
/// spec: HOOK-90
#[test]
fn review_whitespace_source_install_emits_no_deprecated_field() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("mind.toml"),
        "[source]\ninstall = \"   \"\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only review exits 0: {}", r.stdout);
    assert!(
        !r.stdout.contains("deprecated-field"),
        "whitespace-only [source].install must not emit deprecated-field: {}",
        r.stdout
    );
    // It is also treated as absent, so no install-hook advisory either.
    assert!(
        !r.stdout.contains("install-hook"),
        "whitespace-only install is absent, so no install-hook advisory: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// CLI-146: install-hook-safe wording in advisory messages
// ---------------------------------------------------------------------------

/// The hardcoded-path OtherItem advisory message notes that when an install
/// hook places a resource at a fixed path, referencing it there is safe.
/// spec: CLI-146
#[test]
fn review_hardcoded_path_other_item_carries_install_hook_safe_note() {
    let sb = Sandbox::new("agents");
    // A sibling agent's install path is an OtherItem reference from the skill's
    // perspective (not the item's own resource). Use ~/.claude/agents/dev.md.
    write(
        &sb.source.join("agents/dev.md"),
        "---\ndescription: dev agent\n---\n# dev\n",
    );
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\nuse ~/.claude/agents/dev.md for context\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only: {}", r.stdout);
    assert!(
        r.stdout.contains("hardcoded-path"),
        "expected hardcoded-path advisory: {}",
        r.stdout
    );
    // CLI-146: OtherItem message must note install-hook-safe case.
    assert!(
        r.stdout.contains("intentional") || r.stdout.contains("safe"),
        "hardcoded-path OtherItem advisory must note install-hook-safe: {}",
        r.stdout
    );
    // Fragile note must still be present.
    assert!(
        r.stdout.contains("fragile"),
        "hardcoded-path OtherItem advisory must still say fragile: {}",
        r.stdout
    );
}

/// The hardcoded-path OtherItem advisory carries the install-hook-safe note
/// EVEN WHEN no token suggestion is available (the path names a non-sibling, so
/// `token_for_path` yields no suggestion). The safe note must still be present.
/// spec: CLI-146
#[test]
fn review_hardcoded_path_other_item_no_suggestion_still_install_hook_safe() {
    let sb = Sandbox::new("agents");
    // The skill references an agent install path whose item is NOT a sibling of
    // this source (no `ghost` agent exists), so it is an OtherItem with no token
    // suggestion. The install-hook-safe wording must still appear.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\nload ~/.claude/agents/ghost.md for context\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only: {}", r.stdout);
    assert!(
        r.stdout.contains("hardcoded-path"),
        "expected hardcoded-path advisory: {}",
        r.stdout
    );
    // CLI-146: the install-hook-safe note is present even with no `; use <tok>`.
    assert!(
        r.stdout.contains("intentional") || r.stdout.contains("safe"),
        "no-suggestion OtherItem advisory must still note install-hook-safe: {}",
        r.stdout
    );
    // There must be no token suggestion clause for a non-sibling.
    assert!(
        !r.stdout.contains("; use {{"),
        "a non-sibling OtherItem path should carry no token suggestion: {}",
        r.stdout
    );
}

/// The bare-tool-reference advisory message notes that when an install hook
/// places the helper at a known location, calling it there is safe.
/// spec: CLI-146
#[test]
fn review_bare_tool_reference_carries_install_hook_safe_note() {
    let sb = Sandbox::new("agents");
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\nFirst run the detect helper, then review.\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only: {}", r.stdout);
    assert!(
        r.stdout.contains("bare-tool-reference"),
        "expected bare-tool-reference advisory: {}",
        r.stdout
    );
    // CLI-146: bare-tool-reference message must note install-hook-safe case.
    assert!(
        r.stdout.contains("intentional") || r.stdout.contains("safe"),
        "bare-tool-reference advisory must note install-hook-safe: {}",
        r.stdout
    );
}

/// The hardcoded-path OwnResource ({{self}}) advisory keeps its existing
/// fragile-not-broken wording and does NOT gain install-hook-safe language.
/// spec: CLI-145 (unchanged), CLI-146 (OwnResource carve-out)
#[test]
fn review_hardcoded_path_own_resource_wording_unchanged() {
    let sb = Sandbox::new("agents");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\nrun ~/.claude/skills/review/resources/pr.py here\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only: {}", r.stdout);
    // OwnResource wording: "hardcodes its own resource path" + "this works but assumes"
    assert!(
        r.stdout.contains("hardcodes its own resource path"),
        "OwnResource arm must keep existing wording: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("this works but assumes"),
        "OwnResource arm must keep works-but-assumes wording: {}",
        r.stdout
    );
    // Token suggestion must still appear.
    assert!(
        r.stdout.contains("{{self}}/resources/pr.py"),
        "OwnResource arm must suggest the token: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// CLI-145: SharedTool wording is distinct from OtherItem / install-hook-safe
// ---------------------------------------------------------------------------

/// The hardcoded-path SharedTool advisory fires when a skill references a
/// store-only tool path (`~/.mind/store/tool/<name>/...`) that names a real
/// sibling tool. Its message states the tool is store-only and never linked
/// into an agent home. It does NOT carry the install-hook-safe note that
/// OtherItem advisories carry, because no install hook can place a file at an
/// agent-home path for a tool that is never linked there.
///
/// NOTE: the same skill also triggers a bare-tool-reference advisory (the tool
/// name "detect" appears in prose), which DOES carry "intentional"/"safe"
/// wording (CLI-146). We assert only on the hardcoded-path line to isolate the
/// SharedTool arm from the bare-tool-reference arm.
// spec: CLI-145, CLI-146
#[test]
fn review_hardcoded_path_shared_tool_wording_distinct_from_other_item() {
    let sb = Sandbox::new("agents");
    // A sibling tool `detect` alongside a skill that hardcodes its store path.
    // The skill references the tool via its mind-store absolute path, which
    // classify_path sees as SharedTool (a real sibling tool, store-only).
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\ndescription: review\n---\nrun ~/.mind/store/tool/detect/detect to analyze\n",
    );

    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(r.success, "advisory-only: {}", r.stdout);

    // Isolate the hardcoded-path advisory line to check SharedTool wording only.
    // (Other lines, e.g. bare-tool-reference, may legitimately carry "safe".)
    let hardcoded_line = r
        .stdout
        .lines()
        .find(|l| l.contains("hardcoded-path"))
        .unwrap_or_else(|| panic!("expected hardcoded-path advisory: {}", r.stdout));

    // CLI-145: SharedTool message must state the tool is store-only / never linked.
    assert!(
        hardcoded_line.contains("store-only"),
        "SharedTool hardcoded-path advisory must say store-only: {hardcoded_line}"
    );
    assert!(
        hardcoded_line.contains("never linked"),
        "SharedTool hardcoded-path advisory must say never linked: {hardcoded_line}"
    );
    // CLI-146: the hardcoded-path SharedTool line must NOT carry the
    // install-hook-safe note. A tool is store-only regardless of install hooks,
    // so the safe-location note does not apply to this arm.
    assert!(
        !hardcoded_line.contains("intentional") && !hardcoded_line.contains("safe"),
        "SharedTool hardcoded-path advisory must not carry the install-hook-safe note: \
         {hardcoded_line}"
    );
}
