//! Install, upgrade, and uninstall lifecycle (spec/lifecycle.md).

use super::*;

#[test]
fn learn_refuses_to_clobber_a_user_file() {
    // spec: LIFE-41
    let sb = melded();
    // The user already has their own directory where the skill would link.
    let target = sb.claude_home.join("skills/review");
    write(&target.join("MINE.md"), "do not delete me");

    let r = sb.mind(&["learn", "review"]);
    assert!(!r.success, "learn should refuse to overwrite a user file");
    assert!(
        r.stderr.contains("managed by mind") || r.stderr.contains("already exists"),
        "{}",
        r.stderr
    );
    // The user's file is untouched and nothing was recorded.
    assert!(target.join("MINE.md").exists(), "user file was deleted");
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn relearn_replaces_minds_own_symlink() {
    // spec: LIFE-41
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    // Re-learning over mind's own symlink (it points into the store) is allowed.
    let again = sb.mind(&["learn", "review"]);
    assert!(again.success, "{}", again.stderr);
}

#[test]
fn upgrade_treats_a_prefix_change_as_a_rename() {
    // spec: LIFE-10, LIFE-11, LIFE-14, CLI-61
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success); // no prefix yet
    assert!(sb.mind(&["learn", "review"]).success); // installed as skill:review

    // Upstream adds a namespace prefix.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("rename"),
        "report should flag rename: {}",
        r.stdout
    );
    assert!(
        r.stdout
            .contains("upgraded skill:review -> skill:jk:review"),
        "{}",
        r.stdout
    );

    // Manifest now holds only the renamed item.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("skill:jk:review"),
        "{}",
        recall.stdout
    );
    assert!(!recall.stdout.contains("skill:review"), "{}", recall.stdout);

    // Symlinks moved; the old one is gone, the new one exists.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/jk:review")).is_ok());
    // Old store copy removed, new one present.
    assert!(!sb.mind_home.join("store/skill/review").exists());
    assert!(sb.mind_home.join("store/skill/jk:review").exists());
}

#[test]
fn upgrade_rename_refuses_to_evict_a_different_sources_item() {
    // spec: LIFE-46 -- B2: two sources both ship a `review` skill. Source `x`
    // installs prefixed (`skill:jk:review`); source `y` installs unprefixed
    // (`skill:review`). `x` then drops its prefix upstream, so the rename
    // `upgrade` would apply for `x`'s item targets the SAME key `y` already
    // occupies. `upgrade --yes` must refuse rather than evict `y`'s item.
    let x = Sandbox::bare("x");
    x.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: X review\n---\n# X review\n",
    );
    x.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    let y = Sandbox::bare("y");
    y.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Y review\n---\n# Y review\n",
    );

    assert!(x.mind(&["meld", &x.source_spec()]).success);
    assert!(x.mind(&["meld", &y.source_spec()]).success);
    // Y's unprefixed review -> skill:review.
    assert!(x.mind(&["learn", "review"]).success);
    // X's prefixed review -> skill:jk:review.
    assert!(x.mind(&["learn", "jk:review"]).success);

    // X drops its prefix upstream: the next upgrade would rename
    // skill:jk:review -> skill:review, colliding with Y's item.
    x.write_and_commit("mind.toml", "[source]\n");
    assert!(x.mind(&["sync"]).success);

    // spec: CLI-263 -- preview refuses the same collision before changing state.
    let preview = x.mind(&["upgrade", "--dry-run", "--json"]);
    assert!(!preview.success, "{}", preview.stdout);
    assert_eq!(
        parse_json(&preview.stdout)["error"]["kind"],
        "upgrade-rename-collision"
    );

    let r = x.mind(&["upgrade", "--yes"]);
    assert!(
        !r.success,
        "a colliding rename must be refused, not applied: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("skill:review") || r.stderr.contains("ambiguous"),
        "the refusal should name the colliding key: {}",
        r.stderr
    );

    // Y's item must be completely untouched: same store content, same link.
    let store_content =
        std::fs::read_to_string(x.mind_home.join("store/skill/review/SKILL.md")).unwrap();
    assert!(
        store_content.contains("Y review"),
        "Y's store copy must not be evicted by X's refused rename: {store_content}"
    );
    assert!(
        std::fs::symlink_metadata(x.claude_home.join("skills/review")).is_ok(),
        "Y's link must remain in place"
    );
    // X's item is untouched too: still installed under its old key.
    assert!(x.mind(&["recall", "skill:jk:review"]).success);
    let manifest = std::fs::read_to_string(x.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        manifest.contains("\"skill:jk:review\"") && manifest.contains("\"skill:review\""),
        "both items must remain recorded in the manifest: {manifest}"
    );
}

