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
/// Only the size cap is an error, matching [`crate::frontmatter::text_capped`].
/// An absent, unreadable, or non-UTF-8 file yields an empty [`WorkflowMeta`],
/// because a reader that cannot read is exactly the "yields nothing" case WF-5
/// describes, not a reason to fail a scan.
///
/// The cap is a *configured* value (DSC-103), defaulting to 8 MiB against the
/// harness's 512 KiB workflow limit (WF-7): at the default every loadable
/// workflow is read whole and a file that trips it is 16x past the point where
/// the harness would have skipped it anyway, but an operator may lower it below
/// the harness's own limit, where a perfectly loadable workflow trips it. That
/// is why the error is passed up rather than folded into an empty `meta` here:
/// the caller (`workflow_check::read`) keeps it apart from "the file declares
/// nothing" and reports it as mind's own cap (WF-56), since only the caller
/// knows the difference matters.
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
    let mut cur = Cursor { s: text, i: 0 };
    match cur.seek_meta_object() {
        true => cur.read_meta_object(),
        false => WorkflowMeta::default(),
    }
}

/// A cursor over the file, with just enough JavaScript awareness to skip what
/// must be skipped: whitespace, comments, and string literals.
///
/// It walks the `&str` itself, holding a BYTE offset that always sits on a
/// character boundary (every advance moves by a whole character). Collecting a
/// `Vec<char>` first would cost four bytes per character of a file whose size
/// is bounded only by the configured metadata cap (DSC-103), which may be
/// `unlimited`; this way the scan allocates nothing at all.
struct Cursor<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        // The offset is only ever advanced by whole characters, so this never
        // splits one; `get` rather than indexing keeps that a `None` instead of
        // a panic if it ever stopped being true.
        self.s.get(self.i..).unwrap_or("")
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.rest().chars().nth(offset)
    }

    /// Step over exactly one character, whatever its width. Every advance in
    /// this reader goes through here (or through a reader that ends on a
    /// boundary), which is what keeps [`Cursor::i`] on one.
    fn advance(&mut self) {
        if let Some(c) = self.peek() {
            self.i += c.len_utf8();
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if let Some(c) = c {
            self.i += c.len_utf8();
        }
        c
    }

    fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    /// Skip whitespace and both comment forms, reporting whether any of it
    /// crossed a line break (which is a statement boundary for
    /// [`Cursor::seek_meta_object`]). An unterminated `/*` runs to end of file,
    /// which is what a JavaScript tokenizer does before erroring.
    fn skip_trivia(&mut self) -> bool {
        let mut newline = false;
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    newline |= c == '\n';
                    self.advance();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.bump() {
                        if c == '\n' {
                            newline = true;
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
                        newline |= self.peek() == Some('\n');
                        self.advance();
                    }
                }
                _ => return newline,
            }
        }
    }

    /// Read an identifier, or `None` if the cursor is not on one. JavaScript
    /// identifiers may start with `$` or `_`; the exact Unicode identifier rules
    /// do not matter here, since the only identifiers this reader compares
    /// against are ASCII.
    fn read_ident(&mut self) -> Option<&'a str> {
        let start = self.i;
        match self.peek() {
            Some(c) if c.is_alphanumeric() || c == '_' || c == '$' => {}
            _ => return None,
        }
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '$' {
                self.i += c.len_utf8();
            } else {
                break;
            }
        }
        self.s.get(start..self.i)
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
        self.advance();
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
    ///
    /// spec: WF-57 -- the triple has to be at STATEMENT POSITION: the `export`
    /// must be the first token of a statement (start of file, or after `;`,
    /// `}`, or a line break) at bracket depth 0, and nothing but trivia may
    /// come between the three words. Without that, `export`, `const` and `meta`
    /// were matched as a sequence of identifiers with all punctuation between
    /// them skipped, so the member expression
    ///
    /// ```js
    /// shim.export.const.meta = { name: 'decoy' }
    /// ```
    ///
    /// matched, and matched FIRST -- the scan stops at the first hit, so a
    /// decoy like that hid the real declaration below it and mind reported a
    /// `meta.name` the harness never sees. Both halves of the rule are load
    /// bearing: `.` is what a member expression has instead of a boundary, and
    /// the depth test keeps a `meta` declared inside a block or a call from
    /// standing in for the module-level one the harness reads.
    ///
    /// spec: WF-57 -- regex literals are skipped whole, so their contents never
    /// move the bracket count or open a string. A `/` is a regex opener only in
    /// EXPRESSION-START position, the place a JavaScript tokenizer expects an
    /// operand: the start of the file, after a line break at depth 0, after one
    /// of the punctuators `= ( , : [ ! & | ? ; { } ~ + - * % < > ^ /`, a prefix
    /// `++`/`--`, or after one of the keywords `return typeof instanceof in of
    /// new delete void throw case do else yield await`. After any other
    /// identifier, a number, a string or template, `)`, `]`, or a postfix
    /// `++`/`--`, a `/` is the division operator. Inside the literal a
    /// backslash escapes the next character and a `[ ... ]` class hides a `/`;
    /// an unescaped `/` outside a class closes it and its flags are skipped. An
    /// unterminated literal stops at the line break (or end of file) that ends
    /// it. Without this, `/[{]/` left the count one bracket high for the rest
    /// of the file (hiding the real declaration), `/}/` inside a block closed
    /// the block early (exposing a nested one), and `` /`/ `` opened a template
    /// that swallowed the real declaration while a decoy spelled inside the
    /// "template" text was read as code.
    fn seek_meta_object(&mut self) -> bool {
        /// How much of `export const meta` has been matched, contiguously.
        #[derive(PartialEq)]
        enum Seen {
            Nothing,
            Export,
            ExportConst,
        }
        let mut seen = Seen::Nothing;
        // Statement position: the start of the file is one.
        let mut at_statement = true;
        // Bracket nesting. Saturating, so an unbalanced closer in a file mind
        // is not parsing cannot push the scan below zero and lock it out of
        // ever matching.
        let mut depth: usize = 0;
        // Expression-start position: whether a `/` here opens a regex literal
        // rather than dividing. The start of the file is one.
        let mut expr_start = true;
        // Whether the previous token was a `.`: the word after it is a property
        // name, never a keyword, so `obj.return / 2` still divides.
        let mut after_dot = false;
        loop {
            if self.skip_trivia() && depth == 0 {
                // A line break at depth 0 ends a statement (JavaScript's own
                // automatic semicolon insertion reads it the same way), but it
                // does not interrupt the triple: `export /* c */ const\n meta`
                // is one declaration.
                at_statement = true;
                expr_start = true;
            }
            let Some(c) = self.peek() else { return false };
            let was_dot = std::mem::take(&mut after_dot);
            if c == '\'' || c == '"' || c == '`' {
                let _ = self.read_string();
                seen = Seen::Nothing;
                at_statement = false;
                expr_start = false;
                continue;
            }
            // Comments were consumed by `skip_trivia`, so a `/` here is either
            // a regex opener or the division operator.
            if c == '/' && expr_start {
                self.skip_regex();
                seen = Seen::Nothing;
                at_statement = false;
                expr_start = false;
                continue;
            }
            // `++` / `--` leave the position as it was: prefix (at expression
            // start) is followed by an operand, postfix (after one) by an
            // operator. Read singly, the second `+` would flag a postfix
            // `a++ / b` as a regex opener.
            if (c == '+' || c == '-') && self.peek_at(1) == Some(c) {
                self.advance();
                self.advance();
                seen = Seen::Nothing;
                at_statement = false;
                continue;
            }
            let Some(word) = self.read_ident() else {
                expr_start = matches!(
                    c,
                    '=' | '('
                        | ','
                        | ':'
                        | '['
                        | '!'
                        | '&'
                        | '|'
                        | '?'
                        | ';'
                        | '{'
                        | '}'
                        | '~'
                        | '+'
                        | '-'
                        | '*'
                        | '%'
                        | '<'
                        | '>'
                        | '^'
                        | '/'
                );
                match c {
                    '(' | '[' | '{' => {
                        depth += 1;
                        at_statement = false;
                    }
                    ')' | ']' => {
                        depth = depth.saturating_sub(1);
                        at_statement = false;
                    }
                    // A closing brace ends a block or an object; either way the
                    // next token starts a statement.
                    '}' => {
                        depth = depth.saturating_sub(1);
                        at_statement = depth == 0;
                    }
                    ';' => at_statement = depth == 0,
                    _ => at_statement = false,
                }
                seen = Seen::Nothing;
                after_dot = c == '.';
                self.advance();
                continue;
            };
            // A number reads as an identifier here too, and is an operand.
            expr_start = !was_dot
                && matches!(
                    word,
                    "return"
                        | "typeof"
                        | "instanceof"
                        | "in"
                        | "of"
                        | "new"
                        | "delete"
                        | "void"
                        | "throw"
                        | "case"
                        | "do"
                        | "else"
                        | "yield"
                        | "await"
                );
            seen = match (word, &seen) {
                ("export", _) if at_statement && depth == 0 => Seen::Export,
                ("const", Seen::Export) => Seen::ExportConst,
                ("meta", Seen::ExportConst) => {
                    self.skip_trivia();
                    if self.peek() != Some('=') {
                        return false;
                    }
                    self.advance();
                    self.skip_trivia();
                    if self.peek() != Some('{') {
                        // An initializer that is not an object literal: a call,
                        // an identifier, a spread. Nothing to read, and the
                        // harness rejects it too.
                        return false;
                    }
                    self.advance();
                    return true;
                }
                _ => Seen::Nothing,
            };
            at_statement = false;
        }
    }

    /// Skip a regex literal, the cursor sitting on its opening `/` (WF-57).
    /// A backslash escapes the next character, a `[ ... ]` class hides a `/`,
    /// and an unescaped `/` outside a class closes the literal, after which its
    /// flags are skipped. An unterminated literal stops, unconsumed, at the
    /// line terminator (or end of file) that ends it, so the caller still sees
    /// the line break as a statement boundary. Every step is a whole character.
    fn skip_regex(&mut self) {
        fn line_end(c: Option<char>) -> bool {
            matches!(c, None | Some('\n' | '\r' | '\u{2028}' | '\u{2029}'))
        }
        self.advance();
        let mut in_class = false;
        loop {
            let c = self.peek();
            if line_end(c) {
                return;
            }
            match c {
                Some('\\') => {
                    self.advance();
                    if line_end(self.peek()) {
                        return;
                    }
                    self.advance();
                }
                Some('[') => {
                    in_class = true;
                    self.advance();
                }
                Some(']') => {
                    in_class = false;
                    self.advance();
                }
                Some('/') if !in_class => {
                    self.advance();
                    let _ = self.read_ident();
                    return;
                }
                _ => self.advance(),
            }
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
                    self.advance();
                    return meta;
                }
                ',' => {
                    self.advance();
                    continue;
                }
                _ => {}
            }
            let entry_start = self.i;

            // The key: bare, quoted, or computed. A computed key is read as no
            // key at all, so its value is skipped like any unrecognized one.
            let key: Option<String> = match c {
                '\'' | '"' | '`' => self.read_string(),
                '[' => {
                    self.skip_value();
                    None
                }
                // A bare key is a borrow of the text; the quoted form has to be
                // unescaped into a `String`, so the two meet as one here.
                _ => self.read_ident().map(str::to_string),
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
                    self.advance();
                }
                continue;
            }
            self.advance();
            self.skip_trivia();

            let value = match self.peek() {
                Some('\'' | '"' | '`') => {
                    let v = self.read_string();
                    // A template that turned out to be interpolated has been
                    // consumed; there is nothing left of this entry to skip.
                    // A string that is only the head of an expression
                    // (`'a' + 'b'`) is not a literal value: drop it and skip
                    // the rest of the entry.
                    if v.is_some() {
                        self.skip_trivia();
                        if matches!(self.peek(), Some(ch) if ch != ',' && ch != '}') {
                            self.skip_value();
                            None
                        } else {
                            v
                        }
                    } else {
                        v
                    }
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
                    self.advance();
                }
                '}' | ']' | ')' => {
                    if depth == 0 {
                        // The object's own closing brace: the caller handles it.
                        return;
                    }
                    depth -= 1;
                    self.advance();
                }
                ',' if depth == 0 => return,
                _ => self.advance(),
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

    // spec: WF-5 -- a string that begins an expression (`'a' + 'b'`) is not a
    // literal value, so that key is absent and its siblings still read.
    #[test]
    fn a_concatenated_string_is_not_a_value() {
        let m = meta(
            "export const meta = { name: 'a' + 'b', description: 'ok', whenToUse: 'x' /* c */ }",
        );
        assert_eq!(m.name, None);
        assert_eq!(m.description.as_deref(), Some("ok"));
        assert_eq!(m.when_to_use.as_deref(), Some("x"));
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

    // spec: WF-57 -- the triple is matched at statement position only, so a
    // member expression spelling the same three identifiers with `.` between
    // them is not a declaration. Before this rule the decoy matched (all
    // punctuation between the identifiers was skipped) AND matched first, so
    // the real declaration below it was never reached: mind reported the name
    // the decoy chose while the harness registered the real one.
    #[test]
    fn a_member_expression_spelling_the_triple_is_not_a_declaration() {
        let m = meta(
            "shim.export.const.meta = { name: 'decoy', description: 'Decoy' };\n\
             export const meta = { name: 'real', description: 'Real' }\n",
        );
        assert_eq!(m.name.as_deref(), Some("real"));
        assert_eq!(m.description.as_deref(), Some("Real"));
    }

    // spec: WF-57 -- the same rule over the shapes that are NOT a module-level
    // declaration: a member expression on its own, one nested in a call or a
    // block, and a `meta` declared inside a function body. None of them is the
    // `export const meta` the harness reads, so none of them is read here.
    #[test]
    fn only_a_statement_position_declaration_is_read() {
        for text in [
            "shim.export.const.meta = { name: 'decoy' }",
            "x = obj.export.const.meta = { name: 'decoy' }",
            "register(export.const.meta = { name: 'decoy' })",
            "function f() { export const meta = { name: 'decoy' } }",
            "if (x) { export const meta = { name: 'decoy' } }",
            "const holder = [export, const, meta = { name: 'decoy' }]",
        ] {
            let m = parse_bounded(text);
            assert!(m.is_empty(), "expected nothing from {text:?}, got {m:?}");
        }
    }

    // spec: WF-57 -- and the statement-position rule admits every ordinary
    // spelling of the real declaration: first in the file, after a `;`, after
    // a closing brace, after a line break, and with comments in between.
    #[test]
    fn the_real_declaration_is_found_at_every_statement_boundary() {
        for text in [
            "export const meta = { name: 'real' }",
            "import x from 'y';export const meta = { name: 'real' }",
            "function f() { return 1 } export const meta = { name: 'real' }",
            "const a = 1\nexport const meta = { name: 'real' }",
            "// leading\n/* block */\nexport /* mid */ const // eol\n meta = { name: 'real' }",
            "const obj = { a: 1 };\n\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- the statement-position rule has to admit the shapes a real
    // `.js` file arrives in, not only the hand-written one. Minified output has
    // no line breaks at all, so the `;` is the only boundary there is; a bundled
    // file opens and closes a prologue before the declaration; and an ESM import
    // list is a `{ ... }` the depth count has to come back out of.
    #[test]
    fn a_minified_or_bundled_file_still_yields_its_declaration() {
        for text in [
            // No whitespace anywhere: the `;` after the directive is the only
            // statement boundary in the file.
            "\"use strict\";export const meta={name:'min',description:'Minified'};\
             export async function run(){await agent('go')}",
            // A balanced IIFE prologue, as a bundler emits.
            "(()=>{const x=1;})();export const meta={name:'min',description:'Minified'}",
            // A named import list: braces that open and close before the
            // declaration.
            "import { agent, phase } from 'harness';\n\
             export const meta = { name: 'min', description: 'Minified' }",
            // A preceding object literal with nested brackets.
            "const cfg={retries:3,nested:{a:[1,2]}};\
             export const meta={name:'min',description:'Minified'}",
            // A preceding function body with a nested block and object.
            "function helper(){ if (true) { return {a:1} } }\n\
             export const meta={name:'min',description:'Minified'}",
            // A regex literal whose brackets happen to balance, and a comment
            // holding an unbalanced one (comments are trivia, never depth).
            "const re = /[{]}/;\n// a stray { in prose\n/* and a } in a block */\n\
             export const meta={name:'min',description:'Minified'}",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("min"), "from {text:?}");
            assert_eq!(m.description.as_deref(), Some("Minified"), "from {text:?}");
        }
    }

    // spec: WF-5 WF-57 -- the depth-0 half of the statement-position rule is a
    // bracket count. A `{` or `[` the count never sees closed leaves it above
    // zero for the rest of the file, so a declaration after it is not at depth
    // 0 as far as the scan is concerned and reads as absent.
    //
    // That is a FALSE NEGATIVE, and it is the acceptable direction: mind reports
    // the file as one it read no `meta` from (WF-30), the harness still loads it,
    // and WF-5 forbids only failing on the shape of a workflow's code. The
    // opposite error -- mind reading a `meta` the harness does not -- is what
    // WF-57 exists to prevent, and is the one the depth test buys.
    #[test]
    fn an_unclosed_bracket_before_the_declaration_hides_it_rather_than_faking_one() {
        for text in [
            "function f() {\nexport const meta = { name: 'real' }",
            "const open = [\nexport const meta = { name: 'real' }",
            "call(\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert!(m.is_empty(), "expected nothing from {text:?}, got {m:?}");
        }
    }

    // spec: WF-57 -- the property that matters, over every prologue shape worth
    // naming: whatever a source puts in front of the real declaration, the
    // reader finds the real one or finds nothing. It must never find a DECOY,
    // because what it reads as `meta.name` is what WF-24 compares, what WF-29
    // groups by, and what `recall` prints as the harness-facing name.
    #[test]
    fn no_prologue_makes_the_reader_prefer_a_decoy() {
        for prologue in [
            "shim.export.const.meta = { name: 'decoy' };",
            "x = obj.export.const.meta = { name: 'decoy' };",
            "obj[export][const][meta] = { name: 'decoy' };",
            "register(export.const.meta = { name: 'decoy' });",
            "register({ export: { const: { meta: { name: 'decoy' } } } });",
            "x = { 'export const meta': { name: 'decoy' } };",
            "if (x) { export const meta = { name: 'decoy' } }",
            "function f() { export const meta = { name: 'decoy' } }",
            "const s = \"export const meta = { name: 'decoy' }\";",
            "const t = `export const meta = { name: 'decoy' }`;",
            "// export const meta = { name: 'decoy' }",
            "/* export const meta = { name: 'decoy' } */",
            "export default { name: 'decoy' };",
            "export const metadata = { name: 'decoy' };",
            "export const meta2 = { name: 'decoy' };",
            "export function meta() { return { name: 'decoy' } }",
            "export let meta = { name: 'decoy' };",
            "export const metaX = 1, meta = { name: 'decoy' };",
            "export const { meta } = require('x');",
            "export\n.const.meta = { name: 'decoy' };",
            "export const\n.meta = { name: 'decoy' };",
            "shim\n.export.const.meta = { name: 'decoy' };",
            "const q = /`/\nconst t = `\nexport const meta = { name: 'decoy' }\n`;",
            "const q = /\\/\\// + `\nexport const meta = { name: 'decoy' }\n`;",
        ] {
            let text = format!("{prologue}\nexport const meta = {{ name: 'real' }}\n");
            let m = parse_bounded(&text);
            assert_ne!(
                m.name.as_deref(),
                Some("decoy"),
                "a decoy won after {prologue:?}"
            );
            assert!(
                m.name.as_deref() == Some("real") || m.is_empty(),
                "after {prologue:?} the reader must find the real name or nothing: {m:?}"
            );
        }
    }

    // spec: WF-57 -- a regex literal in expression-start position is skipped
    // whole, so a `}` inside one no longer closes, for this scanner, a block
    // the file has not closed. Before the regex skip this read `nested`: the
    // `/}/` brought the count back to 0 and the `meta` declared inside the
    // function looked module-level. Now the block stays open until its own
    // `}`, the nested declaration is at depth 1 and ignored, and the real one
    // below the block is read.
    #[test]
    fn a_regex_closer_inside_a_block_does_not_close_the_block() {
        let m = parse_bounded(
            "function f() {\n  const re = /}/\n  export const meta = { name: 'nested' }\n}\n\
             export const meta = { name: 'real' }\n",
        );
        assert_eq!(m.name.as_deref(), Some("real"));
    }

    // spec: WF-57 -- a regex character class holding a bracket is skipped with
    // the literal, so the count is not left one high for the rest of the file.
    // Before the regex skip `[` and `{` both counted up, `]` brought back only
    // one of them, and the declaration below read as absent.
    #[test]
    fn a_bracket_in_a_regex_class_does_not_hide_the_declaration() {
        for text in [
            "const re = /[{]/\nexport const meta = { name: 'real' }",
            "const re = /[/{]/g\nexport const meta = { name: 'real' }",
            "const re = /\\{/\nexport const meta = { name: 'real' }",
            "if (ok) { return /[(]/.test(x) }\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- a regex literal holding a quote character does not open a
    // string or template. Before the regex skip the backtick in `/`/` opened a
    // template that ran to the next backtick, so the "template" below (whose
    // text spells a decoy declaration) was read as code and the decoy won. The
    // generic prologue property allows "nothing"; these two must read `real`.
    #[test]
    fn a_quote_inside_a_regex_literal_does_not_let_a_decoy_win() {
        for text in [
            "const q = /`/\nconst t = `\nexport const meta = { name: 'decoy' }\n`;\n\
             export const meta = { name: 'real' }\n",
            "const q = /\\/\\// + `\nexport const meta = { name: 'decoy' }\n`;\n\
             export const meta = { name: 'real' }\n",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- a `/` after an operand is the division operator, not a
    // regex opener: after an identifier, a number, `)` or `]`, and a postfix
    // `++`/`--`. Read as regex openers, each of these would swallow the rest
    // of its line, and in the `'/'` cases the quote after it, which then
    // opens a string that hides the declaration.
    #[test]
    fn division_is_not_read_as_a_regex_literal() {
        for text in [
            "const x = a / b / c\nexport const meta = { name: 'real' }",
            "const x = (a) / 2 // c\nexport const meta = { name: 'real' }",
            "const x = a[0] / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = 10 / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = a++ / b; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = a-- / b; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = 'a' / b; const y = '/'\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- the punctuators outside the expression-start set leave the
    // position false, so a `/` after them divides: `.` (a number like `1.`),
    // `@` (a decorator), `#` (a private name), and a non-ASCII character that is
    // not an identifier character. Read as a regex opener, each would swallow
    // `/ 2; y = '/` and flip the parity of the quote after it, hiding the
    // declaration.
    #[test]
    fn division_after_dot_at_hash_or_non_ascii_is_not_a_regex() {
        for text in [
            "const x = 1. / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = @dec / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = # / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = this.#a / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = ✓ / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = 🚀 / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = 日本 / 2; const y = '/'\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- an identifier that merely STARTS with (or ends in, or
    // wraps) an operand-expecting keyword is an ordinary identifier: the word is
    // read whole, so `returnValue / x` divides.
    #[test]
    fn a_keyword_prefixed_identifier_is_an_operand() {
        for word in [
            "returnValue",
            "typeofx",
            "instanceofx",
            "index",
            "offset",
            "newer",
            "deleted",
            "voidable",
            "throwable",
            "casey",
            "double",
            "elsewhere",
            "yielded",
            "awaiting",
            "$return",
            "_in",
            "return$",
            "return2",
        ] {
            let text = format!(
                "const x = {word} / 2; const y = '/'\nexport const meta = {{ name: 'real' }}"
            );
            let m = parse_bounded(&text);
            assert_eq!(m.name.as_deref(), Some("real"), "after {word:?}");
        }
    }

    // spec: WF-57 -- a keyword spelled as a PROPERTY name (after `.`) is not a
    // keyword: `obj.return / 2` divides. Without the after-dot rule the word
    // `return` set expression-start and the `/` swallowed `/ 2; y = '/`.
    #[test]
    fn a_keyword_after_a_dot_is_a_property_name() {
        for kw in [
            "return", "typeof", "in", "of", "new", "delete", "void", "case", "await",
        ] {
            for dot in [".", "?."] {
                let text = format!(
                    "const x = obj{dot}{kw} / 2; const y = '/'\nexport const meta = {{ name: 'real' }}"
                );
                let m = parse_bounded(&text);
                assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
            }
        }
        // The dot only governs the word right after it: a keyword later on
        // still opens a regex.
        let m = parse_bounded("x = a.b; return /[}']/\nexport const meta = { name: 'real' }");
        assert_eq!(m.name.as_deref(), Some("real"));
    }

    // spec: WF-57 -- every punctuator of the expression-start set puts a `/`
    // that follows it in regex position, so the class `[}']` (a closer and a
    // quote) is skipped with the literal. Read as division, the `}` would close
    // the function body early or the `'` would open a string.
    #[test]
    fn a_regex_after_each_expression_start_punctuator_is_skipped() {
        for (open, close) in [
            ("=", ""),
            ("(", ")"),
            (",", ""),
            (":", ""),
            ("[", "]"),
            ("!", ""),
            ("&", ""),
            ("&&", ""),
            ("|", ""),
            ("||", ""),
            ("?", ""),
            (";", ""),
            ("{", "}"),
            ("~", ""),
            ("+", ""),
            ("-", ""),
            ("*", ""),
            ("%", ""),
            ("<", ""),
            (">", ""),
            ("=>", ""),
            ("^", ""),
            ("/", ""),
        ] {
            let text = format!(
                "function f() {{ x {open} /[}}']/ {close} }}\nexport const meta = {{ name: 'real' }}\n"
            );
            let m = parse_bounded(&text);
            assert_eq!(m.name.as_deref(), Some("real"), "after {open:?}");
        }
    }

    // spec: WF-57 -- `)` and `]` end an operand, so a `/` after them divides.
    #[test]
    fn division_after_a_closing_bracket_stays_division() {
        for text in [
            "const x = f(a) / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = f(a)[0] / 2; const y = '/'\nexport const meta = { name: 'real' }",
            "const x = a[i] / b[j] / c; const y = '/'\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
    }

    // spec: WF-57 -- a `/` right after `}` is ALWAYS read as a regex opener: the
    // scanner cannot tell a block (`}` then a regex) from an object literal
    // (`}` then division) without a parser. That is a known, accepted
    // misreading of `x = {} / 2`, pinned here so a change is deliberate. For the
    // ordinary shapes it errs toward a false negative: an unterminated "regex"
    // stops at the line end, so nothing is lost when the division is alone on
    // its line; when a later `'/'` on the same line lets the "regex" close, the
    // quote parity flips and the declaration reads as absent (WF-30), not as
    // something else. A deliberately adversarial file can still steer the
    // flipped parity, as it can steer the line-break rule; closing that takes a
    // JavaScript parser.
    #[test]
    fn a_slash_after_a_closing_brace_is_read_as_a_regex_opener() {
        // Division alone on its line: the "regex" is unterminated and ends at
        // the line break, so the real declaration is found.
        let m = parse_bounded("const x = {} / 2\nexport const meta = { name: 'real' }");
        assert_eq!(m.name.as_deref(), Some("real"));
        // A regex after a block closer on the same line is skipped correctly.
        let m = parse_bounded("if (a) { b() } /[{]/.test(c)\nexport const meta = { name: 'real' }");
        assert_eq!(m.name.as_deref(), Some("real"));
        // A closing `/` on the line lets the misread literal end early; the
        // pinned result is "nothing", not a wrong name.
        let m =
            parse_bounded("const x = {} / 2; const y = '/'\nexport const meta = { name: 'real' }");
        assert!(m.is_empty(), "pinned misread of `{{}} / 2`: {m:?}");
    }

    // spec: WF-57 -- the keyword half of expression-start position: after
    // `return`, `typeof`, and the other operand-expecting keywords a `/` opens
    // a regex, so its brackets and quotes are skipped with it. And a prefix
    // `++`/`--` keeps the position it was in.
    #[test]
    fn a_regex_after_an_operand_expecting_keyword_is_skipped() {
        for kw in [
            "return",
            "typeof",
            "instanceof",
            "in",
            "of",
            "new",
            "delete",
            "void",
            "throw",
            "case",
            "do",
            "else",
            "yield",
            "await",
        ] {
            let text = format!(
                "function f() {{ {kw} /[}}']/ }}\nexport const meta = {{ name: 'real' }}\n"
            );
            let m = parse_bounded(&text);
            assert_eq!(m.name.as_deref(), Some("real"), "after {kw:?}");
        }
        let m = parse_bounded("x = ++/[{]/.lastIndex\nexport const meta = { name: 'real' }");
        assert_eq!(m.name.as_deref(), Some("real"));
    }

    // spec: WF-5 WF-57 -- an unterminated regex literal stops at the line
    // break (or end of file) that ends it, without stalling or panicking, and
    // the line break still counts as a statement boundary. Escapes and classes
    // that run into the line end are unterminated the same way, and multi-byte
    // text inside a literal is stepped a whole character at a time.
    #[test]
    fn an_unterminated_regex_literal_stops_at_the_line_end() {
        for text in [
            "const re = /abc\nexport const meta = { name: 'real' }",
            "const re = /[abc\nexport const meta = { name: 'real' }",
            "const re = /abc\\\nexport const meta = { name: 'real' }",
            "const re = /[/]\\/🚀✓/u\nexport const meta = { name: 'real' }",
            "const re = /説明\r\nexport const meta = { name: 'real' }",
        ] {
            let m = parse_bounded(text);
            assert_eq!(m.name.as_deref(), Some("real"), "from {text:?}");
        }
        for text in [
            "/",
            "x = /",
            "x = /[",
            "x = /\\",
            "x = /🚀",
            "x = /a/gimsuy",
        ] {
            assert!(parse_bounded(text).is_empty(), "from {text:?}");
        }
    }

    // spec: WF-5 -- `skip_trivia`'s block-comment branch is the one place the
    // reader adds a fixed 2 to the offset, so it is the one place a multi-byte
    // character could be split. Every step inside a comment goes through
    // `advance`, and the `*/` test is a character comparison, not a byte one: a
    // byte-indexed regression panics here (a split code point) rather than
    // returning a wrong value.
    #[test]
    fn a_block_comment_of_multibyte_text_is_skipped_whole() {
        let m = parse_bounded(
            "/* 説明: ✓ * ✓ / 🚀 */\nexport /* ✓*✓/✓ */ const meta = {\n  \
             /* 名前 */ name: 'ünïcode',\n  description: '✓ 説明 🚀',\n}\n",
        );
        assert_eq!(m.name.as_deref(), Some("ünïcode"));
        assert_eq!(m.description.as_deref(), Some("✓ 説明 🚀"));
        // A comment whose last character before the closer is multi-byte.
        assert_eq!(
            parse_bounded("export const meta = { name: 'x' /* 🚀*/ }")
                .name
                .as_deref(),
            Some("x")
        );
        // An unterminated comment of multi-byte text runs to end of file, and
        // ending mid-character is not a place the cursor can stop.
        assert!(parse_bounded("export const meta = { /* 説明 🚀 ✓").is_empty());
        assert!(parse_bounded("/* 🚀").is_empty());
        // A line comment of multi-byte text, terminated and not.
        assert_eq!(
            parse_bounded("// 説明 🚀\nexport const meta = { name: 'x' }")
                .name
                .as_deref(),
            Some("x")
        );
        assert!(parse_bounded("export const meta = { // 説明 🚀").is_empty());
        // And a multi-byte character where a `/` could have started a comment.
        assert_eq!(
            parse_bounded("const div = a /✓/ b\nexport const meta = { name: 'x' }")
                .name
                .as_deref(),
            Some("x")
        );
    }

    // spec: WF-5 -- an initializer the harness would not accept as an object
    // literal yields nothing here too, including the JSDoc cast form a `.js`
    // file reaches for when it wants a type without TypeScript. The reader and
    // the harness agree: neither reads a `meta` out of it.
    #[test]
    fn an_initializer_that_is_not_an_object_literal_yields_nothing() {
        for text in [
            "export const meta = /** @type {Meta} */ ({ name: 'x' })",
            "export const meta = Object.freeze({ name: 'x' })",
            "export const meta = base",
            "export const meta: Meta = { name: 'x' }",
        ] {
            let m = parse_bounded(text);
            assert!(
                m.name.is_none(),
                "expected no name from {text:?}, got {m:?}"
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

    // spec: WF-5 WF-63 -- a stray closer after a string means the string did not
    // end its entry, so that key yields nothing (WF-63); the key after the
    // closer is still read.
    #[test]
    fn a_dropped_closer_costs_only_its_own_entry() {
        let m = parse_bounded("export const meta = { name: 'a' ) description: 'b' }");
        assert_eq!(m.name, None);
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
    ///
    /// The returned offset is a BYTE offset into `text` (the cursor walks the
    /// `&str` itself rather than a `Vec<char>`), which equals the character
    /// count for the ASCII inputs below.
    fn read_string_at(text: &str) -> (Option<String>, usize) {
        let mut cur = Cursor { s: text, i: 0 };
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

    // spec: WF-5 WF-57 -- a randomized sweep over the tokens an object literal is
    // made of. The reader must return on every arrangement of them, balanced or
    // not; the seed is fixed, so any failure reproduces exactly. This is the
    // general form of the hang: a character that no reader in the loop will
    // consume, reached in a position nobody wrote a case for.
    #[test]
    fn a_randomized_sweep_of_object_bodies_always_returns() {
        const TOKENS: [&str; 41] = [
            // Regex-shaped and expression-position tokens (WF-57).
            "/[",
            "\\/",
            "/}/",
            "/[{]/g",
            "return",
            "=",
            "++",
            "--",
            ".",
            "#",
            "@",
            "✓",
            "🚀",
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

    // spec: WF-5 -- the other loop in this reader. `seek_meta_object` scans the
    // whole file before the object exists, so a shape it will not consume is the
    // same class of hang one phase earlier, reached on any `.js` file in the
    // source whether or not it declares a `meta` at all.
    #[test]
    fn every_prologue_before_the_object_terminates() {
        for text in [
            ")",
            "]",
            "}",
            "$",
            "${",
            "\\",
            "//",
            "/*",
            "*/",
            "/",
            "export",
            "export const",
            "export const meta",
            "export const meta =",
            "export const meta =/*",
            "export const meta = `",
            "export const meta = '",
            "export const meta = \"",
            "export const meta /* c */ =",
            "export const const meta = {",
            "export meta const = {",
            "export const meta = { } )",
            "`export const meta = { name: 'in a template' }`",
            "\u{0}export\u{0}const\u{0}meta\u{0}=\u{0}{",
            "export const meta\u{FEFF}= {",
            "export const metameta = { name: 'x' }",
            "exportconstmeta = { name: 'x' }",
        ] {
            let m = parse_bounded(text);
            assert!(m.is_empty(), "expected nothing from {text:?}, got {m:?}");
        }
        // The seek drops what it cannot consume rather than giving up, so a real
        // declaration behind a run of junk (quoted closers, a stray `)`) is
        // still found.
        let m = parse_bounded("')))'\n)\nexport const meta = { name: 'found anyway' }");
        assert_eq!(m.name.as_deref(), Some("found anyway"));
    }

    // spec: WF-5 -- the cursor walks `char`s, not bytes, so a multi-byte
    // character is one step of the drop-and-carry-on path and a value keeps
    // every code point it was written with. A byte-indexed regression truncates
    // these values or splits one of them mid-character.
    #[test]
    fn multibyte_text_is_read_and_skipped_one_character_at_a_time() {
        let m = parse_bounded(
            "export const meta = { 🚀: 1, name: 'ünïcode 🚀 ✓', description: '日本語の説明' }",
        );
        assert_eq!(m.name.as_deref(), Some("ünïcode 🚀 ✓"));
        assert_eq!(m.description.as_deref(), Some("日本語の説明"));
        // A stray closer wedged between multi-byte characters is still dropped
        // by itself, and the key after it still reads.
        let m = parse_bounded("export const meta = { 🚀 ) ✓ ] name: 'x', whenToUse: '✔' }");
        assert_eq!(m.name.as_deref(), Some("x"));
        assert_eq!(m.when_to_use.as_deref(), Some("✔"));
    }

    // spec: WF-5 -- `file_meta` is the entry point `workflow_check` uses (the
    // `review`/`recall`/`learn` reports), so the termination guarantee has to
    // hold through the on-disk path too, not only through `parse`.
    #[test]
    fn file_meta_returns_on_a_shape_the_reader_cannot_model() {
        let dir = std::env::temp_dir().join(format!("mind-wfmeta-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("deploy.js");
        std::fs::write(
            &file,
            "export const meta = {\n  name: 'deploy',\n  tag: /rel)ease/,\n  arrow: () => x),\n  \
             [)]: 1,\n  description: 'Deploy a release',\n}\nphase('Deploy')\n",
        )
        .unwrap();
        let read = file.clone();
        let m = run_bounded("file_meta", 10, move || file_meta(&read).unwrap());
        assert_eq!(m.name.as_deref(), Some("deploy"));
        assert_eq!(m.description.as_deref(), Some("Deploy a release"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: WF-5 -- dropping a stray closer costs one pass of the loop, so a
    // body that is mostly strays is linear in its length. A regression that
    // rescans from the top of the object each time still terminates, which the
    // other checks here would not catch; at this size it does not finish.
    #[test]
    fn a_body_that_is_mostly_strays_stays_linear() {
        let text = format!(
            "export const meta = {{ {}name: 'survivor' }}",
            ")] , ".repeat(40_000)
        );
        let m = run_bounded("a 200k-character malformed body", 20, move || parse(&text));
        assert_eq!(m.name.as_deref(), Some("survivor"));
    }

    // spec: WF-5 -- nesting is counted, not recursed into, so a file that opens
    // a hundred thousand brackets and closes none reads as nothing instead of
    // overflowing the stack. The worker thread this runs on has the default
    // stack, so a recursive rewrite of `skip_value` fails here.
    #[test]
    fn unbounded_nesting_yields_nothing_rather_than_overflowing() {
        let text = format!(
            "export const meta = {{ a: {}, name: 'never reached' }}",
            "[".repeat(100_000)
        );
        let m = run_bounded("a deeply nested value", 20, move || parse(&text));
        assert!(m.is_empty(), "the open bracket swallows the rest: {m:?}");
    }
}
