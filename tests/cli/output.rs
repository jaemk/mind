//! Item refs, prompts, output, global flags, and exit status (spec/cli.md).

use super::*;

#[test]
fn bare_mind_prints_help_on_stdout_and_exits_zero() {
    // spec: CLI-207 - the first command a new user types. A required subcommand
    // with no arguments is otherwise a clap usage error: nothing on stdout, a
    // usage string on stderr, exit 2.
    let sb = Sandbox::new();
    let r = sb.mind(&[]);
    assert!(
        r.success,
        "bare `mind` must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("Usage") && r.stdout.contains("meld"),
        "bare `mind` must print help on stdout: {}",
        r.stdout
    );
}

/// `review` is the verb whose entire output is findings, so under CLI-218 it
/// answers with a document rather than the human text it prints in text mode.
/// This is the cross-surface pin: the two modes must genuinely differ, and the
/// JSON one must parse. It was written the other way round while `review` was
/// a CLI-217 exclusion, which made it a test that asserted the gap.
#[test]
fn review_json_answers_with_a_document_not_the_text_output() {
    // spec: CLI-218, CLI-219
    let sb = Sandbox::new();
    let plain = sb.mind_cwd(&["review"], &sb.source);
    let json = sb.mind_cwd(&["--json", "review"], &sb.source);
    assert!(
        plain.success && json.success,
        "{} {}",
        plain.stderr,
        json.stderr
    );
    assert_ne!(
        plain.stdout, json.stdout,
        "`--json` must change review's output; it is no longer an exclusion"
    );
    let doc: serde_json::Value = serde_json::from_str(json.stdout.trim())
        .unwrap_or_else(|e| panic!("review --json must be one document: {e}: {:?}", json.stdout));
    assert_eq!(doc["action"], "review", "{doc}");
}

/// The `init-source` twin of the test above (CLI-217's fourth "no --json
/// output" verb along with `hooks list`, covered in tests/cli_hooks.rs).
#[test]
fn init_source_json_flag_is_ignored_and_still_prints_plain_text() {
    // spec: CLI-217
    let sb = Sandbox::bare("init-json");
    let dir = sb.base.join("scaffold-target");
    std::fs::create_dir_all(&dir).unwrap();
    let dir_str = dir.to_str().unwrap();

    let plain = sb.mind(&["init-source", dir_str]);
    // Re-running init-source on the same, now-scaffolded directory still
    // succeeds (CLI-26-style idempotence is not the point here); only the
    // stdout shape under --json matters, so drive a second, distinct target.
    let dir2 = sb.base.join("scaffold-target-json");
    std::fs::create_dir_all(&dir2).unwrap();
    let dir2_str = dir2.to_str().unwrap();
    let json = sb.mind(&["--json", "init-source", dir2_str]);
    assert!(
        plain.success && json.success,
        "{} {}",
        plain.stderr,
        json.stderr
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(json.stdout.trim()).is_err(),
        "init-source --json must not accidentally look like a JSON document: {:?}",
        json.stdout
    );
    assert!(
        json.stdout.contains("mind.toml") || json.stdout.contains("wrote"),
        "init-source --json must still print its ordinary human report: {:?}",
        json.stdout
    );
}

#[test]
fn probe_no_tui_is_long_only() {
    // spec: CLI-164, TUI-54 - `--no-tui` is long-only; `-n` is no longer accepted
    // (CLI-163 reserves -n for --dry-run).
    let sb = melded();
    // --no-tui still works.
    let long = sb.mind(&["probe", "--no-tui"]);
    assert!(long.success, "{}", long.stderr);
    assert!(long.stdout.contains("skill:review"), "{}", long.stdout);
    assert!(long.stdout.contains("agent:dev"), "{}", long.stdout);
    assert!(long.stdout.contains("rule:style"), "{}", long.stdout);
    // -n must fail (unknown flag).
    let short = sb.mind(&["probe", "-n"]);
    assert!(!short.success, "probe -n should fail: {}", short.stdout);
}

#[test]
fn learn_unknown_item_errors() {
    // spec: CLI-3, CLI-100
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist"]);
    assert!(!r.success);
    assert!(r.stderr.contains("no item matches"), "{}", r.stderr);
}

/// `config show --json` answers with exactly one JSON document on stdout
/// (CLI-217, CLI-218): `commands::config_show` has an
/// `if out.json { return print_json(...) }` branch, and `main.rs`'s
/// `json_reserves_stdout` reserves stdout for it like any other verb that
/// answers `--json` with a document. This pins the actual behavior (a single
/// JSON document, config data included) so a future change that adds a line
/// before the branch is caught by the CLI-217 stdout reservation.
#[test]
fn config_show_json_emits_one_document() {
    // spec: CLI-217 CLI-110
    let sb = Sandbox::new();
    let home = sb.base.join("jsonLobe").display().to_string();
    assert!(sb.mind(&["config", "lobes", "add", &home]).success);

    let r = sb.mind(&["--json", "config", "show"]);
    assert!(r.success, "config show --json: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "config show --json stdout must parse as a single JSON document \
             ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        doc.get("config_file").is_some(),
        "the config-show JSON object must be present, not silently dropped: {doc:#}"
    );
    assert!(
        doc["lobes"]
            .as_array()
            .is_some_and(|a| a.iter().any(|l| l.as_str() == Some(home.as_str()))),
        "the added lobe must appear in the JSON lobes array: {doc:#}"
    );
}

#[test]
fn review_local_path_target_is_left_intact() {
    // CLI-130: a local-path target is the user's working dir, NOT a temp clone,
    // so review must leave it on disk and unmodified. Only the remote-spec path
    // clones to a temp area; a local path is read in place.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let skill = sb.source.join("skills/review/SKILL.md");
    let before = std::fs::read_to_string(&skill).unwrap();

    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "clean local review should exit 0: {} {}",
        r.stdout, r.stderr
    );

    // The source dir and its files still exist and are byte-identical.
    assert!(sb.source.is_dir(), "local source dir must survive review");
    let after = std::fs::read_to_string(&skill).unwrap();
    assert_eq!(before, after, "review must not modify the local source");
    // And nothing was cloned into the scratch area.
    assert_no_review_temp(&sb.mind_home);
}

#[test]
fn review_remote_spec_clone_failure_exits_nonzero_and_leaves_no_temp() {
    // CLI-130: a repo-spec target is shallow-cloned to a temp area. When the
    // clone itself FAILS (unreachable remote), review must exit non-zero and
    // leave nothing behind under MIND_HOME/.tmp. Uses a refused-connection URL
    // so the clone fails fast without real network egress.
    let sb = Sandbox::new();

    // parse_spec keeps this as host="127.0.0.1:1" (non-local), so review takes
    // the clone branch; the connection is refused, so the clone errors.
    let r = sb.mind(&["review", "https://127.0.0.1:1/owner/repo"]);
    assert!(
        !r.success,
        "a failed clone must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No leftover scratch dir, and no registry mutation.
    assert_no_review_temp(&sb.mind_home);
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "failed review must not register anything"
    );
}

#[test]
fn review_report_lists_every_advisory_finding() {
    // CLI-131: the report names per-item results, not just an exit code. With a
    // clean item, a missing-description item, and an unguarded-ref item under a
    // prefix, ALL advisories must be printed (not just the first).
    let sb = Sandbox::new();
    // lead.md: has a description AND an unguarded prose ref to sibling `dev`.
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    // nodesc.md: a sibling with no description (advisory: missing-description).
    sb.write_and_commit("agents/nodesc.md", "# no frontmatter\nbody\n");
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    assert!(
        r.success,
        "advisory-only review exits 0: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("missing-description"),
        "missing-description advisory must be printed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("unguarded-reference"),
        "unguarded-reference advisory must be printed: {}",
        r.stdout
    );
    // The clean fixture skill (skill:review has a description) is not flagged
    // for a missing description.
    assert!(
        !r.stdout.contains("skill:review: no description"),
        "clean item must not be flagged missing-description: {}",
        r.stdout
    );
}

#[test]
fn review_multiple_hard_errors_all_reported_and_counted() {
    // CLI-132: two distinct hard problems (two unresolved {{ns:}} tokens in two
    // items) both surface and the summary counts more than one hard error.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
    );
    sb.write_and_commit(
        "agents/boss.md",
        "---\ndescription: boss\n---\nDefer to {{ns:alsonope}}.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "multiple hard errors must exit non-zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("nope"),
        "first bad ref reported: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("alsonope"),
        "second bad ref reported: {}",
        r.stderr
    );
    // The summary line reports a hard-error count greater than one.
    assert!(
        r.stdout.contains("2 hard error(s)"),
        "summary must count both hard errors: {}",
        r.stdout
    );
}

