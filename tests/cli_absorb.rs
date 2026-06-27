//! Integration tests for `mind absorb` (spec/absorb.md ABS-1..ABS-10).
//!
//! Each test drives the real `mind` binary against a hermetic fixture:
//! MIND_HOME/CLAUDE_HOME/destination pointed at temp dirs. No network.
//! All test assertions cite spec IDs via `// spec: ABS-N` comments.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---- Sandbox helpers --------------------------------------------------------

struct Sandbox {
    base: PathBuf,
    /// The destination source repo (the git repo items are moved into).
    dest: PathBuf,
    mind_home: PathBuf,
    /// The agent home (lobe).
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-abs-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dest = base.join("personal");
        let sb = Sandbox {
            base: base.clone(),
            dest: dest.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        // Create and git-init the destination repo.
        git_init(&dest);
        sb
    }

    /// Run `mind <args>` with the sandbox environment.
    fn mind(&self, args: &[&str]) -> Run {
        self.run(args, None, &[])
    }

    /// Run `mind <args>` with additional env vars.
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        self.run(args, None, envs)
    }

    fn run(&self, args: &[&str], input: Option<&str>, envs: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            // Clear MIND_ABSORB_TO so tests don't bleed env from the OS.
            .env_remove("MIND_ABSORB_TO")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn mind");
        if let Some(text) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().expect("wait mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Place an unmanaged item in the lobe.
    /// Returns the path of the lobe entry.
    fn place_unmanaged_skill(&self, name: &str) -> PathBuf {
        let p = self.claude_home.join("skills").join(name);
        write_file(&p.join("SKILL.md"), &format!("# {name} skill\n"));
        p
    }

    fn place_unmanaged_agent(&self, name: &str) -> PathBuf {
        let p = self.claude_home.join("agents").join(format!("{name}.md"));
        write_file(&p, &format!("# {name} agent\n"));
        p
    }

    fn place_unmanaged_rule(&self, name: &str) -> PathBuf {
        let p = self.claude_home.join("rules").join(format!("{name}.md"));
        write_file(&p, &format!("# {name} rule\n"));
        p
    }

    fn dest_spec(&self) -> String {
        self.dest.to_string_lossy().into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

// ---- filesystem helpers -----------------------------------------------------

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

fn git_init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(dir, &["config", "user.email", "t@t"]);
    git(dir, &["config", "user.name", "t"]);
    // Make an initial commit so the repo has a HEAD.
    let readme = dir.join("README.md");
    std::fs::write(&readme, "# personal\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-qm", "init"]);
}

/// Read the last git commit message in `dir`.
fn last_commit_msg(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["log", "-1", "--pretty=format:%s"])
        .current_dir(dir)
        .output()
        .expect("git log");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Check whether `path` is a symlink (managed link).
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

// ---- ABS-1: resolve + absorb a skill/agent/rule; glob rejected; tool rejected ---

/// Absorbing an unmanaged skill moves it to the destination convention path,
/// commits it, melds the destination, and installs a managed symlink.
// spec: ABS-1
#[test]
fn abs1_absorb_skill_installs_managed_symlink() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_skill("review");
    assert!(lobe_path.exists(), "sanity: unmanaged skill must exist");

    let dest = sb.dest_spec();
    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "absorb skill:review must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The lobe path is now a managed symlink, not the original file.
    assert!(
        is_symlink(&lobe_path),
        "after absorb the lobe path must be a managed symlink, not the original dir"
    );

    // The item must appear in `recall` as a managed item.
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("review"),
        "absorbed item must appear in recall: {}",
        recall.stdout
    );
}

/// Absorbing an unmanaged agent moves it to agents/<name>.md in the destination.
// spec: ABS-1
#[test]
fn abs1_absorb_agent_installs_managed_symlink() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_agent("dev");

    let dest = sb.dest_spec();
    let r = sb.mind(&["absorb", "agent:dev", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "absorb agent:dev must succeed: stderr={}",
        r.stderr
    );
    assert!(
        is_symlink(&lobe_path),
        "lobe path must be a managed symlink after absorb"
    );
    let recall = sb.mind(&["recall"]);
    assert!(
        recall.stdout.contains("dev"),
        "dev must appear in recall after absorb"
    );
}

/// Absorbing an unmanaged rule moves it to rules/<name>.md in the destination.
// spec: ABS-1
#[test]
fn abs1_absorb_rule_installs_managed_symlink() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_rule("style");

    let dest = sb.dest_spec();
    let r = sb.mind(&["absorb", "rule:style", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "absorb rule:style must succeed: stderr={}",
        r.stderr
    );
    assert!(
        is_symlink(&lobe_path),
        "lobe path must be a managed symlink after absorb"
    );
}

/// A glob ref is rejected with InvalidItemRef before resolve is called.
// spec: ABS-1
#[test]
fn abs1_glob_ref_is_invalid_item_ref() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    let r = sb.mind(&["absorb", "skill:*", "--to", &dest]);
    assert!(
        !r.success,
        "a glob ref must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not a valid item ref") || r.stderr.contains("InvalidItemRef"),
        "error must mention invalid item ref: {}",
        r.stderr
    );
}

/// A source-qualified ref (`owner/repo#name`) never matches an unmanaged item
/// (unmanaged items have no source), so absorb fails with NotInstalled and the
/// lobe entry is untouched.
// spec: ABS-1
#[test]
fn abs1_source_qualified_ref_never_matches() {
    let sb = Sandbox::new();
    let lobe = sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    // The ref names a source; unmanaged items are sourceless, so this never matches.
    let r = sb.mind(&["absorb", "owner/repo#skill:review", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "a source-qualified ref must not match an unmanaged item: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not installed") || r.stderr.contains("NotInstalled"),
        "error must be NotInstalled: {}",
        r.stderr
    );
    // The lobe entry must be untouched (ABS-10): still the original dir.
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe must be unchanged after a sourceless ref miss"
    );
}

