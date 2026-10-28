//! Integration tests for install profiles at `meld` and `learn`
//! (spec/profiles.md, PROF-1..20, PROF-26, and PROF-31..35).
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. No network. The fixture mirrors tests/cli_install_items.rs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The profile table most tests use: `default` includes `basic`; `full`
/// includes `basic` plus every agent and `skill:deploy` (which depends on
/// `skill:helper`, a closure item in no profile). `skill:test` is in no
/// profile.
const PROFILES: &str = "[profiles]\n\
default = [\"basic\"]\n\
basic = [\"skill:review\", \"rule:style\"]\n\
full = [\"basic\", \"agent:*\", \"skill:deploy\"]\n";

struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    mind_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

impl Sandbox {
    /// A source repo named `name` with the standard items and `mind_toml` as
    /// its root `mind.toml` (none when empty).
    fn with_toml(name: &str, mind_toml: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-profiles-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        for skill in ["review", "test", "helper"] {
            write_file(
                &source.join(format!("skills/{skill}/SKILL.md")),
                &format!("---\nname: {skill}\ndescription: {skill} skill\n---\n# {skill}\n"),
            );
        }
        write_file(
            &source.join("skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: deploy skill\n---\n# deploy\nUses {{ns:helper}}.\n",
        );
        write_file(
            &source.join("agents/planner.md"),
            "---\nname: planner\ndescription: Planner agent\n---\n# planner\n",
        );
        write_file(
            &source.join("rules/style.md"),
            "---\ndescription: Style rule\n---\n# style\n",
        );
        if !mind_toml.is_empty() {
            write_file(&source.join("mind.toml"), mind_toml);
        }
        git_init(&source);
        sb
    }

    /// A source with the standard [`PROFILES`] table.
    fn new(name: &str) -> Sandbox {
        Sandbox::with_toml(name, PROFILES)
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The registered identity of this repo melded as a local source.
    fn identity(&self) -> String {
        format!(
            "local/{}/{}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, &[], None)
    }

    /// `mind` with extra environment (e.g. `MIND_TTY`) and optional stdin. With
    /// `stdin: None` the child gets `/dev/null` (EOF reads as "no").
    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        use std::io::Write;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_MAX_METADATA_SIZE")
            .env_remove("MIND_TTY")
            .env_remove("COLUMNS")
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

    /// The registry entry for `identity`, or `None` when not melded.
    fn source_entry(&self, identity: &str) -> Option<Value> {
        let raw = std::fs::read_to_string(self.mind_home.join("sources.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).expect("sources.json is JSON");
        let list = v
            .get("sources")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| v.as_array().cloned())
            .unwrap_or_default();
        list.into_iter()
            .find(|s| s["name"].as_str() == Some(identity))
    }

    /// The recorded enabled profiles of this sandbox's own source.
    fn recorded_profiles(&self) -> Option<Vec<String>> {
        strings(&self.source_entry(&self.identity())?["profiles"])
    }

    /// The recorded snapshot of this sandbox's own source.
    fn recorded_snapshot(&self) -> Option<Vec<String>> {
        strings(&self.source_entry(&self.identity())?["profile_snapshot"])
    }

    /// The manifest entry keyed `key` (e.g. `skill:review`).
    fn manifest_item(&self, key: &str) -> Option<Value> {
        let raw = std::fs::read_to_string(self.mind_home.join("manifest.json")).ok()?;
        let v: Value = serde_json::from_str(&raw).expect("manifest.json is JSON");
        v["items"].get(key).cloned()
    }

    /// The sorted manifest keys.
    fn installed(&self) -> Vec<String> {
        let Ok(raw) = std::fs::read_to_string(self.mind_home.join("manifest.json")) else {
            return Vec::new();
        };
        let v: Value = serde_json::from_str(&raw).expect("manifest.json is JSON");
        let mut keys: Vec<String> = v["items"]
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        keys
    }

    /// The profile reasons recorded on manifest entry `key`.
    fn reasons(&self, key: &str) -> Vec<String> {
        self.manifest_item(key)
            .and_then(|i| strings(&i["profiles"]))
            .unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| {
        a.iter()
            .map(|s| s.as_str().unwrap_or_default().to_string())
            .collect()
    })
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git_init(dir: &Path) {
    for args in [
        vec!["-c", "init.defaultBranch=main", "init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "initial"],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} in {dir:?}");
    }
}

/// The text between the first pair of backticks: the command mind printed.
fn backticked(text: &str) -> String {
    let start = text
        .find('`')
        .unwrap_or_else(|| panic!("expected a backticked command in: {text}"));
    let rest = &text[start + 1..];
    let end = rest.find('`').expect("a closing backtick");
    rest[..end].to_string()
}

/// Split a single-quoted shell command line into argv.
fn shell_split(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quoted = false;
    for c in s.chars() {
        if quoted {
            if c == '\'' {
                quoted = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if c.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut cur));
                in_token = false;
            }
            continue;
        }
        in_token = true;
        if c == '\'' {
            quoted = true;
        } else {
            cur.push(c);
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
}

/// Run a printed `mind ...` command (possibly `&&`-joined) verbatim.
fn run_printed(sb: &Sandbox, command: &str, extra: &[&str]) -> Run {
    let mut last = None;
    for step in command.split("&&") {
        let argv = shell_split(step.trim());
        assert_eq!(argv.first().map(String::as_str), Some("mind"), "{command}");
        let mut args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        args.extend_from_slice(extra);
        let run = sb.mind(&args);
        if !run.success {
            return run;
        }
        last = Some(run);
    }
    last.expect("at least one step")
}

fn json_doc(run: &Run) -> Value {
    serde_json::from_str(run.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is one JSON object ({e}): {}", run.stdout))
}

// ----- Regression: sources without a `default` keep install-all -----

#[test]
fn source_without_profiles_keeps_install_all() {
    // spec: PROF-7 PROF-19 -- no [profiles]: the CLI-23 install-all offer is
    // unchanged and the source is not profile-tracked.
    let sb = Sandbox::with_toml("plain", "");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.installed(),
        svec(&[
            "agent:planner",
            "rule:style",
            "skill:deploy",
            "skill:helper",
            "skill:review",
            "skill:test"
        ])
    );
    assert_eq!(sb.recorded_profiles(), None);
    assert_eq!(sb.recorded_snapshot(), None);
    assert!(sb.reasons("skill:review").is_empty());
}

#[test]
fn profiles_without_default_keep_install_all_and_stay_convention() {
    // spec: PROF-1 PROF-7 -- [profiles] is metadata (convention discovery
    // still offers every item), and with no `default` and no flag the
    // install-all offer runs and nothing is recorded.
    let sb = Sandbox::with_toml("nodefault", "[profiles]\nbasic = [\"skill:review\"]\n");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed().len(), 6, "{:?}", sb.installed());
    assert_eq!(sb.recorded_profiles(), None);
    assert!(sb.reasons("skill:review").is_empty());
}

// ----- Selection at meld -----

#[test]
fn plain_meld_offers_the_default_expansion_and_records_it() {
    // spec: PROF-12 PROF-19 PROF-20 -- a source declaring `default` installs
    // only its expansion; the preview names the enabled and the other
    // profiles; the enabled set and the snapshot are recorded; each entry
    // records the profile it was installed for.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert!(r.stdout.contains("enabled default"), "{}", r.stdout);
    assert!(
        r.stdout.contains("not enabled: basic, full"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("--profile"), "{}", r.stdout);
    assert!(r.stdout.contains("--all-profiles"), "{}", r.stdout);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"]))
    );
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
    let item = sb.manifest_item("skill:review").unwrap();
    assert!(item.get("explicit").is_none(), "{item}");
}

#[test]
fn profile_flag_adds_to_default_and_tags_closure_items() {
    // spec: PROF-9 PROF-6 -- `--profile full` enables full on top of default
    // (comma lists accepted); the closure item skill:helper installs through
    // the learn path and carries the profile of the member that pulled it in.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--profile", "full,basic", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.installed(),
        svec(&[
            "agent:planner",
            "rule:style",
            "skill:deploy",
            "skill:helper",
            "skill:review"
        ])
    );
    assert_eq!(
        sb.recorded_profiles(),
        Some(svec(&["basic", "default", "full"]))
    );
    assert_eq!(sb.reasons("skill:helper"), svec(&["full"]));
    assert_eq!(
        sb.reasons("skill:review"),
        svec(&["basic", "default", "full"])
    );
    let snap = sb.recorded_snapshot().unwrap();
    assert!(
        !snap.contains(&"skill:helper".to_string()),
        "a closure item is not a member: {snap:?}"
    );
    assert!(!sb.installed().contains(&"skill:test".to_string()));
}

#[test]
fn no_default_profile_alone_installs_nothing_and_names_the_remedy() {
    // spec: PROF-10 -- nothing is offered; the source is registered with no
    // profile enabled and the note names `mind learn <source> --profile`.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--no-default-profile", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.installed().is_empty(), "{:?}", sb.installed());
    assert!(
        sb.source_entry(&sb.identity()).is_some(),
        "still registered"
    );
    assert_eq!(sb.recorded_profiles(), None);
    assert!(r.stdout.contains("mind learn"), "{}", r.stdout);
    assert!(r.stdout.contains("--profile <name>"), "{}", r.stdout);
}

#[test]
fn no_default_profile_with_profile_enables_only_the_named() {
    // spec: PROF-10 PROF-9
    let sb = Sandbox::new("src");
    let r = sb.mind(&[
        "meld",
        &sb.spec(),
        "--no-default-profile",
        "--profile",
        "full",
        "--yes",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["full"])));
    assert_eq!(sb.reasons("skill:review"), svec(&["full"]));
}

#[test]
fn all_profiles_enables_every_declared_profile_but_not_unlisted_items() {
    // spec: PROF-11
    let sb = Sandbox::new("src");
    let r = sb.mind(&[
        "meld",
        &sb.spec(),
        "--all-profiles",
        "--profile",
        "basic",
        "--yes",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.recorded_profiles(),
        Some(svec(&["basic", "default", "full"]))
    );
    assert!(
        !sb.installed().contains(&"skill:test".to_string()),
        "an item in no profile is not installed: {:?}",
        sb.installed()
    );
}

#[test]
fn malformed_profile_name_is_refused_before_the_clone() {
    // spec: PROF-14 -- InvalidProfileName, nothing registered.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--profile", "Bad", "--yes"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("invalid profile name 'Bad'"),
        "{}",
        r.stderr
    );
    assert!(sb.source_entry(&sb.identity()).is_none());
}

#[test]
fn undeclared_profile_is_profile_not_found_and_source_stays_melded() {
    // spec: PROF-14 -- ProfileNotFound after the clone lists the declared
    // profiles; the source stays melded and nothing is installed.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--profile", "nope", "--yes"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("declares no profile 'nope'"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("basic, default, full"), "{}", r.stderr);
    assert!(sb.source_entry(&sb.identity()).is_some());
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    // A source with no [profiles] says so.
    let plain = Sandbox::with_toml("plain", "");
    let r = plain.mind(&["meld", &plain.spec(), "--profile", "full", "--yes"]);
    assert!(!r.success);
    assert!(r.stderr.contains("declares no profiles"), "{}", r.stderr);
}

#[test]
fn exact_member_naming_no_item_is_bad_reference_at_meld() {
    // spec: PROF-5 -- caught at expansion, not parse; a glob matching nothing
    // is fine.
    let sb = Sandbox::with_toml(
        "src",
        "[profiles]\ndefault = [\"skill:missing\", \"command:*\"]\n",
    );
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("skill:missing"), "{}", r.stderr);
    assert!(sb.installed().is_empty());

    let ok = Sandbox::with_toml(
        "src",
        "[profiles]\ndefault = [\"skill:review\", \"command:*\"]\n",
    );
    let r = ok.mind(&["meld", &ok.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(ok.installed(), svec(&["skill:review"]));
}

#[test]
fn profile_composes_with_learn_and_conflicts_with_register_only() {
    // spec: PROF-15 -- the batch is the union; the --learn match is installed
    // by name (no profile reason, explicit), and both profile flags conflict
    // with --register-only.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--learn", "test", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.installed(),
        svec(&["rule:style", "skill:review", "skill:test"])
    );
    assert!(sb.reasons("skill:test").is_empty());
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["rule:style", "skill:review"])),
        "a --learn match is not a member"
    );

    for flag in [&["--profile", "full"][..], &["--all-profiles"][..]] {
        let other = Sandbox::new("src");
        let spec = other.spec();
        let mut args = vec!["meld", spec.as_str(), "--register-only"];
        args.extend_from_slice(flag);
        let r = other.mind(&args);
        assert!(!r.success, "{flag:?} must conflict with --register-only");
        assert!(r.stderr.contains("cannot be used with"), "{}", r.stderr);
        assert!(other.source_entry(&other.identity()).is_none());
    }
}

