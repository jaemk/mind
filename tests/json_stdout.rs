//! CLI-217 suite-wide gate: under `--json`, stdout must carry exactly one JSON
//! document and nothing else, for every verb -- not just the specific HOOK-106
//! repro that motivated the fix. Three defects this round were independently
//! discovered instances of the same class (an unguarded `println!` on a path
//! that runs before the verb's JSON output), each fixed alongside the feature
//! that introduced it. This file is the structural check that should have
//! caught all three: it drives each verb into a state where an advisory note
//! (or an unconverted progress line, for the known leaks below) actually
//! fires, then asserts stdout parses as a single JSON document. Driving only
//! the happy path would pass vacuously, since a clean run never exercises the
//! `render::note`/`render::warn` chokepoint (or its unconverted look-alikes)
//! at all.
//!
//! Every test here drives the real `mind` binary against a hermetic,
//! network-free fixture (a local git repo melded by filesystem path), with
//! `MIND_HOME`/`CLAUDE_HOME` pointed at a per-test temp dir. No network.

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
    /// A source repo named `name`, initially just a README (fixture content is
    /// added per-test via `write_and_commit`).
    fn new(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-js-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(&source.join("README.md"), "# fixture\n");
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
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

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// Write `~/.mind/config.toml` (the sandbox's isolated mind home) directly,
    /// ahead of any command that reads config (lobes, etc).
    fn write_config(&self, body: &str) {
        write(&self.mind_home.join("config.toml"), body);
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

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

/// The CLI-217 assertion itself: `stdout` (trimmed) must parse as exactly one
/// JSON document and nothing else. `serde_json::from_str` requires the whole
/// input to be consumed by a single value -- any leading prose (a `println!`
/// ahead of the envelope) or trailing bytes (a second document) is a
/// "trailing characters" / syntax parse error here, not silently ignored.
fn assert_stdout_is_one_json_document(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as exactly one JSON document \
             and nothing else ({e}): {stdout:?}"
        )
    })
}

// ---------------------------------------------------------------------------
// PASSING: verbs whose `--json` stdout is exactly one document even when an
// advisory note fires along the way (CLI-217 already covers these paths).
// ---------------------------------------------------------------------------

/// `hooks run`: the HOOK-106 skip note (a `note:`-prefixed line, converted in
/// this shard) fires ahead of the HOOK-107 error; stdout must still be only
/// the CLI-181 envelope.
#[test]
fn hooks_run_skip_note_then_error_is_one_document() {
    // spec: CLI-217 HOOK-106 HOOK-107 CLI-181
    let sb = Sandbox::new("hooks-note");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "hooks", "run", "--event", "install", "hooks-note"]);
    assert!(
        !r.success,
        "the HOOK-107 path must fail: {}\n{}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["error"]["kind"], "hooks-not-run", "{v}");
}

/// `learn`: a lobe whose parent directory does not exist fires the HARN-16
/// "note: lobe '...' is unreachable" note (the exact example CLI-217's spec
/// text cites) during install, but the item still installs successfully into
/// the reachable lobe. Stdout must be only the CLI-153 success envelope.
#[test]
fn learn_unreachable_lobe_note_then_success_is_one_document() {
    // spec: CLI-217 HARN-16 CLI-153
    let sb = Sandbox::new("lobe-note");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\n# widget\n",
    );
    // Two lobes: the real (reachable) claude_home, plus one whose PARENT
    // directory does not exist on disk at all.
    let unreachable = sb.base.join("does-not-exist-parent").join("lobe");
    sb.write_config(&format!(
        "lobes = [\"{}\", \"{}\"]\n",
        sb.claude_home.display(),
        unreachable.display()
    ));
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "learn", "widget"]);
    assert!(
        r.success,
        "learn must still succeed: {}\n{}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["outcome"], "installed", "{v}");
    assert!(
        r.stderr.contains("is unreachable"),
        "the HARN-16 note must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("unreachable"),
        "the note must not also land on stdout: {:?}",
        r.stdout
    );
}

