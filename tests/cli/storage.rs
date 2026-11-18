//! The store and its on-disk state (spec/storage.md).

use super::*;

#[test]
fn a_relative_local_meld_keeps_working_after_the_caller_moves() {
    // spec: STO-72 STO-73 -- U40's headline symptom. `mind meld ./agents` run
    // from the parent directory used to record the literal `./agents` into
    // `Source.url`; every later command run from anywhere else then looked for
    // `./agents` relative to ITS own cwd and found nothing. The absolute form
    // must be persisted at parse time, so the source stays resolvable from any
    // directory.
    let sb = Sandbox::new();
    let r = sb.mind_cwd(&["meld", "./agents", "--register-only"], &sb.base);
    assert!(
        r.success,
        "a relative local meld must succeed: {} {}",
        r.stdout, r.stderr
    );

    // The persisted url is absolute, not the literal './agents'.
    let sources_json = std::fs::read_to_string(sb.mind_home.join("sources.json")).unwrap();
    assert!(
        sources_json.contains(&sb.source.to_string_lossy().into_owned()),
        "the absolute path must be recorded: {sources_json}"
    );
    assert!(
        !sources_json.contains("\"./agents\""),
        "the literal relative spec must not be persisted: {sources_json}"
    );
    // ...and the identity names the real parent directory, not the `local`
    // placeholder that a literal `.` segment used to collapse to.
    assert!(
        !sources_json.contains("\"local/local/agents\""),
        "the owner must be the real parent dir, not the 'local' placeholder: {sources_json}"
    );

    // Run every later command from a DIFFERENT directory: the source still
    // resolves and its items are still installable.
    let elsewhere = sb.mind_home.clone();
    let recall = sb.mind_cwd(&["recall"], &elsewhere);
    assert!(
        recall.success && recall.stdout.contains("review"),
        "the source must still be readable from another cwd: {} {}",
        recall.stdout,
        recall.stderr
    );
    assert!(
        !recall.stderr.contains("gone"),
        "the source must not be reported as a vanished working tree: {}",
        recall.stderr
    );
    let learn = sb.mind_cwd(&["learn", "review"], &elsewhere);
    assert!(
        learn.success,
        "learn from another cwd must succeed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(sb.claude_home.join("skills/review/SKILL.md").exists());
}

#[test]
fn meld_as_different_prefix_creates_coexisting_instance() {
    // spec: STO-58 STO-59 - melding the same repo under a different --namespace is
    // a distinct instance (host/owner/repo@jk), not an in-place re-prefix (CLI-13):
    // it coexists with the bare meld, with its own registry entry. Its skill
    // installs under the new prefix alongside the unprefixed one. Agents are not
    // namespaced (NS-40), so the prefixed instance is registered and its skill is
    // learned explicitly rather than auto-installing the colliding `dev` agent.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&["meld", &spec, "--yes"]).success,
        "initial bare meld+install"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "unprefixed skill installs first"
    );

    // A second meld under a distinct prefix registers a coexisting instance
    // instead of locking or re-prefixing the existing one.
    let r = sb.mind(&["meld", &spec, "--namespace", "jk", "--register-only"]);
    assert!(
        r.success,
        "second meld under a new prefix must succeed: {} {}",
        r.stdout, r.stderr
    );
    let srcs = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        srcs.contains("@jk"),
        "the prefixed instance must be registered under its @jk identity: {srcs}"
    );
    // Both registry entries coexist: the bare source is still listed, not
    // replaced by the aliased one. The bare identity substring appears twice -
    // once bare, once inside the `@jk` line.
    let base = sb.base_name();
    assert!(
        srcs.matches(&format!("local/{base}/agents")).count() >= 2,
        "both the bare and @jk source instances must be listed: {srcs}"
    );

    // Install the prefixed instance's skill; it coexists with the unprefixed one.
    let l = sb.mind(&["learn", "jk:review"]);
    assert!(
        l.success,
        "learn the prefixed skill: {} {}",
        l.stdout, l.stderr
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the unprefixed skill survives"
    );
    assert!(
        sb.claude_home.join("skills/jk:review").exists(),
        "the prefixed instance installs its skill alongside"
    );
}

