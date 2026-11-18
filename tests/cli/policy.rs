//! Managed policy (spec/policy.md).

use super::*;

#[test]
fn example_policy_validates() {
    // spec: POL-50
    // The shipped example managed policy validates clean via `review --policy`,
    // so the example cannot rot as the policy parser/validator changes.
    let sb = Sandbox::new();
    let policy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/policy/policy.toml");
    let r = sb.mind(&["review", "--policy", policy.to_str().unwrap()]);
    assert!(
        r.success,
        "example policy must validate clean:\nstdout: {}\nstderr: {}",
        r.stdout, r.stderr
    );
}

#[test]
fn meld_refused_when_not_in_allow_and_locked() {
    // spec: POL-11
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // allow lists a different repo name; lock enforces it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "locked non-allowed meld must fail: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted") || r.stderr.contains("not permitted by the managed"),
        "error should mention the source is not permitted: {}",
        r.stderr
    );
    // Nothing registered and no clone left on disk for the source. The source's
    // clone dir is sources/local/<base>/agents; the refusal removes it.
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should be left at {}",
        clone_dir.display()
    );
}

#[test]
fn meld_allowed_when_not_in_allow_but_unlocked() {
    // spec: POL-13
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // lock is off, so allow is advisory: a non-match warns but proceeds.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = false\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "unlocked non-allowed meld must succeed: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("advisory") || r.stderr.contains("not in the managed policy"),
        "a warning should be printed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn policy_is_authoritative_over_explicit_user_meld() {
    // spec: POL-3
    // The user explicitly asks to meld this exact source, but a locked policy
    // that does not allow it refuses regardless of user intent.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "policy must override the user's explicit meld request: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted"),
        "refusal should be explained: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn meld_pinned_policy_refuses_floating_branch_and_allows_tag() {
    // spec: POL-20
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-policy");
    let spec = sb.source_spec();
    // pinned requires a tag/ref. allow matches this repo so only the pin gates.
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pintest-policy\"]\n",
    );

    // No pin flag => default branch => refused.
    let floating = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !floating.success,
        "pinned policy must refuse a default-branch meld: {}",
        floating.stdout
    );
    assert!(
        floating.stderr.contains("must be pinned"),
        "refusal should mention pinning: {}",
        floating.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on refusal");

    // A tag pin satisfies the policy.
    let tagged = sb.mind_env(
        &["meld", &spec, "--pin-tag", "v1.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        tagged.success,
        "pinned policy must accept a --pin-tag meld: {}",
        tagged.stderr
    );
    assert_eq!(source_count(&sb), 1, "tagged source should be registered");
}

#[test]
fn learn_skips_disallowed_source_when_locked() {
    // spec: POL-12
    // Meld under no policy, then apply a locked policy that no longer allows the
    // source: learn must skip it with a notice and not error.
    let sb = melded(); // melds + learns nothing extra; source is registered
    // Confirm the source is present and not yet learned beyond `review`.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["learn", "agent:dev"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "learn must not error when skipping disallowed sources: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "learn should report the skipped source: {}",
        r.stdout
    );
    // The item was not installed.
    let recall = sb.mind_env(
        &["recall", "agent:dev"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.success,
        "the disallowed item must not be installed: {}",
        recall.stdout
    );
}

#[test]
fn sync_skips_disallowed_source_when_locked() {
    // spec: POL-12
    let sb = melded();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync must not error on a skipped source: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "sync should report the skipped source: {}",
        r.stdout
    );
}

#[test]
fn sync_provisions_auto_meld_and_is_idempotent() {
    // spec: POL-32
    // The policy declares an auto_meld entry (pinned to a tag). `sync` provisions
    // it: the source appears in the registry. A second sync is a no-op (no new
    // source, no error).
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-src");
    let spec = sb.source_spec();
    // lock/pinned off so the entry validates without an allow/pin match check on
    // the path spec; the entry itself carries a tag pin.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{spec}\"\ntag = \"v1.0\"\n",
        spec = spec.replace('\\', "\\\\")
    );
    let policy = write_policy(&sb, &body);

    assert_eq!(source_count(&sb), 0, "registry starts empty");
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "auto-meld sync should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "auto_meld entry should be provisioned into the registry: {}",
        r.stdout
    );

    // Idempotent: a second sync provisions nothing new and still succeeds.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second sync should succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "auto-meld provisioning must be idempotent: {}",
        r2.stdout
    );
}

#[test]
fn config_lobes_add_refused_when_lobes_locked() {
    // spec: POL-40
    let sb = Sandbox::named("agents");
    let policy = write_policy(&sb, "[lobes]\nlock = true\ntargets = [\"~/.claude\"]\n");

    // The candidate lobe lives INSIDE the sandbox, not at a shared `/tmp` path.
    // HARN-15 made `config lobes add` create the lobe directory; today the
    // POL-40 refusal returns before that `mkdir_p`, but relying on that ordering
    // would leave a real directory outside the sandbox the moment the order
    // changes. The assertion below also proves the refusal happened before the
    // create, which the shared path could not distinguish from "someone else
    // made it".
    let candidate = sb.base.join("refused-lobe");
    let candidate_str = candidate.to_string_lossy().into_owned();

    // Snapshot the lobe list before the refused add.
    let before = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(before.success, "list before: {}", before.stderr);

    let r = sb.mind_env(
        &["config", "lobes", "add", &candidate_str],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "locked lobes add must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("lock") || r.stderr.contains("refused") || r.stderr.contains("pinned"),
        "refusal should be explained: {}",
        r.stderr
    );
    assert!(
        !candidate.exists(),
        "a refused lobe must not have its directory created (HARN-15 runs only \
         after the POL-40 check): {candidate:?}"
    );

    // The lobe list is unchanged: the path was not added.
    let after = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !after.stdout.contains(&candidate_str),
        "the refused lobe must not appear: {}",
        after.stdout
    );
}

#[test]
fn upgrade_skips_disallowed_source_when_locked() {
    // spec: POL-12
    // upgrade operates only on sources whose identity matches allow. Meld + learn
    // under no policy, drift the source upstream so an upgrade is pending, then
    // run upgrade under a locked policy that no longer allows the source: the
    // pending upgrade is reported as skipped (not applied) and upgrade exits zero.
    let sb = melded();
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    // Drift the source and refresh the clone (unmanaged sync), so the catalog now
    // differs from the installed hash and upgrade would otherwise apply it.
    sb.edit_source();
    let synced = sb.mind(&["sync"]);
    assert!(
        synced.success,
        "sync failed: {} {}",
        synced.stdout, synced.stderr
    );

    // Now a locked policy that does not allow the source. upgrade must skip it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "--yes"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "upgrade must not error when skipping disallowed sources: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("skipping") && r.stdout.contains("not permitted"),
        "upgrade should report the skipped source: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("upgraded"),
        "the disallowed item must not be upgraded: {}",
        r.stdout
    );

    // The installed item is unchanged: its recorded commit/hash did not advance.
    // Extract the commit line from each output and compare only that, because the
    // "out of date" status line legitimately differs (the source has drifted but
    // the upgrade was blocked; the displayed outdated marker is expected).
    let after = sb
        .mind_env(
            &["recall", "skill:review"],
            &[("MIND_POLICY_FILE", policy.as_str())],
        )
        .stdout;
    let commit_before = before.lines().find(|l| l.contains("commit")).unwrap_or("");
    let commit_after = after.lines().find(|l| l.contains("commit")).unwrap_or("");
    assert_eq!(
        commit_before, commit_after,
        "the skipped item's recorded commit must not advance: before={before} after={after}"
    );
    let hash_before = before.lines().find(|l| l.contains("hash")).unwrap_or("");
    let hash_after = after.lines().find(|l| l.contains("hash")).unwrap_or("");
    assert_eq!(
        hash_before, hash_after,
        "the skipped item's recorded hash must not advance: before={before} after={after}"
    );
}

#[test]
fn upgrade_applies_allowed_source_while_skipping_disallowed() {
    // spec: POL-12
    // The "rest proceed" half of POL-12: a locked allowlist that matches the
    // source lets upgrade apply the pending upgrade (the same drift that is skipped
    // in the test above is applied here because the source matches allow).
    let sb = melded();
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    sb.edit_source();
    let synced = sb.mind(&["sync"]);
    assert!(
        synced.success,
        "sync failed: {} {}",
        synced.stdout, synced.stderr
    );

    // The allow pattern matches this sandbox's local identity, so the lock does
    // not exclude it; the pending upgrade applies.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "--yes"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("upgraded skill:review"),
        "an allowed source must be upgraded: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skipping"),
        "an allowed source must not be skipped: {}",
        r.stdout
    );
}