/// `meld`: a source install hook skipped for want of a terminal fires the
/// (already-converted) "note: skipped install hook ..." note, but meld itself
/// still succeeds (a skip does not abort). Stdout must be only the envelope.
#[test]
fn meld_skipped_install_hook_note_is_one_document() {
    // spec: CLI-217 HOOK-56 CLI-153
    let sb = Sandbox::new("meld-note");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"echo ran\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["--json", "meld", &sb.source_spec(), "--register-only"]);
    assert!(
        r.success,
        "meld must succeed on a skip: {}\n{}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "meld", "{v}");
    assert!(
        r.stderr.contains("skipped install hook"),
        "the note must be routed to stderr: {:?}",
        r.stderr
    );
}

/// `unmeld`: an uninstall hook skipped for want of a terminal fires the
/// (already-converted) "note: skipped uninstall hook ..." note. Stdout must
/// be only the envelope.
#[test]
fn unmeld_skipped_uninstall_hook_note_is_one_document() {
    // spec: CLI-217 HOOK-87 CLI-153
    let sb = Sandbox::new("unmeld-note");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"teardown\"\n",
            "run = \"echo done\"\n",
            "event = \"uninstall\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "unmeld", "unmeld-note", "--keep-items"]);
    assert!(
        r.success,
        "unmeld must succeed on a skip: {}\n{}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "unmeld", "{v}");
    assert!(
        r.stderr.contains("skipped uninstall hook"),
        "the note must be routed to stderr: {:?}",
        r.stderr
    );
}

/// `config lobes add`: the HARN-17 backfill note ("could not link ...", a
/// `render::warn` call) fires when an already-installed item's target in the
/// new lobe is occupied by a foreign file, but the lobe is still added
/// successfully (the blocked backfill is advisory, not fatal).
#[test]
fn config_lobes_add_backfill_warn_is_one_document() {
    // spec: CLI-217 HARN-17 CLI-153
    let sb = Sandbox::new("lobe-add-note");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\n# widget\n",
    );
    sb.write_config(&format!("lobes = [\"{}\"]\n", sb.claude_home.display()));
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "widget"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    // A foreign (non-mind) file occupies the new lobe's target for `widget`,
    // blocking the backfill without --force.
    let new_lobe = sb.base.join("newlobe");
    write(&new_lobe.join("skills/widget"), "not mind's file\n");

    let r = sb.mind(&[
        "--json",
        "config",
        "lobes",
        "add",
        "--yes",
        &new_lobe.to_string_lossy(),
    ]);
    assert!(
        r.success,
        "adding the lobe succeeds even though one backfill was blocked: {}\n{}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "lobe-add", "{v}");
    assert!(
        r.stderr.contains("could not link"),
        "the HARN-17 warn must be routed to stderr: {:?}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// FORMERLY-LEAKING PROGRESS LINES. These `println!` sites carry no
// `note:`/`warning:` marker and are not composed with `out.warn()`, so the
// earlier round's decidable scope rule ("convert any println! that begins
// `note:`/`warning:`") did not select them -- and a marker-prefix rule is
// precisely why three defects of this class shipped. All are reachable only
// via a `--dangerously-skip-*` flag (a TTY is otherwise required to reach the
// branch that prints them), i.e. the unattended CI invocation that most wants
// `--json`.
//
// None of them was converted call-site by call-site. Under `--json` the
// process now runs with fd 1 pointed at stderr for the whole run (main.rs's
// `json_stdout`), so a `println!` ANYWHERE -- including in a module nobody
// looked at, and including a child process that inherits stdout -- cannot
// reach it; `main` writes the one document to the preserved stdout at the end.
// Each test therefore also asserts the line is still EMITTED on stderr:
// deleting the progress lines would satisfy the parse and lose the output.
// ---------------------------------------------------------------------------

/// `commands.rs` (`run_install_hooks`, reached by `meld`):
/// `println!("running install hook '{}' for {}", ...)`.
#[test]
fn meld_running_install_hook_is_one_document() {
    // spec: CLI-217 CLI-153
    let sb = Sandbox::new("meld-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"setup\"\n",
            "run = \"true\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&[
        "--json",
        "meld",
        &sb.source_spec(),
        "--register-only",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld: {}\n{}", r.stdout, r.stderr);
    assert_stdout_is_one_json_document(&r.stdout);
    assert!(
        r.stderr.contains("running install hook"),
        "the progress line must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
}

/// `commands.rs` (`run_uninstall_hooks`, reached by `unmeld`):
/// `println!("running uninstall hook '{}' for {}", ...)`.
#[test]
fn unmeld_running_uninstall_hook_is_one_document() {
    // spec: CLI-217 CLI-153
    let sb = Sandbox::new("unmeld-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"teardown\"\n",
            "run = \"true\"\n",
            "event = \"uninstall\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "unmeld",
        "unmeld-leak",
        "--keep-items",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "unmeld: {}\n{}", r.stdout, r.stderr);
    assert_stdout_is_one_json_document(&r.stdout);
    assert!(
        r.stderr.contains("running uninstall hook"),
        "the progress line must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
}

/// `install.rs` (`run_item_build_hook`, reached by `learn`):
/// `println!("running build hook for {}", ...)`.
#[test]
fn learn_running_build_hook_is_one_document() {
    // spec: CLI-217 CLI-153
    // `build` is valid only on a tool item (catalog.rs's `tool_field` gate), so
    // this needs an explicit `mind.toml` [[items]] declaration rather than a
    // skill's frontmatter.
    let sb = Sandbox::new("learn-build-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"builder\"\n",
            "path = \"tools/builder\"\n",
            "build = \"true\"\n",
        ),
    );
    sb.write_and_commit("tools/builder/run.sh", "#!/bin/sh\necho hi\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "learn",
        "builder",
        "--dangerously-skip-build-hook-check",
    ]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert_stdout_is_one_json_document(&r.stdout);
    assert!(
        r.stderr.contains("running build hook"),
        "the progress line must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
}

/// `install.rs` (`run_item_hook`, reached by `learn`):
/// `println!("running {event} hook for {key}", ...)` for an item's own
/// install hook (distinct from the source-level hook in `commands.rs`).
#[test]
fn learn_running_item_install_hook_is_one_document() {
    // spec: CLI-217 CLI-153
    // An item install hook is valid on any kind, but only via an explicit
    // `mind.toml` [[items]] declaration -- convention discovery has no
    // `[[items.hooks]]` array to read (DSC-21).
    let sb = Sandbox::new("learn-item-hook-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"widget\"\n",
            "path = \"widget\"\n",
            "install = \"true\"\n",
        ),
    );
    sb.write_and_commit("widget/SKILL.md", "---\ndescription: d\n---\n# widget\n");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "learn",
        "widget",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);
    assert_stdout_is_one_json_document(&r.stdout);
    assert!(
        r.stderr.contains("running install hook for"),
        "the progress line must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
}

/// `src/hooks_cmd.rs`'s `println!("running {event} hook '{}' for {}", ...)`
/// (in `run_source_hooks`), demonstrated end-to-end here too (see
/// `tests/cli_hooks.rs::hooks_run_running_hook_note_is_one_document` for the
/// canonical, more detailed version of this same site).
#[test]
fn hooks_run_running_hook_is_one_document() {
    // spec: CLI-217 CLI-181
    let sb = Sandbox::new("hooks-running-leak");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"first\"\n",
            "run = \"true\"\n",
            "event = \"install\"\n",
            "[[hooks]]\n",
            "name = \"second\"\n",
            "run = \"false\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "hooks-running-leak",
    ]);
    assert!(
        !r.success,
        "second hook must fail the run: {}\n{}",
        r.stdout, r.stderr
    );
    assert_stdout_is_one_json_document(&r.stdout);
    assert!(
        r.stderr.contains("running install hook"),
        "the progress line must be routed to stderr, not dropped: {:?}",
        r.stderr
    );
}

/// A source-supplied hook's OWN output is the worst case in this class: it is
/// arbitrary text chosen by the source author, so if it can reach `--json`
/// stdout then a consumer's stdout can be made to contain anything -- including
/// a well-formed forgery of the result envelope. Nothing about a marker-prefix
/// or call-site rule can bound that; only the redirect can. `hooks run --event
/// install` with the skip check disabled runs the hook for real.
#[test]
fn a_hooks_own_output_cannot_reach_json_stdout() {
    // spec: CLI-217 HOOK-30
    let sb = Sandbox::new("hook-output");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"loud\"\n",
            // Forge a plausible second document, on both of the hook's streams.
            "run = \"echo '{\\\"schema\\\":1,\\\"action\\\":\\\"forged\\\"}'; \
             echo to-stderr >&2\"\n",
            "event = \"install\"\n",
        ),
    );
    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&[
        "--json",
        "hooks",
        "run",
        "--event",
        "install",
        "--dangerously-skip-install-hook-check",
        "hook-output",
    ]);
    assert!(r.success, "hooks run: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("forged"),
        "a hook's stdout must never reach --json stdout: {:?}",
        r.stdout
    );
    assert!(
        r.stderr.contains("forged") && r.stderr.contains("to-stderr"),
        "both of the hook's streams must still be shown, on stderr: {:?}",
        r.stderr
    );
}

/// `learn <url>` (item-link, LNK-1) is a "last-write-wins" case CLI-217's own
/// prose calls out for a nested emitter: `learn_link` registers the link
/// instance via a plain `commands::meld(...)` call (which defers ALL JSON
/// emission to main.rs's dispatcher and so prints nothing itself) and then
/// falls through to the real `learn(...)`, which DOES print the CLI-153
/// result under `--json`. So the single document on stdout must be `learn`'s,
/// not a stray `meld` object and not two objects.
#[test]
fn learn_link_json_answers_with_learn_not_meld() {
    // spec: CLI-217 CLI-153 LNK-1
    let sb = Sandbox::new("link-src");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\n# widget\n",
    );
    let url = format!(
        "file://{}/tree/main/skills/widget",
        sb.source.to_string_lossy()
    );

    let r = sb.mind(&["--json", "learn", &url]);
    assert!(r.success, "learn <url> --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(
        v["action"], "learn",
        "the item-link install answers as `learn`, not the internal `meld` \
         registration step: {v}"
    );
    assert_eq!(v["outcome"], "installed", "{v}");
}

