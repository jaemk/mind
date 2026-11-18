//! `review` (spec/cli.md).

use super::*;

/// Run `mind` with BOTH a working directory and extra environment variables.
/// `Sandbox::mind_cwd` sets only the cwd and `Sandbox::mind_env` only the env;
/// the CLI-215 shadow-note test needs a cwd (to shadow the spec) and a stubbed
/// `git` on PATH (so the clone that follows the note can never reach a remote).
fn mind_in(sb: &Sandbox, args: &[&str], cwd: &Path, envs: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
    cmd.args(args)
        .current_dir(cwd)
        .env("MIND_HOME", &sb.mind_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run mind");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        success: out.status.success(),
    }
}

#[test]
fn review_reads_a_relative_two_segment_directory_as_a_local_path() {
    // spec: CLI-214 -- the headline case. `skills/greet` parses as the remote
    // spec `owner/repo`, so without the existing-directory check `review`
    // shallow-clones it from github.com. The unit coverage passes an ABSOLUTE
    // directory, which never had that ambiguity; only a relative two-segment
    // path exercises the branch the spec is about. A stub `git` that refuses
    // every https clone keeps a regression here from reaching the network.
    let sb = Sandbox::bare("cli214");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("workdir");
    write(
        &workdir.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet the user\n---\n# greet\n",
    );

    let r = mind_in(
        &sb,
        &["review", "skills/greet"],
        &workdir,
        &[("PATH", &new_path)],
    );
    assert!(
        r.success,
        "review of an existing relative directory must succeed locally: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !format!("{}{}", r.stdout, r.stderr).contains("Authentication failed"),
        "review must not have attempted a clone: {} {}",
        r.stdout,
        r.stderr
    );
    assert!(
        r.stderr.contains("reviewed as that local path")
            && r.stderr.contains("mind review 'github:skills/greet'"),
        "review must say which reading it took and how to force the remote one: {}",
        r.stderr
    );
    assert_no_review_temp(&sb.mind_home);
}

#[test]
fn review_of_a_shadowed_directory_emits_only_the_note_for_the_reading_it_took() {
    // `shadow_note` builds its message from `parse_spec_quiet`, not `parse_spec`,
    // whose bare `owner/repo` branch prints the CLI-215 note on this exact shape.
    // Routed through the printing parse, `mind review skills/greet` printed BOTH,
    // and CLI-215's line ("to meld/review it as a local path instead of a remote
    // repo, use './skills/greet'") told the user to do the thing `review` had
    // already done, immediately before the CLI-214 line said so.
    // spec: CLI-214 CLI-215 CLI-216
    let sb = Sandbox::bare("cli214-notes");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("workdir");
    write(
        &workdir.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet the user\n---\n# greet\n",
    );

    let r = mind_in(
        &sb,
        &["review", "skills/greet"],
        &workdir,
        &[("PATH", &new_path)],
    );
    assert!(
        r.stderr.contains("reviewed as that local path"),
        "the CLI-214 note must be present: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("use './skills/greet'"),
        "the CLI-215 note contradicts the reading review actually took: {}",
        r.stderr
    );
}