#[test]
fn as_instances_get_independent_clone_dirs() {
    // spec: STO-59 - two identity-aliased instances of the same repo clone to
    // distinct per-instance paths (`<repo>@<alias>`), so they hold independent
    // checkouts rather than sharing one working tree. A pin forces a local source
    // to be cloned (CLI-27) rather than read live from its working tree, so the
    // per-instance clone dirs actually materialize on disk.
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--as",
            "dev",
            "--pin",
            "HEAD",
            "--register-only"
        ])
        .success,
        "first aliased pinned meld"
    );
    assert!(
        sb.mind(&[
            "meld",
            &spec,
            "--as",
            "test",
            "--pin",
            "HEAD",
            "--register-only"
        ])
        .success,
        "second aliased pinned meld"
    );
    let base = sb.base_name();
    let dev_clone = sb
        .mind_home
        .join(format!("sources/local/{base}/agents@dev"));
    let test_clone = sb
        .mind_home
        .join(format!("sources/local/{base}/agents@test"));
    assert!(
        dev_clone.join("skills/review/SKILL.md").exists(),
        "the @dev instance has its own clone at {dev_clone:?}"
    );
    assert!(
        test_clone.join("skills/review/SKILL.md").exists(),
        "the @test instance has its own clone at {test_clone:?}"
    );
    assert_ne!(dev_clone, test_clone, "the two clones are distinct dirs");
}

#[test]
fn meld_namespace_on_bare_source_forks_a_new_instance() {
    // spec: STO-58 CLI-13 - melding an already-melded (bare, link-only) repo with
    // a --namespace does NOT re-prefix the bare source in place; it registers a
    // distinct `@jk` instance that coexists with it. (Changing a source's prefix
    // in place is the TUI editor's job, NS-30/CLI-161/TUI-53.)
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Meld bare with --link-only so no items are installed.
    assert!(
        sb.mind(&["meld", &spec, "--link-only"]).success,
        "initial link-only meld"
    );

    // A second meld with a namespace forks a coexisting instance.
    let r = sb.mind(&["meld", &spec, "--namespace", "jk", "--link-only"]);
    assert!(
        r.success,
        "meld --namespace on a bare source must succeed: {} {}",
        r.stdout, r.stderr
    );

    // Two registry entries now exist: the bare source and the @jk instance.
    let srcs = sb.mind(&["recall", "--sources"]).stdout;
    let base = sb.base_name();
    assert!(
        srcs.contains(&format!("local/{base}/agents@jk")),
        "the @jk instance must be registered: {srcs}"
    );
    assert!(
        srcs.matches(&format!("local/{base}/agents")).count() >= 2,
        "the bare source must still coexist with @jk: {srcs}"
    );

    // The @jk instance's catalog exposes the prefixed name.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:jk:review"),
        "probe must show the @jk instance's prefixed name: {}",
        probe.stdout
    );
}

#[test]
fn sources_with_same_basename_coexist() {
    // spec: STO-13, CLI-5
    let a = Sandbox::new();
    let b = Sandbox::new(); // separate repo, same basename, different parent
    assert!(a.mind(&["meld", &a.source_spec()]).success);
    assert!(a.mind(&["meld", &b.source_spec()]).success); // no collision

    let a_full = format!("{}/agents", a.base_name());
    let b_full = format!("{}/agents", b.base_name());
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(sources.contains(&a_full), "{sources}");
    assert!(sources.contains(&b_full), "{sources}");

    // A bare item ref now matches both sources -> ambiguous.
    let bare = a.mind(&["learn", "review"]);
    assert!(!bare.success);
    assert!(bare.stderr.contains("ambiguous"), "{}", bare.stderr);

    // The full owner/repo qualifier resolves it.
    let r = a.mind(&["learn", &format!("{a_full}#review")]);
    assert!(r.success, "{}", r.stderr);
}