#[test]
fn meld_json_stdout_is_only_the_result_object_when_a_hook_is_skipped() {
    // spec: CLI-217 HOOK-22
    // The HOOK-22 skip note is printed from `run_install_hooks`, which runs
    // BEFORE `meld` emits its CLI-153 result object, and this harness is never a
    // TTY so the note always fires. On stdout it left `mind meld --json` emitting
    // prose followed by an object, which no consumer can parse. The note routes
    // through `render::note`, so under `--json` it lands on stderr instead -- and
    // it is still emitted, since a source whose tooling was not built is exactly
    // what a machine caller needs to hear about.
    let sb = sandbox_with_declared_hook("agents", "touch hookran");
    let r = sb.mind(&["meld", &sb.source_spec(), "--json"]);
    assert!(r.success, "meld --json: {} {}", r.stdout, r.stderr);
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        r.stderr.contains("note: skipped install hook"),
        "the skip note must still be reported, on stderr: {}",
        r.stderr
    );
}

#[test]
fn learn_json_stdout_is_only_the_result_object_when_an_item_hook_is_skipped() {
    // spec: CLI-217 HOOK-81
    // The same shape one level down: a per-item install hook's non-TTY skip note
    // is printed by `install.rs` in the middle of `learn`, ahead of learn's own
    // JSON result.
    let sb = Sandbox::named("agents");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"skills/review\"\n\
         install = \"touch itemhookran\"\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success, "meld");

    let r = sb.mind(&["learn", "review", "--json"]);
    assert!(r.success, "learn --json: {} {}", r.stdout, r.stderr);
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        r.stderr
            .contains("note: skipped install hook for skill:review"),
        "the per-item skip note must still be reported, on stderr: {}",
        r.stderr
    );
}

#[test]
fn json_does_not_swallow_the_stdout_of_verbs_whose_output_is_not_json() {
    // spec: CLI-217
    // CLI-217 reserves stdout under `--json` by pointing the process's fd 1 at
    // stderr for the whole run (main.rs's `json_reserves_stdout` decides which
    // verbs that applies to). The failure mode of that mechanism is the mirror
    // image of the leak it closes: a verb whose stdout payload is NOT a JSON
    // document would have its output silently redirected into stderr, so
    // `mind --json completions bash > mind.bash` would write an empty file.
    // `--json` is a global flag, so this is reachable by accident (an alias, a
    // wrapper script that always passes it), and it fails quietly.
    //
    // `dump` is the third such verb and has its own coverage
    // (cli_dump.rs::dump_json_flag_prints_stderr_note, DUMP-9).
    let sb = Sandbox::new();

    let comp = sb.mind(&["--json", "completions", "bash"]);
    assert!(comp.success, "completions --json: {}", comp.stderr);
    assert!(
        comp.stdout.contains("mind"),
        "the completion script must still go to STDOUT under --json: stdout={:?} stderr={:?}",
        comp.stdout,
        comp.stderr
    );

    let man = sb.mind(&["--json", "man"]);
    assert!(man.success, "man --json: {}", man.stderr);
    assert!(
        man.stdout.contains(".TH"),
        "the roff page must still go to STDOUT under --json: stdout={:?} stderr={:?}",
        man.stdout,
        man.stderr
    );
}

#[test]
fn json_stdout_is_exactly_one_document_across_the_verb_surface() {
    // spec: CLI-217 CLI-153
    // CLI-217 is a property of `--json`, not of one verb, and the change that
    // introduced it converted seven sites found by inspection. This walks the
    // whole mutating/reporting surface in one ordinary lifecycle so a verb that
    // was never looked at cannot leak prose (or a second object) onto stdout
    // unnoticed. It is a floor, not a substitute for the adverse-condition
    // tests around it: it exercises only the happy path of each verb, which is
    // exactly the path least likely to print an aside.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let lobe = sb.base.join("extra-lobe");
    std::fs::create_dir_all(&lobe).unwrap();
    let lobe_str = lobe.to_string_lossy().into_owned();

    let steps: Vec<Vec<&str>> = vec![
        vec!["meld", &spec, "--json"],
        vec!["meld", &spec, "--json"], // the already-melded (remeld) branch
        vec!["recall", "--json"],
        vec!["recall", "--sources", "--json"],
        vec!["probe", "--json", "--no-tui"],
        vec!["learn", "review", "--json"],
        vec!["learn", "review", "--json"], // the up-to-date branch
        vec!["introspect", "--json"],
        vec!["sync", "--json"],
        vec!["upgrade", "--json", "--yes"],
        vec!["config", "lobes", "add", "--json", &lobe_str],
        vec!["config", "lobes", "remove", "--json", &lobe_str],
        vec!["forget", "review", "--json", "--yes"],
        vec!["unmeld", "agents", "--json", "--yes"],
    ];

    let mut silent: Vec<String> = Vec::new();
    for args in steps {
        let r = sb.mind(&args);
        let body = r.stdout.trim();
        if body.is_empty() {
            silent.push(args.join(" "));
            continue;
        }
        serde_json::from_str::<serde_json::Value>(body).unwrap_or_else(|e| {
            panic!(
                "`mind {}` must leave stdout as exactly one JSON document ({e}); \
                 anything else belongs on stderr (CLI-217):\nstdout={:?}\nstderr={:?}",
                args.join(" "),
                r.stdout,
                r.stderr
            )
        });
    }
    // No invocation above may answer with silence. The last holdout was the
    // second `meld <spec> --json` (re-melding a source that still has items to
    // install, without `--yes`): `remeld`'s `!link_only` block returned as soon
    // as `install_source_items` had run, so its own `already-melded` object was
    // never reached. Zero documents is not the CLI-217 corruption mode, but it
    // left a CLI-153 mutating verb telling a machine caller nothing at all.
    // spec: CLI-153
    assert!(
        silent.is_empty(),
        "every --json invocation must answer with a document; these wrote \
         nothing to stdout: {silent:?}"
    );
}

