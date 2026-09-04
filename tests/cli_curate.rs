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
        self.mind_env(args, &[], None)
    }

    /// `mind` with extra environment and optional scripted stdin. With
    /// `("MIND_TTY", "1")` (HOOK-109, the same seam `cli_hooks.rs` and
    /// `cli_install_items.rs` use) plus a reply, the CUR-9 confirmation prompt
    /// is drivable from a headless test: the child gets a pipe, not a
    /// terminal, so without the override `hook::is_tty()` is false and
    /// `should_apply` never asks. `stdin: None` gives the child `/dev/null`,
    /// whose EOF `read_confirm` reads as No.
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(match stdin {
                Some(_) => Stdio::piped(),
                None => Stdio::null(),
            });
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("run mind");
        if let Some(text) = stdin {
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(text.as_bytes())
                .expect("write stdin");
        }
        let out = child.wait_with_output().expect("wait for mind");
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
    // spec: CUR-13 -- `applied` is always present, even empty, not omitted.
    assert_eq!(
        doc["applied"],
        serde_json::json!([]),
        "nothing applied under --check, but the key must still be an empty array: {}",
        pending.stdout
    );

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
    let register_at = applied
        .iter()
        .position(|a| a.starts_with("register:"))
        .expect("a register change must have applied");
    let upgrade_at = applied
        .iter()
        .position(|a| a.starts_with("upgrade:"))
        .expect("an upgrade change must have applied");
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

#[test]
fn a_marketplace_entry_still_listed_is_not_proposed_for_unlisting() {
    // spec: CUR-4 CUR-7
    // A marketplace catalog curates too (MKT-7): its entries declare no install
    // directive, so they propose nothing while they are listed, and drop out of
    // the listed set (an `unlist`) when the manifest stops naming them.
    let env = Env::new();
    let plugin = env.repo("plugin");
    plugin.write_and_commit("skills/kit/SKILL.md", "---\ndescription: Kit\n---\n# kit\n");
    let catalog = env.repo("catalog");
    // An external entry: a source object with a `url`, which for a hermetic
    // fixture is the plugin repo's local path.
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"Cat","plugins":[{{"name":"kit","source":{{"url":"{}"}}}}]}}"#,
            plugin.spec()
        ),
    );
    let m = env.mind(&["meld", &catalog.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("@kit"),
        "the external marketplace entry must register: {}",
        sources.stdout
    );

    let listed = env.mind(&["curate", "--check"]);
    assert!(listed.success, "curate failed: {}", listed.stderr);
    assert!(
        !listed.stdout.contains("unlist"),
        "an entry still in the manifest must not be proposed for unlisting: {}",
        listed.stdout
    );

    // The catalog drops the entry.
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"Cat","plugins":[]}"#,
    );
    let dropped = env.mind(&["curate", "--check"]);
    assert!(
        dropped.stdout.contains("unlist") && dropped.stdout.contains("@kit"),
        "a dropped catalog entry must be reported: {}",
        dropped.stdout
    );
}

#[test]
fn an_entry_naming_a_directly_melded_source_proposes_nothing_against_it() {
    // spec: CUR-4 CUR-5 CUR-12 CUR-16
    // A curator's entry can resolve to the same identity as a source the
    // consumer melded by hand (same repo, no alias). That entry must not be
    // able to install into or repin a source it does not own: CUR-12 says
    // `curate` never changes a directly-melded source, and that must hold
    // even when some OTHER curator's list happens to name it.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "a direct meld records no curator: {}",
        env.sources_json()
    );

    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-ref = \"{}\" }}",
        lib.spec(),
        lib.head()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        !plan.stdout.contains("repin") && !plan.stdout.contains("install"),
        "an entry naming an identity it does not own must propose nothing: {}",
        plan.stdout
    );
    // spec: CUR-16
    assert!(
        plan.stdout.contains("adopt") && plan.stdout.contains("--adopt"),
        "the unowned identity must be reported as an adopt candidate: {}",
        plan.stdout
    );

    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    assert!(
        !env.sources_json().contains("\"kind\": \"ref\""),
        "the directly melded source's pin must not move off the default branch: {}",
        env.sources_json()
    );
}

#[test]
fn a_marketplace_entry_naming_a_directly_melded_source_proposes_no_upgrade() {
    // spec: CUR-6 CUR-12
    // The same ownership guard applies to the marketplace `market` loop as to
    // the `[discover].sources` entry loop: a catalog plugin that resolves to
    // an identity it does not itself own must not pull that source into the
    // CUR-6 upgrade sweep.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--namespace", "mine", "--yes"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "a direct meld records no curator: {}",
        env.sources_json()
    );

    let catalog = env.repo("catalog");
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"Cat","plugins":[{{"name":"mine","source":{{"url":"{}"}}}}]}}"#,
            lib.spec()
        ),
    );
    let m = env.mind(&["meld", &catalog.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    // Upstream moves, so a source curate is willing to touch would be flagged.
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\nsecond\n",
    );

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        !plan.stdout.contains("upgrade"),
        "a marketplace entry naming an identity it does not own must propose nothing: {}",
        plan.stdout
    );
}

#[test]
fn a_source_cannot_shield_itself_from_unlisting_by_listing_itself() {
    // spec: CUR-17
    // Without the self-listing exclusion, a curated source could list its own
    // identity in its own mind.toml and stay in the CUR-7 "still listed" set
    // forever, even after the curator that actually registered it drops it.
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(
        env.claude_home.join("skills/review").exists(),
        "the curated item must install: {}",
        m.stdout
    );

    // The curated source lists itself.
    lib.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            lib.spec()
        ),
    );
    // The real curator drops it.
    curator.curate_list(&[]);

    let plan = env.mind(&["curate", "--check", "--prune"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        plan.stdout.contains("unlist") && plan.stdout.contains(&env.ident("lib")),
        "self-listing must not shield a source from unlisting once its real curator drops it: {}",
        plan.stdout
    );
}

#[test]
fn an_unreadable_curator_does_not_sweep_its_sources_into_unlist() {
    // spec: CUR-15
    // A curator's clone disappearing (moved, unmounted) must not read the
    // same as "this curator lists nothing now": that would propose `unlist`
    // for every source it legitimately owns.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    std::fs::rename(&curator.path, env.base.join("curator-moved")).unwrap();

    let plan = env.mind(&["curate", "--check", "--prune"]);
    assert!(
        plan.success,
        "curate failed: {} {}",
        plan.stdout, plan.stderr
    );
    assert!(
        !plan.stdout.contains("unlist"),
        "an unreadable curator must not sweep the sources it owns into unlist: {}",
        plan.stdout
    );
    assert!(
        plan.stdout.contains("skipped"),
        "the unreadable curator should be reported, not silently dropped: {}",
        plan.stdout
    );
}