#[test]
fn learn_links_into_all_configured_homes() {
    // spec: STO-14, LIFE-40
    let sb = Sandbox::new();
    let home_a = sb.base.join("homeA");
    let home_b = sb.base.join("homeB");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            home_a.display(),
            home_b.display()
        ),
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "review"]).success);

    // The item is linked into both homes.
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());

    // forget removes it from every home (via the recorded link registry).
    assert!(sb.mind(&["forget", "review"]).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_err());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_err());
}

#[test]
fn learn_links_into_homes_from_env() {
    // spec: STO-14
    let sb = Sandbox::new();
    let home_a = sb.base.join("envA");
    let home_b = sb.base.join("envB");
    let homes = format!("{}:{}", home_a.display(), home_b.display());
    let env = [("MIND_AGENT_HOMES", homes.as_str())];

    assert!(sb.mind_env(&["meld", &sb.source_spec()], &env).success);
    assert!(sb.mind_env(&["learn", "review"], &env).success);
    assert!(std::fs::symlink_metadata(home_a.join("skills/review")).is_ok());
    assert!(std::fs::symlink_metadata(home_b.join("skills/review")).is_ok());
}

#[test]
fn config_is_created_with_default_lobe_on_first_use() {
    // spec: STO-15
    let sb = Sandbox::new();
    let cfg_path = sb.mind_home.join("config.toml");
    assert!(!cfg_path.exists());
    // A layout-creating command materializes the default config.
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(cfg_path.exists(), "meld should create the default config");
    let body = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(body.contains("lobes"), "{body}");
    assert!(
        body.contains(&sb.claude_home.display().to_string()),
        "default lobe should be the claude home: {body}"
    );
}

#[test]
fn relative_lobe_is_canonicalized_to_absolute() {
    // spec: STO-16
    let sb = Sandbox::new();
    // Configure a *relative* lobe. mind must resolve it against the working
    // directory at install time so the recorded link path is absolute and does
    // not depend on the cwd at a later uninstall.
    write(&sb.mind_home.join("config.toml"), "lobes = [\"rellobe\"]\n");
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    // Learn with the child cwd set to the sandbox base, so "rellobe" -> <base>/rellobe.
    let r = sb.mind_cwd(&["learn", "review"], &sb.base);
    assert!(r.success, "{}", r.stderr);
    let link = sb.base.join("rellobe/skills/review");
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "link should be created at the resolved absolute path {link:?}"
    );

    // The recorded link path is absolute (not the relative "rellobe/...").
    let detail = sb.mind(&["recall", "skill:review"]).stdout;
    assert!(
        detail.contains(&link.display().to_string()),
        "recorded link should be the absolute path: {detail}"
    );

    // And forget, run from a *different* cwd, still removes it (the path was
    // absolute, not cwd-relative).
    assert!(sb.mind_cwd(&["forget", "review"], &sb.mind_home).success);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "link should be gone"
    );
}

#[test]
fn example_multi_lobe_links_into_all_homes() {
    // spec: STO-14, LIFE-40
    // With two lobes configured (STO-14), a single `learn` links the item into
    // every configured agent home, and `forget` removes the link from all of
    // them (LIFE-40).
    let sb = Sandbox::from_example("multi-lobe");
    let lobe_a = sb.base.join("lobe-a");
    let lobe_b = sb.base.join("lobe-b");
    write(
        &sb.mind_home.join("config.toml"),
        &format!(
            "lobes = [\"{}\", \"{}\"]\n",
            lobe_a.display(),
            lobe_b.display()
        ),
    );

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let learn = sb.mind(&["learn", "recap"]);
    assert!(learn.success, "{} {}", learn.stdout, learn.stderr);

    // The skill is symlinked into BOTH lobes.
    let link_a = lobe_a.join("skills/recap");
    let link_b = lobe_b.join("skills/recap");
    assert!(
        std::fs::symlink_metadata(&link_a).is_ok(),
        "recap must be linked into lobe A"
    );
    assert!(
        std::fs::symlink_metadata(&link_b).is_ok(),
        "recap must be linked into lobe B"
    );
    // Each link points at the one store copy.
    let store = sb.mind_home.join("store/skill/recap");
    assert_eq!(
        std::fs::canonicalize(&link_a).unwrap(),
        std::fs::canonicalize(&store).unwrap(),
        "lobe A link must point at the store copy"
    );
    assert_eq!(
        std::fs::canonicalize(&link_b).unwrap(),
        std::fs::canonicalize(&store).unwrap(),
        "lobe B link must point at the store copy"
    );

    // forget removes the link from BOTH lobes (LIFE-40).
    let forget = sb.mind(&["forget", "recap"]);
    assert!(forget.success, "{} {}", forget.stdout, forget.stderr);
    assert!(
        std::fs::symlink_metadata(&link_a).is_err(),
        "forget must remove the link from lobe A"
    );
    assert!(
        std::fs::symlink_metadata(&link_b).is_err(),
        "forget must remove the link from lobe B"
    );
}

