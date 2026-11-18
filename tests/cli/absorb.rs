//! `absorb` (spec/absorb.md).

use super::*;

#[test]
fn example_absorb_claims_unmanaged_item() {
    // spec: ABS-1, ABS-8, UNM-1
    // The absorb example is a README-only walkthrough, so build the scenario
    // directly: an unmanaged lobe skill (UNM-1) plus a throwaway git target.
    // `absorb --yes` moves the item to the target's convention path, commits,
    // melds, and learns it (ABS-1); afterward it is an ordinary managed item
    // (ABS-8): a managed lobe symlink and a version-controlled file in the target.
    let sb = Sandbox::new();

    // Seed an unmanaged skill placed directly in the lobe (the seed_unmanaged
    // pattern), with the example's `notes` name and frontmatter.
    write(
        &sb.claude_home.join("skills/notes/SKILL.md"),
        "---\ndescription: my personal notes skill\n---\n# notes\n",
    );

    // recall (before): notes is surfaced as unmanaged (UNM-1).
    let before = sb.mind(&["--ascii", "recall"]);
    assert!(before.success, "{}", before.stderr);
    assert!(
        before.stdout.contains("unmanaged: not installed by mind"),
        "recall must surface the unmanaged group before absorb: {}",
        before.stdout
    );
    assert!(
        before.stdout.contains("skill:notes"),
        "recall must list notes as unmanaged before absorb: {}",
        before.stdout
    );

    // A throwaway git target the user owns.
    let target = sb.base.join("absorb-target");
    std::fs::create_dir_all(&target).unwrap();
    git(&target, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&target, &["config", "user.email", "t@t"]);
    git(&target, &["config", "user.name", "t"]);
    git(&target, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let target_spec = target.to_string_lossy().into_owned();

    // absorb --yes: move, commit, meld, learn (ABS-1).
    let absorb = sb.mind(&[
        "--ascii",
        "absorb",
        "skill:notes",
        "--to",
        &target_spec,
        "--yes",
    ]);
    assert!(absorb.success, "{} {}", absorb.stdout, absorb.stderr);
    assert!(
        absorb
            .stdout
            .contains("absorbed skill:notes -> managed as skill:notes"),
        "absorb must report the managed result: {}",
        absorb.stdout
    );

    // The file now lives in the target repo at the convention path.
    assert!(
        target.join("skills/notes/SKILL.md").exists(),
        "the absorbed file must live in the target at skills/notes/SKILL.md"
    );

    // The lobe path is now a managed symlink into the store (ABS-8).
    let lobe_path = sb.claude_home.join("skills/notes");
    let meta = std::fs::symlink_metadata(&lobe_path).expect("lobe path must exist after absorb");
    assert!(
        meta.file_type().is_symlink(),
        "the lobe path must be a managed symlink after absorb"
    );
    assert_eq!(
        std::fs::canonicalize(&lobe_path).unwrap(),
        std::fs::canonicalize(sb.mind_home.join("store/skill/notes")).unwrap(),
        "the lobe link must point into the store"
    );

    // recall (after): notes is now a managed installed item, not unmanaged (ABS-8).
    let after = sb.mind(&["--ascii", "recall"]);
    assert!(after.success, "{}", after.stderr);
    let line = after
        .stdout
        .lines()
        .find(|l| l.contains("skill:notes"))
        .unwrap_or_else(|| panic!("no notes line in recall output: {}", after.stdout));
    assert!(
        line.contains("installed @"),
        "notes must be a managed installed item after absorb: {line:?}"
    );
    assert!(
        !after.stdout.contains("unmanaged: not installed by mind"),
        "notes must no longer be reported as unmanaged after absorb: {}",
        after.stdout
    );
}
