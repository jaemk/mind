//! Unmanaged lobe items (spec/unmanaged.md).

use super::*;

/// The unmanaged-item counterpart: a hostile name that never goes through
/// catalog scanning (DSC-96 only gates catalog items) is nonetheless caught by
/// UNM-9's own safety check on the NAME DERIVED from an unmanaged lobe entry's
/// filename, at scan time. UNM-9's severity is a per-entry skip (with a
/// warning), the same as DSC-96 gives a hostile catalog-scanned name, so the
/// entry never enters the scan's result at all: `forget` of such a ref sees no
/// matching item (`NotInstalled`), never reaching UNM-5's "not managed by
/// mind" disclosure for it -- with or without `--yes`, since the entry was
/// never a candidate to begin with. The scan's own warning still names the
/// entry with DSC-95 sanitizing applied, so the hostile bytes never reach any
/// output either way.
#[test]
fn hostile_unmanaged_item_name_is_skipped_via_unm9() {
    // spec: UNM-9 DSC-95
    let sb = melded();
    // See the sibling managed-item test for why this avoids '[': it would
    // misroute `forget` through its glob-selection path (`resolve::is_glob`),
    // which never falls through to the unmanaged-item resolution this test
    // means to exercise.
    let unmanaged_name = format!("hand{}ma\u{202E}de", '\x07');
    let unmanaged_bare = format!("skill:hand{}ma\u{202E}de", '\x07');
    let dir = sb.claude_home.join("skills").join(&unmanaged_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    )
    .unwrap();

    let forget_unmanaged = sb.mind(&["forget", &unmanaged_bare]);
    assert!(
        !forget_unmanaged.success,
        "a hostile derived name must never resolve as a forgettable item: {}",
        forget_unmanaged.stdout
    );
    // The scan's own warning line is source-controlled data and must stay
    // sanitized (DSC-95); the `NotInstalled` error below it is free to echo
    // the raw ref back, since that is the literal text the user themselves
    // typed on the command line (the same carve-out `forget --json`'s
    // `target` field gets elsewhere in this file), not source-controlled data
    // newly introduced by this refusal.
    let warning_line = forget_unmanaged
        .stderr
        .lines()
        .find(|l| l.starts_with("warning:"))
        .unwrap_or_else(|| {
            panic!(
                "expected a scan warning line: {:?}",
                forget_unmanaged.stderr
            )
        });
    assert!(
        !warning_line.contains('\x1b') && !warning_line.contains('\u{202E}'),
        "the scan's own warning must not leak raw ANSI/bidi: {:?}",
        warning_line
    );
    assert!(
        warning_line.contains("unsafe") && warning_line.contains("handmade"),
        "the scan's own warning must name the cause and the sanitized name: {:?}",
        warning_line
    );
    assert!(
        forget_unmanaged
            .stderr
            .to_lowercase()
            .contains("not installed")
            || forget_unmanaged
                .stderr
                .to_lowercase()
                .contains("notinstalled"),
        "the ref must fail to resolve, having been skipped out of the scan: {:?}",
        forget_unmanaged.stderr
    );
    assert!(dir.exists(), "a skipped entry must be left in place");

    // `--yes` does not surface it either: the entry was never a candidate.
    let forget_yes = sb.mind(&["forget", &unmanaged_bare, "--yes"]);
    assert!(
        !forget_yes.success,
        "--yes cannot forget an entry that was never a candidate: {}",
        forget_yes.stdout
    );
    assert!(dir.exists(), "the unmanaged item must not be removed");
}

// M7: `absorb`'s reported `kind:name` key (`absorb_effective_key` in
// `src/commands/`) is built from the unmanaged item's on-disk NAME, which
// comes off the lobe filesystem and is NOT DSC-96-gated (that gate only runs
// at catalog-scan time for a managed source's items, not for an unmanaged
// lobe entry `absorb` claims). A CLI-level drive of this with a genuinely
// hostile (control/bidi) name cannot actually reach the fix: `absorb`'s
// internal `learn()` call re-scans the destination source's catalog, and
// DSC-96 skips a hostile-named item there before `absorb` ever reaches the
// success path this key is built for (confirmed: driving `absorb` on such a
// name here fails with `ItemNotFound`, not a leaked escape). The fix is
// covered directly instead by the `absorb_effective_key_sanitizes_a_hostile_name`
// unit test in `src/commands/` (spec: DSC-95).

#[test]
fn recall_shows_unmanaged_lobe_items() {
    // spec: UNM-1 UNM-2
    let sb = melded();
    seed_unmanaged(&sb);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("unmanaged: not installed by mind"),
        "recall must surface an unmanaged group: {}",
        r.stdout
    );
    assert!(r.stdout.contains("skill:handmade"), "{}", r.stdout);
    assert!(r.stdout.contains("agent:custom"), "{}", r.stdout);
}

