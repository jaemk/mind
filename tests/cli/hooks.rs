//! Lifecycle hooks and `hooks run`/`hooks list` (spec/install-hooks.md, spec/cli.md).

use super::*;

#[test]
fn meld_of_a_source_declaring_no_hooks_runs_and_discloses_none() {
    // spec: HOOK-3 - a source with no `[source].install` and no `--install-hook`
    // must run no hook and show no hook disclosure or consent prompt. The
    // fixture ships no `mind.toml` at all, which is the common case: melding an
    // arbitrary repo must never be an arbitrary-code-execution decision unless
    // the source or the consumer explicitly asked for one.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["meld", &spec, "--register-only"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let out = format!("{}{}", r.stdout, r.stderr);
    for marker in ["hook", "Hook", "would run", "run this command"] {
        assert!(
            !out.contains(marker),
            "a source declaring no hooks must not mention `{marker}`: {out}"
        );
    }
}

#[test]
fn example_hooks_lists_declared_hooks() {
    // spec: HOOK-50, HOOK-54
    // The hooks example declares source install and uninstall hooks; `review`
    // discloses each one, so a consumer sees the source will run code.
    let sb = Sandbox::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/hooks");
    let r = sb.mind(&["review", dir.to_str().unwrap()]);
    assert!(r.success, "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    let out = format!("{}{}", r.stdout, r.stderr);
    assert!(
        out.contains("install hook"),
        "discloses an install hook: {out}"
    );
    assert!(
        out.contains("uninstall hook"),
        "discloses the uninstall hook: {out}"
    );
}

#[test]
fn example_explicit_item_hooks_fire() {
    // spec: HOOK-81, HOOK-82
    // The explicit example's `scan` skill declares per-item install/uninstall
    // hooks whose scripts ship under components/scan/hooks/. On a non-TTY they
    // are skipped unless `--accept-hooks=install,update,uninstall` is passed, which
    // runs them. The install hook fires at learn (HOOK-81) and the uninstall hook
    // fires at forget (HOOK-82); each prints a recognizable line.
    let sb = Sandbox::from_example("explicit");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let learn = sb.mind(&["learn", "scan", "--accept-hooks=install,update,uninstall"]);
    assert!(learn.success, "{} {}", learn.stdout, learn.stderr);
    assert!(
        learn.stdout.contains("explicit-example: scan installed"),
        "the install hook must fire at learn (HOOK-81): {}",
        learn.stdout
    );

    let forget = sb.mind(&["forget", "scan", "--accept-hooks=install,update,uninstall"]);
    assert!(forget.success, "{} {}", forget.stdout, forget.stderr);
    assert!(
        forget.stdout.contains("explicit-example: scan removed"),
        "the uninstall hook must fire at forget (HOOK-82): {}",
        forget.stdout
    );
}

#[test]
fn meld_with_declared_hook_non_tty_skips_but_still_installs() {
    // spec: HOOK-22, HOOK-21, HOOK-55
    // stdin is not a TTY in this harness, so a declared hook takes the skip
    // path (HOOK-22): the source and its items still install (HOOK-21), but the
    // tooling is not built. The clone-dir marker the hook would create must not
    // appear, the run is reported as skipped, and the registry records the hook
    // command with a NULL install_hook_commit (never ran).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "meld should still succeed: {}", r.stderr);

    // The source is registered (HOOK-21: skipping still installs the source).
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("agents"),
        "source must be registered after a skipped hook: {}",
        sources.stdout
    );

    // The items are still discoverable / learnable (the tooling, not the items,
    // is what the skip drops).
    assert!(
        sb.mind(&["learn", "review"]).success,
        "items must install even when the hook is skipped"
    );

    // The hook did NOT run: its marker is absent from the clone dir.
    let marker = sb.source.clone().join("hookran");
    assert!(
        !marker.exists(),
        "the install hook must not have run: {} exists",
        marker.display()
    );

    // The skip is reported to the user with the exact note `meld_recursive`
    // prints on the HOOK-22 skip path (commands/); the source name in the
    // middle is the full `host/owner/repo` identity, so assert the two stable
    // fragments around it. A regression that drops or rewords the note fails
    // here rather than passing on any bare "skipped".
    let prefix = "note: skipped install hook ";
    let suffix = "; its items may not work until it runs";
    let reported = (r.stdout.contains(prefix) && r.stdout.contains(suffix))
        || (r.stderr.contains(prefix) && r.stderr.contains(suffix));
    assert!(
        reported,
        "the skip must be reported with the exact note: {} {}",
        r.stdout, r.stderr
    );

    // The registry records the hook in `install_hooks` with a null `ran_at`
    // (skipped, so `upgrade` can re-offer it) per HOOK-55.
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the hook command: {json}"
    );
    assert!(
        json.contains("install_hooks") && json.contains("\"ran_at\": null"),
        "a skipped hook must record in install_hooks with ran_at = null: {json}"
    );
}

