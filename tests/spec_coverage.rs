//! Spec-coverage gate.
//!
//! Every normative spec ID defined in `spec/*.md` (as a `- `ID` ...` list item)
//! must either be cited by a test (`// spec: ID` comments in `src/` or `tests/`)
//! or appear in the ALLOWLIST below. This fails the build when a new spec
//! requirement is added without a coverage decision, so coverage cannot silently
//! regress. See spec/README.md.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Spec IDs intentionally not cited by a dedicated test: structural invariants
/// and schema facts exercised indirectly by many tests, or secondary behaviors
/// not yet given their own test. To add a NEW spec ID, either cite it from a
/// test or add it here with a reason.
const ALLOWLIST: &[&str] = &[
    // Storage layout and JSON schema invariants, exercised by every test that
    // reads/writes the registry or manifest, or installs an item.
    "STO-1", "STO-3", "STO-10", "STO-11", "STO-12", "STO-20", "STO-21", "STO-22", "STO-23",
    "STO-30", "STO-31",
    // Lifecycle invariants covered indirectly: swap mechanics, idempotent
    // reinstall, source-hash basis, removing an absent path.
    "LIFE-3", "LIFE-6", "LIFE-15", "LIFE-21",
    // Namespacing: install-time application and the token's written form are
    // definitional, exercised by the expansion tests.
    "NS-3", "NS-10", // Discovery edge: missing directories yield no items.
    "DSC-13",
    // Planned features (see spec/README.md feature status = planned): documented
    // with stable IDs ahead of implementation. Each must move to a citing test
    // when built, at which point it is removed from this allowlist.
    //   scan roots (subtree/monorepo sources)
    "DSC-50", "DSC-51", "DSC-52", "DSC-53", "STO-17", "CLI-16",
    //   version pinning
    "DSC-41", "STO-18", "CLI-17", "CLI-18", "CLI-55",
    //   review verb (author-side source validation)
    "CLI-130", "CLI-131", "CLI-132", "CLI-133",
    //   probe matches description text
    "CLI-85",
    //   self-update verb (in-place binary upgrade via the self_update crate)
    "CLI-140", "CLI-141", "CLI-142", "CLI-143",
    //   interactive TUI (probe default; see spec/tui.md)
    "TUI-1", "TUI-2", "TUI-10", "TUI-11", "TUI-12", "TUI-13", "TUI-14", "TUI-15",
    "TUI-20", "TUI-21", "TUI-22", "TUI-23", "TUI-24", "TUI-25",
    "TUI-30", "TUI-31", "TUI-40", "TUI-41",
];

#[test]
fn every_spec_id_is_cited_or_allowlisted() {
    let defined = defined_ids();
    assert!(
        defined.len() > 50,
        "found only {} spec IDs; the parser or spec layout likely changed",
        defined.len()
    );
    let cited = cited_ids();
    let allow: BTreeSet<String> = ALLOWLIST.iter().map(|s| s.to_string()).collect();

    // Every ID a test cites must be defined in the spec (catches typos and
    // behavior added without a spec entry).
    let undefined: Vec<_> = cited.difference(&defined).cloned().collect();
    assert!(
        undefined.is_empty(),
        "tests cite spec IDs not defined in spec/ (document them): {undefined:?}"
    );

    // The allowlist must not rot: every entry must be a real defined ID.
    let stale: Vec<_> = allow.difference(&defined).cloned().collect();
    assert!(
        stale.is_empty(),
        "ALLOWLIST references unknown spec IDs: {stale:?}"
    );

    // Keep the allowlist tight: a now-cited ID should be removed from it.
    let redundant: Vec<_> = allow.intersection(&cited).cloned().collect();
    assert!(
        redundant.is_empty(),
        "these IDs are now cited by tests; remove them from ALLOWLIST: {redundant:?}"
    );

    let uncovered: Vec<_> = defined
        .iter()
        .filter(|id| !cited.contains(*id) && !allow.contains(*id))
        .cloned()
        .collect();
    assert!(
        uncovered.is_empty(),
        "spec IDs with no test citation (add a test that cites them, or ALLOWLIST them): {uncovered:?}"
    );
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// True for tokens shaped like a spec ID: 2-4 uppercase letters, `-`, digits.
fn is_id(tok: &str) -> bool {
    match tok.split_once('-') {
        Some((alpha, num)) => {
            (2..=4).contains(&alpha.len())
                && alpha.bytes().all(|b| b.is_ascii_uppercase())
                && !num.is_empty()
                && num.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

/// IDs defined in the spec: the backticked token leading a `- ` list item.
fn defined_ids() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for md in files_with_ext(&root().join("spec"), "md") {
        let text = std::fs::read_to_string(&md).unwrap();
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("- `")
                && let Some(end) = rest.find('`')
            {
                let tok = &rest[..end];
                if is_id(tok) {
                    out.insert(tok.to_string());
                }
            }
        }
    }
    out
}

/// IDs cited in `src/` and `tests/` via `// spec:` comments, excluding this file.
/// Only text after a `// spec:` marker is scanned, so incidental tokens like
/// "UTF-8" in prose are not mistaken for IDs.
fn cited_ids() -> BTreeSet<String> {
    const MARKER: &str = "// spec:";
    let mut out = BTreeSet::new();
    let mut sources = files_with_ext(&root().join("src"), "rs");
    sources.extend(files_with_ext(&root().join("tests"), "rs"));
    for f in sources {
        if f.file_name().is_some_and(|n| n == "spec_coverage.rs") {
            continue; // don't count the ALLOWLIST literals as citations
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for line in text.lines() {
            if let Some(idx) = line.find(MARKER) {
                for tok in id_tokens(&line[idx + MARKER.len()..]) {
                    out.insert(tok);
                }
            }
        }
    }
    out
}

/// Extract maximal `[A-Za-z0-9-]` runs that look like spec IDs.
fn id_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            cur.push(c);
        } else {
            if is_id(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    if is_id(&cur) {
        out.push(cur);
    }
    out
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
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