#[test]
fn upgrade_rename_within_batch_collision_is_refused() {
    // spec: LIFE-46 -- H1: two sources both ship a `review` skill and BOTH
    // install prefixed (`skill:jx:review`, `skill:jy:review`), then both drop
    // their prefixes upstream. In ONE `upgrade` batch both renames target the
    // same bare key `skill:review`. The pre-existing-manifest guard passes both
    // (neither `skill:review` is occupied yet), so only the within-batch guard
    // catches it. Without that guard the first install becomes the second's
    // evicted "previous version" and one item is silently lost.
    let x = Sandbox::bare("x");
    x.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: X review\n---\n# X review\n",
    );
    x.write_and_commit("mind.toml", "[source]\nprefix = \"jx\"\n");
    let y = Sandbox::bare("y");
    y.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Y review\n---\n# Y review\n",
    );
    y.write_and_commit("mind.toml", "[source]\nprefix = \"jy\"\n");

    assert!(x.mind(&["meld", &x.source_spec()]).success);
    assert!(x.mind(&["meld", &y.source_spec()]).success);
    assert!(x.mind(&["learn", "jx:review"]).success);
    assert!(x.mind(&["learn", "jy:review"]).success);

    // Both sources drop their prefix upstream.
    x.write_and_commit("mind.toml", "[source]\n");
    y.write_and_commit("mind.toml", "[source]\n");
    assert!(x.mind(&["sync"]).success);

    // spec: CLI-263 -- within-batch collisions are also rejected by preview.
    let preview = x.mind(&["upgrade", "--dry-run", "--json"]);
    assert!(!preview.success, "{}", preview.stdout);
    assert_eq!(
        parse_json(&preview.stdout)["error"]["kind"],
        "upgrade-rename-collision"
    );

    // One batch upgrade covering both sources: both renames converge on
    // skill:review and must be refused without touching either item.
    let r = x.mind(&["upgrade", "--yes"]);
    assert!(
        !r.success,
        "two same-batch renames converging on one key must be refused: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // M-test3: `!r.success` alone would also be satisfied by an unrelated
    // failure (a hook prompt, a lock error), which would then also trivially
    // satisfy the "nothing was applied" assertions below for the wrong
    // reason. Name the actual cause: the colliding key and the LIFE-46 error
    // kind (`upgrade-rename-collision`).
    assert!(
        r.stderr.contains("skill:review"),
        "the refusal must name the colliding key: {}",
        r.stderr
    );
    // spec: LIFE-51 -- the pending-upgrade report prints to stdout BEFORE
    // this batch's collision check runs (and refuses via stderr/exit code),
    // so a human run always sees the full pending list first, even though
    // the batch then aborts.
    assert!(
        r.stdout.contains("upstream changes"),
        "the pending report must still print before the collision aborts \
         the batch: {}",
        r.stdout
    );
    let r_json = x.mind(&["upgrade", "--yes", "--json"]);
    assert!(!r_json.success);
    let v = parse_json(&r_json.stdout);
    assert_eq!(
        v["error"]["kind"], "upgrade-rename-collision",
        "expected the LIFE-46 error kind: {v}"
    );

    // Neither item was evicted: both old store copies and both manifest entries
    // remain intact.
    let x_store =
        std::fs::read_to_string(x.mind_home.join("store/skill/jx:review/SKILL.md")).unwrap();
    assert!(x_store.contains("X review"), "X's store copy must survive");
    let y_store =
        std::fs::read_to_string(x.mind_home.join("store/skill/jy:review/SKILL.md")).unwrap();
    assert!(y_store.contains("Y review"), "Y's store copy must survive");
    let manifest = std::fs::read_to_string(x.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        manifest.contains("\"skill:jx:review\"") && manifest.contains("\"skill:jy:review\""),
        "both items must remain recorded: {manifest}"
    );
    assert!(
        !manifest.contains("\"skill:review\""),
        "neither rename may have been applied: {manifest}"
    );
}

