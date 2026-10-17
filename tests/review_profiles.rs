//! Integration tests for `mind review`'s install-profile reporting (PROF-30).
//!
//! Each test drives the real `mind` binary against a hermetic fixture source
//! directory (local path, no network), using isolated MIND_HOME / CLAUDE_HOME
//! temp dirs, exactly as tests/review_workflows.rs does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-rp-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("src");
        Sandbox {
            base: base.clone(),
            source,
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        }
    }

    /// Fixture: two skills (`review`, `test`), one rule (`style`), and the
    /// given `mind.toml` (a `[profiles]` table only, so convention discovery
    /// stays on).
    fn with_toml(toml: &str) -> Self {
        let sb = Sandbox::new();
        for skill in ["review", "test"] {
            write(
                &sb.source.join(format!("skills/{skill}/SKILL.md")),
                &format!("---\nname: {skill}\ndescription: the {skill} skill\n---\nbody\n"),
            );
        }
        write(
            &sb.source.join("rules/style.md"),
            "---\ndescription: house style\n---\nbody\n",
        );
        write(&sb.source.join("mind.toml"), toml);
        sb
    }

    fn review_with(&self, extra: &[&str]) -> Run {
        let target = self.source.to_string_lossy().into_owned();
        let mut args = vec!["review", target.as_str()];
        args.extend_from_slice(extra);
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(&args)
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
        self.review_with(&[])
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

/// Lines of `text` carrying `needle`.
fn lines<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines().filter(|l| l.contains(needle)).collect()
}

// spec: PROF-30 PROF-5
#[test]
fn clean_profiles_draw_no_profile_findings() {
    let sb = Sandbox::with_toml(
        "[profiles]\ndefault = [\"basic\"]\nbasic = [\"skill:review\", \"rule:style\"]\n\
         full = [\"basic\", \"skill:*\"]\nall = [\"*\"]\n",
    );
    let r = sb.review();
    assert!(r.success, "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    for tag in ["invalid-profile", "bad-profile-member", "empty-profile"] {
        assert!(
            !r.stdout.contains(tag) && !r.stderr.contains(tag),
            "unexpected {tag}: {}{}",
            r.stdout,
            r.stderr
        );
    }
}

