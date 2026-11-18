//! `upgrade` (spec/cli.md).

use super::*;

#[test]
fn recall_degrades_past_a_vanished_linked_source_instead_of_hard_failing() {
    // spec: CLI-212 CLI-213
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);

    // Melded directory goes away (the docs' own /tmp recipe, no teardown).
    std::fs::remove_dir_all(&sb.source).unwrap();

    // Before this fix: recall hard-fails (exit 1) on the gone source. After:
    // it degrades, warning on stderr and still reporting the source is melded.
    let r = sb.mind(&["recall"]);
    assert!(
        r.success,
        "recall must degrade past a vanished linked source, not hard-fail: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("gone") || r.stderr.contains("unmeld"),
        "must warn about the vanished source with an actionable hint: {}",
        r.stderr
    );

    // recall --sources still lists it (unaffected by the scan failure).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success);
    assert!(
        sources.stdout.contains("agents"),
        "the source itself must still be listed: {}",
        sources.stdout
    );

    // probe (the plain non-TUI fallback under a non-TTY stdin) also degrades.
    let p = sb.mind(&["probe"]);
    assert!(
        p.success,
        "probe must degrade past a vanished linked source: {}\n{}",
        p.stdout, p.stderr
    );
}

#[test]
fn introspect_reports_a_vanished_linked_source_as_an_issue_not_a_hard_error() {
    // spec: CLI-212 -- introspect's existing CLI-210 per-source loop picks up
    // LinkedSourceGone for free, with no commands/ change needed.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    std::fs::remove_dir_all(&sb.source).unwrap();

    let r = sb.mind(&["introspect"]);
    assert!(
        r.success,
        "introspect must not hard-fail: {}\n{}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("gone") || r.stdout.contains("unmeld"),
        "introspect must report the vanished source as an actionable issue: {}",
        r.stdout
    );
}

#[test]
fn recall_still_lists_the_healthy_sources_items_when_another_source_is_gone() {
    // spec: CLI-213 -- "the sources that DID scan are still shown/searched/
    // resolved against". The existing degradation tests register exactly ONE
    // source (the gone one), so they cannot tell "degraded past it" apart from
    // "returned nothing at all". Here a healthy source's items must still be
    // listed after the broken source is skipped.
    let sb = melded(); // healthy `agents` source: review, dev, style.
    let broken = Sandbox::bare("gone-src");
    broken.write_and_commit(
        "skills/ghost/SKILL.md",
        "---\nname: ghost\ndescription: ghost skill\n---\n# ghost\n",
    );
    assert!(
        sb.mind(&["meld", &broken.source_spec(), "--register-only"])
            .success
    );
    std::fs::remove_dir_all(&broken.source).unwrap();

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall must degrade: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("review") && r.stdout.contains("dev"),
        "the healthy source's items must still be listed: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("ghost"),
        "the gone source contributes nothing: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("gone-src"),
        "the warning must name the skipped source: {}",
        r.stderr
    );

    // `probe <query>` searches the partial catalog rather than reporting an
    // empty index.
    let p = sb.mind(&["probe", "review", "--no-tui"]);
    assert!(p.success, "probe must degrade: {} {}", p.stdout, p.stderr);
    assert!(
        p.stdout.contains("review"),
        "probe must still search the sources that did scan: {}",
        p.stdout
    );
}

// ---- B4/DSC-94: source-derived descriptions are sanitized at capture -------

#[test]
fn learn_still_resolves_an_item_when_another_source_is_gone() {
    // spec: CLI-213 -- `learn`'s item resolution is named explicitly in the
    // spec as one of the three degrading call sites, and nothing drove it: a
    // dead source must not make every OTHER source's items unlearnable.
    let sb = melded();
    let broken = Sandbox::bare("gone-for-learn");
    broken.write_and_commit(
        "skills/ghost/SKILL.md",
        "---\nname: ghost\ndescription: ghost skill\n---\n# ghost\n",
    );
    assert!(
        sb.mind(&["meld", &broken.source_spec(), "--register-only"])
            .success
    );
    std::fs::remove_dir_all(&broken.source).unwrap();

    let r = sb.mind(&["learn", "review"]);
    assert!(
        r.success,
        "learn must still resolve items from the healthy source: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.claude_home.join("skills/review/SKILL.md").exists(),
        "the item must actually be installed"
    );
}