#[test]
fn upgrade_rename_collision_and_coincident_links_together_still_refuses_cleanly() {
    // spec: LIFE-46, LIFE-47 -- B2+B3 interaction: a rename that is BOTH a
    // colliding rename (B2) AND has an old-link == new-install-link coincidence
    // (B3, reachable for a skill via an explicit `link =` override, not just an
    // agent's bare harness name) must still refuse via the B2 collision guard,
    // which runs BEFORE the apply loop (and so before B3's link-stripping logic
    // ever executes) -- proving the two fixes compose safely instead of the
    // link-overlap case somehow bypassing the collision refusal.
    let x = Sandbox::bare("x3");
    // X's skill has a link target fixed by an explicit override, independent of
    // its name/prefix, so its OWN link path is identical before and after the
    // upcoming rename (the B3 condition), even though the rename is refused.
    x.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: X review\n---\n# X review\n",
    );
    x.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"review\"\n",
            "path = \"skills/review\"\n",
            "link = \"skills/shared-review\"\n",
        ),
    );
    // Y already occupies the key X's rename will target: skill:jk:review.
    let y = Sandbox::bare("y3");
    y.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Y review\n---\n# Y review\n",
    );
    y.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");

    assert!(x.mind(&["meld", &x.source_spec()]).success);
    assert!(x.mind(&["meld", &y.source_spec()]).success);
    assert!(x.mind(&["learn", "review"]).success); // X -> skill:review, linked at skills/shared-review
    assert!(x.mind(&["learn", "jk:review"]).success); // Y -> skill:jk:review

    let shared_link = x.claude_home.join("skills/shared-review");
    assert!(
        std::fs::symlink_metadata(&shared_link).is_ok(),
        "X's explicit link override must exist after learn"
    );

    // X's source now adds the SAME prefix Y already uses, keeping the fixed
    // link override: the next upgrade would rename skill:review ->
    // skill:jk:review (colliding with Y) while X's own link target
    // (skills/shared-review) would stay byte-identical old vs new.
    x.write_and_commit(
        "mind.toml",
        concat!(
            "[source]\n",
            "prefix = \"jk\"\n\n",
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"review\"\n",
            "path = \"skills/review\"\n",
            "link = \"skills/shared-review\"\n",
        ),
    );
    assert!(x.mind(&["sync"]).success);

    let r = x.mind(&["upgrade", "--yes"]);
    assert!(
        !r.success,
        "the colliding rename must be refused even with coincident links: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Nothing moved: X's own link (which would have coincided old vs new) is
    // untouched -- B3's link-stripping logic never got a chance to run because
    // B2's guard aborted first.
    assert!(
        std::fs::symlink_metadata(&shared_link).is_ok(),
        "X's link must survive the refused rename untouched"
    );
    // Y's item is untouched too (same B2 guarantee as the collision-only test).
    let y_store =
        std::fs::read_to_string(x.mind_home.join("store/skill/jk:review/SKILL.md")).unwrap();
    assert!(
        y_store.contains("Y review"),
        "Y's store copy must not be evicted: {y_store}"
    );
    assert!(x.mind(&["recall", "skill:review"]).success);
    assert!(x.mind(&["recall", "skill:jk:review"]).success);
    let manifest = std::fs::read_to_string(x.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        manifest.contains("\"skill:jk:review\"") && manifest.contains("\"skill:review\""),
        "both items must remain recorded in the manifest: {manifest}"
    );
}

#[test]
fn upgrade_rename_keeps_the_just_created_link_when_old_and_new_paths_coincide() {
    // spec: LIFE-47 -- B3: an agent links under its bare harness name (NS-40)
    // regardless of the item's effective name, so a prefix-only rename can
    // produce the exact same link path before and after. Applying the rename
    // must not remove the link the new install just created.
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_ok(),
        "the agent link must exist after learn"
    );

    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("rename"), "{}", r.stdout);

    // The link (agents/dev.md, same path old and new) must still be present.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("agents/dev.md")).is_ok(),
        "the link must survive the rename: the new install's link must not be \
         removed by the old item's uninstall"
    );
    // The manifest records the renamed key.
    let recall = sb.mind(&["recall"]);
    assert!(recall.stdout.contains("agent:jk:dev"), "{}", recall.stdout);
}