#[test]
fn sync_provisions_auto_meld_under_lock_and_is_idempotent() {
    // spec: POL-32
    // The locked + pinned + allowed round-trip: a locked policy whose auto_meld
    // entry is pinned to a tag and satisfies allow (POL-31) is provisioned by
    // sync, and re-provisioning is idempotent. This exercises the full enforced
    // path (the meld inside provisioning runs under the same locked policy), not
    // just the unlocked provisioning above.
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-locked");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // allow must satisfy BOTH allowlist checks for this entry: POL-31 policy
    // validation matches the raw `repo` string (here the local fixture path), and
    // runtime meld enforcement matches the parsed identity `local/<base>/<name>`.
    // For a real `host/owner/repo` spec these coincide; for a local-path fixture
    // they differ in segment shape, so the allow list carries one pattern for each
    // form: `<base>/*` for the raw path and `local/*/automeld-locked` for the
    // identity. Both use a single-segment `*`, never crossing a `/`.
    let raw_pat = sb.base.join("*").to_string_lossy().replace('\\', "\\\\");
    let body = format!(
        "[sources]\nlock = true\npinned = true\nallow = [\"{raw_pat}\", \"local/*/automeld-locked\"]\n\n[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n"
    );
    let policy = write_policy(&sb, &body);

    assert_eq!(source_count(&sb), 0, "registry starts empty");
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "locked+pinned auto-meld sync should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the allowed+pinned auto_meld entry should be provisioned under lock: {}",
        r.stdout
    );

    // The recorded pin is the declared tag, not a floating branch.
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\"") && pin_json.contains("v1.0"),
        "auto_meld entry should be provisioned at its declared tag pin: {pin_json}"
    );

    // Idempotent under the same locked policy: no second provisioning, no error.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second locked sync should succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "locked auto-meld provisioning must be idempotent: {}",
        r2.stdout
    );
}

#[test]
fn auto_meld_entry_already_melded_is_not_remelded() {
    // spec: POL-32
    // spec: POL-55
    // When the recorded pin already equals the declared pin, the entry is left
    // unchanged: no re-meld, no re-pin message, no duplicate registration.
    let (sb, _v1, _v2) = make_pinnable_repo("automeld-pre");
    let spec = sb.source_spec();

    // User melds it first (unmanaged), pinned to the tag.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(
        pre.success,
        "pre-meld failed: {} {}",
        pre.stdout, pre.stderr
    );
    assert_eq!(source_count(&sb), 1, "source melded once");

    // A policy whose auto_meld declares the same pin: idempotent, no action.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);
    assert_eq!(
        source_count(&sb),
        1,
        "an already-melded auto_meld entry at the same pin must not be re-melded: {}",
        r.stdout
    );
    // No re-pin event: pins matched, so no message should appear.
    assert!(
        !r.stdout.contains("re-pinned"),
        "matching pin must not emit a re-pinned message: {}",
        r.stdout
    );
    // The recorded pin is still v1.0 (unchanged).
    let pin_json = read_source_pin_json(&sb);
    assert!(
        pin_json.contains("\"tag\"") && pin_json.contains("v1.0"),
        "pin must remain at tag v1.0: {pin_json}"
    );
}

#[test]
fn auto_meld_pin_bump_reconciles_on_sync() {
    // spec: POL-55
    // When a policy's auto_meld entry declares a pin that differs from the
    // registered source's recorded pin, sync updates the recorded pin and reports
    // "re-pinned <name> <old> -> <new>". The source stays registered (no
    // unmeld/remeld): only the pin field is updated so the subsequent sync fetch
    // lands the new ref.
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("automeld-repin");
    let spec = sb.source_spec();

    // Add a v2.0 tag at the second commit (sha_v2, the tip of main after
    // make_pinnable_repo advanced it).
    git(&sb.source, &["tag", "v2.0"]);

    // First: pre-provision the source at v1.0.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);
    assert_eq!(source_count(&sb), 1, "one source registered");
    let pin_before = read_source_pin_json(&sb);
    assert!(
        pin_before.contains("\"tag\"") && pin_before.contains("v1.0"),
        "initial pin must be tag v1.0: {pin_before}"
    );

    // IT bumps the policy pin to v2.0.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with bumped pin failed: {} {}",
        r.stdout, r.stderr
    );

    // The source is still registered exactly once (no unmeld/remeld).
    assert_eq!(
        source_count(&sb),
        1,
        "source count must remain 1 after re-pin"
    );

    // The re-pin message must appear in the sync output.
    assert!(
        r.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("v1.0") && r.stdout.contains("v2.0"),
        "re-pinned message must name old and new pins: {}",
        r.stdout
    );

    // The recorded pin is now v2.0.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "recorded pin must be updated to tag v2.0: {pin_after}"
    );
    assert!(
        !pin_after.contains("v1.0"),
        "old pin v1.0 must not remain in sources.json: {pin_after}"
    );
}

#[test]
fn auto_meld_repin_default_branch_to_follow_branch_reconciles() {
    // spec: POL-55
    // A re-pin transition with NO Tag on either side: the recorded pin is the
    // default branch (no explicit pin) and the policy declares a follow_branch.
    // sync reconciles the drift, reports it with the human pin descriptions
    // ("default branch" -> "branch <b>"), lands the branch tip, and leaves the
    // source registered exactly once. Guards the non-Tag pin_description arms and
    // the DefaultBranch == am.pin comparison for a non-Tag declared pin.
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("automeld-db-to-branch");
    let spec = sb.source_spec();

    // Pre-meld with no pin: recorded pin is DefaultBranch, commit at main tip.
    let pre = sb.mind(&["meld", &spec]);
    assert!(
        pre.success,
        "pre-meld (default branch) failed: {}",
        pre.stderr
    );
    assert_eq!(source_count(&sb), 1, "one source registered");
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "a default-branch meld records the main tip (sha_v2)"
    );
    let pin_before = read_source_pin_json(&sb);
    assert!(
        pin_before.contains("default-branch"),
        "initial pin must be default-branch: {pin_before}"
    );

    // Policy declares follow_branch = stable (which points at sha_v1).
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\nfollow_branch = \"stable\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "re-pin sync failed: {} {}", r.stdout, r.stderr);

    // The transition is reported with both non-Tag pin descriptions.
    assert!(
        r.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("default branch") && r.stdout.contains("branch stable"),
        "re-pin message must name the default-branch -> follow-branch transition: {}",
        r.stdout
    );

    // The recorded pin is now follow-branch stable, and the branch tip landed.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("follow-branch") && pin_after.contains("stable"),
        "recorded pin must be follow-branch stable: {pin_after}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "the follow_branch (stable) tip (sha_v1) must be landed after re-pin"
    );
    assert_eq!(source_count(&sb), 1, "source stays registered exactly once");
}

#[test]
fn auto_meld_repin_lands_new_ref_and_second_sync_is_idempotent() {
    // spec: POL-55
    // spec: POL-32
    // The re-pin-only save path: a sync that provisions NO new source but
    // reconciles one pin must (a) persist the updated pin, (b) actually LAND the
    // new ref -- the recorded commit moves to the new tag's commit, not just the
    // pin field -- and (c) a second sync with the now-matching pin is a silent
    // idempotent no-op (POL-32) that keeps the landed commit.
    let (sb, sha_v1, sha_v2) = make_pinnable_repo("automeld-repin-land");
    let spec = sb.source_spec();
    // Tag the second commit v2.0 (make_pinnable_repo only tags v1.0 at sha_v1).
    git(&sb.source, &["tag", "v2.0"]);

    // Pre-provision at v1.0: recorded commit is sha_v1.
    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);
    assert_eq!(read_source_commit(&sb), sha_v1, "v1.0 pin lands sha_v1");

    // Bump the policy pin to v2.0 and sync.
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "re-pin sync failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("re-pinned"),
        "first sync must re-pin: {}",
        r.stdout
    );

    // The new ref actually landed: recorded commit advanced to sha_v2 on the same
    // sync (the per-source fetch after the re-pin resolved the new tag).
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the re-pinned v2.0 tag's commit (sha_v2) must be fetched and landed on the same sync"
    );
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "recorded pin must be persisted as tag v2.0: {pin_after}"
    );

    // Second sync: pins now match, so it is a silent no-op and the commit holds.
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "second sync failed: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        !r2.stdout.contains("re-pinned"),
        "a matching pin on the second sync must not re-pin again: {}",
        r2.stdout
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the landed commit must remain at sha_v2 after the idempotent second sync"
    );
    assert_eq!(source_count(&sb), 1, "still exactly one source");
}

#[test]
fn auto_meld_repin_message_suppressed_under_json() {
    // spec: POL-55
    // The `if !out.json` guard on the re-pin message: under `sync --json` the
    // human "re-pinned" line must NOT appear on stdout, yet the reconciliation
    // still happens (the recorded pin is updated and the new ref lands).
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("automeld-repin-json");
    let spec = sb.source_spec();
    git(&sb.source, &["tag", "v2.0"]);

    let pre = sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]);
    assert!(pre.success, "pre-meld at v1.0 failed: {}", pre.stderr);

    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\n");
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(
        &["sync", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(r.success, "sync --json failed: {} {}", r.stdout, r.stderr);

    // The human re-pin line must be fully suppressed under --json.
    assert!(
        !r.stdout.contains("re-pinned"),
        "the re-pin message must not be emitted under --json: {}",
        r.stdout
    );

    // But the reconciliation still took effect: pin updated and new ref landed.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "the pin must still be reconciled to v2.0 under --json: {pin_after}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the new ref must still land under --json"
    );
}

