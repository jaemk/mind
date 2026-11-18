//! Textual scans over the crate's own Rust sources, shared by the source
//! gates (`spec_coverage.rs`, `settings_reads.rs`).

use std::path::{Path, PathBuf};

/// Byte offset of the first `#[cfg(test)]` attribute that gates a `mod` (a test
/// module), or `None` if the file has none. A `#[cfg(test)]` on a non-module
/// item (e.g. a test-only accessor method) is skipped so a mid-file test helper
/// does not pull the production `// spec:` comments after it into the citation
/// region.
///
/// Between the attribute and the `mod` keyword, further attributes (e.g.
/// `#[allow(..)]`) and `//`/`///` line comments are skipped, and any
/// visibility (`pub`, `pub(crate)`, `pub(in ...)`, ...) before `mod` is
/// accepted. The `mod` keyword itself may be followed by any whitespace,
/// including a newline (`mod\ntests`).
///
/// This is a textual scan, not a real parser: an occurrence of the literal
/// `#[cfg(test)]` text inside a `//`/`///` comment is explicitly excluded (by
/// checking whether the line up to the attribute is itself a comment line), but
/// an occurrence inside a block comment (`/* ... */`) or a string literal is
/// not detected and could still be misread as a real attribute. That is an
/// accepted limitation given the repo's style (block comments and string
/// literals containing this exact attribute text are not in use); if one is
/// ever added, this function must be hardened further rather than trusted
/// blindly.
///
/// If a real test module is ever hidden behind a form this function does not
/// recognize, this returns `None` for that file and its citations stop
/// counting. That is loud ONLY when every ID cited from that module has no
/// other citation and no allowlist entry -- see the caller's note on why a
/// missed module is usually a silent narrowing, not a build failure.
pub fn find_test_module(text: &str) -> Option<usize> {
    const ATTR: &str = "#[cfg(test)]";

    fn line_start(text: &str, at: usize) -> usize {
        text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    // Skip a `#[...]` attribute (balanced on `[`/`]`) starting at `s[0..]`,
    // returning the byte length consumed, or `None` if `s` doesn't start with
    // one or it's unterminated.
    fn skip_attr(s: &str) -> Option<usize> {
        if !s.starts_with("#[") {
            return None;
        }
        let bytes = s.as_bytes();
        let mut depth = 0i32;
        let mut j = 0;
        while j < bytes.len() {
            match bytes[j] {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j + 1);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        None
    }

    // Skip a `//` or `///` line comment starting at `s[0..]`, returning the
    // byte length consumed (up to and including the newline, if any), or
    // `None` if `s` doesn't start with one.
    fn skip_line_comment(s: &str) -> Option<usize> {
        if !s.starts_with("//") {
            return None;
        }
        Some(match s.find('\n') {
            Some(nl) => nl + 1,
            None => s.len(),
        })
    }

    let mut from = 0;
    'outer: while let Some(rel) = text[from..].find(ATTR) {
        let at = from + rel;

        // An occurrence whose own line is itself a comment line (e.g. a doc
        // comment reading "Test-only (`#[cfg(test)]`): ...") is prose, not a
        // real attribute -- skip it entirely rather than risk it gating a
        // `mod` that follows later in the same comment block.
        let prefix = text[line_start(text, at)..at].trim_start();
        if prefix.starts_with("//") {
            from = at + ATTR.len();
            continue;
        }

        // Walk past whitespace, further attributes, and line comments until
        // we hit real code.
        let mut i = at + ATTR.len();
        loop {
            let rest = &text[i..];
            let trimmed = rest.trim_start();
            i += rest.len() - trimmed.len();
            let rest = &text[i..];
            if let Some(n) = skip_attr(rest) {
                i += n;
                continue;
            }
            if let Some(n) = skip_line_comment(rest) {
                i += n;
                continue;
            }
            break;
        }

        let mut after = text[i..].trim_start();
        if let Some(rest) = after.strip_prefix("pub") {
            after = match rest.strip_prefix('(') {
                Some(rest) => match rest.find(')') {
                    Some(p) => rest[p + 1..].trim_start(),
                    None => {
                        // Unterminated `pub(...`: not a real visibility
                        // modifier we understand. Try the next occurrence.
                        from = at + ATTR.len();
                        continue 'outer;
                    }
                },
                None => rest.trim_start(),
            };
        }
        if after
            .strip_prefix("mod")
            .is_some_and(|r| r.starts_with(char::is_whitespace))
        {
            return Some(at);
        }
        from = at + ATTR.len();
    }
    None
}

/// Every file under `dir` (recursively) with extension `ext`.
pub fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_with_ext(&path, ext));
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
    out
}