#[test]
fn one_entrys_invalid_pin_directive_does_not_abort_the_whole_plan() {
    // spec: CUR-15
    let env = Env::new();
    let ok_lib = env.repo("ok-lib");
    ok_lib.write_and_commit("skills/ok/SKILL.md", "---\ndescription: Ok\n---\n# ok\n");
    let bad_lib = env.repo("bad-lib");
    bad_lib.write_and_commit("skills/bad/SKILL.md", "---\ndescription: Bad\n---\n# bad\n");

    let curator = env.repo("curator");
    curator.curate_list(&[
        format!("{{ source = \"{}\", install = true }}", ok_lib.spec()),
        format!("{{ source = \"{}\", install = true }}", bad_lib.spec()),
    ]);
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/ok").exists());
    assert!(env.claude_home.join("skills/bad").exists());

    // bad-lib's entry gains conflicting pin directives (DSC-41's one-of rule).
    curator.curate_list(&[
        format!("{{ source = \"{}\", install = true }}", ok_lib.spec()),
        format!(
            "{{ source = \"{}\", install = true, pin-ref = \"deadbeef\", follow-branch = \"main\" }}",
            bad_lib.spec()
        ),
    ]);
    // Upstream moves, so ok-lib shows an upgrade if the run reaches that far.
    ok_lib.write_and_commit(
        "skills/ok/SKILL.md",
        "---\ndescription: Ok\n---\n# ok\nsecond\n",
    );

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.success,
        "one entry's bad pin directive must not abort the whole plan: {} {}",
        plan.stdout, plan.stderr
    );
    assert!(
        plan.stdout.contains("upgrade") && plan.stdout.contains(&env.ident("ok-lib")),
        "the other entry must still be planned: {}",
        plan.stdout
    );
}

#[test]
fn curate_adopt_stamps_provenance_then_curate_manages_the_source_normally() {
    // spec: CUR-16
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );

    let curator = env.repo("curator");
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    let identity = env.ident("lib");
    assert!(
        plan.stdout.contains("adopt") && plan.stdout.contains(&identity),
        "an unowned identity a curator lists must be reported as adoptable: {}",
        plan.stdout
    );

    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        adopted.success,
        "curate --adopt failed: {} {}",
        adopted.stdout, adopted.stderr
    );
    assert!(
        env.sources_json().contains("\"curated_by\""),
        "adopt must stamp curated_by: {}",
        env.sources_json()
    );

    let after = env.mind(&["curate", "--check"]);
    assert!(after.success, "curate failed: {}", after.stderr);
    assert!(
        after.stdout.contains("install") && !after.stdout.contains("adopt"),
        "once adopted, curate must plan install/repin/upgrade for it normally: {}",
        after.stdout
    );
}

#[test]
fn adopting_an_identity_no_curator_lists_is_refused() {
    // spec: CUR-16
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let m = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let identity = env.ident("lib");
    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        !adopted.success,
        "adopting an identity no curator lists must fail, not silently do nothing: {}",
        adopted.stdout
    );
}

#[test]
fn ansi_in_a_curators_install_items_is_stripped_from_the_plan() {
    // spec: CUR-18
    // The register detail echoes a curator's raw install-items refs before
    // anything validates them against a real catalog, which is exactly why
    // that path (unlike the install detail's already-matched item keys) can
    // carry attacker-controlled text: the entry must still be unregistered
    // when `curate` reads it, so the curator is melded before the entry (with
    // its ANSI-laden ref) exists in its list, mirroring
    // `a_newly_listed_entry_is_registered_and_installed`.
    let env = Env::new();
    let curator = env.repo("curator");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install-items = [\"skill:review\\u001b[31mHIDDEN\\u001b[0m\"] }}",
        lib.spec()
    )]);

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        !plan.stdout.contains('\u{1b}'),
        "curator-controlled text reaching the plan must have ANSI stripped: {:?}",
        plan.stdout
    );
}

#[test]
fn curate_sync_does_not_touch_a_source_that_is_neither_curator_nor_curated() {
    // spec: CUR-19
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let unrelated = env.repo("unrelated");
    unrelated.write_and_commit("skills/x/SKILL.md", "---\ndescription: X\n---\n# x\n");
    let u = env.mind(&[
        "meld",
        &unrelated.spec(),
        "--pin",
        "branch=main",
        "--register-only",
    ]);
    assert!(u.success, "meld failed: {} {}", u.stdout, u.stderr);
    let before = env.sources_json();

    // Upstream moves; a scoped sync must not notice, since `unrelated` is
    // neither a curator nor curated by one.
    unrelated.write_and_commit(
        "skills/x/SKILL.md",
        "---\ndescription: X\n---\n# x\nsecond\n",
    );

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert_eq!(
        before,
        env.sources_json(),
        "curate's refresh must not touch a source that is neither a curator nor curated"
    );
}

