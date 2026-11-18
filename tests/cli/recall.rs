//! `recall` (spec/cli.md).

use super::*;

#[test]
fn recall_lists_and_shows_item_details() {
    // spec: CLI-70, CLI-71
    let sb = melded();
    sb.mind(&["learn", "review"]);

    let list = sb.mind(&["recall"]);
    assert!(list.stdout.contains("skill:review"));

    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(detail.stdout.contains("source  "), "{}", detail.stdout);
    assert!(detail.stdout.contains("/agents"), "{}", detail.stdout);
    assert!(detail.stdout.contains("hash"), "{}", detail.stdout);
}

#[test]
fn recall_detail_shows_description() {
    // spec: CLI-71, DSC-32
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["recall", "skill:review"]);
    assert!(
        r.stdout.contains("desc    Review the diff for bugs"),
        "{}",
        r.stdout
    );
}

#[test]
fn status_is_an_alias_for_recall() {
    // spec: CLI-70
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let recall = sb.mind(&["recall"]);
    let status = sb.mind(&["status"]);
    assert!(status.success, "status alias runs: {}", status.stderr);
    assert_eq!(
        status.stdout, recall.stdout,
        "`status` must produce the same output as `recall`"
    );
    // The alias accepts recall's arguments too.
    assert!(sb.mind(&["status", "--sources"]).success);
}

#[test]
fn recall_json_emits_items_and_sources() {
    // spec: CLI-73, CLI-167 - JSON outputs are wrapped in {"schema":1,"items":[...]}.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // The default view is an envelope with a sources array, each with nested items.
    let items = sb.mind(&["recall", "--json"]);
    assert!(items.success, "{}", items.stderr);
    let env: serde_json::Value =
        serde_json::from_str(&items.stdout).expect("recall --json envelope");
    assert_eq!(env["schema"], 1, "schema must be 1: {}", items.stdout);
    assert!(
        env["items"].is_array(),
        "items key must be array: {}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"items\""),
        "sources carry nested items: {}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"key\": \"skill:review\""),
        "{}",
        items.stdout
    );
    assert!(
        items.stdout.contains("\"installed\": true"),
        "the installed item is flagged: {}",
        items.stdout
    );

    // A single-item lookup is a plain JSON object (not wrapped).
    let one = sb.mind(&["recall", "skill:review", "--json"]).stdout;
    assert!(one.trim_start().starts_with('{'), "{one}");
    assert!(one.contains("\"hash\""), "{one}");

    // --sources is an envelope with sources array.
    let srcs_r = sb.mind(&["recall", "--sources", "--json"]);
    let srcs_env: serde_json::Value =
        serde_json::from_str(&srcs_r.stdout).expect("recall --sources --json envelope");
    assert_eq!(srcs_env["schema"], 1, "schema must be 1: {}", srcs_r.stdout);
    assert!(
        srcs_env["items"].is_array(),
        "items key must be array: {}",
        srcs_r.stdout
    );
    assert!(srcs_r.stdout.contains("\"url\""), "{}", srcs_r.stdout);

    // An empty registry is {"schema":1,"items":[]}, not a human message.
    // spec: CLI-167
    let fresh = Sandbox::new();
    let empty_env: serde_json::Value =
        serde_json::from_str(fresh.mind(&["recall", "--json"]).stdout.trim())
            .expect("empty recall envelope");
    assert_eq!(empty_env["schema"], 1);
    assert_eq!(
        empty_env["items"].as_array().map(|a| a.len()),
        Some(0),
        "empty recall must emit empty items array"
    );
}

