//! `unmeld` (spec/cli.md).

use super::*;

#[test]
fn unmeld_unlink_only_keeps_installed_items() {
    // spec: CLI-20, CLI-22 - `--unlink-only` removes the source but keeps its
    // installed items, listing them with the forget hint.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["unmeld", "agents", "--unlink-only", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Source is gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    // The installed item is left in place and reported with the forget command.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_ok());
    assert!(
        sb.mind(&["recall", "review"]).success,
        "the item remains installed"
    );
    assert!(
        r.stdout.contains("remain installed") && r.stdout.contains("mind forget"),
        "unlink-only must list orphaned items and suggest forget: {}",
        r.stdout
    );
}

#[test]
fn unmeld_forgets_items_by_default() {
    // spec: CLI-21, CLI-27 - a plain unmeld uninstalls the source's items but
    // must not delete the linked local working tree (CLI-27).
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["unmeld", "agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err(),
        "the item link must be removed by default"
    );
    assert!(
        !sb.mind(&["recall", "review"]).success,
        "the item must be uninstalled by default"
    );
    // CLI-27: unmeld must not delete the linked source's working tree.
    assert!(
        sb.source.exists(),
        "unmeld must not delete the linked local working tree at {}",
        sb.source.display()
    );
}

#[test]
fn unmeld_unknown_source_errors() {
    // spec: CLI-20
    let sb = Sandbox::new();
    let r = sb.mind(&["unmeld", "nope"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("no melded source matches"),
        "{}",
        r.stderr
    );
}

#[test]
fn unmeld_full_name_resolves_basename_collision() {
    // spec: CLI-20
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // Bare basename is ambiguous across the two sources.
    let amb = a.mind(&["unmeld", "agents"]);
    assert!(!amb.success);
    assert!(amb.stderr.contains("multiple sources"), "{}", amb.stderr);

    // Full owner/repo unmelds exactly one; the basename is then unambiguous.
    assert!(
        a.mind(&["unmeld", &format!("{}/agents", a.base_name()), "--yes"])
            .success
    );
    assert!(a.mind(&["unmeld", "agents", "--yes"]).success);
}

#[test]
fn unmeld_glob_removes_only_the_matching_source() {
    // spec: CLI-28 - a glob removes the source(s) it matches and leaves the rest.
    // Meld two sources (`foo` and `agents`); `*agents` matches only `agents`.
    let a = Sandbox::named("foo");
    let agents = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &agents.source_spec()]).success);
    // Install an item from the agents source so its teardown is exercised.
    assert!(
        a.mind(&["learn", &format!("{}/agents#review", agents.base_name())])
            .success
    );

    let r = a.mind(&["unmeld", "*agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Only the agents source (and its item) is gone; foo survives.
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(sources.contains("foo"), "foo must remain melded: {sources}");
    assert!(
        !sources.contains(&format!("{}/agents", agents.base_name())),
        "the agents source must be unmelded: {sources}"
    );
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("skills/review")).is_err(),
        "the agents source's item link must be removed"
    );
}

#[test]
fn unmeld_glob_matching_several_lists_and_removes_with_yes() {
    // spec: CLI-28, CLI-42 - a glob may match more than one source; it lists the
    // matched sources and the multi-source confirmation applies. `--yes` skips it
    // and removes every match.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);
    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());

    // Without --yes a multi-source glob refuses in a non-TTY context, listing the
    // matched sources first.
    let refused = a.mind(&["unmeld", "*agents"]);
    assert!(!refused.success, "must refuse: {}", refused.stdout);
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    assert!(
        refused.stdout.contains(&a_full) && refused.stdout.contains(&b_full),
        "both matched sources must be listed: {}",
        refused.stdout
    );
    // Nothing removed by the refusal.
    let still = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        still.contains(&a_full) && still.contains(&b_full),
        "both sources must survive a refused unmeld: {still}"
    );

    // `--yes` removes both matched sources.
    let r = a.mind(&["unmeld", "*agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        a.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "every matching source must be unmelded"
    );
}