#[test]
fn unmeld_json_stdout_is_only_the_result_object_when_an_uninstall_hook_is_skipped() {
    // spec: CLI-217 HOOK-54
    // The third converted `render::note` site (`run_uninstall_hooks`). It runs
    // in `unmeld_one` BEFORE the source is dropped and BEFORE the CLI-153
    // result object is printed, so under `--json` a bare `println!` here put
    // prose ahead of the object exactly as the meld and learn paths did. Only
    // those two paths had tests, which is why this one had to be checked
    // separately rather than inferred from them.
    let sb = Sandbox::named("agents");
    sb.write_and_commit(
        "mind.toml",
        "[[hooks]]\nname = \"cleanup\"\nrun = \"echo clean\"\nevent = \"uninstall\"\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success, "meld");

    let r = sb.mind(&["unmeld", "agents", "--json", "--yes"]);
    assert!(r.success, "unmeld --json: {} {}", r.stdout, r.stderr);
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert!(
        r.stderr.contains("note: skipped uninstall hook 'cleanup'"),
        "the uninstall-hook skip note must still be reported, on stderr: {}",
        r.stderr
    );
}

#[test]
fn hook_skip_notes_stay_on_stdout_in_text_mode() {
    // spec: CLI-217 HOOK-22 HOOK-54
    // CLI-217 routes these notes; it does not move them. In text mode they are
    // "byte for byte what a `println!` would have written", i.e. STDOUT. Every
    // other test of the conversion asserts the `--json` half (the note is on
    // stderr) or accepts either stream, so all of them would still pass if
    // `render::note` unconditionally wrote to stderr -- which would silently
    // move a routine, expected line out of the output a user pipes and reads.
    // Both converted hook sites are checked here, in one run each.
    let src = sandbox_with_declared_hook("agents", "touch hookran");
    let meld = src.mind(&["meld", &src.source_spec()]);
    assert!(meld.success, "meld: {} {}", meld.stdout, meld.stderr);
    assert!(
        meld.stdout.contains("note: skipped install hook "),
        "the install-hook skip note belongs on STDOUT in text mode: stdout={} stderr={}",
        meld.stdout,
        meld.stderr
    );

    let un = Sandbox::named("agents");
    un.write_and_commit(
        "mind.toml",
        "[[hooks]]\nname = \"cleanup\"\nrun = \"echo clean\"\nevent = \"uninstall\"\n",
    );
    assert!(un.mind(&["meld", &un.source_spec()]).success, "meld");
    let r = un.mind(&["unmeld", "agents", "--yes"]);
    assert!(r.success, "unmeld: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("note: skipped uninstall hook 'cleanup'"),
        "the uninstall-hook skip note belongs on STDOUT in text mode: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn item_hook_skip_note_stays_on_stdout_in_text_mode() {
    // spec: CLI-217 HOOK-81
    // The `install.rs` half of the same rule: the per-item install-hook skip
    // note. Same reason as above -- the `--json` test for this site asserts
    // stderr, so nothing pinned the text-mode stream.
    let sb = Sandbox::named("agents");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"skills/review\"\n\
         install = \"touch itemhookran\"\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success, "meld");
    let r = sb.mind(&["learn", "review"]);
    assert!(r.success, "learn: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout
            .contains("note: skipped install hook for skill:review"),
        "the per-item skip note belongs on STDOUT in text mode: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn meld_json_stdout_is_only_the_result_object_when_a_hook_actually_runs() {
    // CLI-217 binds "every line a verb may print", with no exemption for a hook
    // that was consented to. Three sites on this one path wrote to stdout
    // unconditionally:
    //   src/commands/hooks.rs  `println!("running install hook '{}' for {}", ...)`
    //   src/hook.rs      `println!("====== (hook-stdout: {label}) ======")`
    //                    plus `print!("{stdout_str}")` -- the hook's own output
    //   src/hook.rs      the same pair for the hook's stderr, also on stdout
    // `hook::decide` returns `Run` in a non-TTY only when
    // `--accept-hooks=install,update,uninstall` is passed, so this is exactly the
    // unattended/CI invocation the flag exists for, and it is also the one that
    // most wants `--json`. The hook's own stdout is the worst of the three: it
    // is arbitrary text chosen by the SOURCE AUTHOR, so a `--json` consumer's
    // stdout could be made to contain anything at all -- including a forged
    // result envelope. Under `--json` the whole process now runs with fd 1
    // pointed at stderr (main.rs's `json_stdout`), so neither mind's own
    // `println!` nor a child process that inherits stdout can reach it; the one
    // document is written by `main` at the end.
    // spec: CLI-217 HOOK-23
    // The command prints text that does not appear in the command itself:
    // the result document's `hooks` member names the command (HOOK-161), so
    // only the output is a marker for the output.
    let sb = sandbox_with_declared_hook("agents", "printf 'hook-said-%s' this");
    let r = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--json",
        "--accept-hooks=install,update,uninstall",
    ]);
    assert!(r.success, "meld --json: {} {}", r.stdout, r.stderr);
    serde_json::from_str::<serde_json::Value>(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    // The hook DID run, and its output is still shown -- on stderr. Asserting
    // this is what keeps the fix from degenerating into "swallow the hook's
    // output", which would also pass the parse above.
    assert!(
        r.stderr.contains("hook-said-this"),
        "the hook's own output must still be reported, on stderr: {}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("hook-said-this"),
        "source-controlled hook output must never reach --json stdout: {}",
        r.stdout
    );
}

#[test]
fn remeld_json_stdout_is_one_document_when_it_installs_pending_items() {
    // spec: CLI-217 CLI-153 CLI-12
    // `remeld` (the already-melded branch) used to be the one `--json` path that
    // did NOT install silently the way `meld` does: under `--yes` its install
    // called `learn`, which emits its OWN CLI-153 object and returns before
    // remeld's. Register-only
    // first so items are genuinely pending, then re-meld with --yes so that path
    // is taken.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--register-only"]).success,
        "register-only meld"
    );

    let r = sb.mind(&["meld", &spec, "--yes", "--json"]);
    assert!(r.success, "re-meld --json: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    assert_eq!(
        doc["schema"], 1,
        "the one document must be a CLI-153 result envelope: {doc:#}"
    );
    // The one document answers the verb the caller INVOKED. It used to be
    // `learn`'s -- `install_source_items` returned through `learn`'s own
    // `print_json` and `remeld` never reached its `already-melded` object -- so
    // `mind meld --json` reported an action nobody asked for. A re-meld now
    // mirrors the fresh-meld branch (CLI-156): one `meld` object whose
    // `installed` array carries what the install step did.
    assert_eq!(
        doc["action"], "meld",
        "a re-meld must answer with the invoked verb, not the nested one: {doc:#}"
    );
    assert_eq!(
        doc["outcome"], "already-melded",
        "CLI-153's re-meld outcome token: {doc:#}"
    );
    assert!(
        doc["installed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "skill:review")),
        "the pending items installed by this call must be listed: {doc:#}"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the pending items must actually have been installed: {doc:#}"
    );
}

#[test]
fn remeld_json_answers_a_machine_caller_even_when_it_installs_nothing() {
    // spec: CLI-217 CLI-153 CLI-12
    // The other half of the same defect, and the one that produced ZERO
    // documents rather than two: re-melding a source that still has items to
    // install, WITHOUT `--yes`. `remeld`'s `!link_only` block returned as soon
    // as `install_source_items` had run (in a non-TTY without `--yes` that only
    // prints a note, suppressed under `--json`), so remeld's own object was
    // never reached and `mind meld <melded> --json` answered a machine caller
    // with silence -- from a CLI-153 mutating verb. Nothing was corrupted, which
    // is exactly why no parse-based check caught it.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--register-only"]).success,
        "register-only meld"
    );

    let r = sb.mind(&["meld", &spec, "--json"]);
    assert!(r.success, "re-meld --json: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "a mutating verb must answer --json with a document, not silence \
             ({e}): stdout={:?} stderr={:?}",
            r.stdout, r.stderr
        )
    });
    assert_eq!(doc["action"], "meld", "{doc:#}");
    assert_eq!(doc["outcome"], "already-melded", "{doc:#}");
    // The items are still PENDING (no --yes), and the count says so rather than
    // the caller having to infer it from an empty `installed`.
    assert!(
        doc["pending_items"].as_u64().is_some_and(|n| n > 0),
        "the un-installed items must be reported as pending (CLI-156's field on \
         the fresh-meld branch): {doc:#}"
    );
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "nothing may be installed without --yes: {doc:#}"
    );
}

#[test]
fn remeld_json_of_a_curator_emits_one_document_not_two() {
    // CLI-217: "under --json, stdout carries exactly one JSON document and
    // nothing else". `remeld` was the `--json` path with no silent install
    // helper: for a curator with no items of its own it ran
    //     install_curated_sources(...)?;   // -> install_source_items -> learn
    //                                      //    -> learn's own print_json
    //     ...
    //     if out.json { return print_json(already-melded) }   // a SECOND doc
    // (src/commands/meld.rs, `remeld`, the `!link_only` block). `meld`'s fresh-meld
    // branch avoids this by installing silently under `--json` (CLI-156); the re-meld branch has now been given the same
    // treatment, so `mind meld <curator> --yes --json | jq .` no longer trips
    // over a trailing object.
    // spec: CLI-217 CLI-153 CLI-156
    let sb = Sandbox::bare("json-curator");
    nested_repo(&sb.base.join("curated-lib"), "greet");
    let nested_abs = sb.base.join("curated-lib").to_string_lossy().into_owned();
    sb.write_and_commit(
        "mind.toml",
        &format!("[[discover.sources]]\nsource = \"{nested_abs}\"\ninstall = true\n"),
    );
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--register-only"]).success,
        "register-only meld of the curator"
    );

    let r = sb.mind(&["meld", &spec, "--yes", "--json"]);
    assert!(r.success, "re-meld --json: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout under --json must parse as a single JSON document ({e}): {:?}",
            r.stdout
        )
    });
    // The one document is the CALLER'S verb, and it accounts for the curated
    // install that happened underneath it rather than dropping it on the floor.
    assert_eq!(doc["action"], "meld", "{doc:#}");
    assert_eq!(doc["outcome"], "already-melded", "{doc:#}");
    assert!(
        doc["installed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "skill:greet")),
        "the curated chain's install must be reported in meld's object: {doc:#}"
    );
    assert!(
        sb.claude_home.join("skills/greet").exists(),
        "the curated item must actually be installed: {doc:#}"
    );
}

#[test]
fn help_lists_upgrade_evolve_and_self_update_alias() {
    // Confirm clap renders both subcommands. `self-update` is now a visible alias
    // for `evolve` (CLI-172) and must appear in --help.
    let sb = Sandbox::new();
    let r = sb.mind(&["--help"]);
    assert!(
        r.success,
        "mind --help should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("upgrade"),
        "help must list the 'upgrade' subcommand: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("evolve"),
        "help must list the 'evolve' subcommand: {}",
        r.stdout
    );
    // spec: CLI-172 - self-update is now a visible alias for evolve.
    assert!(
        r.stdout.contains("self-update"),
        "help must list the 'self-update' visible alias: {}",
        r.stdout
    );
}

// ---- evolve policy control tests (POL-51..54) --------------------------------
//
// Policy is injected via $MIND_POLICY_FILE. evolve --check --version <X> is
// fully offline (no network); that property is exploited here so tests run
// without a live GitHub connection.

