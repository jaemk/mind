//! `sync` (spec/cli.md).

use super::*;

#[test]
fn sync_reports_up_to_date_then_updated() {
    // spec: CLI-50
    let sb = melded();
    assert!(sb.mind(&["sync"]).stdout.contains("up to date"));
    sb.edit_source();
    assert!(sb.mind(&["sync"]).stdout.contains("updated"));
}

#[test]
fn sync_with_no_sources_is_ok() {
    // spec: CLI-51
    let sb = Sandbox::new();
    let r = sb.mind(&["sync"]);
    assert!(r.success);
    assert!(r.stdout.contains("no sources melded"), "{}", r.stdout);
}

#[test]
fn sync_preserves_consumer_alias() {
    // spec: CLI-52
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec(), "--as", "jk"]).success);
    assert!(sb.mind(&["sync"]).success);

    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("namespace:jk")
    );
    // Items remain namespaced under the alias after sync.
    assert!(sb.mind(&["probe"]).stdout.contains("skill:jk:review"));
}

#[test]
fn sync_upgrade_refreshes_then_applies_upgrades() {
    // spec: CLI-53
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source(); // upstream change, not yet synced

    // One command fetches the change and (on `y`) applies the upgrade.
    let r = sb.mind_tty(&["sync", "--upgrade"], "y\n");
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("updated"), "sync ran: {}", r.stdout);
    assert!(
        r.stdout.contains("upgraded skill:review"),
        "upgrade applied: {}",
        r.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]).stdout;
    assert_ne!(before, after, "commit/hash should have advanced");
}

#[test]
fn sync_continues_past_a_failed_source() {
    // spec: CLI-54
    let a = Sandbox::new(); // healthy
    let b = Sandbox::new(); // will be broken
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // Break b's remote and advance a's, then sync both.
    std::fs::remove_dir_all(&b.source).unwrap();
    a.edit_source();
    let r = a.mind(&["sync"]);

    // The run reports the failure and exits non-zero...
    assert!(!r.success, "sync should exit non-zero when a source fails");
    assert!(
        r.stdout.contains("failed") || r.stderr.contains("failed"),
        "broken source reported: {} / {}",
        r.stdout,
        r.stderr
    );
    // ...but the healthy source was still refreshed (progress persisted).
    assert!(r.stdout.contains("updated"), "healthy source: {}", r.stdout);
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains(&format!("{}/agents", a.base_name())),
        "{sources}"
    );
    assert!(
        sources.contains(&format!("{}/agents", b.base_name())),
        "{sources}"
    );
}

/// HEAD of a sandbox's source repo.
fn source_head(sb: &Sandbox) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&sb.source)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Every `(name, commit)` recorded in `sb`'s registry, in registry order.
fn recorded_commits(sb: &Sandbox) -> Vec<(String, String)> {
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).expect("sources.json");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap().to_string(),
                s["commit"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `count` pinned (cloned, so actually fetched) sources melded into the first
/// sandbox, each advanced upstream after the meld. The first sandbox is the
/// mind home; the rest are only their source repos.
fn advanced_pinned_sources(count: usize) -> Vec<Sandbox> {
    let sbs: Vec<Sandbox> = (0..count).map(|_| Sandbox::new()).collect();
    for sb in &sbs {
        let r = sbs[0].mind(&[
            "meld",
            &sb.source_spec(),
            "--follow-branch",
            "main",
            "--register-only",
        ]);
        assert!(r.success, "meld: {}", r.stderr);
    }
    for sb in &sbs {
        sb.edit_source();
    }
    sbs
}

#[test]
fn sync_fetches_many_sources_and_reports_in_registry_order() {
    // spec: CLI-248 -- more sources than fetch workers: every one is fetched,
    // and the report and recorded commits follow registry order.
    let sbs = advanced_pinned_sources(10);
    let r = sbs[0].mind(&["sync"]);
    assert!(r.success, "sync: {} / {}", r.stdout, r.stderr);

    let names: Vec<String> = recorded_commits(&sbs[0])
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let lines: Vec<&str> = r.stdout.lines().filter(|l| l.contains("syncing")).collect();
    assert_eq!(lines.len(), sbs.len(), "{}", r.stdout);
    for (line, name) in lines.iter().zip(&names) {
        assert!(line.contains(name.as_str()), "order: {}", r.stdout);
        assert!(line.contains("updated"), "fetched: {}", r.stdout);
    }
    let commits: Vec<String> = recorded_commits(&sbs[0])
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let heads: Vec<String> = sbs.iter().map(source_head).collect();
    assert_eq!(commits, heads);
}

#[test]
fn upgrade_prepass_fetches_every_source() {
    // spec: CLI-248 CLI-169 -- the upgrade pre-pass fetches in parallel too.
    let sbs = advanced_pinned_sources(3);
    let r = sbs[0].mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {} / {}", r.stdout, r.stderr);
    let commits: Vec<String> = recorded_commits(&sbs[0])
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let heads: Vec<String> = sbs.iter().map(source_head).collect();
    assert_eq!(commits, heads);
}

/// A `PATH` with a fake `git` in front: it appends `fetch prompt=<0|on>` to
/// `<sb.base>/fetches.log` for each fetch, fails a fetch the way a suppressed
/// credential prompt does whenever prompts are off, and otherwise runs the
/// real git. Returns the `PATH` value and the log path.
#[cfg(unix)]
fn prompt_requiring_git(sb: &Sandbox) -> (String, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH");
    let bin = sb.base.join("bin");
    let log = sb.base.join("fetches.log");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("git");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = fetch ]; then\n\
             echo \"fetch prompt=${{GIT_TERMINAL_PROMPT:-on}}\" >> '{log}'\n\
             if [ \"$GIT_TERMINAL_PROMPT\" = 0 ]; then\n\
             echo \"fatal: could not read Username for 'x': terminal prompts disabled\" >&2\n\
             exit 128\n\
             fi\n\
             fi\n\
             exec '{git}' \"$@\"\n",
            log = log.display(),
            git = real_git.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    (path.to_str().unwrap().to_string(), log)
}

