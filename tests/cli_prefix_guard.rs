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

// spec: DSC-112
#[test]
fn an_alias_the_reserved_list_caught_up_with_warns_but_keeps_scanning() {
    // The reserved-word list is append-only: `workflow` joined it when the
    // workflow kind shipped, under sources already melded with that prefix.
    // A registry entry is never re-validated, so the prefix stays in effect;
    // the items are installed under it and failing every scanning verb would
    // take the user's whole lobe down over a naming problem. Warn instead.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let meld = sb.mind(&["meld", &spec, "--namespace", "acme", "--yes"]);
    assert!(meld.success, "setup meld must succeed: {}", meld.stderr);

    // Rewrite the recorded namespace prefix to what a pre-reservation binary
    // would have accepted. Nothing in mind can produce this state today, which
    // is exactly why the guard has to read the registry rather than trust the
    // ingress. Only the `alias` field (the namespace prefix) is rewritten:
    // `as_alias` (the instance-identity alias) is already revalidated on load
    // by STO-68, which drops the whole entry, and this is about the prefix that
    // survives into the scan.
    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).expect("read sources.json");
    let recorded = "\"alias\": \"acme\"";
    assert!(
        text.contains(recorded),
        "the namespace prefix must be recorded as `{recorded}` to be rewritten: {text}"
    );
    std::fs::write(&registry, text.replace(recorded, "\"alias\": \"workflow\"")).unwrap();

    let recall = sb.mind(&["recall"]);
    assert!(
        recall.success,
        "a reserved recorded prefix must not fail the scan: {} {}",
        recall.stdout, recall.stderr
    );
    assert!(
        recall.stderr.contains("workflow") && recall.stderr.contains("reserve"),
        "the scan must warn that the recorded prefix is now a reserved word: {}",
        recall.stderr
    );
    assert!(
        recall.stderr.contains("--namespace"),
        "the warning must name the way to rename it: {}",
        recall.stderr
    );
    // Advisory only: the item is still there, under the prefix it was
    // installed with.
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("workflow:review"),
        "the items must still be listed under the recorded prefix: {}",
        probe.stdout
    );
}

// spec: DSC-112
#[test]
fn a_melded_source_declaring_a_reserved_prefix_names_unmeld_as_the_remedy() {
    // The other half: a `[source].prefix` is re-validated at every mind.toml
    // load, so once the word is reserved the refusal is hard and lands on
    // every verb that scans the source. The consumer cannot override a value
    // the source declares, so the error has to name the source and the one
    // command that ends the condition, not the pre-meld "cannot be used as a
    // namespace prefix" wording.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let meld = sb.mind(&["meld", &spec, "--yes"]);
    assert!(meld.success, "setup meld must succeed: {}", meld.stderr);

    // The source declares the prefix AFTER the meld, standing in for a binary
    // that accepted the word when the meld happened. A local path source is
    // read from its working tree, so this is what the next scan sees.
    sb.declare_prefix("workflow");

    // `must_fail` is false for the verbs that already degrade per source rather
    // than aborting (upgrade reports the source it could not check). What every
    // verb owes the user is the same either way: the actionable message.
    for (verb, must_fail) in [
        (vec!["recall"], true),
        (vec!["probe", "--no-tui"], true),
        (vec!["learn", "review"], true),
        (vec!["upgrade", "--no-sync", "--yes"], false),
        (vec!["introspect"], false),
    ] {
        let r = sb.mind(&verb);
        assert!(
            !must_fail || !r.success,
            "{verb:?} must refuse a source declaring a now-reserved prefix: {} {}",
            r.stdout,
            r.stderr
        );
        let combined = format!("{}{}", r.stdout, r.stderr);
        assert!(
            combined.contains("mind unmeld"),
            "{verb:?}: the error must name `mind unmeld <source>` as the remedy: {combined}"
        );
        assert!(
            combined.contains("agents"),
            "{verb:?}: the error must name the source it is about: {combined}"
        );
        assert!(
            combined.contains("workflow"),
            "{verb:?}: the error must name the offending prefix: {combined}"
        );
    }

    // And the remedy works: after unmeld, the verbs run again.
    let unmeld = sb.mind(&["unmeld", "agents", "--yes"]);
    assert!(
        unmeld.success,
        "the named remedy must work: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.success,
        "after the remedy, scanning verbs must work again: {} {}",
        recall.stdout, recall.stderr
    );
}