#[test]
fn json_learn_emits_result_object_and_no_prose() {
    // spec: CLI-153, CLI-150
    let sb = melded();

    // --json before the verb.
    let pre = sb.mind(&["--json", "learn", "skill:review"]);
    assert!(pre.success, "learn --json failed: {}", pre.stderr);
    let v = parse_json(&pre.stdout);
    assert_eq!(v["action"], "learn", "{}", pre.stdout);
    assert_eq!(v["target"], "skill:review", "{}", pre.stdout);
    assert_eq!(v["outcome"], "installed", "{}", pre.stdout);
    // The `installed` array names the effective key actually installed.
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        pre.stdout
    );
    // CLI-153: nothing else on stdout. The non-json path prints "learned ...";
    // that prose must be absent under --json.
    assert!(
        !pre.stdout.contains("learned"),
        "human prose `learned` must not appear under --json: {}",
        pre.stdout
    );
    // The JSON-only stdout has no ANSI escapes (also CLI-151).
    assert!(!has_ansi_escape(&pre.stdout), "json stdout: {}", pre.stdout);

    // --json AFTER the verb yields an equivalent object (CLI-150: position-free).
    let sb2 = melded();
    let post = sb2.mind(&["learn", "skill:review", "--json"]);
    assert!(
        post.success,
        "learn --json (suffix) failed: {}",
        post.stderr
    );
    assert_eq!(
        parse_json(&post.stdout),
        v,
        "flag position must not change the JSON: pre={} post={}",
        pre.stdout,
        post.stdout
    );
}

