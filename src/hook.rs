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

/// Whether stdin is an interactive terminal (the HOOK-22 gate).
///
/// `$MIND_TTY` overrides the answer (HOOK-109) so the interactive branches are
/// reachable from a headless test, following the `MIND_HOME` / `CLAUDE_HOME` /
/// `MIND_DETECT_HOME` test-isolation precedent (`paths.rs`): the variable is
/// read first and, when set, decides on its own; `is_terminal()` is the
/// fallback consulted only when the variable is absent. Values are read as a
/// boolean by [`tty_override`].
// spec: HOOK-109
pub fn is_tty() -> bool {
    match std::env::var_os("MIND_TTY") {
        Some(v) => tty_override(&v),
        None => std::io::stdin().is_terminal(),
    }
}

/// Read a `$MIND_TTY` value as a boolean (HOOK-109). Empty, `0`, `false`, `no`,
/// and `off` (any case, surrounding whitespace ignored) mean "not a terminal";
/// every other value means "a terminal". Pure, so the mapping is unit-testable
/// without mutating the process environment.
fn tty_override(value: &std::ffi::OsStr) -> bool {
    !matches!(
        value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
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
    browse_url: Option<&str>,
) -> String {
    // Sanitize identity for the header line; disclosure_body sanitizes all body
    // fields independently (HOOK-91).
    let identity_clean = crate::sanitize::strip_ansi(identity);
    let mut out = String::new();
    out.push_str("====== hook: ");
    out.push_str(&identity_clean);
    out.push_str(" ======\n");
    out.push_str(&disclosure_body(
        identity,
        pin_desc,
        commit,
        clone_path,
        command,
        declared_override,
        browse_url,
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

/// Like `disclosure_text` but prefixed with the hook's label, its lifecycle
/// event, and whether it is required or optional, for the multi-hook
/// disclosures (HOOK-52). Prepends a `====== hook: {label} ======` header so the
/// block is visually distinct.
///
/// spec: HOOK-20 HOOK-120 -- the `Event:` line is what tells the user WHICH
/// step they are approving. Without it an update hook's disclosure is
/// indistinguishable from an install hook's (and an uninstall hook's from
/// either), on the one surface where the answer decides whether arbitrary
/// source code runs.
#[allow(clippy::too_many_arguments)]
pub fn hook_disclosure_text(
    label: &str,
    event: &str,
    optional: bool,
    identity: &str,
    pin_desc: &str,
    commit: &str,
    clone_path: &str,
    command: &str,
    declared_override: Option<&str>,
    browse_url: Option<&str>,
) -> String {
    // Sanitize label for the header and "Hook:" lines; disclosure_body
    // sanitizes identity and the remaining body fields independently (HOOK-91).
    let label_clean = crate::sanitize::strip_ansi(label);
    let kind = if optional { "optional" } else { "required" };
    let mut out = String::new();
    out.push_str("====== hook: ");
    out.push_str(&label_clean);
    out.push_str(" ======\n");
    out.push_str("  Hook:      ");
    out.push_str(&label_clean);
    out.push_str(" (");
    out.push_str(kind);
    out.push_str(")\n");
    // The event is `mind`'s own word (install/update/uninstall/build), not
    // source-derived, so it needs no sanitizing.
    out.push_str("  Event:     ");
    out.push_str(event);
    out.push('\n');
    // Append the base disclosure fields (without its own header since we
    // already prepended one above; call the inner builder directly).
    out.push_str(&disclosure_body(
        identity,
        pin_desc,
        commit,
        clone_path,
        command,
        declared_override,
        browse_url,
    ));
    out
}

/// The fields-only portion of a disclosure block (no header line). Used by
/// both `disclosure_text` (which prepends its own header) and
/// `hook_disclosure_text` (which prepends a different header then calls this).
///
/// All source-derived fields are sanitized (ANSI/control/bidi stripped) before
/// being included in the returned string (HOOK-91). When `browse_url` is
/// `Some`, a `Browse:` line is emitted after `Clone:` (HOOK-24).
fn disclosure_body(
    identity: &str,
    pin_desc: &str,
    commit: &str,
    clone_path: &str,
    command: &str,
    declared_override: Option<&str>,
    browse_url: Option<&str>,
) -> String {
    // Sanitize all source-derived fields: ANSI escapes, C0/DEL/C1 control
    // characters, and bidi-override code points are stripped so a malicious
    // source cannot use cursor/line-clear sequences or bidi reordering to make
    // a dangerous command appear innocuous on the exact surface where the user
    // consents to run it (HOOK-91, S8).
    let identity = crate::sanitize::strip_ansi(identity);
    let pin_desc = crate::sanitize::strip_ansi(pin_desc);
    let commit = crate::sanitize::strip_ansi(commit);
    let clone_path = crate::sanitize::strip_ansi(clone_path);
    let command = crate::sanitize::strip_ansi(command);
    let declared_override_owned: Option<String> =
        declared_override.map(crate::sanitize::strip_ansi);
    let declared_override = declared_override_owned.as_deref();
    // spec: HOOK-24, HOOK-91 - browse URL is source-derived and must be sanitized.
    let browse_url_owned: Option<String> = browse_url.map(crate::sanitize::strip_ansi);
    let browse_url = browse_url_owned.as_deref();

    let mut out = String::new();

    out.push_str("  Source:    ");
    out.push_str(&identity);
    out.push('\n');

    out.push_str("  Pin:       ");
    out.push_str(&pin_desc);
    out.push('\n');

    out.push_str("  Commit:    ");
    out.push_str(&commit);
    out.push('\n');

    out.push_str("  Clone:     ");
    out.push_str(&clone_path);
    out.push('\n');

    // spec: HOOK-24 - emit Browse: line only when a URL is available.
    if let Some(url) = browse_url {
        out.push_str("  Browse:    ");
        out.push_str(url);
        out.push('\n');
    }

    if let Some(declared) = declared_override {
        out.push_str("  Declared:  ");
        out.push_str(declared);
        out.push('\n');
        out.push_str("  Override:  ");
        out.push_str(&command);
        out.push('\n');
        out.push_str("  NOTE: the user-supplied command replaces the source's declared command.\n");
    } else {
        out.push_str("  Command:   ");
        out.push_str(&command);
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
/// Stdout and stderr are INHERITED, so the hook's output reaches the terminal
/// as it is produced rather than being collected and replayed at exit. Stdin is
/// closed so a hook cannot consume mind's input. A non-zero exit (or spawn
/// failure) maps to `MindError::HookFailed`.
///
/// Streaming is the point: a hook is often the slowest step of a meld (a build,
/// a package install), and a progress bar, a compiler's file-by-file output, or
/// a prompt-looking line that arrives only after the command finishes tells the
/// user nothing while they are waiting, and cannot be interrupted on the basis
/// of what it says. The cost is that the two streams interleave exactly as they
/// would in a terminal, so they can no longer be framed under separate
/// `hook-stdout`/`hook-stderr` labels, and the frame is unconditional: nothing
/// is buffered, so mind cannot know in advance whether the hook will print.
///
/// The frame below is a plain `println!` on purpose, NOT `render::note`: what
/// it surrounds is arbitrary text chosen by the source author, so no per-line
/// routing rule could bound it (a hook that printed `note:`-prefixed lines, or
/// a forged result envelope, would defeat one). Under `--json`, `main.rs`'s
/// `json_stdout` points fd 1 at fd 2 for the whole run (a real `dup2`) and
/// marks the saved real stdout close-on-exec, so no descriptor that reaches
/// the result document is inherited by the child: a streaming hook cannot
/// corrupt the result document either.
///
/// `event` is what the hook is ("install", "uninstall", "build") and is fixed
/// by the call site, never by the source. `label` is what the frame is headed
/// with and IS source-controlled (a `[[hooks]].name` from `mind.toml`, or the
/// raw `run` command when there is none), so it is sanitized (`strip_ansi`,
/// DSC-95) before it reaches the frame or any `HookFailed` this returns:
/// unsanitized, a hook name carrying cursor-control escapes could erase mind's
/// own progress output the moment the frame opens. Keeping the two apart is
/// what stops a failing uninstall hook from being reported as an install one.
// spec: CLI-217 HOOK-30
pub fn run_hook(
    command: &str,
    clone_dir: &Path,
    identity: &str,
    event: &'static str,
    label: &str,
) -> Result<()> {
    let label = crate::sanitize::strip_ansi(label);
    let command_clean = crate::sanitize::strip_ansi(command);

    // Flush mind's own buffered output first so it does not interleave with
    // the streamed hook output below.
    let _ = std::io::stdout().flush();

    // Opened before the spawn so the header is on screen while the hook runs,
    // not after it finishes.
    println!("====== (hook: {label}) ======");
    let _ = std::io::stdout().flush();

    let status = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(clone_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| {
            // The spawn never ran, so the frame opened above is left dangling
            // unless we close it here too (L2): HOOK-30 describes the frame as
            // a pair, and the opener must still print before the spawn is
            // attempted (so the header is visible while mind decides whether
            // the spawn will even succeed).
            println!("====== (end hook: {label}) ======");
            MindError::HookFailed {
                event,
                label: label.clone(),
                identity: identity.to_string(),
                command: command_clean.clone(),
                status: None,
                reason: e.to_string(),
                // Spawn failed, so nothing was streamed: the error carries the
                // reason itself rather than pointing at output that never existed.
                streamed: false,
            }
        })?;

    println!("====== (end hook: {label}) ======");

    if status.success() {
        Ok(())
    } else {
        // The hook wrote straight to the terminal, so there is nothing to
        // replay and nothing captured to attach: whatever it said is already on
        // screen inside the frame above. `streamed` means "the process ran",
        // not "it printed something": with inherited streams mind never saw a
        // byte, so it cannot tell those apart, and the message says where
        // output WOULD be rather than asserting that any exists (HOOK-30).
        Err(MindError::HookFailed {
            event,
            label,
            identity: identity.to_string(),
            command: command_clean,
            status: Some(status),
            reason: String::new(),
            streamed: true,
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

    /// `$MIND_TTY` is read as a boolean: the falsey spellings mean "not a
    /// terminal" and everything else means "a terminal". Driven through the
    /// pure mapping so no process-wide env mutation is needed (the variable
    /// itself is exercised end to end by the `hooks run` integration tests).
    // spec: HOOK-109
    #[test]
    fn tty_override_reads_a_boolean() {
        use std::ffi::OsStr;
        for falsey in ["", "0", "false", "FALSE", "no", "off", "  0  "] {
            assert!(
                !tty_override(OsStr::new(falsey)),
                "{falsey:?} must read as not-a-terminal"
            );
        }
        for truthy in ["1", "true", "TRUE", "yes", "on", "anything"] {
            assert!(
                tty_override(OsStr::new(truthy)),
                "{truthy:?} must read as a terminal"
            );
        }
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
            None,
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

        let via_fn = disclosure_text(identity, pin_desc, commit, clone_path, command, None, None);

        let mut expected = String::new();
        expected.push_str("====== hook: ");
        expected.push_str(identity);
        expected.push_str(" ======\n");
        expected.push_str(&disclosure_body(
            identity, pin_desc, commit, clone_path, command, None, None,
        ));

        assert_eq!(
            via_fn, expected,
            "disclosure_text output must equal header + disclosure_body"
        );
    }

    // spec: HOOK-91
    // Source-derived fields containing ANSI escapes and bidi-override code points
    // must be stripped from the produced disclosure string before it reaches the
    // terminal where the user consents to run the hook.
    #[test]
    fn disclosure_sanitizes_ansi_and_bidi_in_source_fields() {
        // The ESC byte (0x1b) starts ANSI sequences; U+202E is a bidi-override.
        let ansi_command = "\x1b[2K\x1b[1A rm -rf /  \x1b[0m";
        let bidi_identity = "github.com/\u{202E}kcal/evil\u{202E}";

        // Test disclosure_text: checks identity in header and command in body.
        let text = disclosure_text(
            bidi_identity,
            "main",
            "abc1234",
            "/tmp/clone",
            ansi_command,
            None,
            None,
        );
        assert!(
            !text.contains('\x1b'),
            "disclosure_text must not contain raw ESC byte; got: {text:?}"
        );
        assert!(
            !text.chars().any(|c| matches!(
                c,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )),
            "disclosure_text must not contain bidi-override code points; got: {text:?}"
        );

        // Test hook_disclosure_text: label is also source-derived (hook name from mind.toml).
        let ansi_label = "\x1b[31mbuild\x1b[0m\u{202E}";
        let text2 = hook_disclosure_text(
            ansi_label,
            "install",
            false,
            bidi_identity,
            "main",
            "abc1234",
            "/tmp/clone",
            ansi_command,
            None,
            None,
        );
        assert!(
            !text2.contains('\x1b'),
            "hook_disclosure_text must not contain raw ESC byte; got: {text2:?}"
        );
        assert!(
            !text2.chars().any(|c| matches!(
                c,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )),
            "hook_disclosure_text must not contain bidi-override code points; got: {text2:?}"
        );

        // Test with declared_override path: both declared and override commands sanitized.
        let text3 = disclosure_text(
            bidi_identity,
            "v1.0",
            "def5678",
            "/tmp/clone",
            ansi_command,
            Some("\x1b[32m malicious_decl \x1b[0m"),
            None,
        );
        assert!(
            !text3.contains('\x1b'),
            "disclosure with override must not contain raw ESC byte; got: {text3:?}"
        );

        // spec: HOOK-91 - browse URL is also source-derived and must be sanitized.
        let ansi_browse_url = "\x1b[34mhttps://github.com/evil/repo/tree/abc\u{202E}1234\x1b[0m";
        let text4 = disclosure_text(
            "github.com/evil/repo",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
            Some(ansi_browse_url),
        );
        assert!(
            !text4.contains('\x1b'),
            "disclosure with browse_url must not contain raw ESC byte; got: {text4:?}"
        );
        assert!(
            !text4.chars().any(|c| matches!(
                c,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )),
            "disclosure with browse_url must not contain bidi-override code points; got: {text4:?}"
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
            None,
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
            None,
        ));

        assert_eq!(
            via_fn, expected,
            "disclosure_text with override must equal header + disclosure_body"
        );
    }

    // spec: HOOK-24
    // The Browse: line must appear in the disclosure when Some and be absent
    // when None, and must never appear when the URL was stripped to empty.
    #[test]
    fn disclosure_browse_url_line_present_when_some_absent_when_none() {
        // With a URL: Browse: line must appear.
        let with_url = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
            Some("https://github.com/acme/tools/tree/abc1234"),
        );
        assert!(
            with_url.contains("Browse:"),
            "disclosure must contain Browse: line when browse_url is Some; got: {with_url}"
        );
        assert!(
            with_url.contains("https://github.com/acme/tools/tree/abc1234"),
            "disclosure must contain the browse URL; got: {with_url}"
        );

        // Without a URL: Browse: line must not appear.
        let without_url = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
            None,
        );
        assert!(
            !without_url.contains("Browse:"),
            "disclosure must not contain Browse: line when browse_url is None; got: {without_url}"
        );
    }

    // spec: HOOK-24
    // The Browse: line is positioned immediately after the Clone: line (HOOK-24:
    // the browse URL is added alongside the clone path). Assert ordering, not
    // just presence: a regression that emitted Browse before Clone, or after the
    // command/WARNING block, would still pass a bare `contains` check.
    #[test]
    fn disclosure_browse_line_positioned_after_clone() {
        let text = disclosure_text(
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/home/user/.mind/sources/github.com/acme/tools",
            "make install",
            None,
            Some("https://github.com/acme/tools/tree/abc1234"),
        );

        let clone_at = text.find("  Clone:     ").expect("Clone line present");
        let browse_at = text.find("  Browse:    ").expect("Browse line present");
        assert!(
            browse_at > clone_at,
            "Browse: line must come after Clone: line; got: {text}"
        );

        // Nothing but the two field lines may sit between them: Browse must
        // immediately follow the Clone line (Clone value has no embedded newline
        // in this fixture), so the text between the Clone label and the Browse
        // label contains exactly one newline.
        let between = &text[clone_at..browse_at];
        assert_eq!(
            between.matches('\n').count(),
            1,
            "Browse: must be the line immediately after Clone:; got between: {between:?}"
        );

        // And the Browse line must precede the WARNING/command consent block.
        let warn_at = text
            .find("WARNING")
            .or_else(|| text.find("make install"))
            .expect("consent block present");
        assert!(
            browse_at < warn_at,
            "Browse: line must precede the command/WARNING consent block; got: {text}"
        );
    }

    // ---- hook_disclosure_text ----

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_has_separator_header_with_label() {
        let text = hook_disclosure_text(
            "Build step",
            "install",
            true,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
            None,
        );
        assert!(
            text.starts_with("====== hook: Build step ======\n"),
            "hook_disclosure_text must start with '====== hook: <label> ======'; got: {text}"
        );
    }

    /// The consent block names the lifecycle event, so approving an update or
    /// uninstall hook is never indistinguishable from approving an install one.
    #[test]
    fn hook_disclosure_text_names_the_lifecycle_event() {
        // spec: HOOK-20 HOOK-120
        for event in ["install", "update", "uninstall"] {
            let text = hook_disclosure_text(
                "Build step",
                event,
                false,
                "github.com/acme/tools",
                "main",
                "abc1234",
                "/tmp/clone",
                "make install",
                None,
                None,
            );
            assert!(
                text.contains(&format!("Event:     {event}")),
                "the disclosure must name the {event} event: {text}"
            );
        }
    }

    // spec: HOOK-52
    #[test]
    fn hook_disclosure_text_optional_contains_label_and_optional_marker() {
        let text = hook_disclosure_text(
            "Build step",
            "install",
            true,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "make install",
            None,
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
            "install",
            false,
            "github.com/acme/tools",
            "v1.0",
            "def5678",
            "/tmp/clone",
            "setup.sh",
            None,
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
            "install",
            false,
            "github.com/acme/tools",
            "main",
            "abc1234",
            "/tmp/clone",
            "./user-custom.sh",
            Some("make install"),
            None,
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
    // M11: `label` is source-controlled (a mind.toml `[[hooks]].name`, or the
    // raw `run` command as a fallback) and is written directly into both hook
    // frame lines (`println!("====== (hook: {label}) ======")` and its
    // closing pair) as well as into any `HookFailed` this returns. An
    // ANSI-bearing label (e.g. cursor-erase sequences) must not reach the
    // real terminal at the moment the frame opens -- that is precisely when
    // mind guarantees it happens, on every hook run, and could erase mind's
    // own just-printed progress output. `command` is displayed raw by
    // `HookFailed`'s `Display` too, so it must be sanitized for the error even
    // though the unsanitized value is still what actually gets executed.
    //
    // This asserts on the exact `label`/`command` strings `run_hook` feeds to
    // both `println!` and `HookFailed` (returned here, and embedded in its
    // `Display`) rather than capturing the real terminal: genuine OS-level fd
    // capture is not reliable under `cargo test`'s parallel harness, where
    // other tests' own status lines and child-process output share the same
    // real stdout descriptor. This mirrors how `disclosure_text` /
    // `hook_disclosure_text` are tested elsewhere in this file (asserting on
    // the sanitized string content, not the terminal).
    #[test]
    fn run_hook_sanitizes_ansi_in_label_and_command_for_the_error() {
        let dir = TempDir::new("ansi-label");
        let evil_label = "\x1b[2K\x1b[1A\x1b[2Kbuild";
        // '#' starts a comment in `sh`, so the trailing escape bytes are
        // inert: this always exits 9, while `command` (as text) still carries
        // a raw ESC for the sanitization check below.
        let evil_command = "exit 9 # \x1b[31mcolor\x1b[0m";

        let result = run_hook(
            evil_command,
            dir.path(),
            "github.com/test/repo",
            "install",
            evil_label,
        );
        let err = result.expect_err("a non-zero exit must produce HookFailed");
        let msg = err.to_string();
        assert!(
            !msg.contains('\x1b'),
            "raw ESC must not appear in the rendered error: {msg:?}"
        );

        match err {
            MindError::HookFailed {
                ref label,
                ref command,
                ..
            } => {
                assert!(
                    !label.contains('\x1b'),
                    "label field must not carry raw ESC: {label:?}"
                );
                assert!(
                    label.contains("build"),
                    "sanitized label text must remain: {label:?}"
                );
                assert!(
                    !command.contains('\x1b'),
                    "command field must not carry raw ESC: {command:?}"
                );
                assert!(
                    command.contains("color"),
                    "sanitized command text must remain: {command:?}"
                );
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    // L2: a spawn failure returns right after the opening frame is printed;
    // the closing divider must still print on that path so the frame stays a
    // matched pair (HOOK-30), not just when a process actually ran. The
    // return-value shape (`streamed: false`, a non-empty `reason`, `status:
    // None`) is exercised elsewhere; this specifically exercises that the
    // spawn-failure branch is the one taken (a nonexistent `clone_dir` makes
    // the underlying `chdir` in the child fail, so `Command::status()` itself
    // returns an `Err` rather than the process running and exiting non-zero).
    #[test]
    fn run_hook_spawn_failure_produces_unstreamed_hook_failed() {
        let missing_dir = std::env::temp_dir().join(format!(
            "mind-hook-test-missing-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        assert!(
            !missing_dir.exists(),
            "the dir must not exist for this test to force a spawn failure"
        );

        let result = run_hook(
            "true",
            &missing_dir,
            "github.com/test/repo",
            "install",
            "spawn-fail",
        );
        match result {
            Err(MindError::HookFailed {
                ref label,
                status,
                streamed,
                ref reason,
                ..
            }) => {
                assert_eq!(label, "spawn-fail");
                assert!(!streamed, "a spawn failure never ran the hook");
                assert!(
                    status.is_none(),
                    "a process that never ran has no exit status"
                );
                assert!(
                    !reason.is_empty(),
                    "a spawn failure must carry its own reason, since nothing was streamed"
                );
            }
            other => panic!("expected a spawn failure to produce HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    #[test]
    fn run_hook_success_creates_marker_file() {
        let dir = TempDir::new("success");
        let marker = dir.path().join("marker.txt");
        let marker_str = marker.to_str().expect("marker path is utf8");
        let command = format!("touch {marker_str}");
        let result = run_hook(
            &command,
            dir.path(),
            "github.com/test/repo",
            "install",
            "test-label",
        );
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
        let result = run_hook(
            "exit 3",
            dir.path(),
            "github.com/test/repo",
            "install",
            "test-label",
        );
        match result {
            Err(
                ref e @ MindError::HookFailed {
                    event,
                    ref label,
                    ref identity,
                    ref command,
                    status,
                    reason: ref stderr,
                    streamed,
                },
            ) => {
                assert_eq!(event, "install", "wrong event");
                assert_eq!(label, "test-label", "wrong label");
                assert_eq!(identity, "github.com/test/repo", "wrong identity");
                assert_eq!(command, "exit 3", "wrong command");
                assert!(
                    status.is_some(),
                    "exit status should be Some for a process that ran"
                );
                let code = status.unwrap().code();
                assert_eq!(code, Some(3), "expected exit code 3, got {code:?}");
                // spec: HOOK-30 -- the hook's streams were inherited, so nothing
                // was captured to attach and the frame is already on screen.
                // `streamed` is therefore true even for a hook that
                // printed nothing: streaming cannot tell a silent hook from a
                // talkative one, and the message points at the (empty) frame
                // rather than claiming "(no output)" for a hook that may have
                // printed plenty. The captured form used to distinguish these.
                assert!(
                    stderr.is_empty(),
                    "an inherited hook captures no stderr to attach"
                );
                assert!(
                    streamed,
                    "a hook that ran streamed its own output, so the error points at the frame"
                );
                let msg = e.to_string();
                assert!(
                    msg.contains("its output, if any, is in the frame above"),
                    "a streamed hook failure must point at the frame: {msg}"
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
    // A hook that prints output before failing must produce a HookFailed with
    // streamed=true, so its Display says "its output, if any, is in the frame above" rather than
    // the contradictory "(no output)".
    #[test]
    fn run_hook_with_output_before_failure_points_at_the_frame() {
        let dir = TempDir::new("output-fail");
        // Print to stdout, then exit non-zero.
        let result = run_hook(
            "echo 'hook output'; exit 2",
            dir.path(),
            "github.com/test/repo",
            "install",
            "test-label",
        );
        match result {
            Err(
                ref e @ MindError::HookFailed {
                    streamed,
                    reason: ref stderr,
                    ..
                },
            ) => {
                assert!(streamed, "streamed must be true when hook produced stdout");
                assert!(
                    stderr.is_empty(),
                    "stderr field must be empty (output was streamed, not captured)"
                );
                let msg = e.to_string();
                assert!(
                    msg.contains("its output, if any, is in the frame above"),
                    "must point at the frame when output was streamed: {msg}"
                );
                assert!(
                    !msg.contains("(no output)"),
                    "must not say '(no output)' when output was printed: {msg}"
                );
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    // Same as above but with stderr output (not stdout): the framed output path
    // uses `printed_any` which covers either stream.
    #[test]
    fn run_hook_with_stderr_output_before_failure_points_at_the_frame() {
        let dir = TempDir::new("stderr-fail");
        let result = run_hook(
            "echo 'hook stderr' >&2; exit 1",
            dir.path(),
            "github.com/test/repo",
            "install",
            "test-label",
        );
        match result {
            Err(ref e @ MindError::HookFailed { streamed, .. }) => {
                assert!(streamed, "streamed must be true when hook produced stderr");
                let msg = e.to_string();
                assert!(
                    msg.contains("its output, if any, is in the frame above"),
                    "stderr output must also trigger '(see output above)': {msg}"
                );
            }
            other => panic!("expected HookFailed, got: {other:?}"),
        }
    }

    // spec: HOOK-30
    #[test]
    fn run_hook_identity_and_command_propagate_to_error() {
        let dir = TempDir::new("propagate");
        let result = run_hook(
            "false",
            dir.path(),
            "github.com/acme/special",
            "install",
            "my-hook",
        );
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