#[test]
fn a_curated_entry_that_shadows_a_local_directory_is_noted_exactly_once() {
    // CLI-216's rule is "a caller that parses a spec to ANSWER a question rather
    // than to act on it must use `parse_spec_quiet`". These callers all parse an
    // already-decided `[discover].sources` entry purely to derive its identity,
    // and every one of them used to print:
    //   src/commands/  the DSC-68/69 auth-failure arm  (fires below)
    //   src/commands/meld.rs  install_curated_sources' walk   (fires below)
    //   src/commands/  the DSC-79 clone-failure arm    (same shape)
    //   src/commands/  the DSC-63 install_items check  (same shape)
    //   src/commands/  the json twin of the walk       (same shape)
    // Concretely: standing in a directory that contains `skills/greet`, melding
    // a curator whose entry is `source = "skills/greet"` printed
    //   note: 'skills/greet' is also a directory here; ... use './skills/greet'
    // THREE times (once from the real recursion, once from the failure arm's
    // identity parse, once from the install walk) -- and every copy is advice
    // the user cannot act on, because the reading is the curator's, not theirs.
    // With `--register-only` it was twice. The DSC-93 predicate was never one of
    // them: it already used `parse_spec_quiet`, which is why the count was 3 and
    // not 4.
    //
    // Exactly one is the right number: zero would mean CLI-215 stopped warning
    // before a curated clone at all.
    // spec: CLI-215 CLI-216 DSC-93
    let sb = Sandbox::named("shadow-curator");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"review\"\npath = \"skills/review\"\n\n\
         [[discover.sources]]\nsource = \"skills/greet\"\n\
         on-auth-failure = { action = \"skip\" }\n",
    );

    // Stand in a directory where `skills/greet` names a real directory, so the
    // curated entry shadows it. A stubbed `git` refuses every https clone, so
    // the remote reading the note is about can never reach the network.
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("standhere");
    write(
        &workdir.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: a decoy that shadows the curated entry\n---\n# greet\n",
    );

    let spec = sb.source_spec();
    let r = mind_in(
        &sb,
        &["meld", &spec, "--yes"],
        &workdir,
        &[("PATH", &new_path)],
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert_eq!(
        combined.matches("is also a directory here").count(),
        1,
        "a curated entry that shadows a local directory must be noted exactly \
         once (the recursion's real parse), not once more by the DSC-93 \
         classification: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
}

#[test]
fn a_top_level_shadowing_spec_is_noted_exactly_once() {
    // The same CLI-216 defect one level UP, on the plainest invocation there is:
    // `mind meld skills/greet` standing in a directory that contains
    // `skills/greet`. Nothing curated, nothing nested. Four parses of the user's
    // own spec ran before the clone, and all four printed:
    //   main.rs   `instance_name` (for the post-meld install target)
    //   main.rs   `is_melded` -> `instance_name` (the fresh/re-meld routing)
    //   commands/meld.rs `meld`'s identity parse (STO-58 source_name)
    //   commands/meld.rs `meld_recursive`'s parse -- the one that actually clones
    // Only the last decides a reading, so the first three are CLI-216
    // "answering a question" parses. Three passes over this class each looked
    // only at the curated/nested sites and missed this one, which is the one an
    // ordinary user hits.
    //
    // A stubbed `git` refuses every https clone, so the remote reading the note
    // is about never reaches the network; the meld fails, which is fine -- the
    // note is printed before the clone (CLI-215) and its count is what is under
    // test.
    // spec: CLI-215 CLI-216
    let sb = Sandbox::bare("top-shadow");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("standhere");
    write(
        &workdir.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: a decoy that shadows the spec\n---\n# greet\n",
    );

    let r = mind_in(
        &sb,
        &["meld", "skills/greet", "--register-only"],
        &workdir,
        &[("PATH", &new_path)],
    );
    let combined = format!("{}{}", r.stdout, r.stderr);
    assert_eq!(
        combined.matches("is also a directory here").count(),
        1,
        "a shadowing top-level spec must be noted exactly once (by the parse \
         that decides what to clone), not once per identity lookup: stdout={} \
         stderr={}",
        r.stdout,
        r.stderr
    );
    assert!(
        combined.contains("use './skills/greet'"),
        "the one note must still name the local-path form (CLI-215): {combined}"
    );
}

#[test]
fn review_registry_selector_still_beats_a_same_named_local_directory() {
    // spec: CLI-214 -- "BETWEEN the registry-selector match and repo-spec
    // parsing". The ordering is load-bearing and untested: if the
    // existing-directory check ran first, `mind review agents` in a directory
    // that happens to contain an unrelated `agents/` folder would silently
    // review the wrong tree instead of the melded source of that name.
    let sb = melded(); // registers `local/<base>/agents`, selector `agents`.
    // A decoy directory of the same name whose `mind.toml` is malformed, so
    // reviewing IT is a hard error. The melded source has no mind.toml and
    // reviews clean, making the two readings trivially distinguishable by exit
    // status alone.
    let decoy = sb.base.join("decoy");
    write(
        &decoy.join("agents/mind.toml"),
        "this is not = = toml {{{\n",
    );
    write(
        &decoy.join("agents/skills/impostor/SKILL.md"),
        "---\nname: impostor\ndescription: not the melded source\n---\n# impostor\n",
    );

    let r = mind_in(&sb, &["review", "agents"], &decoy, &[]);
    assert!(
        r.success,
        "the registry match must win over the same-named local directory (reviewing the decoy \
         would hard-fail on its malformed mind.toml): {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !format!("{}{}", r.stdout, r.stderr).contains("impostor"),
        "the decoy's items must never be reviewed: {} {}",
        r.stdout,
        r.stderr
    );

    // The decoy IS reachable, just not by the registry selector: naming it as
    // an explicit relative path takes the CLI-214 local reading and hard-fails.
    let d = mind_in(&sb, &["review", "./agents"], &decoy, &[]);
    assert!(
        !d.success,
        "control: the decoy directory really is a broken source: {} {}",
        d.stdout, d.stderr
    );
}

#[test]
fn meld_notes_a_shadowing_local_directory_before_it_clones() {
    // spec: CLI-215 -- `parse_spec`'s bare `owner/repo` branch warns, BEFORE
    // any clone, that the spec also names a directory here, and still takes the
    // remote reading. Only the pure `local_dir_shadow` helper was covered; the
    // note itself (emitted from inside `parse_spec`) was not. The stub `git`
    // fails every https clone, so the remote reading this test proves `meld`
    // still takes never leaves the machine.
    let sb = Sandbox::bare("cli215");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("workdir");
    write(
        &workdir.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet the user\n---\n# greet\n",
    );

    let r = mind_in(
        &sb,
        &["meld", "skills/greet"],
        &workdir,
        &[("PATH", &new_path)],
    );
    assert!(
        r.stderr.contains("is also a directory here") && r.stderr.contains("'./skills/greet'"),
        "meld must note the shadowing directory and the './' form: {}",
        r.stderr
    );
    // The reading is unchanged: it still tries the REMOTE repo (which the stub
    // git refuses), and nothing local is registered as a side effect.
    assert!(
        !r.success,
        "the remote reading must still be taken (and fail against the stub git): {} {}",
        r.stdout, r.stderr
    );
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("greet"),
        "nothing may be registered from the failed remote reading: {}",
        sources.stdout
    );
}

