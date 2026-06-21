//! Install hooks: a source-declared (`[source].install`) or user-supplied
//! (`meld --install-hook`) shell command that builds the tooling a source's
//! items rely on. Because it is arbitrary code from the source, `mind` discloses
//! it and prompts before running (see spec/install-hooks.md).

use std::io::BufRead;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use crate::error::{MindError, Result};

/// The user's response to the three-way hook prompt (HOOK-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookChoice {
    RunAndContinue,
    SkipAndContinue,
    Abort,
}

/// Parse a reply to the three-way prompt (HOOK-20): "1" => run, "2" or empty
/// (a bare Enter, the default) => skip, "3" => abort. Anything unrecognized
/// defaults to Skip, so an unclear reply NEVER runs the hook. Trims whitespace.
pub fn parse_hook_choice(input: &str) -> HookChoice {
    match input.trim() {
        "1" => HookChoice::RunAndContinue,
        "2" | "" => HookChoice::SkipAndContinue,
        "3" => HookChoice::Abort,
        _ => HookChoice::SkipAndContinue,
    }
}

/// Resolve the effective hook command (HOOK-2, HOOK-3). A consumer-supplied
/// command (`--install-hook`) overrides a declared `[source].install`. Returns
/// `Some((command, overrides_declared))` or `None` when neither is non-empty.
/// `overrides_declared` is true only when `supplied` is non-empty AND `declared`
/// is non-empty. Empty or whitespace-only values are treated as absent (HOOK-3).
pub fn resolve_hook<'a>(
    declared: Option<&'a str>,
    supplied: Option<&'a str>,
) -> Option<(&'a str, bool)> {
    // Treat empty/whitespace as absent per HOOK-3.
    let effective_supplied = supplied.map(str::trim).filter(|s| !s.is_empty());
    let effective_declared = declared.map(str::trim).filter(|s| !s.is_empty());
    match (effective_declared, effective_supplied) {
        (decl, Some(s)) => {
            let overrides = decl.is_some();
            Some((s, overrides))
        }
        (Some(d), None) => Some((d, false)),
        (None, None) => None,
    }
}

/// Whether stdin is an interactive terminal (the HOOK-22 gate). The one seam
/// that cannot be exercised headlessly.
pub fn is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// The HOOK-20 disclosure shown before running a hook. Pure (returns a String)
/// so it is unit-testable. Includes the source identity, the resolved pin
/// description, the commit, the clone path, the exact command, and a clear
/// arbitrary-code warning. When `declared_override` is Some(declared), also
/// shows the declared command and states the user-supplied command replaces it
/// (HOOK-2's loud override).
pub fn disclosure_text(
    identity: &str,
    pin_desc: &str,
    commit: &str,
    clone_path: &str,
    command: &str,
    declared_override: Option<&str>,
) -> String {
    let mut out = String::new();

    out.push_str("  Source:    ");
    out.push_str(identity);
    out.push('\n');

    out.push_str("  Pin:       ");
    out.push_str(pin_desc);
    out.push('\n');

    out.push_str("  Commit:    ");
    out.push_str(commit);
    out.push('\n');

    out.push_str("  Clone:     ");
    out.push_str(clone_path);
    out.push('\n');

    if let Some(declared) = declared_override {
        out.push_str("  Declared:  ");
        out.push_str(declared);
        out.push('\n');
        out.push_str("  Override:  ");
        out.push_str(command);
        out.push('\n');
        out.push_str("  NOTE: the user-supplied command replaces the source's declared command.\n");
    } else {
        out.push_str("  Command:   ");
        out.push_str(command);
        out.push('\n');
    }

    out.push('\n');
    out.push_str("  WARNING: this executes arbitrary code from the source with your privileges.\n");

    out
}

/// Read one line from `reader` and return the parsed `HookChoice` (HOOK-20).
/// EOF (zero bytes read) returns `SkipAndContinue` so that an absent or unclear
/// reply NEVER runs the hook and NEVER aborts.
fn read_choice<R: BufRead>(mut reader: R) -> Result<HookChoice> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Ok(HookChoice::SkipAndContinue), // EOF => skip, never abort
        Ok(_) => Ok(parse_hook_choice(&line)),
        Err(e) => Err(MindError::io("<stdin>", e)),
    }
}

