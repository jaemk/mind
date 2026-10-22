//! Release versions (spec/versions.md): the `[source].release-tags` template,
//! semver tag parsing and precedence, and caret/exact version ranges.
//!
//! Pure logic only. Listing a remote's tags and checking out the selected
//! release live in `git.rs`; reading the template from a `mind.toml` lives in
//! `mindfile.rs`.

// Not called from the CLI until version pins are wired into meld/sync/upgrade.
#![allow(dead_code)]

use std::cmp::Ordering;
use std::fmt;

/// A semver version `MAJOR.MINOR.PATCH[-pre][+build]` (VER-10).
///
/// Equality and ordering are semver precedence, so build metadata is ignored
/// by both (VER-11): `1.2.3+a == 1.2.3+b`.
#[derive(Debug, Clone)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
    pub build: Option<String>,
}

impl Version {
    /// Parse a strict semver version. `None` for anything else: a missing or
    /// extra component (`1.2`, `1.2.3.4`), a leading zero (`01.2.3`), or an
    /// empty or malformed prerelease/build identifier.
    pub fn parse(s: &str) -> Option<Version> {
        let (rest, build) = match s.split_once('+') {
            Some((r, b)) => {
                if !valid_identifiers(b, false) {
                    return None;
                }
                (r, Some(b.to_string()))
            }
            None => (s, None),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((c, p)) => {
                if !valid_identifiers(p, true) {
                    return None;
                }
                (c, p.split('.').map(str::to_string).collect())
            }
            None => (rest, Vec::new()),
        };
        let mut parts = core.split('.');
        let major = numeric(parts.next()?)?;
        let minor = numeric(parts.next()?)?;
        let patch = numeric(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    // spec: VER-11
    fn cmp(&self, other: &Self) -> Ordering {
        self.triple().cmp(&other.triple()).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A prerelease has lower precedence than its release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => pre_cmp(&self.pre, &other.pre),
            }
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        if let Some(b) = &self.build {
            write!(f, "+{b}")?;
        }
        Ok(())
    }
}

/// A numeric version component: ASCII digits, no leading zero unless it is
/// exactly `0`.
fn numeric(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

/// A dot-separated run of non-empty `[0-9A-Za-z-]` identifiers. In a
/// prerelease, a numeric identifier may not carry a leading zero.
fn valid_identifiers(s: &str, prerelease: bool) -> bool {
    s.split('.').all(|id| {
        !id.is_empty()
            && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            && !(prerelease
                && id.len() > 1
                && id.starts_with('0')
                && id.bytes().all(|c| c.is_ascii_digit()))
    })
}

/// Semver precedence between two prerelease identifier lists: pairwise, a
/// numeric identifier compares numerically and sorts below an alphanumeric
/// one; a shorter list that is a prefix of the longer sorts first.
fn pre_cmp(a: &[String], b: &[String]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let c = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(xn), Ok(yn)) => xn.cmp(&yn),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

/// A `[source].release-tags` template (VER-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Template {
    /// `release-tags = true`: matches both `v{version}` and a bare `{version}`.
    Default,
    /// A literal prefix and suffix around the one `{version}` placeholder.
    Literal { prefix: String, suffix: String },
}

const PLACEHOLDER: &str = "{version}";

impl Template {
    /// Parse a template string. The error is a message for the caller to wrap
    /// in a `MindToml` error (VER-2).
    // spec: VER-1 VER-2
    pub fn parse(s: &str) -> Result<Template, String> {
        let count = s.matches(PLACEHOLDER).count();
        if count != 1 {
            return Err(format!(
                "release-tags {s:?} must contain exactly one {PLACEHOLDER} placeholder, found {count}"
            ));
        }
        let (prefix, suffix) = s.split_once(PLACEHOLDER).expect("one placeholder");
        if [prefix, suffix]
            .iter()
            .any(|p| p.contains('{') || p.contains('}'))
        {
            return Err(format!(
                "release-tags {s:?} may contain no token other than {PLACEHOLDER}"
            ));
        }
        // Only a whole tag name reaches git, so check the template filled in
        // with a sample version: a suffix like `-final` is fine, a leading `-`
        // on the tag is not.
        crate::git::validate_ref_value(&format!("{prefix}0.0.0{suffix}"))
            .map_err(|e| format!("release-tags {s:?}: {e}"))?;
        Ok(Template::Literal {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        })
    }

    /// The version a tag names under this template, or `None` when the tag is
    /// not a release (VER-10).
    // spec: VER-10
    pub fn match_tag(&self, tag: &str) -> Option<Version> {
        self.remainder(tag).and_then(Version::parse)
    }