#[cfg(unix)]
#[test]
fn sync_retries_a_prompt_requiring_fetch_with_prompts_allowed() {
    // spec: CLI-248 -- the parallel pass fails both sources the way a
    // suppressed credential prompt does; the serial retry, prompts allowed,
    // succeeds.
    let sbs = advanced_pinned_sources(2);
    let (path, log) = prompt_requiring_git(&sbs[0]);

    let r = sbs[0].mind_env(&["sync"], &[("PATH", path.as_str())]);
    assert!(r.success, "sync: {} / {}", r.stdout, r.stderr);
    let log = std::fs::read_to_string(log).unwrap();
    assert!(log.contains("prompt=0"), "parallel pass ran: {log}");
    let commits: Vec<String> = recorded_commits(&sbs[0])
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let heads: Vec<String> = sbs.iter().map(source_head).collect();
    assert_eq!(commits, heads);
}

#[cfg(unix)]
#[test]
fn max_parallel_fetches_of_one_syncs_serially_with_prompts_allowed() {
    // spec: CLI-249 -- a limit of 1 is the one-at-a-time sync: no fetch ever
    // runs with prompts off.
    let sbs = advanced_pinned_sources(3);
    let (path, log) = prompt_requiring_git(&sbs[0]);

    let r = sbs[0].mind_env(
        &["sync"],
        &[("PATH", path.as_str()), ("MIND_MAX_PARALLEL_FETCHES", "1")],
    );
    assert!(r.success, "sync: {} / {}", r.stdout, r.stderr);
    let log = std::fs::read_to_string(log).unwrap();
    assert_eq!(log.lines().count(), 3, "one fetch per source: {log}");
    assert!(
        !log.contains("prompt=0"),
        "serial fetches may prompt: {log}"
    );
    let commits: Vec<String> = recorded_commits(&sbs[0])
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let heads: Vec<String> = sbs.iter().map(source_head).collect();
    assert_eq!(commits, heads);
}

#[test]
fn sync_follow_branch_advances_commit() {
    // spec: CLI-55 (follow-branch resets to branch tip on sync)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-follow");
    let spec = sb.source_spec();

    // Meld at stable (sha_v1).
    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );
    let before = read_source_commit(&sb);
    assert_eq!(before, sha_v1);

    // Advance the `stable` branch on the remote.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev agent v3\n---\n# dev v3\n",
    );
    git(&sb.source, &["commit", "-aqm", "v3 on stable"]);
    // Move stable to the new HEAD.
    let sha_v3 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    // The new commit is on main; create stable pointing at it.
    git(&sb.source, &["branch", "-f", "stable", &sha_v3]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync follow-branch: {}", r.stderr);

    let after = read_source_commit(&sb);
    assert_eq!(after, sha_v3, "follow-branch source should advance on sync");
}

#[test]
fn sync_pin_ref_stays_fixed() {
    // spec: CLI-55 (pin-ref source stays fixed on sync)
    let (sb, sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-ref");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-ref", &sha_v1]).success);
    let before = read_source_commit(&sb);
    assert_eq!(before, sha_v1);

    // Advance main further.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: v99\n---\n# v99\n",
    );
    git(&sb.source, &["commit", "-aqm", "v99"]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync pin-ref: {}", r.stderr);

    let after = read_source_commit(&sb);
    assert_eq!(after, sha_v1, "pin-ref should be immutable across sync");
}