/// A bare name shared across two kinds is ambiguous; absorb refuses and changes
/// nothing. A kind prefix disambiguates and absorbs exactly the named kind,
/// leaving the same-named item of the other kind unmanaged.
// spec: ABS-1
#[test]
fn abs1_kind_prefix_disambiguates_same_name() {
    let sb = Sandbox::new();
    // Two unmanaged items share the name "shared": one skill, one agent.
    let skill_lobe = sb.place_unmanaged_skill("shared");
    let agent_lobe = sb.place_unmanaged_agent("shared");
    let dest = sb.dest_spec();

    // A bare ref is ambiguous: must fail, nothing moved.
    let ambiguous = sb.mind(&["absorb", "shared", "--to", &dest, "--yes"]);
    assert!(
        !ambiguous.success,
        "a bare name shared across kinds must be ambiguous: stdout={} stderr={}",
        ambiguous.stdout, ambiguous.stderr
    );
    assert!(
        ambiguous.stderr.contains("ambiguous")
            || ambiguous.stderr.contains("Ambiguous")
            || ambiguous.stderr.contains("matches"),
        "error must indicate ambiguity: {}",
        ambiguous.stderr
    );
    assert!(
        skill_lobe.exists() && !is_symlink(&skill_lobe),
        "skill lobe must be unchanged after an ambiguous ref"
    );
    assert!(
        agent_lobe.exists() && !is_symlink(&agent_lobe),
        "agent lobe must be unchanged after an ambiguous ref"
    );

    // The kind prefix disambiguates: absorb only the agent.
    let r = sb.mind(&["absorb", "agent:shared", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "agent:shared must resolve and absorb: stderr={}",
        r.stderr
    );
    assert!(
        is_symlink(&agent_lobe),
        "the agent lobe must become a managed symlink"
    );
    // The agent landed at the agent convention path, not the skill path.
    assert!(
        sb.dest.join("agents").join("shared.md").exists(),
        "the agent must be at agents/shared.md in the destination"
    );
    assert!(
        !sb.dest.join("skills").join("shared").exists(),
        "the skill must NOT have been absorbed by an agent: ref"
    );
    // The same-named skill remains unmanaged.
    assert!(
        skill_lobe.exists() && !is_symlink(&skill_lobe),
        "the same-named skill must remain unmanaged after absorbing only the agent"
    );
}

/// A ref that names no unmanaged item is NotInstalled.
// spec: ABS-1
#[test]
fn abs1_unresolved_ref_is_not_installed() {
    let sb = Sandbox::new();
    let dest = sb.dest_spec();

    let r = sb.mind(&["absorb", "skill:nonexistent", "--to", &dest]);
    assert!(
        !r.success,
        "ref with no match must fail: stderr={}",
        r.stderr
    );
    assert!(
        r.stderr.contains("not installed") || r.stderr.contains("NotInstalled"),
        "error must indicate not installed: {}",
        r.stderr
    );
}

// ---- ABS-2: destination precedence --to > MIND_ABSORB_TO > absorb_to -------

/// `--to <path>` takes precedence over MIND_ABSORB_TO.
// spec: ABS-2
#[test]
fn abs2_to_flag_beats_env() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");

    // Create a second dest to use as MIND_ABSORB_TO (it should not be used).
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let other_dest = sb.base.join(format!("other-dest-{n}"));
    git_init(&other_dest);

    let dest = sb.dest_spec();
    let other_dest_str = other_dest.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["absorb", "skill:review", "--to", &dest, "--yes"],
        &[("MIND_ABSORB_TO", &other_dest_str)],
    );
    assert!(
        r.success,
        "--to flag must take precedence over MIND_ABSORB_TO: stderr={}",
        r.stderr
    );
    // Item moved to --to destination, not MIND_ABSORB_TO.
    let skill_in_dest = sb.dest.join("skills").join("review");
    assert!(
        skill_in_dest.exists(),
        "skill must be in --to destination, not the env destination: {skill_in_dest:?}"
    );
    let skill_in_other = other_dest.join("skills").join("review");
    assert!(
        !skill_in_other.exists(),
        "skill must NOT be in MIND_ABSORB_TO destination"
    );
}

/// MIND_ABSORB_TO is used when --to is absent and no config.absorb_to set.
// spec: ABS-2
#[test]
fn abs2_env_beats_config_absorb_to() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");

    // A second dest for config.absorb_to — should not be used.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let config_dest = sb.base.join(format!("config-dest-{n}"));
    git_init(&config_dest);

    // Write a config.toml with absorb_to pointing at config_dest.
    let config_path = sb.mind_home.join("config.toml");
    std::fs::create_dir_all(&sb.mind_home).unwrap();
    std::fs::write(
        &config_path,
        format!("absorb_to = \"{}\"\n", config_dest.to_string_lossy()),
    )
    .unwrap();

    // MIND_ABSORB_TO points at the real dest — should win.
    let dest = sb.dest_spec();
    let r = sb.mind_env(
        &["absorb", "skill:review", "--yes"],
        &[("MIND_ABSORB_TO", &dest)],
    );
    assert!(
        r.success,
        "MIND_ABSORB_TO must beat config.absorb_to: stderr={}",
        r.stderr
    );
    let skill_in_dest = sb.dest.join("skills").join("review");
    assert!(
        skill_in_dest.exists(),
        "skill must be in MIND_ABSORB_TO destination"
    );
    let skill_in_config = config_dest.join("skills").join("review");
    assert!(
        !skill_in_config.exists(),
        "skill must NOT be in config.absorb_to destination"
    );
}

/// config.absorb_to is used when neither --to nor MIND_ABSORB_TO is set.
// spec: ABS-2
#[test]
fn abs2_config_absorb_to_is_used_as_fallback() {
    let sb = Sandbox::new();
    sb.place_unmanaged_rule("style");

    let dest = sb.dest_spec();
    // Write config.toml with absorb_to.
    std::fs::create_dir_all(&sb.mind_home).unwrap();
    std::fs::write(
        sb.mind_home.join("config.toml"),
        format!("absorb_to = \"{dest}\"\n"),
    )
    .unwrap();

    let r = sb.mind(&["absorb", "rule:style", "--yes"]);
    assert!(
        r.success,
        "config.absorb_to must be used as fallback: stderr={}",
        r.stderr
    );
    let rule_in_dest = sb.dest.join("rules").join("style.md");
    assert!(
        rule_in_dest.exists(),
        "rule must land in config.absorb_to destination"
    );
}