    /// Whether `tag` fits the template's prefix and suffix but its remainder
    /// is not semver: skipped on resolution, reported by `review` (VER-10).
    pub fn is_near_miss(&self, tag: &str) -> bool {
        self.remainder(tag)
            .is_some_and(|r| !r.is_empty() && Version::parse(r).is_none())
    }

    fn remainder<'a>(&self, tag: &'a str) -> Option<&'a str> {
        match self {
            Template::Default => Some(tag.strip_prefix('v').unwrap_or(tag)),
            Template::Literal { prefix, suffix } => tag
                .strip_prefix(prefix.as_str())?
                .strip_suffix(suffix.as_str()),
        }
    }
}

impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Template::Default => f.write_str("true"),
            Template::Literal { prefix, suffix } => write!(f, "{prefix}{PLACEHOLDER}{suffix}"),
        }
    }
}

/// A version range (VER-20): a caret requirement or an exact version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    /// `^X`, `^X.Y`, `^X.Y.Z` (or the bare form). Missing components are `None`.
    Caret {
        major: u64,
        minor: Option<u64>,
        patch: Option<u64>,
        pre: Vec<String>,
    },
    /// `=X.Y.Z`.
    Exact(Version),
}

impl Range {
    /// Parse a range. `None` for anything else, which the caller reports as
    /// `BadPinSpec` (VER-22).
    // spec: VER-20 VER-22
    pub fn parse(s: &str) -> Option<Range> {
        if let Some(v) = s.strip_prefix('=') {
            return Version::parse(v).map(Range::Exact);
        }
        let body = s.strip_prefix('^').unwrap_or(s);
        // Build metadata means nothing in a requirement.
        if body.contains('+') {
            return None;
        }
        let (core, pre) = match body.split_once('-') {
            Some((c, p)) => {
                if !valid_identifiers(p, true) {
                    return None;
                }
                (c, p.split('.').map(str::to_string).collect::<Vec<_>>())
            }
            None => (body, Vec::new()),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        let nums: Option<Vec<u64>> = parts.iter().map(|p| numeric(p)).collect();
        let nums = nums?;
        // A prerelease only makes sense on a full version.
        if !pre.is_empty() && nums.len() != 3 {
            return None;
        }
        Some(Range::Caret {
            major: nums[0],
            minor: nums.get(1).copied(),
            patch: nums.get(2).copied(),
            pre,
        })
    }

    /// Whether `v` satisfies the range, with cargo's caret and prerelease
    /// rules (VER-20, VER-21).
    // spec: VER-20 VER-21
    pub fn matches(&self, v: &Version) -> bool {
        match self {
            Range::Exact(want) => v == want,
            Range::Caret {
                major,
                minor,
                patch,
                pre,
            } => {
                let lower = Version {
                    major: *major,
                    minor: minor.unwrap_or(0),
                    patch: patch.unwrap_or(0),
                    pre: pre.clone(),
                    build: None,
                };
                if *v < lower {
                    return false;
                }
                // The upper bound: the leftmost nonzero given component may
                // not change.
                let in_bounds = match (major, minor, patch) {
                    (0, None, _) => v.major == 0,
                    (0, Some(0), None) => v.major == 0 && v.minor == 0,
                    (0, Some(0), Some(p)) => v.triple() == (0, 0, *p),
                    (0, Some(m), _) => v.major == 0 && v.minor == *m,
                    (x, _, _) => v.major == *x,
                };
                if !in_bounds {
                    return false;
                }
                // A prerelease is admitted only when the range names one, and
                // then only for that same MAJOR.MINOR.PATCH.
                !v.is_prerelease() || (!pre.is_empty() && v.triple() == lower.triple())
            }
        }
    }

    /// The caret range a breaking move rewrites the pin to (VER-62), and the
    /// default pin a release-tagged source gets on `meld` (VER-50).
    pub fn caret_of(v: &Version) -> Range {
        Range::Caret {
            major: v.major,
            minor: Some(v.minor),
            patch: Some(v.patch),
            pre: v.pre.clone(),
        }
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Range::Exact(v) => write!(f, "={v}"),
            Range::Caret {
                major,
                minor,
                patch,
                pre,
            } => {
                write!(f, "^{major}")?;
                if let Some(m) = minor {
                    write!(f, ".{m}")?;
                }
                if let Some(p) = patch {
                    write!(f, ".{p}")?;
                }
                if !pre.is_empty() {
                    write!(f, "-{}", pre.join("."))?;
                }
                Ok(())
            }
        }
    }
}