#[test]
fn sync_does_not_change_pin() {
    // spec: CLI-55 (sync never changes the pin itself, only moves HEAD)
    let (sb, _sha_v1, _sha_v2) = make_pinnable_repo("pintest-sync-nopin");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );

    // Capture the pin before sync.
    let pin_before = read_source_pin_json(&sb);

    sb.mind(&["sync"]);

    // Pin must be identical after sync.
    let pin_after = read_source_pin_json(&sb);
    assert_eq!(
        pin_before, pin_after,
        "sync must not modify the recorded pin"
    );
    // Specifically still follow-branch=stable.
    assert!(pin_after.contains("follow-branch"), "{pin_after}");
    assert!(pin_after.contains("stable"), "{pin_after}");
}

#[test]
fn sync_reclones_when_clone_dir_is_missing() {
    // spec: CLI-55 - sync resolves each source against its recorded pin. If the
    // clone dir has been removed out from under the registry, sync must recover
    // by re-cloning at the recorded pin rather than erroring, landing back on the
    // pinned commit.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-sync-missing");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1);

    // Delete the clone dir, simulating a wiped/partial sources tree.
    let clone = clone_dir_of(&sb, "pintest-sync-missing");
    assert!(clone.exists(), "clone should exist after meld");
    std::fs::remove_dir_all(&clone).unwrap();

    let r = sb.mind(&["sync"]);
    assert!(
        r.success,
        "sync must recover a missing clone dir: {}",
        r.stderr
    );
    // Recovered and still pinned at v1.0 (sha_v1), not main tip.
    assert_eq!(
        read_source_commit(&sb),
        sha_v1,
        "re-clone on sync must honor the recorded pin"
    );
    assert!(
        clone.join(".git").is_dir(),
        "sync should have re-created the clone"
    );
}

#[test]
fn sync_pin_tag_picks_up_moved_upstream_tag() {
    // spec: CLI-55 - the moved-tag force-fetch is observable end-to-end via the
    // CLI: a re-pointed upstream tag advances the recorded commit on sync (the
    // git-layer unit test alone does not exercise the meld+sync+registry path).
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-moved-tag");
    let spec = sb.source_spec();

    assert!(sb.mind(&["meld", &spec, "--pin-tag", "v1.0"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1, "pinned at v1.0 == sha_v1");

    // Add a new commit upstream and re-point v1.0 at it.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: retagged\n---\n# retagged\n",
    );
    git(&sb.source, &["commit", "-aqm", "retag target"]);
    let sha_new = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&sb.source, &["tag", "-f", "v1.0", &sha_new]);

    let r = sb.mind(&["sync"]);
    assert!(r.success, "sync after moving tag: {}", r.stderr);
    assert_eq!(
        read_source_commit(&sb),
        sha_new,
        "a re-pointed upstream tag must be picked up by sync (force-fetch)"
    );
    // And the pin itself is unchanged (still tag v1.0).
    let pin_json = read_source_pin_json(&sb);
    assert!(pin_json.contains("\"tag\""), "{pin_json}");
    assert!(pin_json.contains("v1.0"), "{pin_json}");
}

// ---- scan roots integration tests (DSC-50, DSC-51, DSC-52, DSC-53, STO-17, CLI-16) ---

#[test]
fn sync_source_upgrade_scopes_to_the_named_source_only() {
    // spec: CLI-232 -- H2: `sync <source> --upgrade` used to call `upgrade_inner`
    // with `item_ref = None`, so `mind sync alpha --upgrade --yes` upgraded
    // EVERY installed item from EVERY melded source, not just `alpha`. Two
    // sources with DISTINCTLY NAMED items (so both can be installed at once
    // with no namespace collision): advance both upstream, then
    // `sync alpha --upgrade --yes` must upgrade alpha's item and leave beta's
    // untouched.
    let alpha = Sandbox::bare("alpha");
    alpha.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let beta = Sandbox::bare("beta");
    beta.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );

    assert!(alpha.mind(&["meld", &alpha.source_spec()]).success);
    assert!(alpha.mind(&["meld", &beta.source_spec()]).success);
    assert!(alpha.mind(&["learn", "skill:widget-a"]).success);
    assert!(alpha.mind(&["learn", "skill:widget-b"]).success);

    // Advance BOTH sources' content.
    alpha.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    beta.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );

    let r = alpha.mind(&["sync", "alpha", "--upgrade", "--yes"]);
    assert!(
        r.success,
        "sync alpha --upgrade failed: {} {}",
        r.stdout, r.stderr
    );

    let a_store =
        std::fs::read_to_string(alpha.mind_home.join("store/skill/widget-a/SKILL.md")).unwrap();
    assert!(
        a_store.contains("edited"),
        "alpha's item must be upgraded by `sync alpha --upgrade`: {a_store}"
    );
    let b_store =
        std::fs::read_to_string(alpha.mind_home.join("store/skill/widget-b/SKILL.md")).unwrap();
    assert!(
        !b_store.contains("edited"),
        "beta's item must NOT be upgraded by a `sync alpha --upgrade` that \
         only named alpha: {b_store}"
    );

    // `recall` agrees: alpha's item is no longer flagged outdated, beta's still is.
    let recall = alpha.mind(&["recall"]);
    assert!(recall.success, "{}", recall.stderr);
    assert!(
        recall.stdout.contains("widget-b") && recall.stdout.contains("outdated"),
        "beta's item must still be reported outdated: {}",
        recall.stdout
    );
}

