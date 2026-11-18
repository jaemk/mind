//! `dump` (spec/dump.md).

use super::*;

#[test]
fn dump_emits_flat_skills_for_flat_source() {
    // spec: DUMP-10
    // A source melded with --flat-skills (the consumer override STO-44) is dumped
    // with `flat-skills = true` on its [discover].sources entry, so re-melding the
    // output reproduces the flat layout. A separate non-flat source emits no key.
    let flat = make_flat_source("dump-flat", None);
    let flat_spec = flat.source_spec();
    assert!(
        flat.mind(&["meld", &flat_spec, "--flat-skills", "--yes"])
            .success
    );

    let dump = flat.mind(&["dump"]);
    assert!(dump.success, "dump failed: {} {}", dump.stdout, dump.stderr);
    assert!(
        dump.stdout.contains("flat-skills = true"),
        "dump must emit flat-skills = true for a flat source: {}",
        dump.stdout
    );

    // Control: a conventional source dumps with no flat-skills key.
    let normal = Sandbox::new();
    let normal_spec = normal.source_spec();
    assert!(normal.mind(&["meld", &normal_spec, "--yes"]).success);
    let dump2 = normal.mind(&["dump"]);
    assert!(
        !dump2.stdout.contains("flat-skills"),
        "a non-flat source must emit no flat-skills key: {}",
        dump2.stdout
    );
}

// ---------------------------------------------------------------------------
// NS-25: reserved kind word rejected as a namespace prefix (CLI path)
// ---------------------------------------------------------------------------

#[test]
fn dump_records_prefix_and_bare_install_items() {
    // spec: DUMP-5
    // A prefixed source's dump must carry the prefix (`namespace = "jk"`, the
    // canonical DSC-78 key) so re-melding reproduces the namespace, while
    // install-items stay in source/catalog truth (bare `skill:review`), never the
    // install-time `skill:jk:review` form. A proper subset (only `review` of
    // {review, dev, style}) forces the install-items listing rather than
    // `install = true`.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);

    let dump = sb.mind(&["dump"]);
    assert!(dump.success, "dump failed: {} {}", dump.stdout, dump.stderr);
    assert!(
        dump.stdout.contains("namespace = \"jk\""),
        "dump must record the effective prefix as `namespace = \"jk\"` (DSC-78): {}",
        dump.stdout
    );
    assert!(
        dump.stdout.contains("\"skill:review\""),
        "install-items must list the BARE ref skill:review: {}",
        dump.stdout
    );
    assert!(
        !dump.stdout.contains("skill:jk:review") && !dump.stdout.contains("jk:review"),
        "install-items must NOT carry the prefixed effective name: {}",
        dump.stdout
    );

    // The dumped document must parse as a mind.toml (re-meldable super-source).
    let parsed: Result<toml::Value, _> = toml::from_str(&dump.stdout);
    assert!(
        parsed.is_ok(),
        "dump output must be valid TOML: {:?}\n{}",
        parsed.err(),
        dump.stdout
    );
}

// ---------------------------------------------------------------------------
// NS-25: the reserved-kind-word prefix guard fires on the re-meld `--as` path
// (CLI-13) and for every reserved word, not just `skill`/`agent`.
// ---------------------------------------------------------------------------