#[test]
fn auto_meld_repin_touches_only_the_drifting_source() {
    // spec: POL-55
    // spec: POL-32
    // With several registered sources, only the entry whose recorded pin differs
    // from its declared pin is re-pinned; entries whose pins already match are
    // left untouched (POL-32). Exactly one re-pin line is emitted and it names the
    // drifting source, not the stable one.
    let (sb, _sha_a_v1, sha_a_v2) = make_pinnable_repo("automeld-multi-a");
    let spec_a = sb.source_spec();
    git(&sb.source, &["tag", "v2.0"]); // drift target for A

    // Build a second source repo (B) under the same sandbox base, tagged v1.0.
    let src_b = sb.base.join("automeld-multi-b");
    write(
        &src_b.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev b\n---\n# dev b\n",
    );
    git(&src_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_b, &["config", "user.email", "t@t"]);
    git(&src_b, &["config", "user.name", "t"]);
    git(&src_b, &["add", "-A"]);
    git(&src_b, &["commit", "-qm", "initial"]);
    git(&src_b, &["tag", "v1.0"]);
    let spec_b = src_b.to_string_lossy().into_owned();

    // Pre-meld both, each pinned to v1.0.
    assert!(
        sb.mind(&["meld", &spec_a, "--pin-tag", "v1.0"]).success,
        "meld A at v1.0 failed"
    );
    assert!(
        sb.mind(&["meld", &spec_b, "--pin-tag", "v1.0"]).success,
        "meld B at v1.0 failed"
    );
    assert_eq!(source_count(&sb), 2, "two sources registered");

    // Policy: A drifts to v2.0, B stays at v1.0 (matching -> untouched).
    let esc_a = spec_a.replace('\\', "\\\\");
    let esc_b = spec_b.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{esc_a}\"\ntag = \"v2.0\"\n\n[[sources.auto_meld]]\nrepo = \"{esc_b}\"\ntag = \"v1.0\"\n"
    );
    let policy = write_policy(&sb, &body);
    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "sync failed: {} {}", r.stdout, r.stderr);

    // Exactly one re-pin, and it names A (the drifting source), not B.
    assert_eq!(
        r.stdout.matches("re-pinned").count(),
        1,
        "exactly one source must be re-pinned: {}",
        r.stdout
    );
    let a_name = format!("local/{}/automeld-multi-a", sb.base_name());
    let b_name = format!("local/{}/automeld-multi-b", sb.base_name());
    let repin_line = r
        .stdout
        .lines()
        .find(|l| l.contains("re-pinned"))
        .unwrap_or("");
    assert!(
        repin_line.contains(&a_name),
        "the re-pin line must name the drifting source A: {repin_line}"
    );
    assert!(
        !repin_line.contains(&b_name),
        "the stable source B must not be re-pinned: {repin_line}"
    );

    // Both still registered; A landed its new v2.0 commit (A is first in the
    // registry, having been melded first).
    assert_eq!(source_count(&sb), 2, "both sources remain registered");
    assert_eq!(
        read_source_commit(&sb),
        sha_a_v2,
        "the drifting source A must have landed its new v2.0 commit"
    );
    // The full registry still records B's unchanged v1.0 tag pin alongside A's v2.0.
    let sources_json = read_sources_json(&sb);
    assert!(
        sources_json.contains("v2.0") && sources_json.contains("v1.0"),
        "A must be re-pinned to v2.0 while B keeps v1.0: {sources_json}"
    );
}

#[test]
fn sync_auto_meld_provisioning_soft_fails_on_unreachable_entry() {
    // spec: POL-34
    // When an auto_meld entry cannot be provisioned (here a nonexistent local
    // path that git cannot clone -- fully offline), `sync` warns and continues
    // rather than aborting. The already-melded source still syncs, and the
    // command exits non-zero with the failed entry named in stderr.
    let sb = Sandbox::named("good-src-pol34");
    let good_spec = sb.source_spec();

    // Meld the good source up front (unmanaged).
    let meld = sb.mind(&["meld", &good_spec]);
    assert!(meld.success, "meld good source failed: {}", meld.stderr);
    assert_eq!(source_count(&sb), 1, "good source should be melded");

    // Policy: one entry already melded (idempotent skip), one pointing at a
    // nonexistent path that will fail to clone without any network access.
    let bad_repo = "/nonexistent/path/that/cannot/be/cloned";
    let escaped_good = good_spec.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped_good}\"\n\n[[sources.auto_meld]]\nrepo = \"{bad_repo}\"\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);

    // (b) exits non-zero because provisioning the bad entry failed.
    assert!(
        !r.success,
        "sync must exit non-zero when an auto_meld entry fails to provision: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // (c) the failed entry is named in the output.
    assert!(
        r.stderr.contains(bad_repo) || r.stdout.contains(bad_repo),
        "the failed entry must be named in the output: stderr={} stdout={}",
        r.stderr,
        r.stdout
    );

    // (a) the already-melded good source is still in the registry: the sync
    // loop ran past the provisioning failure rather than aborting.
    assert_eq!(
        source_count(&sb),
        1,
        "the already-melded source must still be registered after a provisioning failure"
    );
}

#[test]
fn policy_min_mind_version_gate_surfaces_clean_error_end_to_end() {
    // spec: POL-61
    // spec: POL-62
    // The whole point of the version gate: a real command that loads the policy
    // must stop with a clear "requires mind >=" message instead of failing
    // opaquely. The policy also carries an UNKNOWN top-level table ([future]),
    // which the strict deny_unknown_fields parse would otherwise reject first --
    // asserting the version error wins proves phase 1 runs before phase 2 all the
    // way through the binary, not just in a unit test.
    let running = env!("CARGO_PKG_VERSION");
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "min-mind-version = \"999.0.0\"\n[future]\nunknown-key = 1\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "a too-new policy must fail closed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("requires mind >=") && r.stderr.contains("999.0.0"),
        "must surface the version gate error naming the required version: {}",
        r.stderr
    );
    // spec: H2 -- the remedy points at the actual self-update verb (`mind
    // evolve`); `mind upgrade` upgrades installed items, not the binary.
    assert!(
        r.stderr.contains("mind evolve"),
        "must tell the user to run `mind evolve`: {}",
        r.stderr
    );
    // Ordering proof: the version error wins over the unknown-field error even
    // though [future] is an unknown table the strict parse would also reject.
    assert!(
        !r.stderr.contains("unknown field") && !r.stderr.contains("unknown-key"),
        "version gate must fire BEFORE the strict unknown-field parse: {}",
        r.stderr
    );
    // The gate fired before any provisioning: nothing registered on disk.
    assert_eq!(
        source_count(&sb),
        0,
        "no source registered when the gate fires"
    );
    // Sanity: the running binary version really is below the gate.
    assert_ne!(running, "999.0.0");
}

// ---------------------------------------------------------------------------
// auto_meld install = true (POL-58/59/60)
// ---------------------------------------------------------------------------

#[test]
fn auto_meld_install_true_installs_items_after_provisioning() {
    // spec: POL-58
    // When an auto_meld entry carries `install = true`, `sync` provisions the
    // source AND installs all its items headlessly. `recall --json` shows every
    // item with "installed": true.
    let sb = Sandbox::named("am-install-src");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with install=true should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // All three items must be installed.
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall.success,
        "recall after sync+install should succeed: {}",
        recall.stderr
    );
    assert!(
        recall.stdout.contains("\"installed\": true"),
        "items must be installed after auto_meld with install=true: {}",
        recall.stdout
    );
    // None should be uninstalled.
    assert!(
        !recall.stdout.contains("\"installed\": false"),
        "no items should be uninstalled: {}",
        recall.stdout
    );
}

#[test]
fn auto_meld_install_absent_registers_only() {
    // spec: POL-58
    // When `install` is absent (default false), `sync` registers the source but
    // installs nothing. `recall --json` shows all items with "installed": false.
    let sb = Sandbox::named("am-register-only");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // No `install` field: default is false, same as `install = false`.
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with default install should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Source must be registered.
    assert_eq!(source_count(&sb), 1, "source must be provisioned");

    // No items should be installed (recall shows them as available but not installed).
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.stdout.contains("\"installed\": true"),
        "no items must be installed when install is absent: {}",
        recall.stdout
    );
}