#[test]
fn non_tty_meld_without_yes_records_nothing_and_its_remedy_works() {
    // spec: PROF-9 PROF-17 -- a non-TTY meld installs and records nothing and
    // names `mind learn <source> --profile <name>`; pasting it installs the
    // profile and records it.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--profile", "full"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("not a TTY"))
        .unwrap_or_else(|| panic!("{}", r.stdout));
    let cmd = backticked(line);
    assert!(cmd.contains("--profile default,full"), "{cmd}");
    let run = run_printed(&sb, &cmd, &["--yes"]);
    assert!(run.success, "{} {}", run.stdout, run.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    assert!(sb.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn tty_meld_prompts_once_and_records_only_on_yes() {
    // spec: PROF-9 -- one preview and one prompt; a no records nothing.
    let sb = Sandbox::new("src");
    let r = sb.mind_env(
        &["meld", &sb.spec(), "--profile", "basic"],
        &[("MIND_TTY", "1")],
        Some("n\n"),
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("would learn 2 item(s)"), "{}", r.stdout);
    assert!(
        r.stdout.contains("skipped; run `mind learn"),
        "{}",
        r.stdout
    );
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    let sb = Sandbox::new("src");
    let r = sb.mind_env(
        &["meld", &sb.spec(), "--profile", "basic"],
        &[("MIND_TTY", "1")],
        Some("y\n"),
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["basic", "default"])));
}

#[test]
fn interactive_meld_asks_which_profiles_with_default_preselected() {
    // spec: PROF-13 -- an empty answer keeps default; a named answer is the
    // enabled set, as the flags would give it.
    let sb = Sandbox::new("src");
    let r = sb.mind_env(&["meld", &sb.spec()], &[("MIND_TTY", "1")], Some("\ny\n"));
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("enable which profiles?"), "{}", r.stdout);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));

    let sb = Sandbox::new("src");
    let r = sb.mind_env(
        &["meld", &sb.spec()],
        &[("MIND_TTY", "1")],
        Some("full\ny\n"),
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["full"])));
    assert!(sb.installed().contains(&"agent:planner".to_string()));

    // A flag skips the picker.
    let sb = Sandbox::new("src");
    let r = sb.mind_env(
        &["meld", &sb.spec(), "--profile", "basic"],
        &[("MIND_TTY", "1")],
        Some("y\n"),
    );
    assert!(!r.stdout.contains("enable which profiles?"), "{}", r.stdout);
}

