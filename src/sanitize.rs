//! ANSI-escape sanitization shared between the CLI and TUI layers.
//!
//! The CLI applies `strip_ansi` at every catalog-string display site
//! (commands.rs, MKT-9 / DSC-69). This module exposes the same logic as
//! `pub(crate)` so the TUI data layer can sanitize at its model boundary
//! (TUI-60) without depending on commands.rs.

/// Strip ANSI escape sequences, C0/DEL/C1 control characters, and Unicode
/// bidi-override/separator code points from `s`.
///
/// Printable non-ASCII (U+00A0 and above, minus the blocked ranges) is
/// preserved so non-English curator messages are not corrupted. The logic
/// mirrors the private `strip_ansi` in commands.rs; both implement the same
/// sanitization rule (DSC-69, MKT-9).
pub(crate) fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    // Input is valid UTF-8, so output is too; lossy conversion is a no-op in practice.
    String::from_utf8_lossy(&bytes)
        .chars()
        .filter(|&c| {
            (('\x20'..='\x7e').contains(&c) || c > '\u{009f}')
                && !matches!(
                    c,
                    // Bidi-override code points: phishing/spoofing vectors.
                    '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                    // Line separator and paragraph separator.
                    | '\u{2028}' | '\u{2029}'
                )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: TUI-60
    #[test]
    fn strip_ansi_removes_escape_sequences() {
        // An ANSI color sequence must be stripped entirely.
        assert_eq!(strip_ansi("\x1b[31mhello\x1b[0m"), "hello");
        // Nested/compound sequences are stripped too.
        assert_eq!(strip_ansi("\x1b[1;32mgreen bold\x1b[0m"), "green bold");
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_removes_bidi_overrides() {
        // Bidi-override U+202E is a phishing/spoofing vector and must be removed.
        assert_eq!(strip_ansi("pay \u{202E}oot"), "pay oot");
        // Every bidi range is stripped.
        assert_eq!(strip_ansi("\u{202A}\u{202B}\u{202C}\u{202D}\u{202E}"), "");
        assert_eq!(strip_ansi("\u{2066}\u{2067}\u{2068}\u{2069}"), "");
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_removes_line_and_para_separators() {
        assert_eq!(strip_ansi("line\u{2028}break"), "linebreak");
        assert_eq!(strip_ansi("para\u{2029}sep"), "parasep");
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_preserves_printable_ascii() {
        let s = "hello world 123!";
        assert_eq!(strip_ansi(s), s);
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_preserves_printable_unicode() {
        // Accented and non-Latin characters are preserved.
        assert_eq!(strip_ansi("hello\u{00e9}"), "hello\u{00e9}");
        assert_eq!(strip_ansi("caf\u{00e9}"), "caf\u{00e9}");
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_removes_c0_controls() {
        // C0 control characters (below 0x20) are stripped.
        assert_eq!(strip_ansi("a\x00b"), "ab");
        assert_eq!(strip_ansi("a\x01b"), "ab");
        assert_eq!(strip_ansi("a\x1fb"), "ab");
        // Space (0x20) is preserved.
        assert_eq!(strip_ansi("a b"), "a b");
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }
}
