//! Source namespacing: prefixing every item from a source, and rewriting the
//! intra-source references that prefixing would otherwise break.
//!
//! A source's *effective prefix* is the consumer's `--as` alias if set, else the
//! `[source].prefix` declared in its `mind.toml`, else none. When a prefix `p`
//! is in effect, item `name` installs as `p-name` (identity, symlink, ref).
//!
//! References between items in the same source must be written as `{{ns:name}}`
//! tokens so they survive prefixing. [`expand`] rewrites each token to the
//! effective name (`name` when unprefixed, `p-name` when prefixed) and validates
//! that the referent is a real sibling. Sources that instead reference siblings
//! in bare prose can be detected with [`unguarded_refs`].

use std::collections::HashSet;

/// Apply an effective prefix to a bare item name.
pub fn apply(bare: &str, prefix: &Option<String>) -> String {
    match prefix {
        Some(p) => format!("{p}-{bare}"),
        None => bare.to_string(),
    }
}

/// Expand every `{{ns:name}}` token in `content` to its effective name.
///
/// Returns `Err(name)` if a token names something that is not a sibling, so the
/// caller can report the typo. Sources with no tokens pass through unchanged.
pub fn expand(
    content: &str,
    prefix: &Option<String>,
    siblings: &HashSet<String>,
) -> Result<String, String> {
    const OPEN: &str = "{{ns:";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + OPEN.len()..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: leave the rest verbatim.
            out.push_str(&rest[pos..]);
            return Ok(out);
        };
        let name = after[..end].trim();
        if !siblings.contains(name) {
            return Err(name.to_string());
        }
        out.push_str(&apply(name, prefix));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Find sibling names referenced in bare prose (outside any `{{ns:}}` token).
///
/// Heuristic and advisory: used to warn when a source is about to be prefixed
/// but references siblings without the token that would keep them resolvable.
pub fn unguarded_refs(content: &str, siblings: &HashSet<String>) -> Vec<String> {
    let stripped = strip_tokens(content);
    let mut found: Vec<String> = siblings
        .iter()
        .filter(|name| whole_word_present(&stripped, name))
        .cloned()
        .collect();
    found.sort();
    found
}

/// Replace `{{ns:...}}` spans with a space so prose scanning ignores them.
fn strip_tokens(content: &str) -> String {
    const OPEN: &str = "{{ns:";
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + OPEN.len()..];
        match after.find("}}") {
            Some(end) => {
                out.push(' ');
                rest = &after[end + 2..];
            }
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn whole_word_present(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(i) = haystack[start..].find(needle) {
        let idx = start + i;
        let before = haystack[..idx].chars().next_back();
        let after = haystack[idx + needle.len()..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return true;
        }
        start = idx + needle.len();
    }
    false
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

#[cfg(test)]
mod tests {
    // spec: NS-2, NS-11, NS-12, NS-13, NS-14, NS-20, NS-21
    use super::*;

    fn sibs(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn apply_prefixes_or_passes_through() {
        assert_eq!(apply("review", &Some("jk".into())), "jk-review");
        assert_eq!(apply("review", &None), "review");
    }

    #[test]
    fn expand_unprefixed_yields_bare_names() {
        let s = sibs(&["test"]);
        let got = expand("hand off to {{ns:test}} now", &None, &s).unwrap();
        assert_eq!(got, "hand off to test now");
    }

    #[test]
    fn expand_prefixed_yields_prefixed_names() {
        let s = sibs(&["test"]);
        let got = expand("see {{ns:test}}.", &Some("jk".into()), &s).unwrap();
        assert_eq!(got, "see jk-test.");
    }

    #[test]
    fn expand_rejects_unknown_referent() {
        let s = sibs(&["test"]);
        assert_eq!(expand("{{ns:nope}}", &None, &s), Err("nope".to_string()));
    }

    #[test]
    fn expand_passes_content_without_tokens() {
        let s = sibs(&["test"]);
        assert_eq!(
            expand("no tokens here", &None, &s).unwrap(),
            "no tokens here"
        );
    }

    #[test]
    fn unguarded_finds_bare_prose_refs_only() {
        let s = sibs(&["test", "planner"]);
        // 'test' appears in prose; 'planner' only inside a token (guarded).
        let refs = unguarded_refs("run the test, then {{ns:planner}}", &s);
        assert_eq!(refs, vec!["test".to_string()]);
    }

    #[test]
    fn unguarded_respects_word_boundaries() {
        let s = sibs(&["do"]);
        // "doing" must not match the sibling "do".
        assert!(unguarded_refs("doing work", &s).is_empty());
        assert_eq!(unguarded_refs("just do it", &s), vec!["do".to_string()]);
    }
}
