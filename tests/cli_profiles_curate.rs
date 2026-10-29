//! Integration tests for `curate` over install profiles (spec/profiles.md,
//! PROF-27, with PROF-5, PROF-23, PROF-26): a curator entry that names
//! `profiles` proposes the members missing from the source's snapshot, and
//! applying it records the entry's profiles on the source.
//!
//! Hermetic: local git repos in temp dirs, `MIND_HOME`/`CLAUDE_HOME` pointed at
//! temp dirs, no network. Repos melded by plain local path are linked
//! sources, so edits are visible to the next command with no fetch.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Env {
    base: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Repo {
    path: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
    code: Option<i32>,
}

impl Env {
    fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-profcur-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Env {
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
            base,
        }
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
            code: out.status.code(),
        }
    }

    fn repo(&self, name: &str) -> Repo {
        let path = self.base.join(name);
        write(&path.join("README.md"), "# fixture\n");
        git(&path, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&path, &["config", "user.email", "t@t"]);
        git(&path, &["config", "user.name", "t"]);
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "initial"]);
        Repo { path }
    }

    fn sources_json(&self) -> String {
        std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap_or_default()
    }

    fn installed(&self, skill: &str) -> bool {
        self.claude_home.join("skills").join(skill).exists()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

impl Repo {
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.path.join(rel), contents);
        git(&self.path, &["add", "-A"]);
        git(&self.path, &["commit", "-qm", "fixture"]);
    }

    fn spec(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn skill(&self, name: &str) {
        self.write_and_commit(
            &format!("skills/{name}/SKILL.md"),
            &format!("---\ndescription: {name}\n---\n# {name}\n"),
        );
    }

    fn curate_list(&self, entries: &[String]) {
        self.write_and_commit(
            "mind.toml",
            &format!("[discover]\nsources = [\n{}\n]\n", entries.join(",\n")),
        );
    }
}

fn write(path: &Path, contents: &str) {
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

/// A library with skills a, b, c and profiles `basic = [a]`, `extra = [b]`.
fn lib(env: &Env) -> Repo {
    let lib = env.repo("lib");
    lib.skill("a");
    lib.skill("b");
    lib.skill("c");
    lib.write_and_commit(
        "mind.toml",
        "[profiles]\nbasic = [\"skill:a\"]\nextra = [\"skill:b\"]\n",
    );
    lib
}

/// Meld a curator listing `lib` with `entry_extra`, and let the first curate
/// register the entry (register-only unless the extra says otherwise).
fn curator_with(env: &Env, lib: &Repo, entry_extra: &str) -> Repo {
    let curator = env.repo("curator");
    curator.curate_list(&[format!("{{ source = \"{}\"{entry_extra} }}", lib.spec())]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    curator
}

fn lib_ident(env: &Env) -> String {
    format!(
        "local/{}/lib",
        env.base.file_name().unwrap().to_string_lossy()
    )
}

#[test]
fn an_entry_adding_a_profile_proposes_and_installs_its_members_and_records_it() {
    // spec: PROF-27 PROF-26 PROF-19
    let env = Env::new();
    let lib = lib(&env);
    // Register the entry with no install directive (register-only).
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    assert!(
        !env.installed("a"),
        "a register-only entry installs nothing"
    );

    // The curator now names a profile for the entry.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.success,
        "check failed: {} {}",
        plan.stdout, plan.stderr
    );
    assert!(
        plan.stdout.contains("skill:a") && plan.stdout.contains("basic"),
        "the plan names the member and the profile: {}",
        plan.stdout
    );
    assert!(
        !plan.stdout.contains("skill:b"),
        "only the named profile's members: {}",
        plan.stdout
    );

    let applied = env.mind(&["curate", "--yes"]);
    assert!(
        applied.success,
        "curate failed: {} {}",
        applied.stdout, applied.stderr
    );
    assert!(
        env.installed("a"),
        "the member installs: {}",
        applied.stdout
    );
    assert!(!env.installed("b"), "a non-member does not");
    assert!(!env.installed("c"));
    let sources = env.sources_json();
    assert!(
        sources.contains("\"profiles\"") && sources.contains("basic"),
        "the entry's profile is recorded on the source: {sources}"
    );
    assert!(
        sources.contains("skill:a"),
        "and the snapshot records the member: {sources}"
    );

    let again = env.mind(&["curate", "--check"]);
    assert!(
        again.stdout.contains("up to date"),
        "nothing missing proposes nothing: {}",
        again.stdout
    );
}

#[test]
fn a_new_entry_naming_profiles_installs_them_at_registration() {
    // spec: PROF-26 PROF-27
    let env = Env::new();
    let lib = lib(&env);
    // Meld the curator while it lists nothing, then list the entry, so the
    // entry is unregistered when `curate` first sees it (CUR-3).
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("register") && plan.stdout.contains("basic"),
        "the register line names the profiles: {}",
        plan.stdout
    );
    let applied = env.mind(&["curate", "--yes"]);
    assert!(
        applied.success,
        "curate failed: {} {}",
        applied.stdout, applied.stderr
    );
    assert!(env.installed("a"), "{}", applied.stdout);
    assert!(!env.installed("b"));
    assert!(env.sources_json().contains("basic"));
}

