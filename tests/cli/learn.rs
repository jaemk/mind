//! `learn` (spec/cli.md).

use super::*;

#[test]
fn learn_of_a_source_name_hints_at_learn_all() {
    // spec: CLI-208 - `learn <source-name>` is a common miss: it reads like
    // `meld`'s `<repo>` argument, and the `install` alias trains that habit.
    // The plain ItemNotFound dead-ends without naming `--all`, which is what
    // the user wanted.
    let sb = melded();
    // The identity is `local/<base_name>/agents`; `agents` is the unambiguous
    // trailing suffix a user would actually type.
    let source = format!("local/{}/agents", sb.base_name());
    let r = sb.mind(&["learn", &source]);
    assert!(!r.success, "learn of a source name is still an error");
    assert!(
        r.stderr.contains("--all") && r.stderr.contains(&source),
        "the hint must name the source and `learn --all`: {}",
        r.stderr
    );
}

#[test]
fn learn_installs_and_creates_symlink() {
    // spec: CLI-30, STO-2, STO-14, LIFE-5
    let sb = melded();
    let r = sb.mind(&["learn", "review"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("learned skill:review"));

    let link = sb.claude_home.join("skills/review");
    let meta = std::fs::symlink_metadata(&link).expect("symlink should exist");
    assert!(
        meta.file_type().is_symlink(),
        "expected a symlink at {link:?}"
    );
}

#[test]
fn learn_force_overwrites_a_conflicting_target() {
    // spec: CLI-35, LIFE-41
    let sb = melded();
    // Plant a user file where the rule `style` would link.
    let link = sb.claude_home.join("rules/style.md");
    write(&link, "the user's own file\n");

    // Without --force, the clobber guard refuses (non-TTY: no prompt, no change).
    let r = sb.mind(&["learn", "style"]);
    assert!(
        !r.success,
        "learn must refuse to clobber a foreign target: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("not managed by mind"),
        "expected a clobber error: {}",
        r.stderr
    );
    assert!(
        !std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the user's file must be left untouched without --force"
    );

    // With --force, the target is replaced by mind's symlink.
    let f = sb.mind(&["learn", "style", "--force"]);
    assert!(
        f.success,
        "learn --force should overwrite: {} {}",
        f.stdout, f.stderr
    );
    assert!(f.stdout.contains("learned rule:style"), "{}", f.stdout);
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("link should exist")
            .file_type()
            .is_symlink(),
        "--force must replace the file with mind's symlink"
    );
}

#[test]
fn overwrite_accepts_every_conflict_of_a_meld_and_a_learn() {
    // spec: CLI-246
    let plant = |sb: &Sandbox| {
        write(&sb.claude_home.join("rules/style.md"), "the user's rule\n");
        write(
            &sb.claude_home.join("skills/review/SKILL.md"),
            "the user's skill\n",
        );
    };
    let is_link = |p: std::path::PathBuf| {
        std::fs::symlink_metadata(&p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    };

    // Without the flag, a non-TTY meld refuses the first conflict.
    let sb = Sandbox::new();
    plant(&sb);
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(
        !r.success,
        "meld must refuse a foreign target: {}",
        r.stdout
    );
    assert!(r.stderr.contains("not managed by mind"), "{}", r.stderr);
    assert!(!is_link(sb.claude_home.join("rules/style.md")));

    // With --overwrite, every conflict is replaced in one run.
    let sb = Sandbox::new();
    plant(&sb);
    let r = sb.mind(&["meld", &sb.source_spec(), "--yes", "--overwrite"]);
    assert!(r.success, "meld --overwrite: {} {}", r.stdout, r.stderr);
    assert!(is_link(sb.claude_home.join("rules/style.md")));
    assert!(is_link(sb.claude_home.join("skills/review")));

    // learn takes the same flag.
    let sb = melded();
    plant(&sb);
    let r = sb.mind(&["learn", "style", "--overwrite"]);
    assert!(r.success, "learn --overwrite: {} {}", r.stdout, r.stderr);
    assert!(is_link(sb.claude_home.join("rules/style.md")));
}

#[test]
fn remeld_overwrite_does_not_rerun_hooks_that_already_ran() {
    // spec: CLI-246 HOOK-60
    let sb = Sandbox::bare("remeld-overwrite");
    let marker = sb.base.join("hook-ran");
    let m = marker.to_str().unwrap().to_owned();
    sb.write_and_commit(
        "mind.toml",
        &format!("[[hooks]]\nrun = \"touch {m}\"\nevent = \"install\"\n"),
    );
    let spec = sb.source_spec();
    let meld = |extra: &str| {
        let mut args = vec![
            "meld",
            spec.as_str(),
            "--link-only",
            "--accept-hooks=install,update,uninstall",
        ];
        if !extra.is_empty() {
            args.push(extra);
        }
        assert!(sb.mind(&args).success);
    };
    meld("");
    assert!(marker.exists(), "the first meld runs the hook");
    std::fs::remove_file(&marker).unwrap();
    meld("--overwrite");
    assert!(
        !marker.exists(),
        "--overwrite must not re-run a hook already run at this commit"
    );
}

#[test]
fn learn_glob_installs_all_matches() {
    // spec: CLI-31
    let sb = melded();
    assert!(sb.mind(&["learn", "*"]).success);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:dev"), "{recall}");
    assert!(recall.contains("rule:style"), "{recall}");
}

#[test]
fn learn_kind_glob_limits_to_kind() {
    // spec: CLI-31
    let sb = melded();
    assert!(sb.mind(&["learn", "skill:*"]).success);
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "skill installed"
    );
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "agent not installed by a skill glob"
    );
}