#[test]
fn remeld_adds_to_the_recorded_set_and_never_disables() {
    // spec: PROF-16
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));

    let r = sb.mind(&["meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    assert!(sb.installed().contains(&"skill:deploy".to_string()));

    let r = sb.mind(&["meld", &sb.spec(), "--no-default-profile", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.recorded_profiles(),
        Some(svec(&["default", "full"])),
        "a re-meld never disables a profile"
    );
    // A plain re-meld of a tracked source does not fall back to install-all.
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!sb.installed().contains(&"skill:test".to_string()));
}

// ----- learn <source> --profile -----

#[test]
fn learn_source_profile_uses_learns_gate_and_records() {
    // spec: PROF-17 -- the source is a CLI-36 selector; --dry-run installs
    // nothing; the dependency-closure confirmation applies (EOF = no); with
    // --yes the named profile installs (default is not added) and is
    // recorded.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);

    let r = sb.mind(&["learn", "src", "--profile", "full", "--dry-run"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("would learn 5 item(s)"), "{}", r.stdout);
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    let r = sb.mind(&["learn", "src", "--profile", "full"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("cancelled; nothing installed"),
        "{}",
        r.stdout
    );
    assert!(sb.installed().is_empty());

    let r = sb.mind(&["learn", "src", "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["full"])));
    assert_eq!(sb.installed().len(), 5, "{:?}", sb.installed());

    let r = sb.mind(&["learn", "src", "--all-profiles", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.recorded_profiles(),
        Some(svec(&["basic", "default", "full"]))
    );
    assert!(!sb.installed().contains(&"skill:test".to_string()));
}

#[test]
fn learn_source_profile_without_deps_installs_directly() {
    // spec: PROF-17 -- no closure beyond the members: no prompt, as for learn.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let r = sb.mind(&["learn", &sb.identity(), "--profile", "basic"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["basic"])));
    // Undeclared name: ProfileNotFound.
    let r = sb.mind(&["learn", "src", "--profile", "nope"]);
    assert!(!r.success);
    assert!(
        r.stderr.contains("declares no profile 'nope'"),
        "{}",
        r.stderr
    );
}

// ----- --json -----

#[test]
fn json_meld_reports_profiles_and_union_pending() {
    // spec: PROF-18 -- without --yes: `pending_items` counts the expansion and
    // its closures once each, `profiles` names the enabled set, nothing is
    // recorded. With --yes: `installed` lists them all.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["--json", "meld", &sb.spec(), "--profile", "full"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v = json_doc(&r);
    assert_eq!(v["pending_items"], 5, "{v}");
    assert_eq!(strings(&v["profiles"]), Some(svec(&["default", "full"])));
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    let sb = Sandbox::new("src");
    let r = sb.mind(&["--json", "meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v = json_doc(&r);
    assert_eq!(v["installed"].as_array().map(Vec::len), Some(5), "{v}");
    assert_eq!(strings(&v["profiles"]), Some(svec(&["default", "full"])));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
}

#[test]
fn json_learn_reports_profiles() {
    // spec: PROF-18
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let r = sb.mind(&["--json", "learn", "src", "--profile", "basic"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v = json_doc(&r);
    assert_eq!(v["action"], "learn");
    assert_eq!(v["outcome"], "installed");
    assert_eq!(strings(&v["profiles"]), Some(svec(&["basic"])));
    assert_eq!(v["installed"].as_array().map(Vec::len), Some(2), "{v}");
}

// ----- Item links -----

#[test]
fn profile_flags_on_an_item_link_are_a_usage_error() {
    // spec: PROF-8
    let sb = Sandbox::new("src");
    let link = format!("file://{}/tree/main/skills/review", sb.spec());
    let r = sb.mind(&["meld", &link, "--profile", "basic"]);
    assert!(!r.success);
    assert!(
        r.stderr
            .contains("--profile cannot be used with an item link"),
        "{}",
        r.stderr
    );
    let r = sb.mind(&["learn", &link, "--all-profiles"]);
    assert!(!r.success);
    assert!(
        r.stderr
            .contains("--all-profiles cannot be used with an item link"),
        "{}",
        r.stderr
    );
    let r = sb.mind(&["meld", &link, "--no-default-profile"]);
    assert!(!r.success);
    assert!(sb.installed().is_empty());
}

// ----- Recording -----

#[test]
fn by_name_learn_marks_a_profile_item_explicit() {
    // spec: PROF-20 -- keeping the profile names.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    assert!(
        sb.manifest_item("skill:review")
            .unwrap()
            .get("explicit")
            .is_none()
    );
    let r = sb.mind(&["learn", "review"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let item = sb.manifest_item("skill:review").unwrap();
    assert_eq!(item["explicit"], true, "{item}");
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
}

#[test]
fn enabling_a_profile_over_a_hand_installed_item_keeps_it_explicit() {
    // spec: PROF-20 -- an item installed by name before the profile install
    // is tagged, not reinstalled, and marked explicit.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    assert!(sb.mind(&["learn", "review"]).success);
    let r = sb.mind(&["learn", "src", "--profile", "basic", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let item = sb.manifest_item("skill:review").unwrap();
    assert_eq!(item["explicit"], true, "{item}");
    assert_eq!(sb.reasons("skill:review"), svec(&["basic"]));
    assert!(item.get("explicit").is_some());
    let style = sb.manifest_item("rule:style").unwrap();
    assert!(style.get("explicit").is_none(), "{style}");
}

// ----- Curators -----

/// A curator repo whose mind.toml lists `nested` with `entry` (the inline
/// table's body after `source = ...`).
fn curator(nested: &Sandbox, entry: &str) -> Sandbox {
    let toml = format!(
        "[discover]\nsources = [{{ source = {:?}{entry} }}]\n",
        nested.spec()
    );
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-profiles-cur-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let source = base.join("registry");
    write_file(&source.join("README.md"), "# registry\n");
    write_file(&source.join("mind.toml"), &toml);
    git_init(&source);
    Sandbox {
        base: base.clone(),
        source,
        mind_home: base.join("mind"),
        claude_home: base.join("claude"),
    }
}

impl Sandbox {
    /// The recorded profiles of `nested` in this sandbox's registry.
    fn nested_profiles(&self, nested: &Sandbox) -> Option<Vec<String>> {
        strings(&self.source_entry(&nested.identity())?["profiles"])
    }
}

#[test]
fn curator_entry_profiles_apply_like_the_flags() {
    // spec: PROF-26 -- `profiles` acts as --profile (default kept).
    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", profiles = [\"full\"]");
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        reg.nested_profiles(&nested),
        Some(svec(&["default", "full"]))
    );
    assert!(reg.installed().contains(&"skill:deploy".to_string()));
}

#[test]
fn curator_default_profile_false_acts_as_no_default_profile() {
    // spec: PROF-26
    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", profiles = [\"full\"], default-profile = false");
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(reg.nested_profiles(&nested), Some(svec(&["full"])));
}

#[test]
fn curator_profiles_with_install_items_install_the_union() {
    // spec: PROF-26
    let nested = Sandbox::new("nested");
    let reg = curator(
        &nested,
        ", profiles = [\"basic\"], install-items = [\"skill:test\"]",
    );
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        reg.installed(),
        svec(&["rule:style", "skill:review", "skill:test"])
    );
}

#[test]
fn curator_install_true_offers_the_nested_default() {
    // spec: PROF-26 -- the same as a plain meld of the nested source.
    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", install = true");
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(reg.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(reg.nested_profiles(&nested), Some(svec(&["default"])));

    // A nested source without `default` keeps install-all (DSC-58).
    let plain = Sandbox::with_toml("plain", "");
    let reg = curator(&plain, ", install = true");
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(reg.installed().len(), 6, "{:?}", reg.installed());
}

#[test]
fn curator_undeclared_profile_is_bad_reference_at_meld() {
    // spec: PROF-26
    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", profiles = [\"nope\"]");
    let r = reg.mind(&["meld", &reg.spec(), "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("profile 'nope'"), "{}", r.stderr);
    assert!(reg.installed().is_empty());
}

// ----- Adversarial additions (certification) -----

/// Add a skill `name` with `body` to the fixture source and commit it, so a
/// test can shape the catalog beyond the standard items.
fn add_skill(sb: &Sandbox, name: &str, body: &str) {
    write_file(
        &sb.source.join(format!("skills/{name}/SKILL.md")),
        &format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}\n"),
    );
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "more"]] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&sb.source)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }
}

#[test]
fn closure_item_shared_by_two_members_carries_the_union_of_reasons() {
    // spec: PROF-6 PROF-20 -- skill:helper is pulled in by deploy (via p1) and
    // deploy2 (via p2); it is a closure item, not a member, and records both.
    let sb = Sandbox::with_toml(
        "src",
        "[profiles]\np1 = [\"skill:deploy\"]\np2 = [\"skill:deploy2\"]\n",
    );
    add_skill(&sb, "deploy2", "# deploy2\nUses {{ns:helper}}.");
    let r = sb.mind(&["meld", &sb.spec(), "--profile", "p1,p2", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.installed(),
        svec(&["skill:deploy", "skill:deploy2", "skill:helper"])
    );
    assert_eq!(sb.reasons("skill:helper"), svec(&["p1", "p2"]));
    assert_eq!(sb.reasons("skill:deploy"), svec(&["p1"]));
    assert_eq!(sb.reasons("skill:deploy2"), svec(&["p2"]));
    assert_eq!(
        sb.recorded_snapshot(),
        Some(svec(&["skill:deploy", "skill:deploy2"]))
    );
}

#[test]
fn closure_pending_items_count_each_item_once_in_json() {
    // spec: PROF-18 -- two members sharing one closure item count it once.
    let sb = Sandbox::with_toml(
        "src",
        "[profiles]\np1 = [\"skill:deploy\"]\np2 = [\"skill:deploy2\"]\n",
    );
    add_skill(&sb, "deploy2", "# deploy2\nUses {{ns:helper}}.");
    let r = sb.mind(&["--json", "meld", &sb.spec(), "--profile", "p1,p2"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(json_doc(&r)["pending_items"], 3, "{}", r.stdout);
    assert!(sb.installed().is_empty());
}

#[test]
fn policy_skipped_member_stays_out_of_the_snapshot() {
    // spec: PROF-19 -- a source melded before a locked allowlist stopped
    // admitting it: learn skips the members, so nothing installs and none is
    // snapshotted (PROF-21 offers them again later).
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let policy_path = sb.base.join("policy.toml");
    write_file(
        &policy_path,
        "[sources]\nlock = true\nallow = [\"example.invalid/*\"]\n",
    );
    let policy = policy_path.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["learn", "src", "--profile", "basic", "--yes"],
        &[("MIND_POLICY_FILE", policy.as_str())],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("not permitted by the managed policy"),
        "{} {}",
        r.stdout,
        r.stderr
    );
    assert!(sb.installed().is_empty(), "{} {}", r.stdout, r.stderr);
    let snap = sb.recorded_snapshot().unwrap_or_default();
    assert!(snap.is_empty(), "skipped members snapshotted: {snap:?}");
}

#[test]
fn partial_install_failure_still_records_what_installed() {
    // spec: PROF-19 PROF-20 -- a member that fails (bad {{ns:}} reference)
    // stops the batch, but the members installed before it are recorded and
    // the failed one is not snapshotted.
    let sb = Sandbox::with_toml(
        "src",
        "[profiles]\ndefault = [\"skill:review\", \"skill:zbroken\"]\n",
    );
    add_skill(&sb, "zbroken", "# zbroken\nUses {{ns:nonexistent}}.");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    assert!(!sb.installed().contains(&"skill:zbroken".to_string()));
    assert_eq!(sb.installed(), svec(&["skill:review"]), "{}", r.stderr);
    assert_eq!(sb.reasons("skill:review"), svec(&["default"]));
    assert_eq!(sb.recorded_snapshot(), Some(svec(&["skill:review"])));
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert!(sb.source_entry(&sb.identity()).is_some());
}

#[test]
fn remeld_json_with_profile_flags_installs_and_reports() {
    // spec: PROF-16 PROF-18 -- the --json re-meld profile branch.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let r = sb.mind(&["--json", "meld", &sb.spec(), "--profile", "full"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v = json_doc(&r);
    assert_eq!(v["outcome"], "already-melded", "{v}");
    assert_eq!(strings(&v["profiles"]), Some(svec(&["default", "full"])));
    assert_eq!(v["pending_items"], 3, "{v}");
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));

    let r = sb.mind(&["--json", "meld", &sb.spec(), "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let v = json_doc(&r);
    assert_eq!(strings(&v["profiles"]), Some(svec(&["default", "full"])));
    assert_eq!(v["installed"].as_array().map(Vec::len), Some(3), "{v}");
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    assert!(sb.installed().contains(&"skill:helper".to_string()));
}

#[test]
fn curator_json_install_profiles_apply_like_the_flags() {
    // spec: PROF-26 -- the --json twin of the curated-source profile branch.
    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", profiles = [\"full\"], default-profile = false");
    let r = reg.mind(&["--json", "meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(reg.nested_profiles(&nested), Some(svec(&["full"])));
    assert!(reg.installed().contains(&"skill:deploy".to_string()));
    assert!(reg.installed().contains(&"agent:planner".to_string()));

    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", install = true");
    let r = reg.mind(&["--json", "meld", &reg.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(reg.installed(), svec(&["rule:style", "skill:review"]));
    assert_eq!(reg.nested_profiles(&nested), Some(svec(&["default"])));

    let nested = Sandbox::new("nested");
    let reg = curator(&nested, ", profiles = [\"nope\"]");
    let r = reg.mind(&["--json", "meld", &reg.spec(), "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(reg.installed().is_empty());
}

#[test]
fn enabled_profile_matching_nothing_is_recorded_and_installs_nothing() {
    // spec: PROF-5 PROF-19 -- an all-glob profile that matches no item is not
    // an error; it is still enabled, with an empty snapshot.
    let sb = Sandbox::with_toml("src", "[profiles]\ndefault = [\"command:*\"]\n");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.installed().is_empty(), "{:?}", sb.installed());
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));
    assert_eq!(
        sb.recorded_snapshot().unwrap_or_default(),
        Vec::<String>::new()
    );

    let sb = Sandbox::with_toml("src", "[profiles]\ndefault = [\"command:*\"]\n");
    let r = sb.mind(&["--json", "meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(strings(&json_doc(&r)["profiles"]), Some(svec(&["default"])));
    assert!(sb.installed().is_empty());
}

#[test]
fn learn_source_profile_with_unknown_ambiguous_or_hash_selector_fails_cleanly() {
    // spec: PROF-17 -- `<source>` is a CLI-36 selector; a `#` ref is invalid.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);

    let r = sb.mind(&["learn", "nosuch", "--profile", "basic", "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(r.stderr.contains("nosuch"), "{}", r.stderr);

    let r = sb.mind(&["learn", "src#review", "--profile", "basic", "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    // A second melded source with the same repo name makes `src` ambiguous.
    let other = Sandbox::new("src");
    assert!(sb.mind(&["meld", &other.spec(), "--register-only"]).success);
    let r = sb.mind(&["learn", "src", "--profile", "basic", "--yes"]);
    assert!(!r.success, "{}", r.stdout);
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);
    // The full identity disambiguates.
    let r = sb.mind(&["learn", &sb.identity(), "--profile", "basic", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["basic"])));
}

#[test]
fn learn_json_with_closure_needs_confirmation_and_installs_nothing() {
    // spec: PROF-17 PROF-18 -- under --json the closure confirmation cannot be
    // asked, so it is ConfirmationRequired; --yes proceeds.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let r = sb.mind(&["--json", "learn", "src", "--profile", "full"]);
    assert!(!r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("confirmation") || r.stdout.contains("confirmation"),
        "{} {}",
        r.stdout,
        r.stderr
    );
    assert!(sb.installed().is_empty());
    assert_eq!(sb.recorded_profiles(), None);

    let r = sb.mind(&["--json", "learn", "src", "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(json_doc(&r)["outcome"], "installed");
    assert_eq!(sb.installed().len(), 5);
}

#[test]
fn no_default_profile_composes_with_register_only() {
    // spec: PROF-10 PROF-15 -- only --profile/--all-profiles conflict with
    // --register-only; --no-default-profile is accepted and installs nothing.
    let sb = Sandbox::new("src");
    let r = sb.mind(&[
        "meld",
        &sb.spec(),
        "--register-only",
        "--no-default-profile",
    ]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(sb.installed().is_empty());
    assert!(sb.source_entry(&sb.identity()).is_some());
    assert_eq!(sb.recorded_profiles(), None);
}

#[test]
fn help_text_mentions_the_profile_flags() {
    // spec: PROF-9 PROF-10 PROF-11 PROF-17
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", "--help"]);
    assert!(r.success);
    for flag in ["--profile", "--no-default-profile", "--all-profiles"] {
        assert!(r.stdout.contains(flag), "meld --help lacks {flag}");
    }
    let r = sb.mind(&["learn", "--help"]);
    assert!(r.success);
    for flag in ["--profile", "--all-profiles"] {
        assert!(r.stdout.contains(flag), "learn --help lacks {flag}");
    }
}

#[test]
fn learn_all_conflicts_with_profile_flags() {
    // spec: PROF-17 -- `--all` is the whole-source form; mixing is a usage error.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    for flag in ["--profile=basic", "--all-profiles"] {
        let r = sb.mind(&["learn", "src", "--all", flag, "--yes"]);
        assert!(!r.success, "{flag}");
        assert!(r.stderr.contains("cannot be used with"), "{}", r.stderr);
    }
    assert!(sb.installed().is_empty());
}

// ----- Hook environment (HOOK-140..143) -----

/// A [`PROFILES`] source whose source install/uninstall hooks and whose
/// review, deploy, and helper skills' install/uninstall hooks each append
/// `<who> <event> $MIND_PROFILES|$MIND_ITEM_PROFILES` to `$HOOK_LOG`.
fn hooked(name: &str) -> Sandbox {
    let toml = format!(
        "{PROFILES}\n[[hooks]]\nrun = \"sh ./log.sh source install\"\nevent = \"install\"\n\n\
         [[hooks]]\nrun = \"sh ./log.sh source uninstall\"\nevent = \"uninstall\"\n"
    );
    let sb = Sandbox::with_toml(name, &toml);
    let log = "printf '%s %s %s|%s\\n' \"$1\" \"$2\" \"$MIND_PROFILES\" \"$MIND_ITEM_PROFILES\" \
               >> \"$HOOK_LOG\"\n";
    write_file(&sb.source.join("log.sh"), log);
    for skill in ["review", "deploy", "helper"] {
        let body = if skill == "deploy" {
            "Uses {{ns:helper}}."
        } else {
            ""
        };
        write_file(
            &sb.source.join(format!("skills/{skill}/SKILL.md")),
            &format!(
                "---\nname: {skill}\ndescription: {skill} skill\n\
                 install: sh log.sh {skill} install\nuninstall: sh log.sh {skill} uninstall\n\
                 ---\n# {skill}\n{body}\n"
            ),
        );
        write_file(&sb.source.join(format!("skills/{skill}/log.sh")), log);
    }
    add_skill(&sb, "test", "# test");
    sb
}

impl Sandbox {
    fn hook_log(&self) -> std::path::PathBuf {
        self.base.join("hook.log")
    }

    /// `mind` with the hook log wired up and hooks run unattended (HOOK-83).
    fn mind_hooked(&self, args: &[&str], envs: &[(&str, &str)], stdin: Option<&str>) -> Run {
        let log = self.hook_log();
        let mut all: Vec<(&str, &str)> = vec![("HOOK_LOG", log.to_str().unwrap())];
        all.extend_from_slice(envs);
        let mut argv: Vec<&str> = args.to_vec();
        argv.push("--accept-hooks=install,update,uninstall");
        self.mind_env(&argv, &all, stdin)
    }

    /// The hook log lines, then cleared.
    fn take_hook_log(&self) -> Vec<String> {
        let text = std::fs::read_to_string(self.hook_log()).unwrap_or_default();
        let _ = std::fs::remove_file(self.hook_log());
        text.lines().map(str::to_string).collect()
    }
}

#[test]
fn meld_hooks_see_the_enabled_profiles_and_item_reasons() {
    // spec: HOOK-140 HOOK-141 HOOK-142 -- the source hook sees the selection;
    // each item hook sees the profiles it is installed for, a closure item
    // (helper) those of the member that pulled it in.
    let sb = hooked("src");
    let r = sb.mind_hooked(
        &["meld", &sb.spec(), "--profile", "full", "--yes"],
        &[],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = sb.take_hook_log();
    assert_eq!(
        log.first().map(String::as_str),
        Some("source install default,full|")
    );
    assert!(
        log.contains(&"deploy install default,full|full".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(&"helper install default,full|full".to_string()),
        "{log:?}"
    );
    let review = sb.reasons("skill:review").join(",");
    assert!(!review.is_empty());
    assert!(
        log.contains(&format!("review install default,full|{review}")),
        "{log:?}"
    );
}

#[test]
fn hook_vars_are_always_set_and_override_inherited_values() {
    // spec: HOOK-140 HOOK-142 -- with no profile install following
    // (`--register-only`) both are empty, never the inherited value.
    let sb = hooked("src");
    let r = sb.mind_hooked(
        &["meld", &sb.spec(), "--register-only"],
        &[("MIND_PROFILES", "leak"), ("MIND_ITEM_PROFILES", "leak")],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.take_hook_log(), svec(&["source install |"]));
}

#[test]
fn source_hooks_see_the_picker_answer() {
    // spec: HOOK-142 -- the PROF-13 picker is asked before the source's
    // install hooks run, so they see its answer.
    let sb = hooked("src");
    let r = sb.mind_hooked(
        &["meld", &sb.spec()],
        &[("MIND_TTY", "1")],
        Some("full\ny\n"),
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = sb.take_hook_log();
    assert_eq!(
        log.first().map(String::as_str),
        Some("source install full|"),
        "{log:?}"
    );
    assert_eq!(sb.recorded_profiles(), Some(svec(&["full"])));
}

#[test]
fn nested_source_hooks_see_the_curator_entry_selection() {
    // spec: HOOK-142 -- a curator entry's `profiles` is the nested source's
    // selection, so its install hooks see it.
    let nested = hooked("nested");
    let reg = curator(&nested, ", profiles = [\"full\"], default-profile = false");
    let log = reg.hook_log();
    let r = reg.mind_env(
        &[
            "meld",
            &reg.spec(),
            "--yes",
            "--accept-hooks=install,update,uninstall",
        ],
        &[("HOOK_LOG", log.to_str().unwrap())],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = reg.take_hook_log();
    assert_eq!(
        log.first().map(String::as_str),
        Some("source install full|"),
        "{log:?}"
    );
    assert!(
        log.contains(&"deploy install full|full".to_string()),
        "{log:?}"
    );
}

#[test]
fn enabling_and_disabling_a_profile_reports_the_resulting_set() {
    // spec: HOOK-141 HOOK-143 -- `learn --profile` sees the recorded set plus
    // the profile being enabled; `forget --profile` sees the set that stays,
    // with the removed item's reasons; neither re-offers the source hooks.
    let sb = hooked("src");
    assert!(
        sb.mind_hooked(&["meld", &sb.spec(), "--yes"], &[], None)
            .success
    );
    sb.take_hook_log();

    let r = sb.mind_hooked(&["learn", "src", "--profile", "full", "--yes"], &[], None);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = sb.take_hook_log();
    assert!(
        log.contains(&"deploy install default,full|full".to_string()),
        "{log:?}"
    );
    assert!(!log.iter().any(|l| l.starts_with("source ")), "{log:?}");

    let r = sb.mind_hooked(&["forget", "src", "--profile", "full", "--yes"], &[], None);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = sb.take_hook_log();
    assert!(
        log.contains(&"deploy uninstall default|full".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(&"helper uninstall default|full".to_string()),
        "{log:?}"
    );
    assert!(!log.iter().any(|l| l.starts_with("source ")), "{log:?}");
}

#[test]
fn unmeld_and_hooks_run_see_the_recorded_state() {
    // spec: HOOK-141 -- outside a profile change, hooks see the recorded set
    // and the item's recorded reasons.
    let sb = hooked("src");
    assert!(
        sb.mind_hooked(
            &["meld", &sb.spec(), "--profile", "full", "--yes"],
            &[],
            None
        )
        .success
    );
    sb.take_hook_log();

    let r = sb.mind_hooked(
        &["hooks", "run", "src#skill:deploy", "--event", "uninstall"],
        &[],
        None,
    );
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(
        sb.take_hook_log(),
        svec(&["deploy uninstall default,full|full"])
    );

    let r = sb.mind_hooked(&["unmeld", "src", "--yes"], &[], None);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let log = sb.take_hook_log();
    assert!(
        log.contains(&"source uninstall default,full|".to_string()),
        "{log:?}"
    );
    assert!(
        log.contains(&"helper uninstall default,full|full".to_string()),
        "{log:?}"
    );
}

// ----- Discovering profiles after meld -----

/// A profile's block in a listing: its heading line (`  * <name> ...`) and
/// the indented lines under it.
fn profile_block<'a>(stdout: &'a str, profile: &str) -> Vec<&'a str> {
    let mut lines = stdout
        .lines()
        .skip_while(|l| l.starts_with("      ") || l.split_whitespace().nth(1) != Some(profile));
    let heading = lines
        .next()
        .unwrap_or_else(|| panic!("no block for {profile} in: {stdout}"));
    std::iter::once(heading)
        .chain(lines.take_while(|l| l.starts_with("      ")))
        .collect()
}

/// The text after `<label>:` in a profile block, its wrapped lines joined
/// with single spaces.
fn block_field(stdout: &str, profile: &str, label: &str) -> Option<String> {
    let marker = format!("{label}:");
    let block = profile_block(stdout, profile);
    let start = block
        .iter()
        .position(|l| l.trim_start().starts_with(&marker))?;
    let mut words = vec![block[start].trim_start()[marker.len()..].trim()];
    words.extend(
        block[start + 1..]
            .iter()
            .take_while(|l| l.starts_with("               "))
            .map(|l| l.trim()),
    );
    Some(words.join(" "))
}

/// The command printed on a profile's `enable:` line in a listing.
fn enable_command(stdout: &str, profile: &str) -> String {
    block_field(stdout, profile, "enable")
        .unwrap_or_else(|| panic!("no enable line for {profile} in: {stdout}"))
}

#[test]
fn recall_profiles_lists_members_enabled_state_and_counts() {
    // spec: PROF-31 -- each declared profile with its members, the enabled
    // marker, and `<n>/<m> installed`, then a runnable `learn` command per
    // profile that is not enabled.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let r = sb.mind(&["recall", "--profiles"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let out = &r.stdout;
    assert!(
        out.contains(&format!("profiles of {}:", sb.identity())),
        "{out}"
    );
    let row = |name: &str| profile_block(out, name)[0].to_string();
    assert!(row("default").contains("(enabled)"), "{out}");
    assert!(row("default").contains("2/2 installed"), "{out}");
    assert_eq!(
        block_field(out, "basic", "members").as_deref(),
        Some("skill:review, rule:style"),
        "{out}"
    );
    assert!(!row("basic").contains("(enabled)"), "{out}");
    // full = basic + agent:planner + skill:deploy; the closure item
    // skill:helper is not a member.
    assert!(row("full").contains("2/4 installed"), "{out}");
    assert_eq!(block_field(out, "default", "enable"), None, "{out}");
    let cmd = enable_command(out, "full");
    let r = run_printed(&sb, &cmd, &["--yes"]);
    assert!(r.success, "{cmd}: {} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
    assert!(sb.manifest_item("agent:planner").is_some());

    // A selector narrows the listing to one source.
    let r = sb.mind(&["recall", "--profiles", "src"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("full (enabled)"), "{}", r.stdout);
    assert!(r.stdout.contains("4/4 installed"), "{}", r.stdout);
}

#[test]
fn profile_listing_wraps_members_and_aligns_counts() {
    // spec: PROF-36 -- one block per profile: the counts line up across
    // headings, members wrap to COLUMNS under a hanging indent, and the
    // enable command stays on one line.
    let members = [
        "skill:review",
        "skill:test",
        "skill:helper",
        "skill:deploy",
        "agent:planner",
        "rule:style",
        "skill:*",
        "agent:*",
    ];
    let quoted: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    let toml = format!(
        "[profiles]\nwide = [{}]\nlong-profile-name = [\"rule:style\"]\n",
        quoted.join(", ")
    );
    let sb = Sandbox::with_toml("src", &toml);
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let r = sb.mind_env(&["recall", "--profiles"], &[("COLUMNS", "40")], None);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let out = &r.stdout;

    let wide = profile_block(out, "wide");
    assert!(
        wide.iter()
            .filter(|l| l.contains("members:") || l.starts_with("               "))
            .count()
            > 1,
        "members should wrap at 40 columns: {out}"
    );
    for line in out.lines() {
        if line.starts_with("profiles of") || line.trim_start().starts_with("enable:") {
            continue;
        }
        assert!(
            line.chars().count() <= 40,
            "line over 40 columns: {line:?}\n{out}"
        );
    }
    assert_eq!(
        block_field(out, "wide", "members"),
        Some(members.join(", ")),
        "{out}"
    );
    let at = |name: &str| profile_block(out, name)[0].find(" installed");
    assert!(at("wide").is_some(), "{out}");
    assert_eq!(at("wide"), at("long-profile-name"), "{out}");
    let cmd = enable_command(out, "wide");
    assert_eq!(
        shell_split(&cmd),
        svec(&["mind", "learn", &sb.identity(), "--profile", "wide"])
    );
}

#[test]
fn recall_profiles_without_profiled_sources_says_so() {
    // spec: PROF-31 -- nothing declares profiles: say so; a selected source
    // that declares none is named; an unknown selector fails.
    let sb = Sandbox::with_toml("plain", "");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let r = sb.mind(&["recall", "--profiles"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("no melded source declares profiles"),
        "{}",
        r.stdout
    );
    let r = sb.mind(&["recall", "--profiles", "plain"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout
            .contains(&format!("{} declares no profiles", sb.identity())),
        "{}",
        r.stdout
    );
    let r = sb.mind(&["recall", "--profiles", "nope"]);
    assert!(!r.success, "{}", r.stdout);
    let r = sb.mind(&["recall", "--profiles", "--sources"]);
    assert!(
        !r.success,
        "--profiles conflicts with --sources: {}",
        r.stdout
    );
}

#[test]
fn recall_profiles_marks_a_profile_whose_expansion_fails() {
    // spec: PROF-31 PROF-32 -- an exact member naming no item is reported on
    // its row rather than failing the listing.
    let sb = Sandbox::with_toml(
        "src",
        "[profiles]\nbasic = [\"skill:review\"]\nbroken = [\"skill:missing\"]\n",
    );
    assert!(sb.mind(&["meld", &sb.spec(), "--register-only"]).success);
    let r = sb.mind(&["recall", "--profiles"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("0/1 installed"), "{}", r.stdout);
    let error = block_field(&r.stdout, "broken", "error");
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("skill:missing")),
        "{}",
        r.stdout
    );

    let doc = json_doc(&sb.mind(&["recall", "--profiles", "--json"]));
    let profiles = doc["items"][0]["profiles"].as_array().unwrap();
    let broken = profiles.iter().find(|p| p["name"] == "broken").unwrap();
    assert!(broken["error"].is_string(), "{broken}");
    assert!(broken.get("total").is_none(), "{broken}");
}

#[test]
fn recall_profiles_json_carries_items_and_counts() {
    // spec: PROF-32
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let doc = json_doc(&sb.mind(&["recall", "--profiles", "--json"]));
    let src = &doc["items"][0];
    assert_eq!(src["source"], sb.identity());
    let profiles = src["profiles"].as_array().unwrap();
    let names: Vec<&str> = profiles
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["basic", "default", "full"]);
    let full = &profiles[2];
    assert_eq!(full["enabled"], false);
    assert_eq!(
        full["members"],
        serde_json::json!(["basic", "agent:*", "skill:deploy"])
    );
    assert_eq!(
        full["items"],
        serde_json::json!([
            "agent:planner",
            "rule:style",
            "skill:deploy",
            "skill:review"
        ])
    );
    assert_eq!(full["installed"], 2);
    assert_eq!(full["total"], 4);
    assert_eq!(profiles[1]["enabled"], true);
}

#[test]
fn learn_with_a_bare_profile_flag_lists_the_choices() {
    // spec: PROF-33 -- `--profile` with no value lists that source's profiles
    // and installs nothing; under --json it is PROF-32's document; an empty
    // value beside a named profile is ignored.
    let sb = Sandbox::new("src");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let before = sb.installed();
    let r = sb.mind(&["learn", "src", "--profile"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout
            .contains(&format!("profiles of {}:", sb.identity())),
        "{}",
        r.stdout
    );
    assert!(
        enable_command(&r.stdout, "full").starts_with("mind learn"),
        "{}",
        r.stdout
    );
    assert_eq!(sb.installed(), before);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default"])));

    let doc = json_doc(&sb.mind(&["learn", "src", "--profile", "--json"]));
    assert_eq!(doc["items"][0]["source"], sb.identity());
    assert_eq!(sb.installed(), before);

    let r = sb.mind(&["learn", "src", "--profile", "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert_eq!(sb.recorded_profiles(), Some(svec(&["default", "full"])));
}

#[test]
fn profile_header_names_the_recall_and_learn_commands() {
    // spec: PROF-34 -- the not-enabled line names commands that work after
    // the meld, including when nothing installed (PROF-10).
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--no-default-profile"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let line = r
        .stdout
        .lines()
        .find(|l| l.contains("not enabled:"))
        .unwrap_or_else(|| panic!("{}", r.stdout));
    assert!(line.contains("not enabled: basic, default, full"), "{line}");
    let recall = backticked(line);
    assert_eq!(
        shell_split(&recall),
        svec(&["mind", "recall", "--profiles", &sb.identity()])
    );
    let r = run_printed(&sb, &recall, &[]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(r.stdout.contains("full"), "{}", r.stdout);
    let after_recall = &line[line.find(&recall).unwrap() + recall.len() + 1..];
    assert_eq!(
        shell_split(&backticked(after_recall)),
        svec(&["mind", "learn", &sb.identity(), "--profile", "<name>"])
    );
}

#[test]
fn profile_install_prints_progress_before_resolving() {
    // spec: PROF-35 -- a progress line names the enabled profiles before the
    // resolve, ahead of the first install line; none under --json. The
    // members install in one learn pass, so no member reports "already
    // installed" for a dependency an earlier member pulled in.
    let sb = Sandbox::new("src");
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains(&format!(
            "resolving profile(s) default of {} ...",
            sb.identity()
        )),
        "{}",
        r.stdout
    );

    let r = sb.mind(&["learn", "src", "--profile", "full", "--yes"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    let progress = r
        .stdout
        .find(&format!(
            "resolving profile(s) full of {} ...",
            sb.identity()
        ))
        .unwrap_or_else(|| panic!("{}", r.stdout));
    let first_learned = r
        .stdout
        .find("learned ")
        .unwrap_or_else(|| panic!("{}", r.stdout));
    assert!(progress < first_learned, "{}", r.stdout);
    assert!(
        !r.stdout.contains("already installed; nothing to do"),
        "{}",
        r.stdout
    );
    assert_eq!(
        sb.installed(),
        svec(&[
            "agent:planner",
            "rule:style",
            "skill:deploy",
            "skill:helper",
            "skill:review"
        ])
    );

    sb.mind(&["forget", "src", "--profile", "full", "--yes"]);
    let r = sb.mind(&["learn", "src", "--profile", "full", "--yes", "--json"]);
    assert!(r.success, "{} {}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("resolving"), "{}", r.stdout);
    json_doc(&r);
}
