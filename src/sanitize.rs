//! ANSI-escape sanitization shared between the CLI and TUI layers.
//!
//! The CLI applies `strip_ansi` at every catalog-string display site
//! (commands.rs, MKT-9 / DSC-69). This module exposes the same logic as
//! `pub(crate)` so the TUI data layer can sanitize at its model boundary
//! (TUI-60) without depending on commands.rs.

/// Whether `c` is a C0, DEL, or C1 control character (the "collapse to a
/// space" bucket [`strip_ansi`] uses for a run of these -- see there).
fn is_control(c: char) -> bool {
    c < '\x20' || ('\x7f'..='\u{009f}').contains(&c)
}

/// Whether `c` is a security-blocked Unicode code point: a bidi-override,
/// directional-mark, or zero-width/invisible character (spoofing vectors), a
/// line/paragraph separator, or a member of the Unicode tag block or the
/// variation-selector block. Silently dropped by [`strip_ansi`], with no space
/// substituted -- unlike a control character, none of these represents a word
/// boundary, so inserting a space where one of them sat would add whitespace
/// that was never there.
///
/// This is the Unicode format category (Cf) -- code points that affect layout
/// or rendering but carry no visible glyph of their own -- plus two blocks that
/// are not Cf but are equally invisible: the tag block (U+E0000-U+E007F,
/// nominally deprecated but the standard "invisible ASCII smuggling" vector: a
/// payload hidden in tag characters renders as nothing to a human at a
/// terminal, yet is plain text to a parser or an AI agent reading the same
/// string) and the variation selectors (U+FE00-U+FE0F, U+E0100-U+E01EF: they
/// modify the glyph of the character before them and are themselves invisible,
/// so they can be smuggled between two characters to defeat a substring
/// comparison the same way a zero-width character can). Deliberately NOT
/// blocked: combining marks (U+0300 range) and printable non-ASCII generally --
/// both are visible, so stripping them would corrupt legitimate non-English
/// text (see [`strip_ansi`]'s doc comment) without closing an invisibility
/// vector.
///
/// M5 (NS-73) broadened this from the original bidi/zero-width/separator set;
/// every code point blocked before that change is still blocked (a superset),
/// so DSC-96-era callers that only exercise the older set keep working.
fn is_blocked_unicode(c: char) -> bool {
    matches!(
        c,
        // Bidi-override code points: phishing/spoofing vectors.
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        // Line separator and paragraph separator.
        | '\u{2028}' | '\u{2029}'
        // Directional marks (LRM, RLM, Arabic Letter Mark): weaker spoofing
        // vectors than the overrides above, but still non-printing marks that
        // can misrepresent direction-sensitive text; stripped as
        // defense-in-depth (the overrides are the primary threat and are
        // already blocked above).
        | '\u{200E}' | '\u{200F}' | '\u{061C}'
        // Zero-width characters: invisible, so a hostile source can use them
        // to defeat a visual/substring comparison of a sanitized string.
        | '\u{200B}' | '\u{2060}' | '\u{FEFF}'
        // Soft hyphen: renders as nothing unless a line break falls there.
        | '\u{00AD}'
        // Mongolian vowel separator: a zero-width format character.
        | '\u{180E}'
        // Invisible mathematical operators (function application, invisible
        // times/separator/plus): U+2060 (word joiner) is listed above; these
        // fill out the rest of the same Cf run.
        | '\u{2061}'..='\u{2064}'
        // Deprecated Cf format characters (inhibit/activate symmetric
        // swapping, national digit shapes, nominal digit shapes): invisible
        // layout-affecting code points, same rationale as the bidi marks.
        | '\u{206A}'..='\u{206F}'
        // Variation selectors: modify the glyph of the preceding character
        // and are themselves invisible, so one can be smuggled between two
        // characters to defeat a substring comparison.
        | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}'
        // Interlinear annotation characters: invisible markup, never meant to
        // reach a plain-text display.
        | '\u{FFF9}'..='\u{FFFB}'
        // Hangul fillers: assigned code points that render as blank.
        | '\u{115F}' | '\u{3164}'
        // The Unicode tag block: nominally deprecated, but the standard
        // "invisible ASCII smuggling" vector -- a payload hidden here renders
        // as nothing to a human at a terminal, yet is plain text to a parser
        // or an AI agent reading the same string. The most important addition
        // in this broadening (M5): this text is read by both.
        | '\u{E0000}'..='\u{E007F}'
    )
}