#[test]
fn auto_meld_install_true_build_hook_skipped_by_default() {
    // spec: POL-59
    // With `install = true` and `run-build-hooks` absent (default false), a
    // tool's build hook is skipped. The item installs but the hook does not run
    // (sentinel file absent).
    let sb = Sandbox::bare("am-bld-skip");
    let sentinel_name = "built.txt";
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"mytool\"\n",
            "path = \"tools/mytool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    write(
        &sb.source.join("tools/mytool/TOOL.md"),
        "---\ndescription: tool with build hook\n---\n# mytool\n",
    );
    write(&sb.source.join("mind.toml"), &toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add tool"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true, run-build-hooks absent (default false).
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync should succeed even with a build hook present: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The tool must be installed (symlink or store copy exists).
    let store_path = sb.mind_home.join("store/tool/mytool");
    assert!(
        store_path.exists(),
        "tool must be installed in the store: {store_path:?}"
    );

    // The sentinel file must NOT exist: build hook was skipped (HOOK-72 / POL-59).
    let sentinel = store_path.join(sentinel_name);
    assert!(
        !sentinel.exists(),
        "build hook must be skipped without run-build-hooks=true (POL-59): {sentinel:?}"
    );
}

#[test]
fn auto_meld_install_true_build_hook_runs_when_enabled() {
    // spec: POL-59
    // With `install = true` and `run-build-hooks = true`, the tool's build hook
    // executes during the headless install pass. The sentinel file appears in the
    // store proving the hook ran.
    let sb = Sandbox::bare("am-bld-run");
    let sentinel_name = "built.txt";
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"tool\"\n",
            "name = \"mytool\"\n",
            "path = \"tools/mytool\"\n",
            "build = \"touch {sentinel}\"\n",
        ),
        sentinel = sentinel_name
    );
    write(
        &sb.source.join("tools/mytool/TOOL.md"),
        "---\ndescription: tool with build hook\n---\n# mytool\n",
    );
    write(&sb.source.join("mind.toml"), &toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add tool"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true AND run-build-hooks = true.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\nrun-build-hooks = true\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with run-build-hooks=true should succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The sentinel file must exist: build hook ran (POL-59).
    let sentinel = sb.mind_home.join("store/tool/mytool").join(sentinel_name);
    assert!(
        sentinel.exists(),
        "build hook must run when run-build-hooks=true (POL-59): {sentinel:?}"
    );
}

#[test]
fn auto_meld_install_item_failure_soft_fails_and_continues() {
    // spec: POL-60
    // When one item's install fails during the auto_meld install pass, the
    // failure is soft: `sync` warns and continues installing the remaining items.
    // The good items land installed; the overall exit code is non-zero.
    //
    // A build hook that exits non-zero (with run-build-hooks=true) causes its
    // tool's install to fail (HOOK-71 rolls it back). The other items (a skill
    // and an agent) carry no build hook and succeed.
    let sb = Sandbox::bare("am-soft-fail");
    // Write a failing tool.
    let fail_toml = concat!(
        "[[items]]\n",
        "kind = \"tool\"\n",
        "name = \"badtool\"\n",
        "path = \"tools/badtool\"\n",
        "build = \"exit 9\"\n",
        "\n",
        "[[items]]\n",
        "kind = \"skill\"\n",
        "name = \"review\"\n",
        "path = \"skills/review\"\n",
    );
    write(
        &sb.source.join("tools/badtool/TOOL.md"),
        "---\ndescription: tool that fails to build\n---\n# badtool\n",
    );
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review skill\n---\n# review\n",
    );
    write(&sb.source.join("mind.toml"), fail_toml);
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "add items"]);

    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // install = true + run-build-hooks = true so the failing build hook fires.
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\nrun-build-hooks = true\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);

    // Non-zero exit because badtool's install failed (POL-60 / POL-34).
    assert!(
        !r.success,
        "sync must exit non-zero when an item install fails: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The good skill must still be installed (soft-fail: other items proceed).
    let skill_store = sb.mind_home.join("store/skill/review");
    assert!(
        skill_store.exists(),
        "the good skill must be installed despite the tool failure (POL-60): {skill_store:?}"
    );

    // The bad tool's store copy must be absent (hook failure rolled it back).
    let tool_store = sb.mind_home.join("store/tool/badtool");
    assert!(
        !tool_store.exists(),
        "the bad tool must not be in the store after a failed build hook: {tool_store:?}"
    );

    // The failure is named in stderr (POL-60 reporting).
    assert!(
        r.stderr.contains("badtool") || r.stderr.contains("failed"),
        "failed item must be named in stderr: {}",
        r.stderr
    );
}

#[test]
fn auto_meld_install_true_second_sync_is_idempotent() {
    // spec: POL-58
    // A second `sync` under the same install = true policy must be idempotent: the
    // source is already registered at its declared pin (the POL-32 skip), the
    // items stay installed, no reinstall is attempted, no item-failure is warned,
    // and the command exits zero (nothing else failed). Guards against a
    // regression where a repeat sync re-runs install and errors or double-registers.
    let sb = Sandbox::named("am-install-idem");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    // First sync: provisions the source AND installs its items.
    let first = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        first.success,
        "first sync must succeed: stdout={} stderr={}",
        first.stdout, first.stderr
    );
    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall1.stdout.contains("\"installed\": true"),
        "items must be installed after the first sync: {}",
        recall1.stdout
    );

    // Second sync: idempotent no-op for this source. Exit zero, no item-failure
    // warning, no duplicate registration.
    let second = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        second.success,
        "second sync must exit zero (idempotent): stdout={} stderr={}",
        second.stdout, second.stderr
    );
    assert!(
        !second.stderr.contains("item install failed"),
        "second sync must not warn about any item install failure: {}",
        second.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the source must remain singly registered after a second sync"
    );

    // The items remain installed and none flipped to uninstalled.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must remain installed after the second sync: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may become uninstalled after the second sync: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_register_only_then_install_true_converges() {
    // spec: POL-58
    // Regression for the defect where the install pass was only reached on a fresh
    // meld.  A source provisioned register-only (install absent), then given
    // `install = true` in policy at the SAME pin, must install its items on the
    // next sync (POL-32 same-pin path now falls through to the install pass).
    let sb = Sandbox::named("am-converge");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");

    // First sync: register only (install absent -> default false).
    let body_reg = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\n");
    let policy_reg = write_policy(&sb, &body_reg);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_reg.as_str())]);
    assert!(
        r1.success,
        "register-only sync must succeed: stdout={} stderr={}",
        r1.stdout, r1.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must be registered after first sync"
    );

    // Confirm nothing is installed yet.
    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_reg.as_str())],
    );
    assert!(
        !recall1.stdout.contains("\"installed\": true"),
        "items must NOT be installed after register-only sync: {}",
        recall1.stdout
    );

    // Second sync: flip to install = true at the SAME pin (POL-32 same-pin path).
    let body_ins = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy_ins = write_policy(&sb, &body_ins);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_ins.as_str())]);
    assert!(
        r2.success,
        "install=true sync at same pin must succeed: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );

    // Items must now be installed (convergence).
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_ins.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after flipping to install=true at same pin: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may remain uninstalled after convergence sync: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_repin_with_install_true_installs_at_new_ref() {
    // spec: POL-58
    // spec: POL-55
    // When a source is registered at pin A with install=false, and the policy
    // bumps to pin B with install=true, the sync must both re-pin the source AND
    // install its items (the POL-55 re-pin path now falls through to the install
    // pass).
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("am-repin-install");
    let spec = sb.source_spec();

    // Tag v2.0 at the current tip.
    git(&sb.source, &["tag", "v2.0"]);

    // First: register at v1.0 with no install.
    let escaped = spec.replace('\\', "\\\\");
    let body_v1 = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy_v1 = write_policy(&sb, &body_v1);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v1.as_str())]);
    assert!(
        r1.success,
        "first sync (register at v1.0) must succeed: {} {}",
        r1.stdout, r1.stderr
    );
    assert_eq!(source_count(&sb), 1, "one source registered");

    let recall1 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v1.as_str())],
    );
    assert!(
        !recall1.stdout.contains("\"installed\": true"),
        "nothing installed after register-only at v1.0: {}",
        recall1.stdout
    );

    // Second sync: bump to v2.0 AND enable install=true.
    let body_v2 =
        format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\ninstall = true\n");
    let policy_v2 = write_policy(&sb, &body_v2);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r2.success,
        "re-pin+install sync must succeed: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );

    // Re-pin must be reported.
    assert!(
        r2.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r2.stdout
    );

    // Source stays registered exactly once.
    assert_eq!(
        source_count(&sb),
        1,
        "source count must remain 1 after re-pin+install"
    );

    // Items must be installed.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after re-pin+install sync: {}",
        recall2.stdout
    );
    assert!(
        !recall2.stdout.contains("\"installed\": false"),
        "no item may remain uninstalled after re-pin+install: {}",
        recall2.stdout
    );
}

