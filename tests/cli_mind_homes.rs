//! Integration tests for multiple mind homes (spec/homes.md): extra homes
//! listed in `MIND_HOMES` or the `mind-homes` config key, chosen per source by
//! `meld --home` or the source's `[source] mind-home`.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture: a local git repo melded by path, with `MIND_HOME`/`CLAUDE_HOME`
//! pointed at temp dirs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    /// The default home (`MIND_HOME`).
    default_home: PathBuf,
    /// An extra home, listed in `MIND_HOMES` unless a test says otherwise.
    extra: PathBuf,
    claude_home: PathBuf,
    /// Whether `MIND_HOMES` lists `extra`.
    env_lists_extra: bool,
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Run {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

impl Sandbox {
    fn new(mind_toml: Option<&str>) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-homes-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sb = Sandbox {
            source: base.join("src"),
            default_home: base.join("default"),
            extra: base.join("extra"),
            claude_home: base.join("claude"),
            env_lists_extra: true,
            base,
        };
        std::fs::create_dir_all(&sb.claude_home).unwrap();
        // The skill's install hook records the root its store copy is in.
        write(
            &sb.source.join("skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\nbody\n",
        );
        if let Some(toml) = mind_toml {
            write(&sb.source.join("mind.toml"), toml);
        }
        git(&sb.source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&sb.source, &["config", "user.email", "t@t"]);
        git(&sb.source, &["config", "user.name", "t"]);
        git(&sb.source, &["add", "-A"]);
        git(&sb.source, &["commit", "-qm", "initial"]);
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_in(&std::env::current_dir().unwrap(), args)
    }

    fn mind_in(&self, cwd: &Path, args: &[&str]) -> Run {
        self.mind_env(cwd, &[], args)
    }

    fn mind_env(&self, cwd: &Path, env: &[(&str, &Path)], args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .envs(env.iter().copied())
            .current_dir(cwd)
            .env("MIND_HOME", &self.default_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_HOMES")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if self.env_lists_extra {
            cmd.env("MIND_HOMES", &self.extra);
        }
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        }
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// The registry identity a local-path meld gives this fixture (STO-13).
    fn identity(&self) -> String {
        let owner = self.base.file_name().unwrap().to_string_lossy();
        format!("local/{owner}/src")
    }

    fn extra_str(&self) -> String {
        self.extra.to_string_lossy().into_owned()
    }

    fn sources_in(&self, home: &Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(home.join("sources.json")) else {
            return Vec::new();
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        v["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn items_in(&self, home: &Path) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(home.join("manifest.json")) else {
            return Vec::new();
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        v["items"].as_object().unwrap().keys().cloned().collect()
    }

    /// Where `skills/alpha` in the lobe resolves to.
    fn alpha_target(&self) -> PathBuf {
        std::fs::canonicalize(self.claude_home.join("skills/alpha")).expect("alpha is linked")
    }

    fn canon(&self, p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Undo any read-only bits a test set, so the tree can be removed.
        let _ = Command::new("chmod")
            .args(["-R", "u+w"])
            .arg(&self.base)
            .status();
        let _ = std::fs::remove_dir_all(&self.base);
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

fn chmod(mode: &str, path: &Path) {
    let status = Command::new("chmod")
        .arg(mode)
        .arg(path)
        .status()
        .expect("run chmod");
    assert!(status.success());
}

/// `meld --home` into an extra home keeps the source's registry entry, item
/// manifest, and store copy there, and the default home is never written:
/// the meld works with the default home read-only. Every later verb sees the
/// union and writes back to the extra home.
#[test]
fn meld_into_an_extra_home_writes_nothing_in_the_default_home() {
    // spec: HOME-1 HOME-2 HOME-3 HOME-4 HOME-6 HOME-7 HOME-9
    let sb = Sandbox::new(None);
    // An existing, read-only default home (the managed-sandbox case).
    let r = sb.mind(&["config", "show"]);
    assert!(r.ok(), "config show: {}", r.stderr);
    std::fs::create_dir_all(&sb.default_home).unwrap();
    std::fs::write(sb.default_home.join(".lock"), "").unwrap();
    chmod("a-w", &sb.default_home.join(".lock"));
    chmod("a-w", &sb.default_home);
    let read_only = std::fs::write(sb.default_home.join("probe"), "").is_err();

    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    chmod("u+w", &sb.default_home);

    assert_eq!(sb.sources_in(&sb.extra).len(), 1);
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
    assert!(sb.items_in(&sb.default_home).is_empty());
    assert!(
        sb.alpha_target()
            .starts_with(sb.canon(&sb.extra).join("store")),
        "the link resolves into the extra home's store"
    );
    if read_only {
        assert!(!sb.default_home.join("sources.json").exists());
        assert!(!sb.default_home.join("store").exists());
    }

    // The union view and a write-back to the extra home.
    let r = sb.mind(&["recall", "--json"]);
    assert!(r.ok(), "recall: {}", r.stderr);
    assert!(r.stdout.contains("skill:alpha"), "{}", r.stdout);
    let r = sb.mind(&["--yes", "forget", "alpha"]);
    assert!(r.ok(), "forget: {}\n{}", r.stdout, r.stderr);
    assert!(sb.items_in(&sb.extra).is_empty());
    assert!(!sb.claude_home.join("skills/alpha").exists());
    assert!(!sb.extra.join("store/skill/alpha").exists());

    let r = sb.mind(&["--yes", "unmeld", &sb.identity()]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    assert!(sb.sources_in(&sb.extra).is_empty());
}

/// `auto-update run` with a read-only default home warns that its state was
/// not saved instead of failing.
#[test]
fn auto_update_runs_with_a_read_only_default_home() {
    // spec: HOME-1
    let sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    std::fs::write(sb.default_home.join(".lock"), "").unwrap();
    chmod("a-w", &sb.default_home);
    if std::fs::write(sb.default_home.join("probe"), "").is_ok() {
        // Running as root: permissions are not enforced, nothing to check.
        return;
    }
    let r = sb.mind(&["auto-update", "run"]);
    chmod("u+w", &sb.default_home);
    assert!(r.ok(), "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("auto-update state not saved"),
        "{}",
        r.stderr
    );
}

/// `absorb` into a source in an extra home keeps its backup in that home, so it
/// works with a read-only default home.
#[test]
fn absorb_into_an_extra_home_source_with_a_read_only_default_home() {
    // spec: HOME-14
    let sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    // An item placed by hand in the lobe.
    write(
        &sb.claude_home.join("skills/handmade/SKILL.md"),
        "---\ndescription: handmade\n---\nbody\n",
    );
    std::fs::write(sb.default_home.join(".lock"), "").unwrap();
    chmod("a-w", &sb.default_home);
    if std::fs::write(sb.default_home.join("probe"), "").is_ok() {
        // Running as root: permissions are not enforced, nothing to check.
        return;
    }
    let r = sb.mind(&["--yes", "absorb", "skill:handmade", "--to", &sb.spec()]);
    chmod("u+w", &sb.default_home);
    assert!(r.ok(), "absorb: {}\n{}", r.stdout, r.stderr);
    let mut items = sb.items_in(&sb.extra);
    items.sort();
    assert_eq!(items, ["skill:alpha", "skill:handmade"]);
    assert!(sb.source.join("skills/handmade/SKILL.md").is_file());
    assert!(!sb.default_home.join(".tmp").exists());
}

/// A default home that does not exist and cannot be created (its parent is
/// read-only) does not stop a meld into an extra home: its lock is skipped and
/// nothing is written there.
#[test]
fn an_uncreatable_default_home_does_not_block_an_extra_home() {
    // spec: HOME-9
    let mut sb = Sandbox::new(None);
    let parent = sb.base.join("ro");
    std::fs::create_dir_all(&parent).unwrap();
    sb.default_home = parent.join("default");
    chmod("a-w", &parent);
    if std::fs::create_dir(parent.join("probe")).is_ok() {
        // Running as root: permissions are not enforced, nothing to check.
        return;
    }

    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
    assert!(!sb.default_home.exists());
    let r = sb.mind(&["recall", "--json"]);
    assert!(r.ok(), "recall: {}", r.stderr);
    assert!(r.stdout.contains("skill:alpha"), "{}", r.stdout);
}

/// A source and item present in two homes are read from the first, and a save
/// keeps the shadowed copies in the later home's files rather than dropping
/// them: forgetting the first copy brings the later one into view.
#[test]
fn a_shadowed_entry_survives_a_save() {
    // spec: HOME-3
    let mut sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    // The same source melded into the default home while the extra one is
    // not listed.
    sb.env_lists_extra = false;
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--overwrite"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    sb.env_lists_extra = true;

    let r = sb.mind(&["--yes", "sync"]);
    assert!(r.ok(), "sync: {}\n{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("is melded in both"), "{}", r.stderr);
    assert_eq!(sb.sources_in(&sb.extra), [sb.identity()]);

    let r = sb.mind(&["--yes", "forget", "alpha"]);
    assert!(r.ok(), "forget: {}\n{}", r.stdout, r.stderr);
    assert!(sb.items_in(&sb.default_home).is_empty());
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
    let r = sb.mind(&["recall", "--json"]);
    assert!(r.ok(), "recall: {}", r.stderr);
    assert!(r.stdout.contains("skill:alpha"), "{}", r.stdout);
}

/// An item kept by `unmeld --keep-items` stays where it is when its source is
/// melded into another home, and moves there on its next upgrade: the old link
/// into the default home's store is mind's own, so the upgrade replaces it,
/// and the old store copy is removed.
#[test]
fn a_kept_item_moves_to_its_source_home_on_upgrade() {
    // spec: HOME-6 HOME-8
    let sb = Sandbox::new(None);
    let r = sb.mind(&["--yes", "meld", &sb.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["--yes", "unmeld", &sb.identity(), "--keep-items"]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);

    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.items_in(&sb.default_home), ["skill:alpha"]);

    write(
        &sb.source.join("skills/alpha/SKILL.md"),
        "---\ndescription: alpha\n---\nbody 2\n",
    );
    git(&sb.source, &["commit", "-qam", "two"]);
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
    assert!(sb.items_in(&sb.default_home).is_empty());
    assert!(
        sb.alpha_target()
            .starts_with(sb.canon(&sb.extra).join("store"))
    );
    assert!(!sb.default_home.join("store/skill/alpha").exists());
}

/// Without `--home`, a source's `[source] mind-home` picks its home, and
/// `--home` outranks it.
#[test]
fn the_source_names_its_home_and_the_flag_outranks_it() {
    // spec: HOME-4 HOME-5 HOME-8
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let extra = std::env::temp_dir().join(format!("mind-homes-x-{}-{n}", std::process::id()));
    let toml = format!("[source]\nmind-home = \"{}\"\n", extra.to_string_lossy());
    let mut sb = Sandbox::new(Some(&toml));
    let _ = std::fs::remove_dir_all(&sb.extra);
    sb.extra = extra;

    let r = sb.mind(&["--yes", "meld", &sb.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.extra).len(), 1);
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert!(
        sb.alpha_target()
            .starts_with(sb.canon(&sb.extra).join("store"))
    );

    // `--home` naming the same home on a re-meld is fine; another is refused.
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &default]);
    assert_eq!(r.code, Some(1), "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("already melded in mind home"),
        "{}",
        r.stderr
    );

    let r = sb.mind(&["--yes", "unmeld", &sb.identity()]);
    assert!(r.ok(), "unmeld: {}", r.stderr);
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &default]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.default_home).len(), 1);
    assert!(sb.sources_in(&sb.extra).is_empty());
    let _ = std::fs::remove_dir_all(&sb.extra);
}

/// A home that is not `~/.mind` or a listed extra is refused, from the flag
/// or from the source, and the refused meld registers nothing.
#[test]
fn an_unlisted_home_is_refused() {
    // spec: HOME-4 HOME-5
    let sb = Sandbox::new(Some("[source]\nmind-home = \"/nowhere/listed\"\n"));
    let r = sb.mind(&["--yes", "meld", &sb.spec()]);
    assert_eq!(r.code, Some(1), "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("mind home '/nowhere/listed'")
            && r.stderr.contains("is not allowed")
            && r.stderr.contains("MIND_HOMES"),
        "{}",
        r.stderr
    );
    assert!(sb.sources_in(&sb.default_home).is_empty());

    let r = sb.mind(&[
        "--json",
        "--yes",
        "meld",
        &sb.spec(),
        "--home",
        "/also/unlisted",
    ]);
    assert_eq!(r.code, Some(1));
    let v: Value = serde_json::from_str(&r.stdout).expect("a JSON error envelope");
    assert_eq!(v["error"]["kind"], "home-not-allowed", "{v}");
}

/// A curated source with no `mind-home` of its own goes into its curator's
/// home.
#[test]
fn curated_sources_follow_the_curator_home() {
    // spec: HOME-5 HOME-6
    let nested = Sandbox::new(None);
    let curator = Sandbox::new(Some(&format!(
        "[discover]\nsources = [{{ source = \"{}\" }}]\n",
        nested.spec()
    )));
    let extra = curator.extra_str();
    let r = curator.mind(&["--yes", "meld", &curator.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let in_extra = curator.sources_in(&curator.extra);
    assert_eq!(in_extra.len(), 2, "{in_extra:?}");
    assert!(in_extra.contains(&nested.identity()), "{in_extra:?}");
    assert!(curator.sources_in(&curator.default_home).is_empty());
}

/// A source a curator in an extra home lists only after its meld still lands
/// in the curator's home, whether `sync`'s re-walk or `curate` registers it.
#[test]
fn later_curated_sources_follow_the_curator_home() {
    // spec: HOME-5 HOME-6
    let by_sync = Sandbox::new(None);
    let by_curate = Sandbox::new(None);
    let curator = Sandbox::new(Some("[source]\ndescription = \"curator\"\n"));
    let extra = curator.extra_str();
    let r = curator.mind(&["--yes", "meld", &curator.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(curator.sources_in(&curator.extra).len(), 1);

    let list = |specs: &[String]| {
        let entries: Vec<String> = specs
            .iter()
            .map(|s| format!("{{ source = \"{s}\" }}"))
            .collect();
        write(
            &curator.source.join("mind.toml"),
            &format!(
                "[source]\ndescription = \"curator\"\n[discover]\nsources = [{}]\n",
                entries.join(", ")
            ),
        );
        git(&curator.source, &["commit", "-qam", "list"]);
    };

    list(&[by_sync.spec()]);
    let r = curator.mind(&["--yes", "sync"]);
    assert!(r.ok(), "sync: {}\n{}", r.stdout, r.stderr);
    let in_extra = curator.sources_in(&curator.extra);
    assert!(in_extra.contains(&by_sync.identity()), "{in_extra:?}");

    list(&[by_sync.spec(), by_curate.spec()]);
    let r = curator.mind(&["--yes", "curate"]);
    assert!(r.ok(), "curate: {}\n{}", r.stdout, r.stderr);
    let in_extra = curator.sources_in(&curator.extra);
    assert!(in_extra.contains(&by_curate.identity()), "{in_extra:?}");
    assert!(
        curator.sources_in(&curator.default_home).is_empty(),
        "{:?}",
        curator.sources_in(&curator.default_home)
    );
}

/// The `mind-homes` config key lists extra homes like `MIND_HOMES`.
#[test]
fn the_config_key_lists_extra_homes() {
    // spec: HOME-2
    let mut sb = Sandbox::new(None);
    sb.env_lists_extra = false;
    let r = sb.mind(&["config", "show"]);
    assert!(r.ok(), "{}", r.stderr);
    let cfg = sb.default_home.join("config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str(&format!("mind-homes = [\"{}\"]\n", sb.extra_str()));
    std::fs::write(&cfg, text).unwrap();

    let extra = sb.extra_str();
    let r = sb.mind(&["--json", "config", "show"]);
    assert!(r.ok(), "{}", r.stderr);
    let v: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(v["mind_homes"], serde_json::json!([extra]), "{v}");
    let r = sb.mind(&["config", "show"]);
    assert!(
        r.stdout.contains(&format!("mind-homes = [{extra:?}]")),
        "{}",
        r.stdout
    );

    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.extra).len(), 1);
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
}

/// `recall --sources` names an extra home in its text and gives every source's
/// home under `--json`.
#[test]
fn recall_sources_shows_the_home() {
    // spec: HOME-11
    let sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.ok(), "{}", r.stderr);
    assert!(r.stdout.contains(&format!("home:{extra}")), "{}", r.stdout);
    let r = sb.mind(&["--json", "recall", "--sources"]);
    assert!(r.ok(), "{}", r.stderr);
    let v: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(v["items"][0]["mind_home"], serde_json::json!(extra), "{v}");

    // A source in the default home names the default home.
    let other = Sandbox::new(None);
    let r = other.mind(&["--yes", "meld", &other.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let r = other.mind(&["recall", "--sources"]);
    assert!(!r.stdout.contains("home:"), "{}", r.stdout);
    let r = other.mind(&["--json", "recall", "--sources"]);
    let v: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(
        v["items"][0]["mind_home"],
        serde_json::json!(other.default_home.to_string_lossy()),
        "{v}"
    );
}

/// `dump` records an extra home as the entry's `mind-home`, and melding the
/// dump puts the source back in that home.
#[test]
fn dump_records_the_home_and_remelding_restores_it() {
    // spec: HOME-10
    let sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);

    let super_dir = sb.base.join("super");
    std::fs::create_dir_all(&super_dir).unwrap();
    let dump = super_dir.join("mind.toml");
    let r = sb.mind(&["dump", "--output", &dump.to_string_lossy()]);
    assert!(r.ok(), "dump: {}", r.stderr);
    let text = std::fs::read_to_string(&dump).unwrap();
    assert!(text.contains(&format!("mind-home = {extra:?}")), "{text}");
    // A home under the user's home directory is written with `~`.
    let r = sb.mind_env(
        &std::env::current_dir().unwrap(),
        &[("HOME", &sb.base)],
        &["dump"],
    );
    assert!(r.ok(), "dump: {}", r.stderr);
    assert!(r.stdout.contains("mind-home = \"~/extra\""), "{}", r.stdout);
    git(&super_dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&super_dir, &["config", "user.email", "t@t"]);
    git(&super_dir, &["config", "user.name", "t"]);
    git(&super_dir, &["add", "-A"]);
    git(&super_dir, &["commit", "-qm", "dump"]);

    let r = sb.mind(&["--yes", "unmeld", &sb.identity()]);
    assert!(r.ok(), "unmeld: {}\n{}", r.stdout, r.stderr);
    assert!(sb.sources_in(&sb.extra).is_empty());

    let r = sb.mind(&["--yes", "meld", &super_dir.to_string_lossy()]);
    assert!(r.ok(), "meld dump: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.extra), [sb.identity()]);
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
}

/// A curator entry's `mind-home` must be an allowed home.
#[test]
fn a_curator_entry_naming_an_unlisted_home_is_refused() {
    // spec: HOME-10
    let nested = Sandbox::new(None);
    let curator = Sandbox::new(Some(&format!(
        "[[discover.sources]]\nsource = {:?}\ninstall = true\nmind-home = {:?}\n",
        nested.spec(),
        nested.base.join("unlisted").to_string_lossy()
    )));
    let r = curator.mind(&["--yes", "meld", &curator.spec()]);
    assert!(r.ok(), "{}\n{}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("is not allowed") && !r.stderr.contains("clone failed"),
        "{}\n{}",
        r.stdout,
        r.stderr
    );
    let r = curator.mind(&["--yes", "--json", "sync"]);
    assert!(r.ok(), "{}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("home_not_allowed"), "{}", r.stdout);
    assert!(
        !curator
            .sources_in(&curator.default_home)
            .contains(&nested.identity())
    );
}

/// `learn <url> --home` registers the item link in that home; for a plain item
/// ref it is ignored with a note.
#[test]
fn learn_link_takes_home() {
    // spec: HOME-12
    let sb = Sandbox::new(None);
    let extra = sb.extra_str();
    let url = format!("file://{}/tree/main/skills/alpha", sb.spec());
    let r = sb.mind(&["--yes", "learn", &url, "--home", &extra]);
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        sb.sources_in(&sb.extra).len(),
        1,
        "{:?}",
        sb.sources_in(&sb.extra)
    );
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);

    let r = sb.mind(&["--yes", "learn", "alpha", "--home", &extra]);
    assert!(r.stderr.contains("--home is ignored"), "{}", r.stderr);

    let r = sb.mind(&["--yes", "learn", &url, "--home", "/nowhere"]);
    assert!(!r.ok());
    assert!(r.stderr.contains("is not allowed"), "{}", r.stderr);

    // spec: HOME-8 -- the instance is in the extra home already.
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "learn", &url, "--home", &default]);
    assert!(!r.ok());
    assert!(
        r.stderr.contains("already melded in mind home"),
        "{}",
        r.stderr
    );
}

/// A relative `mind-homes` entry is relative to the default home, not the
/// working directory, and an empty entry is ignored.
#[test]
fn a_relative_config_home_is_relative_to_the_default_home() {
    // spec: HOME-2
    let mut sb = Sandbox::new(None);
    sb.env_lists_extra = false;
    let r = sb.mind(&["config", "show"]);
    assert!(r.ok(), "{}", r.stderr);
    let cfg = sb.default_home.join("config.toml");
    let mut text = std::fs::read_to_string(&cfg).unwrap();
    text.push_str("mind-homes = [\"../extra\", \"\"]\n");
    std::fs::write(&cfg, text).unwrap();

    let extra = sb.extra_str();
    for cwd in [sb.base.clone(), sb.source.join("skills/alpha")] {
        let r = sb.mind_in(&cwd, &["--json", "config", "show"]);
        assert!(r.ok(), "{}", r.stderr);
        let v: Value = serde_json::from_str(&r.stdout).unwrap();
        assert_eq!(
            v["mind_homes"],
            serde_json::json!([extra]),
            "from {cwd:?}: {v}"
        );
    }
}