#[test]
fn upgrade_mid_batch_failure_still_saves_the_earlier_successful_upgrades() {
    // spec: LIFE-48 -- H3: a batch of two pending upgrades where the SECOND
    // item's install hook fails. `agent:dev` (key "agent:dev" sorts before
    // "skill:review" in the manifest's BTreeMap, so it is applied first) must
    // still be recorded as upgraded on disk even though the command as a whole
    // fails on the second item.
    let sb = melded();
    assert!(sb.mind(&["learn", "agent:dev"]).success);
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let before_manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let before_hash = before_manifest["items"]["agent:dev"]["hash"].clone();

    // An authoritative mind.toml declaring all three fixture items, so nothing
    // is dropped from the catalog; `review` alone carries a required install
    // hook that always fails, forcing its upgrade's `install_item` to error.
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"review\"\n",
            "path = \"skills/review\"\n",
            "install = \"exit 3\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"agent\"\n",
            "name = \"dev\"\n",
            "path = \"agents/dev.md\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"rules/style.md\"\n",
        ),
    );
    // Bump both dev and review upstream so both are pending.
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests (v2)\n---\n# dev agent v2\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff for bugs (v2)\n---\n# review skill v2\n",
    );
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&[
        "upgrade",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "the batch must fail on review's install hook: {} {}",
        r.stdout, r.stderr
    );

    // agent:dev's upgrade must be recorded on disk (manifest.json's own "hash"
    // field for agent:dev) even though the command failed on the later item.
    // `before`'s hash is the OLD recorded hash; a failure to save here would
    // leave manifest.json holding that same old hash, so this is a direct
    // check of manifest.json's persisted content -- not `recall`'s live
    // drift-comparison summary (CLI-75), which would differ from `before`
    // regardless (the source content changed either way).
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let old_hash = manifest["items"]["agent:dev"]["hash"].clone();
    assert_ne!(
        old_hash, before_hash,
        "sanity: agent:dev's manifest hash must have advanced past the \
         pre-upgrade hash for this test to be meaningful"
    );
    // Its store copy really did advance to the new content too (not just a
    // manifest/disk mismatch in the other direction).
    let store_content = std::fs::read_to_string(sb.mind_home.join("store/agent/dev")).unwrap();
    assert!(
        store_content.contains("v2"),
        "agent:dev's store copy must hold the new content: {store_content}"
    );
}

#[test]
fn upgrade_mid_batch_failure_still_saves_earlier_upgrades_under_json() {
    // spec: LIFE-48 -- H3, --json variant: the mid-batch failure save must not
    // be an artifact of the human-output code path. Same fixture as
    // `upgrade_mid_batch_failure_still_saves_the_earlier_successful_upgrades`,
    // driven with the global `--json` flag; stdout must still carry a valid
    // error envelope, and the manifest save must still have happened.
    let sb = melded();
    assert!(sb.mind(&["learn", "agent:dev"]).success);
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let before_manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let before_hash = before_manifest["items"]["agent:dev"]["hash"].clone();

    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"review\"\n",
            "path = \"skills/review\"\n",
            "install = \"exit 3\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"agent\"\n",
            "name = \"dev\"\n",
            "path = \"agents/dev.md\"\n",
            "\n",
            "[[items]]\n",
            "kind = \"rule\"\n",
            "name = \"style\"\n",
            "path = \"rules/style.md\"\n",
        ),
    );
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests (v2)\n---\n# dev agent v2\n",
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff for bugs (v2)\n---\n# review skill v2\n",
    );
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&[
        "--json",
        "upgrade",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "the batch must fail on review's install hook: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["schema"], 1, "{}", r.stdout);
    assert!(v["error"].is_object(), "{}", r.stdout);

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap())
            .unwrap();
    let old_hash = manifest["items"]["agent:dev"]["hash"].clone();
    assert_ne!(
        old_hash, before_hash,
        "agent:dev's manifest hash must have advanced even though the whole \
         command failed under --json: {manifest:#}"
    );
    let store_content = std::fs::read_to_string(sb.mind_home.join("store/agent/dev")).unwrap();
    assert!(
        store_content.contains("v2"),
        "agent:dev's store copy must hold the new content: {store_content}"
    );
}

#[test]
fn introspect_reports_missing_link() {
    // spec: LIFE-30
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    std::fs::remove_file(sb.claude_home.join("skills/review")).unwrap();
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("symlink missing"), "{}", r.stdout);
}

#[test]
fn introspect_reports_drift_after_source_change() {
    // spec: LIFE-33
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("upstream changed"), "{}", r.stdout);
}

