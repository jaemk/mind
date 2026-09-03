//! Integration tests for `mind curate` (spec/curate.md, CUR-*): one pass that
//! reconciles the melded state with what the registered curators declare.
//!
//! Hermetic: local git repos in temp dirs, `MIND_HOME`/`CLAUDE_HOME` pointed at
//! temp dirs, no network. A curator melded by plain local path is a LINKED
//! source (CLI-27), so edits to its `mind.toml` are visible to the next command
//! with no fetch; the `--no-sync` test melds it pinned, which clones it, so
//! staleness becomes observable.

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
}

impl Env {
    fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-cur-{}-{n}", std::process::id()));
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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// A new git repo under this env, with one commit.
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

    /// The identity a local repo registers under: `local/<base>/<name>`.
    fn ident(&self, name: &str) -> String {
        format!(
            "local/{}/{name}",
            self.base.file_name().unwrap().to_string_lossy()
        )
    }

    fn sources_json(&self) -> String {
        std::fs::read_to_string(self.mind_home.join("sources.json")).unwrap_or_default()
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

    fn head(&self) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.path)
            .output()
            .expect("git rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A `[discover].sources` list naming `entries` verbatim.
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

/// A library repo with one skill, and a curator that lists it with `install`.
fn lib_and_curator(env: &Env, entry_extra: &str) -> (Repo, Repo) {
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff\n---\n# review\n",
    );
    let curator = env.repo("curator");
    curator.curate_list(&[format!("{{ source = \"{}\"{entry_extra} }}", lib.spec())]);
    (lib, curator)
}

#[test]
fn curate_reports_up_to_date_when_nothing_is_curated() {
    // spec: CUR-1
    // No curators registered at all is a clean report, not an error.
    let env = Env::new();
    let plain = env.repo("plain");
    plain.write_and_commit(
        "skills/solo/SKILL.md",
        "---\ndescription: solo\n---\n# solo\n",
    );
    let m = env.mind(&["meld", &plain.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let r = env.mind(&["curate"]);
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("up to date"),
        "expected a clean report: {}",
        r.stdout
    );
}

#[test]
fn a_newly_listed_entry_is_registered_and_installed() {
    // spec: CUR-1 CUR-3 CUR-4
    // The gap `sync` leaves: it registers a newly listed entry but installs
    // nothing, even when the curator marked it `install = true`.
    let env = Env::new();
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff\n---\n# review\n",
    );
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate --check failed: {}", plan.stderr);
    assert!(
        plan.stdout.contains("register") && plan.stdout.contains(&env.ident("lib")),
        "the plan must name the unregistered entry: {}",
        plan.stdout
    );

    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    assert!(
        env.claude_home.join("skills/review").exists(),
        "the entry's declared items must install in the same run: {}",
        applied.stdout
    );
}

#[test]
fn check_changes_nothing() {
    // spec: CUR-9
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    // Register the curator only; the entry is listed but not yet registered.
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let before = env.sources_json();
    let r = env.mind(&["curate", "--check", "--yes"]);
    assert!(r.success, "curate --check failed: {}", r.stderr);
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "--check must install nothing"
    );
    assert_eq!(
        before,
        env.sources_json(),
        "--check must not touch the registry, even with --yes"
    );
}

#[test]
fn declared_items_that_are_not_installed_are_offered() {
    // spec: CUR-4 CUR-12
    // The entry is registered (a `sync` re-walk got there first) but its
    // declared items were never installed.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let s = env.mind(&["sync"]);
    assert!(s.success, "sync failed: {} {}", s.stdout, s.stderr);
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "sync registers the entry but installs nothing (DSC-57)"
    );

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("install") && plan.stdout.contains("skill:review"),
        "the plan must name the uninstalled declared item: {}",
        plan.stdout
    );
    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    assert!(
        env.claude_home.join("skills/review").exists(),
        "applying must install it: {}",
        applied.stdout
    );
}

#[test]
fn a_register_only_entry_proposes_no_install() {
    // spec: CUR-4 CUR-12
    // An entry declaring neither `install` nor `install-items` is register-only
    // by the curator's choice; `curate` never installs from it.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, "");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let r = env.mind(&["curate", "--yes"]);
    assert!(r.success, "curate failed: {}", r.stderr);
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "a register-only entry installs nothing: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("up to date"),
        "and it is not reported as pending: {}",
        r.stdout
    );
}