#[test]
fn json_clean_run_still_emits_empty_applied_and_skipped_arrays() {
    // spec: CUR-13
    // `applied` and `skipped` must always be present, as `[]` when there is
    // nothing to report, never omitted -- a caller should not have to
    // distinguish "key absent" from "key present but empty".
    let env = Env::new();
    let plain = env.repo("plain");
    plain.write_and_commit(
        "skills/solo/SKILL.md",
        "---\ndescription: solo\n---\n# solo\n",
    );
    let m = env.mind(&["meld", &plain.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let r = env.mind(&["curate", "--json"]);
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).expect("json");
    assert_eq!(doc["outcome"], "clean");
    assert_eq!(
        doc["changes"],
        serde_json::json!([]),
        "changes must be an empty array, not absent: {}",
        r.stdout
    );
    assert_eq!(
        doc["applied"],
        serde_json::json!([]),
        "applied must be an empty array, not absent: {}",
        r.stdout
    );
    assert_eq!(
        doc["skipped"],
        serde_json::json!([]),
        "skipped must be an empty array, not absent: {}",
        r.stdout
    );
}
#[test]
fn invisible_unicode_in_curator_and_source_identities_is_stripped_from_json() {
    // spec: CUR-18
    // Unlike a curator-controlled *value* embedded in `detail` (an
    // install-items ref, a pin directive), `curator` and `source` are
    // identity strings assembled from directory names. The identity
    // validator (`validate_identity_part` in source.rs) only rejects Rust's
    // narrow `char::is_control()` set, which blocks a raw ANSI escape (ESC is
    // a C0 control) but NOT the wider "blocked Unicode" set `strip_ansi` also
    // removes (a bidi override, a zero-width character). So a curator or
    // curated source whose local directory name carries one of those must
    // still have it stripped from `--json`'s `curator`/`source` fields, not
    // just from `detail`.
    let env = Env::new();
    let curator = env.repo("curator\u{202E}evil");
    curator.write_and_commit("mind.toml", "[source]\ndescription = \"curator\"\n");
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let lib = env.repo("lib\u{200B}sneaky");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff\n---\n# review\n",
    );
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);

    let plan = env.mind(&["curate", "--check", "--json"]);
    assert!(
        plan.success,
        "curate failed: {} {}",
        plan.stdout, plan.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&plan.stdout).expect("json");
    let change = &doc["changes"][0];
    let curator_field = change["curator"].as_str().expect("curator field");
    let source_field = change["source"].as_str().expect("source field");
    assert!(
        !curator_field.contains('\u{202E}'),
        "the bidi override must be stripped from the curator field: {curator_field:?}"
    );
    assert!(
        !source_field.contains('\u{200B}'),
        "the zero-width space must be stripped from the source field: {source_field:?}"
    );
    assert!(
        curator_field.contains("curator"),
        "curator field must still name the curator: {curator_field:?}"
    );
    assert!(
        source_field.contains("lib"),
        "source field must still name the entry: {source_field:?}"
    );
}
#[test]
fn a_skipped_entry_strips_invisible_unicode_from_the_identity_it_names() {
    // spec: CUR-18 CUR-13
    // `Change` is not the only curator-controlled text `curate` prints: the
    // CUR-13 `skipped` array names identities too, and an identity is
    // assembled from directory names that `validate_identity_part` screens
    // only for C0/C1 controls -- a bidi override or a zero-width character
    // passes it (the same reasoning that made `Change::new` sanitize `curator`
    // and `source`). Text mode strips it on the way out; `--json` is the
    // surface that could still hand a reader an invisible character.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let curator = env.repo("curator\u{202E}evil");
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(
        env.sources_json().contains('\u{202E}'),
        "the fixture is only meaningful if the identity really carries the \
         bidi override: {}",
        env.sources_json()
    );

    // The curator's clone disappears: CUR-15 reports it as `curator_unreadable`,
    // naming the identity.
    std::fs::rename(&curator.path, env.base.join("curator-moved")).unwrap();

    let text = env.mind(&["curate", "--check"]);
    assert!(
        text.success,
        "curate failed: {} {}",
        text.stdout, text.stderr
    );
    assert!(
        !text.stdout.contains('\u{202E}'),
        "text mode must not print the bidi override: {:?}",
        text.stdout
    );

    let json = env.mind(&["curate", "--check", "--json"]);
    assert!(
        json.success,
        "curate --json failed: {} {}",
        json.stdout, json.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(
        doc["skipped"][0]["reason"], "curator_unreadable",
        "fixture must produce the skipped entry under test: {doc}"
    );
    let named = doc["skipped"][0]["source"]
        .as_str()
        .expect("skipped source");
    assert!(
        !named.contains('\u{202E}'),
        "the bidi override must be stripped from the skipped identity too: {named:?}"
    );
    assert!(
        named.contains("curator"),
        "while still naming the curator: {named:?}"
    );
}

#[test]
fn register_detail_names_the_resolved_url_or_path() {
    // spec: CUR-1 CUR-3
    // A `register` change's `detail` must name the entry's resolved URL/path
    // (from parsing its spec), not just the curator identity and the item
    // refs, so a reader sees what is about to be cloned.
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

    let plan = env.mind(&["curate", "--check", "--json"]);
    assert!(
        plan.success,
        "curate failed: {} {}",
        plan.stdout, plan.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&plan.stdout).expect("json");
    let detail = doc["changes"][0]["detail"].as_str().expect("detail field");
    assert_eq!(doc["changes"][0]["kind"], "register");
    assert!(
        detail.contains(&lib.spec()),
        "the register detail must name the resolved path: {detail:?}"
    );

    // Also visible in the plain-text plan, not just `--json`.
    let text_plan = env.mind(&["curate", "--check"]);
    assert!(
        text_plan.stdout.contains(&lib.spec()),
        "the printed plan must also name the resolved path: {}",
        text_plan.stdout
    );
}

#[test]
fn a_prune_only_plan_still_prompts_and_applies_on_yes() {
    // spec: CUR-9 CUR-7 HOOK-109
    // The confirmation gate counted applicable changes with `prune = false`
    // while the apply used the real flag, so a plan of nothing but `unlist`
    // changes under `--prune` on a terminal short-circuited to "nothing
    // applicable": no prompt, no apply, no error, exit 0.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    // The curator drops the entry: the whole plan is now one `unlist`.
    curator.curate_list(&[]);

    let r = env.mind_env(&["curate", "--prune"], &[("MIND_TTY", "1")], Some("y\n"));
    assert!(
        r.success,
        "curate --prune failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("apply these 1 change(s)"),
        "a prune-only plan must still reach the confirmation prompt: {}",
        r.stdout
    );
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "answering yes must apply the unlist: {}",
        r.stdout
    );
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        !sources.stdout.contains(&env.ident("lib")),
        "and drop the source: {}",
        sources.stdout
    );
}

#[test]
fn a_prune_only_plan_answered_no_applies_nothing() {
    // spec: CUR-9 CUR-7 HOOK-109
    // The other side of the gate: reaching the prompt is not the same as
    // applying, so `n` must leave the source and its items in place.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    curator.curate_list(&[]);

    let r = env.mind_env(&["curate", "--prune"], &[("MIND_TTY", "1")], Some("n\n"));
    assert!(
        r.success,
        "curate --prune failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        env.claude_home.join("skills/review").exists(),
        "a declined prompt must uninstall nothing: {}",
        r.stdout
    );
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains(&env.ident("lib")),
        "nor drop the source: {}",
        sources.stdout
    );
}