#[test]
fn meld_prints_no_shadow_note_when_no_such_directory_exists() {
    // spec: CLI-215 -- the note is conditional on the directory existing. A
    // note printed unconditionally would be noise on every ordinary
    // `mind meld owner/repo`, so the negative case is part of the statement.
    let sb = Sandbox::bare("cli215-quiet");
    let fake_dir = fake_git_bin_dir(&sb.base);
    let new_path = prepend_path(&fake_dir);
    let workdir = sb.base.join("empty-workdir");
    std::fs::create_dir_all(&workdir).unwrap();

    let r = mind_in(
        &sb,
        &["meld", "skills/greet"],
        &workdir,
        &[("PATH", &new_path)],
    );
    assert!(
        !r.stderr.contains("is also a directory here"),
        "no shadow note when nothing is shadowed: {}",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// DSC-92: a curated `[discover].sources` entry's relative local path resolves
// against the curator's own directory, end to end through meld and sync.
// ---------------------------------------------------------------------------

#[test]
fn review_with_no_target_reviews_the_current_directory() {
    // spec: CLI-26 - `mind review` with no <target> validates the cwd.
    let sb = Sandbox::new();
    let r = sb.mind_cwd(&["review"], &sb.source);
    assert!(
        r.success,
        "a bare `review` of the current directory should succeed for a clean source: {} {}",
        r.stdout, r.stderr
    );
}

#[test]
fn review_clean_local_path_exits_zero() {
    // A clean local source (valid mind.toml if present, items with descriptions,
    // no bad tokens) exits 0 with no blocking issues.
    // spec: CLI-130, CLI-131
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "clean source should exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("no issues") || r.stdout.contains("publishable") || r.stderr.is_empty(),
        "expected clean report: stdout={} stderr={}",
        r.stdout,
        r.stderr
    );
    // review must not leave any trace in the registry.
    assert!(
        sb.mind(&["recall", "--sources"])
            .stdout
            .contains("no sources melded"),
        "review must not register anything"
    );
}

#[test]
fn review_malformed_mind_toml_exits_nonzero() {
    // A malformed mind.toml is a hard error -> exit non-zero.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit("mind.toml", "[[[[bad toml");
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "malformed mind.toml must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn review_unknown_item_kind_exits_nonzero() {
    // An [[items]] entry with an unknown kind is a hard error -> exit non-zero.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"spell\"\nname = \"x\"\npath = \"x.md\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "unknown kind must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("unknown-kind") || r.stderr.contains("unknown item kind"),
        "expected unknown-kind in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_bad_ns_token_exits_nonzero() {
    // An item with {{ns:nope}} that doesn't resolve to any sibling is hard.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:nope}}.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "bad ns token must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("bad-reference") || r.stderr.contains("does not resolve"),
        "expected bad-reference in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_conflicting_pin_exits_nonzero() {
    // A [source] section with two conflicting pin directives is a hard error.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        "[source]\nfollow-branch = \"main\"\npin-tag = \"v1.0\"\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec]);
    assert!(
        !r.success,
        "conflicting pin must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("conflicting-pin") || r.stderr.contains("conflicting pin"),
        "expected conflicting-pin in output: stderr={}",
        r.stderr
    );
}

#[test]
fn review_missing_description_is_advisory_exit_zero() {
    // An item with no description is advisory only -> exit 0 with finding printed.
    // spec: CLI-132
    let sb = Sandbox::new();
    sb.write_and_commit("agents/nodesc.md", "# no frontmatter here\nsome content\n");
    // Remove the default fixture items so only nodesc.md is in the source.
    let source_dir = sb.source.clone();
    std::fs::remove_dir_all(source_dir.join("skills")).ok();
    std::fs::remove_dir_all(source_dir.join("rules")).ok();
    std::fs::remove_file(source_dir.join("agents/dev.md")).ok();
    git(&source_dir, &["add", "-A"]);
    git(&source_dir, &["commit", "-qm", "nodesc only"]);

    let spec = sb.source_spec();
    let r = sb.mind(&["review", &spec]);
    assert!(
        r.success,
        "missing description is advisory, must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("missing-description") || r.stdout.contains("advisory"),
        "expected advisory finding in stdout: {}",
        r.stdout
    );
}

#[test]
fn review_unguarded_ref_under_as_is_advisory_exit_zero() {
    // An unguarded prose reference under --as <prefix> is advisory -> exit 0.
    // spec: CLI-132, CLI-133
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to the dev agent.\n",
    );
    let spec = sb.source_spec();

    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    assert!(
        r.success,
        "unguarded ref advisory must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("unguarded-reference") || r.stdout.contains("advisory"),
        "expected advisory finding: stdout={}",
        r.stdout
    );
    // No hard errors.
    assert!(
        !r.stderr.contains("error ["),
        "must have no hard errors: stderr={}",
        r.stderr
    );
}