#[test]
fn a_member_the_maintainer_adds_to_an_enabled_profile_is_proposed_alone() {
    // spec: PROF-27 PROF-21
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    assert!(env.installed("a"));

    lib.write_and_commit(
        "mind.toml",
        "[profiles]\nbasic = [\"skill:a\", \"skill:c\"]\nextra = [\"skill:b\"]\n",
    );
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("skill:c") && !plan.stdout.contains("skill:a"),
        "only the new member is proposed: {}",
        plan.stdout
    );
    let applied = env.mind(&["curate", "--yes"]);
    assert!(
        applied.success,
        "curate failed: {} {}",
        applied.stdout, applied.stderr
    );
    assert!(env.installed("c"), "{}", applied.stdout);
    assert!(!env.installed("b"));
}

#[test]
fn a_member_the_consumer_forgot_is_not_reproposed() {
    // spec: PROF-27 PROF-23
    // Unlike CUR-4's ordinary rule, a forgotten profile member stays in the
    // snapshot and is not offered again while it remains a member.
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    assert!(env.installed("a"));

    let forget = env.mind(&["forget", "skill:a", "--yes"]);
    assert!(
        forget.success,
        "forget failed: {} {}",
        forget.stdout, forget.stderr
    );
    assert!(!env.installed("a"));

    let again = env.mind(&["curate", "--yes"]);
    assert!(
        again.success,
        "curate failed: {} {}",
        again.stdout, again.stderr
    );
    assert!(
        again.stdout.contains("up to date"),
        "the forgotten member is not re-proposed: {}",
        again.stdout
    );
    assert!(!env.installed("a"), "and not reinstalled");
}

#[test]
fn an_undeclared_profile_in_an_entry_is_a_bad_reference_that_skips_the_entry() {
    // spec: PROF-27 PROF-26 CUR-15
    let env = Env::new();
    let lib = lib(&env);
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"nope\"] }}",
        lib.spec()
    )]);

    let run = env.mind(&["curate", "--yes", "--json"]);
    assert!(
        run.success,
        "a bad entry must not abort: {} {}",
        run.stdout, run.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&run.stdout).expect("json");
    assert_eq!(doc["changes"], serde_json::json!([]), "{}", run.stdout);
    assert_eq!(
        doc["skipped"][0]["reason"], "profile_selection_invalid",
        "{}",
        run.stdout
    );
    assert!(!env.installed("a"));
    // The text run prints the warning (json mode stays quiet, CUR-13).
    let text = env.mind(&["curate", "--check"]);
    assert!(text.success, "{} {}", text.stdout, text.stderr);
    assert!(
        text.stderr.contains("nope"),
        "the warning names the profile: {}",
        text.stderr
    );
}

#[test]
fn a_profile_naming_an_item_the_source_lacks_is_a_bad_reference() {
    // spec: PROF-5 PROF-27
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    lib.write_and_commit("mind.toml", "[profiles]\nbasic = [\"skill:ghost\"]\n");
    // Name the profile in the curator's entry now.
    let curator_path = env.base.join("curator");
    write(
        &curator_path.join("mind.toml"),
        &format!(
            "[discover]\nsources = [\n{{ source = \"{}\", profiles = [\"basic\"] }}\n]\n",
            lib.spec()
        ),
    );
    git(&curator_path, &["add", "-A"]);
    git(&curator_path, &["commit", "-qm", "profiles"]);

    let run = env.mind(&["curate", "--yes"]);
    assert!(
        run.success,
        "a bad member must not abort: {} {}",
        run.stdout, run.stderr
    );
    assert!(
        run.stderr.contains("ghost"),
        "the warning names the missing member: {}",
        run.stderr
    );
    assert!(!env.installed("a"));
}