#[test]
fn a_mixed_prune_plan_names_the_destructive_changes_in_the_prompt() {
    // spec: CUR-9 CUR-7 HOOK-109
    // With `prune = false` hardcoded in the count, the prompt for this plan
    // read "apply these 1 change(s) now?" while answering yes ALSO uninstalled
    // and dropped a second source. The count must match what applies, and the
    // destructive part of it must be named.
    let env = Env::new();
    let a = env.repo("lib-a");
    a.write_and_commit("skills/a/SKILL.md", "---\ndescription: A\n---\n# a\n");
    let b = env.repo("lib-b");
    b.write_and_commit("skills/b/SKILL.md", "---\ndescription: B\n---\n# b\n");
    let curator = env.repo("curator");
    curator.curate_list(&[
        format!("{{ source = \"{}\", install = true }}", a.spec()),
        format!("{{ source = \"{}\", install = true }}", b.spec()),
    ]);
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/a").exists());
    assert!(env.claude_home.join("skills/b").exists());

    // One pending `install` (a declared item forgotten) ...
    let forget = env.mind(&["forget", "skill:a", "--yes"]);
    assert!(
        forget.success,
        "forget failed: {} {}",
        forget.stdout, forget.stderr
    );
    // ... and one pending `unlist` (lib-b dropped from the list).
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", a.spec())]);

    let r = env.mind_env(&["curate", "--prune"], &[("MIND_TTY", "1")], Some("y\n"));
    assert!(
        r.success,
        "curate --prune failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("apply these 2 change(s)"),
        "the prompt must count every change this run would apply: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("1 `unlist`") && r.stdout.contains("uninstall"),
        "and say that one of them uninstalls a source: {}",
        r.stdout
    );
    assert!(
        env.claude_home.join("skills/a").exists(),
        "yes must apply the install: {}",
        r.stdout
    );
    assert!(
        !env.claude_home.join("skills/b").exists(),
        "and the unlist: {}",
        r.stdout
    );
}

#[test]
fn json_mode_applies_nothing_and_prompts_nobody_even_with_prune() {
    // spec: CUR-9 CUR-13
    // `should_apply` now reads the whole flag set, so the `--prune` count is
    // real; the json/non-TTY bail must still come FIRST. A `--json` run on a
    // terminal with an answer already waiting on stdin is the adversarial
    // shape: there is no prompt to answer, so the run must report and stop.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());
    curator.curate_list(&[]); // the whole plan is now one `unlist`

    let r = env.mind_env(
        &["curate", "--prune", "--json"],
        &[("MIND_TTY", "1")],
        Some("y\n"),
    );
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    let doc: serde_json::Value = serde_json::from_str(&r.stdout).expect("json");
    assert_eq!(doc["outcome"], "pending", "{doc}");
    assert_eq!(doc["changes"][0]["kind"], "unlist", "{doc}");
    assert_eq!(
        doc["applied"],
        serde_json::json!([]),
        "--json without --yes applies nothing, --prune or not: {doc}"
    );
    assert!(
        !r.stdout.contains("apply these"),
        "and never prompts: {}",
        r.stdout
    );
    assert!(
        env.claude_home.join("skills/review").exists(),
        "the source's items stay installed: {}",
        r.stdout
    );

    // The same plan in text mode on a non-TTY: reported, not applied, with the
    // note counting what `--prune` WOULD apply.
    let text = env.mind(&["curate", "--prune"]);
    assert!(
        text.success,
        "curate failed: {} {}",
        text.stdout, text.stderr
    );
    assert!(
        text.stdout.contains("nothing applied") && text.stdout.contains("1 change(s)"),
        "a non-TTY run says how to apply the change it counted: {}",
        text.stdout
    );
    assert!(
        env.claude_home.join("skills/review").exists(),
        "and applies nothing: {}",
        text.stdout
    );
}

#[test]
fn a_prune_run_whose_plan_has_no_unlist_prompts_without_the_destructive_wording() {
    // spec: CUR-9 CUR-7
    // The other end of the destructive-count wording: `--prune` given on a
    // plan that contains no `unlist` at all must read exactly as it does
    // without the flag. A prompt that warned about uninstalls on a plan that
    // performs none teaches the user to ignore the warning.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let forget = env.mind(&["forget", "skill:review", "--yes"]);
    assert!(
        forget.success,
        "forget failed: {} {}",
        forget.stdout, forget.stderr
    );

    let r = env.mind_env(&["curate", "--prune"], &[("MIND_TTY", "1")], Some("y\n"));
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("apply these 1 change(s) now?"),
        "a prune run with no unlist must use the plain wording: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("unlist"),
        "nothing in the run mentions unlisting: {}",
        r.stdout
    );
    assert!(
        env.claude_home.join("skills/review").exists(),
        "and answering yes still applies the install: {}",
        r.stdout
    );
}

/// A library repo melded directly (so it is registered and unowned) plus a
/// curator that lists it: the shape every `--adopt` case starts from.
fn unowned_lib_and_claiming_curator(env: &Env, curator_name: &str) -> (Repo, Repo) {
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );
    let curator = env.repo(curator_name);
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    (lib, curator)
}

#[test]
fn check_outranks_adopt_and_writes_nothing() {
    // spec: CUR-9 CUR-16
    // `--check` reports and applies nothing, and that promise cannot depend on
    // which other flags accompany it. `--adopt` used to be dispatched before
    // `check` was consulted anywhere, so `curate --check --adopt <id>` stamped
    // `curated_by` and saved the registry: a "safe to paste" run that wrote.
    let env = Env::new();
    let (_lib, _curator) = unowned_lib_and_claiming_curator(&env, "curator");
    let identity = env.ident("lib");

    let before = env.sources_json();
    assert!(
        !before.contains("curated_by"),
        "the fixture must start unowned: {before}"
    );
    let checked = env.mind(&["curate", "--check", "--adopt", &identity]);
    assert!(
        checked.success,
        "curate --check --adopt failed: {} {}",
        checked.stdout, checked.stderr
    );
    assert_eq!(
        before,
        env.sources_json(),
        "--check --adopt must leave sources.json byte-identical"
    );
    assert!(
        checked.stdout.contains("would adopt")
            && checked.stdout.contains(&identity)
            && checked.stdout.contains(&env.ident("curator")),
        "--check --adopt must report the claim it would apply: {}",
        checked.stdout
    );

    // spec: CUR-13
    // The `--json` shape of the same run: `pending`, the same outcome word the
    // plan path uses for "a change exists and nothing was applied".
    let json = env.mind(&["curate", "--check", "--adopt", &identity, "--json"]);
    assert!(
        json.success,
        "curate --check --adopt --json failed: {} {}",
        json.stdout, json.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&json.stdout).expect("json");
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["action"], "curate-adopt");
    assert_eq!(doc["outcome"], "pending");
    assert_eq!(doc["source"], identity.as_str());
    assert_eq!(doc["curator"], env.ident("curator").as_str());
    assert_eq!(
        before,
        env.sources_json(),
        "--check --adopt --json must not write either"
    );

    // Without --check the same command writes, so the assertions above are
    // testing the flag and not a fixture that could never adopt anyway.
    let applied = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        applied.success,
        "curate --adopt failed: {} {}",
        applied.stdout, applied.stderr
    );
    assert!(
        env.sources_json().contains("curated_by"),
        "the same identity must be adoptable without --check: {}",
        env.sources_json()
    );
}