#[test]
fn example_drift_upgrade() {
    // spec: CLI-75, CLI-155, CLI-90, LIFE-11, LIFE-13, LIFE-15, LIFE-33
    // The drift example installs skill:audit, edits the source body and syncs so
    // the recorded commit advances while the installed copy's source-content hash
    // lags (LIFE-15: hash is of source content, so detection compares source with
    // source). `recall` then marks the item stale with the `^` left-edge marker
    // (CLI-155) and the trailing `(outdated; run mind upgrade)` text (CLI-75);
    // `introspect` reports the drift (CLI-90, LIFE-33); and `mind upgrade --yes`
    // reports the hash/commit deltas and reinstalls under the same name
    // (LIFE-11 pending on hash change, LIFE-13 content-only reinstall), after
    // which `recall` shows the item current again.
    let sb = Sandbox::from_example("drift");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(sb.mind(&["learn", "audit"]).success);

    // Fresh install: not outdated, leads with the `+` marker.
    let fresh = sb.mind(&["--ascii", "recall"]);
    assert!(fresh.success, "{}", fresh.stderr);
    assert!(
        !fresh.stdout.contains("outdated"),
        "freshly installed audit must not be outdated: {}",
        fresh.stdout
    );

    // Simulate an upstream change: edit the source body, commit, then sync to
    // advance the recorded commit (mirrors the README walkthrough).
    write(
        &sb.source.join("skills/audit/SKILL.md"),
        "---\nname: audit\ndescription: Audit the change\n---\n# audit skill\nedited body\n",
    );
    git(&sb.source, &["commit", "-aqm", "edit audit"]);
    assert!(sb.mind(&["sync"]).success);

    // recall (CLI-75, CLI-155): the audit line leads with the `^` stale marker
    // and carries the `(outdated` text.
    let stale = sb.mind(&["--ascii", "recall"]);
    assert!(stale.success, "{}", stale.stderr);
    let line = stale
        .stdout
        .lines()
        .find(|l| l.contains("skill:audit"))
        .unwrap_or_else(|| panic!("no audit line in recall output: {}", stale.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
    assert!(
        line.contains("(outdated"),
        "the stale line must carry the (outdated; run mind upgrade) text: {line:?}"
    );

    // introspect (CLI-90, LIFE-33): reports the drift and a nonzero issue count.
    let ins = sb.mind(&["--ascii", "introspect"]);
    assert!(
        ins.stdout.contains("skill:audit") && ins.stdout.contains("upstream changed"),
        "introspect must report audit's upstream change: {}",
        ins.stdout
    );
    assert!(
        ins.stdout.contains("issue(s) found") && !ins.stdout.contains("0 issue(s) found"),
        "introspect must report a nonzero issue count: {}",
        ins.stdout
    );

    // upgrade --yes (LIFE-11, LIFE-13): reports the hash and commit `->` deltas
    // and reinstalls under the same name. Assert on shape, not literal hex.
    let up = sb.mind(&["--ascii", "upgrade", "--yes"]);
    assert!(up.success, "{} {}", up.stdout, up.stderr);
    assert!(
        up.stdout.contains("hash") && up.stdout.contains("->"),
        "upgrade must report the hash delta with an arrow: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("commit"),
        "upgrade must report the commit delta: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("upgraded skill:audit"),
        "upgrade must apply audit under the same name: {}",
        up.stdout
    );

    // After upgrade: recall shows audit current (marker back to `+`, no outdated).
    let after = sb.mind(&["--ascii", "recall"]);
    assert!(after.success, "{}", after.stderr);
    let line = after
        .stdout
        .lines()
        .find(|l| l.contains("skill:audit"))
        .unwrap_or_else(|| panic!("no audit line in recall output: {}", after.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('+'),
        "a current install must lead with the `+` marker after upgrade: {line:?}"
    );
    assert!(
        !line.contains("(outdated"),
        "the line must not carry the outdated text after upgrade: {line:?}"
    );
}

#[test]
fn recall_status_view_marks_install_state() {
    // spec: CLI-70, CLI-74
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let out = sb.mind(&["recall"]).stdout;
    // The source header is present, with its items nested and marked.
    assert!(out.contains("agents"), "source header: {out}");
    assert!(
        out.contains("skill:review") && out.contains("installed @"),
        "an installed item is marked installed with its commit: {out}"
    );
    assert!(
        out.contains("agent:dev") && out.contains("available"),
        "a not-installed item is marked available: {out}"
    );
}

/// Meld a local directory source, learn an item, edit the item source file in
/// place (no commit), then check that `mind recall` marks the item outdated.
/// A local linked source is read live from its working tree, so a content
/// change changes the hash while the commit is unchanged.
// spec: CLI-75
#[test]
fn recall_marks_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Verify the item is initially NOT marked outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // Edit the item source file in place without committing. For a linked local
    // source this changes the content hash while the commit is unchanged.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Now `mind recall` must mark skill:review as outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outdated"),
        "recall must mark the item outdated after an in-place content edit: {}",
        r.stdout
    );
}

/// The `probe` non-interactive listing must mark a drifted installed item out of
/// date, and must NOT mark a clean installed item. No other test exercises the
/// probe surface for CLI-75.
// spec: CLI-75
#[test]
fn probe_marks_installed_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Clean: probe must not flag any item out of date.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed items must not be outdated in probe: {}",
        r.stdout
    );

    // Edit one item's source file in place (no commit) -> hash drift only.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nchanged\n",
    );

    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    let review = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review row in probe: {}", r.stdout));
    assert!(
        review.contains("outdated"),
        "probe must mark the drifted item outdated: {review:?}\n{}",
        r.stdout
    );
    // The untouched agent row must remain clean.
    let dev = r
        .stdout
        .lines()
        .find(|l| l.contains("agent:dev"))
        .unwrap_or_else(|| panic!("no dev row in probe: {}", r.stdout));
    assert!(
        !dev.contains("outdated"),
        "an unedited item must not be marked outdated in probe: {dev:?}"
    );
}

