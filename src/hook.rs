//! Install hooks: a source-declared (`[source].install`) or user-supplied
//! (`meld --install-hook`) shell command that builds the tooling a source's
//! items rely on. Because it is arbitrary code from the source, `mind` discloses
//! it and prompts before running (see spec/install-hooks.md).

use std::io::BufRead;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{MindError, Result};

/// The user's response to the three-way hook prompt (HOOK-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookChoice {
    RunAndContinue,
    SkipAndContinue,
    Abort,
}

/// Parse a reply to the three-way prompt (HOOK-20): "y"/"Y" or "" (Enter,
/// the default) => RunAndContinue; "n"/"N" => SkipAndContinue; "a"/"A" =>
/// Abort. Anything unrecognized defaults to SkipAndContinue so an unclear
/// reply never runs the hook. Trims whitespace.
pub fn parse_hook_choice(input: &str) -> HookChoice {
    match input.trim() {
        "y" | "Y" | "" => HookChoice::RunAndContinue,
        "n" | "N" => HookChoice::SkipAndContinue,
        "a" | "A" => HookChoice::Abort,
        _ => HookChoice::SkipAndContinue,
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
/// (HOOK-2's loud override). Prepends a `====== hook: {identity} ======`
/// header so the block reads as distinct from surrounding output.
pub fn disclosure_text(
    identity: &str,
    pin_desc: &str,
    commit: &str,
    clone_path: &str,
    command: &str,
    declared_override: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("====== hook: ");
    out.push_str(identity);
    out.push_str(" ======\n");
    out.push_str(&disclosure_body(
        identity,
        pin_desc,
        commit,
        clone_path,
        command,
        declared_override,
    ));
    out
}

/// Read one line from `reader` and return the parsed `HookChoice` (HOOK-20).
/// EOF (zero bytes read) returns `SkipAndContinue` so that a non-TTY or absent
/// reply never runs the hook and never aborts (HOOK-22).
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
    println!("  [Y] run the hook   [n] skip it   [a] abort - install nothing");
    print!("Run this hook? [Y/n/a] (default Y): ");
    std::io::stdout()
        .flush()
        .map_err(|e| MindError::io("<stdout>", e))?;

    read_choice(std::io::stdin().lock())
}

/// The user's response to the two-way optional-hook prompt (HOOK-52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionalChoice {
    Run,
    Skip,
}

/// Parse a reply to the optional-hook prompt (HOOK-52): "y"/"Y" or "" (Enter,
/// the default) => Run; anything else => Skip. Trims whitespace. An optional
/// hook is never run on an unclear reply.
pub fn parse_optional_choice(input: &str) -> OptionalChoice {
    match input.trim() {
        "y" | "Y" | "" => OptionalChoice::Run,
        _ => OptionalChoice::Skip,
    }
}

/// Read one line from `reader` and return the parsed `OptionalChoice` (HOOK-52).
/// EOF (zero bytes read) returns `Skip` so that a non-TTY or absent reply
/// never runs the hook (HOOK-22).
fn read_optional_choice<R: BufRead>(mut reader: R) -> Result<OptionalChoice> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Ok(OptionalChoice::Skip), // EOF => skip
        Ok(_) => Ok(parse_optional_choice(&line)),
        Err(e) => Err(MindError::io("<stdin>", e)),
    }
}

/// Print the disclosure and the two optional-hook choices, read one line from
/// stdin, and return the parsed choice (HOOK-52). Mirrors `prompt_choice`.
pub fn prompt_choice_optional(disclosure: &str) -> Result<OptionalChoice> {
    print!("{disclosure}");
    println!("  [Y] run   [n] skip");
    print!("Run this optional hook? [Y/n] (default Y): ");
    std::io::stdout()
        .flush()
        .map_err(|e| MindError::io("<stdout>", e))?;

    read_optional_choice(std::io::stdin().lock())
}

/// The action chosen for a hook: the decision ladder shared by every source-hook
/// site. `Abort` is reached only by a required hook the user declines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAct {
    Run,
    Skip,
    Abort,
}