#[test]
fn check_adopt_of_an_identity_no_curator_lists_still_fails() {
    // spec: CUR-9 CUR-16
    // `--check` suppresses the write, never the validation: a stale or
    // mistyped identity must fail as loudly under `--check` as without it,
    // otherwise a `--check` run reports a claim a real run would refuse.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let m = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let checked = env.mind(&["curate", "--check", "--adopt", &env.ident("lib")]);
    assert!(
        !checked.success,
        "an unclaimed identity must fail under --check too: {} {}",
        checked.stdout, checked.stderr
    );
}

#[test]
fn two_curators_claiming_one_identity_refuse_adopt_and_both_appear_in_the_plan() {
    // spec: CUR-20
    // Ownership used to go to the first curator in registry order that listed
    // the identity, silently, with the losing claim never mentioned. Both
    // claims are the user's to see: the plan reports one `adopt` line per
    // curator, and `--adopt` refuses rather than choosing for them.
    let env = Env::new();
    let (lib, _curator_a) = unowned_lib_and_claiming_curator(&env, "curator-a");
    let curator_b = env.repo("curator-b");
    curator_b.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);
    let m = env.mind(&["meld", &curator_b.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        plan.stdout.contains("adopt")
            && plan.stdout.contains(&env.ident("curator-a"))
            && plan.stdout.contains(&env.ident("curator-b")),
        "both claims must be reported, one line each: {}",
        plan.stdout
    );

    let identity = env.ident("lib");
    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        !adopted.success,
        "an ambiguous claim must not silently pick a curator: {} {}",
        adopted.stdout, adopted.stderr
    );
    let said = format!("{}{}", adopted.stdout, adopted.stderr);
    assert!(
        said.contains(&env.ident("curator-a")) && said.contains(&env.ident("curator-b")),
        "the refusal must name both claimants: {said}"
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "a refused adopt must write nothing: {}",
        env.sources_json()
    );

    // spec: CUR-9
    // `--check` suppresses the write, never the validation: an ambiguous claim
    // must fail under it too, or a `--check` run would report a resolution the
    // real run refuses.
    let checked = env.mind(&["curate", "--check", "--adopt", &identity]);
    assert!(
        !checked.success,
        "--check --adopt must refuse an ambiguous claim as well: {} {}",
        checked.stdout, checked.stderr
    );
    assert!(
        !checked.stdout.contains("would adopt"),
        "and must not report a resolution it does not have: {}",
        checked.stdout
    );

    // spec: CLI-181 -- and the same refusal is one JSON error envelope.
    let refused = adopt_json_refusal(&env, &identity);
    assert!(
        refused.contains(&env.ident("curator-a")) && refused.contains(&env.ident("curator-b")),
        "the json refusal must name both claimants too: {refused}"
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "and still write nothing: {}",
        env.sources_json()
    );
}

#[test]
fn a_claim_resolving_to_a_different_path_is_refused_naming_both() {
    // spec: CUR-20
    // A local source's identity is `local/<parent>/<dir>`, only the last two
    // path segments, so a curator can list a directory it controls that
    // derives the SAME identity as a source the consumer melded from
    // somewhere else. Matching on identity alone let that claim take
    // ownership; the claim's resolved path must match the registered one.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );

    // A decoy repo whose parent directory is named like the env base, so it
    // derives the same `local/<base>/lib` identity from a different path.
    let base_name = env.base.file_name().unwrap().to_string_lossy().into_owned();
    let decoy = env.repo(&format!("decoy/{base_name}/lib"));
    decoy.write_and_commit(
        "skills/evil/SKILL.md",
        "---\ndescription: Evil\n---\n# evil\n",
    );
    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true }}",
        decoy.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let identity = env.ident("lib");
    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        !adopted.success,
        "a claim pointing at a different path must not adopt the registered source: {} {}",
        adopted.stdout, adopted.stderr
    );
    let said = format!("{}{}", adopted.stdout, adopted.stderr);
    assert!(
        said.contains(&decoy.spec()) && said.contains(&lib.spec()),
        "the refusal must name the claimed path and the registered one: {said}"
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "a refused adopt must write nothing: {}",
        env.sources_json()
    );

    // spec: CUR-9
    // The CUR-20 upstream check runs under `--check` too: the flag decides
    // whether the claim is WRITTEN, never whether it is validated.
    let checked = env.mind(&["curate", "--check", "--adopt", &identity]);
    assert!(
        !checked.success,
        "--check --adopt must refuse a mismatched claim as well: {} {}",
        checked.stdout, checked.stderr
    );
    assert!(
        !checked.stdout.contains("would adopt"),
        "and must not report a resolution it does not have: {}",
        checked.stdout
    );

    // spec: CLI-181 -- and the same refusal is one JSON error envelope.
    let refused = adopt_json_refusal(&env, &identity);
    assert!(
        refused.contains(&decoy.spec()) && refused.contains(&lib.spec()),
        "the json refusal must name both paths too: {refused}"
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "and still write nothing: {}",
        env.sources_json()
    );
}