#[test]
fn learn_all_flag_installs_whole_source() {
    // spec: CLI-36
    // `--all` is sugar for the `<source>#*` selector: every item of the source
    // installs, equivalent to `learn 'agents#*'`.
    let sb = melded();
    let r = sb.mind(&["learn", "agents", "--all"]);
    assert!(r.success, "{}", r.stderr);
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "{recall}");
    assert!(recall.contains("agent:dev"), "{recall}");
    assert!(recall.contains("rule:style"), "{recall}");
}

#[test]
fn learn_all_flag_rejects_ref_with_hash() {
    // spec: CLI-36
    // Combining `--all` with a ref that already names an item is rejected; the
    // doubled selector is an invalid ref and nothing installs.
    let sb = melded();
    let r = sb.mind(&["learn", "agents#review", "--all"]);
    assert!(!r.success, "expected failure: {}", r.stdout);
    assert!(
        !sb.mind(&["recall", "skill:review"]).success,
        "nothing installed"
    );
}

#[test]
fn learn_dry_run_installs_nothing() {
    // spec: CLI-32
    let sb = melded();
    let r = sb.mind(&["learn", "*", "--dry-run"]);
    assert!(r.success, "{}", r.stderr);
    assert!(r.stdout.contains("would learn"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skill:review"),
        "plan should list items: {}",
        r.stdout
    );
    // Nothing was actually installed.
    assert!(!sb.mind(&["recall"]).stdout.contains("installed @"));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/review")).is_err());
}

#[test]
fn learn_glob_collision_errors_and_installs_nothing() {
    // spec: CLI-33
    let a = Sandbox::new();
    let b = Sandbox::new(); // same item names, different source
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success);

    // '*' matches review/dev/style from both sources -> same install names collide.
    let r = a.mind(&["learn", "*"]);
    assert!(!r.success);
    assert!(r.stderr.contains("ambiguous"), "{}", r.stderr);
    assert!(!a.mind(&["recall"]).stdout.contains("installed @"));
}

#[test]
fn learn_source_and_kind_glob_compose() {
    // spec: CLI-31
    let sb = melded();
    // All skills of this source: review only (fixture has one skill).
    assert!(sb.mind(&["learn", "agents#skill:*"]).success);
    assert!(
        sb.mind(&["recall", "skill:review"]).success,
        "skill installed"
    );
    assert!(
        !sb.mind(&["recall", "agent:dev"]).success,
        "agent not installed by a skill glob"
    );
}

#[test]
fn learn_partial_failure_persists_successes() {
    // spec: CLI-34
    let sb = Sandbox::new();
    // A skill that sorts after `review` (so review installs first) and has a
    // broken reference, so the batch installs one item and then fails.
    sb.write_and_commit(
        "skills/zzz/SKILL.md",
        "---\ndescription: bad\n---\nsee {{ns:ghost}}\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["learn", "skill:*"]);
    assert!(!r.success, "should fail on the bad reference");
    assert!(r.stderr.contains("does not match any item"), "{}", r.stderr);

    // The item installed before the failure is recorded in the manifest.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(
        recall.contains("skill:review"),
        "successes should persist: {recall}"
    );
    // And the manifest matches disk: introspect finds no missing-link issues.
    let ins = sb.mind(&["introspect"]).stdout;
    assert!(
        !ins.contains("symlink missing"),
        "manifest/disk drift: {ins}"
    );
}

/// Re-learning an already-installed item must print a human-readable signal
/// and, under --json, use the distinct "up-to-date" outcome rather than
/// "installed". This lets callers distinguish a real install from a no-op.
#[test]
fn relearn_already_installed_signals_noop() {
    // spec: CLI-157 DEP-23
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // First learn succeeds.
    let r1 = sb.mind(&["meld", &spec, "--yes"]);
    assert!(r1.success, "initial meld+learn must succeed: {}", r1.stderr);

    // Human mode: re-learn prints a recognizable noop message.
    let r2 = sb.mind(&["learn", "skill:review"]);
    assert!(
        r2.success,
        "re-learn must still exit 0: stdout={} stderr={}",
        r2.stdout, r2.stderr
    );
    assert!(
        r2.stdout.contains("already installed") || r2.stdout.contains("nothing to do"),
        "re-learn human output must signal noop: '{}'",
        r2.stdout
    );

    // JSON mode: outcome is "up-to-date", NOT "installed".
    let r3 = sb.mind(&["--json", "learn", "skill:review"]);
    assert!(
        r3.success,
        "re-learn --json must exit 0: stdout={} stderr={}",
        r3.stdout, r3.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r3.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "re-learn --json stdout must be one valid JSON object, got error {e}: '{}'",
            r3.stdout
        )
    });
    assert_eq!(
        v["outcome"], "up-to-date",
        "json outcome for re-learn must be 'up-to-date', not 'installed': {v}"
    );
}

// ---- DSC-74..77 / STO-44 / CLI-158 / DUMP-10: flat skill layout -------------

#[test]
fn learn_not_found_with_sources_hints_probe_not_sync() {
    // spec: CLI-179 -- when sources are melded and the item is unknown, the
    // error and hint point at `mind probe` and never mention `mind sync`.
    let sb = melded();
    let r = sb.mind(&["learn", "does-not-exist"]);
    assert!(!r.success, "learn must fail for unknown item: {}", r.stderr);
    // spec: CLI-225 -- the query is shell-quoted before landing in the
    // pasteable `mind probe <query>` remedy (see the CLI-225 tests above).
    assert!(
        r.stderr.contains("probe 'does-not-exist'"),
        "hint must point at `mind probe <query>`: {}",
        r.stderr
    );
    // Neither the ItemNotFound Display nor the hint line may mention sync.
    assert!(
        !r.stderr.contains("sync"),
        "with sources, no line in stderr may mention sync: {}",
        r.stderr
    );
}