#[test]
fn install_items_offers_only_the_declared_subset() {
    // spec: CUR-4
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    lib.write_and_commit(
        "skills/extra/SKILL.md",
        "---\ndescription: Extra\n---\n# extra\n",
    );
    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install-items = [\"skill:review\"] }}",
        lib.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let r = env.mind(&["curate", "--yes"]);
    assert!(r.success, "curate failed: {}", r.stderr);
    assert!(
        env.claude_home.join("skills/review").exists(),
        "the declared item installs: {}",
        r.stdout
    );
    assert!(
        !env.claude_home.join("skills/extra").exists(),
        "an undeclared item must not: {}",
        r.stdout
    );
}

#[test]
fn a_changed_pin_directive_is_reported_and_applied() {
    // spec: CUR-5
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let first = lib.head();
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    // Upstream moves, and the curator pins the older commit.
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\nsecond\n",
    );
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-ref = \"{first}\" }}",
        lib.spec()
    )]);

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("repin"),
        "a changed pin directive must be reported: {}",
        plan.stdout
    );
    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains(&first[..8]),
        "the source must be re-pinned to the curator's commit: {}",
        sources.stdout
    );
}

#[test]
fn an_outdated_curated_source_is_reported_and_upgraded() {
    // spec: CUR-6
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\nupdated\n",
    );
    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("upgrade"),
        "an out-of-date curated source must be reported: {}",
        plan.stdout
    );
    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    let installed =
        std::fs::read_to_string(env.claude_home.join("skills/review/SKILL.md")).expect("installed");
    assert!(
        installed.contains("updated"),
        "applying must land the new content: {installed}"
    );
}

#[test]
fn an_unlisted_source_is_reported_but_pruned_only_with_prune() {
    // spec: CUR-7
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    // The curator drops the entry.
    curator.write_and_commit("mind.toml", "[discover]\nsources = []\n");

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.stdout.contains("unlist") && plan.stdout.contains(&env.ident("lib")),
        "a dropped entry must be reported: {}",
        plan.stdout
    );

    let yes = env.mind(&["curate", "--yes"]);
    assert!(yes.success, "curate --yes failed: {}", yes.stderr);
    assert!(
        env.claude_home.join("skills/review").exists(),
        "--yes alone must not uninstall a dropped entry's items: {}",
        yes.stdout
    );

    let pruned = env.mind(&["curate", "--yes", "--prune"]);
    assert!(pruned.success, "curate --prune failed: {}", pruned.stderr);
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "--prune applies the unlist: {}",
        pruned.stdout
    );
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains(&env.ident("lib")),
        "and drops the source: {}",
        sources.stdout
    );
}

#[test]
fn a_directly_melded_source_is_never_unlisted() {
    // spec: CUR-7 STO-82
    // Provenance is what separates "no longer listed" from "never curated": a
    // source melded by hand records no curator and is never proposed.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[discover]\nsources = []\n");
    for spec in [lib.spec(), curator.spec()] {
        let m = env.mind(&["meld", &spec, "--register-only"]);
        assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    }
    let json = env.sources_json();
    assert!(
        !json.contains("curated_by"),
        "a direct meld records no curator: {json}"
    );
    let r = env.mind(&["curate", "--check"]);
    assert!(
        !r.stdout.contains("unlist"),
        "a directly melded source must never be proposed for unlisting: {}",
        r.stdout
    );
}

#[test]
fn a_curated_meld_records_its_curator() {
    // spec: STO-82
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let json = env.sources_json();
    assert!(
        json.contains("\"curated_by\"") && json.contains(&env.ident("curator")),
        "the nested source must record the curator that listed it: {json}"
    );
}

#[test]
fn a_changed_namespace_is_advisory_only() {
    // spec: CUR-8
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, namespace = \"acme\" }}",
        lib.spec()
    )]);
    let r = env.mind(&["curate", "--yes"]);
    assert!(r.success, "curate failed: {}", r.stderr);
    assert!(
        r.stdout.contains("namespace") && r.stdout.contains("mind unmeld"),
        "the advisory must name the two-step adoption: {}",
        r.stdout
    );
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains("@acme"),
        "an advisory change must never be applied: {}",
        sources.stdout
    );
}

#[test]
fn no_sync_plans_against_the_clone_on_disk() {
    // spec: CUR-2
    // A pinned local curator is cloned (CLI-27), so an edit upstream is
    // invisible until the fetch `curate` performs by default.
    let env = Env::new();
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[discover]\nsources = []\n");
    let m = env.mind(&[
        "meld",
        &curator.spec(),
        "--pin",
        "branch=main",
        "--register-only",
    ]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);

    let stale = env.mind(&["curate", "--check", "--no-sync"]);
    assert!(stale.success, "curate --no-sync failed: {}", stale.stderr);
    assert!(
        stale.stdout.contains("up to date"),
        "--no-sync plans against the clone, which has not seen the new entry: {}",
        stale.stdout
    );

    let fresh = env.mind(&["curate", "--check"]);
    assert!(
        fresh.stdout.contains("register"),
        "the default fetch must pick the new entry up: {}",
        fresh.stdout
    );
}

