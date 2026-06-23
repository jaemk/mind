//! Minimal YAML-frontmatter reader.
//!
//! Items already carry metadata in a leading `--- ... ---` block (skills in
//! `SKILL.md`, agents/rules in their `.md`). We only need a few top-level
//! string keys (today: `description`), so rather than pull in a full YAML
//! parser we scan the block for `key:` lines at column zero.
//!
//! Limitations (documented on purpose): only top-level scalar keys are read;
//! block scalars (`description: |`) and flow collections are not interpreted.

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
    let mut lines = text.lines();
    // The very first line must be the opening delimiter.
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break; // end of frontmatter
        }
        if let Some(rest) = line.strip_prefix(key)
            && let Some(value) = rest.strip_prefix(':')
        {
            return Some(unquote(value.trim()));
        }
    }
    None
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
    // spec: DSC-20, DSC-21
    use super::*;

    #[test]
    fn reads_plain_description() {
        let t = "---\nname: review\ndescription: Review the diff\n---\n# body\n";
        assert_eq!(field(t, "description").as_deref(), Some("Review the diff"));
    }

    #[test]
    fn strips_quotes() {
        let t = "---\ndescription: \"quoted value\"\n---\n";
        assert_eq!(field(t, "description").as_deref(), Some("quoted value"));
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
}