#[test]
fn a_marketplace_only_claim_is_reported_in_the_plan_and_can_be_adopted() {
    // spec: CUR-16 CUR-20
    // `--adopt` resolves ownership from a marketplace catalog's membership
    // (MKT-7) exactly as it does from a `[discover].sources` entry, but only
    // the entry loop emitted an `adopt` line: a catalog could take ownership
    // of a source the reviewed plan never mentioned.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    // Melded under the same namespace the catalog entry's name supplies
    // (MKT-8), so the catalog resolves to this already-registered identity.
    let direct = env.mind(&["meld", &lib.spec(), "--namespace", "kit", "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );
    assert!(
        !env.sources_json().contains("curated_by"),
        "a direct meld records no curator: {}",
        env.sources_json()
    );

    let catalog = env.repo("catalog");
    catalog.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"Cat","plugins":[{{"name":"kit","source":{{"url":"{}"}}}}]}}"#,
            lib.spec()
        ),
    );
    let m = env.mind(&["meld", &catalog.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let identity = format!("{}@kit", env.ident("lib"));
    let plan = env.mind(&["curate", "--check"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    assert!(
        plan.stdout.contains("adopt") && plan.stdout.contains(&identity),
        "a marketplace claim on an unowned source must appear in the plan: {}",
        plan.stdout
    );

    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        adopted.success,
        "curate --adopt failed: {} {}",
        adopted.stdout, adopted.stderr
    );
    assert!(
        env.sources_json().contains("curated_by"),
        "the marketplace claim must be adoptable: {}",
        env.sources_json()
    );
}

/// The `curated_by` recorded for `identity`, read straight from `sources.json`
/// rather than by substring: several fixtures below have more than one source
/// registered, so "the file mentions curated_by somewhere" proves nothing about
/// which source was stamped.
fn curated_by(env: &Env, identity: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(&env.sources_json()).expect("sources.json");
    let source = doc["sources"]
        .as_array()
        .expect("sources array")
        .iter()
        .find(|s| s["name"] == identity)?;
    source["curated_by"].as_str().map(str::to_string)
}

/// Run `curate --adopt <identity> --json`, require it to refuse, and return the
/// CLI-181 error envelope's `message`.
fn adopt_json_refusal(env: &Env, identity: &str) -> String {
    let r = env.mind(&["curate", "--adopt", identity, "--json"]);
    assert!(
        !r.success,
        "adopt of {identity} must be refused: {} {}",
        r.stdout, r.stderr
    );
    let doc: serde_json::Value =
        serde_json::from_str(&r.stdout).expect("one JSON error envelope on stdout");
    assert_eq!(doc["schema"], 1, "{doc}");
    assert_eq!(
        doc["error"]["kind"], "not-an-adopt-candidate",
        "the refusal must carry the structured kind, not just prose: {doc}"
    );
    doc["error"]["message"]
        .as_str()
        .expect("error message")
        .to_string()
}

#[test]
fn a_curator_claiming_through_both_an_entry_and_its_catalog_reports_one_adopt_line() {
    // spec: CUR-20
    // Both mechanisms are claims, and `entry_claims` yields every one of them.
    // A curator that lists a repo in `[discover].sources` AND ships a
    // marketplace catalog naming it makes two claims on one identity: the plan
    // must report that once (the pair, not the identity, is what is deduped),
    // and `--adopt` must read it as one curator, not as the CUR-20 ambiguity.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    // Melded under the namespace the catalog entry's name supplies (MKT-8), so
    // both claims resolve to this one already-registered identity.
    let direct = env.mind(&["meld", &lib.spec(), "--namespace", "kit", "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );

    let curator = env.repo("curator");
    curator.write_and_commit(
        ".claude-plugin/marketplace.json",
        &format!(
            r#"{{"name":"Cat","plugins":[{{"name":"kit","source":{{"url":"{}"}}}}]}}"#,
            lib.spec()
        ),
    );
    // A bare `[discover].sources` list is not an authoritative inventory
    // (MKT-2/MKT-16), so the catalog is still read alongside it.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", namespace = \"kit\" }}",
        lib.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let identity = format!("{}@kit", env.ident("lib"));
    let plan = env.mind(&["curate", "--check", "--json"]);
    assert!(
        plan.success,
        "curate failed: {} {}",
        plan.stdout, plan.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&plan.stdout).expect("json");
    let adopts: Vec<&serde_json::Value> = doc["changes"]
        .as_array()
        .expect("changes array")
        .iter()
        .filter(|c| c["kind"] == "adopt")
        .collect();
    assert_eq!(
        adopts.len(),
        1,
        "one curator claiming one identity through both mechanisms reports once: {doc}"
    );
    assert_eq!(adopts[0]["source"], identity.as_str(), "{doc}");
    assert_eq!(adopts[0]["curator"], env.ident("curator").as_str(), "{doc}");

    let adopted = env.mind(&["curate", "--adopt", &identity]);
    assert!(
        adopted.success,
        "two claims from ONE curator are not an ambiguity: {} {}",
        adopted.stdout, adopted.stderr
    );
    assert_eq!(
        curated_by(&env, &identity),
        Some(env.ident("curator")),
        "and the source ends owned by that curator: {}",
        env.sources_json()
    );
}

#[test]
fn one_curator_listing_two_paths_for_one_identity_is_a_mismatch_not_an_ambiguity() {
    // spec: CUR-20
    // Two entries in ONE curator resolving to the same identity by DIFFERENT
    // paths: the claimant set has one member, so the ambiguity rule does not
    // fire and the upstream check is what must catch it. A curator whose list
    // contains a decoy path alongside the real one must not be able to launder
    // the decoy into ownership just by also naming the real repo.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );

    let base_name = env.base.file_name().unwrap().to_string_lossy().into_owned();
    let decoy = env.repo(&format!("decoy/{base_name}/lib"));
    decoy.write_and_commit(
        "skills/evil/SKILL.md",
        "---\ndescription: Evil\n---\n# evil\n",
    );
    let curator = env.repo("curator");
    curator.curate_list(&[
        format!("{{ source = \"{}\" }}", lib.spec()),
        format!("{{ source = \"{}\" }}", decoy.spec()),
    ]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    let identity = env.ident("lib");
    let plan = env.mind(&["curate", "--check", "--json"]);
    assert!(plan.success, "curate failed: {}", plan.stderr);
    let doc: serde_json::Value = serde_json::from_str(&plan.stdout).expect("json");
    let adopts = doc["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter(|c| c["kind"] == "adopt")
        .count();
    assert_eq!(
        adopts, 1,
        "the (curator, identity) pair is deduped however many entries make it: {doc}"
    );

    let refused = adopt_json_refusal(&env, &identity);
    assert!(
        refused.contains(&decoy.spec()) && refused.contains(&lib.spec()),
        "the refusal must name the claim that does not match and the registered \
         upstream: {refused}"
    );
    assert!(
        !refused.contains("curators claim it"),
        "one curator listing an identity twice is not the multi-curator \
         ambiguity, and must not be reported as one: {refused}"
    );
    assert_eq!(
        curated_by(&env, &identity),
        None,
        "a refused adopt writes nothing: {}",
        env.sources_json()
    );
}

#[test]
fn every_adopt_refusal_is_one_json_error_envelope_that_writes_nothing() {
    // spec: CUR-16 CLI-181 CLI-217
    // The refusal paths were only ever exercised in text mode. Under `--json`
    // each must be exactly one error envelope on stdout with the structured
    // `kind` a caller branches on, and must leave `sources.json` untouched.
    let env = Env::new();
    let (_lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let plain = env.repo("plain");
    plain.write_and_commit(
        "skills/solo/SKILL.md",
        "---\ndescription: Solo\n---\n# solo\n",
    );
    let p = env.mind(&["meld", &plain.spec(), "--register-only"]);
    assert!(p.success, "meld failed: {} {}", p.stdout, p.stderr);
    let before = env.sources_json();
    assert_eq!(
        curated_by(&env, &env.ident("lib")),
        Some(env.ident("curator")),
        "the fixture must start with one owned and one unowned source: {before}"
    );

    // No source carries that identity at all.
    let missing = adopt_json_refusal(&env, "example.com/no/such");
    assert!(
        missing.contains("no melded source") && missing.contains("example.com/no/such"),
        "a mistyped identity must say it is not registered: {missing}"
    );
    // Registered, but a curator already owns it.
    let owned = adopt_json_refusal(&env, &env.ident("lib"));
    assert!(
        owned.contains("already curated by") && owned.contains(&env.ident("curator")),
        "an owned source must name its owner: {owned}"
    );
    // Registered and unowned, but no curator lists it.
    let unclaimed = adopt_json_refusal(&env, &env.ident("plain"));
    assert!(
        unclaimed.contains("no registered curator"),
        "an unclaimed source must say so: {unclaimed}"
    );
    assert_eq!(
        before,
        env.sources_json(),
        "no refusal path may write to the registry"
    );
}

#[test]
fn a_curator_that_lists_only_itself_offers_nothing_to_adopt() {
    // spec: CUR-17 CUR-16
    // CUR-17 excludes a self-listing entry from the plan; the same exclusion
    // has to hold in `--adopt`'s own claim resolution, or a source could list
    // itself, be adopted as its own curator, and become permanently immune to
    // CUR-7 unlisting -- exactly the self-shield CUR-17 exists to close.
    let env = Env::new();
    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true }}",
        curator.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--register-only"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    let identity = env.ident("curator");

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.success,
        "curate failed: {} {}",
        plan.stdout, plan.stderr
    );
    assert!(
        !plan.stdout.contains("adopt"),
        "a self-listed entry must not be reported as an adopt candidate: {}",
        plan.stdout
    );

    let refused = adopt_json_refusal(&env, &identity);
    assert!(
        refused.contains("no registered curator"),
        "and --adopt must refuse it as unlisted, not adopt it to itself: {refused}"
    );
    assert_eq!(
        curated_by(&env, &identity),
        None,
        "nothing may be written: {}",
        env.sources_json()
    );
}

#[test]
fn adopt_applies_no_other_change_even_with_prune_and_yes() {
    // spec: CUR-16
    // `--adopt` is a narrow sub-mode that returns before a plan is built, so
    // the destructive flags that would otherwise ride along on the same
    // command line must do nothing: an unattended `curate --adopt X --prune
    // --yes` must not also uninstall a source the plan lists for CUR-7
    // unlisting, nor install the adopted source's items.
    let env = Env::new();
    let dropped = env.repo("dropped");
    dropped.write_and_commit(
        "skills/dropped/SKILL.md",
        "---\ndescription: Dropped\n---\n# dropped\n",
    );
    let curator = env.repo("curator");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true }}",
        dropped.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/dropped").exists());

    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    let direct = env.mind(&["meld", &lib.spec(), "--register-only"]);
    assert!(
        direct.success,
        "meld failed: {} {}",
        direct.stdout, direct.stderr
    );
    // The curator now drops `dropped` (a pending unlist) and claims `lib`.
    curator.curate_list(&[format!("{{ source = \"{}\", install = true }}", lib.spec())]);

    let r = env.mind(&["curate", "--adopt", &env.ident("lib"), "--prune", "--yes"]);
    assert!(
        r.success,
        "curate --adopt failed: {} {}",
        r.stdout, r.stderr
    );
    assert_eq!(
        curated_by(&env, &env.ident("lib")),
        Some(env.ident("curator")),
        "the claim is applied: {}",
        env.sources_json()
    );
    assert!(
        env.claude_home.join("skills/dropped").exists(),
        "--prune alongside --adopt must apply no unlist: {}",
        r.stdout
    );
    let sources = env.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains(&env.ident("dropped")),
        "nor drop the source: {}",
        sources.stdout
    );
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "--yes alongside --adopt must install nothing either: {}",
        r.stdout
    );
}