#[test]
fn upgrade_source_syncs_and_upgrades_only_the_named_source() {
    // spec: CLI-244 -- `upgrade --source alpha` fetches and upgrades alpha
    // alone: beta's item is not upgraded, and beta's clone is not fetched, so a
    // later `--no-sync` pass over beta still sees no delta.
    let alpha = Sandbox::bare("alpha");
    alpha.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let beta = Sandbox::bare("beta");
    beta.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );

    // --follow-branch makes each a real clone, so a sync is an actual fetch.
    for spec in [alpha.source_spec(), beta.source_spec()] {
        let m = alpha.mind(&["meld", &spec, "--follow-branch", "main", "--register-only"]);
        assert!(m.success, "{} {}", m.stdout, m.stderr);
    }
    assert!(alpha.mind(&["learn", "skill:widget-a"]).success);
    assert!(alpha.mind(&["learn", "skill:widget-b"]).success);

    alpha.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    beta.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );

    let r = alpha.mind(&["upgrade", "--source", "alpha", "--yes", "--json"]);
    assert!(
        r.success,
        "upgrade --source alpha failed: {} {}",
        r.stdout, r.stderr
    );
    let v = parse_json(&r.stdout);
    assert_eq!(v["outcome"], "upgraded", "{}", r.stdout);

    let a_store =
        std::fs::read_to_string(alpha.mind_home.join("store/skill/widget-a/SKILL.md")).unwrap();
    assert!(
        a_store.contains("edited"),
        "alpha's item must upgrade: {a_store}"
    );
    let b_store =
        std::fs::read_to_string(alpha.mind_home.join("store/skill/widget-b/SKILL.md")).unwrap();
    assert!(
        !b_store.contains("edited"),
        "beta's item must not upgrade under `--source alpha`: {b_store}"
    );

    // beta was never fetched: its clone is stale, so a no-sync pass sees nothing.
    let stale = alpha.mind(&["upgrade", "--source", "beta", "--no-sync", "--json"]);
    assert!(stale.success, "{} {}", stale.stdout, stale.stderr);
    assert_eq!(
        parse_json(&stale.stdout)["outcome"],
        "up-to-date",
        "`--source alpha` must not have fetched beta: {}",
        stale.stdout
    );

    // A sync-first pass over beta now fetches and upgrades it.
    let b = alpha.mind(&["upgrade", "--source", "beta", "--yes", "--json"]);
    assert!(b.success, "{} {}", b.stdout, b.stderr);
    assert_eq!(parse_json(&b.stdout)["outcome"], "upgraded", "{}", b.stdout);
}

#[test]
fn upgrade_source_filter_matching_nothing_is_source_not_found() {
    // spec: CLI-244 -- the filter follows CLI-231: no match is an error, so a
    // typo is not reported as "everything is up to date".
    let sb = Sandbox::bare("alpha");
    sb.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["upgrade", "--source", "nope", "--yes"]);
    assert!(!r.success, "an unmatched --source must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("nope"),
        "error must name the filter: {}",
        r.stderr
    );

    // A malformed glob is rejected before anything is fetched.
    let bad = sb.mind(&["upgrade", "--source", "[bad", "--yes"]);
    assert!(!bad.success, "a malformed glob must fail: {}", bad.stdout);
    assert!(
        bad.stderr.contains("not a valid glob selector"),
        "expected invalid-pattern error: {}",
        bad.stderr
    );
}