/// Re-melding an already-melded local source whose working tree was edited in
/// place reaches the `source_status` view, which must mark the drifted item out
/// of date.
// spec: CLI-75
#[test]
fn remeld_source_status_marks_item_outdated_after_in_place_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // In-place edit (no commit) so only the content hash drifts.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nremeld-edit\n",
    );

    // Re-meld the already-melded source: all items already installed, so this
    // falls through to source_status.
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("already melded"),
        "expected the already-melded status view: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("outdated"),
        "source_status via re-meld must mark the drifted item outdated: {}",
        r.stdout
    );
}

/// Regression: a commit that advances the source WITHOUT changing a given item's
/// content must NOT mark that item outdated in `recall`. The outdated marker must
/// match `upgrade`'s pending condition (LIFE-11): hash drift OR rename, not
/// commit advance. Before the fix the commit-only advance produced a permanent
/// false-positive marker that `mind upgrade` would then report as "up to date".
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_does_not_mark_item_outdated_after_commit_only_advance() {
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean install: not outdated.
    let r = sb.mind(&["recall"]);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // Advance the source commit by touching an UNRELATED file, so the source
    // commit moves past the installed item's commit but the item content is
    // unchanged. sync updates the recorded source commit.
    sb.write_and_commit("CHANGES.md", "unrelated change\n");
    assert!(sb.mind(&["sync"]).success);

    // recall must NOT mark the unchanged item outdated.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "recall must NOT mark the item outdated after a commit-only advance (content unchanged): {}",
        r.stdout
    );

    // upgrade must agree: nothing pending for this item.
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("everything is up to date"),
        "upgrade must report everything up to date after a commit-only advance: {}",
        r.stdout
    );
}

