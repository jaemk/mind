//! The workflow checks mind reports and never enforces (WF-24, WF-29, WF-30).
//!
//! Three questions, all answered by reading a workflow's `meta` (WF-5) and none
//! of them able to fail an install:
//!
//! - would the harness load this file at all (WF-30, reported by `review`,
//!   warned by `learn` under WF-31)? -- and, kept apart from it, did mind's own
//!   metadata cap stop mind from even looking (WF-56)?
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

use crate::error::MindError;
use crate::sanitize::{has_blocked_chars, strip_ansi};
use crate::workflow_meta::WorkflowMeta;

/// The harness's workflow file size cap: it skips a larger file (WF-7).
///
/// mind reports an overage and installs anyway (WF-32): DSC-90 records that mind
/// does not cap the size of item content, and a cap mind enforced would be
/// mind's, not the harness's.
pub const SIZE_CAP: u64 = 524_288;

/// How many other claimants a collision message names before it counts the rest
/// (WF-59). A reader acts on the first few; a source with three hundred
/// same-named workflows would otherwise turn one defect into a message longer
/// than the file that caused it.
const MAX_NAMED_CLAIMANTS: usize = 3;

/// One read of a workflow file: what its `meta` yielded, how big the file is,
/// and whether mind's own metadata cap is why the `meta` is empty (WF-56).
///
/// The last of those is the reason this is a struct and not a pair. "mind read
/// nothing" and "the file declares nothing" are different facts about different
/// things -- mind's configurable cap (DSC-91, DSC-103) versus the source's
/// content -- and collapsing them makes mind report a lowered cap of its own as
/// a defect in someone else's file.
#[derive(Debug, Default, Clone)]
pub struct WorkflowRead {
    /// What the WF-5 reader extracted. Empty when nothing was readable, for
    /// whatever reason.
    pub meta: WorkflowMeta,
    /// The file's size in bytes, or `None` when its metadata cannot be read.
    pub size: Option<u64>,
    /// `Some(limit)` when mind's own metadata cap refused the read (WF-56), so
    /// `meta` is empty because mind never looked, not because the file is bare.
    pub over_mind_cap: Option<u64>,
}

/// Read a workflow file's `meta` and its size in one pass.
///
/// Never fails, matching WF-5: an absent, unreadable, non-UTF-8, or over-cap
/// file yields an empty [`WorkflowMeta`]. An over-cap file is kept apart from
/// the rest (`over_mind_cap`), because that one is mind's own doing and
/// [`cap_notice`] says so rather than letting [`skip_reasons`] blame the source
/// (WF-56). The size is `None` only when the file's metadata cannot be read at
/// all.
pub fn read(file: &Path) -> WorkflowRead {
    let (meta, over_mind_cap) = match crate::workflow_meta::file_meta(file) {
        Ok(meta) => (meta, None),
        // spec: WF-56 -- mind's cap, carried through instead of collapsed.
        Err(MindError::MetadataTooLarge { limit, .. }) => (WorkflowMeta::default(), Some(limit)),
        Err(_) => (WorkflowMeta::default(), None),
    };
    let size = std::fs::metadata(file).ok().map(|m| m.len());
    WorkflowRead {
        meta,
        size,
        over_mind_cap,
    }
}

/// The WF-56 notice: mind read none of this workflow because the file is past
/// mind's own metadata cap, so nothing here is a statement about the harness.
///
/// `None` when the read was not capped, which is every ordinary case.
pub fn cap_notice(read: &WorkflowRead) -> Option<String> {
    let limit = read.over_mind_cap?;
    Some(format!(
        "mind read no `meta` from this workflow: the file is over mind's own {} metadata read cap, \
         so mind read none of it. That cap is mind's, not the harness's, and says nothing about \
         whether the harness loads the file; raise it with `--max-metadata-size` to have mind read \
         this workflow.",
        crate::error::format_metadata_size(limit)
    ))
}

