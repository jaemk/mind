//! The workflow checks mind reports and never enforces (WF-24, WF-29, WF-30).
//!
//! Three questions, all answered by reading a workflow's `meta` (WF-5) and none
//! of them able to fail an install:
//!
//! - would the harness load this file at all (WF-30, reported by `review`,
//!   warned by `learn` under WF-31)?
//! - does the name the harness answers to differ from the name mind installed
//!   the item under (WF-24)?
//! - do two workflows answer to one name (WF-29)?
//!
//! They live together because they share one read and one notion of the
//! harness-facing name, and apart from `workflow_meta` because that module is a
//! reader with no opinion about what it read. Every message is composed here, so
//! `review`'s finding, `learn`'s warning, and `recall <item>`'s detail line say
//! the same thing about the same file.
//!
//! Source-controlled text (a `meta.name`) is sanitized here, once, as it is
//! composed into a message (DSC-95, CLI-224), so no call site has to remember.

use std::collections::HashSet;
use std::path::Path;

use crate::sanitize::strip_ansi;
use crate::workflow_meta::WorkflowMeta;

/// The harness's workflow file size cap: it skips a larger file (WF-7).
///
/// mind reports an overage and installs anyway (WF-32): DSC-90 records that mind
/// does not cap the size of item content, and a cap mind enforced would be
/// mind's, not the harness's.
pub const SIZE_CAP: u64 = 524_288;

/// Read a workflow file's `meta` and its size in one pass.
///
/// Never fails, matching WF-5: an absent, unreadable, non-UTF-8, or over-cap
/// file yields an empty [`WorkflowMeta`], which [`skip_reasons`] reports as the
/// "no readable `meta`" condition. The size is `None` only when the file's
/// metadata cannot be read at all.
pub fn read(file: &Path) -> (WorkflowMeta, Option<u64>) {
    let meta = crate::workflow_meta::file_meta(file).unwrap_or_default();
    let size = std::fs::metadata(file).ok().map(|m| m.len());
    (meta, size)
}

/// Why the harness would not load this workflow, as message clauses (WF-30).
///
/// Empty when mind's reader sees nothing wrong, which is not a promise that the
/// file loads: the harness's reader is stricter and is the authority (WF-5).
/// That asymmetry is the whole reason this is a report and not a gate.
pub fn skip_reasons(meta: &WorkflowMeta, size: Option<u64>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if meta.is_empty() {
        // No key was readable at all; naming the missing `name` and
        // `description` separately here would be three complaints about one
        // fact.
        out.push("it declares no `meta` object mind can read".to_string());
    } else {
        for (key, value) in [
            ("name", meta.name.as_deref()),
            ("description", meta.description.as_deref()),
        ] {
            match value {
                None => out.push(format!("`meta.{key}` is missing")),
                // An empty value is kept as `Some("")` by the reader precisely
                // so this check can tell it apart from an absent one.
                Some(v) if v.trim().is_empty() => out.push(format!("`meta.{key}` is empty")),
                Some(_) => {}
            }
        }
    }
    // spec: WF-7 WF-32 -- reported, never enforced.
    if let Some(bytes) = size
        && bytes > SIZE_CAP
    {
        out.push(format!(
            "the file is {bytes} bytes, over the harness's {SIZE_CAP}-byte cap"
        ));
    }
    out
}

/// The name the harness will answer to (WF-20): `meta.name` after `{{ns:}}`
/// expansion (WF-23), trimmed.
///
/// `None` when there is no usable string name: absent, empty, or carrying a
/// token that resolves to no sibling. That last case is already a hard
/// `bad-reference` wherever this is called (NS-12, WF-27), so guessing at what
/// the token meant here would only add a second, derived complaint.
///
/// For an INSTALLED file the tokens are already expanded, so the caller passes
/// no prefix and no siblings and this is a trim.
pub fn harness_name(
    meta: &WorkflowMeta,
    prefix: &Option<String>,
    siblings: &HashSet<String>,
) -> Option<String> {
    let raw = meta.name.as_deref()?;
    let no_bare: HashSet<String> = HashSet::new();
    let expanded = crate::namespace::expand(raw, prefix, siblings, &no_bare).ok()?;
    let trimmed = expanded.trim();
    match trimmed.is_empty() {
        true => None,
        false => Some(trimmed.to_string()),
    }
}

/// The WF-24 divergence message, or `None` when the two names agree.
///
/// A workflow with no readable name does not diverge: it is unloadable, which
/// [`skip_reasons`] already reports, and saying it also answers to the wrong
/// name would be a second complaint about the same defect.
pub fn divergence(effective_name: &str, harness_name: Option<&str>) -> Option<String> {
    let harness = harness_name?;
    if harness == effective_name {
        return None;
    }
    Some(format!(
        "the harness resolves it as '{}', not '{}' -- it answers to its `meta.name`, not its \
         file name (write `meta.name: '{{{{ns:{}}}}}'` to keep the two in step)",
        strip_ansi(harness),
        strip_ansi(effective_name),
        strip_ansi(effective_name),
    ))
}