#[test]
fn unmeld_glob_matching_no_source_errors() {
    // spec: CLI-28 - a glob that matches nothing is SourceNotFound.
    let sb = melded();
    let r = sb.mind(&["unmeld", "*nope"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("no melded source matches"),
        "{}",
        r.stderr
    );
}

#[test]
fn unmeld_plain_ambiguous_suffix_still_errors() {
    // spec: CLI-28, CLI-20 - a plain (non-glob) ambiguous suffix is still
    // AmbiguousSource; only a glob is allowed to remove several sources.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    let amb = a.mind(&["unmeld", "agents"]);
    assert!(!amb.success);
    assert!(amb.stderr.contains("multiple sources"), "{}", amb.stderr);
}

#[test]
fn unmeld_glob_unlink_only_over_several_keeps_items() {
    // spec: CLI-28, CLI-22 - a glob matching more than one source goes through the
    // source-granularity multi-source confirmation (skipped here with `--yes`), and
    // `--unlink-only` applies to each matched source: every matched source is
    // unmelded but its installed items are KEPT, with the orphaned-items note shown
    // for each. Two distinct sources, each with one installed item.
    let a = Sandbox::named("agents");
    let b = Sandbox::named("agents");
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);
    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());

    // Install one item from each source by its fully-qualified ref so both
    // sources have an installed item to orphan. Both sources carry a skill named
    // `review`; installing it from both would collide, so a different item (`dev`
    // agent) is installed from the second source to avoid a link conflict.
    assert!(
        a.mind(&["learn", &format!("{a_full}#skill:review")])
            .success,
        "install review from the first source"
    );
    assert!(
        a.mind(&["learn", &format!("{b_full}#agent:dev")]).success,
        "install dev from the second source"
    );

    // `*agents` matches both sources. `--yes` clears the multi-source confirmation;
    // `--unlink-only` keeps every matched source's items.
    let r = a.mind(&["unmeld", "*agents", "--unlink-only", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);

    // Both sources are unmelded.
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("no sources melded"),
        "every matched source must be unmelded: {sources}"
    );

    // CLI-22: the items are KEPT (links survive) and the orphaned-items note is
    // shown for each unmelded source.
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("skills/review")).is_ok(),
        "the first source's item link must be kept under --unlink-only"
    );
    assert!(
        std::fs::symlink_metadata(a.claude_home.join("agents/dev.md")).is_ok(),
        "the second source's item link must be kept under --unlink-only"
    );
    assert!(
        r.stdout.matches("item(s) remain installed").count() >= 2,
        "the orphaned-items note must appear for each unmelded source: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("mind forget"),
        "the forget suggestion must be shown: {}",
        r.stdout
    );
}

#[test]
fn unmeld_glob_single_match_honors_item_count_confirmation() {
    // spec: CLI-28, CLI-21 - a glob matching exactly ONE source does not trigger
    // the source-granularity multi-source confirmation, but it DOES honor that
    // single source's per-source item-count confirmation (CLI-21/CLI-42): a non-TTY
    // run refuses without `--yes`, and `--yes` removes all of the source's items.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // A glob that matches the single melded `agents` source. Without `--yes`, the
    // item-count confirmation refuses in this non-TTY harness, listing the items.
    let refused = sb.mind(&["unmeld", "*agents"]);
    assert!(
        !refused.success,
        "a single-match glob must still honor the item-count confirmation: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    // It must NOT have prompted at source granularity (only one source matched).
    assert!(
        !refused.stdout.contains("would remove 1 source"),
        "a single match must not show the multi-source listing: {}",
        refused.stdout
    );
    // Nothing removed by the refusal.
    assert!(sb.mind(&["recall", "review"]).success, "item remains");
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "the source survives a refused single-match glob"
    );

    // `--yes` removes the source and every one of its items.
    let r = sb.mind(&["unmeld", "*agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "the matched source must be unmelded"
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err());
}

#[test]
fn unmeld_glob_aborts_remaining_sources_when_one_item_hook_fails() {
    // spec: CLI-28, HOOK-53, HOOK-54, HOOK-82 - CLI-28 says every matching source
    // is unmelded "each per its normal path"; the normal path includes the
    // hard-stop on a required (item) uninstall-hook failure (HOOK-53/82). So when a
    // matched source's item uninstall hook exits non-zero mid-iteration, the whole
    // `unmeld` aborts: that source stays melded with its item kept, and any
    // not-yet-processed matched source is left untouched (still melded).
    //
    // The failing source is melded FIRST so it is the first processed; an abort
    // therefore leaves the second, good source entirely unprocessed.
    let fail = Sandbox::bare("glob-abort");
    fail.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    // Per-item uninstall hook that exits non-zero (a hard stop, HOOK-82/53).
    fail.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"greet\"\npath = \"skills/greet\"\nuninstall = \"exit 1\"\n",
    );
    let good = Sandbox::bare("glob-abort");
    good.write_and_commit(
        "skills/other/SKILL.md",
        "---\ndescription: another skill\n---\n# other\n",
    );

    // Meld the failing source first, the good source second (iteration order is
    // meld order).
    assert!(
        fail.mind(&["meld", &fail.source_spec(), "--link-only"])
            .success
    );
    assert!(
        fail.mind(&["meld", &good.source_spec(), "--link-only"])
            .success
    );
    assert!(
        fail.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success,
        "install greet (with its uninstall hook)"
    );
    assert!(
        fail.mind(&[
            "learn",
            "skill:other",
            "--accept-hooks=install,update,uninstall"
        ])
        .success,
        "install other from the good source"
    );

    let fail_full = format!("{}/glob-abort", fail.base_name());
    let good_full = format!("{}/glob-abort", good.base_name());

    // `*glob-abort` matches both; `--yes` clears the multi-source confirmation,
    // `--accept-hooks=install,update,uninstall` runs the hooks unattended.
    let r = fail.mind(&[
        "unmeld",
        "*glob-abort",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "a required item uninstall-hook failure must fail the whole unmeld: {} {}",
        r.stdout, r.stderr
    );

    // The failing source stays melded (its item is kept, mirroring HOOK-54).
    let sources = fail.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains(&fail_full),
        "the source whose item uninstall hook failed must stay melded: {sources}"
    );
    assert!(
        fail.mind(&["recall", "skill:greet"]).success,
        "the item is kept when its required uninstall hook fails"
    );

    // The remaining (good) source was processed AFTER the failing one, so the abort
    // leaves it untouched and still melded.
    assert!(
        sources.contains(&good_full),
        "a matched source after the failing one must be left unprocessed (still melded): {sources}"
    );
    assert!(
        fail.mind(&["recall", "skill:other"]).success,
        "the unprocessed source's item must still be installed"
    );
}

