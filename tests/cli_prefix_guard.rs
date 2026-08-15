//! End-to-end coverage of the NS-72/NS-73 prefix-safety guard on its two live
//! ingress points: `meld --namespace`/`-N` (a user-supplied prefix) and a
//! melded repo's `mind.toml` `[source].prefix` (a source-declared prefix).
//! Both funnel through `namespace::validate_prefix`, which rejects a prefix
//! carrying a security-blocked Unicode code point with a structured
//! `UnsafePrefix` error (spec/namespacing.md NS-72, NS-73).
//!
//! See CLAUDE.md: manual checks must be encoded as tests unless genuinely
//! impossible to automate. This suite drives the real `mind` binary against a
//! hermetic local-git fixture, no network.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A throwaway environment: a source git repo plus isolated MIND_HOME/CLAUDE_HOME.
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
    /// A source repo with one skill (`review`), committed.
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-prefix-guard-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("agents");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(
            &source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\n",
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
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
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

    /// Write `mind.toml` at the source repo root and commit it, declaring
    /// `[source].prefix = <prefix>`. `prefix` is written as literal UTF-8, so
    /// a caller-supplied blocked Unicode code point lands in the TOML value
    /// as-is (a real character, not an escape sequence) -- exactly the
    /// payload the declared-prefix ingress must refuse.
    fn declare_prefix(&self, prefix: &str) {
        write(
            &self.source.join("mind.toml"),
            &format!("[source]\nprefix = \"{prefix}\"\n"),
        );
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "declare prefix"]);
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

/// The generic cause wording `unsafe_prefix_message` uses for the blocked
/// Unicode class, shared by every assertion below rather than pinned to a
/// specific code point, per the coupling with error.rs's generic ("an
/// invisible, bidi, or zero-width character") wording.
const BLOCKED_UNICODE_CAUSE: &str = "invisible, bidi, or zero-width character";

// spec: NS-72
#[test]
fn meld_namespace_with_bidi_override_is_refused_as_unsafe_prefix() {
    // The DSC-94-era code point (U+202E, a bidi override): already blocked
    // before NS-73 broadened the set, pinning the baseline `meld --namespace`
    // path still works.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let bad_prefix = format!("pay{}oot", '\u{202E}');
    let r = sb.mind(&["meld", &spec, "--namespace", &bad_prefix, "--yes"]);
    assert!(
        !r.success,
        "meld --namespace with a bidi-override prefix must be refused"
    );
    assert!(
        r.stderr.contains(BLOCKED_UNICODE_CAUSE),
        "UnsafePrefix cause must be reported generically: {}",
        r.stderr
    );
    // Nothing installs: the guard must fire before the source is registered.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        !recall.contains("review"),
        "a refused meld must not install anything: {recall}"
    );
}

// spec: NS-72
#[test]
fn meld_namespace_short_flag_with_bidi_override_is_refused() {
    // The `-N` short form goes through the identical `validate_prefix`
    // chokepoint as `--namespace`.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let bad_prefix = format!("pay{}oot", '\u{202E}');
    let r = sb.mind(&["meld", &spec, "-N", &bad_prefix, "--yes"]);
    assert!(
        !r.success,
        "meld -N with a bidi-override prefix must be refused"
    );
    assert!(r.stderr.contains(BLOCKED_UNICODE_CAUSE), "{}", r.stderr);
}

// spec: NS-73
#[test]
fn meld_namespace_with_tag_block_character_is_refused_as_unsafe_prefix() {
    // The M5/NS-73 broadening's headline addition: a Unicode tag-block code
    // point (U+E0041, TAG LATIN SMALL LETTER A) renders as nothing at a
    // terminal, so a prefix carrying it would look clean while smuggling an
    // invisible payload into every namespaced ref. Refused the same way as a
    // bidi override.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let bad_prefix = format!("acme{}", '\u{E0041}');
    let r = sb.mind(&["meld", &spec, "--namespace", &bad_prefix, "--yes"]);
    assert!(
        !r.success,
        "meld --namespace with a tag-block character in the prefix must be refused"
    );
    assert!(
        r.stderr.contains(BLOCKED_UNICODE_CAUSE),
        "UnsafePrefix cause must be reported generically: {}",
        r.stderr
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        !recall.contains("review"),
        "a refused meld must not install anything: {recall}"
    );
}

// spec: NS-73
#[test]
fn meld_namespace_with_variation_selector_is_refused_as_unsafe_prefix() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let bad_prefix = format!("acme{}", '\u{FE0F}');
    let r = sb.mind(&["meld", &spec, "--namespace", &bad_prefix, "--yes"]);
    assert!(
        !r.success,
        "meld --namespace with a variation-selector character in the prefix must be refused"
    );
    assert!(r.stderr.contains(BLOCKED_UNICODE_CAUSE), "{}", r.stderr);
}

// spec: NS-72
#[test]
fn meld_declared_prefix_with_bidi_override_is_refused_on_load() {
    // A repo's own `mind.toml [source].prefix` carrying a blocked character is
    // rejected at mind.toml load time (mindfile.rs), independent of whether
    // the run is interactive -- so a non-TTY test harness still observes the
    // refusal, not a silent "no prefix" fallback.
    let sb = Sandbox::new();
    let bad_prefix = format!("pay{}oot", '\u{202E}');
    sb.declare_prefix(&bad_prefix);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(
        !r.success,
        "meld of a source declaring an unsafe [source].prefix must be refused"
    );
    assert!(
        r.stderr.contains(BLOCKED_UNICODE_CAUSE),
        "UnsafePrefix cause must be reported generically: {}",
        r.stderr
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        !recall.contains("review"),
        "a refused meld must not install anything: {recall}"
    );
}

// spec: NS-73
#[test]
fn meld_declared_prefix_with_tag_block_character_is_refused_on_load() {
    let sb = Sandbox::new();
    let bad_prefix = format!("acme{}", '\u{E0041}');
    sb.declare_prefix(&bad_prefix);
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(
        !r.success,
        "meld of a source declaring a tag-block [source].prefix must be refused"
    );
    assert!(
        r.stderr.contains(BLOCKED_UNICODE_CAUSE),
        "UnsafePrefix cause must be reported generically: {}",
        r.stderr
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        !recall.contains("review"),
        "a refused meld must not install anything: {recall}"
    );
}

// spec: NS-72 NS-73
#[test]
fn meld_namespace_with_clean_prefix_still_succeeds() {
    // Control: a prefix with no blocked characters is unaffected by the
    // broadened guard.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--namespace", "acme", "--yes"]);
    assert!(
        r.success,
        "a clean prefix must still be accepted: {}",
        r.stderr
    );
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("acme:review"),
        "the clean prefix must apply normally: {recall}"
    );
}