// spec: PROF-30 PROF-5
#[test]
fn exact_member_naming_a_missing_item_is_a_hard_finding() {
    let sb = Sandbox::with_toml("[profiles]\nbasic = [\"skill:review\", \"skill:missing\"]\n");
    let r = sb.review();
    assert!(!r.success, "a missing exact member must fail review");
    let hit = lines(&r.stderr, "error [bad-profile-member]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("profile 'basic'"), "{}", hit[0]);
    assert!(hit[0].contains("skill:missing"), "{}", hit[0]);
    assert!(!hit[0].contains("skill:review"), "{}", hit[0]);
}

// spec: PROF-30 PROF-3
#[test]
fn empty_glob_member_is_advisory_and_review_still_passes() {
    let sb = Sandbox::with_toml(
        "[profiles]\nbasic = [\"skill:review\", \"agent:*\"]\nstar = [\"command:*\"]\n",
    );
    let r = sb.review();
    assert!(r.success, "advisories must not fail review: {}", r.stderr);
    let hit = lines(&r.stdout, "advisory [empty-profile-member]:");
    assert_eq!(hit.len(), 2, "stdout: {}", r.stdout);
    assert!(
        hit.iter()
            .any(|l| l.contains("profile 'basic'") && l.contains("agent:*")),
        "{hit:?}"
    );
    assert!(
        hit.iter()
            .any(|l| l.contains("profile 'star'") && l.contains("command:*")),
        "{hit:?}"
    );
}

// spec: PROF-30 PROF-3
#[test]
fn star_matching_nothing_is_an_empty_profile_member() {
    // A source with no items: `*` matches nothing.
    let sb = Sandbox::new();
    write(&sb.source.join("README.md"), "nothing here\n");
    write(&sb.source.join("mind.toml"), "[profiles]\nall = [\"*\"]\n");
    let r = sb.review();
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let hit = lines(&r.stdout, "advisory [empty-profile-member]:");
    assert_eq!(hit.len(), 1, "stdout: {}", r.stdout);
    assert!(hit[0].contains("'*'"), "{}", hit[0]);
}

// spec: PROF-30 PROF-3
#[test]
fn profile_expanding_to_nothing_is_an_empty_profile_advisory() {
    let sb = Sandbox::with_toml(
        "[profiles]\nnone = [\"command:*\"]\nwraps = [\"none\"]\nok = [\"skill:review\"]\n",
    );
    let r = sb.review();
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let hit = lines(&r.stdout, "advisory [empty-profile]:");
    assert_eq!(hit.len(), 2, "stdout: {}", r.stdout);
    assert!(hit.iter().any(|l| l.contains("'none'")), "{hit:?}");
    assert!(hit.iter().any(|l| l.contains("'wraps'")), "{hit:?}");
    assert!(!hit.iter().any(|l| l.contains("'ok'")), "{hit:?}");
}

// spec: PROF-30 PROF-2
#[test]
fn invalid_profile_name_is_a_hard_invalid_profile_finding() {
    let sb = Sandbox::with_toml("[profiles]\nBad = [\"skill:review\"]\n");
    let r = sb.review();
    assert!(!r.success);
    let hit = lines(&r.stderr, "error [invalid-profile]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("profile name 'Bad'"), "{}", hit[0]);
    assert!(!r.stderr.contains("toml-parse-error"), "{}", r.stderr);
}

// spec: PROF-30 PROF-3
#[test]
fn unknown_kind_and_undeclared_profile_members_are_invalid_profile() {
    let sb = Sandbox::with_toml("[profiles]\nbasic = [\"widget:x\"]\n");
    let r = sb.review();
    assert!(!r.success);
    let hit = lines(&r.stderr, "error [invalid-profile]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("unknown kind 'widget'"), "{}", hit[0]);

    let sb = Sandbox::with_toml("[profiles]\nfull = [\"nope\"]\n");
    let r = sb.review();
    assert!(!r.success);
    let hit = lines(&r.stderr, "error [invalid-profile]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("does not declare"), "{}", hit[0]);
}

// spec: PROF-30 PROF-4
#[test]
fn inclusion_cycle_is_a_hard_invalid_profile_finding() {
    let sb = Sandbox::with_toml("[profiles]\na = [\"b\"]\nb = [\"a\"]\n");
    let r = sb.review();
    assert!(!r.success);
    let hit = lines(&r.stderr, "error [invalid-profile]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("cycle: a -> b -> a"), "{}", hit[0]);
}

// spec: PROF-30
#[test]
fn json_output_carries_the_profile_findings() {
    let sb =
        Sandbox::with_toml("[profiles]\nbasic = [\"skill:missing\"]\nnone = [\"command:*\"]\n");
    let r = sb.review_with(&["--json"]);
    assert!(!r.success);
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(all.contains("bad-profile-member"), "{all}");
    assert!(all.contains("empty-profile-member"), "{all}");
    assert!(all.contains("empty-profile"), "{all}");
}

fn json_kinds(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .expect("findings array")
        .iter()
        .map(|f| f["kind"].as_str().expect("kind").to_string())
        .collect()
}

// spec: PROF-30
#[test]
fn json_failed_review_puts_profile_findings_in_error_details() {
    let sb =
        Sandbox::with_toml("[profiles]\nbasic = [\"skill:missing\"]\nnone = [\"command:*\"]\n");
    let r = sb.review_with(&["--json"]);
    assert!(!r.success);
    let v: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout is one JSON doc: {e}\n{}", r.stdout));
    assert_eq!(v["error"]["kind"], "review-failed", "{v}");
    let d = &v["details"];
    assert_eq!(d["action"], "review");
    assert_eq!(d["outcome"], "failed");
    assert_eq!(json_kinds(&d["hard"]), vec!["bad-profile-member"]);
    let hard = &d["hard"][0];
    assert!(
        hard["message"].as_str().unwrap().contains("skill:missing"),
        "{hard}"
    );
    let mut adv = json_kinds(&d["advisory"]);
    adv.sort();
    // `basic` (its only member is missing) and `none` both expand to nothing.
    assert_eq!(
        adv,
        vec!["empty-profile", "empty-profile", "empty-profile-member"]
    );
}

// spec: PROF-30 PROF-3
#[test]
fn json_advisory_only_review_succeeds_with_advisory_outcome() {
    let sb = Sandbox::with_toml("[profiles]\nnone = [\"command:*\"]\n");
    let r = sb.review_with(&["--json"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout is one JSON doc: {e}\n{}", r.stdout));
    assert_eq!(v["action"], "review", "{v}");
    assert_eq!(v["outcome"], "advisory", "{v}");
    assert!(v["hard"].as_array().unwrap().is_empty(), "{v}");
    let mut adv = json_kinds(&v["advisory"]);
    adv.sort();
    assert_eq!(adv, vec!["empty-profile", "empty-profile-member"]);
}

// spec: PROF-30 PROF-2
#[test]
fn json_invalid_profile_is_a_hard_finding_kind() {
    let sb = Sandbox::with_toml("[profiles]\na = [\"b\"]\nb = [\"a\"]\n");
    let r = sb.review_with(&["--json"]);
    assert!(!r.success);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON stdout");
    assert_eq!(json_kinds(&v["details"]["hard"]), vec!["invalid-profile"]);
}

// spec: PROF-30 PROF-5
#[test]
fn json_clean_profiles_report_clean_outcome() {
    let sb = Sandbox::with_toml("[profiles]\nbasic = [\"skill:review\"]\n");
    let r = sb.review_with(&["--json"]);
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON stdout");
    assert_eq!(v["outcome"], "clean", "{v}");
}

// spec: PROF-30 PROF-5
#[test]
fn exact_member_naming_an_item_of_the_wrong_kind_is_bad_profile_member() {
    // `skill:style` names the rule `style` under the wrong kind: not offered.
    let sb = Sandbox::with_toml("[profiles]\nbasic = [\"skill:style\"]\n");
    let r = sb.review();
    assert!(!r.success);
    let hit = lines(&r.stderr, "error [bad-profile-member]:");
    assert_eq!(hit.len(), 1, "stderr: {}", r.stderr);
    assert!(hit[0].contains("skill:style"), "{}", hit[0]);
}

// spec: PROF-30 PROF-3
#[test]
fn empty_glob_member_in_an_otherwise_nonempty_profile_is_not_an_empty_profile() {
    let sb = Sandbox::with_toml("[profiles]\nbasic = [\"skill:review\", \"agent:*\"]\n");
    let r = sb.review();
    assert!(r.success, "{}{}", r.stdout, r.stderr);
    assert_eq!(lines(&r.stdout, "[empty-profile-member]").len(), 1);
    assert!(
        lines(&r.stdout, "[empty-profile]").is_empty(),
        "{}",
        r.stdout
    );
}