/// `absorb --yes --json` claims an unmanaged item into a managed source and,
/// when that destination is not yet melded, registers it through a nested
/// `commands::meld(...)` call before doing its own `learn_collecting` install
/// and printing its OWN `absorb` result (ABS-11). Same nested-emitter shape
/// as `learn_link` above, so it gets the same check: exactly one document,
/// naming the verb actually invoked.
#[test]
fn absorb_json_answers_with_absorb_not_meld() {
    // spec: CLI-217 CLI-153 ABS-11
    let sb = Sandbox::new("absorb-src");
    // An UNMANAGED item: a plain file dropped directly into the claude home,
    // never installed by `mind learn`.
    write(
        &sb.claude_home.join("skills/orphan/SKILL.md"),
        "---\ndescription: not managed by mind\n---\n# orphan\n",
    );
    // ABS-5: the destination must already be a git repo; `--to` does not
    // auto-init one (only the built-in personal-dir prompt path does).
    let dest = sb.base.join("absorb-dest");
    std::fs::create_dir_all(&dest).unwrap();
    git(&dest, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&dest, &["config", "user.email", "t@t"]);
    git(&dest, &["config", "user.name", "t"]);

    let r = sb.mind(&[
        "--json",
        "absorb",
        "skill:orphan",
        "--to",
        &dest.to_string_lossy(),
        "--yes",
    ]);
    assert!(r.success, "absorb --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(
        v["action"], "absorb",
        "absorb answers as `absorb`, not the internal `meld` step that \
         registers its (freshly created) destination: {v}"
    );
    assert_eq!(v["outcome"], "absorbed", "{v}");
}