#[test]
fn dump_still_hard_fails_on_a_vanished_linked_source() {
    // spec: CLI-213 -- "`dump`'s reconstruction is unaffected: it still
    // hard-fails on any scan error, including this one -- a partial dump is
    // worse than none". The degradation added to `catalog::scan` must not have
    // leaked into `dump`, which uses `catalog::scan_source` per source.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    std::fs::remove_dir_all(&sb.source).unwrap();

    let out_path = sb.base.join("dumped.toml");
    let out_str = out_path.to_string_lossy().into_owned();
    let r = sb.mind(&["dump", "--output", &out_str]);
    assert!(
        !r.success,
        "dump must refuse a partial reproduction: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        format!("{}{}", r.stdout, r.stderr).contains("gone"),
        "dump must name the vanished working tree as the cause: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
    assert!(
        !out_path.exists(),
        "no partial dump file may be left behind: {out_path:?}"
    );
}

#[test]
fn meld_of_a_nonexistent_local_path_still_hard_fails_and_registers_nothing() {
    // spec: CLI-213 -- "`meld`'s own scan of the source it just cloned is
    // unaffected: it still hard-fails on any scan error, including this one".
    // `meld` scans through `catalog::scan` (with a one-source registry), which
    // is the function that now DEGRADES on `LinkedSourceGone`, so the claim
    // rests entirely on meld never reaching that state. This pins the guard:
    // a local path that is not a directory is refused before the scan, so the
    // degradation cannot turn a typo'd `mind meld ./tpyo` into a silently
    // registered empty source.
    let sb = Sandbox::bare("meld-guard");
    let missing = sb.base.join("no-such-dir").to_string_lossy().into_owned();
    let r = sb.mind(&["meld", &missing]);
    assert!(
        !r.success,
        "meld of a nonexistent local path must fail: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not a directory"),
        "the error must name the cause: {}",
        r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("no-such-dir"),
        "nothing may be registered: {}",
        sources.stdout
    );
}