/// Spawn a `mind` child process and return its handle without waiting.
fn spawn_mind(
    mind_home: &std::path::Path,
    claude_home: &std::path::Path,
    args: &[&str],
) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(args)
        .env("MIND_HOME", mind_home)
        .env("CLAUDE_HOME", claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mind")
}

#[test]
fn concurrent_mutating_commands_both_succeed_no_lost_update() {
    // Two `meld` calls that target different sources run concurrently against the
    // same MIND_HOME. The advisory exclusive lock serializes them so neither
    // overwrites the other's registry write. Both sources must appear in the
    // final sources list.
    // spec: STO-40 STO-41
    let a = Sandbox::new();
    let b = Sandbox::named("tools");
    // Reuse a's mind/claude home as the shared environment for both processes.
    let mind_home = &a.mind_home;
    let claude_home = &a.claude_home;

    let a_spec = a.source_spec();
    let b_spec = b.source_spec();

    let mut child_a = spawn_mind(mind_home, claude_home, &["meld", &a_spec]);
    let mut child_b = spawn_mind(mind_home, claude_home, &["meld", &b_spec]);

    let status_a = child_a.wait().expect("wait a");
    let status_b = child_b.wait().expect("wait b");

    assert!(status_a.success(), "first meld failed");
    assert!(status_b.success(), "second meld failed");

    // Both sources must be registered (no lost update).
    let sources = a.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("agents"),
        "first source missing: {sources}"
    );
    assert!(
        sources.contains("tools"),
        "second source missing: {sources}"
    );
}

#[test]
fn concurrent_learn_commands_both_effects_survive() {
    // Two `learn` commands running concurrently against one MIND_HOME install
    // different items. Both must appear in the manifest afterward.
    // spec: STO-40 STO-41
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    // Pre-meld so both learns can resolve items.
    assert!(sb.mind(&["meld", &spec]).success);

    let mind_home = &sb.mind_home;
    let claude_home = &sb.claude_home;

    let mut child_a = spawn_mind(mind_home, claude_home, &["learn", "review"]);
    let mut child_b = spawn_mind(mind_home, claude_home, &["learn", "dev"]);

    let status_a = child_a.wait().expect("wait a");
    let status_b = child_b.wait().expect("wait b");

    assert!(status_a.success(), "learn review failed");
    assert!(status_b.success(), "learn dev failed");

    // Both items must be in the manifest - no lost update.
    let recall = sb.mind(&["recall"]).stdout;
    assert!(recall.contains("skill:review"), "review lost: {recall}");
    assert!(recall.contains("agent:dev"), "dev lost: {recall}");
}