#[test]
fn upgrade_source_with_item_names_only_the_intersection() {
    // spec: CLI-244 -- `[item]` and `--source` intersect: a suffix item ref
    // matching two sources, narrowed by `--source`, upgrades one source and
    // does not disclose the other as in scope (CLI-233).
    let one = Sandbox::bare("skills");
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let two = Sandbox::bare("skills");
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );
    assert!(one.mind(&["meld", &one.source_spec()]).success);
    assert!(one.mind(&["meld", &two.source_spec()]).success);
    assert!(one.mind(&["learn", "skill:widget-a"]).success);
    assert!(one.mind(&["learn", "skill:widget-b"]).success);
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );

    // Without --source the item ref spans both sources and discloses that.
    let wide = one.mind(&["upgrade", "skills#*", "--no-sync"]);
    assert!(wide.stdout.contains("matched 2 sources"), "{}", wide.stdout);

    // A --source that still spans both: the note quotes both filters.
    let both = one.mind(&["upgrade", "skills#*", "--source", "skills", "--no-sync"]);
    assert!(
        both.stdout
            .contains("the filters \"skills#*\" and \"skills\" together matched 2 sources"),
        "{}",
        both.stdout
    );

    let one_id = format!("{}/skills", one.base_name());
    let r = one.mind(&["upgrade", "skills#*", "--source", &one_id, "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let a_store =
        std::fs::read_to_string(one.mind_home.join("store/skill/widget-a/SKILL.md")).unwrap();
    assert!(a_store.contains("edited"), "{a_store}");
    let b_store =
        std::fs::read_to_string(one.mind_home.join("store/skill/widget-b/SKILL.md")).unwrap();
    assert!(!b_store.contains("edited"), "{b_store}");
    assert!(
        !r.stdout.contains("matched 2 sources"),
        "the disclosure must not name a source outside the intersection: {}",
        r.stdout
    );
}

#[test]
fn sync_multi_match_filter_upgrade_names_every_matched_source_before_consent() {
    // spec: CLI-233 -- CLI-232 scoped `sync <filter> --upgrade` to the matched
    // sources, but a FILTER can match more sources than the user pictured while
    // reading as if it named one. Two sources whose identities share the
    // trailing component `skills` (the sandbox identity is
    // `<base_name>/<source dir name>`, and each Sandbox gets its own base), so
    // the filter `skills` matches BOTH: the blast radius (each matched source's
    // items upgraded, and its install hook possibly re-run, HOOK-11) must be
    // named before consent, in the text prompt and in the `--json` refusal.
    let one = Sandbox::bare("skills");
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let two = Sandbox::bare("skills");
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );

    assert!(one.mind(&["meld", &one.source_spec()]).success);
    assert!(one.mind(&["meld", &two.source_spec()]).success);
    assert!(one.mind(&["learn", "skill:widget-a"]).success);
    assert!(one.mind(&["learn", "skill:widget-b"]).success);

    // Both sources drift, so both have a pending upgrade for the filter to hit.
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );

    let one_id = format!("{}/skills", one.base_name());
    let two_id = format!("{}/skills", two.base_name());

    // Text mode, no `--yes`: the note is printed BEFORE the confirmation is
    // sought, so it lands on stdout even though the non-TTY confirm then
    // refuses (LIFE-45). Both matched identities must be named.
    //
    // spec: CLI-233 CLI-235 -- H3: `sync_with_selector` already prints
    // "syncing {source.name} ..." for EVERY in-scope source before the
    // `--upgrade` pass even starts (`sync_inner`), so both identities land on
    // stdout regardless of whether the CLI-233/CLI-235 disclosure fires at
    // all. Asserting only `contains(&one_id) && contains(&two_id)` is
    // therefore vacuous: reverting `multi_source_upgrade_note` entirely still
    // leaves it green. Anchor on the discriminating substring the
    // disclosure itself introduces ("this filter matched"/the filter text),
    // and require it to precede the confirm prompt.
    let r = one.mind_tty(&["sync", "skills", "--upgrade"], "n\n");
    assert!(
        r.stdout.contains("matched 2 sources"),
        "the disclosure's discriminating substring must appear on stdout: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains(&one_id) && r.stdout.contains(&two_id),
        "the confirmation must name every matched source ({one_id}, {two_id}): {}",
        r.stdout
    );
    let disclosure_pos = r.stdout.find("matched 2 sources").expect("checked above");
    let prompt_pos = r
        .stdout
        .find("apply these upgrades?")
        .expect("the confirm prompt must still be reached (both sources have a pending upgrade)");
    assert!(
        disclosure_pos < prompt_pos,
        "the disclosure must precede the confirm prompt: {}",
        r.stdout
    );
    // Declining exits 0 having changed nothing. The invariant that matters is
    // that it did not apply without consent.
    let unapplied =
        std::fs::read_to_string(one.mind_home.join("store/skill/widget-a/SKILL.md")).unwrap();
    assert!(
        !unapplied.contains("edited"),
        "nothing may be applied without consent: {unapplied}"
    );

    // `--json` never prompts, so the same disclosure must ride the refusal's
    // `action` text instead.
    let j = one.mind(&["sync", "skills", "--upgrade", "--json"]);
    assert!(!j.success);
    let v = parse_json(&j.stdout);
    assert_eq!(
        v["error"]["kind"], "confirmation-required",
        "expected the LIFE-45 refusal: {v}"
    );
    let action = v["error"]["message"].as_str().unwrap_or_default();
    assert!(
        action.contains(&one_id) && action.contains(&two_id),
        "the --json refusal must name every matched source: {action}"
    );

    // A filter that resolves to ONE source reads like naming it already, so it
    // must not grow the note.
    //
    // spec: CLI-233 CLI-235 -- H3: the negative assertion below is purely
    // negative (absence of "matched N sources"); on its own it would also
    // pass a regression where this run errored early or never reached the
    // confirm block at all. Anchor it with a positive assertion that the
    // run really did reach the (unscoped) confirmation prompt.
    let single = one.mind_tty(&["sync", &one_id, "--upgrade"], "n\n");
    assert!(
        single.stdout.contains("apply these upgrades?"),
        "the single-match run must still reach the confirm prompt \
         (positive anchor for the negative assertion below): {}",
        single.stdout
    );
    assert!(
        !(single.stdout.contains("matched") && single.stdout.contains("sources;")),
        "a single-match filter must not grow the confirmation: {}",
        single.stdout
    );

    // With consent, the multi-match filter really does upgrade both.
    let applied = one.mind(&["sync", "skills", "--upgrade", "--yes"]);
    assert!(
        applied.success,
        "sync skills --upgrade --yes failed: {} {}",
        applied.stdout, applied.stderr
    );
    for item in ["widget-a", "widget-b"] {
        let store =
            std::fs::read_to_string(one.mind_home.join(format!("store/skill/{item}/SKILL.md")))
                .unwrap();
        assert!(
            store.contains("edited"),
            "{item} must be upgraded by the multi-match filter: {store}"
        );
    }
}