#[test]
fn json_forget_emits_removed_object_and_no_prose() {
    // spec: CLI-153
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    let r = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(r.success, "forget --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "forget", "{}", r.stdout);
    assert_eq!(v["target"], "skill:review", "{}", r.stdout);
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    assert_eq!(
        v["removed"],
        serde_json::json!(["skill:review"]),
        "{}",
        r.stdout
    );
    // The non-json path prints "forgot <key>"; that prose must be absent.
    assert!(
        !r.stdout.contains("forgot"),
        "human prose `forgot` must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_meld_emits_result_object_and_no_prose() {
    // spec: CLI-153
    // A default non-TTY meld registers the source (and installs nothing); under
    // --json it emits a single meld object with outcome "melded".
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    let r = sb.mind(&["--json", "meld", &spec]);
    assert!(r.success, "meld --json failed: {} {}", r.stdout, r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "{}", r.stdout);
    assert_eq!(v["outcome"], "melded", "{}", r.stdout);
    assert!(
        v["target"].is_string() && !v["target"].as_str().unwrap().is_empty(),
        "meld target must name the source: {}",
        r.stdout
    );
    // The non-json default-meld path prints how to "learn" items later; under
    // --json that prose is suppressed.
    assert!(
        !r.stdout.contains("learn") && !r.stdout.contains("melded source"),
        "default-meld prose must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_remeld_already_melded_is_a_single_object() {
    // spec: CLI-153
    // Re-melding a fully-installed source under --json emits exactly one JSON
    // object (outcome "already-melded"), not the human item-status report. The
    // "already-melded" outcome is only reached when nothing remains to install,
    // so the source must be installed first (a default non-TTY meld installs
    // nothing, which would instead route through the install flow).
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld+install");
    let r = sb.mind(&["meld", &spec, "--json"]);
    assert!(r.success, "re-meld --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "meld", "{}", r.stdout);
    assert_eq!(v["outcome"], "already-melded", "{}", r.stdout);
    // The non-json re-meld prints "already melded" prose and "to install ...";
    // none of that may leak onto stdout under --json.
    assert!(
        !r.stdout.contains("already melded") && !r.stdout.contains("to install"),
        "re-meld prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_sync_emits_result_object_and_no_prose() {
    // spec: CLI-153
    let sb = melded();
    let r = sb.mind(&["sync", "--json"]);
    assert!(r.success, "sync --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "sync", "{}", r.stdout);
    assert_eq!(v["outcome"], "synced", "{}", r.stdout);
    assert!(v["count"].is_number(), "sync count: {}", r.stdout);
    // The non-json path prints "syncing <source> ..."; suppressed under --json.
    assert!(
        !r.stdout.contains("syncing") && !r.stdout.contains("up to date"),
        "sync prose must not appear under --json: {}",
        r.stdout
    );
    assert!(!has_ansi_escape(&r.stdout), "json stdout: {}", r.stdout);
}

#[test]
fn json_sync_no_op_on_empty_registry() {
    // spec: CLI-153
    // With no sources melded, sync changes nothing: the outcome is the explicit
    // "no-op" value, not a human "no sources melded" message.
    let sb = Sandbox::new();
    let r = sb.mind(&["sync", "--json"]);
    assert!(r.success, "sync --json on empty registry: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "sync", "{}", r.stdout);
    assert_eq!(v["outcome"], "no-op", "{}", r.stdout);
    assert!(
        !r.stdout.contains("no sources"),
        "no-op prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_upgrade_up_to_date_is_an_object() {
    // spec: CLI-153
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind(&["upgrade", "--json"]);
    assert!(r.success, "upgrade --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "upgrade", "{}", r.stdout);
    assert_eq!(v["outcome"], "up-to-date", "{}", r.stdout);
    assert!(
        !r.stdout.contains("up to date"),
        "upgrade prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_upgrade_applies_and_reports_upgraded() {
    // spec: CLI-153
    // A real delta upgraded under --json emits outcome "upgraded" plus the
    // installed keys, and no "upgraded skill:review" prose.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    sb.edit_source();
    assert!(sb.mind(&["sync"]).success);

    let r = sb.mind(&["upgrade", "--yes", "--json"]);
    assert!(r.success, "upgrade --yes --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "upgrade", "{}", r.stdout);
    assert_eq!(v["outcome"], "upgraded", "{}", r.stdout);
    assert_eq!(
        v["installed"],
        serde_json::json!(["skill:review"]),
        "{}",
        r.stdout
    );
    // The "✓ upgraded ..." prose line must be gone.
    assert!(
        !r.stdout.contains("upgraded skill"),
        "upgrade prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_unmeld_emits_result_object() {
    // spec: CLI-153
    // Unmeld with an installed item removes it and the source; under --json this
    // is a single object (outcome "removed"), with the item-removal prose absent.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--yes"]).success, "meld+install");
    let name = "agents"; // the fixture source's directory name

    let r = sb.mind(&["unmeld", name, "--yes", "--json"]);
    assert!(r.success, "unmeld --json failed: {} {}", r.stdout, r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "unmeld", "{}", r.stdout);
    // `target` is the source's canonical identity (e.g. `local/<base>/agents`),
    // which ends with the source dir name the command was given.
    assert!(
        v["target"]
            .as_str()
            .is_some_and(|t| t.ends_with(&format!("/{name}")) || t == name),
        "unmeld target must name the source: {}",
        r.stdout
    );
    assert_eq!(v["outcome"], "removed", "{}", r.stdout);
    assert!(
        !r.stdout.contains("unmelded"),
        "unmeld prose must not appear under --json: {}",
        r.stdout
    );
}

#[test]
fn json_lobe_add_and_remove_emit_objects() {
    // spec: CLI-153
    let sb = Sandbox::new();
    let extra = sb.base.join("extra-lobe");
    let extra_s = extra.to_string_lossy().into_owned();

    let added = sb.mind(&["config", "lobes", "add", &extra_s, "--json"]);
    assert!(added.success, "lobe add --json failed: {}", added.stderr);
    let v = parse_json(&added.stdout);
    assert_eq!(v["action"], "lobe-add", "{}", added.stdout);
    assert_eq!(v["outcome"], "added", "{}", added.stdout);

    // Re-adding the same lobe is a no-op outcome, not a human message.
    let again = sb.mind(&["config", "lobes", "add", &extra_s, "--json"]);
    assert!(again.success, "{}", again.stderr);
    assert_eq!(
        parse_json(&again.stdout)["outcome"],
        "no-op",
        "{}",
        again.stdout
    );

    let removed = sb.mind(&["config", "lobes", "remove", &extra_s, "--json"]);
    assert!(
        removed.success,
        "lobe remove --json failed: {}",
        removed.stderr
    );
    let rv = parse_json(&removed.stdout);
    assert_eq!(rv["action"], "lobe-remove", "{}", removed.stdout);
    assert_eq!(rv["outcome"], "removed", "{}", removed.stdout);
}

#[test]
fn json_learn_dry_run_lists_nothing_installed_as_prose() {
    // spec: CLI-153
    // A --dry-run under --json reports outcome "dry-run" as an object, and does
    // not print the "would learn N item(s)" prose.
    let sb = melded();
    let r = sb.mind(&["learn", "skill:review", "--dry-run", "--json"]);
    assert!(r.success, "learn --dry-run --json failed: {}", r.stderr);
    let v = parse_json(&r.stdout);
    assert_eq!(v["action"], "learn", "{}", r.stdout);
    assert_eq!(v["outcome"], "dry-run", "{}", r.stdout);
    assert!(
        !r.stdout.contains("would learn"),
        "dry-run prose must not appear under --json: {}",
        r.stdout
    );
    // A dry-run installs nothing.
    assert!(
        !sb.mind(&["recall"]).stdout.contains("installed @"),
        "dry-run must not install anything"
    );
}

#[test]
fn json_error_emits_envelope_on_stdout_not_stderr() {
    // spec: CLI-181, CLI-182
    // Under --json an error emits {"schema":1,"error":{"kind":"...","message":"..."}}
    // to stdout; nothing is written to stderr by the main error handler.
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist", "--json"]);
    assert!(!r.success, "unknown item must fail");
    // The envelope must be valid JSON on stdout.
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    let err = &v["error"];
    assert_eq!(
        err["kind"], "item-not-found",
        "kind must be item-not-found: {}",
        r.stdout
    );
    assert!(
        err["message"]
            .as_str()
            .map(|s| s.contains("does-not-exist"))
            .unwrap_or(false),
        "message must contain the query: {}",
        r.stdout
    );
    // No error text on stderr from the main error handler.
    assert!(
        !r.stderr.contains("error:"),
        "main error handler must not write to stderr under --json: {}",
        r.stderr
    );
    // Exit code must be 1 (FAILURE), not 2.
    // (success == false + no clap usage error means exit 1.)
}

#[test]
fn json_error_envelope_schema_and_kind_fields() {
    // spec: CLI-181, CLI-182
    // A second distinct error class (SourceNotFound via `forget`) also produces
    // the envelope, confirming the schema/kind contract is not specific to learn.
    let sb = melded();
    // `forget` on a name that is not installed -> NotInstalled (not SourceNotFound,
    // since forget resolves from the manifest). Use `unmeld` on a nonexistent
    // source name to get SourceNotFound.
    let r = sb.mind(&["--json", "unmeld", "no-such-source"]);
    assert!(!r.success, "unmeld unknown source must fail");
    let v = parse_json(&r.stdout);
    assert_eq!(v["schema"], 1, "schema field must be 1: {}", r.stdout);
    let err = &v["error"];
    // The kind slug must be the stable source-not-found slug.
    assert_eq!(
        err["kind"], "source-not-found",
        "kind must be source-not-found: {}",
        r.stdout
    );
    // message must be non-empty and contain the queried name.
    let msg = err["message"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "message must be non-empty: {}", r.stdout);
    assert!(
        msg.contains("no-such-source"),
        "message must contain the source name: {}",
        r.stdout
    );
    // Nothing on stderr from the main error handler.
    assert!(
        !r.stderr.contains("error:"),
        "no stderr from main handler under --json: {}",
        r.stderr
    );
}

#[test]
fn clap_usage_error_is_not_json_enveloped() {
    // spec: CLI-183
    // A clap argument-parse failure (unknown flag) exits 2 with plain text on
    // stderr, not a JSON envelope on stdout. The --json envelope only applies to
    // post-parse MindError failures (exit 1).
    let sb = Sandbox::new();
    // An unknown flag causes clap to exit 2 before any command logic runs.
    let r = sb.mind(&["--no-such-flag"]);
    // Exit code 2 is reported by Command::status() as non-success.
    assert!(!r.success, "unknown flag must fail");
    // stdout must NOT contain a JSON envelope; clap writes to stderr.
    assert!(
        r.stdout.trim().is_empty(),
        "clap usage errors must not produce stdout output: {:?}",
        r.stdout
    );
    // stderr must carry clap's plain-text error message.
    assert!(
        !r.stderr.is_empty(),
        "clap must write the usage error to stderr: {:?}",
        r.stderr
    );
}

#[test]
fn non_tty_output_is_plain_ascii_with_no_escapes() {
    // spec: CLI-151
    // The harness pipes stdout, so the capability gate is OFF: every ordinary
    // (non-json) command's stdout must be free of ANSI escape sequences.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    for args in [
        vec!["recall"],
        vec!["recall", "--sources"],
        vec!["recall", "skill:review"],
        vec!["probe"],
        vec!["introspect"],
        vec!["upgrade"],
    ] {
        let r = sb.mind(&args);
        assert!(
            !has_ansi_escape(&r.stdout),
            "non-TTY stdout for `{args:?}` must contain no ANSI escapes: {:?}",
            r.stdout
        );
    }
}

#[test]
fn no_color_env_forces_plain_ascii() {
    // spec: CLI-154
    // NO_COLOR set (even though already non-TTY) must keep the gate OFF: no
    // escapes appear. Passed in the child env so it cannot race other tests.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let r = sb.mind_env(&["recall"], &[("NO_COLOR", "1")]);
    assert!(r.success, "recall failed: {}", r.stderr);
    assert!(
        !has_ansi_escape(&r.stdout),
        "NO_COLOR must force plain ASCII: {:?}",
        r.stdout
    );

    // An empty NO_COLOR value also counts as "set" and forces the gate OFF.
    let empty = sb.mind_env(&["recall"], &[("NO_COLOR", "")]);
    assert!(
        !has_ansi_escape(&empty.stdout),
        "empty NO_COLOR must still force plain ASCII: {:?}",
        empty.stdout
    );
}

#[test]
fn ascii_flag_forces_plain_output() {
    // spec: CLI-154
    // --ascii forces the gate OFF regardless of other state; in this non-TTY
    // harness the result is still escape-free ASCII, and accepted before or
    // after the verb (CLI-150).
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    let pre = sb.mind(&["--ascii", "recall"]);
    assert!(pre.success, "--ascii recall failed: {}", pre.stderr);
    assert!(!has_ansi_escape(&pre.stdout), "{:?}", pre.stdout);
    let post = sb.mind(&["recall", "--ascii"]);
    assert!(!has_ansi_escape(&post.stdout), "{:?}", post.stdout);
}

#[test]
fn ascii_fallback_glyphs_are_present_in_plain_mode() {
    // spec: CLI-152
    // With the gate OFF, every glyph uses its ASCII fallback. recall's status
    // view marks an installed item with `installed @` (the `+` ok glyph) and an
    // available one with `available` (the `-` glyph); probe marks an installed
    // row with the `*` ASCII bullet. None of the Unicode glyphs may appear.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("installed @"),
        "installed marker (ASCII fallback) must show `installed @`: {recall}"
    );
    assert!(
        recall.contains("available"),
        "available marker (ASCII fallback) must show `available`: {recall}"
    );
    // The Unicode status glyphs from src/render.rs must NOT leak into plain mode.
    for glyph in ['✓', '○', '✗', '●'] {
        assert!(
            !recall.contains(glyph),
            "Unicode glyph {glyph:?} must not appear in plain mode: {recall}"
        );
    }

    // probe marks the installed row with the `*` ASCII bullet (not `●`).
    let probe = sb.mind(&["probe", "review"]).stdout;
    assert!(
        probe.contains('*'),
        "probe must mark the installed item with the `*` ASCII bullet: {probe}"
    );
    assert!(
        !probe.contains('●'),
        "probe must not emit the Unicode bullet in plain mode: {probe}"
    );
}

#[test]
fn every_reachable_verb_emits_valid_json_under_json_flag() {
    // spec: CLI-153
    // Cross-check: each mutating verb the hermetic fixture can drive produces a
    // single, parseable JSON object under --json (no torn or doubled output).
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let meld = sb.mind(&["meld", &spec, "--json"]);
    assert!(meld.success, "{}", meld.stderr);
    assert!(parse_json(&meld.stdout).is_object(), "{}", meld.stdout);

    let learn = sb.mind(&["learn", "skill:review", "--json"]);
    assert!(learn.success, "{}", learn.stderr);
    assert!(parse_json(&learn.stdout).is_object(), "{}", learn.stdout);

    let sync = sb.mind(&["sync", "--json"]);
    assert!(sync.success, "{}", sync.stderr);
    assert!(parse_json(&sync.stdout).is_object(), "{}", sync.stdout);

    let upgrade = sb.mind(&["upgrade", "--json"]);
    assert!(upgrade.success, "{}", upgrade.stderr);
    assert!(
        parse_json(&upgrade.stdout).is_object(),
        "{}",
        upgrade.stdout
    );

    let forget = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(forget.success, "{}", forget.stderr);
    assert!(parse_json(&forget.stdout).is_object(), "{}", forget.stdout);

    let unmeld = sb.mind(&["unmeld", "agents", "--json", "--yes"]);
    assert!(unmeld.success, "{}", unmeld.stderr);
    assert!(parse_json(&unmeld.stdout).is_object(), "{}", unmeld.stdout);
}

#[test]
fn json_sync_upgrade_emits_one_document_for_the_invoked_verb() {
    // spec: CLI-153 CLI-217
    // `sync --upgrade --json` performs two logical actions (sync, then the
    // upgrade pass) and USED TO emit one JSON object per action -- a stream that
    // this test previously pinned, asserting that a single-value parse must
    // FAIL. CLI-217 says the opposite and is the newer, stronger statement: two
    // concatenated objects break every ordinary consumer, and no spec statement
    // ever required the second one (CLI-53 describes the pass, not its output).
    //
    // A seventh instance of the same class as this round's fixes, and one no
    // previous pass looked at, because it was hiding behind a passing test that
    // asserted the defect. The upgrade pass no longer prints: `upgrade_pass`
    // returns its result, the real `upgrade` verb prints it, and `sync` folds
    // what the pass applied into its own `installed`.
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);
    // Move the source ahead so the pass has something real to apply; otherwise
    // it is up-to-date and the fold is vacuous.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review code (v2)\n---\n# review v2\n",
    );
    // spec: LIFE-15 -- B1: `--json` is non-interactive, so applying the
    // `--upgrade` pass without `--yes` must refuse (mirroring `mind upgrade
    // --json`) rather than silently apply; `--yes` is required here.
    let r = sb.mind(&["sync", "--upgrade", "--json", "--yes"]);
    assert!(
        r.success,
        "sync --upgrade --json --yes failed: {}",
        r.stderr
    );

    let doc: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "sync --upgrade --json must leave stdout as exactly one JSON \
             document ({e}): {:?}",
            r.stdout
        )
    });
    assert_eq!(
        doc["action"], "sync",
        "the one document answers the invoked verb: {doc:#}"
    );
    // The pass's outcome is not dropped: what it applied appears in sync's
    // `installed`, so a caller can still see that (and what) was upgraded.
    assert!(
        doc["installed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "skill:review")),
        "the --upgrade pass's applied items must be folded into sync's object: {doc:#}"
    );
}

#[test]
fn json_upgrade_verb_still_emits_its_own_object() {
    // The guard on the refactor above: `upgrade_pass` stopped printing, so the
    // real `upgrade` verb must print the result it now returns. Every outcome
    // token comes from the same place, so the up-to-date branch (the one a
    // caller polls) is checked alongside the applied branch.
    // spec: CLI-153
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:review"]).success);

    let clean = sb.mind(&["upgrade", "--json", "--yes"]);
    assert!(clean.success, "upgrade --json: {}", clean.stderr);
    let doc: serde_json::Value = serde_json::from_str(clean.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "upgrade --json must emit one object ({e}): {:?}",
            clean.stdout
        )
    });
    assert_eq!(doc["action"], "upgrade", "{doc:#}");
    assert_eq!(doc["outcome"], "up-to-date", "{doc:#}");

    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review code (v2)\n---\n# review v2\n",
    );
    let applied = sb.mind(&["upgrade", "--json", "--yes"]);
    assert!(applied.success, "upgrade --json: {}", applied.stderr);
    let doc: serde_json::Value = serde_json::from_str(applied.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "upgrade --json must emit one object ({e}): {:?}",
            applied.stdout
        )
    });
    assert_eq!(doc["outcome"], "upgraded", "{doc:#}");
    assert!(
        doc["installed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|k| k == "skill:review")),
        "the upgraded item must be listed: {doc:#}"
    );
}

// ===== Per-item install/uninstall hooks (HOOK-80..85) =====

/// CLI-155: the `recall` status view uses a distinct left-edge marker for an
/// installed-but-stale item. With the capability gate OFF (captured stdout), a
/// current install shows the ASCII installed glyph `+`; an out-of-date install
/// shows the stale glyph `^` instead. Assert on the per-item line so the marker,
/// not just the trailing `(outdated)` text, carries the state.
// spec: CLI-155
#[test]
fn recall_status_view_uses_stale_marker_for_outdated_item() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Fresh install: the review line carries the `+` installed marker, not `^`.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in recall output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('+'),
        "a current install must lead with the `+` marker: {line:?}"
    );

    // Edit the item source in place (hash drift, commit unchanged).
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Now the review line must lead with the `^` stale marker, not `+`.
    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in recall output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
}