// ---------------------------------------------------------------------------
// CLI-218/CLI-219/CLI-220/CLI-221/CLI-222: `--json` is universal. `review`
// (both the `<target>` and `--policy` modes) and `hooks list` used to have no
// JSON output at all -- a documented gap -- and `hooks run` answered a
// SUCCESSFUL run with nothing on stdout. All three now answer with a
// document, closing the gap this file used to describe as permanent.
// `dump`/`completions`/`man`/`evolve`/`init-source` remain the closed
// exclusion list (CLI-218): `dump` always writes TOML by design (DUMP-9) and
// already routes its one note (`--json does not apply to dump`) directly to
// stderr via `eprintln!`; `completions`/`man` print the artifact itself;
// `evolve` writes its own document from `selfupdate.rs` on a separate path
// (untested here: it needs a real or faked release endpoint, already covered
// hermetically in `src/selfupdate.rs`'s fake-`curl`/`gh` tests); `init-source`
// has no JSON output at all. `recall`/`probe`/`introspect` are exercised by
// `tests/cli.rs::json_stdout_is_exactly_one_document_across_the_verb_surface`.
// ---------------------------------------------------------------------------

/// `mind review <target> --json` on a clean source (every item described, no
/// findings at all) answers with the CLI-219 document, `outcome: "clean"`,
/// both finding arrays empty.
#[test]
fn review_json_clean_source_answers_clean_outcome() {
    // spec: CLI-218 CLI-219
    let sb = Sandbox::new("review-clean");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\n# widget\n",
    );

    let r = sb.mind(&["--json", "review", &sb.source_spec()]);
    assert!(r.success, "review --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["schema"], 1, "{v}");
    assert_eq!(v["action"], "review", "{v}");
    assert_eq!(v["outcome"], "clean", "{v}");
    assert_eq!(v["hard"], serde_json::json!([]), "{v}");
    assert_eq!(v["advisory"], serde_json::json!([]), "{v}");
    assert_eq!(v["fixed"], serde_json::json!([]), "{v}");
}