#[test]
fn check_applies_nothing_and_json_reports_the_plan() {
    // spec: PROF-27 CUR-9 CUR-13
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    lib.write_and_commit(
        "mind.toml",
        "[profiles]\nbasic = [\"skill:a\", \"skill:c\"]\n",
    );

    let before = env.sources_json();
    let checked = env.mind(&["curate", "--check", "--yes", "--json"]);
    assert!(
        checked.success,
        "check failed: {} {}",
        checked.stdout, checked.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&checked.stdout).expect("json");
    assert_eq!(doc["outcome"], "pending");
    assert_eq!(doc["changes"][0]["kind"], "install");
    assert_eq!(doc["changes"][0]["source"], lib_ident(&env));
    let detail = doc["changes"][0]["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("skill:c") && detail.contains("basic"),
        "the detail names the member and the profile: {detail}"
    );
    assert_eq!(doc["applied"], serde_json::json!([]));
    assert!(!env.installed("c"), "--check installs nothing");
    // Only the check's sync may have touched the registry; the entry's
    // recorded profile state is unchanged.
    let after = env.sources_json();
    assert_eq!(
        before.contains("skill:c"),
        after.contains("skill:c"),
        "--check must not record the new member"
    );

    let applied = env.mind(&["curate", "--yes", "--json"]);
    assert!(
        applied.success,
        "apply failed: {} {}",
        applied.stdout, applied.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&applied.stdout).expect("json");
    assert_eq!(doc["outcome"], "applied");
    assert!(
        doc["applied"][0]
            .as_str()
            .is_some_and(|s| s.starts_with("install:")),
        "{}",
        applied.stdout
    );
    assert!(env.installed("c"), "{}", applied.stdout);
}

#[test]
fn an_entry_whose_members_are_all_installed_still_records_its_profiles() {
    // spec: PROF-27 PROF-19
    let env = Env::new();
    let lib = lib(&env);
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(
        first.success,
        "curate failed: {} {}",
        first.stdout, first.stderr
    );
    // The consumer hand-learns the member before the curator names the profile.
    let learn = env.mind(&["learn", "skill:a", "--yes"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(env.installed("a"));
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);

    let before = env.sources_json();
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("records profiles basic") && plan.stdout.contains("already installed"),
        "{}",
        plan.stdout
    );
    assert_eq!(
        before.contains("\"profiles\""),
        env.sources_json().contains("\"profiles\""),
        "--check records nothing"
    );

    let applied = env.mind(&["curate", "--yes"]);
    assert!(
        applied.success,
        "curate failed: {} {}",
        applied.stdout, applied.stderr
    );
    let sources = env.sources_json();
    assert!(
        sources.contains("\"profiles\"") && sources.contains("basic"),
        "the profile is recorded: {sources}"
    );
    let again = env.mind(&["curate", "--check"]);
    assert!(again.stdout.contains("up to date"), "{}", again.stdout);
}

/// A library with a `default` profile: default = [basic], basic = [a],
/// extra = [b], full = [basic, skill:c] (so `full` includes `basic`).
fn lib_with_default(env: &Env) -> Repo {
    let lib = env.repo("lib");
    lib.skill("a");
    lib.skill("b");
    lib.skill("c");
    lib.write_and_commit(
        "mind.toml",
        "[profiles]\ndefault = [\"basic\"]\nbasic = [\"skill:a\"]\nextra = [\"skill:b\"]\nfull = [\"basic\", \"skill:c\"]\n",
    );
    lib
}

fn json(run: &Run) -> serde_json::Value {
    serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("json: {e}: {}", run.stdout))
}

#[test]
fn register_with_install_true_offers_the_nested_default_expansion_only() {
    // spec: PROF-26 CUR-3 CUR-4
    let env = Env::new();
    let lib = lib_with_default(&env);
    let _curator = curator_with(&env, &lib, ", install = true");
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(
        env.installed("a"),
        "default expansion installs: {}",
        run.stdout
    );
    assert!(!env.installed("b"), "non-default profile member stays out");
    assert!(!env.installed("c"), "an item in no profile stays out");
    assert!(env.sources_json().contains("default"), "default recorded");
}