/// CLI-155: the `source_status` view (reached by re-melding an already-melded
/// source) also uses the stale marker `^` for an out-of-date item rather than the
/// installed `+`.
// spec: CLI-155
#[test]
fn source_status_uses_stale_marker_for_outdated_item() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // In-place edit (no commit) so only the content hash drifts.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    // Re-meld the already-melded source: all items installed, so this falls
    // through to source_status.
    let r = sb.mind(&["meld", &spec]);
    assert!(r.success, "re-meld failed: {} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("skill:review"))
        .unwrap_or_else(|| panic!("no review line in source_status output: {}", r.stdout));
    assert_eq!(
        line.trim_start().chars().next(),
        Some('^'),
        "an outdated install must lead with the `^` stale marker: {line:?}"
    );
}

/// After an in-place content edit, `mind recall <item>` must show an out-of-date
/// note in the single-item detail view.
#[test]
fn recall_item_detail_shows_out_of_date_after_content_edit() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    // Edit source file in place without committing.
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review the diff for bugs\n---\n# review skill\nmodified content\n",
    );

    let r = sb.mind(&["recall", "skill:review"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("out of date"),
        "recall <item> must show out-of-date note after content edit: {}",
        r.stdout
    );
}

/// Control case: an item whose source file has not been edited must NOT be
/// marked outdated by `mind recall`.
#[test]
fn recall_does_not_mark_unedited_item_outdated() {
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("outdated"),
        "unedited item must not be marked outdated: {}",
        r.stdout
    );

    let r = sb.mind(&["recall", "skill:review"]);
    assert!(r.success, "recall failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("out of date"),
        "recall <item> must not show out-of-date for unedited item: {}",
        r.stdout
    );
}

#[test]
fn meld_register_only_is_canonical() {
    // spec: CLI-165 - `--register-only` is the canonical flag; `--link-only` is a
    // hidden deprecated alias that still works.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // The new canonical flag registers without installing.
    assert!(
        sb.mind(&["meld", &spec, "--register-only"]).success,
        "--register-only must succeed"
    );
    // Verify nothing was installed - no symlink in the lobe.
    let skill_link = sb.claude_home.join("skills/review");
    assert!(
        !skill_link.exists(),
        "--register-only must not install items: lobe symlink exists at {skill_link:?}"
    );
}

#[test]
fn unmeld_keep_items_is_canonical() {
    // spec: CLI-166 - `--keep-items` is the canonical flag; `--unlink-only` is a
    // hidden deprecated alias that still works.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);

    let r = sb.mind(&["unmeld", "agents", "--keep-items", "--yes"]);
    assert!(r.success, "--keep-items must succeed: {}", r.stderr);

    // The item's symlink must still exist in the lobe.
    assert!(
        sb.claude_home.join("skills/review").exists()
            || sb.claude_home.join("skills/review/SKILL.md").exists(),
        "--keep-items must preserve the installed item"
    );
}

#[test]
fn mutation_result_schema_field() {
    // spec: CLI-168 - every mutation JSON result carries `"schema": 1`.
    let sb = melded();
    let r = sb.mind(&["learn", "review", "--json"]);
    assert!(r.success, "{}", r.stderr);
    let v: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("learn --json must produce JSON");
    assert_eq!(
        v["schema"], 1,
        "mutation result must carry schema:1: {}",
        r.stdout
    );
    assert!(
        v["action"].is_string(),
        "action must be present: {}",
        r.stdout
    );
}

#[test]
fn upgrade_no_sync_flag_is_accepted() {
    // spec: CLI-169 - `upgrade --no-sync` skips the pre-upgrade source fetch.
    // Verifies the flag is wired; full sync-vs-no-sync diff is covered by the
    // next test.
    let sb = melded();
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["upgrade", "--no-sync"]);
    // Either exits 0 (nothing to upgrade) or 1 (pending); must not crash with
    // "unexpected argument --no-sync".
    assert!(
        !r.stderr.contains("unexpected argument"),
        "--no-sync must be a recognized flag: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("error: unrecognized"),
        "--no-sync must be a recognized flag: {}",
        r.stderr
    );
}