#[test]
fn recall_excludes_managed_links_from_unmanaged() {
    // spec: UNM-1
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["recall"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stdout.contains("unmanaged: not installed by mind"),
        "a mind-installed link must not be reported as unmanaged: {}",
        r.stdout
    );
}

#[test]
fn probe_lists_and_searches_unmanaged_items() {
    // spec: UNM-3
    let sb = melded();
    seed_unmanaged(&sb);
    // The non-interactive listing includes the unmanaged item, marked.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("skill:handmade") && r.stdout.contains("(unmanaged)"),
        "probe listing must mark the unmanaged item: {}",
        r.stdout
    );
    // The substring search matches its name (CLI-85).
    let s = sb.mind(&["probe", "handmade", "--no-tui"]);
    assert!(
        s.stdout.contains("skill:handmade"),
        "search must find the unmanaged item: {}",
        s.stdout
    );
    // JSON carries the unmanaged flag; managed rows omit it.
    let j = sb.mind(&["probe", "handmade", "--json"]);
    assert!(
        j.stdout.contains("\"unmanaged\": true"),
        "json must flag the unmanaged row: {}",
        j.stdout
    );
}

#[test]
fn forget_unmanaged_removes_after_warning() {
    // spec: UNM-4 UNM-5
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    assert!(skill.is_dir());
    let r = sb.mind(&["forget", "skill:handmade", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("not managed by mind"),
        "the removal must state it is unmanaged: {}",
        r.stdout
    );
    assert!(!skill.exists(), "the unmanaged skill dir must be removed");
}

#[test]
fn forget_unmanaged_refuses_without_yes_in_non_tty() {
    // spec: UNM-5
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // No --yes, non-TTY: refuse and remove nothing, after stating it is unmanaged.
    let r = sb.mind(&["forget", "skill:handmade"]);
    assert!(!r.success, "must refuse without --yes: {}", r.stdout);
    assert!(
        r.stdout.contains("not managed by mind"),
        "must state it is unmanaged: {}",
        r.stdout
    );
    assert!(skill.exists(), "nothing may be removed on refusal");
}

#[test]
fn forget_glob_never_sweeps_unmanaged() {
    // spec: UNM-4
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // A glob matches managed items only; with none installed it removes nothing
    // and must never touch the unmanaged skill.
    let _ = sb.mind(&["forget", "*", "--yes"]);
    assert!(
        skill.exists(),
        "a glob forget must never delete an unmanaged item"
    );
}

// --- UNM-7/UNM-8: forget --unmanaged bulk removal ---------------------------

/// `forget --unmanaged 'skill:*' --yes` removes every unmanaged skill.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_kind_glob_removes_matching() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");
    // skill:* removes the skill, not the agent.
    let r = sb.mind(&["forget", "--unmanaged", "skill:*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.to_lowercase().contains("not managed by mind"),
        "must state items are not managed: {}",
        r.stdout
    );
    assert!(!skill.exists(), "the unmanaged skill dir must be removed");
    assert!(agent.exists(), "the unmanaged agent must be untouched");
}

/// `forget --unmanaged --yes` (no ref) removes ALL unmanaged items.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_no_ref_removes_all() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");
    let r = sb.mind(&["forget", "--unmanaged", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!skill.exists(), "handmade skill must be removed");
    assert!(!agent.exists(), "custom agent must be removed");
}

/// A MANAGED installed item is never matched by `forget --unmanaged '*' --yes`.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_never_removes_managed_items() {
    let sb = melded();
    // Install a managed item.
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let managed_link = sb.claude_home.join("skills/review");
    assert!(managed_link.exists(), "managed link must exist after learn");

    // Also place an unmanaged skill.
    let unmanaged_skill = sb.claude_home.join("skills/handmade");
    write(
        &unmanaged_skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );

    // A broad --unmanaged '*' must remove only the unmanaged item.
    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!unmanaged_skill.exists(), "unmanaged skill must be removed");
    assert!(
        managed_link.exists(),
        "managed link must survive --unmanaged removal"
    );
}

/// Non-TTY without `--yes` exits ConfirmationRequired and removes nothing.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_refuses_non_tty_without_yes() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    // No --yes, piped (non-TTY): must fail and leave the item in place.
    let r = sb.mind(&["forget", "--unmanaged", "skill:*"]);
    assert!(
        !r.success,
        "must refuse without --yes in non-TTY: {}",
        r.stderr
    );
    assert!(skill.exists(), "nothing must be removed on refusal");
}