/// One release found on the remote: its tag, its version, and the commit the
/// tag peels to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub commit: String,
}

/// The releases among `(tag, commit)` pairs under `template`, highest first,
/// with VER-11 duplicates collapsed. A version whose duplicate tags peel to
/// different commits is ambiguous: it is dropped, and the conflicting tags are
/// returned so the caller can warn.
// spec: VER-10 VER-11
pub fn releases(
    template: &Template,
    tags: &[(String, String)],
) -> (Vec<Release>, Vec<(String, String)>) {
    let mut found: Vec<Release> = tags
        .iter()
        .filter_map(|(tag, commit)| {
            template.match_tag(tag).map(|version| Release {
                tag: tag.clone(),
                version,
                commit: commit.clone(),
            })
        })
        .collect();
    // Highest first; a stable tiebreak on the tag name keeps the output
    // deterministic.
    found.sort_by(|a, b| b.version.cmp(&a.version).then_with(|| a.tag.cmp(&b.tag)));
    let mut out: Vec<Release> = Vec::new();
    let mut ambiguous = Vec::new();
    let mut i = 0;
    while i < found.len() {
        let mut j = i + 1;
        while j < found.len() && found[j].version == found[i].version {
            j += 1;
        }
        let group = &found[i..j];
        match group.iter().find(|r| r.commit != group[0].commit) {
            Some(other) => ambiguous.push((group[0].tag.clone(), other.tag.clone())),
            None => out.push(group[0].clone()),
        }
        i = j;
    }
    (out, ambiguous)
}

