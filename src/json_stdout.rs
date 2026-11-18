//! CLI-217's enforcement mechanism: under `--json`, stdout is RESERVED for the
//! one result document the invoked verb answers with.
//!
//! Three rounds of per-site sweeps each shipped a fresh leak into `--json`
//! stdout (an unguarded `println!` ahead of the result, a nested verb's second
//! result object, a hook's own output), so the rule is enforced structurally
//! here instead of by discipline at every call site:
//!
//! 1. [`reserve`] moves the real stdout aside and points fd 1 at stderr for the
//!    rest of the process. After that NOTHING can reach stdout by printing:
//!    not a `println!` anywhere in the tree, not a `print!` in a module this
//!    change never touched, and not a child process that inherits fd 1 (a
//!    source's install hook, whose output is arbitrary text chosen by the
//!    source author -- the worst case in the class).
//! 2. Result documents are [`record`]ed rather than printed. The LAST one
//!    recorded wins, which in this call graph is the outermost frame's: a
//!    nested `learn` performed on `meld`'s or `sync`'s behalf records first and
//!    the invoked verb records after it, so the caller is answered by the verb
//!    it actually invoked (CLI-153).
//! 3. `main` writes that one document to the preserved stdout, exactly once, on
//!    the success path -- or the CLI-181 error envelope INSTEAD of it on the
//!    failure path, so a verb that mutated something and then failed cannot
//!    emit both.
//!
//! The two halves are deliberately coupled: recording (not printing) the
//! document is what lets fd 1 stay redirected for the whole run.

use std::io::Write;
use std::sync::Mutex;

/// The real stdout, moved aside by [`reserve`]. `None` when stdout was never
/// reserved: text mode, or a verb whose stdout payload is not a JSON
/// document (see `json_reserves_stdout`).
static REAL_STDOUT: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// The document this run will answer with; the most recently recorded one.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// Structured findings a verb recorded before failing (CLI-221), folded
/// into the CLI-181 error envelope's `details` member when present.
/// Distinct from `PENDING`: that is the SUCCESS-path document, which
/// `emit_instead` discards on failure; a verb that fails still wants its
/// findings to reach the caller, hence a second slot that survives the
/// discard.
static ERROR_DETAILS: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Duplicate fd 1 as close-on-exec, then point fd 1 at fd 2. Returns the
/// duplicate (the real stdout) on success. Mirrors the TUI's stdout-capture
/// redirect (`tui::action::with_captured_stdout`), which does the same
/// dance.
///
/// The duplicate is created with `F_DUPFD_CLOEXEC` (not a plain `dup`)
/// specifically so a hook or other child process spawned later in this run
/// cannot inherit it: `dup` clears FD_CLOEXEC on the new descriptor, and
/// `Command` only rewires fds 0/1/2 for a child, leaving every other open
/// fd -- including this saved stdout, typically fd 3 -- inherited as-is. A
/// hook could otherwise reach the preserved real stdout directly (e.g. a
/// shell redirect naming fd 3 explicitly) and inject arbitrary text ahead
/// of or instead of the one JSON document HOOK-32 promises stays
/// uncorrupted (see the CLI-217/HOOK-32 docs above and in
/// install-hooks.md).
// spec: HOOK-32
#[cfg(unix)]
fn move_stdout_aside() -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    // Nothing should be buffered this early, but flush before the swap so a
    // stray byte cannot land on the wrong description.
    let _ = std::io::stdout().flush();
    let saved = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 0) };
    if saved < 0 {
        return None;
    }
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        unsafe { libc::close(saved) };
        return None;
    }
    // SAFETY: `saved` is a fresh fd owned by nobody else.
    Some(unsafe { std::fs::File::from_raw_fd(saved) })
}

/// Non-unix fallback: no redirect, so `--json` output stays as it was
/// (correct for every already-guarded site, unenforced for the rest).
#[cfg(not(unix))]
fn move_stdout_aside() -> Option<std::fs::File> {
    None
}

/// Reserve stdout for this run's single JSON document. Idempotent; a failure
/// to redirect is silently tolerated (the verb still works, it just loses
/// the structural guarantee).
pub fn reserve() {
    let mut real = REAL_STDOUT.lock().unwrap_or_else(|e| e.into_inner());
    if real.is_none() {
        *real = move_stdout_aside();
    }
}

/// Whether stdout is reserved, i.e. whether a result document must be
/// recorded rather than printed.
pub fn is_reserved() -> bool {
    REAL_STDOUT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

/// Record `doc` as the document this run answers with, replacing any
/// document an inner frame recorded earlier.
pub fn record(doc: String) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(doc);
}