#[test]
fn upgrade_syncs_before_computing_delta() {
    // spec: CLI-169 - `upgrade` syncs each source first by default; `--no-sync`
    // skips the fetch. With a cloned source (--follow-branch), after a new commit
    // in the source: `--no-sync` sees stale clone (reports up-to-date); the
    // default sync detects the change (reports pending).
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Meld as a clone (--follow-branch main makes it non-linked).
    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "main", "--yes"])
            .success,
        "meld --follow-branch failed"
    );
    assert!(sb.mind(&["learn", "review"]).success, "learn review failed");

    // Advance the source: change the SKILL.md content so the hash differs.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: updated skill\n---\n# review v2\n",
    );

    // --no-sync: the clone is stale -> no pending upgrades.
    let no_sync = sb.mind(&["upgrade", "--no-sync", "--yes"]);
    // Must succeed; no crash.
    assert!(
        !no_sync.stderr.contains("error"),
        "--no-sync must not error: {}",
        no_sync.stderr
    );

    // default sync: fetches the new commit -> detects pending upgrade.
    let with_sync = sb.mind(&["upgrade", "--yes"]);
    // Success (0) means it applied an upgrade; exit 1 is "pending, no --yes prompt
    // answered". Either way, the output must mention the item.
    assert!(
        with_sync.stdout.contains("review") || with_sync.stderr.contains("review"),
        "upgrade with sync must reference the changed item: out={} err={}",
        with_sync.stdout,
        with_sync.stderr
    );
}

#[test]
fn mind_default_lobe_env_var_overrides_claude_home() {
    // spec: CLI-170 - MIND_DEFAULT_LOBE takes precedence over CLAUDE_HOME as the
    // default agent home. Items must be linked into the MIND_DEFAULT_LOBE dir, not
    // the CLAUDE_HOME dir set by the sandbox.
    //
    // All commands use MIND_DEFAULT_LOBE so that ensure_config() (called during
    // meld) bakes alt_lobe into config.toml, not sb.claude_home.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Create an alternative lobe directory first.
    let alt_lobe = sb.mind_home.join("alt_lobe");
    std::fs::create_dir_all(&alt_lobe).expect("create alt_lobe");
    let alt_lobe_str = alt_lobe.to_str().unwrap().to_string();

    // Meld with MIND_DEFAULT_LOBE so the written config.toml records alt_lobe.
    assert!(
        sb.mind_env(
            &["meld", &spec, "--register-only"],
            &[("MIND_DEFAULT_LOBE", alt_lobe_str.as_str())]
        )
        .success,
        "register-only meld with MIND_DEFAULT_LOBE failed"
    );

    // Install with MIND_DEFAULT_LOBE still set.
    let r = sb.mind_env(
        &["learn", "review"],
        &[("MIND_DEFAULT_LOBE", alt_lobe_str.as_str())],
    );
    assert!(
        r.success,
        "learn with MIND_DEFAULT_LOBE failed: {}",
        r.stderr
    );

    // Item must be linked in alt_lobe, not the sandbox's CLAUDE_HOME.
    let in_alt =
        alt_lobe.join("skills/review").exists() || alt_lobe.join("skills/review/SKILL.md").exists();
    let in_claude = sb.claude_home.join("skills/review").exists()
        || sb.claude_home.join("skills/review/SKILL.md").exists();

    assert!(
        in_alt,
        "item must be linked in MIND_DEFAULT_LOBE: {alt_lobe:?}"
    );
    assert!(
        !in_claude,
        "item must NOT be linked in CLAUDE_HOME when MIND_DEFAULT_LOBE is set: {:?}",
        sb.claude_home
    );
}

#[test]
fn visible_aliases_present_in_help() {
    // spec: CLI-172 - `add`, `install`, `uninstall`, `update`, `search`, `list`,
    // `doctor`, `self-update` must appear as visible aliases in the top-level help.
    let sb = Sandbox::new();
    let r = sb.mind(&["--help"]);
    assert!(r.success, "--help failed: {}", r.stderr);
    let out = r.stdout;
    for alias in &[
        "add",
        "install",
        "uninstall",
        "update",
        "search",
        "list",
        "doctor",
    ] {
        assert!(
            out.contains(alias),
            "visible alias `{alias}` must appear in --help: {out}"
        );
    }
    // `detach` and `target` must NOT appear (demoted to hidden).
    assert!(
        !out.contains("detach"),
        "`detach` must not appear in --help (hidden alias): {out}"
    );
    assert!(
        !out.contains("target"),
        "`target` must not appear in --help (hidden alias): {out}"
    );
}

#[test]
fn meld_help_mentions_install_default() {
    // spec: CLI-173 - the meld one-line help must mention that it installs by default.
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", "--help"]);
    assert!(r.success, "meld --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.contains("install") || out.contains("Install"),
        "meld --help must mention install: {out}"
    );
}

#[test]
fn unmeld_long_help_leads_with_uninstall() {
    // spec: CLI-174 - the unmeld long help must lead with the uninstall default and
    // mention --keep-items.
    let sb = Sandbox::new();
    let r = sb.mind(&["unmeld", "--help"]);
    assert!(r.success, "unmeld --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.contains("uninstall") || out.contains("Uninstalls"),
        "unmeld --help must mention uninstall: {out}"
    );
    assert!(
        out.contains("--keep-items"),
        "unmeld --help must mention --keep-items: {out}"
    );
}

#[test]
fn exit_codes_distinguish_runtime_and_usage_errors() {
    // spec: CLI-175 - 0 for success, 1 for a runtime error (MindError), 2 for a
    // usage error (clap parse failure).
    let sb = Sandbox::new();

    let ok = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(ok.status.code(), Some(0), "recall in a fresh home exits 0");

    let runtime = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["learn", "no-such-item"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(
        runtime.status.code(),
        Some(1),
        "a MindError exits 1: {}",
        String::from_utf8_lossy(&runtime.stderr)
    );

    let usage = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["probe", "--definitely-not-a-flag"])
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .output()
        .expect("run mind");
    assert_eq!(
        usage.status.code(),
        Some(2),
        "a clap parse failure exits 2: {}",
        String::from_utf8_lossy(&usage.stderr)
    );
}

#[test]
fn removed_aliases_are_usage_errors() {
    // spec: CLI-172 - the former `detach` (unmeld) and `target` (config lobes)
    // aliases are removed, not hidden; both are usage errors now.
    let sb = Sandbox::new();
    let detach = sb.mind(&["detach", "whatever"]);
    assert!(
        !detach.success,
        "`mind detach` must fail: {}",
        detach.stdout
    );
    let target = sb.mind(&["config", "target", "list"]);
    assert!(
        !target.success,
        "`mind config target` must fail: {}",
        target.stdout
    );
}

#[test]
fn self_update_alias_works() {
    // spec: CLI-172 - `self-update` is a visible alias for `evolve`.
    let sb = Sandbox::new();
    // --help on the alias should succeed and show evolve content.
    let r = sb.mind(&["self-update", "--help"]);
    assert!(
        r.success || r.stderr.contains("evolve") || r.stdout.contains("evolve"),
        "self-update alias must be recognized: out={} err={}",
        r.stdout,
        r.stderr
    );
}

// ---- gap-closing tests added by qa agent ------------------------------------

#[test]
fn all_mutating_verbs_carry_schema_1_in_json_envelope() {
    // spec: CLI-168 - every mutation JSON result carries "schema": 1.
    // mutation_result_schema_field covers `learn`; this test pins the remaining
    // mutating verbs: meld, forget, sync, upgrade, and unmeld.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // meld
    let meld_r = sb.mind(&["meld", &spec, "--json"]);
    assert!(meld_r.success, "meld --json failed: {}", meld_r.stderr);
    assert_eq!(
        parse_json(&meld_r.stdout)["schema"],
        1,
        "meld --json must carry schema:1: {}",
        meld_r.stdout
    );

    // sync
    let sync_r = sb.mind(&["sync", "--json"]);
    assert!(sync_r.success, "sync --json failed: {}", sync_r.stderr);
    assert_eq!(
        parse_json(&sync_r.stdout)["schema"],
        1,
        "sync --json must carry schema:1: {}",
        sync_r.stdout
    );

    // learn then forget
    assert!(
        sb.mind(&["learn", "skill:review"]).success,
        "learn review failed"
    );
    let forget_r = sb.mind(&["forget", "skill:review", "--json"]);
    assert!(
        forget_r.success,
        "forget --json failed: {}",
        forget_r.stderr
    );
    assert_eq!(
        parse_json(&forget_r.stdout)["schema"],
        1,
        "forget --json must carry schema:1: {}",
        forget_r.stdout
    );

    // upgrade (reinstall item first; no delta case)
    assert!(
        sb.mind(&["learn", "skill:review"]).success,
        "learn review failed"
    );
    let upgrade_r = sb.mind(&["upgrade", "--json"]);
    assert!(
        upgrade_r.success,
        "upgrade --json failed: {}",
        upgrade_r.stderr
    );
    assert_eq!(
        parse_json(&upgrade_r.stdout)["schema"],
        1,
        "upgrade --json must carry schema:1: {}",
        upgrade_r.stdout
    );

    // unmeld (removes source and its installed items)
    let unmeld_r = sb.mind(&["unmeld", "agents", "--json", "--yes"]);
    assert!(
        unmeld_r.success,
        "unmeld --json failed: {}",
        unmeld_r.stderr
    );
    assert_eq!(
        parse_json(&unmeld_r.stdout)["schema"],
        1,
        "unmeld --json must carry schema:1: {}",
        unmeld_r.stdout
    );
}