#[test]
fn meld_accept_hooks_runs_hook_and_records_it() {
    // spec: HOOK-23, HOOK-10, HOOK-31, HOOK-55
    // --accept-hooks=install,update,uninstall runs the hook without prompting
    // (HOOK-23). It runs in the clone after checkout (HOOK-10), so its marker
    // lands in the clone dir, and the registry records both the command and the
    // commit it ran at (HOOK-31).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(r.success, "meld should succeed: {}", r.stderr);

    // HOOK-10: the hook ran in the clone dir.
    let marker = sb.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the install hook must have run in the clone: {} missing",
        marker.display()
    );

    // HOOK-31/HOOK-55: the registry records the command in `install_hooks` with a
    // non-null `ran_at` (the commit it ran at).
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the hook command: {json}"
    );
    assert!(
        json.contains("install_hooks") && !json.contains("\"ran_at\": null"),
        "a hook that ran must record a non-null ran_at in install_hooks: {json}"
    );
}

#[test]
fn meld_hook_nonzero_exit_fails_and_registers_nothing() {
    // spec: HOOK-30
    // A non-zero hook exit is a HookFailed error that fails the meld: the source
    // is not registered and the clone is removed, as for any failed meld.
    let sb = sandbox_with_declared_hook("agents", "exit 1");
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(
        !r.success,
        "a non-zero hook exit must fail the meld: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("install hook") && r.stderr.contains("failed"),
        "stderr must report the failed install hook: {}",
        r.stderr
    );

    // Nothing registered.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("no sources melded"),
        "no source must be registered after a failed hook: {}",
        sources.stdout
    );
    let sources_json = sb.mind_home.join("sources.json");
    if sources_json.exists() {
        let json = std::fs::read_to_string(&sources_json).unwrap();
        assert!(
            !json.contains("\"repo\": \"agents\""),
            "sources.json must not list the source after a failed hook: {json}"
        );
    }

    // The source is a linked local working tree (CLI-27), so a failed hook must
    // NOT delete it -- it is the user's directory, not a clone we own.
    assert!(
        sb.source.exists(),
        "a failed hook must not delete a linked source's working tree at {}",
        sb.source.display()
    );
}

#[test]
fn meld_install_hook_flag_supplies_hook_without_mind_toml() {
    // spec: HOOK-2
    // --install-hook supplies a hook for a repo that ships no mind.toml. With
    // --accept-hooks=install,update,uninstall it runs, and the registry records
    // the supplied command and a non-null run-commit.
    let sb = Sandbox::new(); // no mind.toml
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--install-hook",
        "touch hookran",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "meld should succeed: {}", r.stderr);

    let marker = sb.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the supplied hook must have run: {} missing",
        marker.display()
    );

    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("touch hookran"),
        "registry must record the supplied hook command: {json}"
    );
    assert!(
        !json.contains("\"install_hook_commit\": null"),
        "install_hook_commit must be non-null after the supplied hook ran: {json}"
    );
}

#[test]
fn recall_sources_shows_install_hook_marker() {
    // spec: HOOK-31
    // recall --sources reports that a source carries an install hook.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"])
            .success
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(sources.success, "recall failed: {}", sources.stderr);
    // The marker is the ` hook` token inside the bracketed commit/alias column
    // (commands/recall.rs `recall`), e.g. `[<commit> hook]`. Assert the exact bracketed
    // token so a regression that drops the marker (or renames the column) fails.
    assert!(
        sources.stdout.contains(" hook]"),
        "recall --sources must mark a source with the bracketed ` hook]` token: {}",
        sources.stdout
    );
}

#[test]
fn upgrade_reruns_hook_after_source_advances() {
    // spec: HOOK-11
    // After a source advances to a new commit, upgrade re-runs the hook (the
    // tooling tracks the source). When the source has not advanced, upgrade does
    // not re-run the hook (the recorded run-commit already equals the commit).
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"])
            .success,
        "initial meld should run the hook and record commit C1"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");

    // Clear the marker so a re-run is observable.
    std::fs::remove_file(&marker).unwrap();

    // Advance the source and sync (sync alone must not run the hook).
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);
    assert!(
        !marker.exists(),
        "sync alone must not re-run the hook (HOOK-11)"
    );

    // upgrade sees the new commit and re-runs the hook.
    let ev = sb.mind(&["upgrade", "-y", "--accept-hooks=install,update,uninstall"]);
    assert!(ev.success, "upgrade failed: {} {}", ev.stdout, ev.stderr);
    assert!(
        marker.exists(),
        "upgrade must re-run the hook after the source advanced: {} missing",
        marker.display()
    );

    // The recorded run-commit advanced to the new commit; a second upgrade with
    // no source change must NOT re-run the hook.
    std::fs::remove_file(&marker).unwrap();
    let again = sb.mind(&["upgrade", "-y", "--accept-hooks=install,update,uninstall"]);
    assert!(again.success, "second upgrade failed: {}", again.stderr);
    assert!(
        !marker.exists(),
        "upgrade must not re-run the hook when the source has not advanced"
    );
}

