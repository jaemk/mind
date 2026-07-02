//! Keep `CHANGELOG.md`'s link references in step with its version sections.
//!
//! Every `## [X.Y.Z]` section must carry a matching `[X.Y.Z]:` link reference,
//! `[Unreleased]` must compare from the newest released version, and each
//! version's compare link must run from the next-older version to itself (the
//! oldest is a `releases/tag` link). This fails the build when a release adds a
//! section but forgets to add or update the refs at the bottom, so they cannot
//! silently drift out of date.

use std::collections::BTreeMap;

const REPO: &str = "https://github.com/jaemk/mind";

fn changelog() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md");
    std::fs::read_to_string(&path).expect("CHANGELOG.md must exist and be readable")
}

/// Section names in document order, e.g. `["Unreleased", "0.12.0", "0.11.0", ...]`.
fn section_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("## [")?;
            let end = rest.find(']')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// Link references at the bottom: `[name]: url` -> `(name, url)`.
fn link_refs(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix('[')?;
            let close = rest.find("]: ")?;
            let name = rest[..close].to_string();
            let url = rest[close + 3..].trim().to_string();
            Some((name, url))
        })
        .collect()
}

#[test]
fn every_version_section_has_a_link_reference() {
    let text = changelog();
    let refs = link_refs(&text);
    let missing: Vec<String> = section_names(&text)
        .into_iter()
        .filter(|name| !refs.contains_key(name))
        .collect();
    assert!(
        missing.is_empty(),
        "CHANGELOG.md sections without a link reference: {missing:?} (add `[X.Y.Z]: {REPO}/compare/...` at the bottom)"
    );
}

#[test]
fn unreleased_compares_from_the_newest_version() {
    let text = changelog();
    let sections = section_names(&text);
    let refs = link_refs(&text);
    assert_eq!(
        sections.first().map(String::as_str),
        Some("Unreleased"),
        "the first section must be [Unreleased]"
    );
    let newest = sections
        .get(1)
        .expect("there must be at least one released version below [Unreleased]");
    let want = format!("{REPO}/compare/v{newest}...HEAD");
    assert_eq!(
        refs.get("Unreleased").map(String::as_str),
        Some(want.as_str()),
        "[Unreleased] must compare from the newest released version (v{newest})"
    );
}

#[test]
fn each_version_compare_link_chains_to_the_next_older_version() {
    let text = changelog();
    let refs = link_refs(&text);
    // Released versions only (drop the leading Unreleased), newest to oldest.
    let versions: Vec<String> = section_names(&text)
        .into_iter()
        .filter(|n| n != "Unreleased")
        .collect();
    assert!(versions.len() >= 2, "expected multiple released versions");

    for pair in versions.windows(2) {
        let (this, older) = (&pair[0], &pair[1]);
        let want = format!("{REPO}/compare/v{older}...v{this}");
        assert_eq!(
            refs.get(this).map(String::as_str),
            Some(want.as_str()),
            "[{this}] must compare from the next-older version (v{older})"
        );
    }

    // The oldest version links to its own tag, not a compare range.
    let oldest = versions.last().unwrap();
    let want = format!("{REPO}/releases/tag/v{oldest}");
    assert_eq!(
        refs.get(oldest).map(String::as_str),
        Some(want.as_str()),
        "the oldest version [{oldest}] must be a releases/tag link"
    );
}