#[test]
fn review_melded_selector_resolves_via_registry() {
    // `review <melded-selector>` resolves the target via the registry.
    // spec: CLI-130
    let sb = melded();

    // After meld, "agents" (the repo basename) is a registered suffix selector.
    let r = sb.mind(&["review", "agents"]);
    assert!(
        r.success,
        "review via registry selector must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
}

#[test]
fn review_with_prefix_flag_evaluates_under_that_namespace() {
    // `review --as <prefix>` evaluates under the prospective prefix.
    // The source has a good token {{ns:dev}} that expands fine under prefix 'jk'.
    // spec: CLI-133
    let sb = Sandbox::new();
    sb.write_and_commit(
        "agents/lead.md",
        "---\ndescription: lead\n---\nDelegate to {{ns:dev}}.\n",
    );
    let spec = sb.source_spec();

    // With --as jk: the token {{ns:dev}} should resolve to jk:dev (a sibling).
    let r = sb.mind(&["review", &spec, "--as", "jk"]);
    // dev is a sibling, so no bad-reference error.
    assert!(
        r.success,
        "valid ns token with prefix must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // No bad-reference hard error.
    assert!(
        !r.stderr.contains("bad-reference"),
        "valid token must not produce bad-reference: stderr={}",
        r.stderr
    );
}

#[test]
fn review_target_and_policy_are_mutually_exclusive() {
    // spec: CLI-134
    // Supplying both <target> and --policy is a clap usage error: exits non-zero
    // and prints a conflict diagnostic before any logic runs.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // The policy file need not exist; clap rejects the combination before any
    // I/O is attempted.
    let policy_path = sb.base.join("policy.toml").to_string_lossy().into_owned();
    let r = sb.mind(&["review", &spec, "--policy", &policy_path]);
    assert!(
        !r.success,
        "review with both <target> and --policy must exit non-zero: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("cannot be used with"),
        "clap conflict diagnostic must appear in stderr: {}",
        r.stderr
    );
}

#[test]
fn review_flags_tooling_references() {
    // spec: CLI-135 CLI-136 CLI-137
    let sb = Sandbox::bare("agents");
    // A shared tool so `detect` is a sibling tool (and {{tools:nope}} stays bad).
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\n\
         run {{tools:nope}} .\n\
         also ~/.claude/skills/review/resources/pr.py\n\
         mention the detect tool\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);

    assert!(
        !r.success,
        "an unresolved path token is a hard error: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("bad-reference"),
        "expected a bad-reference hard finding: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("hardcoded-path") && r.stdout.contains("{{self}}/resources/pr.py"),
        "expected a hardcoded-path advisory suggesting the token: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("bare-tool-reference"),
        "expected a bare-tool-reference advisory: {}",
        r.stdout
    );
}

#[test]
fn review_hardcoded_path_classifies_and_detects_env_forms() {
    // spec: CLI-145 CLI-136
    let sb = Sandbox::bare("agents");
    write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
    write(
        &sb.source.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: review\n---\n\
         own ~/.claude/skills/review/resources/pr.py\n\
         tool $HOME/.mind/store/tool/detect/detect run\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "advisory-only review exits zero: {} {}",
        r.stdout, r.stderr
    );
    // Own-resource reference: "works but assumes install location" wording + the
    // {{self}} suggestion that generalizes it (CLI-145).
    assert!(
        r.stdout.contains("hardcodes its own resource path")
            && r.stdout.contains("this works but assumes")
            && r.stdout.contains("{{self}}/resources/pr.py"),
        "own-resource classification: {}",
        r.stdout
    );
    // Shared-tool reference, written with the $HOME spelling: store-only wording
    // + {{tools:}} suggestion, proving the extended form is detected too.
    assert!(
        r.stdout.contains("hardcodes a shared tool path")
            && r.stdout.contains("will not resolve")
            && r.stdout.contains("{{tools:detect}}"),
        "shared-tool classification via $HOME form: {}",
        r.stdout
    );
}

#[test]
fn review_flags_helper_script_duplicated_across_items() {
    // spec: CLI-144
    let sb = Sandbox::bare("agents");
    // Two skills ship the same helper script verbatim; it should be a tool.
    write(
        &sb.source.join("skills/a/SKILL.md"),
        "---\nname: a\ndescription: a\n---\n# a\n",
    );
    write(
        &sb.source.join("skills/a/helper.sh"),
        "#!/bin/sh\necho shared\n",
    );
    write(
        &sb.source.join("skills/a/only.sh"),
        "#!/bin/sh\necho unique\n",
    );
    write(
        &sb.source.join("skills/b/SKILL.md"),
        "---\nname: b\ndescription: b\n---\n# b\n",
    );
    write(
        &sb.source.join("skills/b/helper.sh"),
        "#!/bin/sh\necho shared\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(
        r.success,
        "an advisory-only review exits zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("duplicate-tooling") && r.stdout.contains("helper.sh"),
        "expected a duplicate-tooling advisory naming the file: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:a") && r.stdout.contains("skill:b"),
        "the finding names both carriers: {}",
        r.stdout
    );
    // CLI-144: the message is non-prescriptive - keeping the per-item copies is a
    // valid choice (siloing a helper with its skill), not a defect to fix.
    assert!(
        r.stdout.contains("both are valid"),
        "duplicate-tooling must frame the copy as an optional, valid choice: {}",
        r.stdout
    );
    // A script that lives under only one item is not flagged.
    assert!(
        !r.stdout.contains("only.sh"),
        "a non-duplicated script must not be flagged: {}",
        r.stdout
    );
}