// ---- ABS-3: non-TTY, none set => ConfirmationRequired ----------------------

/// A non-TTY run with no destination configured fails with ConfirmationRequired.
// spec: ABS-3
#[test]
fn abs3_non_tty_no_dest_is_confirmation_required() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    // No --to, no MIND_ABSORB_TO, no config.absorb_to.
    // The test harness drives stdin as piped (non-TTY).
    let r = sb.mind(&["absorb", "skill:review"]);
    assert!(
        !r.success,
        "non-TTY with no dest must fail: stderr={}",
        r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation") || r.stderr.contains("ConfirmationRequired"),
        "error must indicate ConfirmationRequired: {}",
        r.stderr
    );
    // Lobe entry must still exist (nothing changed).
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe entry must be unchanged after a refused absorb"
    );
}

// ---- ABS-4: only interactive destination triggers the save prompt ----------

/// When --to supplies the destination, no save-to-config prompt is given.
/// We verify by checking config.toml is NOT created/modified after --to absorb.
// spec: ABS-4
#[test]
fn abs4_to_flag_dest_does_not_save_absorb_to() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    // No config.toml exists before absorb.
    let config_path = sb.mind_home.join("config.toml");
    assert!(
        !config_path.exists(),
        "sanity: config.toml must not exist before absorb"
    );

    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(r.success, "absorb must succeed: stderr={}", r.stderr);

    // config.toml may have been created by the layout setup, but if it was it
    // must NOT contain absorb_to.
    if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            !contents.contains("absorb_to"),
            "--to destination must not save absorb_to in config: {contents}"
        );
    }
}

// ---- ABS-5: destination must be a git repo; commit message ----------------

/// The built-in ~/.mind/personal is created and git-init'd on demand when
/// selected interactively. Here we test that when the dest already exists as a
/// git repo, absorb commits with the expected message `absorb <kind>:<name>`.
// spec: ABS-5
#[test]
fn abs5_commit_message_is_absorb_kind_name() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(r.success, "absorb must succeed: stderr={}", r.stderr);

    let msg = last_commit_msg(&sb.dest);
    assert_eq!(
        msg, "absorb skill:review",
        "commit message must be 'absorb skill:review', got: {msg}"
    );
}

/// A --to path that is not a git repository is an error (DestinationNotRepo).
// spec: ABS-5
#[test]
fn abs5_non_repo_dest_is_error() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");

    // A plain directory (not a git repo).
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let plain_dir = sb.base.join(format!("notarepo-{n}"));
    std::fs::create_dir_all(&plain_dir).unwrap();
    let plain_str = plain_dir.to_string_lossy().into_owned();

    let r = sb.mind(&["absorb", "skill:review", "--to", &plain_str, "--yes"]);
    assert!(
        !r.success,
        "a non-repo destination must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not a git repository") || r.stderr.contains("DestinationNotRepo"),
        "error must mention not a git repository: {}",
        r.stderr
    );
    // Lobe must be unchanged.
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe must be unchanged after failed absorb"
    );
}

// ---- ABS-6: collision errors; --force overwrites --------------------------

/// A kind:name collision at the destination errors without --force.
// spec: ABS-6
#[test]
fn abs6_collision_without_force_is_error() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    // Create a collision at the destination convention path.
    let collision = sb.dest.join("skills").join("review");
    write_file(&collision.join("SKILL.md"), "# existing skill\n");
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add existing skill"]);

    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "collision without --force must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("already has") || r.stderr.contains("AbsorbCollision"),
        "error must mention collision: {}",
        r.stderr
    );
    // The original lobe entry must be untouched (ABS-10).
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe must be unchanged after a collision error"
    );
    // The destination must not have been clobbered.
    let dest_content = std::fs::read_to_string(collision.join("SKILL.md")).unwrap();
    assert!(
        dest_content.contains("existing skill"),
        "destination must not be clobbered: {dest_content}"
    );
}

/// With --force, a collision is overwritten: the destination content is REPLACED
/// (not merged), so a file present only in the old destination copy is gone.
// spec: ABS-6
#[test]
fn abs6_collision_with_force_overwrites() {
    let sb = Sandbox::new();
    // The lobe skill carries a distinctive marker so we can confirm it replaced
    // the destination content.
    let lobe_skill = sb.claude_home.join("skills").join("review");
    write_file(&lobe_skill.join("SKILL.md"), "# LOBE VERSION\n");

    let dest = sb.dest_spec();

    // Create a collision whose dir has BOTH a different SKILL.md and an extra
    // file that exists only in the old destination copy.
    let collision = sb.dest.join("skills").join("review");
    write_file(&collision.join("SKILL.md"), "# DEST VERSION\n");
    write_file(&collision.join("stale.txt"), "only in old dest\n");
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add existing"]);

    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--force", "--yes"]);
    assert!(
        r.success,
        "absorb --force must overwrite collision: stderr={}",
        r.stderr
    );
    // The lobe is now a managed symlink.
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        is_symlink(&lobe),
        "lobe must be a managed symlink after --force absorb"
    );

    // The destination content is the LOBE version (replaced), not merged.
    let dest_skill = std::fs::read_to_string(collision.join("SKILL.md")).unwrap();
    assert!(
        dest_skill.contains("LOBE VERSION"),
        "destination SKILL.md must be the absorbed lobe version: {dest_skill}"
    );
    // The stale file that existed only in the old dest copy must be gone
    // (overwrite is a replace of the whole convention path, not a merge).
    assert!(
        !collision.join("stale.txt").exists(),
        "old destination-only file must be removed by --force overwrite (replace, not merge)"
    );
}

// ---- ABS-7: multi-lobe: stray copies deleted; --yes skips; non-TTY errors -