#[test]
fn introspect_reports_namespace_change() {
    // spec: LIFE-32
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["introspect"]);
    assert!(r.stdout.contains("namespace changed"), "{}", r.stdout);
}

#[test]
fn failed_upgrade_preserves_the_previous_version() {
    // spec: LIFE-1, LIFE-2, LIFE-4
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:dev}}.\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    // `lead` references {{ns:dev}}; confirm the closure with --yes.
    assert!(sb.mind(&["learn", "jk:lead", "--yes"]).success);
    let store = sb.mind_home.join("store/agent/jk:lead");
    // NS-42: {{ns:dev}} expands bare (dev is an agent sibling).
    assert!(std::fs::read_to_string(&store).unwrap().contains("dev"));

    // Upstream introduces a broken reference.
    sb.write_and_commit(
        "agents/lead.md",
        "---\nname: lead\ndescription: lead\n---\nDelegate to {{ns:ghost}}.\n",
    );
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(!r.success, "upgrade should fail on the bad reference");
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);

    // The previously installed good version is untouched.
    let body = std::fs::read_to_string(&store).expect("old store copy should remain");
    // NS-42: agent token expanded bare; NS-40: link is under bare harness name.
    assert!(body.contains("dev"), "old version should be intact: {body}");
    // NS-40: the agent link is under its bare frontmatter name, not the prefixed name.
    assert!(std::fs::symlink_metadata(sb.claude_home.join("agents/lead.md")).is_ok());
}

#[test]
fn removed_upstream_item_is_left_alone_and_flagged() {
    // spec: LIFE-12, LIFE-31
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);

    // The item disappears upstream.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success);

    // upgrade does not touch an item with no catalog match.
    let ev = sb.mind(&["upgrade", "--yes"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("up to date"), "{}", ev.stdout);
    assert!(sb.mind(&["recall"]).stdout.contains("agent:dev"));

    // introspect reports it as gone upstream.
    let ins = sb.mind(&["introspect"]);
    assert!(ins.stdout.contains("no longer present"), "{}", ins.stdout);
}

/// The worst case (UNM-5): `forget --unmanaged <ref> --json` on a TTY without
/// `--yes` must refuse rather than delete the user's own file.
#[test]
fn forget_unmanaged_single_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind_env(
        &["--json", "forget", "skill:handmade"],
        &[("MIND_TTY", "1")],
    );
    assert!(
        !r.success,
        "--json on a TTY without --yes must refuse: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert_eq!(
        v["error"]["kind"], "confirmation-required",
        "kind must be confirmation-required: {}",
        r.stdout
    );
    assert!(
        skill.exists(),
        "the user's own unmanaged file must NOT be deleted: {}",
        r.stdout
    );
}

/// The bulk form (UNM-8): `forget --unmanaged --json` on a TTY without
/// `--yes` must refuse and remove nothing.
#[test]
fn forget_unmanaged_bulk_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let sb = melded();
    let skill = sb.claude_home.join("skills/handmade");
    write(
        &skill.join("SKILL.md"),
        "---\ndescription: mine\n---\n# handmade\n",
    );
    let r = sb.mind_env(
        &["--json", "forget", "--unmanaged", "skill:*"],
        &[("MIND_TTY", "1")],
    );
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["schema"], 1, "{}", r.stdout);
    assert_eq!(v["error"]["kind"], "confirmation-required", "{}", r.stdout);
    assert!(skill.exists(), "nothing must be removed: {}", r.stdout);
}

/// `forget '<glob matching several>' --json` on a TTY without `--yes` must
/// refuse (CLI-42's multi-item confirmation) rather than removing everything.
#[test]
fn forget_glob_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    assert!(sb.mind(&["learn", "agent:dev"]).success);
    let r = sb.mind_env(&["--json", "forget", "*"], &[("MIND_TTY", "1")]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["error"]["kind"], "confirmation-required", "{}", r.stdout);
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "nothing must have been removed"
    );
    assert!(sb.mind(&["recall", "agent:dev"]).success);
}