/// Why the harness would not load this workflow, as message clauses (WF-30).
///
/// Empty when mind's reader sees nothing wrong, which is not a promise that the
/// file loads: the harness's reader is stricter and is the authority (WF-5).
/// That asymmetry is the whole reason this is a report and not a gate.
///
/// A read mind's own cap refused (WF-56) yields no `meta` complaint at all:
/// mind did not read the file, so it knows nothing about what the file
/// declares, and [`cap_notice`] reports that separately. The WF-7 overage still
/// applies there, since it is read off the file's size rather than its content.
pub fn skip_reasons(read: &WorkflowRead) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let meta = &read.meta;
    if read.over_mind_cap.is_none() {
        if meta.is_empty() {
            // No key was readable at all; naming the missing `name` and
            // `description` separately here would be three complaints about one
            // fact. The wording says what mind did, not what the file has: a
            // `meta` object that is present but empty declares a `meta` object.
            out.push(
                "mind read no `name`, `description`, or `whenToUse` from its `meta`".to_string(),
            );
        } else {
            for (key, value) in [
                ("name", meta.name.as_deref()),
                ("description", meta.description.as_deref()),
            ] {
                match value {
                    None => out.push(format!("`meta.{key}` is missing")),
                    // An empty value is kept as `Some("")` by the reader
                    // precisely so this check can tell it apart from an absent
                    // one.
                    Some(v) if v.trim().is_empty() => out.push(format!("`meta.{key}` is empty")),
                    // spec: WF-58 -- an invisible or control character makes a
                    // name mind cannot use: it prints as some OTHER name once
                    // sanitized, so a collision against that name reads as a
                    // comparison of a string with itself. Named here because
                    // `harness_name_with_bare` then yields nothing, and an
                    // unusable name with no reason given is a silent drop.
                    //
                    // Tested on the TRIMMED value, because that is the name mind
                    // uses (WF-20): `harness_name_with_bare` trims first and
                    // then applies this same test, so testing the raw value here
                    // would report `name: '  review\n'` -- padding a template
                    // literal spanning lines produces exactly that -- as a name
                    // mind cannot use while mind went on using it as `review`.
                    Some(v) if key == "name" && has_blocked_chars(v.trim()) => out.push(
                        "`meta.name` contains a control or invisible character, so mind cannot \
                         use it as a harness name"
                            .to_string(),
                    ),
                    Some(_) => {}
                }
            }
        }
    }
    // spec: WF-7 WF-32 -- reported, never enforced.
    if let Some(bytes) = read.size
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
/// This form models an INSTALLED file, where the tokens are already expanded,
/// so there is no prefix and no sibling set to apply and this is a trim. It
/// takes no such arguments precisely so the unsound call cannot be written: a
/// caller that expands an UNINSTALLED file under a prefix must use
/// [`harness_name_with_bare`], or a `{{ns:}}` naming a sibling agent is
/// predicted with a prefix install would not write (NS-42).
pub fn harness_name(meta: &WorkflowMeta) -> Option<String> {
    harness_name_with_bare(meta, &None, &HashSet::new(), &HashSet::new())
}