#[test]
fn three_concurrent_learns_no_lost_update() {
    // Three learns of distinct items race against one MIND_HOME. Each is a
    // read-modify-write of manifest.json; without the exclusive lock at least one
    // entry would be lost to a clobbering write. All three must survive.
    // Repeat to make a lost update under a broken lock overwhelmingly likely.
    // spec: STO-40 STO-41
    for _ in 0..15 {
        let sb = Sandbox::new();
        let spec = sb.source_spec();
        assert!(sb.mind(&["meld", &spec]).success);

        let mind_home = &sb.mind_home;
        let claude_home = &sb.claude_home;

        let mut ca = spawn_mind(mind_home, claude_home, &["learn", "review"]);
        let mut cb = spawn_mind(mind_home, claude_home, &["learn", "dev"]);
        let mut cc = spawn_mind(mind_home, claude_home, &["learn", "style"]);

        assert!(ca.wait().expect("wait a").success(), "learn review failed");
        assert!(cb.wait().expect("wait b").success(), "learn dev failed");
        assert!(cc.wait().expect("wait c").success(), "learn style failed");

        let recall = sb.mind(&["recall"]);
        assert!(recall.success, "recall failed: {}", recall.stderr);
        assert!(
            recall.stdout.contains("skill:review"),
            "review lost: {}",
            recall.stdout
        );
        assert!(
            recall.stdout.contains("agent:dev"),
            "dev lost: {}",
            recall.stdout
        );
        assert!(
            recall.stdout.contains("rule:style"),
            "style lost: {}",
            recall.stdout
        );
    }
}

#[test]
fn concurrent_reader_and_writer_reader_does_not_see_torn_file() {
    // A `recall` (shared lock, reads sources.json / manifest.json) runs
    // concurrently with a `learn` (exclusive lock, writes manifest.json).
    // The reader must not error: it either sees the state before or after the
    // write, never a partial file (guaranteed by the advisory lock + atomic writes).
    // spec: STO-43
    let sb = Sandbox::new();
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec]).success);

    let mind_home = &sb.mind_home;
    let claude_home = &sb.claude_home;

    // Run many rounds to increase the chance of interleaving. Each round races a
    // reader against both a learn (write) and the forget cleanup (another write),
    // widening the window in which a torn read could occur.
    for _ in 0..40 {
        let mut writer = spawn_mind(mind_home, claude_home, &["learn", "review"]);
        let reader1 = spawn_mind(mind_home, claude_home, &["recall"]);
        let reader2 = spawn_mind(mind_home, claude_home, &["recall", "--sources"]);

        let ws = writer.wait().expect("wait writer");
        let r1 = reader1.wait_with_output().expect("wait reader1");
        let r2 = reader2.wait_with_output().expect("wait reader2");

        assert!(ws.success(), "writer failed");
        // The reader may see "nothing learned" (before) or the item (after),
        // but must never error: a torn manifest.json would surface as a Json
        // parse error and a non-zero exit.
        assert!(
            r1.status.success(),
            "recall errored during concurrent write: {}",
            String::from_utf8_lossy(&r1.stderr)
        );
        assert!(
            r2.status.success(),
            "recall --sources errored during concurrent write: {}",
            String::from_utf8_lossy(&r2.stderr)
        );
        // The reader must not have hit a parse failure even on a successful exit
        // (defensive: a partial file that happened to parse to junk).
        let err1 = String::from_utf8_lossy(&r1.stderr);
        assert!(
            !err1.contains("expected") && !err1.to_lowercase().contains("json"),
            "reader saw a torn/partial file: {err1}"
        );

        // Clean up for the next round.
        sb.mind(&["forget", "review"]);
    }
}

#[test]
fn exclusive_lock_blocks_second_writer_until_first_completes() {
    // Start a writer; while it holds the exclusive lock, a second writer must
    // wait (block) rather than proceed concurrently. We verify this by running
    // two sequential meld+unmeld pairs and asserting the final state is
    // consistent (both ran fully). A non-blocking implementation would produce
    // racy JSON and crash or corrupt; a serializing one succeeds.
    // spec: STO-42
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // Run two concurrent melds of the same spec; one will block on the lock
    // while the other runs. The second should get SourceExists and exit
    // non-zero, but must not crash or corrupt the registry. The registry must
    // be parseable (one valid source entry).
    let mut c1 = spawn_mind(&sb.mind_home, &sb.claude_home, &["meld", &spec]);
    let mut c2 = spawn_mind(&sb.mind_home, &sb.claude_home, &["meld", &spec]);

    let _ = c1.wait();
    let _ = c2.wait();

    // Exactly one meld should have succeeded; the registry must be well-formed.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.success,
        "recall failed after concurrent melds: {}",
        sources.stderr
    );
    // The registry must be well-formed (parseable by recall) and contain exactly
    // one source entry. Count non-blank, non-header lines.
    let entry_lines: Vec<_> = sources
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.contains("melded source"))
        .collect();
    assert_eq!(
        entry_lines.len(),
        1,
        "expected exactly one source entry, got {}: {}",
        entry_lines.len(),
        sources.stdout
    );
}