#[test]
fn upgrade_item_ref_filter_names_every_matched_source_before_consent() {
    // spec: CLI-234 -- H2's HOOK-11 half of CLI-233 also applies to `upgrade
    // <item>` directly, not only to `sync <filter> --upgrade`: an item-ref's
    // embedded `owner/repo#` qualifier is a FILTER over sources (CLI-5,
    // CLI-63), so `mind upgrade '<suffix>#*'` can match more than one source
    // exactly like a `sync` filter can. Before CLI-234, `source_scope` (the
    // only input to the old `scope_names`) is always `None` for a plain
    // `upgrade <item>` call, so this note never fired no matter how many
    // sources the item-ref filter matched. Two sources sharing the trailing
    // identity component `skills`, both with an installed, drifted item:
    // `mind upgrade 'skills#*'` must name both before the confirm prompt.
    let one = Sandbox::bare("skills");
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let two = Sandbox::bare("skills");
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );

    assert!(one.mind(&["meld", &one.source_spec()]).success);
    assert!(one.mind(&["meld", &two.source_spec()]).success);
    assert!(one.mind(&["learn", "skill:widget-a"]).success);
    assert!(one.mind(&["learn", "skill:widget-b"]).success);

    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\nedited\n",
    );
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\nedited\n",
    );

    let one_id = format!("{}/skills", one.base_name());
    let two_id = format!("{}/skills", two.base_name());

    let r = one.mind_tty(&["upgrade", "skills#*"], "n\n");
    assert!(
        r.stdout.contains("matched 2 sources"),
        "the disclosure's discriminating substring must appear on stdout: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains(&one_id) && r.stdout.contains(&two_id),
        "the disclosure must name every matched source ({one_id}, {two_id}): {}",
        r.stdout
    );
    let disclosure_pos = r.stdout.find("matched 2 sources").expect("checked above");
    let prompt_pos = r
        .stdout
        .find("apply these upgrades?")
        .expect("both sources have a pending upgrade, so the confirm prompt must be reached");
    assert!(
        disclosure_pos < prompt_pos,
        "the disclosure must precede the confirm prompt: {}",
        r.stdout
    );

    // Nothing applied without consent (the prompt was declined).
    let unapplied =
        std::fs::read_to_string(one.mind_home.join("store/skill/widget-a/SKILL.md")).unwrap();
    assert!(
        !unapplied.contains("edited"),
        "nothing may be applied without consent: {unapplied}"
    );
}