/// Strip ANSI escape sequences, C0/DEL/C1 control characters, and Unicode
/// bidi-override/separator/zero-width/invisible code points from `s`
/// (DSC-69, MKT-9; the invisible/format set is [`is_blocked_unicode`], NS-73).
///
/// Printable non-ASCII (U+00A0 and above, minus the blocked ranges) is
/// preserved so non-English curator messages are not corrupted.
///
/// The first pass, `strip_ansi_escapes::strip`, is itself a small terminal
/// emulator: it does not merely delete a recognized escape sequence, it
/// *executes* it against a virtual terminal state and forwards only what that
/// terminal would have printed. One consequence a caller composing a display
/// string must know about: a bare OSC introducer (`\x1b]`) with no terminator
/// is treated as "sequence still open" and consumes everything after it to the
/// end of input -- so a value ending in an unterminated OSC (e.g. an item named
/// `evil\x1b]`) silently swallows whatever a caller appends *after* it in the
/// same composed string, not just the item name itself. Sanitize each field
/// with this function BEFORE composing a line out of several fields, never
/// after: compose-then-sanitize lets one field's dangling escape eat its
/// neighbors.
///
/// A maximal run of consecutive control characters that reach this function's
/// own filter collapses to a single space rather than vanishing, so text
/// built by joining lines with one -- a multi-line hook command embedded in a
/// consent disclosure, say -- does not have two originally-separate lines
/// silently fuse into one word when the separator between them is stripped
/// (CLI-224). In practice the only control character that ever reaches this
/// filter is `\n`: the first pass, `strip_ansi_escapes::strip`, is itself a
/// small terminal emulator that forwards a C0 control byte to its output only
/// when it is `\n` (see its `Perform::execute`), so every other C0/DEL/C1
/// control is already gone, without a trace, before this function's loop
/// runs -- unaffected by the collapse behavior below, which only ever
/// operates on what actually arrives. A security-blocked Unicode code point
/// ([`is_blocked_unicode`]) is still dropped with no space substituted: none
/// of those represents a word boundary the way a control character does.
pub(crate) fn strip_ansi(s: &str) -> String {
    let bytes = strip_ansi_escapes::strip(s);
    // Input is valid UTF-8, so output is too; lossy conversion is a no-op in practice.
    let text = String::from_utf8_lossy(&bytes);
    let mut out = String::with_capacity(text.len());
    let mut in_control_run = false;
    for c in text.chars() {
        if is_blocked_unicode(c) {
            continue;
        }
        if is_control(c) {
            if !in_control_run {
                out.push(' ');
                in_control_run = true;
            }
            continue;
        }
        out.push(c);
        in_control_run = false;
    }
    out
}