/// `mind review <target> --json` on a source with only advisory findings (a
/// skill with no description, CLI-131) answers `outcome: "advisory"`, still
/// exit 0.
#[test]
fn review_json_advisory_only_answers_advisory_outcome() {
    // spec: CLI-218 CLI-219
    let sb = Sandbox::new("review-advisory");
    sb.write_and_commit("agents/dev.md", "# dev agent\nno frontmatter here\n");

    let r = sb.mind(&["--json", "review", &sb.source_spec()]);
    assert!(r.success, "review --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["outcome"], "advisory", "{v}");
    assert_eq!(v["hard"], serde_json::json!([]), "{v}");
    let advisory = v["advisory"]
        .as_array()
        .unwrap_or_else(|| panic!("advisory must be an array: {v}"));
    assert!(
        advisory.iter().any(|f| f["kind"] == "missing-description"),
        "{v}"
    );
}

/// `mind review <target> --json` on a source with a HARD finding (a malformed
/// `mind.toml`) still exits non-zero, and stdout is the ONE CLI-181 error
/// envelope -- not the CLI-219 success document -- with the findings folded
/// into its `details` member (CLI-221) instead of dropped.
#[test]
fn review_json_hard_finding_is_folded_into_the_error_envelope_details() {
    // spec: CLI-218 CLI-219 CLI-221
    let sb = Sandbox::new("review-hard");
    sb.write_and_commit("mind.toml", "[[[[bad toml");

    let r = sb.mind(&["--json", "review", &sb.source_spec()]);
    assert!(
        !r.success,
        "a hard finding must still fail review under --json: {} {}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["schema"], 1, "{v}");
    assert_eq!(
        v["error"]["kind"], "review-failed",
        "the CLI-181 envelope, not the CLI-219 success document: {v}"
    );
    assert_eq!(v["details"]["action"], "review", "{v}");
    assert_eq!(v["details"]["outcome"], "failed", "{v}");
    let hard = v["details"]["hard"]
        .as_array()
        .unwrap_or_else(|| panic!("details.hard must be an array: {v}"));
    assert!(hard.iter().any(|f| f["kind"] == "toml-parse-error"), "{v}");
}

/// `mind review --policy <path> --json` shares the CLI-219 shape: a clean
/// policy answers `outcome: "clean"`.
#[test]
fn review_policy_json_clean_answers_clean_outcome() {
    // spec: CLI-218 CLI-219
    let sb = Sandbox::new("review-policy-clean");
    let policy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/policy/policy.toml");

    let r = sb.mind(&["--json", "review", "--policy", &policy.to_string_lossy()]);
    assert!(
        r.success,
        "review --policy --json: {} {}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "review", "{v}");
    assert_eq!(v["outcome"], "clean", "{v}");
}

/// `mind review --policy <path> --json` on an invalid (unparseable) policy
/// file fails and folds the finding into the CLI-181 envelope's `details`,
/// exactly like the source-target hard-finding case.
#[test]
fn review_policy_json_hard_finding_is_folded_into_the_error_envelope_details() {
    // spec: CLI-218 CLI-219 CLI-221
    let sb = Sandbox::new("review-policy-hard");
    let policy = sb.base.join("bad-policy.toml");
    write(&policy, "not valid toml [[[");

    let r = sb.mind(&["--json", "review", "--policy", &policy.to_string_lossy()]);
    assert!(
        !r.success,
        "an invalid policy file must fail under --json: {} {}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["error"]["kind"], "review-failed", "{v}");
    let hard = v["details"]["hard"]
        .as_array()
        .unwrap_or_else(|| panic!("details.hard must be an array: {v}"));
    assert!(hard.iter().any(|f| f["kind"] == "invalid-policy"), "{v}");
}

/// `mind hooks list <target> --json` answers with the CLI-220 document
/// (schema/action/target/sources), the same shape asserted end to end in
/// `tests/cli_hooks.rs::hooks_list_json_answers_with_the_cli_220_document`;
/// this one just pins that it is exactly ONE document under this file's
/// generic CLI-217 assertion, alongside every other verb here.
#[test]
fn hooks_list_json_is_one_document() {
    // spec: CLI-218 CLI-220
    let sb = Sandbox::new("hooks-list-doc");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "hooks", "list", "hooks-list-doc"]);
    assert!(r.success, "hooks list --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "hooks-list", "{v}");
}

/// `mind hooks run <target> --json` on a source with no hooks declared (the
/// "nothing to do" path) still answers with the CLI-222 document, all counts
/// zero, rather than silence.
#[test]
fn hooks_run_json_no_hooks_still_answers_with_zeroed_document() {
    // spec: CLI-218 CLI-222
    let sb = Sandbox::new("hooks-run-doc");
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["--json", "hooks", "run", "hooks-run-doc"]);
    assert!(r.success, "hooks run --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "hooks-run", "{v}");
    assert_eq!(v["existed"], 0, "{v}");
    assert_eq!(v["ran"], 0, "{v}");
    assert_eq!(v["skipped"], 0, "{v}");
}

/// CLI-218's boundary as executable code: every verb driven here answers
/// `--json` with exactly one JSON document, EXCEPT the closed exclusion list
/// (`dump`, `completions`, `man`), whose stdout is asserted to NOT parse as
/// JSON. This is the rule the spec statement states, not an enumeration of
/// what happens to work today: a verb added later and left off both lists
/// would fail here, whichever side it landed on.
#[test]
fn cli_218_every_driven_verb_is_json_or_a_named_exclusion() {
    // spec: CLI-218
    let sb = Sandbox::new("boundary");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\n# widget\n",
    );
    let spec = sb.source_spec();
    let policy = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/policy/policy.toml")
        .to_string_lossy()
        .into_owned();

    // Every one of these must answer with exactly one JSON document.
    let included: Vec<Vec<&str>> = vec![
        vec!["--json", "meld", &spec, "--register-only"],
        vec!["--json", "meld", &spec, "--register-only"], // remeld branch
        vec!["--json", "learn", "widget"],
        vec!["--json", "recall"],
        vec!["--json", "probe", "--no-tui"],
        vec!["--json", "review", &spec],
        vec!["--json", "review", "--policy", &policy],
        vec!["--json", "introspect"],
        vec!["--json", "config", "show"],
        vec!["--json", "config", "lobes", "list"],
        vec!["--json", "hooks", "list", "boundary"],
        vec!["--json", "hooks", "run", "boundary"],
        vec!["--json", "sync"],
        vec!["--json", "upgrade", "--yes"],
        vec!["--json", "forget", "widget", "--yes"],
        vec!["--json", "unmeld", "boundary", "--yes"],
    ];
    for args in included {
        let r = sb.mind(&args);
        assert_stdout_is_one_json_document(&r.stdout);
        assert!(
            r.stdout.trim().lines().count() >= 1,
            "{args:?}: {:?} / {:?}",
            r.stdout,
            r.stderr
        );
    }

    // The closed exclusion list: stdout is a non-JSON product (`--json` or
    // not), so it must NOT parse as JSON.
    for args in [
        vec!["--json", "dump"],
        vec!["--json", "completions", "bash"],
        vec!["--json", "man"],
    ] {
        let r = sb.mind(&args);
        assert!(
            serde_json::from_str::<serde_json::Value>(r.stdout.trim()).is_err(),
            "{args:?} is a named CLI-218 exclusion; its stdout must not parse \
             as JSON: {:?}",
            r.stdout
        );
    }
    // `evolve` and `init-source` are also named CLI-218 exclusions but are not
    // driven here: `evolve` needs a real or faked release endpoint (covered
    // hermetically by `src/selfupdate.rs`'s fake-`curl`/`gh` unit tests, not
    // this end-to-end file), and `init-source` mutates the target repo's
    // working tree in a way orthogonal to this file's fixtures.
    //
    // NOTE: the lists above are a fixed set of INVOCATIONS, so they can only
    // ever cover verbs someone thought to list. The gate that actually closes
    // CLI-218's boundary against a verb added later is
    // `src/main.rs`'s `tests::cli_218_boundary_is_closed_over_every_clap_
    // subcommand`, which enumerates the verbs from clap itself and fails when
    // one is classified neither way. This test is the end-to-end companion:
    // it proves the classification is honored by the real binary.
}

/// A second, independent source repo -- git-initialized under its own parent
/// directory (so its `<parent>/<name>` identity is distinct from any other
/// source sharing the same final path component `name`) but not tied to its
/// own mind/claude home. Melded into the CALLER's `Sandbox` instead, so two
/// such repos can share a trailing identity suffix within ONE environment
/// (the CLI-233/CLI-234/CLI-235 multi-source-match scenario).
fn extra_source_repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    write(&dir.join("README.md"), "# extra source\n");
    git(&dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "initial"]);
    dir
}

