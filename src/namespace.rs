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

/// Apply an effective prefix to a bare item name. An empty prefix is treated as
/// no prefix (the "no prefix" override; see [`prefix_choice`]).
pub fn apply(bare: &str, prefix: &Option<String>) -> String {
    match prefix {
        Some(p) if !p.is_empty() => format!("{p}-{bare}"),
        _ => bare.to_string(),
    }
}

/// Whether `c` is part of an item-name word (alphanumerics plus `-`/`_`), used
/// for whole-word matching when templating bare references.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// Rewrite bare whole-word sibling mentions in `content` into `{{ns:name}}`
/// tokens, skipping any text already inside a `{{ns:}}` token (INIT-5). Returns
/// the new content and the number of replacements made. Heuristic: a sibling
/// name that is also an ordinary word will be wrapped, so callers (init-source)
/// keep this opt-in and reviewable.
pub fn templatize(content: &str, siblings: &HashSet<String>) -> (String, usize) {
    const OPEN: &str = "{{ns:";
    let mut out = String::with_capacity(content.len());
    let mut count = 0;
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        let (rep, n) = wrap_bare_words(&rest[..pos], siblings);
        out.push_str(&rep);
        count += n;
        // Copy the token span verbatim (do not re-wrap inside it).
        let after = &rest[pos + OPEN.len()..];
        match after.find("}}") {
            Some(end) => {
                let token_end = pos + OPEN.len() + end + 2;
                out.push_str(&rest[pos..token_end]);
                rest = &rest[token_end..];
            }
            None => {
                // Unterminated token: copy the remainder verbatim and stop.
                out.push_str(&rest[pos..]);
                rest = "";
                break;
            }
        }
    }
    let (rep, n) = wrap_bare_words(rest, siblings);
    out.push_str(&rep);
    count += n;
    (out, count)
}

/// Wrap whole-word sibling names in `prose` (no `{{ns:}}` tokens) with tokens.
fn wrap_bare_words(prose: &str, siblings: &HashSet<String>) -> (String, usize) {
    let mut out = String::with_capacity(prose.len());
    let mut count = 0;
    let mut word = String::new();
    for c in prose.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            count += emit_word(&word, siblings, &mut out);
            word.clear();
            out.push(c);
        }
    }
    count += emit_word(&word, siblings, &mut out);
    (out, count)
}

/// Emit one word: wrapped as a token when it is a sibling name, else verbatim.
/// Returns 1 if it was wrapped.
fn emit_word(word: &str, siblings: &HashSet<String>, out: &mut String) -> usize {
    if word.is_empty() {
        return 0;
    }
    if siblings.contains(word) {
        out.push_str("{{ns:");
        out.push_str(word);
        out.push_str("}}");
        1
    } else {
        out.push_str(word);
        0
    }
}