#[test]
fn sync_upgrade_runs_hook_rerun_only_with_the_skip_check_flag() {
    // spec: HOOK-11, HOOK-23
    // `sync --upgrade` drives an upgrade pass, so it must honor the same hook
    // re-run rules. In a non-TTY context the re-run is skipped (HOOK-22), and
    // `--accept-hooks=install,update,uninstall` threaded through `sync` is what
    // runs it unattended (HOOK-23) -- the CI workflow the flag exists for.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"])
            .success,
        "initial meld should run the hook and record commit C1"
    );

    let marker = sb.source.clone().join("hookran");
    assert!(marker.exists(), "the hook should have run on meld");
    std::fs::remove_file(&marker).unwrap();

    // Advance the source so a re-run is warranted (the recorded run-commit now
    // lags the source's commit).
    sb.edit_source();

    // `sync --upgrade` with no flag: sync advances the commit, the upgrade pass
    // sees the new commit but takes the non-TTY skip path, so the hook does not
    // re-run.
    let no_flag = sb.mind(&["sync", "--upgrade"]);
    assert!(
        no_flag.success,
        "sync --upgrade failed: {} {}",
        no_flag.stdout, no_flag.stderr
    );
    assert!(
        !marker.exists(),
        "sync --upgrade without the flag must not re-run the hook (HOOK-22)"
    );

    // `sync --upgrade --accept-hooks=install,update,uninstall`: the flag now
    // reaches the upgrade pass, which re-runs the still-warranted hook unattended.
    let with_flag = sb.mind(&[
        "sync",
        "--upgrade",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        with_flag.success,
        "sync --upgrade --accept-hooks=install,update,uninstall failed: {} {}",
        with_flag.stdout, with_flag.stderr
    );
    assert!(
        marker.exists(),
        "sync --upgrade with the flag must re-run the hook unattended: {} missing",
        marker.display()
    );
}