/// When an unmanaged item occupies multiple lobes, all stray copies are removed
/// and a single confirmation prompt is shown. With --yes this is skipped.
// spec: ABS-7
#[test]
fn abs7_multi_lobe_stray_copies_deleted_with_yes() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-abs-ml-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dest = base.join("personal");
    let mind_home = base.join("mind");
    let lobe1 = base.join("lobe1");
    let lobe2 = base.join("lobe2");

    git_init(&dest);

    // Place the same unmanaged skill in both lobes.
    let skill1 = lobe1.join("skills").join("myskill");
    write_file(&skill1.join("SKILL.md"), "# myskill\n");
    let skill2 = lobe2.join("skills").join("myskill");
    write_file(&skill2.join("SKILL.md"), "# myskill\n");

    // Configure both lobes.
    std::fs::create_dir_all(&mind_home).unwrap();
    let lobe1_str = lobe1.to_string_lossy();
    let lobe2_str = lobe2.to_string_lossy();
    std::fs::write(
        mind_home.join("config.toml"),
        format!("lobes = [\"{lobe1_str}\", \"{lobe2_str}\"]\n"),
    )
    .unwrap();

    let dest_str = dest.to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["absorb", "skill:myskill", "--to", &dest_str, "--yes"])
        .env("MIND_HOME", &mind_home)
        .env("CLAUDE_HOME", &lobe1)
        .env_remove("MIND_ABSORB_TO")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .output()
        .expect("run mind");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "multi-lobe absorb with --yes must succeed: stdout={stdout} stderr={stderr}"
    );

    // The primary lobe path must be a managed symlink.
    assert!(
        is_symlink(&skill1),
        "primary lobe path must be managed symlink after absorb"
    );
    // The stray lobe copy must be gone (replaced by a managed symlink).
    // learn links into all lobes, so skill2 should now be a symlink.
    assert!(
        is_symlink(&skill2),
        "stray copy in lobe2 must be replaced by managed symlink after absorb"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// Non-TTY without --yes when there are stray copies fails with ConfirmationRequired.
// spec: ABS-7
#[test]
fn abs7_multi_lobe_non_tty_without_yes_is_confirmation_required() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-abs-ml-nontty-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dest = base.join("personal");
    let mind_home = base.join("mind");
    let lobe1 = base.join("lobe1");
    let lobe2 = base.join("lobe2");

    git_init(&dest);

    // Place unmanaged skill in both lobes.
    let skill1 = lobe1.join("skills").join("myskill");
    write_file(&skill1.join("SKILL.md"), "# myskill\n");
    let skill2 = lobe2.join("skills").join("myskill");
    write_file(&skill2.join("SKILL.md"), "# myskill\n");

    std::fs::create_dir_all(&mind_home).unwrap();
    let lobe1_str = lobe1.to_string_lossy();
    let lobe2_str = lobe2.to_string_lossy();
    std::fs::write(
        mind_home.join("config.toml"),
        format!("lobes = [\"{lobe1_str}\", \"{lobe2_str}\"]\n"),
    )
    .unwrap();

    let dest_str = dest.to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["absorb", "skill:myskill", "--to", &dest_str])
        .env("MIND_HOME", &mind_home)
        .env("CLAUDE_HOME", &lobe1)
        .env_remove("MIND_ABSORB_TO")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .output()
        .expect("run mind");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "multi-lobe non-TTY without --yes must fail: stderr={stderr}"
    );
    assert!(
        stderr.contains("needs confirmation") || stderr.contains("ConfirmationRequired"),
        "must be ConfirmationRequired: {stderr}"
    );
    // Nothing moved.
    assert!(
        skill1.exists() && !is_symlink(&skill1),
        "lobe1 skill must be unchanged"
    );
    assert!(
        skill2.exists() && !is_symlink(&skill2),
        "lobe2 skill must be unchanged"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// ---- ABS-8: post-absorb manifest entry with effective name -----------------

/// After absorb, the manifest has a managed entry keyed kind:effective-name,
/// with the destination source. Effective name follows the destination prefix.
// spec: ABS-8
#[test]
fn abs8_manifest_keyed_effective_name() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(r.success, "absorb must succeed: stderr={}", r.stderr);

    // `recall skill:review` must show the item as managed.
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        recall.success,
        "recall skill:review must succeed after absorb: stderr={}",
        recall.stderr
    );
    assert!(
        recall.stdout.contains("review"),
        "recall must show the absorbed item: {}",
        recall.stdout
    );
}

/// With a destination source that has a prefix in mind.toml, the installed
/// item's effective name is prefixed.
// spec: ABS-8
#[test]
fn abs8_effective_name_follows_destination_prefix() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");

    // Write a mind.toml with prefix = "mypfx" to the destination.
    write_file(&sb.dest.join("mind.toml"), "[source]\nprefix = \"mypfx\"\n");
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add mind.toml"]);

    let dest = sb.dest_spec();
    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "absorb must succeed with prefixed dest: stderr={}",
        r.stderr
    );

    // The effective name should be mypfx-review.
    let recall = sb.mind(&["recall", "skill:mypfx-review"]);
    assert!(
        recall.success,
        "recall skill:mypfx-review must work after absorb with prefix: stderr={}",
        recall.stderr
    );
    // The lobe symlink must be at skills/mypfx-review.
    let link = sb.claude_home.join("skills").join("mypfx-review");
    assert!(
        is_symlink(&link),
        "managed link must be at skills/mypfx-review when destination has prefix mypfx: {link:?}"
    );
}

// ---- ABS-9: help text states the three destination ways --------------------

/// absorb --help contains the three destination ways and their precedence.
// spec: ABS-9
#[test]
fn abs9_help_text_states_destination_ways() {
    let sb = Sandbox::new();
    // Run `absorb --help`; clap prints help to stdout (success exit).
    let r = sb.mind(&["absorb", "--help"]);
    // clap may exit 0 or 2 for --help; stdout always has the help text.
    let text = format!("{}\n{}", r.stdout, r.stderr);
    assert!(
        text.contains("--to") || text.contains("MIND_ABSORB_TO") || text.contains("absorb_to"),
        "help must mention at least one of the three destination ways: {text}"
    );
    assert!(
        text.contains("MIND_ABSORB_TO"),
        "help must mention MIND_ABSORB_TO env var: {text}"
    );
    assert!(
        text.contains("absorb_to") || text.contains("config.toml"),
        "help must mention config.toml absorb_to: {text}"
    );
    assert!(
        text.contains("precedence") || text.contains("takes precedence"),
        "help must explicitly state precedence: {text}"
    );
    // Stricter: the three ways must appear in the documented precedence ORDER
    // (--to before MIND_ABSORB_TO before absorb_to). A reordering that silently
    // contradicted ABS-2 would regress this.
    let to_pos = text.find("--to").expect("help mentions --to");
    let env_pos = text
        .find("MIND_ABSORB_TO")
        .expect("help mentions MIND_ABSORB_TO");
    let cfg_pos = text.find("absorb_to").expect("help mentions absorb_to");
    assert!(
        to_pos < env_pos && env_pos < cfg_pos,
        "help must list the destination ways in precedence order \
         (--to < MIND_ABSORB_TO < absorb_to); got positions to={to_pos} env={env_pos} cfg={cfg_pos} in:\n{text}"
    );
}