#[test]
fn registered_entry_with_install_true_and_nested_default_installs_default_only() {
    // spec: PROF-26 PROF-12 CUR-4
    let env = Env::new();
    let lib = lib_with_default(&env);
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(first.success, "{} {}", first.stdout, first.stderr);
    assert!(!env.installed("a"));
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);
    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.stdout.contains("skill:a"), "{}", plan.stdout);
    assert!(!plan.stdout.contains("skill:c"), "{}", plan.stdout);
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("a"));
    assert!(!env.installed("b") && !env.installed("c"));
}

#[test]
fn profiles_with_install_items_is_one_install_change_covering_the_union() {
    // spec: PROF-26 PROF-27 CUR-4
    let env = Env::new();
    let lib = lib(&env);
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(first.success, "{} {}", first.stdout, first.stderr);
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"], install-items = [\"skill:c\"] }}",
        lib.spec()
    )]);
    let plan = env.mind(&["curate", "--check", "--json"]);
    assert!(plan.success, "{} {}", plan.stdout, plan.stderr);
    let doc = json(&plan);
    let changes = doc["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "one change: {}", plan.stdout);
    assert_eq!(changes[0]["kind"], "install");
    let detail = changes[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("skill:a") && detail.contains("skill:c") && !detail.contains("skill:b"),
        "{detail}"
    );
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("a") && env.installed("c"), "{}", run.stdout);
    assert!(!env.installed("b"));
    let again = env.mind(&["curate", "--check"]);
    assert!(again.stdout.contains("up to date"), "{}", again.stdout);
}

#[test]
fn a_transitive_profile_include_installs_through_curate() {
    // spec: PROF-5 PROF-27
    let env = Env::new();
    let lib = lib_with_default(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"full\"]");
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("a"), "included basic member: {}", run.stdout);
    assert!(env.installed("c"), "direct member");
    assert!(!env.installed("b"), "a profile not included stays out");
    let src = env.sources_json();
    assert!(src.contains("full"), "{src}");
}

#[test]
fn a_transitive_member_added_upstream_is_proposed_alone() {
    // spec: PROF-5 PROF-27 PROF-21
    let env = Env::new();
    let lib = lib_with_default(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"full\"]");
    let first = env.mind(&["curate", "--yes"]);
    assert!(first.success, "{} {}", first.stdout, first.stderr);
    lib.write_and_commit(
        "mind.toml",
        "[profiles]\ndefault = [\"basic\"]\nbasic = [\"skill:a\", \"skill:b\"]\nextra = [\"skill:b\"]\nfull = [\"basic\", \"skill:c\"]\n",
    );
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("skill:b") && !plan.stdout.contains("skill:c"),
        "{}",
        plan.stdout
    );
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("b"));
}