#[test]
fn scoped_upgrade_does_not_rerun_unrelated_source_hook() {
    // spec: HOOK-11
    // A scoped `upgrade <item>` must NOT re-run install hooks (arbitrary code) for
    // sources unrelated to the targeted item. Meld a hooked source (`agents`,
    // recorded via --accept-hooks=install,update,uninstall) plus a second,
    // hook-free source (`tools`); learn an item only from `tools`; advance the
    // hooked source and sync. A scoped upgrade targeting the `tools` item must
    // leave the hooked source's marker untouched, while an UNSCOPED upgrade (the
    // positive control) does re-run it.
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

    // Learn an item from the OTHER source only, source-qualified so it resolves
    // unambiguously across the two sources that share fixture item names.
    let learn = agents.mind(&["learn", "tools#skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    // The hook ran on meld; clear its marker so any re-run is observable.
    let marker = agents.source.clone().join("hookran");
    assert!(
        marker.exists(),
        "the hook should have run on the initial meld"
    );
    std::fs::remove_file(&marker).unwrap();

    // Advance the hooked source so its commit moves past the recorded run-commit,
    // i.e. an UNSCOPED upgrade would re-run its hook. sync alone must not.
    agents.edit_source();
    assert!(agents.mind(&["sync"]).success, "sync failed");
    assert!(!marker.exists(), "sync alone must not re-run the hook");

    // Scoped upgrade targeting the OTHER source's item: the hooked source is out
    // of scope, so its hook must NOT re-run even though its commit advanced.
    let scoped = agents.mind(&[
        "upgrade",
        "tools#skill:review",
        "-y",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        scoped.success,
        "scoped upgrade failed: {} {}",
        scoped.stdout, scoped.stderr
    );
    assert!(
        !marker.exists(),
        "a scoped upgrade of an unrelated item must not re-run the hooked source's hook: {} exists",
        marker.display()
    );

    // Positive control: an UNSCOPED upgrade DOES re-run the hooked source's hook.
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
fn remeld_reoffers_pending_install_hooks_and_force_reruns() {
    // spec: HOOK-60
    let sb = Sandbox::bare("remeld-hook");
    let marker = sb.base.join("hook-ran");
    let m = marker.to_str().unwrap().to_owned();
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"install\"\n"),
    );
    let spec = sb.source_spec();

    // A fresh non-TTY meld registers but skips the hook (HOOK-22).
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(!marker.exists(), "hook skipped on the non-TTY meld");

    // Re-melding re-offers the pending (skipped) hook; the dangerous flag runs it.
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(marker.exists(), "re-meld must run the pending hook");

    // Now recorded as run at this commit: a plain re-meld does not re-run it.
    std::fs::remove_file(&marker).unwrap();
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(
        !marker.exists(),
        "a hook already run at this commit is not re-offered"
    );

    // --force re-offers (and re-runs) every hook regardless.
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--link-only",
            "--force",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(marker.exists(), "--force must re-run an already-run hook");
}

#[test]
fn install_hook_stdout_streams_to_mind_stdout() {
    // spec: HOOK-30 - a hook's streams are inherited, so its stdout reaches
    // mind's stdout as it is produced, inside one frame around the whole run.
    let sb = Sandbox::bare("hook-output");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nrun = \"echo HELLO-FROM-HOOK\"\nname = \"build\"\nevent = \"install\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--link-only",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("====== (hook: build) ======"),
        "the opening frame must appear in mind's output: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("HELLO-FROM-HOOK"),
        "the hook's stdout must reach mind's stdout: {}",
        r.stdout
    );
    // The frame is one pair around the whole run: there is no separate
    // per-stream block any more, because the streams are never separated.
    assert!(
        !r.stdout.contains("hook-stdout:") && !r.stdout.contains("hook-stderr:"),
        "the per-stream capture frames are gone: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("====== (end hook: build, exit 0, "),
        "the closing divider must separate the hook output from what follows: {}",
        r.stdout
    );
}

#[test]
fn install_hook_stderr_goes_to_mind_stderr_not_stdout() {
    // spec: HOOK-30 - with the streams inherited, a hook's stderr goes to MIND's
    // stderr rather than being captured and mirrored into stdout. That is the
    // point of streaming (each stream keeps its identity, so a caller's
    // redirection means what it says), and it is the visible behavior change:
    // the old form replayed captured stderr onto stdout.
    let sb = Sandbox::bare("hook-stderr");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nrun = \"echo OOPS 1>&2\"\nname = \"warn\"\nevent = \"install\"\n",
    );
    let spec = sb.source_spec();
    let r = sb.mind(&[
        "meld",
        &spec,
        "--link-only",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("====== (hook: warn) ======"),
        "the frame still opens on stdout, where mind's own output is: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("OOPS"),
        "the hook's stderr must reach mind's stderr: {}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("OOPS"),
        "and must NOT be copied onto stdout as the captured form did: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("====== (end hook: warn, exit 0, "),
        "the closing divider must separate the hook output from what follows: {}",
        r.stdout
    );
}

#[test]
fn meld_runs_multiple_install_hooks_with_dangerous_flag() {
    // spec: HOOK-50
    // A source with two [[hooks]] entries (both event = "install") runs both
    // hooks in declaration order when --accept-hooks=install,update,uninstall is
    // given. Both marker files must exist after the meld succeeds.
    let sb = Sandbox::bare("multi-hook");
    let marker1 = sb.base.join("marker1");
    let marker2 = sb.base.join("marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--accept-hooks=install,update,uninstall",
        "--link-only",
    ]);
    assert!(
        r.success,
        "meld with two install hooks should succeed: {} {}",
        r.stdout, r.stderr
    );

    assert!(
        marker1.exists(),
        "first install hook must have run (marker1 missing): {}",
        marker1.display()
    );
    assert!(
        marker2.exists(),
        "second install hook must have run (marker2 missing): {}",
        marker2.display()
    );

    // Source is registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("multi-hook"),
        "source must be registered after both hooks ran: {sources}"
    );
}

#[test]
fn meld_non_tty_skips_install_hooks_and_still_registers_source() {
    // spec: HOOK-22 (preserved with multi-hook)
    // Without --accept-hooks=install,update,uninstall, a non-TTY meld skips all
    // hooks, prints a skip note, and still registers the source.
    let sb = Sandbox::bare("multi-hook-skip");
    let marker1 = sb.base.join("skip-marker1");
    let marker2 = sb.base.join("skip-marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Default meld: non-TTY, no dangerous flag.
    let r = sb.mind(&["meld", &spec]);
    assert!(
        r.success,
        "meld should still succeed on non-TTY skip: {} {}",
        r.stdout, r.stderr
    );

    // Neither hook ran.
    assert!(
        !marker1.exists(),
        "hook must not have run in non-TTY mode (marker1 exists)"
    );
    assert!(
        !marker2.exists(),
        "hook must not have run in non-TTY mode (marker2 exists)"
    );

    // Skip note is printed with the exact prefix that `run_install_hooks` emits
    // on the HOOK-22 skip path. Asserting the literal prefix ensures the message
    // is present and not just any word "skipped" in unrelated output.
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert!(
        combined.contains("note: skipped install hook "),
        "non-TTY skip must print a note starting with 'note: skipped install hook ': {combined}"
    );

    // Source is still registered (HOOK-22: skip-and-continue registers the source).
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("multi-hook-skip"),
        "source must be registered even when hooks are skipped: {sources}"
    );
}

#[test]
fn optional_install_hook_failure_aborts_meld() {
    // spec: HOOK-53
    // An optional hook's non-zero exit is a hard stop, like a required hook: the
    // meld fails and the source is not registered. `optional` only governs whether
    // the user may decline to run it, never whether it may fail.
    let sb = Sandbox::bare("optional-hook-fail");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"install\"\noptional = true\n";
    sb.write_and_commit("mind.toml", toml);
    let spec = sb.source_spec();

    let r = sb.mind(&[
        "meld",
        &spec,
        "--accept-hooks=install,update,uninstall",
        "--link-only",
    ]);
    assert!(
        !r.success,
        "an optional hook failure must abort the meld: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !sb.mind(&["recall", "--sources"])
            .stdout
            .contains("optional-hook-fail"),
        "the source must not be registered after a failed optional hook"
    );
}

#[test]
fn required_install_hook_failure_aborts_meld() {
    // spec: HOOK-53
    // A required install hook that exits non-zero fails the meld entirely: the
    // source is NOT registered and the command exits with a non-zero status.
    let sb = Sandbox::bare("required-fail");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"install\"\n";
    sb.write_and_commit("mind.toml", toml);
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--accept-hooks=install,update,uninstall"]);
    assert!(
        !r.success,
        "meld must fail when a required install hook exits non-zero: {} {}",
        r.stdout, r.stderr
    );

    // Source is NOT registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("required-fail"),
        "source must not be registered after a required hook failure: {sources}"
    );
}

