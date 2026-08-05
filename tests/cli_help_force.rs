//! `--force`'s `--help` text must name both of its effects: overwriting a
//! colliding target in snapshot mode, and overriding the managed backfill's
//! `ensure_unoccupied` guard (HARN-17). Regression test so the two doc
//! comments in src/cli.rs cannot silently drift back to naming only one.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

/// The result of running the real `mind` binary once.
struct Run {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-help-force-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Sandbox {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    fn help(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    }

    /// Run `mind <args>` against this sandbox and capture the outcome.
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
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

/// True when `text` carries a clap parse-error signature (an unrecognized
/// flag or subcommand), as opposed to an ordinary `MindError` surfaced after
/// successful parsing. Used to prove a renamed/aliased flag or subcommand was
/// actually accepted by the parser, not merely tolerated because the rest of
/// the command happened to fail first for an unrelated reason.
fn looks_like_a_clap_parse_error(text: &str) -> bool {
    text.contains("unexpected argument")
        || text.contains("unrecognized subcommand")
        || text.contains("error: unrecognized")
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn link_project_help_names_both_force_effects() {
    // spec: HARN-17 - --force overwrites a colliding snapshot target and also
    // overrides the backfill's ensure_unoccupied guard; --help must name both.
    let sb = Sandbox::new();
    let out = sb.help(&["link-project", "--help"]);
    assert!(
        out.contains("snapshot"),
        "link-project --help must still mention snapshot mode for --force: {out}"
    );
    assert!(
        out.contains("backfill"),
        "link-project --help must mention the backfill guard override for --force: {out}"
    );
}

#[test]
fn config_lobes_add_help_names_both_force_effects() {
    // spec: HARN-17 - same requirement for `config lobes add --force`.
    let sb = Sandbox::new();
    let out = sb.help(&["config", "lobes", "add", "--help"]);
    assert!(
        out.contains("snapshot"),
        "config lobes add --help must still mention snapshot mode for --force: {out}"
    );
    assert!(
        out.contains("backfill"),
        "config lobes add --help must mention the backfill guard override for --force: {out}"
    );
}

// ---- CLI-227: unmeld/forget's uninstall-hook consent flag is renamed -----

#[test]
fn unmeld_dangerously_skip_hook_check_help_shows_new_name() {
    // spec: CLI-227
    let sb = Sandbox::new();
    let out = sb.help(&["unmeld", "--help"]);
    assert!(
        out.contains("--dangerously-skip-hook-check"),
        "unmeld --help must show the renamed flag: {out}"
    );
}

#[test]
fn unmeld_accepts_new_flag_and_legacy_alias_both() {
    // spec: CLI-227 - the new spelling and the old (deprecated, hidden) alias
    // must both parse: a run against a nonexistent source must fail with the
    // ordinary SourceNotFound error, never a clap parse error, for either.
    let sb = Sandbox::new();
    let new_flag = sb.mind(&["unmeld", "no-such-source", "--dangerously-skip-hook-check"]);
    assert!(!new_flag.success, "{}", new_flag.stderr);
    assert!(
        !looks_like_a_clap_parse_error(&new_flag.stderr),
        "the new flag name must parse: {}",
        new_flag.stderr
    );
    assert!(
        new_flag.stderr.contains("no source named"),
        "must fail with SourceNotFound, not a parse error: {}",
        new_flag.stderr
    );

    let old_flag = sb.mind(&[
        "unmeld",
        "no-such-source",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(!old_flag.success, "{}", old_flag.stderr);
    assert!(
        !looks_like_a_clap_parse_error(&old_flag.stderr),
        "the legacy alias must still parse: {}",
        old_flag.stderr
    );
    assert!(
        old_flag.stderr.contains("no source named"),
        "legacy alias must fail with SourceNotFound, not a parse error: {}",
        old_flag.stderr
    );
}

#[test]
fn forget_dangerously_skip_hook_check_help_shows_new_name() {
    // spec: CLI-227
    let sb = Sandbox::new();
    let out = sb.help(&["forget", "--help"]);
    assert!(
        out.contains("--dangerously-skip-hook-check"),
        "forget --help must show the renamed flag: {out}"
    );
}

#[test]
fn forget_accepts_new_flag_and_legacy_alias_both() {
    // spec: CLI-227
    let sb = Sandbox::new();
    let new_flag = sb.mind(&[
        "forget",
        "skill:no-such-item",
        "--dangerously-skip-hook-check",
    ]);
    assert!(!new_flag.success, "{}", new_flag.stderr);
    assert!(
        !looks_like_a_clap_parse_error(&new_flag.stderr),
        "the new flag name must parse: {}",
        new_flag.stderr
    );
    assert!(
        new_flag.stderr.contains("is not installed"),
        "must fail with NotInstalled, not a parse error: {}",
        new_flag.stderr
    );

    let old_flag = sb.mind(&[
        "forget",
        "skill:no-such-item",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(!old_flag.success, "{}", old_flag.stderr);
    assert!(
        !looks_like_a_clap_parse_error(&old_flag.stderr),
        "the legacy alias must still parse: {}",
        old_flag.stderr
    );
    assert!(
        old_flag.stderr.contains("is not installed"),
        "legacy alias must fail with NotInstalled, not a parse error: {}",
        old_flag.stderr
    );
}

// ---- CLI-230: `unmeld` gains `remove`/`rm` visible aliases ----------------

#[test]
fn unmeld_remove_and_rm_aliases_route_to_unmeld() {
    // spec: CLI-230
    let sb = Sandbox::new();
    for verb in ["remove", "rm", "unmeld"] {
        let r = sb.mind(&[verb, "no-such-source"]);
        assert!(!r.success, "[{verb}] {}", r.stderr);
        assert!(
            !looks_like_a_clap_parse_error(&r.stderr),
            "[{verb}] must be a recognized subcommand: {}",
            r.stderr
        );
        assert!(
            r.stderr.contains("no source named"),
            "[{verb}] must route to unmeld's SourceNotFound: {}",
            r.stderr
        );
    }
}

// ---- CLI-228: `hooks run --rerun` is a visible alias for `--force` -------

#[test]
fn hooks_run_rerun_is_an_alias_for_force() {
    // spec: CLI-228
    let sb = Sandbox::new();
    let force = sb.mind(&["hooks", "run", "no-such-target", "--force"]);
    let rerun = sb.mind(&["hooks", "run", "no-such-target", "--rerun"]);
    assert!(
        !looks_like_a_clap_parse_error(&force.stderr),
        "--force must parse: {}",
        force.stderr
    );
    assert!(
        !looks_like_a_clap_parse_error(&rerun.stderr),
        "--rerun must parse: {}",
        rerun.stderr
    );
    assert_eq!(
        force.success, rerun.success,
        "--force and --rerun must behave identically: force={:?} rerun={:?}",
        force.stderr, rerun.stderr
    );
    assert_eq!(
        force.stderr, rerun.stderr,
        "--force and --rerun must produce byte-identical output (same field)"
    );
}

#[test]
fn hooks_run_help_mentions_rerun_alias() {
    // spec: CLI-228
    let sb = Sandbox::new();
    let out = sb.help(&["hooks", "run", "--help"]);
    assert!(
        out.contains("--rerun"),
        "hooks run --help must mention the --rerun alias: {out}"
    );
}

// ---- CLI-229: `evolve`'s pin-a-version flag is `--to`, not `--version` ---

#[test]
fn evolve_help_shows_to_flag() {
    // spec: CLI-229
    let sb = Sandbox::new();
    let out = sb.help(&["evolve", "--help"]);
    assert!(
        out.contains("--to"),
        "evolve --help must show the --to flag: {out}"
    );
}

#[test]
fn evolve_check_accepts_to_and_legacy_version_alias_both() {
    // spec: CLI-229 - both spellings resolve an explicit target version and
    // make no network call (an explicit target bypasses the GitHub API), so
    // this stays hermetic.
    let sb = Sandbox::new();
    let to_flag = sb.mind(&["evolve", "--check", "--to", "9.9.9"]);
    assert!(
        to_flag.success,
        "evolve --check --to 9.9.9 should succeed: {} {}",
        to_flag.stdout, to_flag.stderr
    );
    assert!(
        to_flag.stdout.contains("9.9.9") && to_flag.stdout.contains("available"),
        "expected the pinned version to be reported as available: {}",
        to_flag.stdout
    );

    let version_flag = sb.mind(&["evolve", "--check", "--version", "9.9.9"]);
    assert!(
        version_flag.success,
        "the legacy --version alias must still work: {} {}",
        version_flag.stdout, version_flag.stderr
    );
    assert!(
        version_flag.stdout.contains("9.9.9") && version_flag.stdout.contains("available"),
        "legacy alias must report the same outcome: {}",
        version_flag.stdout
    );
}

// ---- M10: no bare spec IDs or Rust enum names leak into --help text ------

#[test]
fn help_text_does_not_leak_spec_ids_or_enum_names() {
    // spec: CLI-206 CLI-209
    let sb = Sandbox::new();
    let cases: [&[&str]; 3] = [
        &["meld", "--help"],
        &["config", "lobes", "add", "--help"],
        &["recall", "--help"],
    ];
    for args in cases {
        let out = sb.help(args);
        assert!(
            !out.contains("CLI-209") && !out.contains("CLI-206") && !out.contains("DEP-61"),
            "[{}] --help text must not leak a bare spec ID: {out}",
            args.join(" ")
        );
        assert!(
            !out.contains("LobeTargetRequired"),
            "[{}] --help text must not leak a Rust enum name: {out}",
            args.join(" ")
        );
    }
}

// ---- M4: meld's --force doc mentions the hook-rerun behavior -------------

#[test]
fn meld_force_help_mentions_hook_rerun() {
    // spec: HOOK-60
    let sb = Sandbox::new();
    let out = sb.help(&["meld", "--help"]);
    assert!(
        out.contains("install hook"),
        "meld --help's --force doc must mention re-running install hooks: {out}"
    );
}

// ---- Item 11: EXAMPLES block on meld/learn/forget ------------------------

#[test]
fn meld_learn_forget_help_include_examples_block() {
    let sb = Sandbox::new();
    for verb in ["meld", "learn", "forget"] {
        let out = sb.help(&[verb, "--help"]);
        assert!(
            out.contains("EXAMPLES"),
            "[{verb}] --help must include an EXAMPLES block: {out}"
        );
    }
}