#[test]
fn remelding_a_vanished_linked_source_names_it_rather_than_reporting_zero_items() {
    // `source_status` (reached from `meld`'s already-melded branch) scans the one
    // named source through `scan_one`/`catalog::scan_source`, NOT the
    // whole-registry `catalog::scan` that CLI-212/CLI-213 made degrade past a
    // `LinkedSourceGone` source. With the degrading scan, re-melding a source
    // whose working tree has vanished printed
    //     * local/<owner>/agents is already melded
    //     * local/<owner>/agents: 0 item(s) (source @ <sha>)
    // and exited 0, describing a dead source as a healthy source that happens to
    // offer nothing: stdout contradicted the stderr warning beside it, and the
    // exit status said success. CLI-213 reserves the degradation for LISTING
    // every source; a caller that named one source is told what happened to it.
    // spec: CLI-212 CLI-213
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--register-only"]).success);
    std::fs::remove_dir_all(&sb.source).unwrap();

    let r = sb.mind(&["meld", &spec, "--register-only"]);
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("gone") || combined.contains("not a directory"),
        "re-melding a vanished source must name the vanished working tree rather than \
         reporting a healthy source with no items: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
    assert!(
        !r.stdout.contains("0 item(s)"),
        "a vanished source must not be reported as an empty but healthy source: {}",
        r.stdout
    );
    assert!(
        !r.success,
        "the scan error must fail the command, not be degraded into a success: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn upgrade_reports_nothing_when_up_to_date() {
    // spec: CLI-64
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let r = sb.mind(&["upgrade"]);
    assert!(r.stdout.contains("up to date"), "{}", r.stdout);
}

#[test]
fn upgrade_reports_delta_and_declining_changes_nothing() {
    // spec: CLI-60, CLI-61
    let sb = melded();
    sb.mind(&["learn", "review"]);
    sb.edit_source();
    sb.mind(&["sync"]);

    // Dry-run report: shows hash and commit deltas with arrows.
    let report = sb.mind_tty(&["upgrade"], "n\n");
    assert!(report.stdout.contains("skill:review"), "{}", report.stdout);
    assert!(report.stdout.contains("hash"), "{}", report.stdout);
    assert!(report.stdout.contains("->"), "{}", report.stdout);
    assert!(report.stdout.contains("aborted"), "{}", report.stdout);

    // Declining must leave the installed commit untouched.
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    let again = sb.mind_tty(&["upgrade"], "n\n");
    assert!(again.stdout.contains("aborted"));
    assert_eq!(before, sb.mind(&["recall", "skill:review"]).stdout);
}

#[test]
fn upgrade_without_a_tty_or_yes_refuses() {
    // spec: CLI-60 CLI-259 -- piped stdin is not consent: the run refuses
    // instead of reading a reply, and changes nothing.
    let sb = melded();
    sb.mind(&["learn", "review"]);
    sb.edit_source();
    sb.mind(&["sync"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    let r = sb.mind_with_input(&["upgrade"], Some("y\n"));
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
    assert_eq!(before, sb.mind(&["recall", "skill:review"]).stdout);
}

#[test]
fn upgrade_prompt_defaults_to_yes_on_bare_enter() {
    // spec: CLI-60 CLI-258 - the apply prompt defaults to Yes, so a bare Enter
    // applies the upgrade, and so does EOF.
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;
    sb.edit_source();
    sb.mind(&["sync"]);

    // A bare Enter (newline, not EOF) confirms.
    let applied = sb.mind_tty(&["upgrade"], "\n");
    assert!(applied.success, "{}", applied.stderr);
    assert!(
        applied.stdout.contains("upgraded skill:review"),
        "a bare Enter must apply the upgrade: {}",
        applied.stdout
    );
    assert_ne!(
        before,
        sb.mind(&["recall", "skill:review"]).stdout,
        "the installed commit should have advanced"
    );

    // EOF (no input at all) takes the same default.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nedited again\n",
    );
    sb.mind(&["sync"]);
    let eof = sb.mind_tty(&["upgrade"], "");
    assert!(
        eof.stdout.contains("upgraded skill:review"),
        "EOF must apply: {}",
        eof.stdout
    );
}

#[test]
fn upgrade_yes_applies_and_is_then_idempotent() {
    // spec: CLI-62, LIFE-13
    let sb = melded();
    sb.mind(&["learn", "review"]);
    let before = sb.mind(&["recall", "skill:review"]).stdout;

    sb.edit_source();
    sb.mind(&["sync"]);

    let applied = sb.mind(&["upgrade", "--yes"]);
    assert!(applied.success, "{}", applied.stderr);
    assert!(
        applied.stdout.contains("upgraded skill:review"),
        "{}",
        applied.stdout
    );

    let after = sb.mind(&["recall", "skill:review"]).stdout;
    assert_ne!(before, after, "commit/hash should have advanced");

    // Running again finds nothing to do.
    let idem = sb.mind(&["upgrade"]);
    assert!(idem.stdout.contains("up to date"), "{}", idem.stdout);
}

#[test]
fn upgrade_item_filter_limits_to_one() {
    // spec: CLI-63
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Change both items upstream.
    sb.edit_source(); // touches skills/review
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // Filtered upgrade applies only the named item.
    let ev = sb.mind(&["upgrade", "--yes", "review"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(ev.stdout.contains("upgraded skill:review"), "{}", ev.stdout);
    assert!(!ev.stdout.contains("agent:dev"), "{}", ev.stdout);

    // dev is still pending (reported by an unfiltered, declined upgrade).
    let rest = sb.mind(&["upgrade"]);
    assert!(rest.stdout.contains("agent:dev"), "{}", rest.stdout);
    assert!(!rest.stdout.contains("skill:review"), "{}", rest.stdout);
}

#[test]
fn upgrade_glob_upgrades_multiple_items() {
    // spec: CLI-65 -- a glob ref (`skill:*`) upgrades every matching pending item
    // in a single `upgrade --yes` pass; `agents#*` (source-scoped) upgrades all
    // pending items in that source.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    // Make both items drift upstream.
    sb.edit_source(); // touches skills/review
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // A kind-scoped glob upgrades all matching items in one pass.
    let ev = sb.mind(&["upgrade", "--yes", "skill:*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:review"),
        "skill:* must upgrade the skill: {}",
        ev.stdout
    );
    // agent:dev is out of scope for `skill:*`.
    assert!(
        !ev.stdout.contains("upgraded agent:dev"),
        "skill:* must not touch the agent: {}",
        ev.stdout
    );

    // Use source-scoped glob `agents#*` to upgrade the agent that is still pending.
    // The source identity ends with `/agents` so the bare suffix `agents` resolves it.
    let ev2 = sb.mind(&["upgrade", "--yes", "agents#*"]);
    assert!(ev2.success, "{}", ev2.stderr);
    assert!(
        ev2.stdout.contains("upgraded agent:dev"),
        "agents#* must upgrade the pending agent: {}",
        ev2.stdout
    );
}

#[test]
fn upgrade_glob_no_match_is_not_an_error() {
    // spec: CLI-65 -- a glob that matches no installed item reports up-to-date
    // and exits 0; it is NOT an error (unlike `forget`'s no-match glob).
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    // A glob that matches nothing pending: exits 0, reports nothing to do.
    let ev = sb.mind(&["upgrade", "--yes", "xyz*"]);
    assert!(ev.success, "no-match glob must exit 0: {}", ev.stderr);
    // The item is still pending (unchanged by the no-match glob).
    let pending = sb.mind(&["upgrade"]);
    assert!(
        pending.stdout.contains("skill:review"),
        "item must remain pending: {}",
        pending.stdout
    );
}

#[test]
fn upgrade_namespaced_glob_upgrades_namespace() {
    // spec: CLI-65 -- `upgrade 'jk:*'` upgrades only the items in that namespace.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Meld with an alias, learn all items.
    assert!(sb.mind(&["meld", &spec, "--as", "jk"]).success);
    assert!(sb.mind(&["learn", "jk:review"]).success);
    assert!(sb.mind(&["learn", "jk:dev"]).success);

    // Edit both upstream and sync.
    sb.edit_source();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // Glob over the whole namespace upgrades all items under it.
    let ev = sb.mind(&["upgrade", "--yes", "jk:*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:jk:review"),
        "jk:* must upgrade jk:review: {}",
        ev.stdout
    );
    assert!(
        ev.stdout.contains("upgraded agent:jk:dev"),
        "jk:* must upgrade jk:dev: {}",
        ev.stdout
    );
}

#[test]
fn upgrade_exact_ref_still_works_after_glob_change() {
    // spec: CLI-65 (regression) -- non-glob exact refs continue to work exactly
    // as before.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "dev"]).success);

    sb.edit_source();
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: Implements a spec with tests\n---\n# dev agent\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);

    // An exact name ref upgrades only that item, not the glob-matched items.
    let ev = sb.mind(&["upgrade", "--yes", "review"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:review"),
        "exact ref must upgrade the item: {}",
        ev.stdout
    );
    assert!(
        !ev.stdout.contains("agent:dev"),
        "exact ref must not touch other items: {}",
        ev.stdout
    );

    // dev is still pending.
    let rest = sb.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("agent:dev"),
        "dev must remain pending: {}",
        rest.stdout
    );
}

#[test]
fn upgrade_source_glob_isolates_to_named_source() {
    // spec: CLI-65 -- `<source>#*` composes the source qualifier with the glob and
    // must upgrade ONLY that source's pending items, leaving another melded
    // source's pending items untouched in the same pass. The existing glob tests
    // meld a single source; this proves the source qualifier actually isolates
    // when two sources both have pending items.
    let agents = melded(); // source `agents`, carries skill:review
    assert!(agents.mind(&["learn", "review"]).success);

    // A second, independent source with a uniquely-named skill.
    let tools = Sandbox::bare("tools");
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\n",
    );
    assert!(
        agents.mind(&["meld", &tools.source_spec()]).success,
        "meld of the second source failed"
    );
    assert!(agents.mind(&["learn", "deploy"]).success);

    // Drift an item in EACH source, then sync so both are pending.
    agents.edit_source(); // touches skills/review in the agents source
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\nedited\n",
    );
    assert!(agents.mind(&["sync"]).success);

    // Source-scoped glob for the tools source only.
    let ev = agents.mind(&["upgrade", "--yes", "tools#*"]);
    assert!(ev.success, "{}", ev.stderr);
    assert!(
        ev.stdout.contains("upgraded skill:deploy"),
        "tools#* must upgrade the tools source item: {}",
        ev.stdout
    );
    assert!(
        !ev.stdout.contains("skill:review"),
        "tools#* must NOT touch the other source's pending item: {}",
        ev.stdout
    );

    // The agents source item is still pending (unchanged by the scoped glob).
    let rest = agents.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("skill:review"),
        "the other source's item must remain pending: {}",
        rest.stdout
    );
    assert!(
        !rest.stdout.contains("skill:deploy"),
        "the tools source item was already upgraded: {}",
        rest.stdout
    );
}

#[test]
fn upgrade_exact_ref_no_match_is_up_to_date_not_error() {
    // spec: CLI-63, CLI-64 -- an EXACT (non-glob) ref that matches no installed
    // item reports up to date and exits 0 (like the glob no-match, CLI-65), rather
    // than erroring; and it leaves a genuinely-pending item untouched. This guards
    // the exact-ref path that the glob refactor left in place.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source(); // review is now pending
    assert!(sb.mind(&["sync"]).success);

    // An exact name that matches no installed item: not an error, nothing applied.
    let ev = sb.mind(&["upgrade", "--yes", "nonexistent"]);
    assert!(
        ev.success,
        "exact no-match ref must exit 0, not error: {} {}",
        ev.stdout, ev.stderr
    );
    assert!(
        !ev.stdout.contains("upgraded skill:review"),
        "a non-matching exact ref must not upgrade the pending item: {}",
        ev.stdout
    );

    // review is still pending (the no-match filter excluded it).
    let rest = sb.mind(&["upgrade"]);
    assert!(
        rest.stdout.contains("skill:review"),
        "the pending item must be untouched by the no-match ref: {}",
        rest.stdout
    );
}

#[test]
fn json_upgrade_glob_outcomes() {
    // spec: CLI-65, CLI-153 -- under --json a glob upgrade emits the standard
    // mutation object: a glob that matches a pending item yields outcome
    // "upgraded" with the installed keys; a glob that matches nothing yields
    // "up-to-date" (NOT an error) even while a non-matching item is still pending.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    sb.edit_source(); // review pending
    assert!(sb.mind(&["sync"]).success);

    // A glob matching nothing: outcome up-to-date, single object, no prose.
    let none = sb.mind(&["upgrade", "--yes", "--json", "zzz*"]);
    assert!(
        none.success,
        "no-match glob under --json failed: {}",
        none.stderr
    );
    let v = parse_json(&none.stdout);
    assert_eq!(v["action"], "upgrade", "{}", none.stdout);
    assert_eq!(
        v["outcome"], "up-to-date",
        "a no-match glob must report up-to-date under --json: {}",
        none.stdout
    );
    assert!(
        !none.stdout.contains("up to date"),
        "no prose under --json: {}",
        none.stdout
    );

    // review is still pending; a matching glob now upgrades it.
    let some = sb.mind(&["upgrade", "--yes", "--json", "skill:*"]);
    assert!(
        some.success,
        "matching glob under --json failed: {}",
        some.stderr
    );
    let v = parse_json(&some.stdout);
    assert_eq!(v["action"], "upgrade", "{}", some.stdout);
    assert_eq!(
        v["outcome"], "upgraded",
        "a matching glob must report upgraded under --json: {}",
        some.stdout
    );
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        some.stdout
    );
    assert!(
        !some.stdout.contains("upgraded skill"),
        "no prose under --json: {}",
        some.stdout
    );
}

