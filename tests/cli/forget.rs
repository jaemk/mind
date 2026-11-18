//! `forget` and `prune` (spec/cli.md).

use super::*;

#[test]
fn forget_removes_symlink_and_manifest_entry() {
    // spec: CLI-40, LIFE-20
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["forget", "review"]);
    assert!(r.success, "{}", r.stderr);

    let link = sb.claude_home.join("skills/review");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "symlink should be gone"
    );

    // The item is no longer installed (a single-item recall lookup fails).
    assert!(
        !sb.mind(&["recall", "review"]).success,
        "review should no longer be installed"
    );
}

#[test]
fn forget_unknown_item_errors() {
    // spec: CLI-40
    let sb = melded();
    let r = sb.mind(&["forget", "review"]);
    assert!(!r.success);
    assert!(r.stderr.contains("not installed"), "{}", r.stderr);
}

#[test]
fn forget_bare_name_is_ambiguous_across_kinds_and_qualifier_resolves() {
    // spec: CLI-40, CLI-71
    let sb = Sandbox::bare("dup");
    sb.write_and_commit(
        "skills/dup/SKILL.md",
        "---\nname: dup\ndescription: skill dup\n---\n# dup skill\n",
    );
    sb.write_and_commit(
        "agents/dup.md",
        "---\nname: dup\ndescription: agent dup\n---\n# dup agent\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);
    assert!(sb.mind(&["learn", "skill:dup"]).success);
    assert!(sb.mind(&["learn", "agent:dup"]).success);

    // A bare name now matches both the skill and the agent -> ambiguous.
    let bare = sb.mind(&["forget", "dup"]);
    assert!(!bare.success);
    assert!(bare.stderr.contains("ambiguous"), "{}", bare.stderr);
    // recall <item> with the same bare name is ambiguous too.
    assert!(!sb.mind(&["recall", "dup"]).success);

    // A wrong source qualifier matches nothing.
    let wrong = sb.mind(&["forget", "other/repo#skill:dup"]);
    assert!(!wrong.success);
    assert!(wrong.stderr.contains("not installed"), "{}", wrong.stderr);

    // The kind prefix disambiguates and forgets exactly one.
    assert!(sb.mind(&["forget", "skill:dup"]).success);
    assert!(
        sb.mind(&["recall", "agent:dup"]).success,
        "agent:dup remains installed"
    );
    assert!(
        !sb.mind(&["recall", "skill:dup"]).success,
        "skill:dup uninstalled"
    );
}

#[test]
fn unlearn_is_an_alias_for_forget() {
    // spec: CLI-40
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["unlearn", "review"]).success);
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn forget_glob_uninstalls_all_matches() {
    // spec: CLI-41
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);
    assert!(sb.mind(&["recall"]).stdout.contains("skill:review"));

    // A kind glob forgets only that kind.
    assert!(sb.mind(&["forget", "skill:*"]).success);
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "skill:review should be uninstalled"
    );
    assert!(
        sb.mind(&["recall", "agent:dev"]).success,
        "agent:dev should remain installed"
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());

    // A bare `*` forgets everything that is left (multi-match needs --yes, CLI-42).
    assert!(sb.mind(&["forget", "*", "--yes"]).success);
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));

    // A glob matching no installed item is an error.
    let none = sb.mind(&["forget", "zzz*"]);
    assert!(!none.success);
    assert!(none.stderr.contains("not installed"), "{}", none.stderr);
}

#[test]
fn forget_confirms_before_removing_multiple_items() {
    // spec: CLI-42 - a multi-match glob refuses in a non-TTY context without
    // --yes (rather than removing silently), and lists what it would remove.
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);

    let r = sb.mind(&["forget", "*"]);
    assert!(
        !r.success,
        "a multi-item forget must refuse without --yes: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("would remove") && r.stdout.contains("skill:review"),
        "it must list what would be removed: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "non-TTY refusal: {}",
        r.stderr
    );
    // Nothing was removed.
    assert!(
        sb.mind(&["recall"]).stdout.contains("skill:review"),
        "items must remain after a refused forget"
    );

    // A single exact forget is not prompted.
    assert!(sb.mind(&["forget", "skill:review"]).success);
}