/// The WF-29 collision message for a harness name two or more workflows claim.
///
/// `others` is every OTHER item key answering to the same name, already
/// display-sanitized by its own producer.
pub fn collision(harness_name: &str, others: &[String]) -> String {
    format!(
        "it answers to the harness name '{}', which {} also claim{}: the harness sees one \
         workflow under that name and mind installed both",
        strip_ansi(harness_name),
        others.join(", "),
        match others.len() {
            1 => "s",
            _ => "",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(name: Option<&str>, description: Option<&str>) -> WorkflowMeta {
        WorkflowMeta {
            name: name.map(str::to_string),
            description: description.map(str::to_string),
            when_to_use: None,
        }
    }

    /// A complete `meta` under the cap is not reported.
    // spec: WF-30
    #[test]
    fn a_loadable_workflow_has_no_skip_reasons() {
        let m = meta(Some("review"), Some("Review the diff"));
        assert!(skip_reasons(&m, Some(1024)).is_empty());
    }

    /// An unreadable `meta` is ONE reason, not three.
    // spec: WF-30
    #[test]
    fn an_unreadable_meta_is_a_single_skip_reason() {
        let reasons = skip_reasons(&WorkflowMeta::default(), Some(10));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("no `meta` object"), "{reasons:?}");
    }

    /// Missing and empty are distinguished, and both are reported.
    // spec: WF-30
    #[test]
    fn a_missing_name_and_an_empty_description_are_both_reported() {
        let reasons = skip_reasons(&meta(None, Some("   ")), None);
        assert_eq!(reasons.len(), 2, "{reasons:?}");
        assert!(reasons[0].contains("`meta.name` is missing"), "{reasons:?}");
        assert!(
            reasons[1].contains("`meta.description` is empty"),
            "{reasons:?}"
        );
    }

    /// The size cap is a reason on its own, over an otherwise complete `meta`.
    // spec: WF-7 WF-30 WF-32
    #[test]
    fn a_file_over_the_cap_is_reported_by_itself() {
        let m = meta(Some("review"), Some("Review the diff"));
        assert!(skip_reasons(&m, Some(SIZE_CAP)).is_empty(), "cap is not <");
        let reasons = skip_reasons(&m, Some(SIZE_CAP + 1));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("524288-byte cap"), "{reasons:?}");
    }

    /// `read` of an absent file yields the empty meta WF-5 promises, not an error.
    // spec: WF-5 WF-30
    #[test]
    fn reading_an_absent_file_yields_an_empty_meta() {
        let (m, size) = read(Path::new("./does-not-exist-workflow.js"));
        assert!(m.is_empty());
        assert_eq!(size, None);
    }

    /// A token in `meta.name` expands before the comparison (WF-23), so a
    /// tokenized name under a prefix does NOT diverge.
    // spec: WF-23 WF-24
    #[test]
    fn a_tokenized_meta_name_matches_the_prefixed_effective_name() {
        let m = meta(Some("{{ns:review}}"), Some("d"));
        let siblings: HashSet<String> = ["review".to_string()].into_iter().collect();
        let name = harness_name(&m, &Some("jk".to_string()), &siblings);
        assert_eq!(name.as_deref(), Some("jk:review"));
        assert_eq!(divergence("jk:review", name.as_deref()), None);
    }

    /// The same file under no prefix expands bare and still agrees.
    // spec: WF-23 WF-24
    #[test]
    fn a_tokenized_meta_name_expands_bare_without_a_prefix() {
        let m = meta(Some("{{ns:review}}"), Some("d"));
        let siblings: HashSet<String> = ["review".to_string()].into_iter().collect();
        let name = harness_name(&m, &None, &siblings);
        assert_eq!(name.as_deref(), Some("review"));
        assert_eq!(divergence("review", name.as_deref()), None);
    }

    /// A literal name that differs from the item's effective name diverges, and
    /// the message names both spellings plus the token remedy.
    // spec: WF-24
    #[test]
    fn a_literal_meta_name_diverges_from_a_prefixed_effective_name() {
        let m = meta(Some("review"), Some("d"));
        let name = harness_name(&m, &Some("jk".to_string()), &HashSet::new());
        assert_eq!(name.as_deref(), Some("review"));
        let msg = divergence("jk:review", name.as_deref()).expect("diverges");
        assert!(msg.contains("'review'"), "{msg}");
        assert!(msg.contains("'jk:review'"), "{msg}");
        assert!(msg.contains("{{ns:jk:review}}"), "{msg}");
    }

    /// No readable name is WF-30's business, not WF-24's: no divergence.
    // spec: WF-24 WF-30
    #[test]
    fn an_unreadable_name_does_not_also_report_a_divergence() {
        let m = meta(None, Some("d"));
        assert_eq!(harness_name(&m, &None, &HashSet::new()), None);
        assert_eq!(divergence("review", None), None);
    }

    /// A `meta.name` token naming no sibling yields no name, so the hard
    /// `bad-reference` elsewhere is not doubled by a derived divergence.
    // spec: WF-24 WF-27
    #[test]
    fn an_unresolvable_token_in_a_meta_name_yields_no_harness_name() {
        let m = meta(Some("{{ns:typo}}"), Some("d"));
        assert_eq!(harness_name(&m, &None, &HashSet::new()), None);
    }

    /// The collision message agrees in number with how many others claim it.
    // spec: WF-29
    #[test]
    fn the_collision_message_names_every_other_claimant() {
        let one = collision("review", &["workflow:jk:review".to_string()]);
        assert!(one.contains("workflow:jk:review also claims"), "{one}");
        let two = collision(
            "review",
            &["workflow:a".to_string(), "workflow:b".to_string()],
        );
        assert!(two.contains("workflow:a, workflow:b also claim:"), "{two}");
    }

    /// Source-controlled text is stripped as it is composed, at this boundary,
    /// so no call site has to remember.
    // spec: DSC-95 CLI-224
    #[test]
    fn a_meta_name_is_sanitized_into_every_message() {
        let msg = divergence("review", Some("ev\u{1b}[31mil")).expect("diverges");
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
        let msg = collision("ev\u{1b}[31mil", &["workflow:x".to_string()]);
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
    }
}