#[test]
fn unmeld_runs_uninstall_hook_with_dangerous_flag() {
    // spec: HOOK-54
    // A source with an event = "uninstall" hook: after meld, `unmeld --accept-hooks=install,update,uninstall`
    // runs the hook and removes the source from the registry.
    let sb = Sandbox::bare("uninstall-hook");
    let uninstall_marker = sb.base.join("uninstall-ran");
    let m = uninstall_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Meld first (no uninstall hooks run at meld time).
    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        !uninstall_marker.exists(),
        "uninstall hook must not run at meld time"
    );

    // unmeld with dangerous flag: uninstall hook runs, source removed.
    let unmeld = sb.mind(&[
        "unmeld",
        "uninstall-hook",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        unmeld.success,
        "unmeld should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );

    assert!(
        uninstall_marker.exists(),
        "uninstall hook must have run at unmeld: marker missing at {}",
        uninstall_marker.display()
    );

    // Source is no longer registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("uninstall-hook"),
        "source must be removed after unmeld: {sources}"
    );
}

#[test]
fn unmeld_uninstall_hook_override_replaces_declared() {
    // spec: HOOK-59
    // `unmeld --uninstall-hook <cmd>` replaces the source's declared uninstall
    // hook: the override command runs, the declared one does not.
    let sb = Sandbox::bare("uninstall-override");
    let declared_marker = sb.base.join("declared-ran");
    let override_marker = sb.base.join("override-ran");
    let dm = declared_marker.to_str().unwrap().to_owned();
    let om = override_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {dm}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success, "meld");

    let unmeld = sb.mind(&[
        "unmeld",
        "uninstall-override",
        "--yes",
        "--uninstall-hook",
        &format!("touch {om}"),
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        unmeld.success,
        "unmeld --uninstall-hook should succeed: {} {}",
        unmeld.stdout, unmeld.stderr
    );
    assert!(
        override_marker.exists(),
        "the override uninstall hook must run: {}",
        override_marker.display()
    );
    assert!(
        !declared_marker.exists(),
        "the declared uninstall hook must not run when overridden"
    );
    assert!(
        !sb.mind(&["recall", "--sources"])
            .stdout
            .contains("uninstall-override"),
        "source must be removed"
    );
}

#[test]
fn unmeld_non_tty_skips_uninstall_hook_but_still_removes_source() {
    // spec: HOOK-54 (non-TTY path)
    // A plain non-TTY `unmeld` (no dangerous flag) skips the uninstall hook
    // (no marker) but still removes the source from the registry.
    let sb = Sandbox::bare("uninstall-skip");
    let uninstall_marker = sb.base.join("uninstall-skip-ran");
    let m = uninstall_marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"uninstall\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );

    // Unmeld without the dangerous flag: non-TTY -> skip hook, still remove
    // source. `--yes` confirms the removal itself (CLI-259).
    let unmeld = sb.mind(&["unmeld", "uninstall-skip", "--yes"]);
    assert!(
        unmeld.success,
        "unmeld should succeed even when hook is skipped: {} {}",
        unmeld.stdout, unmeld.stderr
    );

    // Hook did NOT run.
    assert!(
        !uninstall_marker.exists(),
        "uninstall hook must not run in non-TTY mode without the dangerous flag"
    );

    // Source is still removed (skip-and-continue).
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources.contains("uninstall-skip"),
        "source must be removed even when uninstall hook is skipped: {sources}"
    );
}

