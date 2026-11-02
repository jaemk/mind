//! Fixed-choice terminal prompts (CLI-257, CLI-258, CLI-259).
//!
//! Every `[y/N]`-style question `mind` asks goes through [`ask`]: it accepts
//! only the listed replies and a bare Enter (the default), rejects anything
//! else and asks again, and takes EOF as the default. Free-text prompts (the
//! namespace prefix, a collision rename, the profile picker) do not use it.

use std::cell::Cell;
use std::io::{BufRead, Write};

use crate::error::{MindError, Result};

/// One accepted reply: the spellings that select it (lowercase) and its value.
pub struct Choice<T> {
    pub keys: &'static [&'static str],
    pub value: T,
}

/// The rejection line for an unrecognized reply: names each choice by its
/// first spelling, e.g. `please answer y, n, or a (Enter for the default)`.
fn rejection<T>(choices: &[Choice<T>]) -> String {
    let names: Vec<&str> = choices
        .iter()
        .filter_map(|c| c.keys.first().copied())
        .collect();
    let list = match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    };
    format!("please answer {list} (Enter for the default)")
}

/// Ask `prompt` on `output` and read replies from `input` until one is valid
/// (CLI-257). A bare Enter, and EOF, take `default` (CLI-258). Replies are
/// trimmed and matched case-insensitively.
pub fn ask_from<T: Copy, R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    prompt: &str,
    choices: &[Choice<T>],
    default: T,
) -> Result<T> {
    loop {
        write!(output, "{prompt}").map_err(|e| MindError::io("<stdout>", e))?;
        output.flush().map_err(|e| MindError::io("<stdout>", e))?;
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|e| MindError::io("<stdin>", e))?;
        if read == 0 {
            // spec: CLI-258 -- EOF is the default, exactly as a bare Enter.
            writeln!(output).map_err(|e| MindError::io("<stdout>", e))?;
            return Ok(default);
        }
        let reply = line.trim().to_ascii_lowercase();
        if reply.is_empty() {
            return Ok(default);
        }
        if let Some(c) = choices.iter().find(|c| c.keys.contains(&reply.as_str())) {
            return Ok(c.value);
        }
        // spec: CLI-257 -- an unclear reply neither proceeds nor declines.
        writeln!(output, "{}", rejection(choices)).map_err(|e| MindError::io("<stdout>", e))?;
    }
}

thread_local! {
    /// Set while the probe TUI runs a verb with its stdout captured (TUI-24):
    /// the terminal is in raw mode behind the browser, so a prompt would be
    /// invisible and its line read would never end.
    static CAPTURED: Cell<bool> = const { Cell::new(false) };
    /// Set when a prompt was refused under `CAPTURED`, so the TUI knows to run
    /// the verb again on the normal terminal (TUI-44).
    static REFUSED: Cell<bool> = const { Cell::new(false) };
}

/// Mark the verbs this thread runs from now on as captured (`true`) or not.
/// Entering capture clears [`refused`].
pub fn set_captured(on: bool) {
    if on {
        REFUSED.with(|r| r.set(false));
    }
    CAPTURED.with(|c| c.set(on));
}

/// Whether a prompt was refused since capture began (TUI-44).
pub fn refused() -> bool {
    REFUSED.with(Cell::get)
}

/// Refuse to prompt while captured (TUI-44): every terminal prompt calls this
/// before writing its question, and the TUI reruns the verb unsuspended.
pub fn guard(action: &str) -> Result<()> {
    if CAPTURED.with(Cell::get) {
        REFUSED.with(|r| r.set(true));
        return Err(MindError::ConfirmationRequired {
            action: action.to_string(),
        });
    }
    Ok(())
}

/// [`ask_from`] on the terminal, with the progress spinner cleared first
/// (PRG-5). Without a TTY no prompt is shown (CLI-259): the run refuses with
/// `ConfirmationRequired` naming `action`, so a piped command never applies
/// anything nobody was shown. Callers whose verb defines a non-TTY fallback
/// (HOOK-22, CLI-23, CUR-9) take it before reaching here.
pub fn ask<T: Copy>(prompt: &str, choices: &[Choice<T>], default: T, action: &str) -> Result<T> {
    guard(action)?;
    if !crate::hook::is_tty() {
        return Err(MindError::ConfirmationRequired {
            action: action.to_string(),
        });
    }
    // spec: PRG-5
    crate::progress::suspend(|| {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut output = std::io::stdout();
        ask_from(&mut input, &mut output, prompt, choices, default)
    })
}

/// The `y`/`yes` and `n`/`no` replies of a yes/no confirmation.
pub const YES_NO: [Choice<bool>; 2] = [
    Choice {
        keys: &["y", "yes"],
        value: true,
    },
    Choice {
        keys: &["n", "no"],
        value: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str, default: bool) -> (bool, String) {
        let mut reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let v = ask_from(&mut reader, &mut out, "go? [Y/n] ", &YES_NO, default).unwrap();
        (v, String::from_utf8(out).unwrap())
    }

    #[test]
    fn listed_replies_are_accepted_in_any_case() {
        // spec: CLI-257
        assert!(run("y\n", false).0);
        assert!(run(" YES \n", false).0);
        assert!(!run("n\n", true).0);
        assert!(!run("No\n", true).0);
    }

    #[test]
    fn a_bare_enter_takes_the_default() {
        // spec: CLI-257
        assert!(run("\n", true).0);
        assert!(!run("\n", false).0);
    }

    #[test]
    fn an_unrecognized_reply_is_rejected_and_asked_again() {
        // spec: CLI-257 -- `maybe` used to confirm a `[Y/n]` prompt.
        let (v, out) = run("maybe\nn\n", true);
        assert!(!v, "the second, valid reply decides");
        assert_eq!(out.matches("go? [Y/n] ").count(), 2, "{out}");
        assert!(out.contains("please answer y or n"), "{out}");
    }

    #[test]
    fn eof_takes_the_default() {
        // spec: CLI-258
        assert!(run("", true).0);
        assert!(!run("", false).0);
        // EOF after a rejected reply still ends the loop on the default.
        assert!(run("huh\n", true).0);
    }

    #[test]
    fn a_captured_verb_refuses_to_prompt_and_says_so() {
        // spec: TUI-44 -- under the TUI's stdout capture a prompt would be
        // invisible and block on a raw-mode read; it refuses instead, and
        // `refused` tells the TUI to rerun the verb on the normal terminal.
        set_captured(true);
        assert!(!refused(), "entering capture starts clean");
        assert!(guard("x").is_err());
        assert!(refused());
        let err = ask("go? [Y/n] ", &YES_NO, true, "running a hook").unwrap_err();
        assert!(
            matches!(err, MindError::ConfirmationRequired { ref action } if action == "running a hook"),
            "{err}"
        );
        set_captured(false);
        assert!(guard("x").is_ok(), "outside capture a prompt may ask");
        assert!(refused(), "the flag survives until the next capture");
        set_captured(true);
        assert!(!refused());
        set_captured(false);
    }

    #[test]
    fn the_rejection_names_every_choice() {
        let three = [
            Choice {
                keys: &["y"],
                value: 0,
            },
            Choice {
                keys: &["n"],
                value: 1,
            },
            Choice {
                keys: &["a"],
                value: 2,
            },
        ];
        assert_eq!(
            rejection(&three),
            "please answer y, n, or a (Enter for the default)"
        );
    }
}