/// Write `doc` (plus a trailing newline, exactly as `println!` would) to the
/// preserved stdout, or to the process stdout when nothing was reserved.
fn write_out(doc: &str) {
    let mut real = REAL_STDOUT.lock().unwrap_or_else(|e| e.into_inner());
    match real.as_mut() {
        Some(f) => {
            let _ = writeln!(f, "{doc}");
            let _ = f.flush();
        }
        None => println!("{doc}"),
    }
}

/// Emit the recorded document, if any. Called once, by `main`, on success.
pub fn flush_pending() {
    let doc = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(doc) = doc {
        write_out(&with_hooks(doc));
    }
}

/// `doc` with this run's hook results as its `hooks` member (HOOK-161),
/// when any hook was considered and `doc` is a JSON object. Added where
/// the document is written (here, or `commands::print_json` when stdout
/// was not reserved), so every verb that runs hooks reports them without
/// each building the member itself.
// spec: HOOK-161
pub fn with_hooks(doc: String) -> String {
    let records = crate::hook_log::records();
    if records.is_empty() {
        return doc;
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&doc) else {
        return doc;
    };
    let Some(obj) = value.as_object_mut() else {
        return doc;
    };
    let Ok(hooks) = serde_json::to_value(&records) else {
        return doc;
    };
    obj.insert("hooks".to_string(), hooks);
    serde_json::to_string_pretty(&value).unwrap_or(doc)
}

/// Emit `value` as the run's answer INSTEAD of anything recorded (CLI-181:
/// on failure the error envelope is the one document, even if a verb
/// already recorded a result before failing).
pub fn emit_instead<T: serde::Serialize>(value: &T) {
    let _ = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Ok(s) = serde_json::to_string_pretty(value) {
        write_out(&s);
    }
}

/// Record `value` as the CLI-181 error envelope's `details` member for
/// this run's eventual failure (CLI-221). Call before returning the
/// error; a run that never fails, or fails without recording details,
/// leaves this `None` and the envelope carries no `details` member at all.
pub fn record_error_details<T: serde::Serialize>(value: &T) {
    if let Ok(v) = serde_json::to_value(value) {
        *ERROR_DETAILS.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
    }
}

/// Build the CLI-181 error envelope for `kind`/`message`, folding in
/// whatever was recorded via [`record_error_details`] as an optional
/// `details` member (CLI-221). Takes (clears) the recorded details, so a
/// second call in the same run starts clean.
pub fn build_error_envelope(kind: &str, message: &str) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "schema": 1,
        "error": {
            "kind": kind,
            "message": message,
        }
    });
    if let Some(details) = ERROR_DETAILS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        envelope["details"] = details;
    }
    // spec: HOOK-161 -- a failed run reports the hooks it considered too,
    // the failing one last.
    let records = crate::hook_log::records();
    if !records.is_empty()
        && let Ok(hooks) = serde_json::to_value(&records)
    {
        envelope["hooks"] = hooks;
    }
    envelope
}

#[cfg(test)]
mod tests {
    use crate::json_stdout;

    /// The CLI-221 `details` slot is process-global state, so its whole risk
    /// surface is "what is in there when nobody put anything in there". All of
    /// it is asserted in ONE test on purpose: `build_error_envelope` TAKES the
    /// recorded value, so two tests touching the static would race under the
    /// default parallel test runner and pass or fail by scheduling.
    ///
    /// Pins, in order: an envelope built with nothing recorded has no `details`
    /// KEY at all (not `null`, not `{}`); a recorded value lands verbatim; the
    /// take is real, so a second failure in the same run cannot inherit the
    /// first one's findings; and a second record before the take overwrites
    /// rather than accumulating.
    // spec: CLI-181 CLI-182 CLI-221
    #[test]
    fn cli_221_error_details_is_absent_unless_recorded_and_never_leaks() {
        // Nothing recorded -> no `details` member whatsoever.
        let plain = json_stdout::build_error_envelope("not-installed", "item not installed");
        assert_eq!(
            plain,
            serde_json::json!({
                "schema": 1,
                "error": {"kind": "not-installed", "message": "item not installed"}
            }),
            "an error with no recorded findings must carry no `details` member"
        );
        assert!(
            !plain.as_object().is_some_and(|o| o.contains_key("details")),
            "`details` must be ABSENT, not null: {plain}"
        );

        // Recorded -> folded in verbatim.
        json_stdout::record_error_details(&serde_json::json!({
            "action": "review", "outcome": "failed", "hard": [{"kind": "toml-parse-error"}]
        }));
        let with = json_stdout::build_error_envelope("review-failed", "1 hard finding");
        assert_eq!(with["error"]["kind"], "review-failed", "{with}");
        assert_eq!(with["details"]["outcome"], "failed", "{with}");
        assert_eq!(
            with["details"]["hard"][0]["kind"], "toml-parse-error",
            "{with}"
        );

        // ...and TAKEN: an unrelated later error cannot inherit them.
        let after = json_stdout::build_error_envelope("source-not-found", "no such source");
        assert!(
            !after.as_object().is_some_and(|o| o.contains_key("details")),
            "recorded details must not leak into the next envelope: {after}"
        );

        // Recording twice before the take replaces rather than accumulating,
        // so the caller sees the LAST verb's findings, matching `record`'s
        // outermost-frame-wins rule (CLI-153).
        json_stdout::record_error_details(&serde_json::json!({"which": "first"}));
        json_stdout::record_error_details(&serde_json::json!({"which": "second"}));
        let twice = json_stdout::build_error_envelope("review-failed", "x");
        assert_eq!(
            twice["details"],
            serde_json::json!({"which": "second"}),
            "{twice}"
        );

        // Leave the static clean for anything else in this binary.
        let _ = json_stdout::build_error_envelope("x", "y");
    }