/// The marker is a human-view affordance only: `recall --json` output must be
/// byte-identical before and after a content edit drifts the item.
// spec: CLI-75
#[test]
fn recall_json_is_unchanged_by_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let before_status = sb.mind(&["recall", "--json"]);
    let before_detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(before_status.success && before_detail.success);

    // Drift the item in place.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\njson-drift\n",
    );

    let after_status = sb.mind(&["recall", "--json"]);
    let after_detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(after_status.success && after_detail.success);

    assert_eq!(
        before_status.stdout, after_status.stdout,
        "recall --json status output must not change with drift"
    );
    assert_eq!(
        before_detail.stdout, after_detail.stdout,
        "recall <item> --json output must not change with drift"
    );
    assert!(
        !after_status.stdout.contains("outdated") && !after_status.stdout.contains("out of date"),
        "JSON must carry no human out-of-date marker: {}",
        after_status.stdout
    );
}

/// Regression: `probe --no-tui` must not mark an installed item outdated when
/// the source commit advanced without changing that item's content. Both `recall`
/// and `probe` must agree with `upgrade` on what is pending (CLI-75 / LIFE-11).
// spec: CLI-75
// spec: LIFE-11
#[test]
fn probe_does_not_mark_item_outdated_after_commit_only_advance() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Clean: probe must not flag any item out of date.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed items must not be outdated in probe: {}",
        r.stdout
    );

    // Advance the source by committing an unrelated file; review content unchanged.
    sb.write_and_commit("NOTES.md", "unrelated\n");
    assert!(sb.mind(&["sync"]).success);

    // probe must still not flag the unchanged item.
    let r = sb.mind(&["probe", "--no-tui"]);
    assert!(r.success, "probe failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "probe must NOT mark item outdated after commit-only advance: {}",
        r.stdout
    );

    // recall must also not flag it.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "recall must NOT mark item outdated after commit-only advance: {}",
        r.stdout
    );

    // upgrade confirms: nothing pending.
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("everything is up to date"),
        "upgrade must report everything up to date after commit-only advance: {}",
        r.stdout
    );
}

/// After a commit that also changes item content, recall and probe must still
/// mark the item outdated (hash drift triggers the marker regardless of commit).
/// This ensures the commit-only fix did not regress content-drift detection.
// spec: CLI-75
#[test]
fn recall_still_marks_item_outdated_after_commit_with_content_change() {
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    // Clean install: not outdated.
    let r = sb.mind(&["recall"]);
    assert!(
        !r.stdout.contains("outdated"),
        "freshly installed item must not be outdated: {}",
        r.stdout
    );

    // edit_source changes the review skill content AND commits.
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    // recall must mark the item outdated because content (hash) changed.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outdated"),
        "recall must mark item outdated when commit also changed content: {}",
        r.stdout
    );
}

/// PRIMARY GAP: the `rename_lag` half of the fix. An item whose effective NAME
/// changed (a namespace/prefix rename) but whose content hash did NOT must be
/// marked outdated by `recall` (status view) and `probe --no-tui`, and `upgrade`
/// must report it pending as a rename. The drift is created without re-melding
/// `--as` (which would apply the rename immediately): the source declares a
/// `[source].prefix` in `mind.toml` after install, so the catalog's effective
/// name (`jk:review`) diverges from the still-recorded manifest name (`review`)
/// with the item's SKILL.md content byte-identical (the hash is of the item
/// content, not mind.toml -- LIFE-15). The four surfaces must agree with upgrade.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_and_probe_mark_item_outdated_on_rename_without_content_change() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Sanity: freshly installed, unprefixed, nothing outdated anywhere.
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("outdated"),
        "fresh install must not be outdated in recall: {}",
        recall.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        !probe.stdout.contains("outdated"),
        "fresh install must not be outdated in probe: {}",
        probe.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        !detail.stdout.contains("out of date"),
        "fresh install single-item must not be out of date: {}",
        detail.stdout
    );

    // Introduce a namespace prefix via mind.toml WITHOUT re-melding. After sync,
    // the catalog computes effective name `jk:review` while the manifest still
    // records `review`; the SKILL.md content is unchanged so the hash matches.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    // probe --no-tui listing: must mark the renamed item outdated.
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "probe failed: {}", probe.stderr);
    assert!(
        probe.stdout.contains("outdated"),
        "probe must mark a renamed item outdated: {}",
        probe.stdout
    );

    // recall single-item detail: looked up by the OLD installed name, still
    // present (matched by stable identity); must report out of date.
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(detail.success, "recall detail failed: {}", detail.stderr);
    assert!(
        detail.stdout.contains("out of date"),
        "recall single-item detail must report a renamed item out of date: {}",
        detail.stdout
    );

    // upgrade must agree: the item is pending as a rename, not "up to date".
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);
    assert!(
        !up.stdout.contains("everything is up to date"),
        "upgrade must NOT report up to date when an effective name changed: {}",
        up.stdout
    );
    assert!(
        up.stdout.contains("rename")
            && up.stdout.contains("review -> ")
            && up.stdout.contains("jk:review"),
        "upgrade must report the rename review -> jk:review: {}",
        up.stdout
    );
}