#[test]
fn upgrade_names_an_unscannable_source_instead_of_only_up_to_date() {
    // spec: CLI-211 -- when a source cannot be scanned and no other item has a
    // pending upgrade, `upgrade` must not silently report "everything is up
    // to date": it must name the source that could not be checked.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    let broken = Sandbox::bare("broken-src-2");
    broken.write_and_commit(
        "skills/x/SKILL.md",
        "---\nname: x\ndescription: x skill\n---\n# x\n",
    );
    assert!(sb.mind(&["meld", &broken.source_spec()]).success);
    std::fs::remove_dir_all(&broken.source).unwrap();

    let r = sb.mind(&["upgrade", "--yes"]);
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        !r.stdout
            .trim()
            .eq_ignore_ascii_case("everything is up to date"),
        "must not silently report only up to date: stdout={}",
        r.stdout
    );
    assert!(
        combined.contains("broken-src-2"),
        "must name the unscannable source: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
    assert!(
        combined.contains("sync") || combined.contains("introspect"),
        "must point at a way to diagnose the source: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn glob_scoped_upgrade_does_not_rerun_unrelated_source_hook() {
    // spec: CLI-65, HOOK-11
    // The hook_scope filter is computed via installed_matches_glob, so a GLOB ref
    // must scope install-hook re-runs the same way an exact ref does: a source
    // qualifier glob (`tools#*`) targeting one source must NOT re-run an unrelated
    // hooked source's install hook, even though that source's commit advanced.
    // This exercises the glob branch of the hook_scope computation, distinct from
    // the exact-ref path covered by scoped_upgrade_does_not_rerun_unrelated_source_hook.
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

    // Install an item from the hook-free tools source only.
    let learn = agents.mind(&["learn", "tools#skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    // Clear the meld-time marker so any re-run is observable.
    let marker = agents.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the hook should have run on the initial meld"
    );
    std::fs::remove_file(&marker).unwrap();

    // Advance the hooked source so an UNSCOPED upgrade would re-run its hook.
    agents.edit_source();
    assert!(agents.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // A SOURCE-GLOB scoped upgrade of the OTHER source: the hooked agents source
    // is out of scope, so its hook must NOT re-run.
    let scoped = agents.mind(&[
        "upgrade",
        "tools#*",
        "-y",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        scoped.success,
        "glob-scoped upgrade failed: {} {}",
        scoped.stdout, scoped.stderr
    );
    assert!(
        !marker.exists(),
        "a glob-scoped upgrade of an unrelated source must not re-run the hooked source's hook: {} exists",
        marker.display()
    );

    // Positive control: an unscoped upgrade DOES re-run the hooked source's hook.
    let unscoped = agents.mind(&["upgrade", "-y", "--accept-hooks=install,update,uninstall"]);
    assert!(
        unscoped.success,
        "unscoped upgrade failed: {} {}",
        unscoped.stdout, unscoped.stderr
    );
    assert!(
        marker.exists(),
        "an unscoped upgrade must re-run the hooked source's hook: {} missing",
        marker.display()
    );
}

#[test]
fn upgrade_takes_a_plain_source_when_no_installed_item_matches() {
    // spec: CLI-262
    let (sb, tools) = melded_two_sources();
    assert!(sb.mind(&["learn", "review"]).success);
    assert!(sb.mind(&["learn", "deploy"]).success);
    sb.edit_source();
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\nedited\n",
    );

    // `tools` names no installed item, so it scopes the pass to that source.
    let r = sb.mind(&["upgrade", "tools", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("upgraded skill:deploy"), "{}", r.stdout);
    assert!(!r.stdout.contains("upgraded skill:review"), "{}", r.stdout);

    // An item ref keeps its meaning (CLI-63).
    let r = sb.mind(&["upgrade", "review", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("upgraded skill:review"), "{}", r.stdout);

    // With `--source` given, the positional is always an item filter: `tools`
    // names no installed item, so nothing is upgraded.
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\nagain\n",
    );
    let r = sb.mind(&["upgrade", "tools", "--source", "tools", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("upgraded skill:deploy"), "{}", r.stdout);

    // A ref matching neither is still up to date, exit 0 (CLI-64).
    let r = sb.mind(&["upgrade", "nothing-here"]);
    assert!(r.success, "{}", r.stderr);
    assert!(
        r.stdout.contains("everything is up to date"),
        "{}",
        r.stdout
    );
}

#[test]
fn upgrade_prefers_an_installed_item_over_a_source_of_the_same_name() {
    // spec: CLI-262
    let (sb, tools) = melded_two_sources();
    // A skill named like the `tools` source, installed from `agents`.
    sb.write_and_commit(
        "skills/tools/SKILL.md",
        "---\nname: tools\ndescription: t\n---\n# tools skill\n",
    );
    assert!(sb.mind(&["sync"]).success);
    assert!(sb.mind(&["learn", "skill:tools"]).success);
    assert!(sb.mind(&["learn", "deploy"]).success);
    tools.write_and_commit(
        "skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Ship the build\n---\n# deploy skill\nedited\n",
    );

    let r = sb.mind(&["upgrade", "tools", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("also names a melded source"),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("mind upgrade --source 'tools'"),
        "{}",
        r.stderr
    );
    assert!(!r.stdout.contains("upgraded skill:deploy"), "{}", r.stdout);

    // A glob is an item filter as written: no note, even though it also
    // matches the source.
    let r = sb.mind(&["upgrade", "*", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("also names a melded source"),
        "{}",
        r.stderr
    );
}
