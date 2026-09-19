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
    ///
    /// Every pass of the loop advances the cursor, unconditionally. Neither
    /// `read_ident` nor `skip_value` consumes a character the other cannot
    /// start with - a stray `)` or `]` at depth 0, say, which `read_ident`
    /// refuses and `skip_value` leaves for a caller that is not there - so
    /// without the explicit bump below the loop would restart on the same index
    /// forever. This reader runs on every discovered workflow in the ordinary
    /// catalog scan, holding the process lock, so a no-progress path is a hang
    /// of every verb, not a bad read. WF-5 lets this yield less on malformed
    /// code; it never lets it fail or stall.
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
            let entry_start = self.i;

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
                if self.i == entry_start {
                    // Nothing here was consumable. Drop the character and carry
                    // on: the object is malformed, and the keys after it are
                    // still worth reading.
                    self.i += 1;
                }
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

    /// [`parse`] under a deadline, so a no-progress regression fails the test
    /// rather than hanging the whole suite on it. A stuck worker thread is
    /// abandoned: the test has already failed and the process is going down.
    fn parse_bounded(text: &str) -> WorkflowMeta {
        let owned = text.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(parse(&owned));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(m) => m,
            Err(_) => panic!("parse did not return on {text:?}: the cursor is not advancing"),
        }
    }

    // spec: WF-5 -- a stray closer at depth 0 is a character no key reader will
    // start on and no value skipper will consume, so the entry loop has to drop
    // it itself. This reader runs on every discovered workflow while the
    // process lock is held, so standing still here would wedge every verb.
    #[test]
    fn returns_on_a_stray_closing_paren() {
        assert!(parse_bounded("export const meta = { )").is_empty());
    }

    // spec: WF-5 -- the same dead end reached through the value position.
    #[test]
    fn returns_on_a_stray_closing_bracket_in_the_value_position() {
        let m = parse_bounded("export const meta = { name: ]");
        assert_eq!(m.name, None);
        assert!(m.is_empty());
    }

    // spec: WF-5 -- a regex literal is a shape this reader does not know, and a
    // `)` inside one reads as an unbalanced closer. Dropping it must cost only
    // that key: the string keys on either side still read.
    #[test]
    fn reads_around_a_regex_literal_holding_a_closing_paren() {
        let m = parse_bounded(
            "export const meta = { name: 'deploy', pattern: /foo)bar/, description: 'Deploy it' }",
        );
        assert_eq!(m.name.as_deref(), Some("deploy"));
        assert_eq!(m.description.as_deref(), Some("Deploy it"));
    }

    // spec: WF-5 -- an arrow body whose parens do not balance leaves a closer
    // in the entry position, and the object still ends where its `}` says.
    #[test]
    fn returns_on_an_unbalanced_arrow_body() {
        let m = parse_bounded("export const meta = { arrow: () => x), }");
        assert!(m.is_empty());
    }

    /// Run `f` on a worker thread under a deadline, the way [`parse_bounded`]
    /// runs one parse, for the checks below that cover many inputs at once. A
    /// stuck worker is abandoned: the test has already failed.
    fn run_bounded<T: Send + 'static>(
        what: &str,
        secs: u64,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
            Ok(v) => v,
            Err(_) => panic!("{what} did not return: the cursor is not advancing"),
        }
    }

    // spec: WF-5 -- `skip_value` returns at a closer it did not open, so an
    // unbalanced one inside a nested context surfaces in the entry position
    // once the nesting it did open is spent. The entry loop drops it and reads
    // on; only a closer that balances the value's own opener (the `}` case)
    // ends the literal early, which costs the keys after it but never a stall.
    #[test]
    fn returns_on_a_closer_nested_inside_a_value() {
        for (text, expected) in [
            ("export const meta = { a: [ ) ], name: 'x' }", Some("x")),
            ("export const meta = { a: [ } ], name: 'x' }", Some("x")),
            ("export const meta = { a: ( ] ), name: 'x' }", Some("x")),
            ("export const meta = { a: [[[)))]]], name: 'x' }", Some("x")),
            ("export const meta = { [)]: 1, name: 'x' }", Some("x")),
            ("export const meta = { a: { ) }, name: 'x' }", None),
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), expected, "from {text:?}");
        }
    }

    // spec: WF-5 -- the same dead end at the very first entry, where
    // `seek_meta_object` hands a cursor sitting directly on the closer.
    #[test]
    fn returns_on_a_closer_immediately_after_the_opening_brace() {
        for (text, expected) in [
            ("export const meta = {)", None),
            ("export const meta = {]", None),
            ("export const meta = {)))", None),
            ("export const meta = {)name: 'x' }", Some("x")),
            ("export const meta = {]]]name: 'x'}", Some("x")),
            ("export const meta = {)}name: 'late'}", None),
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), expected, "from {text:?}");
        }
    }

    // spec: WF-5 -- dropping a stray closer costs exactly that character: the
    // key after it is still read, and the key before it is kept.
    #[test]
    fn a_dropped_closer_costs_only_itself() {
        let m = parse_bounded("export const meta = { name: 'a' ) description: 'b' }");
        assert_eq!(m.name.as_deref(), Some("a"));
        assert_eq!(m.description.as_deref(), Some("b"));
    }

    // spec: WF-5 -- the value position, entered only after a `:` has been
    // consumed, on every malformed shape worth naming. None of these can bind a
    // value, and none of them may fail to return.
    #[test]
    fn every_malformed_value_position_terminates() {
        for text in [
            "export const meta = { name: )",
            "export const meta = { name: ]",
            "export const meta = { name: }",
            "export const meta = { name: :",
            "export const meta = { name: ,",
            "export const meta = { name: ",
            "export const meta = { name:",
            "export const meta = { name: /re)gex/ }",
            "export const meta = { name: () => ) }",
            "export const meta = { name: [ ) }",
            "export const meta = { name: { ) } }",
            "export const meta = { name: `${",
            "export const meta = { name: `open",
            "export const meta = { name: '",
            "export const meta = { name: \"",
            "export const meta = { name: 'abc\\",
            "export const meta = { name: \\ }",
            "export const meta = { name: /* unterminated",
            "export const meta = { name: // eol only",
            "export const meta = { name: 0x, description: ) }",
            "export const meta = { name: ...spread }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name, None, "nothing readable in {text:?}");
        }
    }

    // spec: WF-5 -- an unterminated literal in the value position consumes to
    // end of file (that is what keeps its body from being re-read as code), so
    // the keys after it are inside the literal and absent, not misread.
    #[test]
    fn an_unterminated_value_swallows_the_rest_of_the_object() {
        for text in [
            "export const meta = { description: `open, name: 'x' }",
            "export const meta = { description: 'open, name: 'x' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name, None, "from {text:?}");
        }
    }

    // spec: WF-5 -- a long run of characters that neither reader will consume
    // is dropped one per pass, and the keys after it still read. A no-progress
    // regression shows up here as a hang rather than a wrong value.
    #[test]
    fn a_long_run_of_stray_closers_still_terminates() {
        let text = format!(
            "export const meta = {{ {}{}, name: 'survivor' }}",
            ")".repeat(5000),
            "]".repeat(5000)
        );
        let m = parse_bounded(&text);
        assert_eq!(m.name.as_deref(), Some("survivor"));
    }

    /// Drive [`Cursor::read_string`] directly. Its "consumed either way"
    /// contract is what stops the entry loop from re-reading a literal's body
    /// as code, and `parse` shows only the half where a value comes back.
    fn read_string_at(text: &str) -> (Option<String>, usize) {
        let chars: Vec<char> = text.chars().collect();
        let mut cur = Cursor { s: &chars, i: 0 };
        let value = cur.read_string();
        (value, cur.i)
    }

    // spec: WF-5 -- an unterminated template runs to end of file, so the caller
    // is left at EOF and returns rather than re-reading the body.
    #[test]
    fn an_unterminated_template_consumes_to_end_of_file() {
        let text = "`no closer here";
        let (value, at) = read_string_at(text);
        assert_eq!(value, None);
        assert_eq!(at, text.chars().count());
        // The same for the other two quote forms.
        assert_eq!(read_string_at("'open").0, None);
        assert_eq!(read_string_at("\"open").0, None);
    }

    // spec: WF-5 -- an interpolated template yields no value but is consumed
    // through its closing backtick, leaving the cursor on the code after it.
    #[test]
    fn an_interpolated_template_is_consumed_through_its_closer() {
        let (value, at) = read_string_at("`a${b}c`, name: 'x'");
        assert_eq!(value, None);
        assert_eq!(at, 8, "the cursor sits on the comma after the template");
    }

    // spec: WF-5 -- a literal ending in a dangling escape is unterminated, and
    // the escape consumes the end of input rather than looping on it.
    #[test]
    fn a_trailing_backslash_ends_an_unterminated_literal() {
        let (value, at) = read_string_at("'abc\\");
        assert_eq!(value, None);
        assert_eq!(at, 5);
    }

    // spec: WF-5 -- `${` is interpolation only in a template; elsewhere it is
    // two ordinary characters, and a lone `$` in a template is literal too.
    #[test]
    fn a_dollar_outside_an_interpolation_is_literal() {
        assert_eq!(read_string_at("`cost: $5`").0.as_deref(), Some("cost: $5"));
        assert_eq!(read_string_at("'${x}'").0.as_deref(), Some("${x}"));
        assert_eq!(read_string_at("\"${x}\"").0.as_deref(), Some("${x}"));
    }

    // spec: WF-5 -- asked to read a literal where there is none, it declines
    // without moving, so the key reader keeps the character for `read_ident`.
    #[test]
    fn read_string_declines_a_non_literal_without_moving() {
        let (value, at) = read_string_at("name: 'x'");
        assert_eq!(value, None);
        assert_eq!(at, 0);
    }

    // spec: WF-5 -- a randomized sweep over the tokens an object literal is
    // made of. The reader must return on every arrangement of them, balanced or
    // not; the seed is fixed, so any failure reproduces exactly. This is the
    // general form of the hang: a character that no reader in the loop will
    // consume, reached in a position nobody wrote a case for.
    #[test]
    fn a_randomized_sweep_of_object_bodies_always_returns() {
        const TOKENS: [&str; 28] = [
            "{",
            "}",
            "[",
            "]",
            "(",
            ")",
            "'",
            "\"",
            "`",
            ",",
            ":",
            "/",
            "*",
            "\\",
            "$",
            "${",
            "=>",
            "//",
            "/*",
            "*/",
            " ",
            "\n",
            "name",
            "description",
            "whenToUse",
            "meta",
            "export const meta =",
            "x",
        ];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize
        };
        let mut cases: Vec<String> = Vec::with_capacity(4000);
        for n in 0..4000usize {
            let len = 1 + next() % 24;
            let mut body = String::new();
            for _ in 0..len {
                body.push_str(TOKENS[next() % TOKENS.len()]);
            }
            // Half the cases are handed straight to `read_meta_object` through a
            // well-formed opening; the other half also exercise the seek.
            cases.push(match n % 2 {
                0 => format!("export const meta = {{{body}"),
                _ => body,
            });
        }
        run_bounded("the randomized sweep", 60, move || {
            for case in &cases {
                let _ = parse(case);
            }
        });
    }
}