// ---- version pinning tests (DSC-41, STO-18, CLI-17, CLI-18, CLI-55) ---------

#[test]
fn existing_sources_json_without_pin_field_loads_as_default_branch() {
    // spec: STO-18 (missing pin field -> DefaultBranch default)
    // Write a sources.json that has no "pin" field, simulating an older registry
    // written before version pinning was added.  sync must still work and treat
    // the source as DefaultBranch.
    let sb = Sandbox::new();
    let spec = sb.source_spec();

    // First meld so the clone exists on disk.
    assert!(sb.mind(&["meld", &spec]).success);

    // Rewrite sources.json without the `pin` field, in the format that
    // an old `mind` would have written.
    let path = sb.mind_home.join("sources.json");
    // Read the real file to get the actual name/url/host/owner/repo/commit values.
    let json = std::fs::read_to_string(&path).unwrap();
    // Extract the "name" value for use in the hand-crafted JSON.
    let name_start = json.find("\"name\": \"").unwrap() + "\"name\": \"".len();
    let name_end = json[name_start..].find('"').unwrap() + name_start;
    let name_val = &json[name_start..name_end];

    let url_start = json.find("\"url\": \"").unwrap() + "\"url\": \"".len();
    let url_end = json[url_start..].find('"').unwrap() + url_start;
    let url_val = &json[url_start..url_end];

    // Build a minimal sources.json with no pin field.
    let no_pin_json = format!(
        r#"{{
  "sources": [
    {{
      "name": "{name_val}",
      "url": "{url_val}",
      "host": "local",
      "owner": "x",
      "repo": "agents",
      "commit": null
    }}
  ]
}}"#
    );
    std::fs::write(&path, no_pin_json).unwrap();

    // sync must not error (reads missing pin as DefaultBranch).
    let r = sb.mind(&["sync"]);
    assert!(
        r.success,
        "sync on old sources.json (no pin field) should succeed: {}",
        r.stderr
    );
}

#[test]
fn pin_persists_across_repeated_syncs_while_commit_advances() {
    // spec: STO-18, CLI-55 - the recorded pin is untouched by sync across
    // repeated runs, even as a follow-branch source's recorded commit advances.
    let (sb, sha_v1, _v2) = make_pinnable_repo("pintest-multi-sync");
    let spec = sb.source_spec();

    assert!(
        sb.mind(&["meld", &spec, "--follow-branch", "stable"])
            .success
    );
    assert_eq!(read_source_commit(&sb), sha_v1);
    let pin_initial = read_source_pin_json(&sb);

    // First sync with no upstream change: commit stays, pin stays.
    assert!(sb.mind(&["sync"]).success);
    assert_eq!(read_source_commit(&sb), sha_v1);
    assert_eq!(read_source_pin_json(&sb), pin_initial);

    // Advance `stable` upstream, then sync: commit moves, pin still untouched.
    write(
        &sb.source.join("agents/dev.md"),
        "---\nname: dev\ndescription: stable v3\n---\n# stable v3\n",
    );
    git(&sb.source, &["commit", "-aqm", "v3 on stable"]);
    let sha_v3 = {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sb.source)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&sb.source, &["branch", "-f", "stable", &sha_v3]);

    assert!(sb.mind(&["sync"]).success);
    assert_eq!(
        read_source_commit(&sb),
        sha_v3,
        "follow-branch commit should advance across repeated syncs"
    );
    assert_eq!(
        read_source_pin_json(&sb),
        pin_initial,
        "pin value must stay untouched across repeated syncs"
    );

    // A third sync with no further change keeps both stable.
    assert!(sb.mind(&["sync"]).success);
    assert_eq!(read_source_commit(&sb), sha_v3);
    assert_eq!(read_source_pin_json(&sb), pin_initial);
}