#[test]
fn review_does_not_flag_duplicated_markdown() {
    // spec: CLI-144
    // Markdown is prose, not tooling: identical docs across items are not a
    // duplicate-tooling finding.
    let sb = Sandbox::bare("agents");
    write(
        &sb.source.join("skills/a/SKILL.md"),
        "---\nname: a\ndescription: a\n---\n# shared heading\n",
    );
    write(&sb.source.join("skills/a/NOTES.md"), "same notes\n");
    write(
        &sb.source.join("skills/b/SKILL.md"),
        "---\nname: b\ndescription: b\n---\n# shared heading\n",
    );
    write(&sb.source.join("skills/b/NOTES.md"), "same notes\n");
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("duplicate-tooling"),
        "duplicated markdown must not be flagged: {}",
        r.stdout
    );
}

#[test]
fn review_fix_rewrites_local_copy() {
    // spec: CLI-138
    let sb = Sandbox::bare("agents");
    let skill = sb.source.join("skills/review/SKILL.md");
    write(
        &skill,
        "---\nname: review\ndescription: review\n---\n\
         run ~/.claude/skills/review/run.sh; hand off to dev\n",
    );
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: dev\n---\n# dev\n",
    );
    let target = sb.source_spec();
    let r = sb.mind(&["review", &target, "--fix"]);
    assert!(
        r.success,
        "advisory-only fix must exit zero: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("fixed"),
        "must report the fixed file: {}",
        r.stdout
    );

    let rewritten = std::fs::read_to_string(&skill).unwrap();
    assert!(
        rewritten.contains("{{self}}/run.sh"),
        "hardcoded path rewritten to a token: {rewritten}"
    );
    assert!(
        rewritten.contains("{{ns:dev}}"),
        "bare sibling name templatized: {rewritten}"
    );
}

#[test]
fn review_fix_refuses_a_registry_target() {
    // spec: CLI-138
    let sb = melded();
    let r = sb.mind(&["review", "agents", "--fix"]);
    assert!(
        !r.success,
        "--fix against a melded selector must refuse: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("fix-not-local"),
        "expected a fix-not-local refusal: {}",
        r.stderr
    );
}