/// `unmeld '<glob matching several sources>' --json` on a TTY without `--yes`
/// must refuse (CLI-28's multi-source confirmation) rather than dropping them.
#[test]
fn unmeld_glob_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let (sb, _tools) = melded_two_sources();
    let r = sb.mind_env(&["--json", "unmeld", "*"], &[("MIND_TTY", "1")]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["error"]["kind"], "confirmation-required", "{}", r.stdout);
    // A precise count, not just "not empty": a bug that unmelds only SOME of
    // the glob's matches (rather than none) would still pass a substring check
    // but must fail this one.
    let sources = sb.mind(&["recall", "--sources", "--json"]);
    let sv: serde_json::Value = serde_json::from_str(sources.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "recall --sources --json must be JSON, got {e}: {:?}",
            sources.stdout
        )
    });
    assert_eq!(
        sv["items"].as_array().map(|a| a.len()),
        Some(2),
        "both sources must remain melded, none removed: {}",
        sources.stdout
    );
}

/// `upgrade --json` on a TTY without `--yes` must refuse rather than apply
/// pending upgrades unprompted.
#[test]
fn upgrade_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    let r = sb.mind_env(&["--json", "upgrade"], &[("MIND_TTY", "1")]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["error"]["kind"], "confirmation-required", "{}", r.stdout);
    assert_eq!(
        before,
        sb.mind(&["recall", "skill:review"]).stdout,
        "nothing must have been upgraded"
    );
}

/// `evolve --json` on a TTY without `--yes` must refuse rather than swap the
/// running binary unprompted. `--version` bypasses the network release lookup
/// entirely, so this stays hermetic.
#[test]
fn evolve_json_on_tty_without_yes_is_confirmation_required() {
    // spec: LIFE-45
    let sb = Sandbox::bare("evolve-confirm");
    let r = sb.mind_env(
        &["--json", "evolve", "--version", "99.0.0"],
        &[("MIND_TTY", "1")],
    );
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be a JSON error envelope, got {e}: {:?}",
            r.stdout
        )
    });
    assert_eq!(v["schema"], 1, "{}", r.stdout);
    assert_eq!(v["error"]["kind"], "confirmation-required", "{}", r.stdout);
}

// ---- M1/LIFE-49: `sync --upgrade` honors the global `--yes` -----------------