#[test]
fn init_source_scaffold_offers_hook_examples() {
    // spec: HOOK-57
    // `mind init-source <dir>` on a fresh repo dir writes a mind.toml scaffold
    // whose content contains commented [[hooks]] examples for both install and
    // uninstall events, including optional = true.
    let sb = Sandbox::new();
    let repo = sb.base.join("new-source");
    // Write a minimal item so init-source has something to discover.
    write(
        &repo.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: A greeting skill\n---\n# greet\n",
    );
    let dir = repo.to_str().unwrap();

    let r = sb.mind(&["init-source", dir]);
    assert!(
        r.success,
        "init-source should succeed: {} {}",
        r.stdout, r.stderr
    );

    let scaffold =
        std::fs::read_to_string(repo.join("mind.toml")).expect("init-source must create mind.toml");

    // The scaffold must contain commented [[hooks]] entries.
    assert!(
        scaffold.contains("[[hooks]]"),
        "scaffold must contain [[hooks]] examples: {scaffold}"
    );

    // Must have both install and uninstall event examples, on comment lines.
    let has_install_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("event") && l.contains("install"));
    assert!(
        has_install_comment,
        "scaffold must have a commented event = \"install\" line: {scaffold}"
    );

    let has_uninstall_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("event") && l.contains("uninstall"));
    assert!(
        has_uninstall_comment,
        "scaffold must have a commented event = \"uninstall\" line: {scaffold}"
    );

    // Must have optional = true on a comment line.
    let has_optional_comment = scaffold
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.contains("optional") && l.contains("true"));
    assert!(
        has_optional_comment,
        "scaffold must have a commented optional = true line: {scaffold}"
    );
}

#[test]
fn recall_sources_marks_multi_hook_source() {
    // spec: HOOK-58
    // After a multi-hook meld (with --accept-hooks=install,update,uninstall so the
    // hooks are recorded), `recall --sources` contains a `hook` token indicating
    // the source has hooks.
    let sb = Sandbox::bare("hook-report");
    let marker1 = sb.base.join("report-marker1");
    let marker2 = sb.base.join("report-marker2");
    let m1 = marker1.to_str().unwrap().to_owned();
    let m2 = marker2.to_str().unwrap().to_owned();
    let toml = format!(
        "[[hooks]]\nrun = \"touch {m1}\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"touch {m2}\"\nevent = \"install\"\n"
    );
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--accept-hooks=install,update,uninstall",
            "--link-only"
        ])
        .success,
        "meld should succeed"
    );

    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.success,
        "recall --sources failed: {}",
        sources.stderr
    );

    // The output must contain the count-aware ` hooks(2)` token (HOOK-58:
    // N > 1 renders as ` hooks(N)`) for the two declared install hooks.
    // This assertion would fail if the token were dropped or rendered differently.
    assert!(
        sources.stdout.contains(" hooks(2)"),
        "recall --sources must mark a two-hook source with ' hooks(2)': {}",
        sources.stdout
    );
}

#[test]
fn upgrade_pending_filter_treats_none_ran_at_as_always_pending() {
    // spec: HOOK-55, HOOK-11
    // A hook recorded with ran_at=None (skipped at meld time) must be re-offered
    // by `upgrade` even when the source's commit is also None (a commitless linked
    // source). The predicate `ran_at.is_none() || ran_at != commit` ensures this.
    //
    // The test melds a local source declaring a hook (non-TTY meld skips it,
    // recording ran_at=null), then runs `upgrade --accept-hooks=install,update,uninstall`.
    // The hook must re-run (marker appears) proving the none-pending filter works.
    let sb = Sandbox::bare("upgrade-pending");
    let marker = sb.base.join("upgrade-pending-ran");
    let m = marker.to_str().unwrap().to_owned();
    let toml = format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"install\"\n");
    sb.write_and_commit("mind.toml", &toml);
    let spec = sb.source_spec();

    // Meld without the dangerous flag: non-TTY skips the hook, ran_at=null.
    let meld = sb.mind(&["meld", &spec, "--link-only"]);
    assert!(
        meld.success,
        "meld should succeed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        !marker.exists(),
        "hook must not run at meld time (non-TTY skip)"
    );

    // Verify the registry has ran_at=null for the hook.
    let json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        json.contains("\"ran_at\": null"),
        "registry must record ran_at=null for the skipped hook: {json}"
    );

    // Upgrade with the dangerous flag: the skipped (ran_at=null) hook must re-run.
    let upgrade = sb.mind(&["upgrade", "--accept-hooks=install,update,uninstall"]);
    assert!(
        upgrade.success,
        "upgrade should succeed: {} {}",
        upgrade.stdout, upgrade.stderr
    );
    assert!(
        marker.exists(),
        "upgrade must re-run a hook with ran_at=null (none-pending filter): marker absent"
    );
}

