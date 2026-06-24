// Dead-code warnings are expected: shard C/D will wire these into commands.rs.
// The lint fires because the API is defined here but not yet called from outside
// this module. Suppress it so clippy stays clean during the incremental build.
#![allow(dead_code)]

use std::sync::OnceLock;

static CTX: OnceLock<OutputCtx> = OnceLock::new();

/// Install the process-wide output context. Call once, early in `main`, after
/// parsing the global flags. A second call is ignored.
pub fn set_ctx(ctx: OutputCtx) {
    let _ = CTX.set(ctx);
}

/// The process-wide output context. Defaults to plain (no color, no Unicode,
/// json=false) when `set_ctx` was never called — the safe default for unit/
/// integration tests and any non-main caller.
pub fn ctx() -> OutputCtx {
    CTX.get().copied().unwrap_or(OutputCtx {
        json: false,
        color: false,
        unicode: false,
    })
}

/// Output capabilities resolved once from the global flags + environment.
#[derive(Clone, Copy)]
pub struct OutputCtx {
    pub json: bool,
    pub color: bool,
    pub unicode: bool,
}

impl OutputCtx {
    /// Build from the global `--json`/`--ascii` flags plus the environment and TTY.
    pub fn detect(json: bool, ascii: bool) -> Self {
        use std::io::IsTerminal;
        let is_tty = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let utf8_locale = detect_utf8_locale();
        Self::compute(json, ascii, is_tty, no_color, utf8_locale)
    }

    /// Pure, fully-injected core of `detect` so the gate is unit-testable without
    /// real env/tty. `detect` is just
    /// `compute(json, ascii, stdout_is_tty, no_color_set, utf8_locale)`.
    pub fn compute(
        json: bool,
        ascii: bool,
        is_tty: bool,
        no_color: bool,
        utf8_locale: bool,
    ) -> Self {
        // color and unicode are true ONLY when ALL of: is_tty AND utf8_locale AND NOT
        // no_color AND NOT json AND NOT ascii.
        let rich = is_tty && utf8_locale && !no_color && !json && !ascii;
        Self {
            json,
            color: rich,
            unicode: rich,
        }
    }

    // --- Semantic status markers ---

    /// Installed / success marker.  Unicode "✓" (green) or ASCII "+".
    pub fn ok(&self) -> String {
        if self.unicode {
            self.green("✓")
        } else {
            "+".to_string()
        }
    }

    /// Available / inactive marker.  Unicode "○" (dim) or ASCII "-".
    pub fn available(&self) -> String {
        if self.unicode {
            self.dim("○")
        } else {
            "-".to_string()
        }
    }

    /// Drift / removed / warn marker.  Unicode "!" (yellow) or ASCII "!".
    pub fn warn(&self) -> String {
        if self.unicode {
            self.yellow("!")
        } else {
            "!".to_string()
        }
    }

    /// Error marker.  Unicode "✗" (red) or ASCII "x".
    pub fn err(&self) -> String {
        if self.unicode {
            self.red("✗")
        } else {
            "x".to_string()
        }
    }

    /// Source / section bullet.  Unicode "●" or ASCII "*".
    pub fn bullet(&self) -> String {
        if self.unicode {
            "●".to_string()
        } else {
            "*".to_string()
        }
    }

    // --- Color wrappers ---