#[test]
fn unmeld_forgets_all_items_with_yes() {
    // spec: CLI-21, CLI-42 - default unmeld removes the source's items; `--yes`
    // skips the multi-item confirmation.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Without --yes, a multi-item unmeld refuses in a non-TTY context (CLI-42).
    let refused = sb.mind(&["unmeld", "agents"]);
    assert!(
        !refused.success,
        "must refuse without --yes: {}",
        refused.stdout
    );
    assert!(
        refused.stderr.contains("needs confirmation"),
        "{}",
        refused.stderr
    );
    // The source and items are untouched after the refusal.
    assert!(sb.mind(&["recall", "review"]).success, "item remains");

    let r = sb.mind(&["unmeld", "agents", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("removed"), "{}", r.stdout);

    // Both the source and every installed item are gone.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded")
    );
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_err());
    // CLI-27: unmeld must not delete the linked source's working tree.
    assert!(
        sb.source.exists(),
        "unmeld --yes must not delete the linked local working tree at {}",
        sb.source.display()
    );
}

#[test]
fn unmeld_confirm_decline_leaves_source_melded_and_hook_not_run() {
    // spec: CLI-21, CLI-42, HOOK-54
    // When the default unmeld would remove multiple items, the multi-item
    // confirmation must happen BEFORE uninstall hooks run. A user who answers
    // "no" must leave the source melded AND the hook must not have executed.
    //
    // TTY simulation: send "n\n" as stdin to exercise the confirm path.
    let sb = Sandbox::bare("unmeld-confirm-order");
    let sentinel = sb.base.join("uninstall-ran");
    let s = sentinel.to_str().unwrap().to_owned();
    let hook_toml = format!("[[hooks]]\nrun = \"touch {s}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &hook_toml);

    // Also add two items so the multi-item confirm triggers.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    sb.write_and_commit(
        "agents/ops.md",
        "---\nname: ops\ndescription: ops\n---\n# ops\n",
    );

    let spec = sb.source_spec();

    // Meld and install both items.
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success, "meld");
    assert!(sb.mind(&["learn", "agent:dev"]).success, "learn dev");
    assert!(sb.mind(&["learn", "agent:ops"]).success, "learn ops");

    // Unmeld with TTY input "n" to decline the multi-item confirm.
    // The test harness sets stdin to a pipe, so the subprocess sees a TTY-like
    // stdin for reading input but is_tty() is false (piped). We therefore use
    // --yes=false path by omitting --yes, and the non-TTY branch refuses with
    // ConfirmationRequired rather than prompting.
    //
    // Non-TTY behavior: with 2 items and no --yes, unmeld errors BEFORE running
    // hooks. Assert the source is still registered and the hook sentinel is absent.
    let r = sb.mind(&["unmeld", "unmeld-confirm-order"]);
    assert!(
        !r.success,
        "unmeld without --yes must fail in non-TTY: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation"),
        "must report ConfirmationRequired: {}",
        r.stderr
    );

    // Sentinel must be absent: hook did NOT run before the confirmation gate.
    assert!(
        !sentinel.exists(),
        "uninstall hook must not run before the multi-item confirmation gate: sentinel exists"
    );

    // Source must still be registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("unmeld-confirm-order"),
        "source must remain melded after a declined confirm: {sources}"
    );
}