/// CLI-75 applies the rename marker to the default `recall` status view too. The
/// status view matches catalog items to the manifest by stable identity
/// `(source, kind, bare_name)`, so a renamed item (effective name `skill:jk:review`
/// vs the manifest's `skill:review`) still lands in the matched arm and `rename_lag`
/// marks it outdated, rather than being misreported as `available` + an orphan
/// `(removed upstream)`. The status view agrees with `probe`, the single-item
/// detail, and `upgrade`.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn recall_status_view_marks_renamed_item_outdated() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);
    // The renamed item must be marked outdated, not shown as removed-upstream.
    assert!(
        recall.stdout.contains("outdated"),
        "recall status view must mark a renamed (effective-name-changed) item \
         outdated to agree with probe/detail/upgrade: {}",
        recall.stdout
    );
    assert!(
        !recall.stdout.contains("removed upstream"),
        "a pure namespace rename must not be reported as removed upstream: {}",
        recall.stdout
    );
}

/// FOUR-SURFACE CONSISTENCY (hash-drift case): for one in-place content edit
/// (no new commit, no rename), `recall` status, `recall <item>` detail,
/// `probe --no-tui`, and `upgrade` must all agree the item is changed. None may
/// call it current that upgrade would change, and `upgrade` applying it must
/// clear the marker on all surfaces afterwards.
// spec: CLI-75
// spec: LIFE-11
#[test]
fn all_four_surfaces_agree_on_hash_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Edit the item content in place WITHOUT a new commit (local-source drift).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nfour-surface-drift\n",
    );

    // All three human views must flag it, matching what upgrade will do.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("outdated"),
        "recall status must flag hash drift: {}",
        recall.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        detail.stdout.contains("out of date"),
        "recall single-item detail must flag hash drift: {}",
        detail.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("outdated"),
        "probe must flag hash drift: {}",
        probe.stdout
    );

    // upgrade is the source of truth: it must find this item pending and apply it.
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);
    assert!(
        !up.stdout.contains("everything is up to date"),
        "upgrade must act on the drifted item: {}",
        up.stdout
    );

    // After upgrade, every surface must agree the item is now current.
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("outdated"),
        "recall must be clean after upgrade: {}",
        recall.stdout
    );
    let detail = sb.mind(&["recall", "skill:review"]);
    assert!(
        !detail.stdout.contains("out of date"),
        "recall detail must be clean after upgrade: {}",
        detail.stdout
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        !probe.stdout.contains("outdated"),
        "probe must be clean after upgrade: {}",
        probe.stdout
    );
}

