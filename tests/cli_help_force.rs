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
        new_flag.stderr.contains("no melded source matches"),
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
        old_flag.stderr.contains("no melded source matches"),
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
            r.stderr.contains("no melded source matches"),
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

// ---- M10 / M-test7: no bare spec IDs or Rust enum names leak into --help --
//
// This used to hardcode three commands (`meld`, `config lobes add`, `recall`)
// and three literal IDs (`CLI-209`, `CLI-206`, `DEP-61`), so it passed both
// before and after a batch that dropped `(CLI-50)`/`(CLI-169)` from `sync`'s
// doc comments -- nothing was actually gating that drift. Instead, walk the
// real subcommand tree (discovered from each level's own `--help` output, not
// a hardcoded list) and scan every page's full text for anything shaped like
// a bare spec ID (`[A-Z]{2,4}-\d+`, e.g. `CLI-209`), plus the one known enum
// leak. A future verb or nesting level is covered automatically because the
// tree is walked structurally, not enumerated by name.

/// Whether `token` (already isolated on non-alnum/non-hyphen boundaries) has
/// the shape of a bare spec ID: 2-4 uppercase ASCII letters, one hyphen, then
/// one or more ASCII digits, and nothing else (anchored, not a substring
/// match) -- e.g. `CLI-209`, `HARN-20`, `DEP-61`.
fn looks_like_a_spec_id(token: &str) -> bool {
    let Some(dash) = token.find('-') else {
        return false;
    };
    // Exactly one hyphen: reject anything with a second one so a compound
    // token (there are none among current false positives, but this keeps
    // the shape strict) does not slip through.
    if token[dash + 1..].contains('-') {
        return false;
    }
    let (letters, rest) = token.split_at(dash);
    let digits = &rest[1..];
    (2..=4).contains(&letters.len())
        && !letters.is_empty()
        && letters.chars().all(|c| c.is_ascii_uppercase())
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
}

/// Known legitimate strings that match [`looks_like_a_spec_id`]'s shape by
/// coincidence (e.g. a standards name). None appear in `--help` text today
/// (verified by walking the full command tree while writing this test), but
/// if one is ever added deliberately, name it here explicitly rather than
/// loosening `looks_like_a_spec_id`'s letter/digit rule to make room for it.
const KNOWN_NON_SPEC_ID_TOKENS: &[&str] = &[];

/// Every maximal run of ASCII alphanumerics/hyphens in `text` that looks like
/// a bare spec ID and is not an explicitly known false positive.
fn leaked_spec_id_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            current.push(c);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
        .into_iter()
        .filter(|t| looks_like_a_spec_id(t) && !KNOWN_NON_SPEC_ID_TOKENS.contains(&t.as_str()))
        .collect()
}