#[test]
fn forget_dropped_removes_only_items_removed_upstream() {
    // spec: CLI-245
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let r = sb.mind(&["forget", "--dropped"]);
    assert!(
        r.success,
        "forget --dropped failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("forgot agent:dev"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
    assert!(
        !sb.claude_home.join("agents/dev.md").exists(),
        "the dropped item's link must be removed"
    );
    assert!(sb.claude_home.join("skills/review").exists());
}

#[test]
fn forget_dropped_takes_items_of_an_unmelded_source_and_confirms_several() {
    // spec: CLI-245
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(
        sb.mind(&["unmeld", "agents", "--keep-items", "--yes"])
            .success
    );

    // Two items: CLI-42's confirmation applies, and a non-TTY run refuses.
    let r = sb.mind(&["forget", "--dropped"]);
    assert!(!r.success, "a multi-item forget must confirm: {}", r.stdout);
    assert!(
        r.stdout.contains("forget would remove 2 item(s)"),
        "{}",
        r.stdout
    );
    assert_eq!(installed_keys(&sb).len(), 2, "nothing removed on refusal");

    let r = sb.mind(&["forget", "--dropped", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(installed_keys(&sb).is_empty(), "{}", r.stdout);
}

#[test]
fn forget_dropped_is_narrowed_by_a_ref() {
    // spec: CLI-245
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(
        sb.mind(&["unmeld", "agents", "--keep-items", "--yes"])
            .success
    );

    let r = sb.mind(&["forget", "--dropped", "agent:*"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
}

#[test]
fn forget_dropped_with_nothing_dropped_removes_nothing() {
    // spec: CLI-245
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["forget", "--dropped"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("no dropped items"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);

    // --json reports the no-op as one, like the other no-op mutations.
    let r = sb.mind(&["forget", "--dropped", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON");
    assert_eq!(v["outcome"], "no-op", "{v}");
}

#[test]
fn forget_dropped_keeps_items_of_a_source_whose_clone_is_missing() {
    // spec: CLI-245 -- a linked source whose working tree vanished scans to
    // nothing, which says nothing about what it dropped.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let moved = sb.base.join("moved");
    std::fs::rename(&sb.source, &moved).unwrap();
    let r = sb.mind(&["forget", "--dropped"]);
    std::fs::rename(&moved, &sb.source).unwrap();
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("no dropped items"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
}

// ---------------------------------------------------------------------------
// prune (CLI-261) and source-declared prune on upgrade (LIFE-57, LIFE-58)
// ---------------------------------------------------------------------------

#[test]
fn prune_removes_items_no_longer_part_of_a_melded_source() {
    // spec: CLI-261
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");
    let r = sb.mind(&["recall"]);
    assert!(
        r.stdout.contains("(removed upstream; run mind prune)"),
        "recall points at prune: {}",
        r.stdout
    );

    let r = sb.mind(&["prune"]);
    assert!(r.success, "prune failed: {} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
    assert!(!sb.claude_home.join("agents/dev.md").exists());
    assert!(sb.claude_home.join("skills/review").exists());

    let r = sb.mind(&["prune"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout
            .contains("no dropped items or profile leftovers; nothing to prune"),
        "{}",
        r.stdout
    );
    let r = sb.mind(&["prune", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON");
    assert_eq!(v["action"], "prune", "{v}");
    assert_eq!(v["outcome"], "no-op", "{v}");
}

#[test]
fn prune_confirms_several_and_matches_an_unmelded_source() {
    // spec: CLI-261
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(
        sb.mind(&["unmeld", "agents", "--keep-items", "--yes"])
            .success
    );

    let r = sb.mind(&["prune"]);
    assert!(!r.success, "a multi-item prune must confirm: {}", r.stdout);
    assert!(
        r.stdout.contains("prune would remove 2 item(s)"),
        "{}",
        r.stdout
    );
    assert_eq!(installed_keys(&sb).len(), 2, "nothing removed on refusal");

    // The filter matches the source the items were installed from, though
    // it is no longer melded.
    let r = sb.mind(&["prune", "agents", "--json", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON");
    assert_eq!(v["action"], "prune", "{v}");
    assert_eq!(v["target"], "agents", "{v}");
    assert_eq!(
        v["removed"],
        serde_json::json!(["agent:dev", "skill:review"]),
        "{v}"
    );
    assert!(installed_keys(&sb).is_empty());
}

#[test]
fn prune_is_narrowed_to_the_sources_a_filter_matches() {
    // spec: CLI-261
    let (sb, tools) = melded_two_sources();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "deploy"]).success);
    sb.remove_and_commit("agents/dev.md");
    tools.remove_and_commit("skills/deploy/SKILL.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let r = sb.mind(&["prune", "*tools"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned skill:deploy"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["agent:dev".to_string()]);

    // A filter naming no source is an error, not a silent no-op.
    let r = sb.mind(&["prune", "nosuchsource"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("nosuchsource"), "{}", r.stderr);
    assert_eq!(installed_keys(&sb), vec!["agent:dev".to_string()]);

    // A malformed glob is refused before anything is removed.
    let r = sb.mind(&["prune", "[agents"]);
    assert!(!r.success, "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["agent:dev".to_string()]);
}

#[test]
fn prune_removes_a_dropped_item_a_remaining_item_still_requires() {
    // spec: CLI-261 DEP-50 -- the dependency graph comes from the catalog,
    // which no longer lists the dropped agent, so no dependents gate applies
    // and the requiring skill is left installed.
    let sb = Sandbox::bare("prune-deps");
    sb.write_and_commit(
        "agents/target.md",
        "---\nname: target\ndescription: Target\n---\n# target\n",
    );
    sb.write_and_commit(
        "skills/user/SKILL.md",
        "---\nname: user\ndescription: U\nrequires: agent:target\n---\n# user\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "prune-deps#*", "--yes"]).success);
    // Upstream drops the agent but the skill still requires it.
    sb.remove_and_commit("agents/target.md");

    let r = sb.mind(&["prune"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned agent:target"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:user".to_string()]);

    // `forget`'s `--force` is not offered; `--accept-hooks` is, for a
    // profile leftover's uninstall hooks (PROF-38).
    let r = sb.mind(&["prune", "--force"]);
    assert!(!r.success, "--force: {}", r.stdout);
    assert!(r.stderr.contains("unexpected argument"), "{}", r.stderr);
    let r = sb.mind(&["prune", "--accept-hooks"]);
    assert!(r.success, "--accept-hooks: {} {}", r.stdout, r.stderr);
}
