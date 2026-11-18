//! `introspect` (spec/cli.md).

use super::*;

#[test]
fn introspect_is_clean_after_learn() {
    // spec: CLI-90
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["introspect"]);
    assert!(r.success);
    assert!(r.stdout.contains("all good"), "{}", r.stdout);
}

#[test]
fn introspect_fix_relinks_missing_symlink() {
    // spec: CLI-91
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let link = sb.claude_home.join("skills/review");
    std::fs::remove_file(&link).unwrap();

    let r = sb.mind(&["introspect", "--fix"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("relinked"), "{}", r.stdout);

    // The link is back and introspect is now clean.
    assert!(std::fs::symlink_metadata(&link).is_ok());
    assert!(sb.mind(&["introspect"]).stdout.contains("all good"));
}

#[test]
fn introspect_json_emits_report() {
    // spec: CLI-92, CLI-189
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean: schema=1, empty issues array, integer counts for sources and items.
    let clean = sb.mind(&["introspect", "--json"]).stdout;
    assert!(clean.trim_start().starts_with('{'), "{clean}");
    let v: serde_json::Value = serde_json::from_str(&clean).expect("valid JSON");
    assert_eq!(v["schema"], 1, "schema field must be 1: {clean}");
    assert!(v["issues"].is_array(), "issues must be an array: {clean}");
    assert!(
        v["sources"].is_number(),
        "sources must be an integer count: {clean}"
    );
    assert!(
        v["items"].is_number(),
        "items must be an integer count: {clean}"
    );

    // A broken link surfaces as a missing-link issue with its stable kind tag.
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();
    let broken = sb.mind(&["introspect", "--json"]).stdout;
    let bv: serde_json::Value = serde_json::from_str(&broken).expect("valid JSON");
    assert_eq!(
        bv["schema"], 1,
        "schema present on error report too: {broken}"
    );
    assert!(broken.contains("\"missing-link\""), "{broken}");
}

#[test]
fn introspect_survives_a_source_scan_failure_and_still_reports_the_healthy_source() {
    // spec: CLI-210 -- a source `introspect` cannot scan (here: its clone/
    // working tree vanished) must not abort the whole run: the run completes,
    // reports a `source-scan-failed` issue naming the broken source, AND still
    // computes findings for the healthy source from the partial catalog (a
    // `drifted` finding, CLI-90, which requires the healthy source's items to
    // have actually been scanned).
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // A second local source, melded then broken by deleting its working tree
    // (it is an unpinned local source, so `clone_dir` IS the working tree).
    let broken = Sandbox::bare("broken-src");
    broken.write_and_commit(
        "skills/x/SKILL.md",
        "---\nname: x\ndescription: x skill\n---\n# x\n",
    );
    assert!(sb.mind(&["meld", &broken.source_spec()]).success);
    std::fs::remove_dir_all(&broken.source).unwrap();

    // Drift the healthy source so a `drifted` finding is expected too.
    sb.edit_source();

    let r = sb.mind(&["introspect", "--json"]);
    let v: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("introspect must still emit valid JSON");
    let issues = v["issues"].as_array().expect("issues must be an array");

    let scan_failed = issues
        .iter()
        .find(|i| i["kind"] == "source-scan-failed")
        .unwrap_or_else(|| panic!("must report source-scan-failed: {}", r.stdout));
    assert!(
        scan_failed["target"]
            .as_str()
            .unwrap()
            .contains("broken-src"),
        "source-scan-failed must name the broken source: {}",
        r.stdout
    );

    assert!(
        issues
            .iter()
            .any(|i| i["kind"] == "drifted" && i["target"] == "skill:review"),
        "the healthy source's drift must still be reported: {}",
        r.stdout
    );
}