/// The direct child subcommand names listed under a `Commands:` heading in a
/// `--help` page (e.g. `mind config --help` lists `show`, `lobes`, `help`).
/// `help` itself is excluded -- it is clap's built-in help-of-subcommand
/// pseudo-command, not part of the surface under test.
fn direct_subcommand_names(help_text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in help_text.lines() {
        if line.trim_end() == "Commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(name) = line.split_whitespace().next()
            && name != "help"
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Walk the whole `mind` subcommand tree, starting at the root, following
/// every nested `Commands:` listing (e.g. `config` -> `lobes` -> `add`).
/// Returns each command path (e.g. `["config", "lobes", "add"]`, empty for
/// the root) paired with its full `--help` text.
fn collect_all_help_pages(sb: &Sandbox) -> Vec<(Vec<String>, String)> {
    let mut pages = Vec::new();
    let mut queue: Vec<Vec<String>> = vec![Vec::new()];
    while let Some(path) = queue.pop() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let text = sb.help(&args);
        for name in direct_subcommand_names(&text) {
            let mut child = path.clone();
            child.push(name);
            queue.push(child);
        }
        pages.push((path, text));
    }
    pages
}

#[test]
fn help_text_does_not_leak_spec_ids_or_enum_names() {
    // spec: CLI-206 CLI-209
    let sb = Sandbox::new();
    let pages = collect_all_help_pages(&sb);
    // Sanity: the walk must actually have discovered the real tree (top-level
    // verbs plus nested `config lobes ...` / `hooks ...`), not just the root,
    // or this test would vacuously pass by scanning nothing.
    assert!(
        pages.len() > 20,
        "expected to discover the full command tree (>20 pages), found {}: {:?}",
        pages.len(),
        pages.iter().map(|(p, _)| p.join(" ")).collect::<Vec<_>>()
    );
    for (path, text) in &pages {
        let label = if path.is_empty() {
            "mind".to_string()
        } else {
            format!("mind {}", path.join(" "))
        };
        let leaked = leaked_spec_id_tokens(text);
        assert!(
            leaked.is_empty(),
            "[{label}] --help text must not leak a bare spec ID, found {leaked:?}: {text}"
        );
        assert!(
            !text.contains("LobeTargetRequired"),
            "[{label}] --help text must not leak a Rust enum name: {text}"
        );
    }
}

#[test]
fn looks_like_a_spec_id_matches_real_ids_and_rejects_lookalikes() {
    // spec: CLI-206 CLI-209 -- unit-level guard on the matcher itself: proves
    // the recursive scan above would actually catch a real ID (it fails on
    // unfixed code that reintroduces one) and does not false-positive on
    // ordinary hyphenated tokens that show up in --help prose.
    for real in ["CLI-209", "CLI-206", "DEP-61", "HARN-20", "TUI-1", "NS-40"] {
        assert!(looks_like_a_spec_id(real), "{real} should match");
    }
    for not_id in [
        "self-update",
        "dry-run",
        "owner-repo",
        "9-9",
        "TOOLONGPREFIX-1",
        "A-1",
        "CLI-",
        "CLI",
        "CLI-20-30",
    ] {
        assert!(!looks_like_a_spec_id(not_id), "{not_id} should not match");
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

// ---- M16: `sync`'s about-line and `[source]` selector doc drift -----------

#[test]
fn sync_about_line_mentions_the_source_selector() {
    // spec: CLI-172 CLI-231 -- M16(a): the about-line shown both in `mind
    // --help` and as `mind sync --help`'s header must not contradict the
    // `[source]` selector declared directly beneath it. A user scanning
    // `mind --help` for "sync one source" must find a hint here.
    let sb = Sandbox::new();
    let top = sb.help(&["--help"]);
    let sync_line = top
        .lines()
        .find(|l| l.trim_start().starts_with("sync "))
        .unwrap_or_else(|| panic!("mind --help must list sync: {top}"));
    assert!(
        sync_line.contains("source") || sync_line.contains("[source]"),
        "sync's about-line in `mind --help` must mention the selector it takes: {sync_line}"
    );

    let sync_help = sb.help(&["sync", "--help"]);
    let about = sync_help
        .lines()
        .next()
        .expect("sync --help must have a first line");
    assert!(
        about.contains("source"),
        "`mind sync --help`'s header must not contradict the [source] selector \
         declared beneath it: {about}"
    );
}

#[test]
fn sync_source_selector_help_does_not_claim_ambiguity_is_rejected() {
    // spec: CLI-231 -- M16(b): unlike `unmeld`/`upgrade`, `sync` has no
    // `AmbiguousSource` check -- it syncs EVERY source a selector matches
    // (`source_matches_glob`, applied with no uniqueness requirement). The
    // selector's help text must not borrow unmeld's "unambiguous trailing
    // suffix" wording, which describes a hard-error semantics `sync` does not
    // have.
    let sb = Sandbox::new();
    let out = sb.help(&["sync", "--help"]);
    assert!(
        !out.contains("unambiguous"),
        "sync --help must not claim its selector requires an unambiguous match \
         (sync has no AmbiguousSource check, unlike unmeld/upgrade): {out}"
    );
    assert!(
        out.contains("trailing suffix"),
        "sync --help should still describe the trailing-suffix selector form: {out}"
    );
}

#[test]
fn sync_source_selector_help_notes_the_skipped_whole_set_steps() {
    // spec: CLI-231 -- M16(c): CLI-231 states that a `[source]` selector
    // skips the whole-set-only operations (managed-policy auto-meld
    // provisioning, the nested-source re-walk). Nothing user-visible said so
    // before this fix; an admin scoping `mind sync corp-skills` in a runbook
    // must be told provisioning is skipped, not left to discover it silently.
    let sb = Sandbox::new();
    let out = sb.help(&["sync", "--help"]);
    assert!(
        out.contains("skip") && out.contains("provisioning"),
        "sync --help's [source] selector doc must note that a selector skips \
         managed-policy auto-meld provisioning: {out}"
    );
}

#[test]
fn sync_upgrade_help_notes_the_pass_is_scoped_to_the_selector() {
    // spec: CLI-169 -- H2: `--upgrade`'s help must describe the pass as
    // scoped to the source(s) the `[source]` selector matched, matching the
    // fixed (scoped) behavior rather than the old unscoped one.
    let sb = Sandbox::new();
    let out = sb.help(&["sync", "--help"]);
    let upgrade_block = out
        .split("--upgrade")
        .nth(1)
        .expect("sync --help must document --upgrade");
    assert!(
        upgrade_block.contains("scoped") || upgrade_block.contains("selector matched"),
        "sync --help's --upgrade doc must say the pass is scoped to the \
         selected source(s): {out}"
    );
    assert!(
        out.contains("deprecated") || out.contains("prefer"),
        "sync --help's --upgrade doc must keep the existing deprecation note \
         (CLI-169): {out}"
    );
}

// ---- L20: `evolve --check`'s help must cover the prerelease rule ----------

#[test]
fn evolve_help_mentions_prerelease_offered_its_matching_release() {
    // spec: CLI-140 -- after CLI-140, a prerelease/dev build is offered its
    // own base release even though the numeric version matches (e.g. `mind
    // 0.23.1-dev -> 0.23.1 available`), which otherwise reads like a bug.
    // `evolve --help` must say so.
    let sb = Sandbox::new();
    let out = sb.help(&["evolve", "--help"]);
    assert!(
        out.to_lowercase().contains("prerelease") || out.contains("-dev"),
        "evolve --help must mention that a prerelease/dev build is offered \
         its matching release: {out}"
    );
}