/// Interpret the user's answer to the meld prefix prompt for a source that
/// declares `[source].prefix` (CLI-24). Returns the alias to set on the source:
/// `None` keeps the declared prefix; `Some("")` is the explicit "no prefix"
/// override; `Some(other)` is a custom prefix. Empty / `y` / `yes` accept the
/// declared prefix, `n` / `no` / `none` drop it, and anything else is taken
/// verbatim (trimmed) as a custom prefix.
pub fn prefix_choice(answer: &str) -> Option<String> {
    let a = answer.trim();
    match a.to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => None,
        "n" | "no" | "none" => Some(String::new()),
        _ => Some(a.to_string()),
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

/// Extract the bare name of every `{{ns:name}}` token in `content`.
///
/// Mirrors [`expand`]'s inline parser: the open delimiter is `{{ns:`, the name
/// is the text up to the next `}}` with surrounding whitespace trimmed, and an
/// unterminated token (no closing `}}`) stops the scan and is not a reference
/// (NS-15). Names are returned in first-seen order, de-duplicated. These are the
/// intra-source dependency edges (DEP-1). Called by [`crate::deps::resolve`].
pub fn referenced_names(content: &str) -> Vec<String> {
    const OPEN: &str = "{{ns:";
    let mut names: Vec<String> = Vec::new();
    let mut rest = content;
    while let Some(pos) = rest.find(OPEN) {
        let after = &rest[pos + OPEN.len()..];
        let Some(end) = after.find("}}") else {
            // Unterminated token: stop, exactly like `expand` does.
            break;
        };
        let name = after[..end].trim();
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        rest = &after[end + 2..];
    }
    names
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
        // An empty prefix is "no prefix" (the override), not a leading dash.
        assert_eq!(apply("review", &Some(String::new())), "review");
    }

    #[test]
    fn templatize_wraps_bare_siblings_and_skips_tokens() {
        // spec: INIT-5
        let s = sibs(&["dev", "style"]);
        let (out, n) = templatize("hand off to dev, see {{ns:style}}, not develop", &s);
        assert_eq!(
            out, "hand off to {{ns:dev}}, see {{ns:style}}, not develop",
            "bare `dev` is wrapped; the token and the longer word `develop` are left alone"
        );
        assert_eq!(n, 1, "only the one bare sibling mention is rewritten");

        // Idempotent: a second pass changes nothing (everything is now tokenized).
        let (again, m) = templatize(&out, &s);
        assert_eq!(again, out);
        assert_eq!(m, 0);
    }

    #[test]
    fn prefix_choice_interprets_the_meld_prompt() {
        // spec: CLI-24
        // Empty / yes -> keep the declared prefix (no alias change).
        assert_eq!(prefix_choice(""), None);
        assert_eq!(prefix_choice("y"), None);
        assert_eq!(prefix_choice("YES"), None);
        // no/none -> the explicit "no prefix" override (empty alias).
        assert_eq!(prefix_choice("n"), Some(String::new()));
        assert_eq!(prefix_choice("none"), Some(String::new()));
        // Anything else is a custom prefix, trimmed and verbatim-cased.
        assert_eq!(prefix_choice("  MyPfx "), Some("MyPfx".to_string()));
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
    fn expand_trims_token_and_leaves_unterminated_verbatim() {
        // spec: NS-15
        let s = sibs(&["dev"]);
        // Whitespace inside the token is trimmed before the sibling lookup.
        assert_eq!(
            expand("{{ns:  dev  }}", &Some("jk".into()), &s).unwrap(),
            "jk-dev"
        );
        // An unterminated token (no closing `}}`) is passed through unchanged.
        assert_eq!(expand("see {{ns:dev", &None, &s).unwrap(), "see {{ns:dev");
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

    #[test]
    fn referenced_names_extracts_tokens_in_order_deduped() {
        // spec: DEP-1
        // Bare names from each token, first-seen order, de-duplicated.
        let got = referenced_names("see {{ns:test}} then {{ns:do}} then {{ns:test}}");
        assert_eq!(got, vec!["test".to_string(), "do".to_string()]);
    }

    #[test]
    fn referenced_names_trims_whitespace_inside_token() {
        // spec: DEP-1
        // Whitespace inside a token is trimmed, mirroring `expand`.
        let got = referenced_names("{{ns:  dev  }}");
        assert_eq!(got, vec!["dev".to_string()]);
    }

    #[test]
    fn referenced_names_no_tokens_is_empty() {
        // spec: DEP-1
        assert!(referenced_names("plain prose, no tokens").is_empty());
    }

    #[test]
    fn referenced_names_unterminated_token_is_not_a_reference() {
        // spec: NS-15
        // An unterminated token (no closing `}}`) stops the scan, exactly like
        // `expand` leaves the remainder verbatim. A terminated token before it is
        // still captured; the dangling one is not.
        assert!(referenced_names("see {{ns:dev").is_empty());
        assert_eq!(
            referenced_names("{{ns:test}} then {{ns:dev"),
            vec!["test".to_string()]
        );
    }

    #[test]
    fn referenced_names_empty_token_is_skipped() {
        // spec: NS-15
        // A token with an empty name (`{{ns:}}`) or whitespace-only name
        // (`{{ns:   }}`) trims to "" and is not a reference: it is skipped, but
        // the scan continues past it to any following valid token.
        assert!(referenced_names("{{ns:}}").is_empty());
        assert!(referenced_names("{{ns:   }}").is_empty());
        assert_eq!(
            referenced_names("{{ns:}} then {{ns:dev}}"),
            vec!["dev".to_string()]
        );
    }

    #[test]
    fn referenced_names_valid_then_unterminated_returns_valid_only() {
        // spec: NS-15
        // A valid terminated token followed by an unterminated one yields the
        // valid name then stops at the dangling token (which is not a reference).
        assert_eq!(
            referenced_names("use {{ns:dev}} and then {{ns:planner"),
            vec!["dev".to_string()]
        );
    }

    #[test]
    fn referenced_names_whitespace_or_empty_content_is_empty() {
        // spec: NS-15
        // Whitespace-only or empty content carries no tokens and no references.
        assert!(referenced_names("").is_empty());
        assert!(referenced_names("   \n\t  ").is_empty());
    }

    #[test]
    fn referenced_names_empty_token_does_not_swallow_following_close() {
        // spec: NS-15
        // `{{ns:}}{{ns:dev}}` is two adjacent tokens: the first is empty (skipped)
        // and the scan resumes after its `}}`, so the second is still parsed.
        assert_eq!(
            referenced_names("{{ns:}}{{ns:dev}}"),
            vec!["dev".to_string()]
        );
    }
}