/// The highest release in `releases` (already highest first) that `range`
/// admits and `yanked` does not list (VER-46).
// spec: VER-46
pub fn select<'a>(
    releases: &'a [Release],
    range: &Range,
    yanked: &[Version],
) -> Option<&'a Release> {
    releases
        .iter()
        .find(|r| range.matches(&r.version) && !yanked.contains(&r.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s:?} should parse"))
    }

    fn r(s: &str) -> Range {
        Range::parse(s).unwrap_or_else(|| panic!("{s:?} should parse as a range"))
    }

    #[test]
    fn version_parses_strict_semver() {
        // spec: VER-10
        for good in [
            "0.0.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-rc.1",
            "1.0.0+b.5",
            "1.0.0-a-b.0+x",
        ] {
            assert_eq!(v(good).to_string(), good);
        }
        for bad in [
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-01",
            "1.2.3-a..b",
            "v1.2.3",
            "",
            "1.2.x",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn version_orders_by_precedence_ignoring_build() {
        // spec: VER-11
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.10.0",
            "2.0.0",
        ];
        for w in ordered.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.2.3+a"), v("1.2.3+b"));
        assert_eq!(v("1.2.3+a"), v("1.2.3"));
    }

    #[test]
    fn template_parses_and_rejects() {
        // Template parsing for VER-1/VER-2; reading it from mind.toml is not
        // wired yet, so the IDs stay allowlisted.
        assert_eq!(
            Template::parse("v{version}").unwrap(),
            Template::Literal {
                prefix: "v".into(),
                suffix: String::new()
            }
        );
        assert_eq!(
            Template::parse("skills-v{version}-final").unwrap(),
            Template::Literal {
                prefix: "skills-v".into(),
                suffix: "-final".into()
            }
        );
        assert!(Template::parse("{version}").is_ok());
        for bad in [
            "v",
            "",
            "{version}-{version}",
            "{name}-{version}",
            "v{version}{x}",
            "-v{version}",
            "a b{version}",
            "a..{version}",
        ] {
            assert!(Template::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn template_matches_tags() {
        // spec: VER-10
        let t = Template::parse("skills-v{version}").unwrap();
        assert_eq!(t.match_tag("skills-v1.2.3"), Some(v("1.2.3")));
        assert_eq!(t.match_tag("v1.2.3"), None);
        assert_eq!(t.match_tag("skills-v1.2"), None);
        assert!(t.is_near_miss("skills-v1.2"));
        assert!(t.is_near_miss("skills-v1.2.3.4"));
        assert!(!t.is_near_miss("skills-v1.2.3"));
        assert!(!t.is_near_miss("other"));

        let d = Template::Default;
        assert_eq!(d.match_tag("v1.2.3"), Some(v("1.2.3")));
        assert_eq!(d.match_tag("1.2.3"), Some(v("1.2.3")));
        assert_eq!(d.match_tag("release-1.2.3"), None);
    }

    #[test]
    fn range_parses_and_rejects() {
        // spec: VER-20
        for (input, shown) in [
            ("^1", "^1"),
            ("^1.2", "^1.2"),
            ("^1.2.3", "^1.2.3"),
            ("1.2", "^1.2"),
            ("=1.2.3", "=1.2.3"),
            ("^1.3.0-rc.1", "^1.3.0-rc.1"),
        ] {
            assert_eq!(r(input).to_string(), shown);
        }
        for bad in [
            "",
            "^",
            "v1",
            "~1.2",
            ">=1",
            "^1.2.3.4",
            "=1.2",
            "^1.2-rc.1",
            "^1.2.3+b",
            "main",
        ] {
            assert!(Range::parse(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn caret_follows_cargo() {
        // spec: VER-20
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("^1.2", &["1.2.0", "1.9.9"], &["1.1.9", "2.0.0"]),
            ("^1", &["1.0.0", "1.99.0"], &["0.9.0", "2.0.0"]),
            ("^1.2.3", &["1.2.3", "1.3.0"], &["1.2.2", "2.0.0"]),
            ("^0.3", &["0.3.0", "0.3.9"], &["0.2.9", "0.4.0"]),
            ("^0.3.1", &["0.3.1", "0.3.5"], &["0.3.0", "0.4.0"]),
            ("^0.0.3", &["0.0.3"], &["0.0.4", "0.0.2", "0.1.0"]),
            ("^0.0", &["0.0.0", "0.0.9"], &["0.1.0"]),
            ("^0", &["0.0.0", "0.9.0"], &["1.0.0"]),
            ("=1.2.3", &["1.2.3", "1.2.3+b"], &["1.2.4", "1.2.2"]),
        ];
        for (range, yes, no) in cases {
            let rg = r(range);
            for x in *yes {
                assert!(rg.matches(&v(x)), "{range} should admit {x}");
            }
            for x in *no {
                assert!(!rg.matches(&v(x)), "{range} should refuse {x}");
            }
        }
    }

    #[test]
    fn prereleases_need_an_opt_in() {
        // spec: VER-21
        assert!(!r("^1.2").matches(&v("1.3.0-rc.1")));
        let rc = r("^1.3.0-rc.1");
        assert!(rc.matches(&v("1.3.0-rc.2")));
        assert!(rc.matches(&v("1.3.0")));
        assert!(rc.matches(&v("1.4.0")));
        assert!(!rc.matches(&v("1.3.0-rc.0")));
        assert!(!rc.matches(&v("1.4.0-rc.1")));
        assert!(r("=1.3.0-rc.1").matches(&v("1.3.0-rc.1")));
    }

    #[test]
    fn releases_dedupe_and_flag_ambiguity() {
        // spec: VER-11
        let tags: Vec<(String, String)> = [
            ("v1.0.0", "aaa"),
            ("1.0.0", "aaa"),
            ("v1.1.0", "bbb"),
            ("1.1.0", "ccc"),
            ("v2.0.0-rc.1", "ddd"),
            ("v1.2", "eee"),
            ("junk", "fff"),
        ]
        .iter()
        .map(|(t, c)| (t.to_string(), c.to_string()))
        .collect();
        let (rels, ambiguous) = releases(&Template::Default, &tags);
        let shown: Vec<String> = rels.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(shown, ["2.0.0-rc.1", "1.0.0"]);
        assert_eq!(ambiguous, [("1.1.0".to_string(), "v1.1.0".to_string())]);
    }

    #[test]
    fn select_skips_yanked_and_out_of_range() {
        // The yanked filter VER-46 uses; reading the list is not wired yet.
        let tags: Vec<(String, String)> = ["v1.0.0", "v1.1.0", "v1.2.0", "v2.0.0"]
            .iter()
            .map(|t| (t.to_string(), format!("c-{t}")))
            .collect();
        let (rels, _) = releases(&Template::Default, &tags);
        let pick = select(&rels, &r("^1"), &[]).unwrap();
        assert_eq!(pick.tag, "v1.2.0");
        let pick = select(&rels, &r("^1"), &[v("1.2.0")]).unwrap();
        assert_eq!(pick.tag, "v1.1.0");
        assert!(select(&rels, &r("^3"), &[]).is_none());
    }

    #[test]
    fn caret_of_rewrites_to_the_new_major() {
        // The range rewrite an accepted breaking move applies (VER-62).
        assert_eq!(Range::caret_of(&v("2.0.0")).to_string(), "^2.0.0");
        assert!(Range::caret_of(&v("2.0.0")).matches(&v("2.3.0")));
    }
}