/// `sync --upgrade --yes` must actually skip the confirmation and apply the
/// pending upgrade, not silently drop `--yes` (which would either prompt on a
/// TTY or, worse, refuse/hang on a non-interactive run).
#[test]
fn sync_upgrade_yes_applies_without_a_prompt_reply() {
    // spec: LIFE-49
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    sb.edit_source();
    // No stdin reply is supplied at all (stdin is closed): if `--yes` were
    // dropped internally, the text-mode confirm would read EOF and decline
    // (report "aborted; nothing changed"); with `--yes` correctly threaded,
    // the pass applies without ever reading stdin.
    let r = sb.mind(&["sync", "--upgrade", "--yes"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        !r.stdout.contains("aborted"),
        "sync --upgrade --yes must not fall back to the confirmation prompt: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("upgraded skill:review"),
        "sync --upgrade --yes must apply the pending upgrade: {}",
        r.stdout
    );
}

#[test]
fn upgrade_source_hook_rerun_mid_batch_failure_saves_earlier_hook_runs() {
    // spec: LIFE-48 -- H3: two sources both need their install hook re-run in
    // the same upgrade pass (rerun_source_hooks, a source-level pass separate
    // from the per-item loop). Source A's hook succeeds; source B's (visited
    // second, in registration order) fails. A's `ran_at` update must be
    // persisted even though the pass as a whole fails on B.
    let a = sandbox_with_declared_hook("amarker", "true");
    assert!(
        a.mind(&[
            "meld",
            &a.source_spec(),
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    // B declares a hook that always fails from the start. Melded WITHOUT the
    // dangerous flag (non-TTY): the hook is offered and SKIPPED (never run,
    // ran_at stays None), which HOOK-11 also treats as pending -- so it does
    // not need a source-content advance to be re-offered by the upgrade pass.
    let b = Sandbox::bare("bfailer");
    b.write_and_commit("mind.toml", "[source]\ninstall = \"exit 7\"\n");
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // Advance A so its (successful) hook is pending re-offer too (HOOK-11).
    a.edit_source();
    assert!(a.mind(&["sync"]).success);

    let r = a.mind(&[
        "upgrade",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "must fail on B's hook: {} {}",
        r.stdout, r.stderr
    );

    // A's install hook ran_at must have advanced to A's new (post-sync) commit
    // even though the pass overall failed on B.
    let sources_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(a.mind_home.join("sources.json")).unwrap())
            .unwrap();
    let sources = sources_json["sources"].as_array().unwrap();
    let a_entry = sources
        .iter()
        .find(|s| s["name"].as_str().unwrap().contains("amarker"))
        .expect("source A recorded");
    let ran_at = a_entry["install_hooks"][0]["ran_at"].as_str();
    let commit = a_entry["commit"].as_str();
    assert_eq!(
        ran_at, commit,
        "source A's hook re-run must be persisted before B's failure propagated: {a_entry:#}"
    );
}

#[test]
fn remeld_hook_failure_mid_batch_saves_earlier_successful_hook_run() {
    // spec: LIFE-48 -- H3: a re-meld with two pending install hooks where the
    // SECOND fails. The first hook's successful run must be persisted (its
    // `ran_at` recorded) even though the re-meld as a whole fails on the
    // second (mirrors HOOK-53's "leave the source melded" intent, but for the
    // hook run record too).
    let sb = Sandbox::named("remeld-hooks");
    sb.write_and_commit(
        "mind.toml",
        concat!(
            "[[hooks]]\n",
            "name = \"first\"\n",
            "run = \"true\"\n",
            "event = \"install\"\n",
            "\n",
            "[[hooks]]\n",
            "name = \"second\"\n",
            "run = \"exit 9\"\n",
            "event = \"install\"\n",
        ),
    );
    let spec = sb.source_spec();
    // Initial meld: non-TTY, no --accept-hooks -> both hooks are offered
    // and SKIPPED (ran_at stays None for both).
    assert!(sb.mind(&["meld", &spec]).success);

    // Re-meld with --accept-hooks=install,update,uninstall: both pending hooks
    // run unattended, in declaration order; the first succeeds, the second
    // fails, stopping the batch.
    let r = sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(
        !r.success,
        "the re-meld must fail on the second hook: {} {}",
        r.stdout, r.stderr
    );

    let sources_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap())
            .unwrap();
    let entry = &sources_json["sources"][0];
    let first_ran_at = entry["install_hooks"][0]["ran_at"].as_str();
    assert!(
        first_ran_at.is_some(),
        "the first hook's successful run must be persisted even though the \
         second hook in the same pass failed: {entry:#}"
    );
}

/// A sandbox with `dev` and `review` installed, then `agents/dev.md` removed
/// upstream in a commit that also writes `mind_toml` (when given).
fn dropped_upstream(mind_toml: Option<&str>) -> Sandbox {
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    if let Some(text) = mind_toml {
        write(&sb.source.join("mind.toml"), text);
    }
    sb.remove_and_commit("agents/dev.md");
    sb
}

#[test]
fn upgrade_prunes_items_a_pruning_source_dropped() {
    // spec: LIFE-57
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("1 item removed upstream, to prune"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
    assert!(!sb.claude_home.join("agents/dev.md").exists());
    assert!(sb.claude_home.join("skills/review").exists());

    let again = sb.mind(&["upgrade", "--yes"]);
    assert!(again.success, "{}", again.stderr);
    assert!(
        again.stdout.contains("everything is up to date"),
        "{}",
        again.stdout
    );
}

#[test]
fn a_pruning_source_prunes_only_its_own_items() {
    // spec: LIFE-57 -- `prune` is read from each item's own source: one
    // source declaring it leaves another source's dropped items installed.
    let (sb, tools) = melded_two_sources();
    assert!(sb.mind(&["learn", "dev"]).success);
    assert!(sb.mind(&["learn", "deploy"]).success);
    write(&sb.source.join("mind.toml"), "[source]\nprune = true\n");
    sb.remove_and_commit("agents/dev.md");
    tools.remove_and_commit("skills/deploy/SKILL.md");

    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert!(!r.stdout.contains("pruned skill:deploy"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb), vec!["skill:deploy".to_string()]);
    assert!(sb.claude_home.join("skills/deploy").exists());

    let recall = sb.mind(&["recall"]);
    let deploy = recall
        .stdout
        .lines()
        .find(|l| l.contains("skill:deploy"))
        .unwrap_or_default();
    assert!(deploy.contains("removed upstream"), "{}", recall.stdout);
}

#[test]
fn upgrade_keeps_dropped_items_of_a_source_that_does_not_prune() {
    // spec: LIFE-57
    for toml in [None, Some("[source]\nprune = false\n")] {
        let sb = dropped_upstream(toml);
        let r = sb.mind(&["upgrade", "--yes"]);
        assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
        assert!(!r.stdout.contains("prune"), "{toml:?}: {}", r.stdout);
        assert_eq!(
            installed_keys(&sb),
            vec!["agent:dev".to_string(), "skill:review".to_string()],
            "{toml:?}"
        );
        assert!(sb.claude_home.join("agents/dev.md").exists(), "{toml:?}");
    }
}

#[test]
fn an_upgrade_item_filter_narrows_what_is_pruned() {
    // spec: LIFE-57
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let r = sb.mind(&["upgrade", "review", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("pruned"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb).len(), 2, "{}", r.stdout);

    let r = sb.mind(&["upgrade", "agent:*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
}

#[test]
fn an_upgrade_prune_is_confirmed_and_reported_under_json() {
    // spec: LIFE-58
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let r = sb.mind(&["upgrade", "--json"]);
    assert!(!r.success, "a prune must be confirmed: {}", r.stdout);
    assert_eq!(installed_keys(&sb).len(), 2, "nothing removed on refusal");

    let r = sb.mind(&["upgrade", "--json", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON");
    assert_eq!(v["outcome"], "pruned", "{v}");
    assert_eq!(v["removed"], serde_json::json!(["agent:dev"]), "{v}");
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
}

#[test]
fn sync_upgrade_prunes_and_reports_the_pruned_keys_under_json() {
    // spec: LIFE-57 LIFE-58
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let r = sb.mind(&["sync", "--upgrade", "--json", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v: serde_json::Value = serde_json::from_str(&r.stdout).expect("JSON");
    assert_eq!(v["action"], "sync", "{v}");
    assert_eq!(v["removed"], serde_json::json!(["agent:dev"]), "{v}");
    assert_eq!(installed_keys(&sb), vec!["skill:review".to_string()]);
}

#[test]
fn declining_an_upgrade_that_would_prune_removes_nothing() {
    // spec: LIFE-57 -- the pass's confirmation covers the prunes.
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let r = sb.mind_tty(&["upgrade"], "n\n");
    assert!(
        r.stdout.contains("1 item removed upstream, to prune"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb).len(), 2, "{}", r.stdout);
    assert!(sb.claude_home.join("agents/dev.md").exists());

    // Off a terminal, without --yes, the pass refuses rather than prune.
    let r = sb.mind(&["upgrade"]);
    assert!(!r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert_eq!(installed_keys(&sb).len(), 2, "{}", r.stdout);
}

#[test]
fn a_pruned_item_runs_no_uninstall_hook() {
    // spec: LIFE-57 CLI-245 -- an item's hooks come from its source's catalog,
    // which no longer lists a dropped item.
    let sb = melded();
    let marker = sb.base.join("uninstalled");
    let hooked = format!(
        "---\nname: dev\ndescription: d\nuninstall: touch {}\n---\n# dev\n",
        marker.display()
    );
    sb.write_and_commit("agents/dev.md", &hooked);
    assert!(sb.mind(&["learn", "dev", "--accept-hooks"]).success);
    // Control: forgetting the item while its source still offers it runs the
    // hook.
    let r = sb.mind(&["forget", "dev", "--accept-hooks"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(marker.exists(), "the hook is live: {}", r.stdout);
    std::fs::remove_file(&marker).unwrap();

    assert!(sb.mind(&["learn", "dev", "--accept-hooks"]).success);
    write(&sb.source.join("mind.toml"), "[source]\nprune = true\n");
    sb.remove_and_commit("agents/dev.md");
    let r = sb.mind(&["upgrade", "--yes", "--accept-hooks"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("pruned agent:dev"), "{}", r.stdout);
    assert!(!marker.exists(), "no uninstall hook ran: {}", r.stdout);
}

#[test]
fn an_upgrade_prunes_nothing_from_a_source_whose_tree_is_missing() {
    // spec: LIFE-57 -- a source with no clone says nothing about what it
    // dropped, so the pass must not read its empty scan as every item gone.
    let sb = dropped_upstream(Some("[source]\nprune = true\n"));
    let moved = sb.base.join("moved");
    std::fs::rename(&sb.source, &moved).unwrap();
    let r = sb.mind(&["upgrade", "--yes"]);
    std::fs::rename(&moved, &sb.source).unwrap();
    assert!(!r.stdout.contains("pruned"), "{} {}", r.stdout, r.stderr);
    assert_eq!(
        installed_keys(&sb),
        vec!["agent:dev".to_string(), "skill:review".to_string()]
    );
}