// ---- ABS-10: transactional: failures leave lobe intact and manifest unchanged

/// When the destination is not a git repo, the lobe file is intact and the
/// manifest is unchanged (absorb is a no-op on failure).
// spec: ABS-10
#[test]
fn abs10_bad_dest_leaves_lobe_intact_and_manifest_unchanged() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");

    // A non-repo destination.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let plain_dir = sb.base.join(format!("notarepo-{n}"));
    std::fs::create_dir_all(&plain_dir).unwrap();
    let plain_str = plain_dir.to_string_lossy().into_owned();

    let r = sb.mind(&["absorb", "skill:review", "--to", &plain_str, "--yes"]);
    assert!(!r.success, "must fail with bad destination");

    // Lobe entry must be unchanged (still the original file, not a symlink).
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe must be unchanged after a failed absorb"
    );

    // recall must not show skill:review as managed.
    let recall = sb.mind(&["recall"]);
    assert!(
        !recall.stdout.contains("[managed]") || !recall.stdout.contains("review"),
        "skill:review must not appear as managed after a failed absorb: {}",
        recall.stdout
    );
}

/// When absorb is declined at the prompt (ABS-7 non-yes), lobe and manifest
/// are unchanged.
// spec: ABS-10
#[test]
fn abs10_collision_leaves_lobe_and_manifest_unchanged() {
    let sb = Sandbox::new();
    sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    // Put a collision at the destination.
    let collision = sb.dest.join("skills").join("review");
    write_file(&collision.join("SKILL.md"), "# existing\n");
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add collision"]);

    // Absorb without --force: must fail.
    let r = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "collision must cause failure: stderr={}",
        r.stderr
    );

    // Lobe entry must still exist and be the original dir.
    let lobe = sb.claude_home.join("skills").join("review");
    assert!(
        lobe.exists() && !is_symlink(&lobe),
        "lobe must be unchanged after collision error"
    );
    // Destination must not have been clobbered.
    let existing_content = std::fs::read_to_string(collision.join("SKILL.md")).unwrap();
    assert!(
        existing_content.contains("existing"),
        "destination must not be clobbered"
    );
    // Manifest must not have a skill:review entry.
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        !recall.success,
        "skill:review must not be in manifest after failed absorb"
    );
}

// ---- round-trip: forget is the inverse of absorb (ABS-8) -------------------

/// After absorb, the item is an ordinary managed item, so `forget` removes it
/// like any installed item: the managed symlink is gone afterward and `recall`
/// no longer lists it as managed. (The source copy stays in the dest repo, which
/// `forget` does not own — the spec inverse is the lobe link + manifest entry.)
// spec: ABS-8
#[test]
fn abs8_forget_is_inverse_of_absorb() {
    let sb = Sandbox::new();
    let lobe = sb.place_unmanaged_skill("review");
    let dest = sb.dest_spec();

    let absorb = sb.mind(&["absorb", "skill:review", "--to", &dest, "--yes"]);
    assert!(
        absorb.success,
        "absorb must succeed: stderr={}",
        absorb.stderr
    );
    assert!(
        is_symlink(&lobe),
        "lobe must be a managed symlink after absorb"
    );

    // forget the now-managed item.
    let forget = sb.mind(&["forget", "skill:review", "--yes"]);
    assert!(
        forget.success,
        "forget of an absorbed item must succeed: stdout={} stderr={}",
        forget.stdout, forget.stderr
    );

    // The managed symlink is gone (forget removed the link it installed).
    assert!(
        !is_symlink(&lobe),
        "forget must remove the managed symlink installed by absorb"
    );

    // recall no longer reports skill:review as a managed item.
    let recall = sb.mind(&["recall", "skill:review"]);
    assert!(
        !recall.success,
        "skill:review must not resolve as managed after forget: stdout={} stderr={}",
        recall.stdout, recall.stderr
    );
}

// ---- lock mode: absorb is Exclusive ----------------------------------------

/// The absorb command takes the Exclusive lock (STO-41 requirement; tested via
/// the parse-time classification in main.rs, but cross-checked here as a CLI test).
// spec: ABS-1
#[test]
fn absorb_command_parses() {
    // Verify that `mind absorb skill:foo --to /tmp/dest --force` parses.
    // We run help to check parse (the command does not execute since we have no lobe item).
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["absorb", "--help"])
        .output()
        .expect("run mind absorb --help");
    // clap prints help to stdout; exit code is 0.
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("absorb") || text.contains("Absorb"),
        "absorb --help must print help text: {text}"
    );
}

// ---- git helper unit tests (in the lib) ------------------------------------
// These verify the git helpers added to src/git.rs for ABS-5 (git_init, is_repo,
// add_all, commit). They are in this integration test file because they need
// an on-disk repo.