/// The "outdated; run mind upgrade" marker is a HUMAN-view concern only (CLI-75):
/// the JSON outputs must never carry it. This complements
/// `recall_json_is_unchanged_by_drift` (hash-drift case) by covering the rename
/// case across `recall --json`, `recall <item> --json`, and `probe --json`.
///
/// Note: the JSON bytes DO legitimately change under a rename drift -- the synced
/// commit advances and the catalog's effective keys are now prefixed -- so this
/// asserts the real invariant (no human marker string leaks) rather than byte
/// equality, which holds only for an in-place edit with no commit/key change.
// spec: CLI-75
// spec: CLI-73
// spec: CLI-84
#[test]
fn json_outputs_carry_no_outdated_marker_under_rename_drift() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Rename drift via mind.toml prefix (item content unchanged), then sync.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall", "--json"]);
    let detail = sb.mind(&["recall", "skill:review", "--json"]);
    let probe = sb.mind(&["probe", "--json"]);
    assert!(recall.success && detail.success && probe.success);

    for (label, body) in [
        ("recall --json", &recall.stdout),
        ("recall detail --json", &detail.stdout),
        ("probe --json", &probe.stdout),
    ] {
        // The JSON must parse and must carry no human out-of-date marker text.
        let _: serde_json::Value =
            serde_json::from_str(body).unwrap_or_else(|e| panic!("{label} not valid JSON: {e}"));
        assert!(
            !body.contains("outdated") && !body.contains("out of date"),
            "{label} must carry no human out-of-date marker: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Certification of the orphan-classification rework (recall status view matches
// catalog<->manifest by stable identity (source, kind, bare_name); orphans_of
// flags removed-upstream only when NO catalog item shares that identity).
//
// The dev shard covered the rename half (the item must be marked outdated on the
// four surfaces). These add the adversarial edges that were not covered:
//  - a renamed item must appear EXACTLY ONCE (no double-listing as both an
//    outdated row and an orphan/removed-upstream row), in human view and JSON;
//  - a genuine removal must STILL show (removed upstream) in the human recall
//    view and as installed-but-no-catalog-match (orphaned) in recall --json;
//  - identity is (source, kind, bare_name): a same-named item in another source
//    must not cross-match, so removing/renaming in one source never mislabels the
//    other's item;
//  - the unmanaged-item accounting is unaffected by the orphan change.
// ---------------------------------------------------------------------------

/// EDGE 1 (no double-listing under rename): after a pure prefix rename (effective
/// name changed, content unchanged), the item appears EXACTLY ONCE in the human
/// `recall` status view -- as the installed `(outdated; run mind upgrade)` row --
/// and is NOT ALSO emitted as a `(removed upstream)` orphan. The dev test asserts
/// the markers are present/absent; this one asserts the stronger structural
/// property by COUNTING the rows that mention the item, so a regression that
/// reintroduced both an outdated row and an orphan row would be caught.
// spec: CLI-75
#[test]
fn recall_status_renamed_item_appears_exactly_once_no_orphan_dup() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld failed");

    // Rename drift: declare a prefix after install, then sync so the catalog's
    // effective name (`jk:review`) diverges from the recorded manifest name
    // (`review`) with the SKILL.md content byte-identical.
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // Exactly one line carries the review-skill key (either the installed name
    // `skill:review` or the catalog's new effective name `skill:jk:review`).
    // The `skill:` prefix anchors to an item row and avoids spurious matches
    // on output chrome that merely contains the word "review".
    let review_lines: Vec<&str> = recall
        .stdout
        .lines()
        .filter(|l| l.contains("skill:review") || l.contains("skill:jk:review"))
        .collect();
    assert_eq!(
        review_lines.len(),
        1,
        "the renamed item must appear on exactly one row (no orphan dup), got: {:#?}",
        review_lines
    );
    assert!(
        review_lines[0].contains("outdated"),
        "the single review row must be the outdated row: {}",
        review_lines[0]
    );
    assert!(
        !recall.stdout.contains("removed upstream"),
        "a pure rename must not be flagged removed upstream: {}",
        recall.stdout
    );
}

/// EDGE 5 (JSON under rename): in `recall --json` the renamed item resolves to its
/// manifest entry by stable identity -- `installed:true` with the correct commit
/// -- and is emitted EXACTLY ONCE, never duplicated as a separate `orphaned:true`
/// row. The single-item `recall <item> --json`, looked up by the OLD installed
/// name, also resolves correctly.
// spec: CLI-73
// spec: CLI-75
#[test]
fn recall_json_renamed_item_installed_once_not_orphaned() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld failed");

    // Record the install commit so we can assert the JSON carries it.
    // spec: CLI-167 - recall --json now wrapped in {"schema":1,"items":[...]}.
    let before = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let source_commit = before["items"][0]["commit"].as_str().unwrap().to_string();

    sb.write_and_commit("mind.toml", "[source]\nprefix = \"jk\"\n");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    let j = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let items = j["items"][0]["items"].as_array().expect("items array");

    // The skill must appear under its NEW effective key, installed, with the
    // commit it was installed at, and must not be duplicated as an orphan.
    let review_rows: Vec<&serde_json::Value> = items
        .iter()
        .filter(|r| {
            let k = r["key"].as_str().unwrap_or("");
            k == "skill:review" || k == "skill:jk:review"
        })
        .collect();
    assert_eq!(
        review_rows.len(),
        1,
        "the renamed skill must be emitted exactly once in recall --json: {items:#?}"
    );
    let row = review_rows[0];
    assert_eq!(
        row["key"].as_str(),
        Some("skill:jk:review"),
        "the renamed item must carry its new effective key: {row}"
    );
    assert_eq!(
        row["installed"].as_bool(),
        Some(true),
        "the renamed item must resolve installed by stable identity: {row}"
    );
    assert_eq!(
        row["commit"].as_str(),
        Some(source_commit.as_str()),
        "the renamed item must carry its install commit: {row}"
    );
    assert!(
        row.get("orphaned").is_none(),
        "the renamed item must not be flagged orphaned: {row}"
    );
    // No item in this source's JSON should be orphaned at all.
    assert!(
        !items.iter().any(|r| r.get("orphaned").is_some()),
        "no catalog-matched item may be reported orphaned under a pure rename: {items:#?}"
    );

    // The single-item lookup by the OLD installed name still resolves.
    let detail = sb.mind(&["recall", "skill:review", "--json"]);
    assert!(
        detail.success,
        "recall detail --json failed: {}",
        detail.stderr
    );
    let d = parse_json(&detail.stdout);
    assert_eq!(
        d["name"].as_str(),
        Some("review"),
        "the single-item lookup resolves by the recorded (old) name: {d}"
    );
}