// ---- B1/LIFE-45: `--json` is always non-interactive for a destructive
// confirmation, even on a simulated TTY (MIND_TTY=1). Model: cli_absorb.rs
// "json is always non-interactive" (ABS-7). ----------------------------------

/// A ref that matches no unmanaged item exits NotInstalled.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_no_match_is_not_installed() {
    let sb = melded();
    let r = sb.mind(&["forget", "--unmanaged", "nope*", "--yes"]);
    assert!(!r.success, "must fail when no match: {}", r.stderr);
    assert!(
        r.stderr.contains("not installed") || r.stderr.contains("nope"),
        "error must name the unmatched ref: {}",
        r.stderr
    );
}

/// `--json --yes --unmanaged <glob>` emits one MutationResult object whose
/// `removed` array carries the `kind:name` keys of every removed unmanaged item,
/// with no human prose, and removes the matched files.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_json_lists_removed_keys() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let agent = sb.claude_home.join("agents/custom.md");
    write(&agent, "---\nname: custom\n---\n# custom\n");

    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes", "--json"]);
    assert!(r.success, "forget --unmanaged --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "forget", "{}", r.stdout);
    assert_eq!(v["target"], "*", "{}", r.stdout);
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    // The removed array carries both keys, in scan order (BTreeMap by
    // (ItemKind, name); ItemKind declares Skill before Agent).
    assert_eq!(
        v["removed"],
        serde_json::json!(["skill:handmade", "agent:custom"]),
        "removed keys must list every removed unmanaged item: {}",
        r.stdout
    );
    // No human prose under --json.
    assert!(
        !r.stdout.contains("forgot") && !r.stdout.contains("not managed by mind"),
        "human prose must be absent under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
    assert!(!skill.exists() && !agent.exists(), "both must be removed");
}

/// The `-y` short form skips the prompt for `--unmanaged` just like `--yes`.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_short_y_skips_prompt() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["forget", "--unmanaged", "skill:*", "-y"]);
    assert!(
        r.success,
        "-y must skip the prompt: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists(),
        "the unmanaged skill must be removed with -y"
    );
}

/// The `unlearn` visible alias works with `--unmanaged`.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_via_unlearn_alias() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["unlearn", "--unmanaged", "--yes"]);
    assert!(
        r.success,
        "unlearn alias must accept --unmanaged: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists(),
        "the unmanaged skill must be removed via unlearn"
    );
}

/// A kind-qualified EXACT name (not a glob) removes exactly that one unmanaged
/// item and leaves a same-name item of a different kind alone.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_kind_exact_name_removes_one() {
    let sb = melded();
    // A skill and an agent that share the name `shared`.
    let skill = sb.claude_home.join("skills/shared");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# shared\n",
    );
    let agent = sb.claude_home.join("agents/shared.md");
    write(&agent, "---\nname: shared\n---\n# shared\n");

    let r = sb.mind(&["forget", "--unmanaged", "agent:shared", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!agent.exists(), "the agent:shared must be removed");
    assert!(
        skill.exists(),
        "the same-named skill must be untouched by an exact agent: ref"
    );
}

/// A BARE exact name shared across kinds (skill+agent both named `shared`)
/// removes BOTH through the list-and-confirm path. Unlike the single-item UNM-4
/// `resolve` path (which errors AmbiguousItem), the bulk `select` path treats a
/// bare name uniformly: every kind with that name matches and is removed.
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_bare_name_removes_all_kinds() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/shared");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# shared\n",
    );
    let agent = sb.claude_home.join("agents/shared.md");
    write(&agent, "---\nname: shared\n---\n# shared\n");

    let r = sb.mind(&["forget", "--unmanaged", "shared", "--yes"]);
    assert!(
        r.success,
        "a bare shared name must not error under --unmanaged: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !skill.exists() && !agent.exists(),
        "both same-named unmanaged items must be removed"
    );
}

/// A source-qualified ref never matches an unmanaged item, so it is NotInstalled
/// and removes nothing.
// spec: UNM-7
#[test]
fn forget_unmanaged_bulk_source_qualified_is_not_installed() {
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&[
        "forget",
        "--unmanaged",
        "owner/repo#skill:handmade",
        "--yes",
    ]);
    assert!(
        !r.success,
        "a source-qualified ref must not match an unmanaged item: {}",
        r.stdout
    );
    assert!(
        skill.exists(),
        "nothing must be removed when the ref is source-qualified"
    );
}