    // ==========================================================================
    // CLI-217: `json_stdout::emit_instead` discards a prior recording
    // ==========================================================================

    /// Turns a re-exec of this test binary into a one-shot driver of the
    /// `record` -> `emit_instead` sequence: the "verb recorded a result, then
    /// failed" corridor CLI-181 relies on (the error envelope REPLACES
    /// whatever a verb recorded before failing, CLI-217 point 2). No verb in
    /// the current call graph reaches this in practice -- every `print_json`
    /// call site in `commands/` is the last statement before its function
    /// returns, so nothing can record and then still error out of the SAME
    /// call -- which is exactly why this is a direct unit test of the
    /// mechanism rather than an end-to-end one: `emit_instead`'s own contract
    /// (discard the pending recording, write only the replacement) needs to
    /// hold regardless of whether today's call graph exercises it, since a
    /// future call site (or a refactor that moves a `print_json` earlier) could
    /// start relying on it.
    const EMIT_INSTEAD_TEST: &str = "json_stdout::tests::emit_instead_discards_a_prior_recording";
    const EMIT_INSTEAD_MODE_ENV: &str = "MIND_TEST_EMIT_INSTEAD_MODE";

    /// `json_stdout::reserve` does a real `dup2` of this process's fd 1, which
    /// would corrupt libtest's own output capturing for every OTHER test
    /// running concurrently in this binary if called in-process. Re-executing
    /// the test binary filtered to just this test isolates the fd mutation to
    /// a throwaway child, the same technique `render::tests::note_and_warn_route_by_output_mode`
    /// uses for the same reason.
    #[test]
    fn emit_instead_discards_a_prior_recording() {
        if std::env::var_os(EMIT_INSTEAD_MODE_ENV).is_some() {
            // Child role: reserve stdout for real, record a first document,
            // then let `emit_instead` replace it, exactly as main()'s error
            // path does on a verb that recorded before failing.
            json_stdout::reserve();
            json_stdout::record(
                serde_json::json!({"schema": 1, "action": "first-recorded"}).to_string(),
            );
            json_stdout::emit_instead(&serde_json::json!({
                "error": {"kind": "replacement", "message": "the one document"}
            }));
            // A THIRD write attempt (flush_pending, as main() calls on the
            // success path) must be a no-op: emit_instead already took the
            // pending slot, so there is nothing left to flush. If this wrote
            // again, the parent would see a second JSON value tacked onto
            // stdout, which does not parse as one document.
            json_stdout::flush_pending();
            std::process::exit(0);
        }

        let exe = std::env::current_exe().expect("path of the running test binary");
        let out = std::process::Command::new(exe)
            .args([
                "--exact",
                EMIT_INSTEAD_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(EMIT_INSTEAD_MODE_ENV, "1")
            .output()
            .expect("re-exec the test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // `--nocapture` means libtest's own "running 1 test" / "test ... "
        // preamble shares this stdout ahead of our real output (it is written
        // by libtest itself, before the test body runs `reserve()`), so this
        // checks for the replacement/discard by substring rather than parsing
        // the whole capture as one JSON value.
        assert!(
            stdout.contains("\"replacement\""),
            "emit_instead's value must reach the preserved stdout: {stdout:?}"
        );
        assert!(
            !stdout.contains("first-recorded"),
            "the document recorded before the failure must be discarded, not \
             concatenated ahead of the replacement: {stdout:?}"
        );
        // The trailing `flush_pending()` call in the child must not add a
        // second occurrence of the replacement text (it would if PENDING were
        // not actually cleared by `emit_instead`, so `flush_pending` wrote it
        // again on top).
        assert_eq!(
            stdout.matches("\"replacement\"").count(),
            1,
            "the replacement document must be written exactly once, not once \
             by emit_instead and again by a later flush_pending: {stdout:?}"
        );
    }
}