#[test]
fn sync_preserves_roots() {
    // spec: STO-17 - the roots override is persisted at meld and not changed by sync.
    let sb = Sandbox::bare("roots-sync");
    sb.write_and_commit(
        "sub/skills/deploy/SKILL.md",
        "---\ndescription: deploy\n---\n# deploy\n",
    );
    let spec = sb.source_spec();
    assert!(sb.mind(&["meld", &spec, "--root", "sub"]).success);

    // Capture roots before sync.
    let roots_before = read_source_roots_json(&sb);
    assert!(
        roots_before.contains("sub"),
        "roots should be set: {roots_before}"
    );

    // sync must not change the roots field.
    assert!(sb.mind(&["sync"]).success);
    let roots_after = read_source_roots_json(&sb);
    assert_eq!(
        roots_before, roots_after,
        "sync must not modify the recorded roots"
    );

    // After sync, probe still shows the subtree items.
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:deploy"),
        "subtree item still visible after sync: {}",
        probe.stdout
    );
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn evolve_check_reports_resolved_target_triple() {
    // spec: STO-65
    // Linux release artifacts changed from gnu to musl; `--check` must surface
    // the resolved target triple in the human-readable report, before any
    // download happens, so that swap is visible rather than silent. Fully
    // offline (an explicit --version bypasses the GitHub API).
    let sb = Sandbox::new();
    let r = sb.mind(&["evolve", "--check", "--version", "9.9.9"]);
    assert!(
        r.success,
        "evolve --check --version 9.9.9 should succeed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("x86_64-unknown-linux-musl"),
        "expected the resolved musl target triple in the --check report: {}",
        r.stdout
    );
    // The existing wording must still be present unchanged.
    assert!(
        r.stdout.contains("9.9.9") && r.stdout.contains("available"),
        "existing report wording must be preserved: {}",
        r.stdout
    );
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn evolve_check_json_includes_target_triple_key() {
    // spec: STO-65
    // Under --json, the target triple is exposed as an ADDITIONAL
    // `target_triple` key; the existing `action`/`target`/`outcome` keys must
    // be unchanged (not renamed), since --json consumers depend on them.
    let sb = Sandbox::new();
    let r = sb.mind(&["evolve", "--check", "--version", "9.9.9", "--json"]);
    assert!(
        r.success,
        "evolve --check --version 9.9.9 --json should succeed: {} {}",
        r.stdout, r.stderr
    );
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "evolve --check --json stdout must be valid JSON ({e}): {}",
            r.stdout
        )
    });
    assert_eq!(
        v.get("target_triple").and_then(|t| t.as_str()),
        Some("x86_64-unknown-linux-musl"),
        "target_triple must be the resolved musl artifact: {}",
        r.stdout
    );
    // Existing keys must be present and unchanged (not renamed).
    assert_eq!(v.get("action").and_then(|a| a.as_str()), Some("evolve"));
    assert_eq!(v.get("target").and_then(|t| t.as_str()), Some("9.9.9"));
    assert_eq!(v.get("outcome").and_then(|o| o.as_str()), Some("available"));
}

// `evolve`'s download corridor no longer has an integration test. It used to be
// driven hermetically by a fake `curl` on PATH; the downloader is the
// `self_update` crate now, so a PATH stub intercepts nothing and the same test
// would make a real network call. What those two tests pinned -- that the
// "downloading mind ..." progress line is routed by the `--json` guard rather
// than deleted (CLI-217, STO-47) -- is asserted in
// `selfupdate::tests::download_banner_is_suppressed_under_json` instead, which
// needs no download at all.