/// An unmanaged item present in TWO configured lobes is one logical item; a bulk
/// `--unmanaged` removal deletes every occupied lobe path (STO-14, UNM-1).
// spec: UNM-7 UNM-8
#[test]
fn forget_unmanaged_bulk_removes_from_all_lobes() {
    let sb = Sandbox::new();
    let home_a = sb.base.join("homeA");
    let home_b = sb.base.join("homeB");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            home_a.display(),
            home_b.display()
        ),
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // The same unmanaged skill `dup` placed by hand in both lobes.
    let skill_a = home_a.join("skills/dup");
    let skill_b = home_b.join("skills/dup");
    write(
        &skill_a.join("SKILL.md"),
        "---\ndescription: mine\n---\n# dup\n",
    );
    write(
        &skill_b.join("SKILL.md"),
        "---\ndescription: mine\n---\n# dup\n",
    );

    let r = sb.mind(&["forget", "--unmanaged", "skill:dup", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!skill_a.exists(), "lobe A copy must be removed");
    assert!(!skill_b.exists(), "lobe B copy must be removed");
}

/// `--unmanaged` never touches the manifest: a managed item installed alongside
/// unmanaged ones survives in the manifest after a broad `--unmanaged '*'`.
// spec: UNM-8
#[test]
fn forget_unmanaged_bulk_leaves_manifest_unchanged() {
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let manifest = sb.mind_home.join("manifest.json");
    let before = std::fs::read_to_string(&manifest).unwrap();

    // Place an unmanaged item and sweep all unmanaged.
    let unmanaged = sb.claude_home.join("skills/handmade");
    write(
        &unmanaged.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind(&["forget", "--unmanaged", "*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!unmanaged.exists(), "unmanaged item removed");

    let after = std::fs::read_to_string(&manifest).unwrap();
    assert_eq!(
        before, after,
        "the manifest must be byte-identical after --unmanaged removal"
    );
    // The managed item's link survives.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok(),
        "the managed review link must survive"
    );
}

// --- TUI-2 fallback tests ---------------------------------------------------
//
// TUI-2: `probe` falls back to the non-interactive catalog listing when
// `--no-tui` is given, `--json` is given, or stdout is not a TTY (piped or
// redirected). The `query`, `--kind`, `--source` args apply in both modes.
//
// These tests run `mind probe` with stdout piped (non-TTY), which is the same
// condition the test harness always uses. We verify that:
//   (a) the plain listing is produced (not raw-mode garbage),
//   (b) `--no-tui` produces the same listing,
//   (c) `--json` produces JSON (not raw-mode garbage),
//   (d) query/--kind/--source args are honoured in fallback mode.
// TUI-1 (interactive launch with a real TTY) is allowlisted; it cannot be
// verified headlessly. These tests verify TUI-2 (fallback) and are sufficient
// to prove the opt-out logic is correct.

/// EDGE 4 (unmanaged accounting unaffected by the orphan rework): an agent-home
/// item mind did not install still lists as unmanaged, AND a genuinely
/// removed-upstream mind-installed item is still flagged removed upstream in the
/// same `recall` run. The two classifications are independent: the orphan change
/// must not swallow an unmanaged item, and unmanaged scanning must not suppress a
/// removed-upstream flag.
// spec: UNM-2
// spec: CLI-75
#[test]
fn unmanaged_listing_unaffected_by_orphan_detection() {
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");

    // Seed an unmanaged skill directly in the lobe (mind did not install it).
    write(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\nname: handmade\ndescription: hand written\n---\n# handmade\n",
    );

    // Genuinely remove the installed agent upstream.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // The unmanaged group and item are present, unchanged by the orphan rework.
    assert!(
        recall.stdout.contains("unmanaged: not installed by mind"),
        "the unmanaged group must still be shown: {}",
        recall.stdout
    );
    assert!(
        recall.stdout.contains("skill:handmade"),
        "the unmanaged item must still be listed: {}",
        recall.stdout
    );
    // The removed-upstream mind item is still flagged, in the same run.
    assert!(
        recall.stdout.contains("agent:dev") && recall.stdout.contains("removed upstream"),
        "the removed-upstream mind item must still be flagged alongside unmanaged: {}",
        recall.stdout
    );
    // The unmanaged item must NOT be misreported as a source orphan.
    assert!(
        !recall.stdout.contains("handmade") || !recall_handmade_is_in_a_source(&recall.stdout),
        "the unmanaged item must not be classified as a source's removed-upstream item: {}",
        recall.stdout
    );
}

/// True if a `handmade` mention appears on a `removed upstream` line, which would
/// mean the unmanaged item was misclassified as a source orphan.
fn recall_handmade_is_in_a_source(stdout: &str) -> bool {
    stdout
        .lines()
        .any(|l| l.contains("handmade") && l.contains("removed upstream"))
}

// ---- DSC-59/60/61: curator adoption of an un-onboarded nested source --------