/// CLI-235(d): `mind upgrade '<suffix>#*' --json --yes` scoped to more than
/// one source keeps stdout a single valid JSON document (the CLI-217
/// invariant this whole file gates) while still disclosing the multi-source
/// scope -- on stderr, since `--yes` skips the `ConfirmationRequired` error
/// that is the only OTHER channel (CLI-233) a `--json` caller sees the
/// disclosure through, and printing it to stdout directly would break the
/// one-document invariant.
#[test]
fn upgrade_json_yes_keeps_stdout_one_document_with_disclosure_on_stderr() {
    // spec: CLI-234 CLI-235 CLI-217
    let sb = Sandbox::new("skills");
    sb.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let two = extra_source_repo(&sb.base.join("other"), "skills");
    write(
        &two.join("skills/widget-b/SKILL.md"),
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );
    git(&two, &["add", "-A"]);
    git(&two, &["commit", "-qm", "widget-b"]);
    let two_spec = two.to_string_lossy().into_owned();

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["meld", &two_spec]).success);
    assert!(sb.mind(&["learn", "skill:widget-a"]).success);
    assert!(sb.mind(&["learn", "skill:widget-b"]).success);

    // Both sources drift, so both have a pending upgrade for the filter.
    sb.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    write(
        &two.join("skills/widget-b/SKILL.md"),
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );
    git(&two, &["add", "-A"]);
    git(&two, &["commit", "-qm", "edit widget-b"]);

    let r = sb.mind(&["--json", "upgrade", "skills#*", "--yes"]);
    assert!(
        r.success,
        "upgrade 'skills#*' --json --yes: {} {}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "upgrade", "{v}");
    assert!(
        r.stderr.contains("matched 2 sources"),
        "the multi-source disclosure must still reach the user, on stderr: {:?}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("matched 2 sources"),
        "the disclosure must not also land on stdout: {:?}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// CLI-221: the `details` member is optional, and the slot behind it is a
// process-global static. These pin its ABSENCE as carefully as its presence.
// ---------------------------------------------------------------------------

/// The ordinary failure: a verb that fails without recording any findings must
/// produce an envelope with NO `details` key at all -- not `null`, not `{}`.
/// A consumer doing `if "details" in doc` must not see one on every error just
/// because the mechanism exists.
#[test]
fn error_envelope_without_recorded_details_has_no_details_member() {
    // spec: CLI-181 CLI-221
    let sb = Sandbox::new("no-details");
    let r = sb.mind(&["--json", "learn", "does-not-exist"]);
    assert!(
        !r.success,
        "learn of a missing item must fail: {}",
        r.stdout
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert!(v["error"]["kind"].is_string(), "{v}");
    assert!(
        !v.as_object().is_some_and(|o| o.contains_key("details")),
        "an error with no recorded findings must carry no `details` member: {v}"
    );
}

/// A failing `review` records details; a LATER, unrelated failure must not
/// inherit them. Each `mind` run is its own process so the static cannot
/// physically carry over today -- this pins that it stays that way, i.e. that
/// nobody "improves" the slot by persisting it to the mind home, and that the
/// review run really is the only reason `details` ever appears.
#[test]
fn recorded_details_do_not_leak_into_a_later_unrelated_failure() {
    // spec: CLI-181 CLI-221
    let sb = Sandbox::new("leak");
    sb.write_and_commit("mind.toml", "[[[[bad toml");

    let failed_review = sb.mind(&["--json", "review", &sb.source_spec()]);
    assert!(!failed_review.success, "{}", failed_review.stdout);
    let v = assert_stdout_is_one_json_document(&failed_review.stdout);
    assert!(v["details"].is_object(), "precondition: {v}");

    let next = sb.mind(&["--json", "forget", "nothing-installed"]);
    assert!(!next.success, "{}", next.stdout);
    let v = assert_stdout_is_one_json_document(&next.stdout);
    assert!(
        !v.as_object().is_some_and(|o| o.contains_key("details")),
        "the previous run's review findings must not appear here: {v}"
    );
}

/// A hard finding does not suppress the rest of the CLI-219 document: the
/// `details` member carries the advisory findings and the `--fix` list too, not
/// just the hard ones. The implementor's test asserted only `details.hard`, so
/// a `details` that dropped everything else would have passed it.
#[test]
fn review_json_error_details_carry_advisory_and_fixed_too() {
    // spec: CLI-219 CLI-221
    let sb = Sandbox::new("review-both");
    // A `{{ns:}}` token naming no sibling is a HARD bad-reference...
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\ndescription: d\n---\nhand off to {{ns:nope}}\n",
    );
    // ...and an item with no frontmatter description is an ADVISORY finding.
    sb.write_and_commit("agents/dev.md", "# dev agent\n");

    let r = sb.mind(&["--json", "review", &sb.source_spec()]);
    assert!(
        !r.success,
        "a hard finding must fail: {} {}",
        r.stdout, r.stderr
    );
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["error"]["kind"], "review-failed", "{v}");
    assert_eq!(v["details"]["outcome"], "failed", "{v}");
    let hard = v["details"]["hard"]
        .as_array()
        .unwrap_or_else(|| panic!("details.hard must be an array: {v}"));
    assert!(
        hard.iter().any(|f| f["kind"] == "bad-reference"),
        "the hard finding: {v}"
    );
    let advisory = v["details"]["advisory"]
        .as_array()
        .unwrap_or_else(|| panic!("details.advisory must be an array: {v}"));
    assert!(
        advisory.iter().any(|f| f["kind"] == "missing-description"),
        "the advisory findings must survive into `details`, not be dropped \
         because the run failed: {v}"
    );
    assert!(
        v["details"]["fixed"].is_array(),
        "`fixed` must be present (empty) rather than omitted: {v}"
    );
    // Every finding carries a non-empty human message alongside the slug.
    assert!(
        hard.iter()
            .chain(advisory.iter())
            .all(|f| f["message"].as_str().is_some_and(|m| !m.is_empty())),
        "{v}"
    );
}

/// `review --fix --json`: the rewritten files reach the caller in the
/// document's `fixed` array, and the per-file `fixed <path>` progress lines
/// (plain `println!`s that run BEFORE the document is recorded) land on stderr
/// rather than corrupting stdout. Only the empty-`fixed` case was covered.
#[test]
fn review_fix_json_reports_the_rewritten_files_and_one_document() {
    // spec: CLI-217 CLI-219 CLI-138
    let sb = Sandbox::new("review-fix");
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: review\n---\nrun ~/.claude/skills/review/run.sh; hand off to dev\n",
    );
    sb.write_and_commit("agents/dev.md", "---\ndescription: dev\n---\n# dev\n");

    let r = sb.mind(&["--json", "review", &sb.source_spec(), "--fix"]);
    assert!(r.success, "review --fix --json: {} {}", r.stdout, r.stderr);
    let v = assert_stdout_is_one_json_document(&r.stdout);
    assert_eq!(v["action"], "review", "{v}");
    let fixed = v["fixed"]
        .as_array()
        .unwrap_or_else(|| panic!("fixed must be an array: {v}"));
    assert!(
        fixed.iter().any(|f| f
            .as_str()
            .is_some_and(|s| s.ends_with("skills/review/SKILL.md"))),
        "the rewritten file must be reported in `fixed`: {v}"
    );
    // The rewrite really happened (so `fixed` is not just a hopeful label).
    let rewritten = std::fs::read_to_string(sb.source.join("skills/review/SKILL.md")).unwrap();
    assert!(
        rewritten.contains("{{ns:dev}}"),
        "the bare sibling name must have been templatized: {rewritten}"
    );
    assert!(
        r.stderr.contains("fixed"),
        "the per-file `fixed <path>` line must reach the user on stderr: {:?}",
        r.stderr
    );
}