#[test]
fn a_non_tty_run_without_yes_reports_the_profile_plan_and_applies_nothing() {
    // spec: PROF-27 CUR-9
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let run = env.mind(&["curate"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(run.stdout.contains("basic"), "plan printed: {}", run.stdout);
    assert!(!env.installed("a"), "nothing applied without --yes");
    let sources = env.sources_json();
    assert!(!sources.contains("basic"), "no profile recorded: {sources}");
    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "{} {}", applied.stdout, applied.stderr);
    assert!(env.installed("a"));
}

#[test]
fn no_sync_still_plans_and_applies_profile_entries_from_local_state() {
    // spec: PROF-27 CUR-2 CUR-19
    let env = Env::new();
    let lib = lib(&env);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let run = env.mind(&["curate", "--no-sync", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("a"), "{}", run.stdout);
    assert!(!env.installed("b"));
    let again = env.mind(&["curate", "--no-sync", "--check"]);
    assert!(again.stdout.contains("up to date"), "{}", again.stdout);
}

#[test]
fn a_profile_entry_never_touches_a_source_owned_by_a_direct_meld() {
    // spec: CUR-12 PROF-27
    let env = Env::new();
    let lib = lib(&env);
    let m = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(m.success, "{} {}", m.stdout, m.stderr);
    let _curator = curator_with(&env, &lib, ", profiles = [\"basic\"]");
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(
        !env.installed("a"),
        "a direct meld is not mutated: {}",
        run.stdout
    );
    assert!(!env.sources_json().contains("basic"), "no profile recorded");
}

#[test]
fn a_profile_entry_never_touches_a_source_owned_by_another_curator() {
    // spec: CUR-12 PROF-27
    let env = Env::new();
    let lib = lib(&env);
    let owner = env.repo("owner");
    owner.curate_list(&[format!("{{ source = \"{}\" }}", lib.spec())]);
    let m = env.mind(&["meld", &owner.spec(), "--register-only"]);
    assert!(m.success, "{} {}", m.stdout, m.stderr);
    let first = env.mind(&["curate", "--yes"]);
    assert!(first.success, "{} {}", first.stdout, first.stderr);
    // A second curator lists the same source with a profile.
    let other = env.repo("other");
    other.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);
    let m = env.mind(&["meld", &other.spec(), "--register-only"]);
    assert!(m.success, "{} {}", m.stdout, m.stderr);
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(
        !env.installed("a"),
        "not the owner's source: {}",
        run.stdout
    );
    assert!(!env.sources_json().contains("basic"));
}

#[test]
fn default_profile_false_with_a_named_profile_installs_only_that_profile() {
    // spec: PROF-26 PROF-10 PROF-27
    let env = Env::new();
    let lib = lib_with_default(&env);
    let _curator = curator_with(
        &env,
        &lib,
        ", profiles = [\"extra\"], default-profile = false",
    );
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(env.installed("b"), "{}", run.stdout);
    assert!(!env.installed("a"), "default opted out");
    assert!(!env.installed("c"));
}

#[test]
fn default_profile_false_with_install_true_and_no_profiles_installs_nothing() {
    // spec: PROF-26 PROF-10
    let env = Env::new();
    let lib = lib_with_default(&env);
    let _curator = curator_with(&env, &lib, ", install = true, default-profile = false");
    let run = env.mind(&["curate", "--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert!(
        !env.installed("a") && !env.installed("b") && !env.installed("c"),
        "{}",
        run.stdout
    );
    let again = env.mind(&["curate", "--check"]);
    assert!(again.success, "{} {}", again.stdout, again.stderr);
    assert!(again.stdout.contains("up to date"), "{}", again.stdout);
}

/// A library like `lib`, but skill `a` declares an install hook that fails.
fn lib_with_failing_hook(env: &Env) -> Repo {
    let lib = lib(env);
    lib.write_and_commit(
        "skills/a/SKILL.md",
        "---\ndescription: a\ninstall: exit 7\n---\n# a\n",
    );
    lib
}

/// Assert a curate run stopped on the hook (exit 3, `hook-failed`) rather
/// than skipping the entry as `profile_selection_invalid`.
fn assert_hook_stop(run: &Run) {
    assert_eq!(run.code, Some(3), "{} {}", run.stdout, run.stderr);
    let doc = json(run);
    assert_eq!(doc["error"]["kind"], "hook-failed", "{doc}");
    assert!(
        !run.stdout.contains("profile_selection_invalid"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_failed_member_hook_at_registration_stops_the_curate() {
    // spec: HOOK-172 PROF-26
    let env = Env::new();
    let lib = lib_with_failing_hook(&env);
    // List the entry after the curator is melded, so `curate` registers it
    // (CUR-3) and installs its profiles in the same step.
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);
    let run = env.mind(&["curate", "--yes", "--json", "--accept-hooks"]);
    assert_hook_stop(&run);
}

#[test]
fn a_failed_member_hook_on_a_registered_entry_stops_the_curate() {
    // spec: HOOK-172 PROF-27
    let env = Env::new();
    let lib = lib_with_failing_hook(&env);
    let curator = curator_with(&env, &lib, "");
    let first = env.mind(&["curate", "--yes"]);
    assert!(first.success, "{} {}", first.stdout, first.stderr);
    curator.curate_list(&[format!(
        "{{ source = \"{}\", profiles = [\"basic\"] }}",
        lib.spec()
    )]);
    let run = env.mind(&["curate", "--yes", "--json", "--accept-hooks"]);
    assert_hook_stop(&run);
}