/// Whether `s` contains any character [`strip_ansi`] would remove or collapse: a
/// C0/DEL/C1 control character (ESC, `\x1b`, is one, so any ANSI escape sequence
/// makes this true) or a security-blocked Unicode code point -- bidi override,
/// directional mark, zero-width/invisible format character, line/paragraph
/// separator, or a member of the Unicode tag block or the variation-selector
/// block ([`is_blocked_unicode`], broadened by NS-73).
///
/// Use this to *reject* a source-controlled value that will key a filesystem
/// path, a namespace prefix, or an item's identity (DSC-95, DSC-96, NS-72,
/// NS-73), where sanitizing in place would silently mutate identity -- e.g. two
/// item names that differ only by a tag-block code point would otherwise
/// render identically after display sanitization, making them indistinguishable
/// in `recall`/`probe` while remaining distinct on disk. To clean a value that
/// is only ever displayed, use [`strip_ansi`] instead.
pub(crate) fn has_blocked_chars(s: &str) -> bool {
    s.chars().any(|c| is_control(c) || is_blocked_unicode(c))
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
        // `strip_ansi_escapes::strip` (the first pass) is itself a terminal
        // emulator that only forwards a C0 control byte to the raw output when
        // it is `\n`; every other C0 control (NUL, a plain `\x01`, unit
        // separator `\x1f`, ...) is consumed there and never reaches this
        // module's own control-character filter at all, so it leaves no
        // space -- unaffected by the CLI-224 collapse-to-space fix below,
        // which only ever sees a control character that survived that first
        // pass (in practice, just `\n`).
        assert_eq!(strip_ansi("a\x00b"), "ab");
        assert_eq!(strip_ansi("a\x01b"), "ab");
        assert_eq!(strip_ansi("a\x1fb"), "ab");
        // Space (0x20) is preserved.
        assert_eq!(strip_ansi("a b"), "a b");
    }

    // spec: CLI-224
    #[test]
    fn strip_ansi_collapses_newline_runs_to_one_space_not_joining_words() {
        // The motivating case (security fix 3): a multi-line hook command
        // embedded in a disclosure message must not have its lines silently
        // fused into one word when the newline between them is stripped.
        assert_eq!(
            strip_ansi("make build\nmake install"),
            "make build make install"
        );
        // A run of several control characters (e.g. "\r\n") still collapses to
        // exactly one space, not one per character.
        assert_eq!(strip_ansi("line one\r\nline two"), "line one line two");
        assert_eq!(strip_ansi("a\n\n\nb"), "a b");
        // A leading or trailing run still produces a boundary space rather than
        // vanishing, since the rule is "a run of control bytes is one space",
        // not "unless it's at an edge".
        assert_eq!(strip_ansi("\ntail"), " tail");
        assert_eq!(strip_ansi("head\n"), "head ");
    }

    // spec: CLI-224
    #[test]
    fn strip_ansi_blocked_unicode_still_vanishes_without_a_space() {
        // A security-blocked Unicode code point (bidi override, directional
        // mark, zero-width, line/paragraph separator) is not a word boundary
        // the way a control character is, so it is still dropped outright, with
        // no space substituted -- this pins that the CLI-224 space-collapse
        // fix applies only to the control-character bucket.
        assert_eq!(strip_ansi("pay\u{202E}oot"), "payoot");
        assert_eq!(strip_ansi("wo\u{200B}rd"), "word");
        assert_eq!(strip_ansi("line\u{2028}break"), "linebreak");
    }

    // spec: CLI-224
    #[test]
    fn strip_ansi_removes_directional_marks_and_zero_width() {
        // Directional marks (LRM, RLM, ALM) and zero-width characters (ZWSP,
        // WORD JOINER, BOM) are residual spoofing vectors left after the strong
        // bidi overrides were already stripped; removed as defense-in-depth.
        assert_eq!(strip_ansi("a\u{200E}b"), "ab", "LRM");
        assert_eq!(strip_ansi("a\u{200F}b"), "ab", "RLM");
        assert_eq!(strip_ansi("a\u{061C}b"), "ab", "ALM");
        assert_eq!(strip_ansi("a\u{200B}b"), "ab", "zero-width space");
        assert_eq!(strip_ansi("a\u{2060}b"), "ab", "word joiner");
        assert_eq!(
            strip_ansi("a\u{FEFF}b"),
            "ab",
            "BOM / zero-width no-break space"
        );
    }

    // spec: NS-73
    #[test]
    fn strip_ansi_removes_m5_broadened_set() {
        // M5: the blocked set is broadened to cover the rest of the Unicode
        // format category (Cf) plus the tag block and the variation
        // selectors. Each of these was invisible before this change and is
        // now removed with no space substituted, same as the rest of
        // `is_blocked_unicode`.
        assert_eq!(strip_ansi("a\u{00AD}b"), "ab", "soft hyphen");
        assert_eq!(strip_ansi("a\u{180E}b"), "ab", "Mongolian vowel separator");
        assert_eq!(
            strip_ansi("a\u{2061}\u{2062}\u{2063}\u{2064}b"),
            "ab",
            "invisible math operators"
        );
        assert_eq!(
            strip_ansi("a\u{206A}b"),
            "ab",
            "deprecated Cf format character"
        );
        assert_eq!(strip_ansi("a\u{FE0F}b"), "ab", "variation selector");
        assert_eq!(
            strip_ansi("a\u{E0100}b"),
            "ab",
            "supplementary variation selector"
        );
        assert_eq!(strip_ansi("a\u{FFF9}b"), "ab", "interlinear annotation");
        assert_eq!(strip_ansi("a\u{115F}b"), "ab", "Hangul choseong filler");
        assert_eq!(strip_ansi("a\u{3164}b"), "ab", "Hangul filler");
        // The Unicode tag block (M5's headline addition): renders as nothing
        // in a terminal, so a name like `review\u{E0041}` (TAG LATIN SMALL
        // LETTER A appended) would otherwise sanitize identically to `review`
        // while remaining a distinct raw string -- exactly the ambiguity
        // DSC-96 relies on this module to close.
        assert_eq!(
            strip_ansi("review\u{E0041}"),
            "review",
            "Unicode tag block character"
        );
        assert_eq!(
            strip_ansi("\u{E0001}\u{E007F}"),
            "",
            "tag-block language tag and cancel tag"
        );
    }

    // spec: TUI-60
    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    // spec: DSC-95 NS-73
    #[test]
    fn has_blocked_chars_flags_exactly_what_strip_removes() {
        // Clean strings (including printable non-ASCII) are not flagged, and
        // `strip_ansi` is a no-op on them -- the two must agree in both
        // directions (M5b), not merely on membership in these sample lists.
        let ok = ["", "hello", "my-skill", "caf\u{00e9}", "a.b_c-2"];
        for s in ok {
            assert!(!has_blocked_chars(s), "{s:?} should be clean");
            assert_eq!(
                has_blocked_chars(s),
                strip_ansi(s) != s,
                "has_blocked_chars/strip_ansi disagree on {s:?}"
            );
        }
        // Control characters, ANSI escapes (ESC is a control char), and the
        // security-blocked Unicode set (the original bidi/zero-width/separator
        // set plus the NS-73 broadening: soft hyphen, Mongolian vowel
        // separator, invisible math operators, deprecated Cf format chars,
        // variation selectors, interlinear annotation, Hangul fillers, and the
        // Unicode tag block) are all flagged, and every one of them is also a
        // case where `strip_ansi` actually changes the string -- pinning that
        // the predicate flags EXACTLY what the stripper removes, not a
        // superset or subset of it (M5b).
        let bad = [
            "a\x00b",
            "a\nb",
            "a\x1b[0mb",
            "pay\u{202E}oot",
            "a\u{2066}b",
            "a\u{200B}b",
            "a\u{200E}b",
            "a\u{FEFF}b",
            "a\u{2028}b",
            "a\u{0085}b",
            // NS-73 additions.
            "a\u{00AD}b",
            "a\u{180E}b",
            "a\u{2061}b",
            "a\u{206A}b",
            "a\u{FE0F}b",
            "a\u{E0100}b",
            "a\u{FFF9}b",
            "a\u{115F}b",
            "a\u{3164}b",
            "review\u{E0041}",
        ];
        for s in bad {
            assert!(has_blocked_chars(s), "{s:?} should be flagged");
            assert_eq!(
                has_blocked_chars(s),
                strip_ansi(s) != s,
                "has_blocked_chars/strip_ansi disagree on {s:?}"
            );
        }
    }

    // spec: CLI-224
    // INDEPENDENT CERTIFICATION of the load-bearing claim in `strip_ansi`'s doc
    // comment: `strip_ansi_escapes::strip` (the first pass) forwards only `\n`
    // among C0/DEL/C1 control bytes, so the collapse-to-space behavior added by
    // CLI-224 can only ever affect a `\n` in practice. If a TAB or CR survived
    // the first pass, it would reach the collapse loop and become a space,
    // widening the shared-caller impact beyond what the doc comment documents
    // (hook.rs disclosure, selfupdate.rs, tui/data.rs). These pin that they do
    // NOT survive: a TAB or CR is consumed by the first pass and leaves nothing
    // (not even a space), exactly like NUL / `\x01` / `\x1f` above.
    #[test]
    fn strip_ansi_tab_and_cr_are_consumed_by_first_pass_leaving_no_space() {
        // TAB (0x09) is dropped outright, no space -- proving it never reaches
        // the collapse loop. If this became "a b", TAB survived the first pass
        // and the collapse behavior now affects tabs too (wider than documented).
        assert_eq!(
            strip_ansi("a\tb"),
            "ab",
            "TAB must not survive to become a space"
        );
        // A lone CR (0x0d), not part of a CRLF, is likewise consumed with no
        // space. (Within a run alongside `\n`, e.g. "\r\n", the run collapses to
        // one space; that is covered separately above.)
        assert_eq!(
            strip_ansi("a\rb"),
            "ab",
            "lone CR must not survive to become a space"
        );
        // Vertical tab and form feed, other C0 controls, are also consumed.
        assert_eq!(strip_ansi("a\x0bb"), "ab", "VT must not survive");
        assert_eq!(strip_ansi("a\x0cb"), "ab", "FF must not survive");
        // An ESC that opens a recognized sequence is consumed with the sequence,
        // never emitted as a control char the collapse loop would turn to a space.
        assert_eq!(
            strip_ansi("a\x1b[0mb"),
            "ab",
            "an ANSI reset leaves no space"
        );
        // A trailing lone ESC (nothing after it to form a sequence) is likewise
        // dropped, leaving no space.
        assert_eq!(
            strip_ansi("ab\x1b"),
            "ab",
            "a trailing bare ESC leaves no space"
        );
        // The one control that DOES survive the first pass is `\n`, and only it
        // drives the collapse-to-space -- the asymmetry the whole CLI-224
        // rationale rests on.
        assert_eq!(
            strip_ansi("a\nb"),
            "a b",
            "newline is the sole survivor and becomes a space"
        );
    }
}