/// Print the disclosure and the three choices, read one line from stdin, and
/// return the parsed choice (HOOK-20). Delegates the read to `read_choice` so
/// the read path is independently testable.
pub fn prompt_choice(disclosure: &str) -> Result<HookChoice> {
    print!("{disclosure}");
    println!("  [1] Run the hook and continue");
    println!("  [2] Skip the hook but continue installing (default: skip)");
    println!("  [3] Abort - install nothing");
    print!("Choice [1/2/3, default 2]: ");
    std::io::stdout()
        .flush()
        .map_err(|e| MindError::io("<stdout>", e))?;

    read_choice(std::io::stdin().lock())
}

/// Run `command` via the shell (`sh -c <command>`) in `clone_dir` (HOOK-30).
/// A non-zero exit (or spawn failure) maps to `MindError::HookFailed` carrying
/// the identity, command, exit status, and stderr.
pub fn run_hook(command: &str, clone_dir: &Path, identity: &str) -> Result<()> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(clone_dir)
        .output()
        .map_err(|e| MindError::HookFailed {
            identity: identity.to_string(),
            command: command.to_string(),
            status: None,
            stderr: e.to_string(),
        })?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Err(MindError::HookFailed {
            identity: identity.to_string(),
            command: command.to_string(),
            status: Some(output.status),
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// RAII guard that removes a temp directory when dropped.
    /// Uses process id + atomic counter to avoid collisions between parallel or
    /// stale runs.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "mind-hook-test-{}-{}-{n}",
                std::process::id(),
                tag
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // ---- parse_hook_choice ----

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_run_on_1() {
        assert_eq!(parse_hook_choice("1"), HookChoice::RunAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_skip_on_2() {
        assert_eq!(parse_hook_choice("2"), HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_skip_on_empty_default() {
        assert_eq!(parse_hook_choice(""), HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_abort_on_3() {
        assert_eq!(parse_hook_choice("3"), HookChoice::Abort);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_garbage_defaults_to_skip() {
        assert_eq!(parse_hook_choice("garbage"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice("yes"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice("run"), HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_trims_whitespace() {
        assert_eq!(parse_hook_choice(" 1 "), HookChoice::RunAndContinue);
        assert_eq!(parse_hook_choice("\t2\n"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice(" 3 "), HookChoice::Abort);
    }

    // ---- read_choice (tests the stdin read path, not just the parser) ----

    // spec: HOOK-20
    // EOF (empty reader) must produce SkipAndContinue: an absent reply never
    // runs and never aborts.
    #[test]
    fn read_choice_eof_returns_skip_and_continue() {
        let reader = std::io::Cursor::new("");
        let result = read_choice(reader).expect("read_choice should not error on EOF");
        assert_eq!(
            result,
            HookChoice::SkipAndContinue,
            "EOF must yield SkipAndContinue, not run or abort"
        );
    }

    // spec: HOOK-20
    // "3\n" through the read path must produce Abort (not just the parser).
    #[test]
    fn read_choice_abort_on_3() {
        let reader = std::io::Cursor::new("3\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::Abort);
    }

    // spec: HOOK-20
    // "1\n" through the read path must produce RunAndContinue.
    #[test]
    fn read_choice_run_and_continue_on_1() {
        let reader = std::io::Cursor::new("1\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::RunAndContinue);
    }

    // spec: HOOK-20
    // "2\n" through the read path must produce SkipAndContinue.
    #[test]
    fn read_choice_skip_and_continue_on_2() {
        let reader = std::io::Cursor::new("2\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::SkipAndContinue);
    }

    // ---- resolve_hook ----

    // spec: HOOK-2
    #[test]
    fn resolve_hook_supplied_overrides_declared() {
        let result = resolve_hook(Some("make install"), Some("./custom.sh"));
        assert_eq!(result, Some(("./custom.sh", true)));
    }

    // spec: HOOK-2
    #[test]
    fn resolve_hook_supplied_only_no_override() {
        let result = resolve_hook(None, Some("./custom.sh"));
        assert_eq!(result, Some(("./custom.sh", false)));
    }

    // spec: HOOK-2
    #[test]
    fn resolve_hook_declared_only() {
        let result = resolve_hook(Some("make install"), None);
        assert_eq!(result, Some(("make install", false)));
    }

    // spec: HOOK-3
    #[test]
    fn resolve_hook_both_none_is_none() {
        let result = resolve_hook(None, None);
        assert_eq!(result, None);
    }

    // spec: HOOK-3
    // Empty declared with no supplied => None (empty is treated as absent).
    #[test]
    fn resolve_hook_empty_declared_no_supplied_is_none() {
        let result = resolve_hook(Some(""), None);
        assert_eq!(result, None, "empty declared must be treated as absent");
    }

    // spec: HOOK-3
    // No declared with empty supplied => None (empty is treated as absent).
    #[test]
    fn resolve_hook_no_declared_empty_supplied_is_none() {
        let result = resolve_hook(None, Some(""));
        assert_eq!(result, None, "empty supplied must be treated as absent");
    }

    // spec: HOOK-3
    // Whitespace-only declared => None.
    #[test]
    fn resolve_hook_whitespace_declared_is_none() {
        let result = resolve_hook(Some("   "), None);
        assert_eq!(
            result, None,
            "whitespace-only declared must be treated as absent"
        );
    }

    // spec: HOOK-3
    // Whitespace supplied with a real declared => falls back to the declared
    // (whitespace supplied does not override).
    #[test]
    fn resolve_hook_whitespace_supplied_falls_back_to_declared() {
        let result = resolve_hook(Some("make install"), Some("  "));
        assert_eq!(
            result,
            Some(("make install", false)),
            "whitespace supplied should not override a real declared"
        );
    }

    // spec: HOOK-3
    // A real supplied still overrides a real declared (regression guard).
    #[test]
    fn resolve_hook_real_supplied_overrides_real_declared() {
        let result = resolve_hook(Some("make install"), Some("./override.sh"));
        assert_eq!(result, Some(("./override.sh", true)));
    }

    // ---- disclosure_text ----

    // spec: HOOK-2
    #[test]
    fn disclosure_text_contains_required_fields() {
        let text = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/home/user/.mind/sources/github.com/acme/tools",
            "make install",
            None,
        );
        assert!(text.contains("github.com/acme/tools"), "missing identity");
        assert!(text.contains("main"), "missing pin_desc");
        assert!(text.contains("abc1234"), "missing commit");
        assert!(
            text.contains("/home/user/.mind/sources/github.com/acme/tools"),
            "missing clone_path"
        );
        assert!(text.contains("make install"), "missing command");
        assert!(text.contains("arbitrary"), "missing arbitrary-code warning");
    }

    // spec: HOOK-2
    #[test]
    fn disclosure_text_override_shows_both_commands_and_replacement_note() {
        let text = disclosure_text(
            "github.com/acme/tools",
            "v1.0",
            "def5678",
            "/tmp/clone",
            "./user-custom.sh",
            Some("make install"),
        );
        // Both commands must appear.
        assert!(text.contains("make install"), "missing declared command");
        assert!(
            text.contains("./user-custom.sh"),
            "missing override command"
        );
        // Replacement statement must appear.
        assert!(
            text.contains("replaces"),
            "missing replacement statement; text: {text}"
        );
        // Arbitrary-code warning must appear.
        assert!(text.contains("arbitrary"), "missing arbitrary-code warning");
    }

    // spec: HOOK-2
    #[test]
    fn disclosure_text_no_override_does_not_mention_replacement() {
        let text = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
        );
        assert!(
            !text.contains("replaces"),
            "should not mention replacement when no override"
        );
    }

    // ---- run_hook ----

    // spec: HOOK-30
    #[test]
    fn run_hook_success_creates_marker_file() {
        let dir = TempDir::new("success");
        let marker = dir.path().join("marker.txt");
        let marker_str = marker.to_str().expect("marker path is utf8");
        let command = format!("touch {marker_str}");
        let result = run_hook(&command, dir.path(), "github.com/test/repo");
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert!(
            marker.exists(),
            "marker file should exist after successful hook"
        );
    }

    // spec: HOOK-30
    #[test]
    fn run_hook_nonzero_exit_returns_hook_failed() {
        let dir = TempDir::new("fail");
        let result = run_hook("exit 3", dir.path(), "github.com/test/repo");
        match result {
            Err(MindError::HookFailed {
                ref identity,
                ref command,
                status,
                ..
            }) => {
                assert_eq!(identity, "github.com/test/repo", "wrong identity");
                assert_eq!(command, "exit 3", "wrong command");
                assert!(
                    status.is_some(),
                    "exit status should be Some for a process that ran"
                );
                let code = status.unwrap().code();
                assert_eq!(code, Some(3), "expected exit code 3, got {code:?}");
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    #[test]
    fn run_hook_identity_and_command_propagate_to_error() {
        let dir = TempDir::new("propagate");
        let result = run_hook("false", dir.path(), "github.com/acme/special");
        match result {
            Err(MindError::HookFailed {
                ref identity,
                ref command,
                ..
            }) => {
                assert_eq!(identity, "github.com/acme/special");
                assert_eq!(command, "false");
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }
}