/// EDGE 2 (genuine removal still works): an item actually deleted from the source
/// (file removed, then sync) has NO catalog item sharing its identity, so it is a
/// real orphan. It must still show `(removed upstream)` in the human `recall` view
/// AND appear as installed-but-no-catalog-match (orphaned, installed:true) in
/// `recall --json`. The orphan rework must not have regressed this.
// spec: CLI-73
// spec: CLI-75
#[test]
fn removed_upstream_still_flagged_in_recall_human_and_json() {
    let sb = melded();
    assert!(sb.mind(&["learn", "dev"]).success, "learn dev failed");

    // The agent disappears upstream, then sync drops it from the catalog.
    sb.remove_and_commit("agents/dev.md");
    assert!(sb.mind(&["sync"]).success, "sync failed");

    // Human view: the installed-but-removed item is flagged removed upstream.
    let recall = sb.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);
    assert!(
        recall.stdout.contains("agent:dev"),
        "the removed item must still be listed: {}",
        recall.stdout
    );
    assert!(
        recall.stdout.contains("removed upstream"),
        "a genuinely removed item must be flagged removed upstream: {}",
        recall.stdout
    );

    // JSON: the removed item appears as installed:true with orphaned:true (no
    // catalog match). The review skill (still in the catalog) is not orphaned.
    // spec: CLI-167 - recall --json wrapped in envelope.
    let j = parse_json(&sb.mind(&["recall", "--json"]).stdout);
    let items = j["items"][0]["items"].as_array().expect("items array");
    let dev = items
        .iter()
        .find(|r| r["key"].as_str() == Some("agent:dev"))
        .expect("the removed agent must be present in recall --json");
    assert_eq!(
        dev["installed"].as_bool(),
        Some(true),
        "the removed-upstream item is still installed: {dev}"
    );
    assert_eq!(
        dev["orphaned"].as_bool(),
        Some(true),
        "the removed-upstream item must be flagged orphaned in JSON: {dev}"
    );
    assert!(
        !items
            .iter()
            .any(|r| r["key"].as_str() == Some("skill:review") && r.get("orphaned").is_some()),
        "a still-cataloged item must not be orphaned: {items:#?}"
    );
}