/// The advisory warning is written by the SCAN, which runs under `--json` too.
/// A line on stdout there would corrupt the one document a `--json` caller
/// parses, so the warning has to be stderr-only and the document has to stay
/// whole.
// spec: DSC-112 CLI-217
#[test]
fn the_reserved_alias_warning_does_not_reach_the_json_document() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let meld = sb.mind(&["meld", &spec, "--namespace", "acme", "--yes"]);
    assert!(meld.success, "setup meld must succeed: {}", meld.stderr);
    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).expect("read sources.json");
    std::fs::write(
        &registry,
        text.replace("\"alias\": \"acme\"", "\"alias\": \"workflow\""),
    )
    .unwrap();

    // Two catalog-reading verbs with a JSON stdout contract, one of which
    // (`probe`) scans every source twice over in its listing path.
    for args in [
        vec!["--json", "recall"],
        vec!["--json", "probe", "--no-tui"],
    ] {
        let r = sb.mind(&args);
        assert!(
            r.success,
            "{args:?} must still succeed under the warning: {} {}",
            r.stdout, r.stderr
        );
        let doc: serde_json::Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
            panic!(
                "{args:?}: stdout must be exactly one JSON document ({e}): {}",
                r.stdout
            )
        });
        assert!(doc.is_object(), "{args:?}: {doc}");
        assert!(
            !r.stdout.contains("reserve"),
            "{args:?}: the warning must not be on stdout: {}",
            r.stdout
        );
        assert!(
            r.stderr.contains("reserve"),
            "{args:?}: the warning must still be emitted, on stderr: {}",
            r.stderr
        );
    }
}

/// The condition is advisory, so the verbs that touch a source's git state have
/// to keep working under it: an operator who cannot sync or upgrade is not
/// getting advice, they are locked out by a naming problem.
// spec: DSC-112
#[test]
fn a_reserved_alias_does_not_block_sync_or_upgrade() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let meld = sb.mind(&["meld", &spec, "--namespace", "acme", "--yes"]);
    assert!(meld.success, "setup meld must succeed: {}", meld.stderr);
    let registry = sb.mind_home.join("sources.json");
    let text = std::fs::read_to_string(&registry).expect("read sources.json");
    std::fs::write(
        &registry,
        text.replace("\"alias\": \"acme\"", "\"alias\": \"workflow\""),
    )
    .unwrap();

    for args in [
        vec!["sync"],
        vec!["upgrade", "--no-sync", "--yes"],
        vec!["introspect"],
    ] {
        let r = sb.mind(&args);
        assert!(
            r.success,
            "{args:?} must not be blocked by an advisory prefix warning: {} {}",
            r.stdout, r.stderr
        );
    }

    // The installed item keeps its identity throughout: the warning renames
    // nothing on its own, so nothing is orphaned or re-linked behind the
    // operator's back.
    assert!(
        sb.claude_home.join("skills/workflow:review").exists(),
        "the item must still be linked under the prefix it was installed with"
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("workflow:review"),
        "the item must still be reported: {}",
        recall.stdout
    );
}

/// The new variant is an error like any other, so a `--json` caller has to
/// reach it through the CLI-181 envelope with its own machine-readable kind,
/// not as a text line or as the generic `reserved-prefix` kind (which names a
/// different remedy).
// spec: DSC-112 CLI-181
#[test]
fn the_melded_reserved_prefix_error_has_its_own_json_envelope() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let meld = sb.mind(&["meld", &spec, "--yes"]);
    assert!(meld.success, "setup meld must succeed: {}", meld.stderr);
    sb.declare_prefix("workflow");

    let r = sb.mind(&["--json", "recall"]);
    assert!(!r.success, "the run must fail: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    assert_eq!(doc["schema"], 1, "the envelope must be schema 1: {doc}");
    assert_eq!(
        doc["error"]["kind"], "melded-source-reserved-prefix",
        "the melded-source case must be distinguishable from the pre-meld \
         `reserved-prefix` one, whose remedy is different: {doc}"
    );
    let msg = doc["error"]["message"].as_str().unwrap_or_default();
    for needle in ["agents", "workflow", "mind unmeld"] {
        assert!(
            msg.contains(needle),
            "the envelope message must name '{needle}': {doc}"
        );
    }
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