#[test]
fn auto_meld_repin_install_persists_new_pin_and_lands_ref_durably() {
    // spec: POL-55
    // spec: POL-58
    // spec: POL-32
    // Registry-save / `provisioned`-counter interaction on the re-pin + install
    // path.  The restructured loop increments `provisioned` on a POL-55 re-pin,
    // the install pass then saves the registry and resets `provisioned` to 0 (so
    // the outer `if provisioned > 0` does NOT redundantly save).  This test
    // certifies the OBSERVABLE contract that a missed save would break: after a
    // re-pin+install sync the NEW pin must be durably on disk (sources.json, not
    // just in memory) and the new ref's commit must have landed -- neither is
    // asserted by auto_meld_repin_with_install_true_installs_at_new_ref, which
    // only checks in-process recall output.  It then flips to the POL-32 same-pin
    // path (where `provisioned` is never incremented, so the install pass does NOT
    // pre-save) and confirms nothing on disk is lost by the counter being 0.
    let (sb, _sha_v1, sha_v2) = make_pinnable_repo("am-repin-install-durable");
    let spec = sb.source_spec();
    // Tag v2.0 at the current main tip (sha_v2); make_pinnable_repo tags v1.0 at sha_v1.
    git(&sb.source, &["tag", "v2.0"]);
    let escaped = spec.replace('\\', "\\\\");

    // First sync: register at v1.0, install = false.
    let body_v1 = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v1.0\"\n");
    let policy_v1 = write_policy(&sb, &body_v1);
    let r1 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v1.as_str())]);
    assert!(
        r1.success,
        "first sync (register at v1.0) must succeed: {} {}",
        r1.stdout, r1.stderr
    );
    let pin_v1 = read_source_pin_json(&sb);
    assert!(
        pin_v1.contains("\"tag\"") && pin_v1.contains("v1.0"),
        "initial recorded pin must be tag v1.0: {pin_v1}"
    );

    // Second sync: re-pin to v2.0 AND install = true (POL-55 + POL-58).
    let body_v2 =
        format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ntag = \"v2.0\"\ninstall = true\n");
    let policy_v2 = write_policy(&sb, &body_v2);
    let r2 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r2.success,
        "re-pin+install sync must succeed: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("re-pinned"),
        "sync must report the re-pin: {}",
        r2.stdout
    );

    // DURABILITY: reload sources.json from disk. The new pin must be persisted and
    // the old one gone -- the install-pass save must not have dropped the re-pin,
    // and the outer save must not have clobbered it back.
    let pin_after = read_source_pin_json(&sb);
    assert!(
        pin_after.contains("\"tag\"") && pin_after.contains("v2.0"),
        "re-pinned tag v2.0 must be durably persisted to sources.json: {pin_after}"
    );
    assert!(
        !pin_after.contains("v1.0"),
        "old pin v1.0 must not remain on disk after re-pin+install: {pin_after}"
    );
    // The new ref actually landed: the per-source fetch after the re-pin advanced
    // the recorded commit to sha_v2 and the final save persisted it.
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the re-pinned v2.0 commit (sha_v2) must be landed and persisted on disk"
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must stay registered exactly once after re-pin+install"
    );
    // Items installed on the re-pin path.
    let recall2 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall2.stdout.contains("\"installed\": true"),
        "items must be installed after re-pin+install sync: {}",
        recall2.stdout
    );

    // Third sync: pins now match (POL-32 same-pin path, `provisioned` never
    // incremented, so the install pass does NOT pre-save the registry). Nothing on
    // disk may be lost by the counter being 0: no re-pin, pin/commit hold, items
    // stay installed, exit zero.
    let r3 = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy_v2.as_str())]);
    assert!(
        r3.success,
        "third sync (same-pin + install) must succeed: {} {}",
        r3.stdout, r3.stderr
    );
    assert!(
        !r3.stdout.contains("re-pinned"),
        "a matching pin must not re-pin again on the same-pin path: {}",
        r3.stdout
    );
    let pin_final = read_source_pin_json(&sb);
    assert!(
        pin_final.contains("v2.0") && !pin_final.contains("v1.0"),
        "the same-pin install pass must leave the persisted pin at v2.0: {pin_final}"
    );
    assert_eq!(
        read_source_commit(&sb),
        sha_v2,
        "the landed commit must remain at sha_v2 after the same-pin+install sync"
    );
    let recall3 = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy_v2.as_str())],
    );
    assert!(
        recall3.stdout.contains("\"installed\": true"),
        "items must remain installed after the same-pin+install sync: {}",
        recall3.stdout
    );
    assert!(
        !recall3.stdout.contains("\"installed\": false"),
        "no item may flip to uninstalled on the same-pin+install path: {}",
        recall3.stdout
    );
    assert_eq!(
        source_count(&sb),
        1,
        "source must stay registered exactly once after the same-pin+install sync"
    );
}

#[test]
fn auto_meld_install_true_with_zero_items_succeeds() {
    // spec: POL-58
    // install = true on a source that offers NO items (a pure registry) must
    // provision successfully with an empty install pass: the source is registered,
    // no item failure is reported, and sync exits zero. Guards against the empty
    // catalog being treated as an error.
    let sb = Sandbox::bare("am-install-empty");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\ninstall = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "sync with install=true on an item-less source must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stderr.contains("item install failed"),
        "an item-less source must not report any item failure: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the item-less source must still be registered"
    );
}

#[test]
fn auto_meld_mixed_install_flags_install_only_the_opted_in_source() {
    // spec: POL-58
    // Two auto_meld entries: one carries install = true, the other does not. Only
    // the opted-in source's items are installed; the other registers only. Each
    // source ships a uniquely named skill so the two are unambiguous even though
    // they are provisioned in the same run.
    let sb = Sandbox::bare("am-mixed");

    // Source A: opted in (install = true), ships skill:alpha.
    let src_a = sb.base.join("am-mixed-a");
    write(
        &src_a.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: alpha skill\n---\n# alpha\n",
    );
    git(&src_a, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_a, &["config", "user.email", "t@t"]);
    git(&src_a, &["config", "user.name", "t"]);
    git(&src_a, &["add", "-A"]);
    git(&src_a, &["commit", "-qm", "initial"]);
    let spec_a = src_a.to_string_lossy().into_owned();

    // Source B: register-only (no install), ships skill:beta.
    let src_b = sb.base.join("am-mixed-b");
    write(
        &src_b.join("skills/beta/SKILL.md"),
        "---\nname: beta\ndescription: beta skill\n---\n# beta\n",
    );
    git(&src_b, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&src_b, &["config", "user.email", "t@t"]);
    git(&src_b, &["config", "user.name", "t"]);
    git(&src_b, &["add", "-A"]);
    git(&src_b, &["commit", "-qm", "initial"]);
    let spec_b = src_b.to_string_lossy().into_owned();

    let esc_a = spec_a.replace('\\', "\\\\");
    let esc_b = spec_b.replace('\\', "\\\\");
    let body = format!(
        "[[sources.auto_meld]]\nrepo = \"{esc_a}\"\ninstall = true\n\n[[sources.auto_meld]]\nrepo = \"{esc_b}\"\n"
    );
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "mixed-policy sync must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Both sources registered.
    assert_eq!(
        source_count(&sb),
        2,
        "both auto_meld sources must be registered"
    );
    // A's item is installed; B's item is not (register-only).
    assert!(
        sb.claude_home.join("skills/alpha").exists(),
        "alpha from the install=true source must be installed: {:?}",
        sb.claude_home
    );
    assert!(
        !sb.claude_home.join("skills/beta").exists(),
        "beta from the register-only source must NOT be installed"
    );
}

#[test]
fn auto_meld_run_build_hooks_without_install_is_inert() {
    // spec: POL-58
    // spec: POL-59
    // run-build-hooks = true WITHOUT install = true is a harmless no-op: run-build-hooks
    // only gates build hooks during the install pass, and there is no install pass
    // when install is absent. The policy parses, the source registers only, nothing
    // is installed, and sync exits zero.
    let sb = Sandbox::named("am-rbh-noinstall");
    let spec = sb.source_spec();
    let escaped = spec.replace('\\', "\\\\");
    // run-build-hooks set, install absent (default false).
    let body = format!("[[sources.auto_meld]]\nrepo = \"{escaped}\"\nrun-build-hooks = true\n");
    let policy = write_policy(&sb, &body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "run-build-hooks without install must be a harmless no-op: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // Source registered, nothing installed.
    assert_eq!(
        source_count(&sb),
        1,
        "the source must be registered when only run-build-hooks is set"
    );
    let recall = sb.mind_env(
        &["recall", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !recall.stdout.contains("\"installed\": true"),
        "no items may be installed when install is absent (run-build-hooks alone is inert): {}",
        recall.stdout
    );
}

#[test]
fn meld_pinned_policy_accepts_source_directive_tag() {
    // spec: POL-20
    // The pin may come from the source's own mind.toml `[source]` directive
    // (DSC-41), not just the --pin-tag flag. A directive that resolves to a tag
    // satisfies pinned = true and the meld is accepted.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pindir-tag");
    sb.write_and_commit("mind.toml", "[source]\npin-tag = \"v1.0\"\n");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pindir-tag\"]\n",
    );

    // No consumer pin flag: the [source] directive supplies the tag pin.
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "a [source] tag directive must satisfy a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the directive-pinned source should register"
    );
    // The landed pin is the directive's tag (sha_v1), not the floating main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "the directive tag pin should land on the tagged commit"
    );
}