/// Resolve whether to run a hook from its disclosure and flags: a
/// `--dangerously-skip` run is unattended (HOOK-23), a non-TTY skips (HOOK-22),
/// an optional hook prompts two-way (run/skip, HOOK-52), and a required hook
/// prompts three-way (run/skip/abort, HOOK-20). An optional hook never aborts.
pub fn decide(disclosure: &str, optional: bool, dangerously_skip: bool) -> Result<HookAct> {
    if dangerously_skip {
        return Ok(HookAct::Run);
    }
    if !is_tty() {
        return Ok(HookAct::Skip);
    }
    if optional {
        return Ok(match prompt_choice_optional(disclosure)? {
            OptionalChoice::Run => HookAct::Run,
            OptionalChoice::Skip => HookAct::Skip,
        });
    }
    Ok(match prompt_choice(disclosure)? {
        HookChoice::RunAndContinue => HookAct::Run,
        HookChoice::SkipAndContinue => HookAct::Skip,
        HookChoice::Abort => HookAct::Abort,
    })
}

/// Like `disclosure_text` but prefixed with the hook's label and whether it is
/// required or optional, for the multi-hook disclosures (HOOK-52). Prepends a
/// `====== hook: {label} ======` header so the block is visually distinct.
#[allow(clippy::too_many_arguments)]
pub fn hook_disclosure_text(
    label: &str,
    optional: bool,
    identity: &str,
    pin_desc: &str,
    commit: &str,
    clone_path: &str,
    command: &str,
    declared_override: Option<&str>,
) -> String {
    let kind = if optional { "optional" } else { "required" };
    let mut out = String::new();
    out.push_str("====== hook: ");
    out.push_str(label);
    out.push_str(" ======\n");
    out.push_str("  Hook:      ");
    out.push_str(label);
    out.push_str(" (");
    out.push_str(kind);
    out.push_str(")\n");
    // Append the base disclosure fields (without its own header since we
    // already prepended one above; call the inner builder directly).
    out.push_str(&disclosure_body(
        identity,
        pin_desc,
        commit,
        clone_path,
        command,
        declared_override,
    ));
    out
}

/// The fields-only portion of a disclosure block (no header line). Used by
/// both `disclosure_text` (which prepends its own header) and
/// `hook_disclosure_text` (which prepends a different header then calls this).
fn disclosure_body(
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

/// Apply a user-supplied hook override for one event (HOOK-56, HOOK-59).
///
/// Replaces every hook of `event` in `resolved` with one required hook of that
/// event running `supplied`, leaving the other event's hooks in their order.
/// `supplied` is the user command (None or empty/whitespace => no override).
/// Returns the resulting hook list plus, when the override replaced declared
/// hook(s) of that event, the list of declared commands it replaced (for the
/// loud override note). `meld --install-hook` uses `Install`; `unmeld
/// --uninstall-hook` uses `Uninstall`.
pub fn apply_hook_override(
    resolved: Vec<crate::mindfile::ResolvedHook>,
    supplied: Option<&str>,
    event: crate::mindfile::HookEvent,
) -> (Vec<crate::mindfile::ResolvedHook>, Option<Vec<String>>) {
    use crate::mindfile::ResolvedHook;

    // Treat empty/whitespace as absent.
    let effective = match supplied.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => return (resolved, None),
    };

    // Split into the overridden event's commands vs. the other event's hooks.
    let mut replaced: Vec<String> = Vec::new();
    let mut others: Vec<ResolvedHook> = Vec::new();
    for hook in resolved {
        if hook.event == event {
            replaced.push(hook.run);
        } else {
            others.push(hook);
        }
    }

    // Result: the override hook first, then the untouched other-event hooks.
    let override_hook = ResolvedHook {
        run: effective.to_owned(),
        name: None,
        optional: false,
        event,
    };
    let mut result = Vec::with_capacity(1 + others.len());
    result.push(override_hook);
    result.extend(others);

    let replaced = if replaced.is_empty() {
        None
    } else {
        Some(replaced)
    };
    (result, replaced)
}

