//! Minimal reader for a workflow's `export const meta = { ... }` block.
//!
//! A workflow is a `.js` file (workflows.md WF-1), so it carries no YAML
//! frontmatter and `frontmatter.rs` has nothing to read. The metadata mind
//! wants lives in the object literal the harness requires at the top of the
//! file, so this is the same idea as the frontmatter reader applied to a
//! different syntax: scan for a few top-level string keys rather than pull in a
//! JavaScript parser.
//!
//! It reads exactly three keys, all optional: `name`, `description`, and
//! `whenToUse` (WF-5). Everything else in the object is skipped over, including
//! the nested `phases` array a real workflow carries.
//!
//! This is deliberately NOT a JavaScript parser, and the harness's own reader is
//! the authority on what actually loads (WF-5): it parses to an AST and admits a
//! narrower literal form than this accepts. A disagreement in either direction
//! changes only the description mind displays and the WF-30 finding, never
//! whether a file installs, so this reader never fails. Where it cannot read a
//! value it yields `None`.
//!
//! What it does handle:
//! - The `export const meta = {` opening, with any whitespace or comment
//!   (`//` and `/* */`) between the tokens.
//! - Keys written bare (`name:`) or quoted (`'name':`).
//! - String values in single quotes, double quotes, or an uninterpolated
//!   backtick template. A template containing `${` is skipped: the harness
//!   rejects it, and mind is not going to evaluate it.
//! - Backslash escapes in a string value: `\\`, `\'`, `\"`, `` \` ``, `\n`,
//!   `\r`, `\t`, `\0`. Any other escape yields the escaped character itself,
//!   so `\d` reads as `d`.
//! - Any non-string value (a number, an array, a nested object, a call), by
//!   skipping to the next top-level comma. A `name` key nested inside `phases`
//!   is therefore invisible: only depth-1 keys are read.
//!
//! Values are returned exactly as written, untrimmed and with an empty string
//! kept as `Some("")`. Deciding that an empty `name` is a defect is WF-30's job
//! and an empty `description` is the catalog's; conflating either with "absent"
//! here would throw away the distinction before its reader sees it.
//!
//! The consumers are the catalog scan (WF-4, WF-51) and `workflow_check`, which
//! turns what is read here into the WF-24, WF-29, and WF-30 reports.

use std::path::Path;

use crate::error::{MindError, Result};

/// The three fields mind reads out of a workflow's `meta` object (WF-5). Every
/// field is absent unless the object declared it with a readable string value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkflowMeta {
    /// `meta.name`: the name the harness resolves the workflow by (WF-20),
    /// which is not necessarily the item's name (WF-21, WF-24).
    pub name: Option<String>,
    /// `meta.description`: the item's description (WF-4).
    pub description: Option<String>,
    /// `meta.whenToUse`: a second description field no other kind has,
    /// surfaced beside the description (WF-51).
    pub when_to_use: Option<String>,
}

impl WorkflowMeta {
    /// Whether no field was read at all: an unreadable `meta`, or one declaring
    /// none of the three keys with a string value. The WF-30 "no readable
    /// `meta`" condition.
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none() && self.when_to_use.is_none()
    }
}

/// Read a workflow file's `meta`, size-capped like every other metadata read
/// (DSC-91).
///
/// Only the size cap is a hard error, matching [`crate::frontmatter::text_capped`].
/// An absent, unreadable, or non-UTF-8 file yields an empty [`WorkflowMeta`],
/// because a reader that cannot read is exactly the "yields nothing" case WF-5
/// describes, not a reason to fail a scan. The cap is 8 MiB against the
/// harness's 512 KiB workflow limit (WF-7), so every loadable workflow is read
/// whole; a file that trips it is 16x past the point where the harness would
/// have skipped it anyway.
pub fn file_meta(file: &Path) -> Result<WorkflowMeta> {
    match crate::error::read_capped_metadata(file) {
        Ok(text) => Ok(parse(&text)),
        Err(err @ MindError::MetadataTooLarge { .. }) => Err(err),
        Err(_) => Ok(WorkflowMeta::default()),
    }
}