/// Rewrite `identity`'s recorded pin in `sources.json` to `Pin::Ref(value)`.
///
/// Some registry states cannot be produced by any command this binary offers
/// (a pin `git clone` never fetches, a value a newer rule now refuses), yet a
/// registry written by an older binary can hold them. Writing the state
/// directly is the only hermetic way to plan against it.
fn set_recorded_pin(env: &Env, identity: &str, value: &str) {
    let path = env.mind_home.join("sources.json");
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("sources.json"))
            .expect("sources.json parses");
    let source = doc["sources"]
        .as_array_mut()
        .expect("sources array")
        .iter_mut()
        .find(|s| s["name"] == identity)
        .expect("the fixture must have registered that identity");
    source["pin"] = serde_json::json!({ "kind": "ref", "value": value });
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).expect("write sources.json");
}

#[test]
fn a_curator_pin_outside_refs_heads_or_tags_is_skipped_not_applied() {
    // spec: CUR-21 CUR-15
    // `refs/pull/<n>/head` is a well-formed ref that `validate_ref_value`
    // accepts, points at content anyone can push to a repo the consumer
    // otherwise trusts, and `curate` re-applies a curator's pin on every run.
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    // Control: an ordinary branch directive DOES propose a repin, so the
    // refusal below is the rule at work rather than a fixture that would have
    // proposed nothing either way.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, follow-branch = \"main\" }}",
        lib.spec()
    )]);
    let ok = env.mind(&["curate", "--check"]);
    assert!(ok.success, "curate failed: {} {}", ok.stdout, ok.stderr);
    assert!(
        ok.stdout.contains("repin"),
        "a branch directive must propose a repin: {}",
        ok.stdout
    );

    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, follow-branch = \"refs/pull/9999/head\" }}",
        lib.spec()
    )]);
    let r = env.mind(&["curate", "--check"]);
    assert!(
        r.success,
        "one refused pin must not fail the run: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        !r.stdout.contains("repin"),
        "a refs/pull pin must propose no repin: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skipped") && r.stdout.contains(&env.ident("lib")),
        "the refused directive must be reported as skipped: {}",
        r.stdout
    );
    assert!(
        r.stderr.contains("warning:") && r.stderr.contains("refs/pull/9999/head"),
        "and warned about by name: {}",
        r.stderr
    );

    let doc: serde_json::Value =
        serde_json::from_str(&env.mind(&["curate", "--json", "--check"]).stdout).expect("json");
    assert_eq!(
        doc["skipped"][0]["reason"], "pin_ref_not_allowed",
        "the CUR-13 skipped slug must name the cause: {doc}"
    );
    assert!(
        doc["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "repin"),
        "and no repin change is emitted: {doc}"
    );
}