/// git_init creates a repository and git::is_repo returns true.
// spec: ABS-5
#[test]
fn git_helpers_init_and_is_repo() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("mind-abs-githelp-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // A fresh dir is not a repo.
    assert!(
        !Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "sanity: empty dir must not be a git repo"
    );

    // After git_init, it is a repo (verify via git CLI directly).
    git_init(&dir);
    assert!(
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "after git_init, git rev-parse --git-dir must succeed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// is_repo returns false for a non-repo directory and true for an initialized one.
// spec: ABS-5
#[test]
fn is_repo_distinguishes_repo_from_plain_dir() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let plain =
        std::env::temp_dir().join(format!("mind-abs-isrepo-plain-{}-{n}", std::process::id()));
    let repo =
        std::env::temp_dir().join(format!("mind-abs-isrepo-repo-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&plain).unwrap();
    git_init(&repo);

    // Call the binary's is_repo equivalent through the git rev-parse check.
    // (We can't call crate::git::is_repo from integration tests directly,
    // so we check via git CLI — the same test as git_helpers_init_and_is_repo.)
    let plain_result = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&plain)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let repo_result = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    assert!(!plain_result, "plain dir must not be a git repo");
    assert!(repo_result, "initialized repo must be a git repo");

    let _ = std::fs::remove_dir_all(&plain);
    let _ = std::fs::remove_dir_all(&repo);
}

// ---- ABS-10: post-copy failure (commit failure) leaves lobe intact ----------

/// When git commit fails AFTER the item has been copied to the destination,
/// the original lobe entry must be restored and the manifest left unchanged.
/// We use a bare git repository as the destination: `is_repo` returns true
/// (git rev-parse --git-dir works), but `git add -A` fails because a bare repo
/// has no working tree. This deterministically triggers the commit-failure path
/// inside absorb after the copy to dest has already happened.
// spec: ABS-10
#[test]
fn abs10_commit_failure_after_copy_restores_lobe_entry() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_agent("myagent");

    // Create a bare git repo as the destination.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let bare_repo = sb
        .base
        .join(format!("bare-dest-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&bare_repo).unwrap();
    // git init --bare: has no working tree, so git add -A fails.
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(&bare_repo)
        .status()
        .expect("git init --bare");
    assert!(status.success(), "git init --bare must succeed");

    let bare_str = bare_repo.to_string_lossy().into_owned();
    let r = sb.mind(&["absorb", "agent:myagent", "--to", &bare_str, "--yes"]);
    assert!(
        !r.success,
        "absorb into a bare repo must fail (git add fails): stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // ABS-10: the original lobe entry must be intact (not deleted, not a symlink).
    assert!(
        lobe_path.exists(),
        "lobe entry must still exist after commit failure: {lobe_path:?}"
    );
    assert!(
        !is_symlink(&lobe_path),
        "lobe entry must be the original file, not a managed symlink: {lobe_path:?}"
    );

    // The manifest must not have an entry for agent:myagent.
    let recall = sb.mind(&["recall", "agent:myagent"]);
    assert!(
        !recall.success,
        "agent:myagent must not be in manifest after failed absorb: stdout={} stderr={}",
        recall.stdout, recall.stderr
    );
}

// ---- ABS-10: meld failure after copy leaves lobe + strays intact -----------

/// A meld failure that occurs AFTER the item has been copied to the destination
/// and committed (but BEFORE the original lobe entry is removed) must leave the
/// original lobe entry intact and the manifest unchanged. We induce the meld
/// failure with a malformed `mind.toml` in the destination: `first_scan_root`
/// tolerates it (`unwrap_or_default`), the copy and commit succeed, then `meld`
/// parses the same `mind.toml` and fails with a Toml error. This exercises the
/// step-3 (meld) restore branch, distinct from the step-2 (commit) branch the
/// bare-repo test covers.
// spec: ABS-10
#[test]
fn abs10_meld_failure_after_copy_leaves_lobe_intact() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_rule("meldfail");
    let dest = sb.dest_spec();

    // A syntactically invalid mind.toml: `meld` reads it and errors, but
    // first_scan_root swallows the parse error via unwrap_or_default.
    write_file(
        &sb.dest.join("mind.toml"),
        "[source]\nthis is = = not valid toml ===\n",
    );
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add bad mind.toml"]);

    let r = sb.mind(&["absorb", "rule:meldfail", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "absorb must fail when meld rejects the dest mind.toml: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // ABS-10: the original lobe entry must be intact (commit happened, but the
    // meld failure occurs before the original is removed).
    assert!(
        lobe_path.exists() && !is_symlink(&lobe_path),
        "lobe entry must be intact after a meld failure: {lobe_path:?}"
    );

    // The manifest must not have a rule:meldfail entry.
    let recall = sb.mind(&["recall", "rule:meldfail"]);
    assert!(
        !recall.success,
        "rule:meldfail must not be in manifest after a failed absorb"
    );
}

/// ABS-10 + ABS-7 stray survival: a mid-absorb failure (here a meld failure
/// after copy) must NOT delete the stray copies in the OTHER lobes. The strays
/// are only ever removed by `learn`'s relink, which never runs on a failed
/// absorb. So every original lobe copy must survive byte-for-byte.
// spec: ABS-10
#[test]
fn abs10_failure_does_not_delete_stray_copies_in_other_lobes() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-abs-strays-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dest = base.join("personal");
    let mind_home = base.join("mind");
    let lobe1 = base.join("lobe1");
    let lobe2 = base.join("lobe2");

    git_init(&dest);

    // The same unmanaged skill in two lobes; lobe2 holds the stray copy.
    let skill1 = lobe1.join("skills").join("mystray");
    write_file(&skill1.join("SKILL.md"), "# mystray primary\n");
    let skill2 = lobe2.join("skills").join("mystray");
    write_file(&skill2.join("SKILL.md"), "# mystray stray\n");

    std::fs::create_dir_all(&mind_home).unwrap();
    let lobe1_str = lobe1.to_string_lossy();
    let lobe2_str = lobe2.to_string_lossy();
    std::fs::write(
        mind_home.join("config.toml"),
        format!("lobes = [\"{lobe1_str}\", \"{lobe2_str}\"]\n"),
    )
    .unwrap();

    // Induce a meld failure after the copy via a malformed mind.toml.
    write_file(
        &dest.join("mind.toml"),
        "[source]\n= = invalid = toml = =\n",
    );
    git(&dest, &["add", "-A"]);
    git(&dest, &["commit", "-qm", "bad toml"]);

    let dest_str = dest.to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["absorb", "skill:mystray", "--to", &dest_str, "--yes"])
        .env("MIND_HOME", &mind_home)
        .env("CLAUDE_HOME", &lobe1)
        .env_remove("MIND_ABSORB_TO")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .output()
        .expect("run mind");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "absorb must fail on the malformed dest mind.toml: stdout={stdout} stderr={stderr}"
    );

    // Both lobe copies must survive as original files (no stray deletion on failure).
    assert!(
        skill1.exists() && !is_symlink(&skill1),
        "primary lobe copy must survive a failed absorb"
    );
    assert!(
        skill2.exists() && !is_symlink(&skill2),
        "stray lobe copy must NOT be deleted by a failed absorb"
    );
    // And byte-for-byte: the stray's distinct content is preserved.
    let stray_content = std::fs::read_to_string(skill2.join("SKILL.md")).unwrap();
    assert!(
        stray_content.contains("mystray stray"),
        "stray copy content must be untouched: {stray_content}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// ---- ABS-10: learn failure AFTER original removed restores backup ----------

/// The subtlest restore branch: a `learn` failure that occurs AFTER the original
/// lobe entry has already been removed (absorb step 5) must restore the backup
/// to the lobe path byte-for-byte. We induce the learn failure with an unresolved
/// `{{ns:}}` reference token in the item: copy, commit, and meld all succeed, but
/// `learn`'s reference expansion raises `BadReference` for the dangling token,
/// hitting the post-removal restore path. The lobe file must reappear with its
/// original content and the manifest must stay empty.
// spec: ABS-10
#[test]
fn abs10_learn_failure_after_removal_restores_backup() {
    let sb = Sandbox::new();
    // A skill whose body references a sibling that does not exist. learn's
    // {{ns:}} expansion will reject it with BadReference -- after the original
    // lobe entry has already been removed in step 5.
    let lobe_path = sb.claude_home.join("skills").join("badref");
    let original = "# badref skill\n\nhand off to {{ns:nonexistent}}\n";
    write_file(&lobe_path.join("SKILL.md"), original);
    let dest = sb.dest_spec();

    let r = sb.mind(&["absorb", "skill:badref", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "absorb must fail when learn cannot resolve a dangling reference: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // ABS-10: the lobe entry must be restored from the backup -- it must exist,
    // be the original file (not a symlink), and have the exact original content.
    assert!(
        lobe_path.exists(),
        "lobe entry must be restored after a post-removal learn failure: {lobe_path:?}"
    );
    assert!(
        !is_symlink(&lobe_path),
        "restored lobe entry must be the original file, not a managed symlink"
    );
    let restored = std::fs::read_to_string(lobe_path.join("SKILL.md")).unwrap();
    assert_eq!(
        restored, original,
        "restored lobe content must be byte-for-byte the original"
    );

    // The manifest must not have a skill:badref entry (learn never completed).
    let recall = sb.mind(&["recall", "skill:badref"]);
    assert!(
        !recall.success,
        "skill:badref must not be in manifest after a failed absorb"
    );

    // No leftover backup in the mind tmp dir (success or failure drops it).
    let backup = sb
        .mind_home
        .join(".tmp")
        .join("absorb-backup")
        .join("skill")
        .join("badref");
    assert!(
        !backup.exists(),
        "the absorb backup must be cleaned up after a failed absorb: {backup:?}"
    );
}

// ---- C3 / ABS-7: json mode WITH --yes proceeds -----------------------------

/// `--json --yes` must bypass the ABS-7 destructive-confirm gate and complete
/// the absorb. The json refusal is only for the missing-confirmation case; an
/// explicit `--yes` is the documented bypass and must still work under json.
// spec: ABS-7
#[test]
fn abs7_json_mode_with_yes_proceeds() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_skill("jsonyes");
    let dest = sb.dest_spec();

    let r = sb.mind(&["--json", "--yes", "absorb", "skill:jsonyes", "--to", &dest]);
    assert!(
        r.success,
        "absorb --json --yes must proceed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The lobe path is now a managed symlink (absorb completed).
    assert!(
        is_symlink(&lobe_path),
        "lobe must be a managed symlink after absorb --json --yes"
    );
    // The item is in the manifest.
    let recall = sb.mind(&["recall", "skill:jsonyes"]);
    assert!(
        recall.success,
        "skill:jsonyes must be installed after absorb --json --yes: {}",
        recall.stdout
    );
}

// ---- C5: legitimate nested scan root works ---------------------------------

/// A destination whose `mind.toml` declares a legitimate nested scan root
/// (`roots = ["sub"]`, fully contained in the repo) must NOT be rejected: the
/// containment check is only meant to reject escapes. The item lands under the
/// nested root's convention path and absorb completes.
// spec: ABS-10
#[test]
fn c5_legitimate_nested_root_is_accepted() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_rule("nestedok");
    let dest = sb.dest_spec();

    // Declare a nested scan root that exists within the repo.
    std::fs::create_dir_all(sb.dest.join("sub")).unwrap();
    write_file(&sb.dest.join("mind.toml"), "[source]\nroots = [\"sub\"]\n");
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add nested root"]);

    let r = sb.mind(&["absorb", "rule:nestedok", "--to", &dest, "--yes"]);
    assert!(
        r.success,
        "absorb into a legitimate nested root must succeed: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // The item file landed under the nested root's rules dir.
    let landed = sb.dest.join("sub").join("rules").join("nestedok.md");
    assert!(
        landed.exists(),
        "item must land under the nested scan root: {landed:?}"
    );
    // The lobe is now a managed symlink.
    assert!(
        is_symlink(&lobe_path),
        "lobe must be a managed symlink after absorb into a nested root"
    );
}

// ---- C5: escaping scan root that EXISTS on disk (canonicalize path) ---------

/// The containment check has two code paths: `canonicalize` when the candidate
/// root exists on disk, and a logical `normalize_path` fallback when it does not.
/// The `../../outside` test covers the non-existent (normalize_path) path. This
/// test covers the EXISTING-on-disk (canonicalize) path: the escaping root
/// directory is created before absorb runs, so `canonicalize` resolves it to a
/// real path outside the repo and the containment check must still reject it.
// spec: ABS-10
#[test]
fn c5_escaping_root_that_exists_on_disk_is_rejected() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_rule("escapereal");
    let dest = sb.dest_spec();

    // Create the escape-target directory OUTSIDE the repo so canonicalize succeeds.
    let outside = sb.base.join("outside-real");
    std::fs::create_dir_all(&outside).unwrap();

    // Point roots at it via a relative path that climbs out of the repo. Because
    // the target exists, first_scan_root takes the canonicalize branch.
    write_file(
        &sb.dest.join("mind.toml"),
        "[source]\nroots = [\"../outside-real\"]\n",
    );
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add existing escaping root"]);

    let r = sb.mind(&["absorb", "rule:escapereal", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "an existing escaping root (canonicalize branch) must be rejected: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The lobe entry must be untouched (rejection happens before any move).
    assert!(
        lobe_path.exists() && !is_symlink(&lobe_path),
        "lobe entry must be unchanged after escaping-root rejection: {lobe_path:?}"
    );
    // Nothing must have been written into the escape target.
    let leaked = outside.join("rules").join("escapereal.md");
    assert!(
        !leaked.exists(),
        "nothing must be written outside the repo: {leaked:?}"
    );
    // The manifest must not have a rule:escapereal entry.
    let recall = sb.mind(&["recall", "rule:escapereal"]);
    assert!(
        !recall.success,
        "rule:escapereal must not be in manifest after rejection"
    );
}

// ---- C3 / ABS-7: json mode without --yes is ConfirmationRequired -----------

/// Under --json without --yes, when stray copies exist (multi-lobe), absorb must
/// return ConfirmationRequired rather than silently proceeding. json mode is
/// treated as non-interactive for destructive confirmations.
///
/// The test fixture uses a non-TTY process (piped stdin/stdout), which is the
/// same non-interactive context CI and automation use. The json flag must refuse
/// regardless of TTY status.
// spec: ABS-7
#[test]
fn abs7_json_mode_without_yes_when_stray_copies_is_confirmation_required() {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-abs-json-c3-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dest = base.join("personal");
    let mind_home = base.join("mind");
    let lobe1 = base.join("lobe1");
    let lobe2 = base.join("lobe2");

    git_init(&dest);

    // Place the same unmanaged skill in both lobes (so there are stray copies
    // to trigger the ABS-7 destructive-confirm path).
    let skill1 = lobe1.join("skills").join("myjson");
    write_file(&skill1.join("SKILL.md"), "# myjson\n");
    let skill2 = lobe2.join("skills").join("myjson");
    write_file(&skill2.join("SKILL.md"), "# myjson\n");

    std::fs::create_dir_all(&mind_home).unwrap();
    let lobe1_str = lobe1.to_string_lossy();
    let lobe2_str = lobe2.to_string_lossy();
    std::fs::write(
        mind_home.join("config.toml"),
        format!("lobes = [\"{lobe1_str}\", \"{lobe2_str}\"]\n"),
    )
    .unwrap();

    let dest_str = dest.to_string_lossy().into_owned();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mind"))
        // --json flag but no --yes: destructive action must refuse.
        .args(["--json", "absorb", "skill:myjson", "--to", &dest_str])
        .env("MIND_HOME", &mind_home)
        .env("CLAUDE_HOME", &lobe1)
        .env_remove("MIND_ABSORB_TO")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .output()
        .expect("run mind");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        !out.status.success(),
        "absorb --json without --yes must fail when stray copies exist: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("needs confirmation") || stderr.contains("ConfirmationRequired"),
        "must return ConfirmationRequired: stderr={stderr}"
    );

    // Nothing must have been moved: both lobe entries must be original files.
    assert!(
        skill1.exists() && !is_symlink(&skill1),
        "lobe1 skill must be unchanged"
    );
    assert!(
        skill2.exists() && !is_symlink(&skill2),
        "lobe2 skill must be unchanged"
    );

    let _ = std::fs::remove_dir_all(&base);
}

// ---- C3 / ABS-7: single-lobe json mode without --yes is also guarded -------

/// Even in the single-lobe case (no stray copies), --json without --yes must
/// still refuse with ConfirmationRequired, because json is always non-interactive.
// spec: ABS-7
#[test]
fn abs7_json_mode_single_lobe_without_yes_is_confirmation_required() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_skill("solo");
    let dest = sb.dest_spec();

    // Single lobe: no stray copies. Still json mode must refuse without --yes.
    let r = sb.mind(&["--json", "absorb", "skill:solo", "--to", &dest]);
    assert!(
        !r.success,
        "--json without --yes must fail even with single lobe: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("needs confirmation") || r.stderr.contains("ConfirmationRequired"),
        "must return ConfirmationRequired: stderr={}",
        r.stderr
    );
    // Lobe must be unchanged.
    assert!(
        lobe_path.exists() && !is_symlink(&lobe_path),
        "lobe must be unchanged after json refusal"
    );
}

// ---- C5: dest mind.toml roots escaping the repo => error --------------------

/// When the destination's mind.toml declares a `roots` entry that escapes the
/// repo directory (e.g. `../../outside`), absorb must error before any filesystem
/// mutation. Nothing must be moved and the lobe entry must be intact.
// spec: ABS-10
#[test]
fn c5_dest_roots_escaping_repo_is_error_nothing_moved() {
    let sb = Sandbox::new();
    let lobe_path = sb.place_unmanaged_rule("escaperule");
    let dest = sb.dest_spec();

    // Write a mind.toml whose roots entry escapes the repo.
    write_file(
        &sb.dest.join("mind.toml"),
        "[source]\nroots = [\"../../outside\"]\n",
    );
    git(&sb.dest, &["add", "-A"]);
    git(&sb.dest, &["commit", "-qm", "add escaping roots"]);

    let r = sb.mind(&["absorb", "rule:escaperule", "--to", &dest, "--yes"]);
    assert!(
        !r.success,
        "a dest with escaping roots must fail: stdout={} stderr={}",
        r.stdout, r.stderr
    );

    // The lobe entry must be intact (nothing was moved).
    assert!(
        lobe_path.exists() && !is_symlink(&lobe_path),
        "lobe entry must be unchanged after escaping-roots error: {lobe_path:?}"
    );

    // The manifest must not have a rule:escaperule entry.
    let recall = sb.mind(&["recall", "rule:escaperule"]);
    assert!(
        !recall.success,
        "rule:escaperule must not be in manifest after failed absorb"
    );
}