#[test]
fn short_n_is_usage_error_on_meld_review_and_init_source() {
    // spec: CLI-163 - -n is reserved for --dry-run on `learn` only. The verbs
    // meld, review, and init-source do not define -n; passing it must be
    // rejected as a clap usage error (unknown flag), not accepted.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let meld_n = sb.mind(&["meld", "-n", &spec]);
    assert!(
        !meld_n.success,
        "meld -n must be rejected as an unknown flag: {}",
        meld_n.stderr
    );

    let review_n = sb.mind(&["review", "-n", &spec]);
    assert!(
        !review_n.success,
        "review -n must be rejected as an unknown flag: {}",
        review_n.stderr
    );

    let init_n = sb.mind(&["init-source", "-n"]);
    assert!(
        !init_n.success,
        "init-source -n must be rejected as an unknown flag: {}",
        init_n.stderr
    );

    // learn -n is still accepted as --dry-run (the reserved use).
    assert!(sb.mind(&["meld", &spec]).success, "meld without -n failed");
    let learn_n = sb.mind(&["learn", "-n", "skill:review"]);
    assert!(
        learn_n.success,
        "learn -n must still work as --dry-run: {}",
        learn_n.stderr
    );
}

#[test]
fn deprecated_flag_aliases_are_hidden_from_help() {
    // spec: CLI-165 - --link-only is a hidden deprecated alias for
    // --register-only; it must NOT appear in `meld --help` output.
    // spec: CLI-166 - --unlink-only is a hidden deprecated alias for
    // --keep-items; it must NOT appear in `unmeld --help` output.
    // The canonical names must still be visible in their respective help text.
    let sb = Sandbox::new();

    let meld_help = sb.mind(&["meld", "--help"]).stdout;
    assert!(
        !meld_help.contains("link-only"),
        "--link-only must not appear in meld --help (hidden alias): {meld_help}"
    );
    assert!(
        meld_help.contains("register-only"),
        "--register-only must appear in meld --help (canonical name): {meld_help}"
    );

    let unmeld_help = sb.mind(&["unmeld", "--help"]).stdout;
    assert!(
        !unmeld_help.contains("unlink-only"),
        "--unlink-only must not appear in unmeld --help (hidden alias): {unmeld_help}"
    );
    assert!(
        unmeld_help.contains("keep-items"),
        "--keep-items must appear in unmeld --help (canonical name): {unmeld_help}"
    );
}

#[test]
fn verb_aliases_dispatch_to_correct_commands() {
    // spec: CLI-172 - aliases must not merely appear in --help; they must
    // dispatch the correct command and produce real output. Tests the verbs
    // most likely to silently mis-route: add, install, list, search, update,
    // doctor, uninstall.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // `add` -> meld: registers the source.
    let add_r = sb.mind(&["add", &spec]);
    assert!(
        add_r.success,
        "mind add must dispatch to meld: {}",
        add_r.stderr
    );
    assert!(
        sb.mind(&["recall", "--sources"]).stdout.contains("agents"),
        "mind add must register the source"
    );

    // `install` -> learn: installs an item.
    let install_r = sb.mind(&["install", "skill:review"]);
    assert!(
        install_r.success,
        "mind install must dispatch to learn: {}",
        install_r.stderr
    );

    // `list` -> recall: lists installed items.
    let list_r = sb.mind(&["list"]);
    assert!(
        list_r.success,
        "mind list must dispatch to recall: {}",
        list_r.stderr
    );
    assert!(
        list_r.stdout.contains("skill:review"),
        "mind list must show the installed item: {}",
        list_r.stdout
    );

    // `search` -> probe: lists catalog items (--no-tui forces text output in non-TTY).
    let search_r = sb.mind(&["search", "--no-tui"]);
    assert!(
        search_r.success,
        "mind search must dispatch to probe: {}",
        search_r.stderr
    );
    assert!(
        search_r.stdout.contains("review"),
        "mind search must list catalog items: {}",
        search_r.stdout
    );

    // `update` -> sync: syncs sources without error.
    let update_r = sb.mind(&["update"]);
    assert!(
        update_r.success,
        "mind update must dispatch to sync: {}",
        update_r.stderr
    );

    // `doctor` -> introspect: reports health diagnostics.
    let doctor_r = sb.mind(&["doctor"]);
    assert!(
        doctor_r.success,
        "mind doctor must dispatch to introspect: {}",
        doctor_r.stderr
    );

    // `uninstall` -> forget: removes the installed item (no more "installed @" marker).
    let uninstall_r = sb.mind(&["uninstall", "skill:review"]);
    assert!(
        uninstall_r.success,
        "mind uninstall must dispatch to forget: {}",
        uninstall_r.stderr
    );
    let recall_after = sb.mind(&["recall"]).stdout;
    assert!(
        !recall_after.contains("installed @"),
        "mind uninstall must have removed skill:review (no installed-at marker): {recall_after}"
    );
}

#[test]
fn sync_upgrade_is_noted_as_deprecated_in_help() {
    // spec: CLI-169 - `sync --upgrade` continues to work but the --upgrade flag
    // is noted as deprecated in help text; prefer `upgrade` which now syncs
    // first by default.
    let sb = Sandbox::new();
    let r = sb.mind(&["sync", "--help"]);
    assert!(r.success, "sync --help failed: {}", r.stderr);
    let out = r.stdout + &r.stderr;
    assert!(
        out.to_lowercase().contains("deprecated") || out.to_lowercase().contains("prefer"),
        "sync --help must note that --upgrade is deprecated: {out}"
    );
}

#[test]
fn upgrade_sync_first_outcome_with_drift_example() {
    // spec: CLI-169 - upgrade fetches each involved source before computing
    // deltas (sync-first). With a cloned source and a new upstream commit that
    // the clone has not yet fetched:
    //   --no-sync: clone is stale -> item appears up-to-date (no delta seen).
    //   default (sync-first): fetches the commit -> detects and applies upgrade.
    //
    // Uses from_example("drift") to drive a real-world-ish source layout
    // (skills/audit/SKILL.md) and --follow-branch to create an actual git clone
    // so the sync path runs a real fetch.
    let sb = Sandbox::from_example("drift");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "main", "--register-only"])
            .success,
        "meld --follow-branch failed"
    );
    assert!(sb.mind(&["learn", "audit"]).success, "learn audit failed");

    // Advance the source without syncing the clone.
    write(
        &sb.source.join("skills/audit/SKILL.md"),
        "---\nname: audit\ndescription: Updated audit skill\n---\n# audit v2 body\n",
    );
    git(&sb.source, &["commit", "-aqm", "update audit"]);

    // --no-sync: clone is stale, so the installed hash still matches the old content.
    let no_sync = sb.mind(&["upgrade", "--no-sync", "--json"]);
    assert!(
        no_sync.success,
        "--no-sync must succeed: {}",
        no_sync.stderr
    );
    let ns = parse_json(&no_sync.stdout);
    assert_eq!(
        ns["outcome"], "up-to-date",
        "--no-sync on stale clone must report up-to-date (no delta visible from old clone): {}",
        no_sync.stdout
    );

    // Default upgrade (sync-first): fetches the new commit then upgrades.
    let with_sync = sb.mind(&["upgrade", "--yes", "--json"]);
    assert!(
        with_sync.success,
        "upgrade with sync must succeed: {}",
        with_sync.stderr
    );
    let ws = parse_json(&with_sync.stdout);
    assert_eq!(
        ws["outcome"], "upgraded",
        "upgrade with sync must detect and apply the upstream change: {}",
        with_sync.stdout
    );

    // After the sync-first upgrade the item is current (not outdated).
    let after = sb.mind(&["recall"]);
    assert!(
        !after.stdout.contains("outdated"),
        "item must be current after sync-first upgrade: {}",
        after.stdout
    );
}

// ---- helper: fake git that emits a proxy 407 on https:// clone ----------