#[test]
fn a_curator_pin_outside_refs_heads_or_tags_blocks_registering_a_new_entry() {
    // spec: CUR-21 CUR-15
    // The repin-path test above covers an ALREADY-registered source. This
    // covers the other branch of `build_plan`: an entry no one has registered
    // yet. Left unchecked there, `curator_pin_allowed` would only ever run on
    // a repin, so a curator's `refs/pull/<n>/head` on a brand-new entry would
    // reach `meld_curated_entry` unexamined -- happening to fail only because
    // `git clone --branch` cannot resolve it, not because policy refused it.
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
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, follow-branch = \"refs/pull/9999/head\" }}",
        lib.spec()
    )]);

    let plan = env.mind(&["curate", "--check"]);
    assert!(
        plan.success,
        "one refused pin must not fail the run: {} {}",
        plan.stdout, plan.stderr
    );
    assert!(
        !plan.stdout.contains("register"),
        "a refs/pull pin must propose no register: {}",
        plan.stdout
    );
    assert!(
        plan.stdout.contains("skipped") && plan.stdout.contains(&env.ident("lib")),
        "the refused entry must be reported as skipped: {}",
        plan.stdout
    );
    assert!(
        plan.stderr.contains("warning:") && plan.stderr.contains("refs/pull/9999/head"),
        "and warned about by name: {}",
        plan.stderr
    );

    let doc: serde_json::Value =
        serde_json::from_str(&env.mind(&["curate", "--json", "--check"]).stdout).expect("json");
    assert_eq!(
        doc["skipped"][0]["reason"], "pin_ref_not_allowed",
        "the CUR-13 skipped slug must name the cause: {doc}"
    );
    assert!(
        doc["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["kind"] != "register"),
        "and no register change is emitted: {doc}"
    );

    // Even with `--yes`, the entry must never be cloned/installed: a refused
    // register-path pin is not a "register with an unchecked pin", it is no
    // change at all for that entry.
    let applied = env.mind(&["curate", "--yes"]);
    assert!(applied.success, "curate --yes failed: {}", applied.stderr);
    assert!(
        !env.sources_json().contains(&env.ident("lib")),
        "the source must never be registered: {}",
        env.sources_json()
    );
    assert!(
        !env.claude_home.join("skills/review").exists(),
        "and its items must never be installed: {}",
        applied.stdout
    );
}

#[test]
fn a_curator_pin_tag_or_pin_ref_outside_refs_heads_or_tags_is_skipped_too() {
    // spec: CUR-21 CUR-15
    // CUR-21 is a rule about the ref VALUE, so it must hold whichever
    // directive carries it. The stage's own integration test covered
    // `follow-branch` only; a curator that could smuggle
    // `refs/pull/<n>/head` in through `pin-ref` would have exactly the
    // standing substitution channel the rule exists to close.
    let env = Env::new();
    let (lib, curator) = lib_and_curator(&env, ", install = true");
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);

    // Control: `pin-tag` DOES reach the repin path with an ordinary value, so
    // the refusals below are the rule at work and not an inert directive.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-tag = \"v1.0\" }}",
        lib.spec()
    )]);
    let ok = env.mind(&["curate", "--check"]);
    assert!(ok.success, "curate failed: {} {}", ok.stdout, ok.stderr);
    assert!(
        ok.stdout.contains("repin"),
        "a plain tag directive must propose a repin: {}",
        ok.stdout
    );

    for (directive, value) in [
        ("pin-ref", "refs/pull/9999/head"),
        ("pin-tag", "refs/remotes/origin/main"),
    ] {
        curator.curate_list(&[format!(
            "{{ source = \"{}\", install = true, {directive} = \"{value}\" }}",
            lib.spec()
        )]);
        let r = env.mind(&["curate", "--check", "--json"]);
        assert!(
            r.success,
            "one refused pin must not fail the run ({directive}): {} {}",
            r.stdout, r.stderr
        );
        let doc: serde_json::Value = serde_json::from_str(&r.stdout).expect("json");
        assert!(
            doc["changes"]
                .as_array()
                .expect("changes")
                .iter()
                .all(|c| c["kind"] != "repin"),
            "{directive} = {value} must propose no repin: {doc}"
        );
        assert_eq!(
            doc["skipped"][0]["reason"], "pin_ref_not_allowed",
            "and be reported as skipped ({directive}): {doc}"
        );
    }
}

#[test]
fn a_refused_curator_pin_that_already_matches_the_recorded_pin_proposes_nothing() {
    // spec: CUR-21 CUR-5
    // CUR-21 is reached only through a `repin` CHANGE, and a directive equal
    // to the recorded pin is not a change: `curate` proposes nothing and
    // reports nothing, rather than warning on every run about a pin it is not
    // about to apply. The rule is about what `curate` would newly point a
    // clone at, not an audit of what the clone is already on.
    let env = Env::new();
    let lib = env.repo("lib");
    lib.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review\n---\n# review\n",
    );
    git(&lib.path, &["tag", "v1"]);
    let curator = env.repo("curator");
    // Pinned, so the source is CLONED rather than linked (`is_linked` is
    // "local AND unpinned"): the rewritten pin below must leave a readable
    // clone behind, or the run would report an unreadable source instead of
    // planning against it.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-tag = \"v1\" }}",
        lib.spec()
    )]);
    let m = env.mind(&["meld", &curator.spec(), "--yes"]);
    assert!(m.success, "meld failed: {} {}", m.stdout, m.stderr);
    assert!(env.claude_home.join("skills/review").exists());

    // The recorded pin is already the ref the curator declares. A clone never
    // fetches `refs/pull/*`, so this state is only reachable from a registry
    // written earlier (a consumer's own pin, or a binary older than CUR-21);
    // it is written directly here rather than through a meld that cannot
    // produce it.
    set_recorded_pin(&env, &env.ident("lib"), "refs/pull/1/head");
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-ref = \"refs/pull/1/head\" }}",
        lib.spec()
    )]);

    let r = env.mind(&["curate", "--check", "--no-sync"]);
    assert!(r.success, "curate failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("repin"),
        "an unchanged pin is no change: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skipped") && !r.stderr.contains("warning:"),
        "and nothing is reported about it: {} {}",
        r.stdout,
        r.stderr
    );

    // Control: the moment the curator declares a DIFFERENT out-of-namespace
    // ref, that IS a change, and CUR-21 refuses it.
    curator.curate_list(&[format!(
        "{{ source = \"{}\", install = true, pin-ref = \"refs/pull/2/head\" }}",
        lib.spec()
    )]);
    let after = env.mind(&["curate", "--check", "--no-sync", "--json"]);
    assert!(
        after.success,
        "curate failed: {} {}",
        after.stdout, after.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&after.stdout).expect("json");
    assert_eq!(
        doc["skipped"][0]["reason"], "pin_ref_not_allowed",
        "a changed out-of-namespace pin is refused and reported: {doc}"
    );
}