#[test]
fn meld_pinned_policy_refuses_source_directive_floating_branch() {
    // spec: POL-20
    // The negative of the directive case: a `[source]` directive that resolves to
    // a floating branch (follow-branch) does NOT satisfy pinned = true and is
    // refused, leaving nothing registered.
    let (sb, _v1, _v2) = make_pinnable_repo("pindir-branch");
    sb.write_and_commit("mind.toml", "[source]\nfollow-branch = \"stable\"\n");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\npinned = true\nlock = true\nallow = [\"local/*/pindir-branch\"]\n",
    );

    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "a [source] follow-branch directive must be refused under a pinned policy: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("must be pinned"),
        "refusal should mention pinning: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        0,
        "nothing registered on a floating refusal"
    );
}

#[test]
fn config_lobes_add_allowed_when_lobes_not_locked() {
    // spec: POL-40
    // The refusal is specific to the lobe lock: with [lobes].lock = false (and a
    // policy otherwise present), `config lobes add` still works. The lock is the
    // only thing that pins the agent homes.
    let sb = Sandbox::named("agents");
    let policy = write_policy(&sb, "[lobes]\nlock = false\ntargets = [\"~/.claude\"]\n");
    let lobe = sb.base.join("extra-home");
    let lobe_str = lobe.to_string_lossy().into_owned();

    let r = sb.mind_env(
        &["config", "lobes", "add", &lobe_str],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "an unlocked lobes add must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("added lobe"),
        "the add should be reported: {}",
        r.stdout
    );

    // The lobe is now listed, confirming the write took effect.
    let after = sb.mind_env(
        &["config", "lobes", "list"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        after.stdout.contains(&lobe_str),
        "the added lobe must appear in the list: {}",
        after.stdout
    );
}

#[test]
fn config_lobes_add_allowed_with_no_lobes_section() {
    // spec: POL-40
    // A policy that controls only sources (no [lobes] section at all) leaves the
    // lobe lock unset, so `config lobes add` is unaffected.
    let sb = Sandbox::named("agents");
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let lobe = sb.base.join("home-no-lobes-section");
    let lobe_str = lobe.to_string_lossy().into_owned();

    let r = sb.mind_env(
        &["config", "lobes", "add", &lobe_str],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "lobes add must work when the policy has no [lobes] lock: {} {}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("added lobe"), "{}", r.stdout);
}

#[test]
fn meld_refused_when_not_allowed_leaves_no_clone_and_no_registry() {
    // spec: POL-11
    // Reinforce the "nothing cloned or registered" half: after a refused meld the
    // clone dir is absent AND sources.json records nothing (no partial registry),
    // and no link leaked into the hermetic claude_home.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(!r.success, "refused meld must fail: {}", r.stdout);

    assert_eq!(source_count(&sb), 0, "registry must record nothing");

    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should survive a refusal at {}",
        clone_dir.display()
    );
    let leaked = sb.claude_home.join("agents/dev.md");
    assert!(
        std::fs::symlink_metadata(&leaked).is_err(),
        "no item should be installed on a refused meld"
    );
}

#[test]
fn meld_unlocked_advisory_warning_text() {
    // spec: POL-13
    // The advisory warning under lock=false names the allowlist and explains it is
    // not enforced because the lock is off.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = false\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "advisory meld must succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("allowlist") && r.stderr.contains("advisory"),
        "warning should name the allowlist and mark it advisory: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("lock is false"),
        "warning should explain the lock is off: {}",
        r.stderr
    );
    assert_eq!(
        source_count(&sb),
        1,
        "the advisory source is still registered"
    );
}

#[test]
fn upgrade_skips_disallowed_source_hook_when_locked() {
    // spec: POL-12
    // Install hooks are arbitrary code; running a disallowed source's hook would
    // violate POL-12. Meld + record a hooked source while it is allowed, then
    // advance it and run upgrade under a locked policy whose `allow` excludes the
    // source: the hook must NOT re-run (marker not re-created) and the skip is
    // reported.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"])
            .success,
        "initial meld should run the hook and record its commit"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");
    std::fs::remove_file(&marker).unwrap();

    // Advance the source so an UNSCOPED upgrade would otherwise re-run the hook.
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // A locked policy whose allowlist excludes this source.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/never-match\"]\n",
    );
    let r = sb.mind_env(
        &["upgrade", "-y", "--accept-hooks=install,update,uninstall"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "upgrade must not error when skipping a disallowed source's hook: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !marker.exists(),
        "a policy-disallowed source's hook must not re-run: {} exists",
        marker.display()
    );
    assert!(
        r.stdout.contains("skipping install hook for")
            && r.stdout
                .contains("not permitted by the managed policy's allowlist"),
        "the skipped hook must be reported: {}",
        r.stdout
    );
}

#[test]
fn evolve_disabled_by_policy_rejects_both_check_and_run() {
    // spec: POL-52 -- self-update = false must gate both evolve and evolve --check
    // before any network call.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = false\n");

    // --check mode: must fail even though it normally makes no network call.
    let r = sb.mind_env(
        &["evolve", "--check", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "evolve --check must be refused when self-update = false: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("disabled by the managed policy"),
        "error must name the policy: {}",
        r.stderr
    );
    // spec: POL-66 -- self-update = false errors BEFORE the PinnedBelowCurrent
    // decision, so the skew warning path must never be reached: no warning text on
    // stdout even though a --version below current is supplied.
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "disabled policy must not reach/print the skew warning: {}",
        r.stdout
    );

    // run mode (--yes skips the interactive prompt): must also fail, no download.
    let r = sb.mind_env(
        &["evolve", "--yes", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "evolve --yes must be refused when self-update = false: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("disabled by the managed policy"),
        "run error must also name the policy: {}",
        r.stderr
    );
    // spec: POL-66 -- likewise the run path must not reach the skew warning.
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "disabled policy (run mode) must not reach/print the skew warning: {}",
        r.stdout
    );
}

#[test]
fn evolve_pinned_by_policy_check_is_offline_and_names_pin() {
    // spec: POL-53 -- self-update = "<version>" forces that version as the target
    // without any API call. evolve --check with the pin set must succeed offline
    // and mention the pinned version in its report.
    let sb = Sandbox::new();
    // Pin to "0.1.0", which is below any real build, so the decision is
    // PinnedBelowCurrent and the output says "not downgrading". No network needed.
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with a pin below current must succeed (not-downgrading path): {} {}",
        r.stdout, r.stderr
    );
    // The output must reference the pinned version.
    assert!(
        r.stdout.contains("0.1.0"),
        "check output must name the pinned version: {}",
        r.stdout
    );
}

#[test]
fn evolve_pinned_mismatched_version_arg_fails() {
    // spec: POL-53 -- when policy pins to X and --version Y (Y != X) is given,
    // the command must fail naming the conflict.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.14.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "0.15.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        !r.success,
        "mismatched --version must be refused by a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("0.14.0"),
        "error must name the policy pin: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("conflicts") || r.stderr.contains("conflict"),
        "error must say 'conflicts': {}",
        r.stderr
    );
}

#[test]
fn evolve_pinned_matching_version_arg_succeeds() {
    // spec: POL-53 -- when --version matches the policy pin exactly, the command
    // proceeds normally. Pin = 0.1.0 (below current) with --version 0.1.0 ->
    // PinnedBelowCurrent report, exit 0.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "0.1.0"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "matching --version must be accepted by a pinned policy: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("0.1.0"),
        "output must name the pinned version: {}",
        r.stdout
    );
}

#[test]
fn evolve_self_update_true_allows_evolve() {
    // spec: POL-54 -- self-update = true is identical to the absent key.
    // evolve --check --version 9.9.9 (offline explicit check) must succeed.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = true\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--version", "9.9.9"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "self-update = true must allow evolve: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("9.9.9"),
        "output must name the target version: {}",
        r.stdout
    );
}

// ---- POL-66: policy pin skew warning -----------------------------------------

#[test]
fn evolve_policy_pin_below_running_emits_skew_warning_human_mode() {
    // spec: POL-66 -- when the running binary is above the policy pin
    // (PinnedBelowCurrent), evolve and evolve --check must print a human-readable
    // warning that the running version differs from the policy pin and that the
    // pin is an upper bound. Pin "0.1.0" is always below any real build version.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    // Test the --check path.
    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("warning:"),
        "check mode must print a warning when running > policy pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("0.1.0"),
        "warning must name the policy pin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("upper bound") || r.stdout.contains("does not downgrade"),
        "warning must say the pin is an upper bound or does not downgrade: {}",
        r.stdout
    );

    // Test the non-check (evolve --yes --version 0.1.0) path. We cannot do a real
    // download, but we can test the PinnedBelowCurrent branch by letting the policy
    // pin steer the version. Without --yes the binary would prompt; with a policy
    // pin below current the PinnedBelowCurrent branch returns before the prompt.
    let r2 = sb.mind_env(&["evolve"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r2.success,
        "evolve (no --check) with a pin below current must exit 0: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("warning:"),
        "non-check evolve must also print the skew warning: {}",
        r2.stdout
    );
}