#[test]
fn json_reports_the_plan_and_what_was_applied() {
    // spec: CUR-13 CUR-14
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let pending = env.mind(&["curate", "--json", "--check"]);
    assert!(
        pending.success,
        "pending changes must still exit 0 (CUR-14): {}",
        pending.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&pending.stdout).expect("json");
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["action"], "curate");
    assert_eq!(doc["outcome"], "pending");
    assert_eq!(doc["changes"][0]["kind"], "install");
    assert!(doc["applied"].is_null(), "nothing applied under --check");

    let applied = env.mind(&["curate", "--json", "--yes"]);
    assert!(applied.success, "apply failed: {}", applied.stderr);
    let doc: serde_json::Value = serde_json::from_str(&applied.stdout).expect("json");
    assert_eq!(doc["outcome"], "applied");
    assert!(
        doc["applied"][0]
            .as_str()
            .is_some_and(|s| s.starts_with("install:")),
        "the applied list names each change: {}",
        applied.stdout
    );
}

#[test]
fn changes_apply_in_a_fixed_order() {
    // spec: CUR-10
    // A plan with a register and an upgrade applies the register first, so a
    // newly listed entry's items are installed before the upgrade pass runs.
    let env = Env::new();
    let first = env.repo("first");
    first.write_and_commit("skills/one/SKILL.md", "---\ndescription: One\n---\n# one\n");
    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true }}",
        first.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    // `first` moves upstream (an upgrade), and a second entry appears (a register).
    first.write_and_commit(
        "skills/one/SKILL.md",
        "---\ndescription: One\n---\n# one\nupdated\n",
    );
    let second = env.repo("second");
    second.write_and_commit("skills/two/SKILL.md", "---\ndescription: Two\n---\n# two\n");
    curator.curate_list(&[
        format!("{{ source = \"{}\", install = true }}", first.spec()),
        format!("{{ source = \"{}\", install = true }}", second.spec()),
    ]);

    let r = env.mind(&["curate", "--json", "--yes"]);
    assert!(r.success, "curate failed: {}", r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).expect("json");
    let applied: Vec<&str> = doc["applied"]
        .as_array()
        .expect("applied")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    let register_at = applied.iter().position(|a| a.starts_with("register:"));
    let upgrade_at = applied.iter().position(|a| a.starts_with("upgrade:"));
    assert!(
        register_at < upgrade_at,
        "register must apply before upgrade: {applied:?}"
    );
    assert!(
        env.claude_home.join("skills/two").exists(),
        "the newly listed entry's item installs"
    );
}

#[test]
fn hook_consent_passes_through_to_the_registrations_curate_applies() {
    // spec: CUR-11
    // `curate` adds no hook path of its own: a newly registered entry's source
    // install hook takes the ordinary consent route (skipped on a non-TTY run,
    // HOOK-22), and the skip flag passes through to it.
    let env = Env::new();
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[discover]\nsources = []\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let marker = env.base.join("hook-ran");
    lib.write_and_commit(
        "mind.toml",
        &format!(
            "[[hooks]]\nname = \"setup\"\nrun = \"touch {}\"\nevent = \"install\"\n",
            marker.display()
        ),
    );
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);

    // No TTY and no skip flag: the hook is disclosed and skipped, not run.
    let guarded = env.mind(&["curate", "--yes"]);
    assert!(guarded.success, "curate failed: {}", guarded.stderr);
    assert!(
        !marker.exists(),
        "an unconsented hook must not run: {} {}",
        guarded.stdout,
        guarded.stderr
    );

    // With the skip flag it runs, on the same path any meld would take.
    let un = env.mind(&["unmeld", &env.ident("lib"), "--yes"]);
    assert!(un.success, "unmeld failed: {} {}", un.stdout, un.stderr);
    let skipped = env.mind(&["curate", "--yes", "--dangerously-skip-install-hook-check"]);
    assert!(skipped.success, "curate failed: {}", skipped.stderr);
    assert!(
        marker.exists(),
        "the skip flag must reach the registration's hook: {} {}",
        skipped.stdout,
        skipped.stderr
    );
}