#[test]
fn unmeld_failing_uninstall_hook_leaves_source_melded() {
    // spec: HOOK-53, HOOK-54, HOOK-87
    // A source uninstall hook that exits non-zero is a hard stop: the unmeld
    // fails and the source remains registered. Under HOOK-87 the source hook runs
    // AFTER the items are torn down, so the item is already removed when the
    // source hook fails; the source itself stays melded.
    let sb = Sandbox::bare("failing-uninstall-hook");
    let toml = "[[hooks]]\nrun = \"exit 1\"\nevent = \"uninstall\"\n";
    sb.write_and_commit("mind.toml", toml);
    sb.write_and_commit(
        "agents/dev.md",
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let spec = sb.source_spec();

    // Meld and install the item.
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "meld should succeed"
    );
    assert!(sb.mind(&["learn", "agent:dev"]).success, "learn dev");

    // Unmeld with dangerous flag so the hook runs (non-TTY would skip it).
    let r = sb.mind(&[
        "unmeld",
        "failing-uninstall-hook",
        "--yes",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "unmeld must fail when uninstall hook exits non-zero: {} {}",
        r.stdout, r.stderr
    );

    // Source must still be registered.
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("failing-uninstall-hook"),
        "source must remain melded after a failed uninstall hook: {sources}"
    );

    // HOOK-87: teardown reverses install, so the item is removed BEFORE the
    // source uninstall hook runs; by the time that hook fails the item is gone.
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "the item is torn down before the source uninstall hook fires (HOOK-87)"
    );
}

/// A source named `name` (a `bare` repo) with one skill `greet` declared in
/// `mind.toml` `[[items]]` carrying per-item `install` and `uninstall` hooks.
/// The commands are arbitrary; markers under `<base>/markers` let a test observe
/// which fired. The install command also drops a relative `built-here` file so a
/// test can confirm the hook ran with the store dir as its working directory.
fn sandbox_with_item_hook_cmds(name: &str, install: &str, uninstall: &str) -> Sandbox {
    let sb = Sandbox::bare(name);
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"{install}\"\n",
            "uninstall = \"{uninstall}\"\n",
        ),
        install = install,
        uninstall = uninstall,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    sb
}

/// The success-marker variant: the install hook drops `built-here` (relative, in
/// the store dir) plus an absolute `<base>/markers/installed`; the uninstall hook
/// drops an absolute `<base>/markers/uninstalled`.
fn sandbox_with_item_hooks(name: &str) -> Sandbox {
    // Build first so we know the base path, then rewrite the mind.toml commands
    // with absolute marker paths under that base.
    let sb = Sandbox::bare(name);
    let markers = sb.base.join("markers");
    let m = markers.display();
    let install = format!("touch built-here && mkdir -p '{m}' && touch '{m}/installed'");
    let uninstall = format!("mkdir -p '{m}' && touch '{m}/uninstalled'");
    write(
        &sb.source.join("skills/greet/SKILL.md"),
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    let toml = format!(
        concat!(
            "[[items]]\n",
            "kind = \"skill\"\n",
            "name = \"greet\"\n",
            "path = \"skills/greet\"\n",
            "install = \"{install}\"\n",
            "uninstall = \"{uninstall}\"\n",
        ),
        install = install,
        uninstall = uninstall,
    );
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet\n",
    );
    sb.write_and_commit("mind.toml", &toml);
    sb
}

#[test]
fn learn_runs_item_install_hook_in_store_dir() {
    // spec: HOOK-81, HOOK-83
    // An item install hook runs as the final install step, in the item's store
    // directory, when run unattended via --accept-hooks=install,update,uninstall.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    // Register without auto-installing (so the install runs under our flag).
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "learn should succeed: {} {}", r.stdout, r.stderr);

    // The item installed.
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the skill must be installed"
    );
    // HOOK-81: the install hook ran with the store dir as cwd (relative marker).
    assert!(
        sb.mind_home.join("store/skill/greet/built-here").exists(),
        "install hook must run in the item's store directory"
    );
    // And its absolute side effect happened.
    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook's side effect must have run"
    );
}

#[test]
fn learn_without_flag_skips_item_install_hook_in_non_tty() {
    // spec: HOOK-83
    // A non-TTY learn with no flag skips the item install hook: the item still
    // installs, but the side effect does not run, and a note says so.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&["learn", "skill:greet"]);
    assert!(
        r.success,
        "learn should still succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the item must install even though the hook is skipped"
    );
    assert!(
        !sb.base.join("markers/installed").exists(),
        "a non-TTY learn must skip the install hook"
    );
    assert!(
        r.stdout.contains("skipped install hook"),
        "the skip must be reported: {}",
        r.stdout
    );
}

#[test]
fn learn_item_install_hook_failure_rolls_back_the_install() {
    // spec: HOOK-81
    // A non-zero install-hook exit rolls the item's install back: its store copy
    // and link are removed and it is left not installed.
    let sb = sandbox_with_item_hook_cmds("agents", "exit 1", "true");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "a failing install hook must fail learn: {}",
        r.stdout
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the store copy must be removed on rollback"
    );
    assert!(
        !sb.claude_home.join("skills/greet").exists(),
        "the link must be removed on rollback"
    );
    let manifest = std::fs::read_to_string(sb.mind_home.join("manifest.json")).unwrap_or_default();
    assert!(
        !manifest.contains("greet"),
        "a rolled-back item must not be recorded in the manifest: {manifest}"
    );
}