#[test]
fn evolve_policy_pin_skew_json_no_warning_on_stdout_outcome_is_not_downgrading() {
    // spec: POL-66 -- --json mode must NOT print the human warning text on stdout;
    // the machine-readable hook is the structured `outcome` field. Exit code 0.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--check", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check --json with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    // The whole stdout must parse as ONE well-formed JSON object: a stray warning
    // line prepended/appended (or unescaped text) would corrupt it. Substring
    // matching alone would not catch such corruption, so parse it.
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "evolve --json stdout must be valid JSON ({e}): {}",
            r.stdout
        )
    });
    assert_eq!(
        v.get("action").and_then(|a| a.as_str()),
        Some("evolve"),
        "structured action must be 'evolve': {}",
        r.stdout
    );
    // The structured outcome must be exactly "not-downgrading".
    assert_eq!(
        v.get("outcome").and_then(|o| o.as_str()),
        Some("not-downgrading"),
        "JSON outcome must be 'not-downgrading': {}",
        r.stdout
    );
    // The human warning text must not appear anywhere on stdout.
    assert!(
        !r.stdout.contains("warning:"),
        "JSON mode must not print the human warning on stdout: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("upper bound"),
        "JSON mode must not leak the skew-warning wording onto stdout: {}",
        r.stdout
    );
}

#[test]
fn evolve_policy_pin_skew_non_check_json_no_warning_valid_json() {
    // spec: POL-66 -- the NON-check --json path (the second, distinct warning site
    // in selfupdate::run, guarded by its own `if out.json { return }`) must also
    // emit only the structured outcome with no warning text and produce valid JSON.
    // The --check --json test above exercises the first site; this pins the second.
    let sb = Sandbox::new();
    let policy = write_policy(&sb, "[binary]\nself-update = \"0.1.0\"\n");

    let r = sb.mind_env(
        &["evolve", "--json"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --json (non-check) with a pin below current must exit 0: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "non-check evolve --json stdout must be valid JSON ({e}): {}",
            r.stdout
        )
    });
    assert_eq!(
        v.get("outcome").and_then(|o| o.as_str()),
        Some("not-downgrading"),
        "non-check JSON outcome must be 'not-downgrading': {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("warning:") && !r.stdout.contains("upper bound"),
        "non-check JSON mode must not print the skew warning on stdout: {}",
        r.stdout
    );
}

#[test]
fn evolve_policy_pin_equal_to_running_no_skew_warning() {
    // spec: POL-66 -- the skew warning fires only when the running version is
    // strictly ABOVE the pin (PinnedBelowCurrent). A pin at the running
    // version's numeric base is never above it, so no warning either way.
    let sb = Sandbox::new();
    // A policy pin must be dotted numeric, so pin the numeric base of the
    // running version. On a release build that pin equals the running version
    // (UpToDate); on a `-dev` build the prerelease compares strictly below its
    // own base (CLI-140: a dev build is offered its base release), so the pin
    // sits above and the base release is reported available. Neither case is
    // PinnedBelowCurrent.
    let running = env!("CARGO_PKG_VERSION");
    let base = running.split('-').next().unwrap();
    let policy = write_policy(&sb, &format!("[binary]\nself-update = \"{base}\"\n"));

    let r = sb.mind_env(
        &["evolve", "--check"],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "evolve --check with pin at the running base must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("warning:"),
        "no skew warning when the pin is not below the running version: {}",
        r.stdout
    );
    if running.contains('-') {
        // Dev build: strictly below its own base release (CLI-140).
        assert!(
            r.stdout.contains("available"),
            "a dev build must be offered its base release: {}",
            r.stdout
        );
    } else {
        assert!(
            r.stdout.contains("up to date"),
            "output must say 'up to date' when pin == running: {}",
            r.stdout
        );
    }
}

#[test]
fn evolve_explicit_version_below_running_no_policy_no_skew_warning() {
    // spec: POL-66 -- the skew warning is specific to the policy-pin case.
    // When the user passes --version X (below current) with no policy in effect,
    // the existing "not downgrading" message is sufficient; no skew warning.
    let sb = Sandbox::new();
    let r = sb.mind(&["evolve", "--check", "--version", "0.1.0"]);
    assert!(
        r.success,
        "evolve --check --version 0.1.0 (below current, no policy) must exit 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("warning:"),
        "no skew warning when pin is from --version with no policy: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("not downgrading"),
        "output must say 'not downgrading': {}",
        r.stdout
    );
}

// ---- lifecycle-hook system tests (HOOK-50..58) --------------------------------
//
// These tests cover the extended hook system: multiple named [[hooks]] entries,
// optional hooks, uninstall hooks at `unmeld`, and the `init-source` scaffold.
// All tests run non-TTY (stdin piped), so interactive prompts never fire.