/// [`harness_name`] with the NS-42 bare-name set spelled out.
///
/// This form models what INSTALL will write (`install.rs`'s `expand_references`):
/// a `{{ns:}}` token naming a sibling agent expands BARE even under a prefix, so
/// predicting the harness-facing name without that set would report a `prefix:x`
/// the store never contains and raise a false WF-24 divergence. `bare_names` is
/// the set install computes: sibling agent names minus any name a non-agent
/// sibling also holds (the cross-kind shadow rule).
///
/// spec: WF-58 -- a name carrying a control or invisible code point is no name
/// either. The WF-29 collision test is an exact comparison while every message
/// prints the name sanitized, so a `revi<U+200B>ew` would print as `review`,
/// collide with nothing, and compose the absurd "resolves it as 'review', not
/// 'review'". [`skip_reasons`] reports it as its own defect instead.
pub fn harness_name_with_bare(
    meta: &WorkflowMeta,
    prefix: &Option<String>,
    siblings: &HashSet<String>,
    bare_names: &HashSet<String>,
) -> Option<String> {
    let raw = meta.name.as_deref()?;
    let expanded = crate::namespace::expand(raw, prefix, siblings, bare_names).ok()?;
    let trimmed = expanded.trim();
    if trimmed.is_empty() || has_blocked_chars(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// The WF-24 divergence message, or `None` when the two names agree.
///
/// A workflow with no readable name does not diverge: it is unloadable, which
/// [`skip_reasons`] already reports, and saying it also answers to the wrong
/// name would be a second complaint about the same defect.
/// The remedy clause names the BARE name, not the effective one: `{{ns:}}`
/// resolves against an item's bare sibling names (NS-11), so `{{ns:jk:review}}`
/// under a `jk` prefix names no sibling at all and the install that followed
/// mind's own advice would die with a hard `bad-reference` (NS-12). The token
/// renders as the effective name once expanded, which is the whole point of
/// WF-23.
pub fn divergence(
    effective_name: &str,
    bare_name: &str,
    harness_name: Option<&str>,
) -> Option<String> {
    let harness = harness_name?;
    if harness == effective_name {
        return None;
    }
    Some(format!(
        "the harness resolves it as '{}', not '{}' -- it answers to its `meta.name`, not its \
         file name (write `meta.name: '{{{{ns:{}}}}}'` to keep the two in step)",
        strip_ansi(harness),
        strip_ansi(effective_name),
        strip_ansi(bare_name),
    ))
}

/// The WF-29 collision message for a harness name two or more workflows claim.
///
/// `others` is every OTHER item key answering to the same name, already
/// display-sanitized by its own producer, and must not be empty: a name nobody
/// else claims is not a collision, and the count-aware wording below reads as
/// nonsense ("which  also claim") if it is called anyway.
///
/// spec: WF-59 -- at most [`MAX_NAMED_CLAIMANTS`] are named and the rest are
/// counted, so the message stays bounded however many workflows pile onto one
/// name.
///
/// This is the installed-site wording (`learn`, `upgrade`, `recall <item>`);
/// `review` says the same thing about what it would install ([`collisions`]).
pub fn collision(harness_name: &str, others: &[String]) -> String {
    collision_at(harness_name, others, Site::Installed)
}

/// Where a collision message is read, which decides only its tail.
#[derive(Clone, Copy)]
enum Site {
    /// The workflows are (or are being) installed.
    Installed,
    /// `review` of an uninstalled source: nothing is installed yet.
    Review,
}

fn collision_at(harness_name: &str, others: &[String], site: Site) -> String {
    debug_assert!(
        !others.is_empty(),
        "collision() takes the OTHER claimants of a shared name; there is always at least one"
    );
    let listed = match others.len() > MAX_NAMED_CLAIMANTS {
        true => format!(
            "{}, and {} more",
            others[..MAX_NAMED_CLAIMANTS].join(", "),
            others.len() - MAX_NAMED_CLAIMANTS
        ),
        false => others.join(", "),
    };
    let total = others.len() + 1;
    let q = match total {
        2 => "both".to_string(),
        n => format!("all {n}"),
    };
    let tail = match site {
        Site::Installed => {
            format!("the harness sees one workflow under that name, and {q} are installed")
        }
        Site::Review => format!(
            "the harness would see one workflow under that name, and mind would install {q}"
        ),
    };
    format!(
        "it answers to the harness name '{}', which {listed} also claim{}: {tail}",
        strip_ansi(harness_name),
        match others.len() {
            1 => "s",
            _ => "",
        },
    )
}

/// Group harness-name claims into the WF-59 collision groups they produce: one
/// entry per COLLIDING name, never one per claimant.
///
/// `claims` is `(harness name, item key)` in report order; the result is
/// `(name, every key claiming it)` for each name with two or more claimants, in
/// order of the name's first claim. Grouping once is what keeps the report
/// linear: the per-claimant form this replaces rebuilt the other n-1 keys for
/// each of n claims, so a source with many same-named workflows produced
/// quadratic output from linear input.
pub fn claim_groups(claims: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut order: Vec<&str> = Vec::new();
    let mut by_name: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for (name, key) in claims {
        let claimants = by_name.entry(name.as_str()).or_default();
        if claimants.is_empty() {
            order.push(name.as_str());
        }
        claimants.push(key.clone());
    }
    order
        .into_iter()
        .filter_map(|name| {
            let claimants = by_name.remove(name)?;
            match claimants.len() >= 2 {
                true => Some((name.to_string(), claimants)),
                false => None,
            }
        })
        .collect()
}

/// The WF-59 collision reports for `claims`: one `(subject key, message)` per
/// colliding name, the subject being the first claimant of that name.
///
/// The shape `review` wants, which has no reason to prefer one claimant over
/// another. `learn`/`upgrade` build theirs from [`claim_groups`] directly, so
/// the subject can be an item that run actually touched.
pub fn collisions(claims: &[(String, String)]) -> Vec<(String, String)> {
    claim_groups(claims)
        .into_iter()
        .map(|(name, claimants)| {
            let others: Vec<String> = claimants[1..].iter().map(|k| strip_ansi(k)).collect();
            (
                claimants[0].clone(),
                collision_at(&name, &others, Site::Review),
            )
        })
        .collect()
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

    /// A [`WorkflowRead`] as an uncapped read of a file of `size` would produce.
    fn read_of(meta: WorkflowMeta, size: Option<u64>) -> WorkflowRead {
        WorkflowRead {
            meta,
            size,
            over_mind_cap: None,
        }
    }

    /// A complete `meta` under the cap is not reported.
    // spec: WF-30
    #[test]
    fn a_loadable_workflow_has_no_skip_reasons() {
        let m = meta(Some("review"), Some("Review the diff"));
        assert!(skip_reasons(&read_of(m, Some(1024))).is_empty());
    }

    /// An unreadable `meta` is ONE reason, not three, and it says what mind
    /// read rather than claiming the file declares no `meta` at all -- which is
    /// false for a present-but-empty `export const meta = {}`.
    // spec: WF-30
    #[test]
    fn an_unreadable_meta_is_a_single_skip_reason() {
        let reasons = skip_reasons(&read_of(WorkflowMeta::default(), Some(10)));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("mind read no `name`, `description`, or `whenToUse`"),
            "{reasons:?}"
        );
    }

    /// Missing and empty are distinguished, and both are reported.
    // spec: WF-30
    #[test]
    fn a_missing_name_and_an_empty_description_are_both_reported() {
        let reasons = skip_reasons(&read_of(meta(None, Some("   ")), None));
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
        assert!(
            skip_reasons(&read_of(m.clone(), Some(SIZE_CAP))).is_empty(),
            "cap is not <"
        );
        let reasons = skip_reasons(&read_of(m, Some(SIZE_CAP + 1)));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("524288-byte cap"), "{reasons:?}");
    }

    /// A read mind's OWN cap refused says nothing about the file's `meta`: it
    /// is reported as mind's cap, naming the flag that raises it, and not as a
    /// defect in the source. The harness's own cap still applies, since that
    /// one is read off the file's size rather than its content.
    // spec: WF-56
    #[test]
    fn minds_own_cap_is_reported_apart_from_what_the_file_declares() {
        let capped = WorkflowRead {
            meta: WorkflowMeta::default(),
            size: Some(SIZE_CAP + 1),
            over_mind_cap: Some(2048),
        };
        let reasons = skip_reasons(&capped);
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("524288-byte cap"), "{reasons:?}");
        assert!(
            !reasons.iter().any(|r| r.contains("mind read no")),
            "an unread file must not be reported as an empty one: {reasons:?}"
        );
        let notice = cap_notice(&capped).expect("the cap is reported on its own terms");
        assert!(notice.contains("2 KiB"), "{notice}");
        assert!(notice.contains("--max-metadata-size"), "{notice}");
        assert!(
            !notice.contains("will not load"),
            "mind read nothing, so it claims nothing about the harness: {notice}"
        );
        // An ordinary read has no such notice.
        assert_eq!(
            cap_notice(&read_of(WorkflowMeta::default(), Some(10))),
            None
        );
    }

    /// `read` of an absent file yields the empty meta WF-5 promises, not an
    /// error, and does not claim mind's cap was involved.
    // spec: WF-5 WF-30 WF-56
    #[test]
    fn reading_an_absent_file_yields_an_empty_meta() {
        let r = read(Path::new("./does-not-exist-workflow.js"));
        assert!(r.meta.is_empty());
        assert_eq!(r.size, None);
        assert_eq!(r.over_mind_cap, None);
    }

    /// A `meta.name` carrying an invisible code point is no usable name: it is
    /// reported as its own defect, and it never becomes a harness name that
    /// would print as some other name entirely.
    // spec: WF-58
    #[test]
    fn an_invisible_character_in_a_meta_name_is_not_a_usable_name() {
        let m = meta(Some("revi\u{200B}ew"), Some("Review the diff"));
        assert_eq!(
            harness_name(&m),
            None,
            "a name that prints as a different string is not one mind can use"
        );
        let reasons = skip_reasons(&read_of(m.clone(), Some(10)));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(
            reasons[0].contains("control or invisible character"),
            "{reasons:?}"
        );
        // And with no harness name there is no divergence to report on top,
        // which would have read "resolves it as 'review', not 'review'".
        assert_eq!(
            divergence("review", "review", harness_name(&m).as_deref()),
            None
        );
    }

    /// The trim comes BEFORE the WF-58 character test, so a name whose only
    /// defect is padding is a usable name (and agrees with the item's own name),
    /// while a control character the trim cannot reach makes it unusable. Get
    /// the order wrong the other way and every padded name becomes a defect.
    // spec: WF-58 WF-24
    #[test]
    fn a_padded_name_trims_while_an_interior_control_character_does_not() {
        for padded in ["  review  ", "\treview\n", "review\n"] {
            let m = meta(Some(padded), Some("d"));
            assert_eq!(
                harness_name(&m).as_deref(),
                Some("review"),
                "padding is trimmed, not a defect: {padded:?}"
            );
            assert!(
                skip_reasons(&read_of(m.clone(), Some(10))).is_empty(),
                "a padded name draws no reason: {padded:?}"
            );
            assert_eq!(
                divergence("review", "review", harness_name(&m).as_deref()),
                None,
                "and it agrees with the item's own name: {padded:?}"
            );
        }
        for hostile in [
            "revi\tew",
            "revi\u{1b}[31mew",
            "revi\u{202E}ew",
            "revi\u{200B}ew",
        ] {
            let m = meta(Some(hostile), Some("d"));
            assert_eq!(
                harness_name(&m),
                None,
                "an interior control/invisible character is unusable: {hostile:?}"
            );
            let reasons = skip_reasons(&read_of(m, Some(10)));
            assert!(
                reasons.iter().any(|r| r.contains("control or invisible")),
                "and is reported as its own reason: {hostile:?} -> {reasons:?}"
            );
        }
        // A name that is nothing BUT whitespace is empty, not unusable: the trim
        // reaches it, so the wording that fits is `is empty`.
        let reasons = skip_reasons(&read_of(meta(Some("\t  "), Some("d")), Some(10)));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("`meta.name` is empty"), "{reasons:?}");
        // A name that is nothing but an invisible character has no trim to reach
        // it, so it is the WF-58 reason instead.
        let reasons = skip_reasons(&read_of(meta(Some("\u{200B}"), Some("d")), Some(10)));
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert!(reasons[0].contains("control or invisible"), "{reasons:?}");
    }

    /// An unusable name is one reason among the others its `meta` earns, not a
    /// replacement for them: the missing `description` beside it is still
    /// reported, and so is the WF-7 overage.
    // spec: WF-58 WF-30 WF-7
    #[test]
    fn an_unusable_name_does_not_suppress_the_other_reasons() {
        let reasons = skip_reasons(&read_of(
            meta(Some("revi\u{200B}ew"), None),
            Some(SIZE_CAP + 1),
        ));
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons[0].contains("control or invisible"), "{reasons:?}");
        assert!(
            reasons[1].contains("`meta.description` is missing"),
            "{reasons:?}"
        );
        assert!(reasons[2].contains("524288-byte cap"), "{reasons:?}");
    }

    /// The other half of WF-56: a capped read whose file size is unknown too
    /// (the file went away between the read and the `stat`) reports the cap and
    /// nothing else. No reason may be invented for a file mind never saw.
    // spec: WF-56
    #[test]
    fn a_capped_read_with_no_size_reports_only_the_cap() {
        let capped = WorkflowRead {
            meta: WorkflowMeta::default(),
            size: None,
            over_mind_cap: Some(1024),
        };
        assert!(skip_reasons(&capped).is_empty(), "nothing is known");
        assert!(cap_notice(&capped).is_some());
    }

    /// The claimant cap is exclusive: exactly [`MAX_NAMED_CLAIMANTS`] others are
    /// all named with no count appended, and one more tips it over. A cap that
    /// were off by one here would either drop a claimant silently or say "and 0
    /// more".
    // spec: WF-59
    #[test]
    fn the_claimant_cap_is_exclusive_at_its_boundary() {
        let keys: Vec<String> = (0..4).map(|i| format!("workflow:w{i}")).collect();
        let at = collision("deploy", &keys[..MAX_NAMED_CLAIMANTS]);
        assert!(at.contains("workflow:w0, workflow:w1, workflow:w2"), "{at}");
        assert!(!at.contains("more"), "exactly the cap names them all: {at}");
        assert!(at.contains("also claim:"), "plural at three: {at}");
        let over = collision("deploy", &keys);
        assert!(over.contains("and 1 more"), "{over}");
        assert!(!over.contains("workflow:w3"), "{over}");
    }

    /// Grouping is by name, in order of each name's FIRST claim, and a claimant
    /// list keeps the order the claims arrived in. `collisions` then names the
    /// first claimant as the subject and never repeats it among the others --
    /// which is what keeps a report from reading "x also claims" against x.
    // spec: WF-59 WF-29
    #[test]
    fn claim_groups_keep_first_claim_order_and_never_list_the_subject() {
        let claims: Vec<(String, String)> = [
            ("ship", "workflow:zeta"),
            ("deploy", "workflow:mid"),
            ("ship", "workflow:alpha"),
            ("solo", "workflow:only"),
            ("deploy", "workflow:other"),
        ]
        .into_iter()
        .map(|(n, k)| (n.to_string(), k.to_string()))
        .collect();

        let groups = claim_groups(&claims);
        assert_eq!(
            groups
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<&str>>(),
            vec!["ship", "deploy"],
            "first-claim order, and `solo` is no group: {groups:?}"
        );
        assert_eq!(groups[0].1, vec!["workflow:zeta", "workflow:alpha"]);

        let reports = collisions(&claims);
        assert_eq!(reports.len(), 2, "{reports:?}");
        assert_eq!(
            reports[0].0, "workflow:zeta",
            "the first claimant is subject"
        );
        assert!(
            reports[0].1.contains("workflow:alpha also claims"),
            "{}",
            reports[0].1
        );
        for (subject, msg) in &reports {
            assert!(
                !msg.contains(&format!("{subject} also")),
                "the subject must not be listed among the others: {msg}"
            );
        }
        // Nothing to group is nothing to report.
        assert!(claim_groups(&[]).is_empty());
        assert!(collisions(&[]).is_empty());
    }

    /// Two DIFFERENT names never share a group, however similar they look once
    /// displayed: the comparison is the exact string, which is why WF-58 refuses
    /// a name that only prints like another one.
    // spec: WF-59 WF-58
    #[test]
    fn grouping_compares_exact_names_not_displayed_ones() {
        let claims: Vec<(String, String)> = [
            ("deploy", "workflow:a"),
            ("deploy ", "workflow:b"),
            ("Deploy", "workflow:c"),
        ]
        .into_iter()
        .map(|(n, k)| (n.to_string(), k.to_string()))
        .collect();
        assert!(
            claim_groups(&claims).is_empty(),
            "three distinct strings are three names"
        );
    }

    /// A token in `meta.name` expands before the comparison (WF-23), so a
    /// tokenized name under a prefix does NOT diverge. The UNINSTALLED form is
    /// the only one that takes a prefix: the installed file carries the
    /// expanded literal, which is what [`harness_name`] models.
    // spec: WF-23 WF-24
    #[test]
    fn a_tokenized_meta_name_matches_the_prefixed_effective_name() {
        let m = meta(Some("{{ns:review}}"), Some("d"));
        let siblings: HashSet<String> = ["review".to_string()].into_iter().collect();
        let name = harness_name_with_bare(&m, &Some("jk".to_string()), &siblings, &HashSet::new());
        assert_eq!(name.as_deref(), Some("jk:review"));
        assert_eq!(divergence("jk:review", "review", name.as_deref()), None);
        // And the installed copy, where the token is already a literal.
        let installed = meta(Some("jk:review"), Some("d"));
        assert_eq!(harness_name(&installed).as_deref(), Some("jk:review"));
    }

    /// The same file under no prefix expands bare and still agrees.
    // spec: WF-23 WF-24
    #[test]
    fn a_tokenized_meta_name_expands_bare_without_a_prefix() {
        let m = meta(Some("{{ns:review}}"), Some("d"));
        let siblings: HashSet<String> = ["review".to_string()].into_iter().collect();
        let name = harness_name_with_bare(&m, &None, &siblings, &HashSet::new());
        assert_eq!(name.as_deref(), Some("review"));
        assert_eq!(divergence("review", "review", name.as_deref()), None);
    }

    /// A literal name that differs from the item's effective name diverges, and
    /// the message names both spellings plus the token remedy.
    ///
    /// The remedy names the BARE name. `{{ns:}}` resolves against bare sibling
    /// names, so the prefixed spelling would be a token naming no sibling, and
    /// an author who followed the advice would get a hard `bad-reference`
    /// instead of a working install.
    // spec: WF-24 NS-11
    #[test]
    fn a_literal_meta_name_diverges_from_a_prefixed_effective_name() {
        let m = meta(Some("review"), Some("d"));
        let name = harness_name_with_bare(
            &m,
            &Some("jk".to_string()),
            &HashSet::new(),
            &HashSet::new(),
        );
        assert_eq!(name.as_deref(), Some("review"));
        let msg = divergence("jk:review", "review", name.as_deref()).expect("diverges");
        assert!(msg.contains("'review'"), "{msg}");
        assert!(msg.contains("'jk:review'"), "{msg}");
        assert!(
            msg.contains("{{ns:review}}"),
            "the remedy must be a token that resolves: {msg}"
        );
        assert!(
            !msg.contains("{{ns:jk:review}}"),
            "a prefixed referent resolves to no sibling (NS-11): {msg}"
        );
    }

    /// No readable name is WF-30's business, not WF-24's: no divergence.
    // spec: WF-24 WF-30
    #[test]
    fn an_unreadable_name_does_not_also_report_a_divergence() {
        let m = meta(None, Some("d"));
        assert_eq!(harness_name(&m), None);
        assert_eq!(divergence("review", "review", None), None);
    }

    /// A `meta.name` token naming no sibling yields no name, so the hard
    /// `bad-reference` elsewhere is not doubled by a derived divergence.
    // spec: WF-24 WF-27
    #[test]
    fn an_unresolvable_token_in_a_meta_name_yields_no_harness_name() {
        let m = meta(Some("{{ns:typo}}"), Some("d"));
        assert_eq!(
            harness_name_with_bare(&m, &None, &HashSet::new(), &HashSet::new()),
            None
        );
    }

    /// A `{{ns:}}` naming a sibling AGENT expands bare even under a prefix, the
    /// way install writes it, so the predicted name carries no prefix and there
    /// is no divergence to report.
    // spec: NS-42 WF-23 WF-24
    #[test]
    fn an_agent_referent_in_a_meta_name_expands_bare_under_a_prefix() {
        let m = meta(Some("{{ns:review}}"), Some("d"));
        let siblings: HashSet<String> = ["review".to_string()].into_iter().collect();
        let bare: HashSet<String> = ["review".to_string()].into_iter().collect();
        let name = harness_name_with_bare(&m, &Some("jk".to_string()), &siblings, &bare);
        assert_eq!(name.as_deref(), Some("review"));
        assert_eq!(divergence("review", "review", name.as_deref()), None);
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

    /// However many workflows pile onto one name, the message names a few and
    /// counts the rest: the report is bounded by the wording, not by the size
    /// of the source.
    // spec: WF-59
    #[test]
    fn a_collision_message_caps_the_claimants_it_names() {
        let others: Vec<String> = (0..300).map(|i| format!("workflow:w{i:03}")).collect();
        let msg = collision("deploy", &others);
        assert!(
            msg.contains("workflow:w000, workflow:w001, workflow:w002"),
            "{msg}"
        );
        assert!(msg.contains("and 297 more"), "{msg}");
        assert!(
            !msg.contains("workflow:w003"),
            "the fourth claimant is counted, not named: {msg}"
        );
        assert!(msg.len() < 400, "the message must stay bounded: {msg}");
    }

    /// Many claims of one name are ONE group, not one per claimant, and a name
    /// only one workflow claims is no group at all.
    // spec: WF-59
    #[test]
    fn claims_group_by_name_into_one_report_each() {
        let mut claims: Vec<(String, String)> = (0..300)
            .map(|i| ("deploy".to_string(), format!("workflow:w{i:03}")))
            .collect();
        claims.push(("lonely".to_string(), "workflow:solo".to_string()));
        claims.push(("ship".to_string(), "workflow:a".to_string()));
        claims.push(("ship".to_string(), "workflow:b".to_string()));

        let groups = claim_groups(&claims);
        assert_eq!(groups.len(), 2, "one group per colliding name: {groups:?}");
        assert_eq!(groups[0].0, "deploy");
        assert_eq!(groups[0].1.len(), 300);
        assert_eq!(groups[1].0, "ship");

        let reports = collisions(&claims);
        assert_eq!(reports.len(), 2, "{reports:?}");
        assert_eq!(
            reports[0].0, "workflow:w000",
            "the first claimant is the subject"
        );
        assert!(reports[0].1.contains("and 296 more"), "{}", reports[0].1);
        assert_eq!(reports[1].0, "workflow:a");
        assert!(
            reports[1].1.contains("workflow:b also claims"),
            "{}",
            reports[1].1
        );
    }

    /// The tail counts the claimants: "both" for two, "all N" beyond, and the
    /// review wording speaks of what mind WOULD install, never "are installed".
    // spec: WF-29 WF-59
    #[test]
    fn the_collision_tail_is_count_aware_and_site_aware() {
        let one = collision("d", &["workflow:a".to_string()]);
        assert!(one.ends_with("and both are installed"), "{one}");
        let three: Vec<String> = (0..3).map(|i| format!("workflow:w{i}")).collect();
        let four = collision("d", &three);
        assert!(four.ends_with("and all 4 are installed"), "{four}");
        assert!(!four.contains("both"), "{four}");

        let claims2: Vec<(String, String)> = vec![
            ("d".into(), "workflow:a".into()),
            ("d".into(), "workflow:b".into()),
        ];
        let r2 = &collisions(&claims2)[0].1;
        assert!(
            r2.ends_with("and mind would install both"),
            "review wording: {r2}"
        );
        assert!(r2.contains("the harness would see one"), "{r2}");
        assert!(!r2.contains("are installed"), "{r2}");
        let claims4: Vec<(String, String)> = (0..4)
            .map(|i| ("d".to_string(), format!("workflow:w{i}")))
            .collect();
        let r4 = &collisions(&claims4)[0].1;
        assert!(r4.ends_with("and mind would install all 4"), "{r4}");
        assert!(!r4.contains("are installed"), "{r4}");
    }

    /// The 2/3 claimant boundary: two is "both", three is already "all 3" at both
    /// sites, and the head is byte-identical between them.
    // spec: WF-29 WF-59
    #[test]
    fn three_claimants_say_all_3_not_both_at_either_site() {
        let others = vec!["workflow:a".to_string(), "workflow:b".to_string()];
        let inst = collision("d", &others);
        assert!(inst.ends_with("and all 3 are installed"), "{inst}");
        assert!(!inst.contains("both"), "{inst}");
        let claims: Vec<(String, String)> = ["workflow:x", "workflow:a", "workflow:b"]
            .iter()
            .map(|k| ("d".to_string(), k.to_string()))
            .collect();
        let rev = &collisions(&claims)[0].1;
        assert!(rev.ends_with("and mind would install all 3"), "{rev}");
        assert!(!rev.contains("both"), "{rev}");
        let head = |s: &str| s.split(": ").next().unwrap().to_string();
        assert!(head(&inst).starts_with("it answers to the harness name 'd'"));
        assert!(rev.starts_with("it answers to the harness name 'd', which"));
    }

    /// Source-controlled text is stripped as it is composed, at this boundary,
    /// so no call site has to remember.
    // spec: DSC-95 CLI-224
    #[test]
    fn a_meta_name_is_sanitized_into_every_message() {
        let msg = divergence("review", "review", Some("ev\u{1b}[31mil")).expect("diverges");
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
        let msg = collision("ev\u{1b}[31mil", &["workflow:x".to_string()]);
        assert!(!msg.contains('\u{1b}'), "{msg:?}");
    }
}
