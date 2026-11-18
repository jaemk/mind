//! `curate` (spec/curate.md).

use super::*;

#[test]
fn example_super_source_curate_reports_up_to_date() {
    // spec: CUR-1
    // The README's "Reconciling with `mind curate`" section says the curated
    // chain reads clean right after melding, since a plain `meld --yes`
    // already registers and installs everything the curator's entries declare
    // (the same work `curate` would otherwise do for a newly listed entry) --
    // this is what proves that claim against the shipped example, not a
    // hand-built stand-in.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-it-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let sb = Sandbox {
        base: base.clone(),
        source: base.clone(),
        mind_home: base.join("mind"),
        claude_home: base.join("claude"),
    };
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/super-source");
    let meld = sb.mind(&["meld", dir.to_str().unwrap(), "--yes"]);
    assert!(
        meld.success,
        "super-source example meld failed:\nstdout: {}\nstderr: {}",
        meld.stdout, meld.stderr
    );

    let r = sb.mind(&["curate", "--check"]);
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("up to date"),
        "the shipped super-source example must be curate-clean right after melding: {}",
        r.stdout
    );
}