#[test]
fn forget_runs_item_uninstall_hook() {
    // spec: HOOK-82
    // forget runs the item's uninstall hook (in its store dir) before removing it.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );

    let r = sb.mind(&[
        "forget",
        "skill:greet",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        r.success,
        "forget should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "the uninstall hook must run at forget"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item must be removed after its uninstall hook"
    );
}

#[test]
fn forget_without_flag_skips_item_uninstall_hook_in_non_tty() {
    // spec: HOOK-83
    // A non-TTY forget with no flag skips the uninstall hook but still removes the
    // item (cleanup is the graceful decline).
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );

    let r = sb.mind(&["forget", "skill:greet"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !sb.base.join("markers/uninstalled").exists(),
        "a non-TTY forget must skip the uninstall hook"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item is still removed when the hook is skipped"
    );
    assert!(
        r.stdout.contains("skipped uninstall hook"),
        "the skip must be reported: {}",
        r.stdout
    );
}

#[test]
fn forget_item_uninstall_hook_failure_leaves_item_installed() {
    // spec: HOOK-82
    // A non-zero uninstall-hook exit is a hard stop: the removal stops and the
    // item is left installed.
    let sb = sandbox_with_item_hook_cmds("agents", "true", "exit 1");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );

    let r = sb.mind(&[
        "forget",
        "skill:greet",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        !r.success,
        "a failing uninstall hook must fail forget: {}",
        r.stdout
    );
    assert!(
        sb.mind_home.join("store/skill/greet/SKILL.md").exists(),
        "the item must remain installed when its uninstall hook fails"
    );
}

#[test]
fn unmeld_runs_item_uninstall_hook() {
    // spec: HOOK-82
    // unmeld removes the source's items, running each item's uninstall hook first.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );

    let r = sb.mind(&[
        "unmeld",
        "agents",
        "-y",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(
        r.success,
        "unmeld should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "the item uninstall hook must run at unmeld"
    );
    assert!(
        !sb.mind_home.join("store/skill/greet").exists(),
        "the item must be removed at unmeld"
    );
}

#[test]
fn item_install_hook_reruns_on_reinstall() {
    // spec: HOOK-84
    // Nothing is recorded for the hook: it fires on every removal and re-runs on
    // every reinstall. learn -> forget -> learn fires install, uninstall, install.
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(sb.base.join("markers/installed").exists());

    // Clear the markers, then remove and reinstall.
    std::fs::remove_dir_all(sb.base.join("markers")).unwrap();
    assert!(
        sb.mind(&[
            "forget",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    assert!(
        sb.base.join("markers/uninstalled").exists(),
        "uninstall hook fires on removal"
    );

    let r = sb.mind(&[
        "learn",
        "skill:greet",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook must re-run on reinstall (HOOK-84)"
    );
}

#[test]
fn in_place_upgrade_reruns_install_hook_but_not_uninstall_hook() {
    // spec: HOOK-82, HOOK-81
    // An in-place upgrade (same effective name, content swapped) re-runs the item
    // install hook (HOOK-81) but does NOT run the uninstall hook, since the item
    // is not removed (HOOK-82).
    let sb = sandbox_with_item_hooks("agents");
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--link-only"]).success);
    assert!(
        sb.mind(&[
            "learn",
            "skill:greet",
            "--accept-hooks=install,update,uninstall"
        ])
        .success
    );
    std::fs::remove_dir_all(sb.base.join("markers")).unwrap();

    // Change the skill upstream so upgrade swaps its content in place.
    sb.write_and_commit(
        "skills/greet/SKILL.md",
        "---\ndescription: greet the user\n---\n# greet v2\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "-y", "--accept-hooks=install,update,uninstall"]);
    assert!(
        r.success,
        "upgrade should succeed: {} {}",
        r.stdout, r.stderr
    );

    assert!(
        sb.base.join("markers/installed").exists(),
        "the install hook must re-run on an in-place upgrade (HOOK-81)"
    );
    assert!(
        !sb.base.join("markers/uninstalled").exists(),
        "an in-place upgrade must NOT run the uninstall hook (HOOK-82)"
    );
}

#[test]
fn review_lists_item_install_and_uninstall_hooks() {
    // spec: HOOK-85
    // `mind review` surfaces an item's declared install/uninstall hooks as
    // advisory findings so a consumer sees, before installing, that the item runs
    // code on the host.
    let sb = sandbox_with_item_hooks("agents");
    let r = sb.mind(&["review", &sb.source_spec()]);
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(
        all.contains("item-hook"),
        "review must emit item-hook advisories: {all}"
    );
    // The advisory carries the required/optional distinction, as the source
    // hook loop's does: an optional item hook must not read identically to a
    // required one on the surface whose job is pre-install disclosure.
    assert!(
        all.contains("declares a required install hook"),
        "review must list the install hook and whether it is required: {all}"
    );
    assert!(
        all.contains("declares a required uninstall hook"),
        "review must list the uninstall hook and whether it is required: {all}"
    );
}

// ---- CLI-75: hash-based outdated detection ----------------------------------
