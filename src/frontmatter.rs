//! Minimal YAML-frontmatter reader.
//!
//! Items already carry metadata in a leading `--- ... ---` block (skills in
//! `SKILL.md`, agents/rules in their `.md`). We only need a few top-level
//! string keys (today: `description`), so rather than pull in a full YAML
//! parser we scan the block for `key:` lines at column zero.
//!
//! Supported value forms:
//! - Plain scalars:  `key: some text`
//! - Quoted scalars: `key: "some text"` or `key: 'some text'`
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

/// Read the top-level `description` from a file's frontmatter, if present.
pub fn description(file: &Path) -> Option<String> {
    file_field(file, "description")
}

/// Read a top-level scalar `key` from a file's frontmatter, if present.
pub fn file_field(file: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    field(&text, key)
}

/// Extract a top-level scalar `key` from the leading frontmatter block.
pub fn field(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    // The very first line must be the opening delimiter.
    if lines.next()?.trim() != "---" {
        return None;
    }
    while let Some(line) = lines.next() {
        if line.trim() == "---" {
            break; // end of frontmatter
        }
        if let Some(rest) = line.strip_prefix(key)
            && let Some(value) = rest.strip_prefix(':')
        {
            let trimmed = value.trim();
            // Detect a block-scalar indicator: `>` or `|`, optional chomping.
            if let Some((style, chomp)) = parse_block_indicator(trimmed) {
                // Collect subsequent lines until a column-zero non-empty line or `---`.
                let mut block_lines: Vec<&str> = Vec::new();
                loop {
                    match lines.peek() {
                        None => break,
                        Some(&next) => {
                            // A non-empty line at column zero ends the block.
                            let is_col0_nonempty = !next.is_empty()
                                && !next.starts_with(' ')
                                && !next.starts_with('\t');
                            if is_col0_nonempty {
                                // Closing `---` also ends the block; don't consume it
                                // so the outer loop can handle it as the delimiter.
                                break;
                            }
                            lines.next(); // consume
                            block_lines.push(next);
                        }
                    }
                }
                return Some(render_block(&block_lines, style, chomp));
            }
            return Some(unquote(trimmed));
        }
    }
    None
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

    // Dedent: strip min_indent leading spaces from each line.
    let dedented: Vec<&str> = lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                // Shorter line (only spaces) -> treat as empty.
                ""
            }
        })
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

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
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
    // plain scalar (the literal text) without panicking.
    #[test]
    fn malformed_indicator_falls_back_to_plain_scalar() {
        let t = "---\ndescription: >x\n  not part of a block\n---\n";
        let result = field(t, "description").unwrap();
        assert_eq!(result, ">x");
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
}