/// EDGE 3 (same bare name across two sources does not cross-match): identity is
/// (source, kind, bare_name). Two melded sources each ship a `review` skill (the
/// second namespaced so both can install). Removing `review` from source A must
/// flag ONLY A's item as removed upstream and must NOT mislabel B's `zz:review`,
/// which is unchanged. This is the isolation the rework depends on: A's orphan
/// scan must not match B's manifest entry by bare name, and B's catalog must not
/// rescue A's removed item.
// spec: CLI-75
#[test]
fn same_bare_name_across_sources_does_not_cross_match_on_removal() {
    let a = Sandbox::new();
    let b = Sandbox::new();
    assert!(a.mind(&["meld", &a.source_spec()]).success, "meld a");
    assert!(
        a.mind(&["meld", &b.source_spec(), "--as", "zz"]).success,
        "meld b as zz"
    );

    // Both review skills install side by side under distinct effective names.
    assert!(a.mind(&["learn", "review"]).success, "learn review (a)");
    assert!(
        a.mind(&["learn", "zz:review"]).success,
        "learn zz:review (b)"
    );

    // Remove review from source A only, then sync.
    a.remove_and_commit("skills/review/SKILL.md");
    assert!(a.mind(&["sync"]).success, "sync failed");

    let recall = a.mind(&["recall"]);
    assert!(recall.success, "recall failed: {}", recall.stderr);

    // A's review is removed upstream; B's zz:review is untouched (not flagged).
    let removed_lines: Vec<&str> = recall
        .stdout
        .lines()
        .filter(|l| l.contains("removed upstream"))
        .collect();
    assert_eq!(
        removed_lines.len(),
        1,
        "exactly one item (A's review) must be removed upstream: {:#?}",
        removed_lines
    );
    assert!(
        removed_lines[0].contains("skill:review") && !removed_lines[0].contains("zz:review"),
        "the removed-upstream row must be A's review, not B's zz:review: {}",
        removed_lines[0]
    );

    // JSON confirms the cross-match isolation: A's review orphaned, B's
    // zz:review installed and NOT orphaned.
    // spec: CLI-167 - recall --json wrapped in envelope.
    let jj = parse_json(&a.mind(&["recall", "--json"]).stdout);
    let sources = jj["items"].as_array().expect("sources array");
    let mut saw_review_orphan = false;
    let mut saw_zz_review_ok = false;
    for s in sources {
        for r in s["items"].as_array().unwrap() {
            match r["key"].as_str() {
                Some("skill:review") => {
                    assert_eq!(
                        r["orphaned"].as_bool(),
                        Some(true),
                        "A's review must be orphaned: {r}"
                    );
                    saw_review_orphan = true;
                }
                Some("skill:zz:review") => {
                    assert!(
                        r.get("orphaned").is_none(),
                        "B's zz:review must not be orphaned: {r}"
                    );
                    assert_eq!(
                        r["installed"].as_bool(),
                        Some(true),
                        "B's zz:review must stay installed: {r}"
                    );
                    saw_zz_review_ok = true;
                }
                _ => {}
            }
        }
    }
    assert!(
        saw_review_orphan && saw_zz_review_ok,
        "both A's orphaned review and B's intact zz:review must be present: {jj:#?}"
    );
}