/// Extract `name`, `description`, and `whenToUse` from the `export const meta`
/// object literal in `text`. Yields an empty [`WorkflowMeta`] when there is no
/// such declaration or its initializer is not an object literal.
///
/// A UTF-8 BOM at the start of the text is stripped first, as the frontmatter
/// reader does (DSC-23).
pub fn parse(text: &str) -> WorkflowMeta {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let chars: Vec<char> = text.chars().collect();
    let mut cur = Cursor { s: &chars, i: 0 };
    match cur.seek_meta_object() {
        true => cur.read_meta_object(),
        false => WorkflowMeta::default(),
    }
}

/// A character cursor over the file, with just enough JavaScript awareness to
/// skip what must be skipped: whitespace, comments, and string literals.
struct Cursor<'a> {
    s: &'a [char],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.s.get(self.i + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    /// Skip whitespace and both comment forms. An unterminated `/*` runs to end
    /// of file, which is what a JavaScript tokenizer does before erroring.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.i += 1;
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    self.i += 2;
                    while !self.eof() {
                        if self.peek() == Some('*') && self.peek_at(1) == Some('/') {
                            self.i += 2;
                            break;
                        }
                        self.i += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// Read an identifier, or `None` if the cursor is not on one. JavaScript
    /// identifiers may start with `$` or `_`; the exact Unicode identifier rules
    /// do not matter here, since the only identifiers this reader compares
    /// against are ASCII.
    fn read_ident(&mut self) -> Option<String> {
        let start = self.i;
        match self.peek() {
            Some(c) if c.is_alphanumeric() || c == '_' || c == '$' => {}
            _ => return None,
        }
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '$' {
                self.i += 1;
            } else {
                break;
            }
        }
        Some(self.s[start..self.i].iter().collect())
    }

    /// Read a string literal, consuming it either way. Yields `None` for an
    /// interpolated template (the harness rejects those) and for an unterminated
    /// literal, in both cases after consuming what is there so the caller does
    /// not re-read it as code.
    fn read_string(&mut self) -> Option<String> {
        let quote = match self.peek() {
            Some(c @ ('\'' | '"' | '`')) => c,
            _ => return None,
        };
        self.i += 1;
        let mut out = String::new();
        let mut interpolated = false;
        let mut terminated = false;
        while let Some(c) = self.bump() {
            match c {
                c if c == quote => {
                    terminated = true;
                    break;
                }
                '\\' => match self.bump() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('0') => out.push('\0'),
                    Some(other) => out.push(other),
                    None => break,
                },
                '$' if quote == '`' && self.peek() == Some('{') => {
                    interpolated = true;
                    out.push(c);
                }
                c => out.push(c),
            }
        }
        if !terminated || interpolated {
            return None;
        }
        Some(out)
    }

    /// Advance to the `{` of the `export const meta` initializer, consuming it.
    /// Returns `false` if the file has no such declaration.
    ///
    /// The match is on the identifier sequence `export`, `const`, `meta`, so
    /// whitespace and comments between the tokens are immaterial and the
    /// identifier `metadata` does not match `meta`. Scanning skips strings and
    /// comments, so the same three words inside a prompt do not trip it.
    fn seek_meta_object(&mut self) -> bool {
        // The two identifiers most recently read, oldest first.
        let mut prev: [Option<String>; 2] = [None, None];
        loop {
            self.skip_trivia();
            let Some(c) = self.peek() else { return false };
            if c == '\'' || c == '"' || c == '`' {
                let _ = self.read_string();
                continue;
            }
            let Some(word) = self.read_ident() else {
                self.i += 1;
                continue;
            };
            let matched = word == "meta"
                && prev[1].as_deref() == Some("const")
                && prev[0].as_deref() == Some("export");
            if matched {
                self.skip_trivia();
                if self.peek() != Some('=') {
                    return false;
                }
                self.i += 1;
                self.skip_trivia();
                if self.peek() != Some('{') {
                    // An initializer that is not an object literal: a call, an
                    // identifier, a spread. Nothing to read, and the harness
                    // rejects it too.
                    return false;
                }
                self.i += 1;
                return true;
            }
            prev = [prev[1].take(), Some(word)];
        }
    }

    /// Read the depth-1 keys of the object literal the cursor sits inside,
    /// having just consumed its `{`.
    fn read_meta_object(&mut self) -> WorkflowMeta {
        let mut meta = WorkflowMeta::default();
        loop {
            self.skip_trivia();
            let Some(c) = self.peek() else { return meta };
            match c {
                '}' => {
                    self.i += 1;
                    return meta;
                }
                ',' => {
                    self.i += 1;
                    continue;
                }
                _ => {}
            }

            // The key: bare, quoted, or computed. A computed key is read as no
            // key at all, so its value is skipped like any unrecognized one.
            let key = match c {
                '\'' | '"' | '`' => self.read_string(),
                '[' => {
                    self.skip_value();
                    None
                }
                _ => self.read_ident(),
            };

            self.skip_trivia();
            if self.peek() != Some(':') {
                // Shorthand (`name,`), a method (`run() {}`), or a spread.
                // Nothing to bind, so skip to the next entry.
                self.skip_value();
                continue;
            }
            self.i += 1;
            self.skip_trivia();

            let value = match self.peek() {
                Some('\'' | '"' | '`') => {
                    let v = self.read_string();
                    // A template that turned out to be interpolated has been
                    // consumed; there is nothing left of this entry to skip.
                    v
                }
                _ => {
                    self.skip_value();
                    None
                }
            };

            // Last writer wins, as it does in JavaScript. A duplicate key in a
            // hand-written `meta` is pathological, but guessing the other way
            // would report a value the harness does not use.
            match (key.as_deref(), value) {
                (Some("name"), Some(v)) => meta.name = Some(v),
                (Some("description"), Some(v)) => meta.description = Some(v),
                (Some("whenToUse"), Some(v)) => meta.when_to_use = Some(v),
                _ => {}
            }
        }
    }

    /// Skip a value (or anything else) up to the comma or closing brace that
    /// ends it, tracking nesting and skipping strings and comments. The
    /// terminator itself is left for the caller.
    fn skip_value(&mut self) {
        let mut depth = 0usize;
        loop {
            self.skip_trivia();
            let Some(c) = self.peek() else { return };
            match c {
                '\'' | '"' | '`' => {
                    let _ = self.read_string();
                }
                '{' | '[' | '(' => {
                    depth += 1;
                    self.i += 1;
                }
                '}' | ']' | ')' => {
                    if depth == 0 {
                        // The object's own closing brace: the caller handles it.
                        return;
                    }
                    depth -= 1;
                    self.i += 1;
                }
                ',' if depth == 0 => return,
                _ => self.i += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(text: &str) -> WorkflowMeta {
        parse(text)
    }

    // spec: WF-5 -- the three string keys are read out of the object literal
    // the harness requires, and the description comes from here rather than
    // from frontmatter a `.js` file does not have (WF-4).
    #[test]
    fn reads_the_three_string_keys() {
        let m = meta(
            "export const meta = {\n  name: 'review-changes',\n  description: 'Review a diff',\n  whenToUse: 'Before a release',\n}\n",
        );
        assert_eq!(m.name.as_deref(), Some("review-changes"));
        assert_eq!(m.description.as_deref(), Some("Review a diff"));
        assert_eq!(m.when_to_use.as_deref(), Some("Before a release"));
        assert!(!m.is_empty());
    }

    // spec: WF-5 -- all three JavaScript string forms are values; an
    // uninterpolated template is one the harness admits.
    #[test]
    fn reads_every_uninterpolated_string_form() {
        let m = meta(
            "export const meta = { name: \"double\", description: 'single', whenToUse: `backtick` }",
        );
        assert_eq!(m.name.as_deref(), Some("double"));
        assert_eq!(m.description.as_deref(), Some("single"));
        assert_eq!(m.when_to_use.as_deref(), Some("backtick"));
    }

    // spec: WF-5 -- an interpolated template is not a value this reader can
    // produce, so that key alone is absent and its siblings still read.
    #[test]
    fn skips_an_interpolated_template_without_losing_its_siblings() {
        let m = meta("export const meta = { name: `pre${x}post`, description: 'kept' }");
        assert_eq!(m.name, None);
        assert_eq!(m.description.as_deref(), Some("kept"));
    }

    // spec: WF-5 -- backslash escapes inside a value.
    #[test]
    fn resolves_backslash_escapes_in_a_value() {
        let m = meta(r"export const meta = { name: 'it\'s', description: 'a\\b\nc\td\qe' }");
        assert_eq!(m.name.as_deref(), Some("it's"));
        // `\\` -> a backslash, `\n`/`\t` -> the control characters, and the
        // unrecognized `\q` -> a bare `q`.
        assert_eq!(m.description.as_deref(), Some("a\\b\nc\tdqe"));
    }

    // spec: WF-5 -- only depth-1 keys are read. A real workflow's `phases`
    // array carries `title`/`detail` objects, and a nested `name` must not be
    // mistaken for the workflow's own.
    #[test]
    fn reads_only_top_level_keys() {
        let m = meta(
            "export const meta = {\n  name: 'outer',\n  phases: [{ name: 'inner', detail: 'x' }, { name: 'inner2' }],\n  description: 'after the array',\n}",
        );
        assert_eq!(m.name.as_deref(), Some("outer"));
        assert_eq!(m.description.as_deref(), Some("after the array"));
    }

    // spec: WF-5 -- a non-string value is skipped, whatever its shape, and the
    // keys around it still read.
    #[test]
    fn skips_non_string_values() {
        let m = meta(
            "export const meta = {\n  count: 3,\n  nested: { a: { b: [1, 2] } },\n  computed: fn(1, 2),\n  flag: true,\n  neg: -1,\n  name: 'still read',\n}",
        );
        assert_eq!(m.name.as_deref(), Some("still read"));
        assert_eq!(m.description, None);
    }

    // spec: WF-5 -- a quoted key is a key.
    #[test]
    fn reads_a_quoted_key() {
        let m = meta("export const meta = { 'name': 'q', \"description\": 'd' }");
        assert_eq!(m.name.as_deref(), Some("q"));
        assert_eq!(m.description.as_deref(), Some("d"));
    }

    // spec: WF-5 -- a computed key binds nothing, and does not consume the
    // entry after it.
    #[test]
    fn skips_a_computed_key() {
        let m = meta("export const meta = { [k]: 'ignored', name: 'kept' }");
        assert_eq!(m.name.as_deref(), Some("kept"));
    }

    // spec: WF-5 -- shorthand and method entries bind nothing here.
    #[test]
    fn skips_shorthand_and_method_entries() {
        let m = meta("export const meta = { name, run() { return 1 }, description: 'kept' }");
        assert_eq!(m.name, None);
        assert_eq!(m.description.as_deref(), Some("kept"));
    }

    // spec: WF-5 -- comments anywhere in or before the object are trivia.
    #[test]
    fn skips_comments() {
        let m = meta(
            "// leading\n/* block */\nexport /* mid */ const // eol\n meta = {\n  // about the name\n  name: 'c', /* trailing */\n}",
        );
        assert_eq!(m.name.as_deref(), Some("c"));
    }

    // spec: WF-5 -- a trailing comma is legal and ends the object cleanly.
    #[test]
    fn accepts_a_trailing_comma() {
        let m = meta("export const meta = { name: 'x', }");
        assert_eq!(m.name.as_deref(), Some("x"));
    }

    // spec: WF-5 -- the reader never fails on the shape of a workflow's code.
    // Each of these yields nothing rather than an error.
    #[test]
    fn yields_nothing_rather_than_failing() {
        for text in [
            "",
            "console.log('no meta here')",
            "export const metadata = { name: 'not meta' }",
            "const meta = { name: 'no export' }",
            "export const meta = buildMeta()",
            "export const meta = ",
            "export const meta = {",
            "export const meta = { name: 'unterminated",
            "export const meta = { /* unterminated comment",
        ] {
            assert!(
                meta(text).is_empty(),
                "expected no fields from {text:?}, got {:?}",
                meta(text)
            );
        }
    }

    // spec: WF-5 -- the declaration is found by identifier sequence, so the
    // same words inside a string (an agent prompt quoting the form) do not
    // start the object early.
    #[test]
    fn ignores_the_declaration_spelled_inside_a_string() {
        let m = meta(
            "const doc = \"export const meta = { name: 'fake' }\"\nexport const meta = { name: 'real' }",
        );
        assert_eq!(m.name.as_deref(), Some("real"));
    }

    // spec: WF-5 -- a value is returned as written. An empty string stays a
    // present-but-empty value, which is what WF-30 reports on.
    #[test]
    fn keeps_an_empty_value_present() {
        let m = meta("export const meta = { name: '', description: '  padded  ' }");
        assert_eq!(m.name.as_deref(), Some(""));
        assert_eq!(m.description.as_deref(), Some("  padded  "));
        assert!(!m.is_empty());
    }

    // spec: WF-5 -- a duplicate key resolves as JavaScript resolves it.
    #[test]
    fn last_duplicate_key_wins() {
        let m = meta("export const meta = { name: 'first', name: 'second' }");
        assert_eq!(m.name.as_deref(), Some("second"));
    }

    // spec: DSC-23 -- a leading BOM is stripped before the scan, as it is for
    // frontmatter.
    #[test]
    fn strips_a_leading_bom() {
        let m = meta("\u{FEFF}export const meta = { name: 'bom' }");
        assert_eq!(m.name.as_deref(), Some("bom"));
    }

    // spec: WF-5 -- an unreadable file is "no meta", not an error, so a scan is
    // never failed by one. The other half of `file_meta`'s contract, the DSC-91
    // size cap being its one hard error, belongs to `read_capped_metadata` and
    // is covered against that reader in `error.rs`.
    #[test]
    fn an_absent_file_reads_as_empty() {
        let dir = std::env::temp_dir().join(format!("mind-wfmeta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = file_meta(&dir.join("nope.js")).unwrap();
        assert!(m.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: WF-5 -- a file that is there reads through the same entry point.
    #[test]
    fn a_file_on_disk_reads_its_meta() {
        let dir = std::env::temp_dir().join(format!("mind-wfmeta-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("review.js");
        std::fs::write(&file, "export const meta = { name: 'from-disk' }\n").unwrap();
        assert_eq!(file_meta(&file).unwrap().name.as_deref(), Some("from-disk"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: WF-5 -- a real workflow file, read whole.
    #[test]
    fn reads_a_realistic_workflow_header() {
        let m = meta(
            r#"
export const meta = {
  name: 'find-flaky-tests',
  description: 'Find flaky tests and propose fixes',
  whenToUse: 'When CI is red intermittently',
  phases: [
    { title: 'Scan', detail: 'grep test logs for retries' },
    { title: 'Fix', detail: 'one agent per flaky test' },
  ],
}

phase('Scan')
const flaky = await agent('grep CI logs for retry markers', { schema: FLAKY_SCHEMA })
"#,
        );
        assert_eq!(m.name.as_deref(), Some("find-flaky-tests"));
        assert_eq!(
            m.description.as_deref(),
            Some("Find flaky tests and propose fixes")
        );
        assert_eq!(
            m.when_to_use.as_deref(),
            Some("When CI is red intermittently")
        );
    }
}