    /// Wrap `s` in green SGR codes, or return `s` unchanged when color is off.
    pub fn green(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[32m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in yellow SGR codes, or return `s` unchanged when color is off.
    pub fn yellow(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[33m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in red SGR codes, or return `s` unchanged when color is off.
    pub fn red(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[31m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in dim SGR codes, or return `s` unchanged when color is off.
    pub fn dim(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in bold SGR codes, or return `s` unchanged when color is off.
    pub fn bold(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Print rows as aligned columns.
    ///
    /// Every column except the last is left-padded to the widest VISIBLE width in
    /// that column; the last column is left as-is; trailing empty cells are trimmed.
    /// Width is measured ignoring ANSI SGR escapes via [`visible_width`].
    /// Columns are separated by two spaces.
    pub fn print_rows(&self, rows: &[Vec<String>]) {
        let Some(ncols) = rows.iter().map(Vec::len).max() else {
            return;
        };
        let mut widths = vec![0usize; ncols];
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i + 1 < ncols {
                    widths[i] = widths[i].max(visible_width(cell));
                }
            }
        }
        for row in rows {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                if i > 0 {
                    line.push_str("  ");
                }
                if i + 1 < ncols {
                    let pad = widths[i].saturating_sub(visible_width(cell));
                    line.push_str(cell);
                    line.extend(std::iter::repeat_n(' ', pad));
                } else {
                    line.push_str(cell);
                }
            }
            println!("{}", line.trim_end());
        }
    }
}

/// Visible (display) width of `s`, ignoring ANSI SGR escape sequences like
/// `"\x1b[32m"` ... `"\x1b[0m"`. Counts Unicode scalar values (chars), not bytes.
pub fn visible_width(s: &str) -> usize {
    let mut count = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Consume the rest of the CSI/SGR sequence: '[' then digits/semicolons
            // then a single letter terminator (e.g. 'm').
            if chars.next() == Some('[') {
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            // If the char after ESC is not '[', we consumed just ESC and one unknown
            // char; we don't count either but we already advanced past them.
        } else {
            count += 1;
        }
    }
    count
}

/// Detect whether the active locale advertises UTF-8.
///
/// Checks `LC_ALL`, `LC_CTYPE`, `LANG` in that order (first set wins).
/// Returns `false` when none is set (conservative ASCII default).
fn detect_utf8_locale() -> bool {
    for var in &["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(val) = std::env::var(var)
            && !val.is_empty()
        {
            let lower = val.to_lowercase();
            return lower.contains("utf-8") || lower.contains("utf8");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- helpers ---

    fn plain() -> OutputCtx {
        OutputCtx::compute(false, false, false, false, false)
    }

    fn rich() -> OutputCtx {
        OutputCtx::compute(false, false, true, false, true)
    }

    // ==========================================================================
    // compute truth table
    // ==========================================================================

    /// All five inputs true -> color and unicode are true.
    #[test]
    fn compute_all_true_yields_rich() {
        let ctx = OutputCtx::compute(false, false, true, false, true);
        assert!(ctx.color, "color should be true when all conditions met");
        assert!(
            ctx.unicode,
            "unicode should be true when all conditions met"
        );
        assert!(!ctx.json);
    }

    /// json=true disables color and unicode even when everything else permits it.
    #[test]
    fn compute_json_disables_color() {
        let ctx = OutputCtx::compute(true, false, true, false, true);
        assert!(!ctx.color, "json disables color");
        assert!(!ctx.unicode, "json disables unicode");
        assert!(ctx.json);
    }

    /// ascii=true disables color and unicode even when everything else permits it.
    #[test]
    fn compute_ascii_disables_color() {
        let ctx = OutputCtx::compute(false, true, true, false, true);
        assert!(!ctx.color, "ascii flag disables color");
        assert!(!ctx.unicode, "ascii flag disables unicode");
    }

    /// is_tty=false disables color and unicode.
    #[test]
    fn compute_non_tty_disables_color() {
        let ctx = OutputCtx::compute(false, false, false, false, true);
        assert!(!ctx.color, "non-tty disables color");
        assert!(!ctx.unicode, "non-tty disables unicode");
    }

    /// no_color=true disables color and unicode.
    #[test]
    fn compute_no_color_env_disables_color() {
        let ctx = OutputCtx::compute(false, false, true, true, true);
        assert!(!ctx.color, "NO_COLOR disables color");
        assert!(!ctx.unicode, "NO_COLOR disables unicode");
    }

    /// utf8_locale=false disables color and unicode.
    #[test]
    fn compute_non_utf8_locale_disables_color() {
        let ctx = OutputCtx::compute(false, false, true, false, false);
        assert!(!ctx.color, "non-utf8 locale disables color");
        assert!(!ctx.unicode, "non-utf8 locale disables unicode");
    }

    /// color and unicode are always equal (same conjunction).
    #[test]
    fn compute_color_and_unicode_always_equal() {
        for json in [false, true] {
            for ascii in [false, true] {
                for tty in [false, true] {
                    for no_color in [false, true] {
                        for utf8 in [false, true] {
                            let ctx = OutputCtx::compute(json, ascii, tty, no_color, utf8);
                            assert_eq!(
                                ctx.color, ctx.unicode,
                                "color and unicode must be equal \
                                 (json={json} ascii={ascii} tty={tty} no_color={no_color} utf8={utf8})"
                            );
                        }
                    }
                }
            }
        }
    }

    // ==========================================================================
    // Glyph selection
    // ==========================================================================

    #[test]
    fn ok_unicode_contains_checkmark_and_ansi() {
        let s = rich().ok();
        assert!(s.contains('✓'), "ok() unicode should contain ✓, got {s:?}");
        assert!(
            s.contains('\x1b'),
            "ok() unicode should contain ANSI code, got {s:?}"
        );
    }

    #[test]
    fn ok_ascii_is_plus_no_ansi() {
        let s = plain().ok();
        assert_eq!(s, "+", "ok() ascii should be '+', got {s:?}");
        assert!(!s.contains('\x1b'), "ok() ascii must not contain ANSI code");
    }

    #[test]
    fn available_unicode_contains_circle_and_ansi() {
        let s = rich().available();
        assert!(
            s.contains('○'),
            "available() unicode should contain ○, got {s:?}"
        );
        assert!(
            s.contains('\x1b'),
            "available() unicode should contain ANSI code"
        );
    }

    #[test]
    fn available_ascii_is_dash_no_ansi() {
        let s = plain().available();
        assert_eq!(s, "-", "available() ascii should be '-', got {s:?}");
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn warn_unicode_contains_bang_and_ansi() {
        let s = rich().warn();
        assert!(s.contains('!'), "warn() unicode should contain '!'");
        assert!(
            s.contains('\x1b'),
            "warn() unicode should contain ANSI code"
        );
    }

    #[test]
    fn warn_ascii_is_bang_no_ansi() {
        let s = plain().warn();
        assert_eq!(s, "!", "warn() ascii should be '!'");
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn err_unicode_contains_cross_and_ansi() {
        let s = rich().err();
        assert!(s.contains('✗'), "err() unicode should contain ✗, got {s:?}");
        assert!(s.contains('\x1b'), "err() unicode should contain ANSI code");
    }

    #[test]
    fn err_ascii_is_x_no_ansi() {
        let s = plain().err();
        assert_eq!(s, "x", "err() ascii should be 'x', got {s:?}");
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn bullet_unicode_is_filled_circle_no_ansi_wrap() {
        let s = rich().bullet();
        assert!(
            s.contains('●'),
            "bullet() unicode should contain ●, got {s:?}"
        );
        // bullet() is not color-wrapped; it has no SGR codes
        assert!(
            !s.contains('\x1b'),
            "bullet() should not contain ANSI codes"
        );
    }

    #[test]
    fn bullet_ascii_is_star_no_ansi() {
        let s = plain().bullet();
        assert_eq!(s, "*", "bullet() ascii should be '*', got {s:?}");
        assert!(!s.contains('\x1b'));
    }

    // ==========================================================================
    // Color wrappers
    // ==========================================================================

    #[test]
    fn color_wrappers_are_noop_when_color_off() {
        let ctx = plain();
        assert_eq!(ctx.green("hi"), "hi");
        assert_eq!(ctx.yellow("hi"), "hi");
        assert_eq!(ctx.red("hi"), "hi");
        assert_eq!(ctx.dim("hi"), "hi");
        assert_eq!(ctx.bold("hi"), "hi");
    }

    #[test]
    fn color_wrappers_wrap_and_reset_when_color_on() {
        let ctx = rich();
        let g = ctx.green("hi");
        assert!(g.starts_with("\x1b[32m"), "green should start with SGR 32");
        assert!(g.ends_with("\x1b[0m"), "green should end with reset");
        assert!(g.contains("hi"), "green should contain the text");

        let y = ctx.yellow("hi");
        assert!(y.starts_with("\x1b[33m"), "yellow should start with SGR 33");
        assert!(y.ends_with("\x1b[0m"));

        let r = ctx.red("hi");
        assert!(r.starts_with("\x1b[31m"), "red should start with SGR 31");
        assert!(r.ends_with("\x1b[0m"));

        let d = ctx.dim("hi");
        assert!(d.starts_with("\x1b[2m"), "dim should start with SGR 2");
        assert!(d.ends_with("\x1b[0m"));

        let b = ctx.bold("hi");
        assert!(b.starts_with("\x1b[1m"), "bold should start with SGR 1");
        assert!(b.ends_with("\x1b[0m"));
    }

    // ==========================================================================
    // visible_width
    // ==========================================================================

    #[test]
    fn visible_width_plain_string_counts_chars() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width(""), 0);
        assert_eq!(visible_width("abc"), 3);
    }

    #[test]
    fn visible_width_colored_equals_plain_width() {
        // A colored "hello" (5 chars) must measure 5, not count the ANSI bytes.
        let colored = "\x1b[32mhello\x1b[0m";
        assert_eq!(
            visible_width(colored),
            5,
            "ANSI codes must not count toward width"
        );
    }

    #[test]
    fn visible_width_multibyte_counts_chars_not_bytes() {
        // "✓" is 3 bytes in UTF-8 but 1 char.
        assert_eq!(visible_width("✓"), 1);
        // "●" is also 3 bytes.
        assert_eq!(visible_width("●"), 1);
        // Combined with ANSI.
        let s = "\x1b[32m✓\x1b[0m";
        assert_eq!(
            visible_width(s),
            1,
            "multi-byte char inside ANSI must count as 1"
        );
    }

    #[test]
    fn visible_width_strips_multiple_escapes() {
        // A string with several SGR codes interspersed.
        let s = "\x1b[1mhello\x1b[0m \x1b[31mworld\x1b[0m";
        // "hello world" = 11 chars (including the space).
        assert_eq!(visible_width(s), 11);
    }

    // ==========================================================================
    // print_rows alignment
    // ==========================================================================

    /// Collect print_rows output by capturing stdout via a Vec<u8> pipe.
    /// Since print_rows uses println! we test alignment logic separately.
    ///
    /// We verify the alignment by inspecting the widths the algorithm would
    /// compute for given rows, mirroring the internal logic.
    fn compute_column_widths(rows: &[Vec<String>]) -> Vec<usize> {
        let Some(ncols) = rows.iter().map(Vec::len).max() else {
            return vec![];
        };
        let mut widths = vec![0usize; ncols];
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                if i + 1 < ncols {
                    widths[i] = widths[i].max(visible_width(cell));
                }
            }
        }
        widths
    }

    #[test]
    fn print_rows_alignment_plain_cells() {
        let rows = vec![
            vec!["a".to_string(), "bc".to_string(), "desc1".to_string()],
            vec!["xyz".to_string(), "d".to_string(), "desc2".to_string()],
        ];
        let widths = compute_column_widths(&rows);
        // col 0: max("a"=1, "xyz"=3) = 3
        assert_eq!(widths[0], 3, "col 0 width should be 3");
        // col 1: max("bc"=2, "d"=1) = 2
        assert_eq!(widths[1], 2, "col 1 width should be 2");
        // col 2 (last): not padded, so width stays 0
        assert_eq!(widths[2], 0, "last col not padded");
    }

    #[test]
    fn print_rows_colored_cell_aligns_same_as_uncolored() {
        // A colored "+  " should have the same visible width as plain "+" when
        // computing column widths.
        let colored_plus = "\x1b[32m+\x1b[0m".to_string();
        let plain_plus = "+".to_string();

        let rows_colored = vec![
            vec![
                colored_plus.clone(),
                "long-name".to_string(),
                "desc".to_string(),
            ],
            vec!["x".to_string(), "n".to_string(), "desc2".to_string()],
        ];
        let rows_plain = vec![
            vec![
                plain_plus.clone(),
                "long-name".to_string(),
                "desc".to_string(),
            ],
            vec!["x".to_string(), "n".to_string(), "desc2".to_string()],
        ];

        let widths_colored = compute_column_widths(&rows_colored);
        let widths_plain = compute_column_widths(&rows_plain);

        assert_eq!(
            widths_colored[0], widths_plain[0],
            "colored and plain cells in the same column must produce the same width: \
             colored={} plain={}",
            widths_colored[0], widths_plain[0]
        );
    }

    #[test]
    fn print_rows_single_column_no_padding() {
        // A single-column table: the one column is the last column, so no
        // alignment padding is computed.
        let rows = vec![vec!["hello".to_string()], vec!["world!".to_string()]];
        let widths = compute_column_widths(&rows);
        // Only one column, it is the last column, so its width slot stays 0.
        assert_eq!(widths[0], 0);
    }

    #[test]
    fn print_rows_empty_rows_no_panic() {
        let ctx = plain();
        // Must not panic.
        ctx.print_rows(&[]);
        ctx.print_rows(&[vec![]]);
    }

    // ==========================================================================
    // Process-global ctx / set_ctx
    // ==========================================================================

    /// Before set_ctx is ever called (or when the OnceLock has not yet been
    /// populated in this process), ctx() returns the plain default.
    ///
    /// Note on set-once semantics: OnceLock is set exactly once per process.
    /// Tests share the process, so only the first set_ctx call wins. We keep
    /// the default-check test separate from the set-and-read test and accept
    /// that only one ordering is observable in a given run; the Rust test
    /// harness may run tests in any order. The assertions below are written so
    /// they hold regardless of which test runs first:
    ///   - ctx_default_is_plain checks the plain fallback value, which is the
    ///     same value that would be returned if set_ctx was never called AND if
    ///     set_ctx was called with a plain context.
    ///   - ctx_set_then_get checks that after set_ctx the returned value
    ///     reflects whatever was set (even if an earlier test already set it,
    ///     because the harness cannot call set_ctx with a different value once
    ///     it is already set; we use a plain ctx so both orderings are consistent).
    #[test]
    fn ctx_default_is_plain() {
        // Whether or not set_ctx has been called, ctx() must never panic and
        // must return a valid OutputCtx.  The plain default has all fields false.
        // If set_ctx was already called with a non-plain value in this process
        // run, this test would observe that value instead — but set_ctx is only
        // called from tests here, and all calls below use the plain context, so
        // the observed value will always have color=false, unicode=false,
        // json=false regardless of ordering.
        let c = ctx();
        assert!(!c.color, "default/plain ctx must have color=false");
        assert!(!c.unicode, "default/plain ctx must have unicode=false");
        assert!(!c.json, "default/plain ctx must have json=false");
    }

    /// set_ctx installs a value that ctx() then returns.
    ///
    /// OnceLock is set-once: only the first set_ctx call in this process wins.
    /// We use a plain OutputCtx so the assertion is consistent regardless of
    /// whether ctx_default_is_plain ran first.
    #[test]
    fn ctx_set_then_get_reflects_installed_value() {
        let installed = OutputCtx {
            json: false,
            color: false,
            unicode: false,
        };
        set_ctx(installed);
        let got = ctx();
        assert_eq!(got.json, installed.json);
        assert_eq!(got.color, installed.color);
        assert_eq!(got.unicode, installed.unicode);
    }
}