#[test]
fn a_streaming_hook_cannot_reach_the_json_document_on_stdout() {
    // spec: HOOK-32 CLI-217
    // Hook output is streamed by INHERITING the child's stdio (HOOK-30), which
    // would put it straight onto fd 1 -- the same descriptor the result document
    // uses. It stays out because `--json` points fd 1 at fd 2 with a real
    // `dup2` for the whole run, so the inherited child writes to stderr too.
    // Without that, a hook echoing a single line would wedge itself into the
    // one JSON document and break every machine consumer.
    let sb = Sandbox::new("json-stream-hook");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nrun = \"echo NOISE-ON-STDOUT; echo NOISE-ON-STDERR 1>&2\"\n\
         name = \"build\"\nevent = \"install\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "--json",
        "meld",
        &spec,
        "--register-only",
        "--dangerously-skip-install-hook-check",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Exactly one JSON document, and it parses: the hook's chatter is not in it.
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {}", r.stdout));
    assert!(doc.get("action").is_some(), "the meld result: {}", r.stdout);
    assert!(
        !r.stdout.contains("NOISE-ON-STDOUT") && !r.stdout.contains("====== (hook:"),
        "neither the hook output nor its frame may reach stdout: {}",
        r.stdout
    );
    // It is not discarded, just routed: both streams land on stderr.
    assert!(
        r.stderr.contains("NOISE-ON-STDOUT") && r.stderr.contains("NOISE-ON-STDERR"),
        "the hook's output must still be visible on stderr: {}",
        r.stderr
    );
}