#[test]
fn meld_policy_refused_before_git_clone_no_clone_dir() {
    // spec: POL-36 -- the allow/lock refusal fires before any git network call
    // or directory creation; no clone dir should exist after a refused meld.
    let sb = Sandbox::bare("pol36-no-clone-test");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    // The lock excludes every external repo.
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let r = sb.mind_env(
        &["meld", "https://example.com/owner/refused-repo"],
        &[("PATH", &new_path), ("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "locked meld must be refused: {}", r.stdout);
    // The error must name the policy refusal, not a git error.
    assert!(
        r.stderr.contains("not permitted"),
        "stderr must report SourceNotAllowed: {}",
        r.stderr
    );
    // The fake git emits "Authentication failed" on clone; if that appears, the
    // clone ran before the policy check (wrong order).
    assert!(
        !r.stderr.contains("Authentication failed"),
        "git clone must not have run: policy check must fire first: {}",
        r.stderr
    );
    // sources tree for the refused URL must not exist.
    let sources_root = sb.mind_home.join("sources");
    let clone_exists = sources_root.exists()
        && std::fs::read_dir(&sources_root)
            .map(|d| d.count() > 0)
            .unwrap_or(false);
    assert!(
        !clone_exists,
        "no clone directory should exist after a refused meld"
    );
}

#[test]
fn meld_policy_refused_prints_policy_file_hint() {
    // spec: POL-37 -- when SourceNotAllowed is returned, the effective policy
    // file path is printed to stderr as a hint.
    let sb = Sandbox::bare("pol37-hint-test");
    let policy = write_policy(&sb, "[sources]\nlock = true\nallow = []\n");
    let spec = sb.source_spec();
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(!r.success, "locked meld must be refused: {}", r.stdout);
    // The hint line must name the policy file path.
    assert!(
        r.stderr.contains("policy") && r.stderr.contains("policy.toml"),
        "hint must mention the policy file path: {}",
        r.stderr
    );
}

// --- allow-local policy knob (POL-56/POL-57) ---------------------------------

#[test]
fn allow_local_false_refuses_local_path_meld_even_with_permissive_pattern() {
    // spec: POL-56 -- allow-local = false under lock refuses a local-path meld
    // regardless of allow patterns (even one that would otherwise match), leaves
    // no clone dir, and error text names the allow-local = false reason.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // The allow pattern would match the local identity, but allow-local = false
    // overrides it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = false under lock must refuse a local-path meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("allow-local"),
        "error must name the allow-local = false reason: {}",
        r.stderr
    );
    // The error must be distinct from the generic pattern-miss message.
    assert!(
        !r.stderr
            .contains("not permitted by the managed policy's allowlist"),
        "error must not be the generic SourceNotAllowed message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
    // No clone dir left on disk.
    let clone_dir = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base_name())
        .join("agents");
    assert!(
        !clone_dir.exists(),
        "no clone should be left at {}",
        clone_dir.display()
    );
}

#[test]
fn allow_local_false_under_no_lock_does_not_refuse() {
    // spec: POL-56 -- with lock absent or false, allow-local has no effect;
    // the meld proceeds regardless of the allow-local value.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = false\nallow-local = false\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "allow-local = false with lock = false must not refuse: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn allow_local_true_still_requires_allow_pattern_under_lock() {
    // spec: POL-57 -- allow-local = true (explicit or absent default) preserves
    // existing behavior: a local-path meld still requires an allow pattern match
    // when lock = true. A meld that fails the pattern is refused as SourceNotAllowed.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = true\nallow = [\"local/*/other-repo\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = true with lock must still enforce allow patterns: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not permitted"),
        "error should be the standard SourceNotAllowed message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_true_default_allows_local_meld_matching_pattern() {
    // spec: POL-57 -- absent allow-local (default true) allows a local-path meld
    // that satisfies the allow pattern under lock.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    // Pattern matches the local identity local/<base>/agents.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        r.success,
        "default allow-local (true) with matching pattern must succeed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn allow_local_false_refuses_file_url_meld() {
    // spec: POL-56 -- the `file://` spelling of a local source parses to
    // host == "local" (Source::is_local), so allow-local = false under a lock
    // must refuse it exactly as it refuses a bare absolute path. Guards against a
    // regression where the guard only catches the `/`-prefixed spelling and lets
    // the `file://` URL through.
    let sb = Sandbox::named("agents");
    // `file://` + an absolute path yields the canonical `file:///abs/path` URL.
    let spec = format!("file://{}", sb.source_spec());
    // A permissive allow pattern would match the identity, but allow-local = false
    // overrides it.
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "allow-local = false under lock must refuse a file:// meld: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("allow-local"),
        "error must name the allow-local = false reason: {}",
        r.stderr
    );
    // Must be LocalMeldForbidden, not the generic pattern-miss error (proves the
    // guard fired on is_local, not that the pattern happened to miss).
    assert!(
        !r.stderr
            .contains("not permitted by the managed policy's allowlist"),
        "error must be LocalMeldForbidden, not the generic SourceNotAllowed: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_false_meld_json_error_envelope_has_kind_slug() {
    // spec: POL-56 -- under --json a refused local meld emits the structured
    // error envelope {"schema":1,"error":{"kind":"local-meld-forbidden",...}}
    // with the stable kind slug and the allow-local message, rather than a human
    // string. Mirrors the Phase-1 --json error-envelope tests.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(
        &sb,
        "[sources]\nlock = true\nallow-local = false\nallow = [\"local/*/*\"]\n",
    );
    let r = sb.mind_env(
        &["--json", "meld", &spec],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(!r.success, "meld must fail: {}", r.stdout);
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    assert_eq!(
        v["error"]["kind"].as_str(),
        Some("local-meld-forbidden"),
        "kind slug must be local-meld-forbidden: {}",
        r.stdout
    );
    let msg = v["error"]["message"]
        .as_str()
        .expect("error.message must be a string");
    assert!(
        msg.contains("allow-local"),
        "message must name the allow-local reason, not a placeholder: {msg}"
    );
    assert_eq!(source_count(&sb), 0, "registry must be unchanged");
}

#[test]
fn allow_local_false_no_lock_emits_no_allow_local_warning() {
    // spec: POL-56 -- with lock = false the allow-local guard is inert: the meld
    // proceeds and NO allow-local text appears on stderr (the only warning is the
    // unrelated POL-13 advisory allowlist notice). Guards against the guard
    // dropping its `policy.lock()` precondition and warning/refusing with the lock
    // off.
    let sb = Sandbox::named("agents");
    let spec = sb.source_spec();
    let policy = write_policy(&sb, "[sources]\nlock = false\nallow-local = false\n");
    let r = sb.mind_env(&["meld", &spec], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(r.success, "lock = false must not refuse: {}", r.stderr);
    assert!(
        !r.stderr.contains("allow-local"),
        "no allow-local warning must be emitted when lock is false: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "source should be registered");
}

#[test]
fn auto_meld_partial_failure_does_not_persist_partial_entry() {
    // spec: POL-35 -- when a provisioning entry fails after meld_recursive has
    // pushed sources into the registry, the registry is rolled back so a
    // subsequent save does not persist the partial entry.
    //
    // Setup:
    //   entry-1 (first-source): succeeds, gets provisioned -> provisioned += 1.
    //   entry-2 (super-source): has a nested source but install-items names an
    //     item that does not exist -> meld_recursive returns Err(BadReference)
    //     after pushing super-source (and nested source) into registry.sources.
    //
    // Without the rollback, registry.save would persist entry-2's partial data.
    // With the rollback, only entry-1 is in the registry after the save.
    //
    // NOTE: the discover-nested field is `install-items` (kebab), not the
    // underscore form -- the underscore form is an unknown-field TOML *parse*
    // error that fails before the super-source is ever pushed, which would make
    // this test pass trivially without exercising the rollback at all.
    let sb = Sandbox::new(); // provides "first-source" with the standard fixture
    let first_spec = sb.source_spec();

    // Build the nested source (has real-skill, but NOT nonexistent-skill).
    let nested = sb.base.join("nested-src");
    write(
        &nested.join("skills/real-skill/SKILL.md"),
        "---\nname: real-skill\ndescription: a real skill\n---\n# real skill\n",
    );
    git(&nested, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&nested, &["config", "user.email", "t@t"]);
    git(&nested, &["config", "user.name", "t"]);
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-qm", "initial"]);

    // Build the super-source: discover.sources -> nested, install-items -> bad ref.
    let super_src = sb.base.join("super-src");
    let nested_str = nested.to_string_lossy();
    let super_toml = format!(
        "[[discover.sources]]\nsource = \"{nested_str}\"\ninstall-items = [\"skill:nonexistent-skill\"]\n"
    );
    write(&super_src.join("mind.toml"), &super_toml);
    git(&super_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&super_src, &["config", "user.email", "t@t"]);
    git(&super_src, &["config", "user.name", "t"]);
    git(&super_src, &["add", "-A"]);
    git(&super_src, &["commit", "-qm", "initial"]);

    let super_str = super_src.to_string_lossy();
    let first_str = first_spec.as_str();
    let policy_body = format!(
        "[[sources.auto_meld]]\nrepo = \"{first_str}\"\n\n[[sources.auto_meld]]\nrepo = \"{super_str}\"\n"
    );
    let policy = write_policy(&sb, &policy_body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    // sync exits non-zero because the second entry failed.
    assert!(
        !r.success,
        "sync must fail due to provisioning failure: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The failure must be the DSC-63 BadReference (which fires only after the
    // super-source AND its nested source are pushed into the registry), not an
    // earlier parse/clone error that would never push anything and make the
    // rollback assertion pass for the wrong reason.
    assert!(
        r.stderr.contains("nonexistent-skill") && r.stderr.contains("does not match"),
        "failure must be the DSC-63 bad-reference error (fires post-push): {}",
        r.stderr
    );
    // Only the first (successful) entry should be in the registry.
    assert_eq!(
        source_count(&sb),
        1,
        "only the first source should be persisted; the failed entry must be rolled back: stderr={}",
        r.stderr
    );
}

#[test]
fn auto_meld_rollback_on_dsc64_validate_failure() {
    // spec: POL-35 -- a second, distinct failure mode from the install_items
    // bad-ref case above. Here meld_recursive pushes the super-source into the
    // registry, then a DSC-64 validation error (install = true and install_items
    // both set on a nested entry) propagates out. Without the POL-35 rollback the
    // partially-pushed super-source would be persisted by the save; with it, only
    // the first successful entry survives.
    let sb = Sandbox::new();
    let first_spec = sb.source_spec();

    // A nested source that offers a real skill (never reached: validate errors
    // before the nested meld runs, but the entry must parse as a valid source).
    let nested = sb.base.join("nested-dsc64-src");
    write(
        &nested.join("skills/real-skill/SKILL.md"),
        "---\nname: real-skill\ndescription: a real skill\n---\n# real skill\n",
    );
    git(&nested, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&nested, &["config", "user.email", "t@t"]);
    git(&nested, &["config", "user.name", "t"]);
    git(&nested, &["add", "-A"]);
    git(&nested, &["commit", "-qm", "initial"]);

    // Super-source: install = true AND install_items set is a DSC-64 violation.
    let super_src = sb.base.join("super-dsc64-src");
    let nested_str = nested.to_string_lossy();
    let super_toml = format!(
        "[[discover.sources]]\nsource = \"{nested_str}\"\ninstall = true\ninstall-items = [\"skill:real-skill\"]\n"
    );
    write(&super_src.join("mind.toml"), &super_toml);
    git(&super_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&super_src, &["config", "user.email", "t@t"]);
    git(&super_src, &["config", "user.name", "t"]);
    git(&super_src, &["add", "-A"]);
    git(&super_src, &["commit", "-qm", "initial"]);

    let super_str = super_src.to_string_lossy();
    let first_str = first_spec.as_str();
    let policy_body = format!(
        "[[sources.auto_meld]]\nrepo = \"{first_str}\"\n\n[[sources.auto_meld]]\nrepo = \"{super_str}\"\n"
    );
    let policy = write_policy(&sb, &policy_body);

    let r = sb.mind_env(&["sync"], &[("MIND_POLICY_FILE", policy.as_str())]);
    assert!(
        !r.success,
        "sync must fail on the DSC-64 provisioning error: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The failure must be the DSC-64 mutual-exclusion error (reached only after
    // the super-source was pushed), not some earlier parse/clone error that would
    // never push the super and make this test trivially pass.
    assert!(
        r.stderr.contains("mutually exclusive"),
        "failure must be the DSC-64 validate error (fires after the super push): {}",
        r.stderr
    );
    // Only the first (successful) entry survives; the super-source pushed before
    // the validate error must be rolled back, not persisted.
    assert_eq!(
        source_count(&sb),
        1,
        "the DSC-64-failing entry must be rolled back, leaving only the first: stderr={}",
        r.stderr
    );
}
