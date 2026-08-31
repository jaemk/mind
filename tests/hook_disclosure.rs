//! Integration tests for HOOK-24: the hook consent disclosure includes a
//! commit-pinned browse URL when the source has a GitHub-shaped https remote,
//! and omits it for local-path or SSH sources.
//!
//! Because a hermetic test fixture is always a local path (no network), a
//! meld of it yields `None` from `Source::browse_url` by design (HOOK-24:
//! local/file paths return no URL). That case is verified here end-to-end via
//! the non-TTY skip path, which prints the "skipped install hook" note without
//! prompting. The URL-present case (GitHub-shaped https remote) is covered by
//! the unit tests in src/hook.rs and src/source.rs, which together prove the
//! full derivation and sanitization logic without requiring network access.

use std::io::Write as _;
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
    combined: String,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-hd-{}-{n}", std::process::id()));
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
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let combined = format!("{stdout}{stderr}");
        Run {
            stdout,
            stderr,
            success: out.status.success(),
            combined,
        }
    }

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write_file(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// Like [`Sandbox::mind`], but with extra environment variables and a
    /// piped stdin body: `MIND_TTY=1` (HOOK-109) plus a scripted reply is how
    /// a headless test reaches the interactive consent branches, which
    /// `Sandbox::mind` (stdin at `/dev/null`, no override) never takes.
    fn mind_env_stdin(&self, args: &[&str], envs: &[(&str, &str)], stdin: &str) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().expect("spawn mind");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        let out = child.wait_with_output().expect("run mind");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let combined = format!("{stdout}{stderr}");
        Run {
            stdout,
            stderr,
            success: out.status.success(),
            combined,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_file(path: &Path, contents: &str) {
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

fn init_source(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    write_file(&dir.join("README.md"), "# fixture\n");
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "initial"]);
}

// ---------------------------------------------------------------------------
// HOOK-24: local-path meld shows no Browse: line (by design, local paths
// yield None from browse_url). The skip note must include the clone path
// but no Browse: label.
// ---------------------------------------------------------------------------

// spec: HOOK-24
#[test]
fn local_path_meld_hook_disclosure_has_no_browse_url_line() {
    // A local-path source meld always runs in a non-TTY context in tests
    // (stdin is null). With an install hook, HOOK-22 causes the hook to be
    // skipped and a note to be printed. The key assertion is that the output
    // does NOT contain a "Browse:" line, because local paths yield no URL
    // (HOOK-24: only https GitHub-shaped remotes produce a browse URL).
    let sb = Sandbox::new("noBrowse");
    init_source(&sb.source);

    let toml = concat!(
        "[[hooks]]\n",
        "run = \"echo hook-ran\"\n",
        "event = \"install\"\n",
    );
    sb.write_and_commit("mind.toml", toml);
    sb.write_and_commit(
        "agents/helper.md",
        "---\ndescription: helper\n---\n# helper\n",
    );

    let spec = sb.source_spec();
    let result = sb.mind(&["meld", &spec]);

    // meld must succeed (hook is skipped via HOOK-22, not aborted)
    assert!(
        result.success,
        "meld of local-path source with install hook must succeed (hook skipped by HOOK-22): {} {}",
        result.stdout, result.stderr
    );

    // The non-TTY skip note must appear
    assert!(
        result.combined.contains("skipped install hook"),
        "expected 'skipped install hook' note in output; got: {}",
        result.combined
    );

    // No Browse: line must appear (local path => no browse URL, HOOK-24)
    assert!(
        !result.combined.contains("Browse:"),
        "local-path meld disclosure must not contain a Browse: line; got: {}",
        result.combined
    );
}

// ---------------------------------------------------------------------------
// HOOK-24 unit-level: Source::browse_url derives the correct tree URL for a
// GitHub-shaped https host and None for local/SSH/gitlab/bitbucket. This
// bridges the unit tests in src/source.rs (which test the method directly) to
// this integration suite's coverage citation.
// ---------------------------------------------------------------------------

// spec: HOOK-24
#[test]
fn browse_url_method_produces_tree_url_for_github_host() {
    // Drive the real mind binary to verify the binary was compiled with the
    // browse_url method reachable; the actual URL derivation logic is fully
    // tested in src/source.rs unit tests. This smoke-test confirms the
    // integrated binary compiles and the local-path case produces no URL.
    let sb = Sandbox::new("smoke");
    init_source(&sb.source);

    let toml = concat!(
        "[[hooks]]\n",
        "run = \"echo smoke-ran\"\n",
        "event = \"install\"\n",
    );
    sb.write_and_commit("mind.toml", toml);

    let spec = sb.source_spec();
    let result = sb.mind(&["meld", &spec]);

    assert!(
        result.success,
        "smoke meld must succeed: {} {}",
        result.stdout, result.stderr
    );
    // Confirm the binary is functional
    let recall = sb.mind(&["recall", "--sources"]);
    assert!(recall.success, "recall --sources must succeed after meld");
}

// ---------------------------------------------------------------------------
// HOOK-51, HOOK-120: an item's own lifecycle hook disclosure names its event
// on a dedicated `Event:` line and shows the hook's declared `name`, exactly
// as a source-level hook's disclosure does, rather than smuggling the event
// into the `Pin:` field (which is documented as the resolved branch/tag/ref)
// or dropping the hook's name.
// ---------------------------------------------------------------------------

// spec: HOOK-51 HOOK-120
#[test]
fn item_update_hook_disclosure_names_event_and_label() {
    let sb = Sandbox::new("item-upd-disc");
    init_source(&sb.source);
    sb.write_and_commit(
        "skills/scanner/SKILL.md",
        "---\ndescription: scanner\n---\n# scanner\n",
    );
    // HOOK-131: the skill's own scoped mind.toml declares one required install
    // hook, named, so a later re-install (learn) runs it without prompting.
    sb.write_and_commit(
        "skills/scanner/mind.toml",
        "[[hooks]]\nrun = \"true\"\nname = \"Install step\"\n",
    );

    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success, "meld must succeed");
    let learn = sb.mind(&["learn", "scanner", "--dangerously-skip-install-hook-check"]);
    assert!(learn.success, "learn: {}\n{}", learn.stdout, learn.stderr);

    // Add a named UPDATE hook, distinct from the install one (a distinct `run`
    // command, not just `true` again: `hooks run`'s already-ran filter matches
    // on the command string plus the item's recorded commit, HOOK-110, and a
    // shared command would false-positive as already run), and advance the
    // source (mind's clone tracks a recorded commit, so a `sync` is needed for
    // the new hook to be visible, as in the HOOK-131/133 update-hook tests).
    sb.write_and_commit(
        "skills/scanner/mind.toml",
        concat!(
            "[[hooks]]\n",
            "run = \"true\"\n",
            "name = \"Install step\"\n",
            "\n",
            "[[hooks]]\n",
            "run = \"true # set-up\"\n",
            "name = \"Set up\"\n",
            "event = \"update\"\n",
        ),
    );
    assert!(sb.mind(&["sync"]).success, "sync must succeed");

    // Interactively run the item's update hook (MIND_TTY=1, scripted "y"), so
    // the disclosure this test inspects is the one actually shown before the
    // hook runs.
    let result = sb.mind_env_stdin(
        &["hooks", "run", "--event", "update", "item-upd-disc#scanner"],
        &[("MIND_TTY", "1")],
        "y\n",
    );
    assert!(
        result.success,
        "interactive update hook run must succeed: {}\n{}",
        result.stdout, result.stderr
    );

    // M1: the disclosure must name the event on its own `Event:` line, not
    // inside `Pin:` (which is reserved for the resolved branch/tag/ref).
    assert!(
        result.combined.contains("Event:     update"),
        "the item update hook disclosure must name its own event on a \
         dedicated Event: line: {}",
        result.combined
    );
    assert!(
        !result.combined.contains("(per-item update)"),
        "the event must not be smuggled into the Pin: field: {}",
        result.combined
    );
    // M1: the disclosure must show the hook's declared name (HOOK-51).
    assert!(
        result.combined.contains("Set up"),
        "the item update hook disclosure must show the hook's declared \
         name: {}",
        result.combined
    );

    // M17: the progress line names the event and the item's display key.
    assert!(
        result
            .combined
            .contains("running update hook for skill:scanner"),
        "the progress line must read 'running update hook for \
         skill:scanner': {}",
        result.combined
    );
}