#[test]
fn upgrade_disclosure_fires_before_the_hook_rerun_pass_and_under_yes() {
    // spec: CLI-235 -- (a) the disclosure must precede the first hook-execution
    // marker in stdout (M1: it used to print only from inside the `!yes`
    // confirmation block, which runs AFTER `rerun_source_hooks`, so a human
    // reading the transcript saw the hook already run before ever being told
    // more than one source was in scope). (c) `--yes` must not suppress the
    // disclosure either, even though it skips the confirmation prompt
    // entirely (unlike a `[Y/n]` prompt, this is a disclosure, not a
    // confirmation step).
    let one = sandbox_with_declared_hook("skills", "touch hookran-one");
    let two = sandbox_with_declared_hook("skills", "touch hookran-two");
    // Distinctly-named items (beyond the shared "review"/"dev"/"style"
    // fixture both sources carry) so `mind learn` does not collide, and so
    // the `skills#*` item-ref filter's `hook_scope` (computed from installed
    // items matching the filter) includes BOTH sources: with no installed
    // item at all, `hook_scope` would be the empty set and neither hook
    // would be considered due, defeating this test.
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );
    assert!(
        one.mind(&[
            "meld",
            &one.source_spec(),
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(
        one.mind(&[
            "meld",
            &two.source_spec(),
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(one.mind(&["learn", "skill:widget-a"]).success);
    assert!(one.mind(&["learn", "skill:widget-b"]).success);
    let marker_one = one.source.join("hookran-one");
    let marker_two = two.source.join("hookran-two");
    assert!(marker_one.exists() && marker_two.exists());
    std::fs::remove_file(&marker_one).unwrap();
    std::fs::remove_file(&marker_two).unwrap();

    // Advance both sources so each source's install hook is due to re-run
    // (`hook_rerun_warranted`: commit moved past the hook's last-run commit).
    one.edit_source();
    two.edit_source();

    // (c): `--yes` applies without a confirmation prompt, but the disclosure
    // must still print.
    let r = one.mind(&[
        "upgrade",
        "skills#*",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        r.success,
        "upgrade 'skills#*' --yes failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("matched 2 sources"),
        "--yes must not suppress the multi-source disclosure: {}",
        r.stdout
    );

    // (a): the disclosure precedes the first hook-execution marker
    // ("note: re-running install hook for ..." / "re-ran install hook for
    // ..."), both of which contain "install hook for".
    let disclosure_pos = r.stdout.find("matched 2 sources").expect("checked above");
    let hook_marker_pos = r.stdout.find("install hook for").expect(
        "both sources' hooks are due to re-run under --accept-hooks=install,update,uninstall",
    );
    assert!(
        disclosure_pos < hook_marker_pos,
        "the disclosure must precede the first hook-execution marker: {}",
        r.stdout
    );
    assert!(marker_one.exists() && marker_two.exists());
}

#[test]
fn upgrade_disclosure_fires_even_with_nothing_pending() {
    // spec: CLI-235 -- (b) a filter matching more than one source must still
    // disclose even when neither matched source ends up with a pending
    // upgrade: M1's bug was that the note lived inside the `pending.is_empty()`
    // early-return's unreachable tail, so a re-run that found nothing pending
    // (after possibly re-running install hooks for every matched source)
    // reported "everything is up to date" with no disclosure at all.
    let one = Sandbox::bare("skills");
    one.write_and_commit(
        "skills/widget-a/SKILL.md",
        "---\nname: widget-a\ndescription: a\n---\n# widget a\n",
    );
    let two = Sandbox::bare("skills");
    two.write_and_commit(
        "skills/widget-b/SKILL.md",
        "---\nname: widget-b\ndescription: b\n---\n# widget b\n",
    );

    assert!(one.mind(&["meld", &one.source_spec()]).success);
    assert!(one.mind(&["meld", &two.source_spec()]).success);
    assert!(one.mind(&["learn", "skill:widget-a"]).success);
    assert!(one.mind(&["learn", "skill:widget-b"]).success);

    // Neither source drifts: both installed items stay up to date.
    let r = one.mind(&["upgrade", "skills#*"]);
    assert!(
        r.success,
        "upgrade 'skills#*' failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("matched 2 sources"),
        "the disclosure must fire even with nothing pending: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("up to date"),
        "the run must still report up to date: {}",
        r.stdout
    );
}

#[test]
fn sync_source_upgrade_does_not_rerun_an_unrelated_sources_hook() {
    // spec: CLI-232 -- H2's HOOK-11 half: `sync <source> --upgrade` ran
    // `rerun_source_hooks` with `hook_scope = None` (every source), so it could
    // re-run an unrelated source's install hook (arbitrary shell) even though
    // the caller only named a different source. Meld a hooked source
    // (`agents`) and a hook-free source (`tools`); advance the hooked source;
    // `sync tools --upgrade` must NOT re-run the hooked source's install hook.
    let agents = sandbox_with_declared_hook("agents", "touch hookran");
    let agents_spec = agents.source_spec();
    assert!(
        agents
            .mind(&[
                "meld",
                &agents_spec,
                "--accept-hooks=install,update,uninstall"
            ])
            .success,
        "initial meld of the hooked source should run the hook and record its commit"
    );

    let tools = Sandbox::named("tools");
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of the second (hook-free) source failed"
    );

    let marker = agents.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on initial meld");
    std::fs::remove_file(&marker).unwrap();

    // Advance the hooked source so its commit moves past the recorded
    // run-commit -- an unscoped (or `agents`-scoped) `--upgrade` pass would
    // re-run its hook. A `tools`-scoped one must not.
    agents.edit_source();

    let r = agents.mind(&[
        "sync",
        "tools",
        "--upgrade",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        r.success,
        "sync tools --upgrade failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !marker.exists(),
        "`sync tools --upgrade` must not re-run the unrelated `agents` source's \
         install hook: {} exists",
        marker.display()
    );

    // Positive control: `sync agents --upgrade` (naming the hooked source
    // itself) DOES re-run it.
    let r2 = agents.mind(&[
        "sync",
        "agents",
        "--upgrade",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        r2.success,
        "sync agents --upgrade failed: {} {}",
        r2.stdout, r2.stderr
    );
    assert!(
        marker.exists(),
        "`sync agents --upgrade` (naming the hooked source) must re-run its \
         install hook: {} missing",
        marker.display()
    );
}

#[test]
fn upgrade_source_does_not_rerun_an_unrelated_sources_hook() {
    // spec: CLI-244 HOOK-11 -- `upgrade --source tools` re-runs only tools'
    // hooks; the advanced, hooked `agents` source is left alone until named.
    let agents = sandbox_with_declared_hook("agents", "touch hookran");
    let agents_spec = agents.source_spec();
    assert!(
        agents
            .mind(&[
                "meld",
                &agents_spec,
                "--accept-hooks=install,update,uninstall"
            ])
            .success
    );
    let tools = Sandbox::named("tools");
    assert!(agents.mind(&["meld", &tools.source_spec()]).success);

    let marker = agents.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on initial meld");
    std::fs::remove_file(&marker).unwrap();
    agents.edit_source();

    let r = agents.mind(&[
        "upgrade",
        "--source",
        "tools",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !marker.exists(),
        "`upgrade --source tools` must not re-run agents' hook"
    );

    let r2 = agents.mind(&[
        "upgrade",
        "--source",
        "agents",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r2.success, "{} {}", r2.stdout, r2.stderr);
    assert!(
        marker.exists(),
        "`upgrade --source agents` must re-run its hook"
    );
}

#[test]
fn sync_source_selector_scopes_to_one_source() {
    // spec: CLI-231
    let sb = Sandbox::named("alpha");

    // A second source (`beta`) melded into the same mind home.
    let beta = sb.base.join("beta");
    write(
        &beta.join("skills/scan/SKILL.md"),
        "---\nname: scan\ndescription: b\n---\n# scan\n",
    );
    git(&beta, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&beta, &["config", "user.email", "t@t"]);
    git(&beta, &["config", "user.name", "t"]);
    git(&beta, &["add", "-A"]);
    git(&beta, &["commit", "-qm", "initial"]);
    let beta_spec = beta.to_string_lossy().into_owned();

    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );
    assert!(sb.mind(&["meld", &beta_spec, "--register-only"]).success);

    // Targeted sync fetches only the matching source.
    let one = sb.mind(&["sync", "alpha"]);
    assert!(one.success, "sync alpha failed: {}", one.stderr);
    assert!(
        one.stdout.contains("alpha"),
        "sync alpha must refresh alpha: {}",
        one.stdout
    );
    assert!(
        !one.stdout.contains("beta"),
        "sync alpha must NOT refresh beta: {}",
        one.stdout
    );

    // A selector that matches no source is a SourceNotFound error.
    let bad = sb.mind(&["sync", "no-such-source"]);
    assert!(
        !bad.success,
        "an unknown selector must fail: {}",
        bad.stdout
    );
    assert!(
        bad.stderr.contains("no melded source matches"),
        "unknown selector must be SourceNotFound: {}",
        bad.stderr
    );

    // No selector still fetches every source (CLI-50 unchanged).
    let all = sb.mind(&["sync"]);
    assert!(all.success, "bare sync failed: {}", all.stderr);
    assert!(
        all.stdout.contains("alpha") && all.stdout.contains("beta"),
        "bare sync must fetch both sources: {}",
        all.stdout
    );
}

/// `sync '[bad'` carries glob metacharacters but is not a valid pattern. It
/// must report InvalidPattern (validated up front, CLI-231) rather than
/// silently matching zero sources or surfacing as SourceNotFound.
#[test]
fn sync_malformed_glob_selector_reports_invalid_pattern() {
    // spec: CLI-231
    let sb = Sandbox::named("alpha");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );

    let r = sb.mind(&["sync", "[bad"]);
    assert!(!r.success, "malformed glob must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("not a valid glob selector"),
        "expected invalid-pattern error, got stderr={} stdout={}",
        r.stderr,
        r.stdout
    );
    assert!(
        !r.stderr.contains("no source named"),
        "malformed glob must NOT surface as SourceNotFound: {}",
        r.stderr
    );
}

// ---- Downgrade recipe: re-pin to an older sha, then upgrade ----------------