/// Apply a `meld --install-hook` override to a source's resolved hooks (HOOK-56):
/// `apply_hook_override` specialized to the install event.
pub fn apply_install_override(
    resolved: Vec<crate::mindfile::ResolvedHook>,
    supplied: Option<&str>,
) -> (Vec<crate::mindfile::ResolvedHook>, Option<Vec<String>>) {
    apply_hook_override(resolved, supplied, crate::mindfile::HookEvent::Install)
}

/// Run `command` via the shell (`sh -c <command>`) in `clone_dir` (HOOK-30).
/// Stdout and stderr are captured separately and printed under labeled separator
/// frames so both streams are visible in mind's output. Stdin is closed so a
/// hook cannot consume mind's input. A non-zero exit (or spawn failure) maps to
/// `MindError::HookFailed`.
pub fn run_hook(command: &str, clone_dir: &Path, identity: &str, label: &str) -> Result<()> {
    // Flush mind's own buffered output first so it does not interleave with the
    // hook's output blocks.
    let _ = std::io::stdout().flush();

    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| MindError::HookFailed {
            identity: identity.to_string(),
            command: command.to_string(),
            status: None,
            stderr: e.to_string(),
        })?;

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);

    let mut printed_any = false;
    if !stdout_str.is_empty() {
        println!("====== (hook-stdout: {label}) ======");
        print!("{stdout_str}");
        // Ensure the output ends with a newline so the next line is clean.
        if !stdout_str.ends_with('\n') {
            println!();
        }
        printed_any = true;
    }

    if !stderr_str.is_empty() {
        println!("====== (hook-stderr: {label}) ======");
        print!("{stderr_str}");
        if !stderr_str.ends_with('\n') {
            println!();
        }
        printed_any = true;
    }

    // Close the framed output with an end divider so the hook's output is clearly
    // separated from whatever `mind` prints next (e.g. the install preview).
    if printed_any {
        println!("====== (end hook: {label}) ======");
    }

    if output.status.success() {
        Ok(())
    } else {
        Err(MindError::HookFailed {
            identity: identity.to_string(),
            command: command.to_string(),
            status: Some(output.status),
            stderr: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// `decide` runs unattended under `--dangerously-skip` (HOOK-23) and skips in
    /// a non-TTY context (HOOK-22) without prompting, for both optional and
    /// required hooks. (The interactive run/skip/abort branches need a TTY and are
    /// covered by the prompt-parsing tests.)
    // spec: HOOK-22, HOOK-23
    #[test]
    fn decide_dangerously_skip_runs_and_non_tty_skips() {
        // dangerously_skip => Run regardless of optionality (no prompt).
        assert_eq!(decide("d", false, true).unwrap(), HookAct::Run);
        assert_eq!(decide("d", true, true).unwrap(), HookAct::Run);
        // Test runs with no TTY on stdout, so a non-skip decision is Skip, never
        // Abort or Run (HOOK-22: never run silently).
        assert_eq!(decide("d", false, false).unwrap(), HookAct::Skip);
        assert_eq!(decide("d", true, false).unwrap(), HookAct::Skip);
    }

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
    fn parse_hook_choice_run_on_y() {
        assert_eq!(parse_hook_choice("y"), HookChoice::RunAndContinue);
        assert_eq!(parse_hook_choice("Y"), HookChoice::RunAndContinue);
    }

    // spec: HOOK-20
    // Empty input (bare Enter) now RUNS the hook - the key default flip.
    #[test]
    fn parse_hook_choice_run_on_empty_default() {
        assert_eq!(parse_hook_choice(""), HookChoice::RunAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_skip_on_n() {
        assert_eq!(parse_hook_choice("n"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice("N"), HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_abort_on_a() {
        assert_eq!(parse_hook_choice("a"), HookChoice::Abort);
        assert_eq!(parse_hook_choice("A"), HookChoice::Abort);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_garbage_defaults_to_skip() {
        assert_eq!(parse_hook_choice("garbage"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice("1"), HookChoice::SkipAndContinue);
        assert_eq!(parse_hook_choice("yes"), HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    #[test]
    fn parse_hook_choice_trims_whitespace() {
        assert_eq!(parse_hook_choice(" y "), HookChoice::RunAndContinue);
        assert_eq!(parse_hook_choice("\ta\n"), HookChoice::Abort);
        assert_eq!(parse_hook_choice(" n "), HookChoice::SkipAndContinue);
    }

    // ---- read_choice (tests the stdin read path, not just the parser) ----

    // spec: HOOK-20, HOOK-22
    // EOF (empty reader) must produce SkipAndContinue: a non-TTY or absent
    // reply never runs and never aborts.
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
    // "y\n" through the read path must produce RunAndContinue.
    #[test]
    fn read_choice_run_and_continue_on_y() {
        let reader = std::io::Cursor::new("y\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::RunAndContinue);
    }

    // spec: HOOK-20
    // "n\n" through the read path must produce SkipAndContinue.
    #[test]
    fn read_choice_skip_and_continue_on_n() {
        let reader = std::io::Cursor::new("n\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::SkipAndContinue);
    }

    // spec: HOOK-20
    // A bare "\n" (interactive Enter) through the read path must produce
    // RunAndContinue - the default-run behavior.
    #[test]
    fn read_choice_run_and_continue_on_bare_newline() {
        let reader = std::io::Cursor::new("\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(
            result,
            HookChoice::RunAndContinue,
            "bare Enter must yield RunAndContinue (default Y)"
        );
    }

    // spec: HOOK-20
    // "a\n" through the read path must produce Abort.
    #[test]
    fn read_choice_abort_on_a() {
        let reader = std::io::Cursor::new("a\n");
        let result = read_choice(reader).expect("read_choice should not error");
        assert_eq!(result, HookChoice::Abort);
    }

    // ---- parse_optional_choice ----

    // spec: HOOK-52
    #[test]
    fn parse_optional_choice_run_on_y() {
        assert_eq!(parse_optional_choice("y"), OptionalChoice::Run);
        assert_eq!(parse_optional_choice("Y"), OptionalChoice::Run);
    }

    // spec: HOOK-52
    // Empty input (bare Enter) now RUNS the optional hook - the default flip.
    #[test]
    fn parse_optional_choice_run_on_empty_default() {
        assert_eq!(parse_optional_choice(""), OptionalChoice::Run);
    }

    // spec: HOOK-52
    #[test]
    fn parse_optional_choice_skip_on_n() {
        assert_eq!(parse_optional_choice("n"), OptionalChoice::Skip);
    }

    // spec: HOOK-52
    #[test]
    fn parse_optional_choice_garbage_defaults_to_skip() {
        assert_eq!(parse_optional_choice("garbage"), OptionalChoice::Skip);
        assert_eq!(parse_optional_choice("yes"), OptionalChoice::Skip);
        assert_eq!(parse_optional_choice("1"), OptionalChoice::Skip);
    }

    // spec: HOOK-52
    #[test]
    fn parse_optional_choice_trims_whitespace() {
        assert_eq!(parse_optional_choice(" y "), OptionalChoice::Run);
        assert_eq!(parse_optional_choice("\ty\n"), OptionalChoice::Run);
        assert_eq!(parse_optional_choice(" n "), OptionalChoice::Skip);
    }

    // ---- read_optional_choice ----

    // spec: HOOK-52
    // "y\n" through the read path must produce Run.
    #[test]
    fn read_optional_choice_run_on_y() {
        let reader = std::io::Cursor::new("y\n");
        let result = read_optional_choice(reader).expect("no error");
        assert_eq!(result, OptionalChoice::Run);
    }

    // spec: HOOK-52
    // "n\n" through the read path must produce Skip.
    #[test]
    fn read_optional_choice_skip_on_n() {
        let reader = std::io::Cursor::new("n\n");
        let result = read_optional_choice(reader).expect("no error");
        assert_eq!(result, OptionalChoice::Skip);
    }

    // spec: HOOK-52, HOOK-22
    // EOF (empty reader) must produce Skip: a non-TTY or absent reply never runs.
    #[test]
    fn read_optional_choice_eof_returns_skip() {
        let reader = std::io::Cursor::new("");
        let result = read_optional_choice(reader).expect("no error on EOF");
        assert_eq!(result, OptionalChoice::Skip, "EOF must yield Skip, not Run");
    }

    // spec: HOOK-52
    // A bare "\n" (interactive Enter) through the read path must produce Run.
    #[test]
    fn read_optional_choice_run_on_bare_newline() {
        let reader = std::io::Cursor::new("\n");
        let result = read_optional_choice(reader).expect("no error");
        assert_eq!(
            result,
            OptionalChoice::Run,
            "bare Enter must yield Run (default Y)"
        );
    }

    // ---- disclosure_text ----

    // spec: HOOK-2
    #[test]
    fn disclosure_text_contains_separator_header() {
        let text = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/home/user/.mind/sources/github.com/acme/tools",
            "make install",
            None,
        );
        assert!(
            text.starts_with("====== hook: github.com/acme/tools ======\n"),
            "disclosure_text must start with the separator header; got: {text}"
        );
    }

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

    // spec: HOOK-2
    // disclosure_text must produce the same output as composing the header plus
    // disclosure_body directly. This guards the L1 DRY refactor: one source of
    // truth for the field layout.
    #[test]
    fn disclosure_text_matches_header_plus_disclosure_body() {
        let identity = "github.com/acme/tools";
        let pin_desc = "main";
        let commit = "abc1234";
        let clone_path = "/home/user/.mind/sources/github.com/acme/tools";
        let command = "make install";

        let via_fn = disclosure_text(identity, pin_desc, commit, clone_path, command, None);

        let mut expected = String::new();
        expected.push_str("====== hook: ");
        expected.push_str(identity);
        expected.push_str(" ======\n");
        expected.push_str(&disclosure_body(
            identity, pin_desc, commit, clone_path, command, None,
        ));

        assert_eq!(
            via_fn, expected,
            "disclosure_text output must equal header + disclosure_body"
        );
    }

    // spec: HOOK-2
    // Same as above but with a declared_override, to cover the override branch.
    #[test]
    fn disclosure_text_matches_header_plus_disclosure_body_with_override() {
        let identity = "github.com/acme/tools";
        let pin_desc = "v1.0";
        let commit = "def5678";
        let clone_path = "/tmp/clone";
        let command = "./user-custom.sh";
        let declared = "make install";

        let via_fn = disclosure_text(
            identity,
            pin_desc,
            commit,
            clone_path,
            command,
            Some(declared),
        );

        let mut expected = String::new();
        expected.push_str("====== hook: ");
        expected.push_str(identity);
        expected.push_str(" ======\n");
        expected.push_str(&disclosure_body(
            identity,
            pin_desc,
            commit,
            clone_path,
            command,
            Some(declared),
        ));

        assert_eq!(
            via_fn, expected,
            "disclosure_text with override must equal header + disclosure_body"
        );
    }

    // ---- hook_disclosure_text ----

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_has_separator_header_with_label() {
        let text = hook_disclosure_text(
            "Build step",
            true,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
        );
        assert!(
            text.starts_with("====== hook: Build step ======\n"),
            "hook_disclosure_text must start with '====== hook: <label> ======'; got: {text}"
        );
    }

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_optional_contains_label_and_optional_marker() {
        let text = hook_disclosure_text(
            "Build step",
            true,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
        );
        assert!(text.contains("Build step"), "missing label");
        assert!(text.contains("optional"), "missing optional marker");
        assert!(!text.contains("required"), "should not say required");
        assert!(text.contains("github.com/acme/tools"), "missing identity");
        assert!(text.contains("make install"), "missing command");
        assert!(text.contains("arbitrary"), "missing arbitrary-code warning");
    }

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_required_contains_required_marker() {
        let text = hook_disclosure_text(
            "setup.sh",
            false,
            "github.com/acme/tools",
            "v1.0",
            "def5678",
            "/tmp/clone",
            "setup.sh",
            None,
        );
        assert!(text.contains("setup.sh"), "missing label/command");
        assert!(text.contains("required"), "missing required marker");
        assert!(text.contains("github.com/acme/tools"), "missing identity");
        assert!(text.contains("arbitrary"), "missing arbitrary-code warning");
    }

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_override_shows_both_commands() {
        let text = hook_disclosure_text(
            "custom.sh",
            false,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "./user-custom.sh",
            Some("make install"),
        );
        assert!(text.contains("make install"), "missing declared command");
        assert!(
            text.contains("./user-custom.sh"),
            "missing override command"
        );
        assert!(text.contains("replaces"), "missing replacement note");
        assert!(text.contains("arbitrary"), "missing arbitrary-code warning");
    }

    // ---- apply_install_override ----

    // spec: HOOK-56
    #[test]
    fn apply_install_override_none_supplied_returns_unchanged() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![ResolvedHook {
            run: "make install".into(),
            name: None,
            optional: false,
            event: HookEvent::Install,
        }];
        let (result, replaced) = apply_install_override(hooks.clone(), None);
        assert_eq!(result, hooks, "hooks must be unchanged");
        assert!(replaced.is_none(), "no override => replaced is None");
    }

    // spec: HOOK-56
    #[test]
    fn apply_install_override_empty_supplied_returns_unchanged() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![ResolvedHook {
            run: "make install".into(),
            name: None,
            optional: false,
            event: HookEvent::Install,
        }];
        let (result, replaced) = apply_install_override(hooks.clone(), Some(""));
        assert_eq!(result, hooks, "empty supplied => unchanged");
        assert!(replaced.is_none());

        let (result2, replaced2) = apply_install_override(hooks.clone(), Some("   "));
        assert_eq!(result2, hooks, "whitespace supplied => unchanged");
        assert!(replaced2.is_none());
    }

    // spec: HOOK-56
    #[test]
    fn apply_install_override_replaces_declared_install_and_returns_them() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![
            ResolvedHook {
                run: "make build".into(),
                name: Some("Build".into()),
                optional: false,
                event: HookEvent::Install,
            },
            ResolvedHook {
                run: "make install".into(),
                name: None,
                optional: false,
                event: HookEvent::Install,
            },
        ];
        let (result, replaced) = apply_install_override(hooks, Some("./custom.sh"));
        // Result has exactly one install hook (the override).
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].run, "./custom.sh");
        assert_eq!(result[0].event, HookEvent::Install);
        assert!(!result[0].optional);
        assert_eq!(result[0].name, None);
        // Replaced lists both original install commands.
        let replaced = replaced.expect("should be Some when install hooks were declared");
        assert_eq!(replaced, vec!["make build", "make install"]);
    }

    // spec: HOOK-56
    #[test]
    fn apply_install_override_uninstall_hooks_survive_in_order() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![
            ResolvedHook {
                run: "make install".into(),
                name: None,
                optional: false,
                event: HookEvent::Install,
            },
            ResolvedHook {
                run: "first-uninstall".into(),
                name: Some("First".into()),
                optional: false,
                event: HookEvent::Uninstall,
            },
            ResolvedHook {
                run: "second-uninstall".into(),
                name: None,
                optional: true,
                event: HookEvent::Uninstall,
            },
        ];
        let (result, replaced) = apply_install_override(hooks, Some("./override.sh"));
        // First entry: override install hook.
        assert_eq!(result[0].run, "./override.sh");
        assert_eq!(result[0].event, HookEvent::Install);
        // Then the uninstall hooks in original order.
        assert_eq!(result[1].run, "first-uninstall");
        assert_eq!(result[1].event, HookEvent::Uninstall);
        assert_eq!(result[2].run, "second-uninstall");
        assert_eq!(result[2].event, HookEvent::Uninstall);
        assert_eq!(result.len(), 3);
        // Replaced contains the original install command.
        assert_eq!(replaced, Some(vec!["make install".to_string()]));
    }

    // spec: HOOK-56
    // When the source declared no install hooks, supplied adds one, and replaced is None.
    #[test]
    fn apply_install_override_no_declared_install_adds_hook_replaced_is_none() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![ResolvedHook {
            run: "teardown.sh".into(),
            name: None,
            optional: false,
            event: HookEvent::Uninstall,
        }];
        let (result, replaced) = apply_install_override(hooks, Some("./new-install.sh"));
        // One install hook + one uninstall hook.
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].run, "./new-install.sh");
        assert_eq!(result[0].event, HookEvent::Install);
        assert_eq!(result[1].run, "teardown.sh");
        assert_eq!(result[1].event, HookEvent::Uninstall);
        // No install hooks were declared, so replaced is None.
        assert!(
            replaced.is_none(),
            "no declared install hooks => replaced is None even when supplied is given"
        );
    }

    // spec: HOOK-59
    #[test]
    fn apply_hook_override_uninstall_replaces_uninstall_and_keeps_install() {
        use crate::mindfile::{HookEvent, ResolvedHook};
        let hooks = vec![
            ResolvedHook {
                run: "build".into(),
                name: None,
                optional: false,
                event: HookEvent::Install,
            },
            ResolvedHook {
                run: "old-teardown".into(),
                name: None,
                optional: false,
                event: HookEvent::Uninstall,
            },
        ];
        let (result, replaced) =
            apply_hook_override(hooks, Some("./new-teardown.sh"), HookEvent::Uninstall);
        // The override uninstall hook replaces the declared one; the install hook
        // is untouched.
        let uninstall: Vec<&ResolvedHook> = result
            .iter()
            .filter(|h| h.event == HookEvent::Uninstall)
            .collect();
        assert_eq!(uninstall.len(), 1);
        assert_eq!(uninstall[0].run, "./new-teardown.sh");
        assert!(
            result
                .iter()
                .any(|h| h.event == HookEvent::Install && h.run == "build"),
            "the install hook must survive an uninstall override"
        );
        assert_eq!(replaced.as_deref(), Some(&["old-teardown".to_string()][..]));
    }

    // ---- run_hook ----

    // spec: HOOK-30
    #[test]
    fn run_hook_success_creates_marker_file() {
        let dir = TempDir::new("success");
        let marker = dir.path().join("marker.txt");
        let marker_str = marker.to_str().expect("marker path is utf8");
        let command = format!("touch {marker_str}");
        let result = run_hook(&command, dir.path(), "github.com/test/repo", "test-label");
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
        let result = run_hook("exit 3", dir.path(), "github.com/test/repo", "test-label");
        match result {
            Err(
                ref e @ MindError::HookFailed {
                    ref identity,
                    ref command,
                    status,
                    ref stderr,
                },
            ) => {
                assert_eq!(identity, "github.com/test/repo", "wrong identity");
                assert_eq!(command, "exit 3", "wrong command");
                assert!(
                    status.is_some(),
                    "exit status should be Some for a process that ran"
                );
                let code = status.unwrap().code();
                assert_eq!(code, Some(3), "expected exit code 3, got {code:?}");
                // A silent hook (no output) must have an empty stderr field and the
                // rendered message must say "(no output)" rather than pointing at
                // framed output blocks that were never printed (M5).
                assert!(
                    stderr.is_empty(),
                    "run_hook must set stderr to empty for a process that produced no output"
                );
                let msg = e.to_string();
                assert!(
                    msg.contains("(no output)"),
                    "silent hook failure must render '(no output)': {msg}"
                );
                assert!(
                    !msg.contains("see the hook"),
                    "must not say 'see the hook's output above' when nothing was printed: {msg}"
                );
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    #[test]
    fn run_hook_identity_and_command_propagate_to_error() {
        let dir = TempDir::new("propagate");
        let result = run_hook("false", dir.path(), "github.com/acme/special", "my-hook");
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
