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

    // `--home` naming the same home on a re-meld is fine; another moves it,
    // even against the source's own `mind-home`.
    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("moved"), "{}", r.stdout);
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &default]);
    assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.default_home).len(), 1);
    assert!(sb.sources_in(&sb.extra).is_empty());
    let _ = std::fs::remove_dir_all(&sb.extra);
}

/// `meld --home` on a melded source moves its clone, registry entry, store
/// copies, and links to that home, and back again; running it again changes
/// nothing.
#[test]
fn meld_home_moves_a_melded_source() {
    // spec: HOME-8
    let sb = Sandbox::new(None);
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--pin", "HEAD"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let clone_rel = PathBuf::from("sources/local")
        .join(sb.base.file_name().unwrap())
        .join("src");
    assert!(sb.default_home.join(&clone_rel).is_dir());

    let extra = sb.extra_str();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("moved"), "{}", r.stdout);
    assert!(r.stdout.contains("1 item to move"), "{}", r.stdout);
    assert_eq!(sb.sources_in(&sb.extra), [sb.identity()]);
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);
    assert!(sb.items_in(&sb.default_home).is_empty());
    assert!(sb.extra.join(&clone_rel).is_dir());
    assert!(!sb.default_home.join(&clone_rel).exists());
    assert!(!sb.default_home.join("store/skill/alpha").exists());
    assert!(
        sb.alpha_target()
            .starts_with(sb.canon(&sb.extra).join("store"))
    );

    // Idempotent.
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
    assert!(r.ok(), "again: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("moved"), "{}", r.stdout);
    assert!(!r.stdout.contains("to move"), "{}", r.stdout);
    assert_eq!(sb.items_in(&sb.extra), ["skill:alpha"]);

    // And back.
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &default]);
    assert!(r.ok(), "back: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&sb.default_home), [sb.identity()]);
    assert!(sb.sources_in(&sb.extra).is_empty());
    assert_eq!(sb.items_in(&sb.default_home), ["skill:alpha"]);
    assert!(sb.items_in(&sb.extra).is_empty());
    assert!(sb.default_home.join(&clone_rel).is_dir());
    assert!(!sb.extra.join(&clone_rel).exists());
    assert!(
        sb.alpha_target()
            .starts_with(sb.canon(&sb.default_home).join("store"))
    );
    let r = sb.mind(&["introspect"]);
    assert!(r.ok(), "introspect: {}\n{}", r.stdout, r.stderr);
}

/// Moving a curator moves the sources it curates that follow its home, and
/// leaves one that names its own home where it is.
#[test]
fn moving_a_curator_moves_the_sources_that_follow_it() {
    // spec: HOME-8
    let follows = Sandbox::new(None);
    let pinned = Sandbox::new(None);
    let curator = Sandbox::new(None);
    let default = curator.default_home.to_string_lossy().into_owned();
    write(
        &curator.source.join("mind.toml"),
        &format!(
            "[[discover.sources]]\nsource = {:?}\ninstall = true\n\n[[discover.sources]]\nsource = {:?}\ninstall = true\nmind-home = {:?}\n",
            follows.spec(),
            pinned.spec(),
            default
        ),
    );
    git(&curator.source, &["add", "-A"]);
    git(&curator.source, &["commit", "-qm", "curate"]);
    let r = curator.mind(&["--yes", "meld", &curator.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(curator.sources_in(&curator.default_home).len(), 3);

    let extra = curator.extra_str();
    let r = curator.mind(&["--yes", "meld", &curator.spec(), "--home", &extra]);
    assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
    let mut in_extra = curator.sources_in(&curator.extra);
    in_extra.sort();
    let mut want = vec![curator.identity(), follows.identity()];
    want.sort();
    assert_eq!(in_extra, want);
    assert_eq!(
        curator.sources_in(&curator.default_home),
        [pinned.identity()]
    );
}

/// A move refused for one family member (a shadowed copy of a follower in the
/// target home) moves nothing: the curator stays where it was too.
#[test]
fn a_refused_follower_moves_no_one() {
    // spec: HOME-8
    let follows = Sandbox::new(None);
    let curator = Sandbox::new(None);
    write(
        &curator.source.join("mind.toml"),
        &format!(
            "[[discover.sources]]\nsource = {:?}\ninstall = true\n",
            follows.spec()
        ),
    );
    git(&curator.source, &["add", "-A"]);
    git(&curator.source, &["commit", "-qm", "curate"]);
    let r = curator.mind(&["--yes", "meld", &curator.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);

    // A shadowed copy of the follower's entry in the extra home.
    let text = std::fs::read_to_string(curator.default_home.join("sources.json")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let follower = follows.identity();
    v["sources"]
        .as_array_mut()
        .unwrap()
        .retain(|s| s["name"] == serde_json::json!(follower));
    write(
        &curator.extra.join("sources.json"),
        &serde_json::to_string(&v).unwrap(),
    );

    let extra = curator.extra_str();
    let r = curator.mind(&["--yes", "meld", &curator.spec(), "--home", &extra]);
    assert!(!r.ok(), "move should be refused: {}", r.stdout);
    assert!(r.stderr.contains("shadowed copy"), "{}", r.stderr);
    assert_eq!(curator.sources_in(&curator.extra), [follower]);
    assert_eq!(curator.sources_in(&curator.default_home).len(), 2);
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

    // spec: HOME-8 -- naming another home moves the instance there.
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "learn", &url, "--home", &default]);
    assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
    assert!(sb.sources_in(&sb.extra).is_empty());
    assert_eq!(sb.sources_in(&sb.default_home).len(), 1);
    assert_eq!(sb.items_in(&sb.default_home), ["skill:alpha"]);
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

/// A move reinstalls every item of the source in the new home, so the path
/// tokens in its markdown (`{{tools:}}`, `{{path:}}`, `{{self}}`,
/// `{{resources:}}`) name the new store, and the shared tool and resource
/// they reach are there. Both moves: `meld --home`, and `upgrade` following
/// a new `[source] mind-home`.
#[test]
fn a_move_rewrites_path_tokens_to_the_new_store() {
    // spec: HOME-8 HOME-15
    for by_upgrade in [false, true] {
        let sb = Sandbox::new(None);
        write(&sb.source.join("tools/detect/detect"), "#!/bin/sh\n");
        write(&sb.source.join("resources/shared.md"), "shared\n");
        write(
            &sb.source.join("skills/alpha/SKILL.md"),
            "---\ndescription: alpha\n---\nrun {{tools:detect}}\nlib {{path:tool:detect}}/x\nself {{self}}\nread {{resources:shared.md}}\n",
        );
        git(&sb.source, &["add", "-A"]);
        git(&sb.source, &["commit", "-qm", "tokens"]);
        let r = sb.mind(&["--yes", "meld", &sb.spec()]);
        assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
        let old = sb.default_home.join("store");
        let skill = std::fs::read_to_string(old.join("skill/alpha/SKILL.md")).unwrap();
        assert!(
            skill.contains(&old.to_string_lossy().into_owned()),
            "{skill}"
        );

        if by_upgrade {
            write(
                &sb.source.join("mind.toml"),
                &format!("[source]\nmind-home = {:?}\n", sb.extra_str()),
            );
            git(&sb.source, &["add", "-A"]);
            git(&sb.source, &["commit", "-qm", "own home"]);
            let r = sb.mind(&["--yes", "upgrade"]);
            assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
        } else {
            let extra = sb.extra_str();
            let r = sb.mind(&["--yes", "meld", &sb.spec(), "--home", &extra]);
            assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
        }

        let new = sb.extra.join("store");
        let skill = std::fs::read_to_string(new.join("skill/alpha/SKILL.md")).unwrap();
        let new_str = new.to_string_lossy().into_owned();
        for line in [
            format!("run {new_str}/tool/detect/detect"),
            format!("lib {new_str}/tool/detect/x"),
            format!("self {new_str}/skill/alpha"),
            format!("read {new_str}/resource/shared.md"),
        ] {
            assert!(
                skill.contains(&line),
                "by upgrade: {by_upgrade}: {line}\n{skill}"
            );
        }
        assert!(
            !skill.contains(&old.to_string_lossy().into_owned()),
            "by upgrade: {by_upgrade}: {skill}"
        );
        assert!(new.join("tool/detect/detect").exists());
        assert!(new.join("resource/shared.md").exists());
        assert!(!old.join("tool/detect").exists());
        assert!(!old.join("resource/shared.md").exists());
        let mut items = sb.items_in(&sb.extra);
        items.sort();
        assert_eq!(items, ["resource:shared.md", "skill:alpha", "tool:detect"]);
    }
}

/// A tool with a build hook is built again by a move; where that hook cannot
/// run, `meld --home` is refused and `upgrade` leaves the source in place,
/// and `--accept-hooks=build` lets either move it with the tool built.
#[test]
fn a_move_that_would_skip_a_build_does_not_happen() {
    // spec: HOME-16
    let items = "[[items]]\nkind = \"skill\"\nname = \"alpha\"\npath = \"skills/alpha\"\n\n[[items]]\nkind = \"tool\"\nname = \"mytool\"\npath = \"tools/mytool\"\nbuild = \"touch built.txt\"\n";
    for by_upgrade in [false, true] {
        let sb = Sandbox::new(Some(items));
        write(
            &sb.source.join("tools/mytool/TOOL.md"),
            "---\ndescription: t\n---\n",
        );
        git(&sb.source, &["add", "-A"]);
        git(&sb.source, &["commit", "-qm", "tool"]);
        let r = sb.mind(&["--yes", "meld", &sb.spec(), "--accept-hooks=build"]);
        assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
        assert!(sb.default_home.join("store/tool/mytool/built.txt").exists());

        let extra = sb.extra_str();
        let move_args: Vec<&str> = if by_upgrade {
            write(
                &sb.source.join("mind.toml"),
                &format!("[source]\nmind-home = {extra:?}\n\n{items}"),
            );
            git(&sb.source, &["add", "-A"]);
            git(&sb.source, &["commit", "-qm", "own home"]);
            vec!["--yes", "upgrade"]
        } else {
            vec![
                "--yes",
                "meld",
                &sb.source.to_str().unwrap(),
                "--home",
                &extra,
            ]
        };
        let r = sb.mind(&move_args);
        let all = format!("{}{}", r.stdout, r.stderr);
        assert_eq!(r.ok(), by_upgrade, "by upgrade: {by_upgrade}: {all}");
        assert!(all.contains("without its build hook"), "{all}");
        assert_eq!(sb.sources_in(&sb.default_home), [sb.identity()]);
        assert!(sb.default_home.join("store/tool/mytool/built.txt").exists());

        let mut accepted = move_args.clone();
        accepted.push("--accept-hooks=build");
        let r = sb.mind(&accepted);
        assert!(r.ok(), "accepted: {}\n{}", r.stdout, r.stderr);
        assert_eq!(sb.sources_in(&sb.extra), [sb.identity()]);
        assert!(sb.extra.join("store/tool/mytool/built.txt").exists());
        assert!(!sb.default_home.join("store/tool/mytool").exists());
    }
}

fn config_json(sb: &Sandbox) -> Value {
    let r = sb.mind(&["--json", "config", "show"]);
    assert!(r.ok(), "{}", r.stderr);
    serde_json::from_str(&r.stdout).unwrap()
}

/// With no extra homes set, a source's own `mind-home` is taken, recorded under
/// `source-homes` so later runs read it, and called out; its lobe stays
/// unrestricted. Once the user sets `mind-homes`, only that list is read.
#[test]
fn a_source_home_is_taken_and_recorded_with_no_extra_homes_set() {
    // spec: HOME-17 HOME-5 LOBE-2
    let home = std::env::temp_dir().join(format!(
        "mind-homes-{}-{}-own",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let mut sb = Sandbox::new(Some(&format!(
        "[source]\nmind-home = {:?}\nlobe = {:?}\n",
        home.join("mind").to_string_lossy(),
        home.join("lobe").to_string_lossy()
    )));
    sb.env_lists_extra = false;
    let own = home.join("mind");
    let r = sb.mind(&["--yes", "meld", &sb.spec()]);
    let _cleanup = Cleanup(home.clone());
    let all = format!("{}{}", r.stdout, r.stderr);
    assert!(r.ok(), "meld: {all}");
    assert!(
        all.contains("a new mind home, recorded under source-homes"),
        "{all}"
    );
    assert_eq!(sb.sources_in(&own), [sb.identity()]);
    assert_eq!(sb.items_in(&own), ["skill:alpha"]);
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert!(home.join("lobe/skills/alpha").exists());
    let v = config_json(&sb);
    assert_eq!(
        v["source_homes"],
        serde_json::json!([own.to_string_lossy()]),
        "{v}"
    );
    assert_eq!(v["mind_homes"], v["source_homes"], "{v}");
    assert_eq!(v["homes_restricted"], false, "{v}");

    // A later run reads the recorded home, and records it once.
    let r = sb.mind(&["recall", "--sources"]);
    assert!(r.stdout.contains(&sb.identity()), "{}", r.stdout);
    let r = sb.mind(&["--yes", "meld", &sb.spec()]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stderr.contains("a new mind home"), "{}", r.stderr);
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        config_json(&sb)["source_homes"].as_array().unwrap().len(),
        1
    );

    // The user's own list replaces the recorded homes: the source drops out
    // of view, and an unlisted home or lobe is refused again.
    sb.env_lists_extra = true;
    let r = sb.mind(&["recall", "--sources"]);
    assert!(!r.stdout.contains(&sb.identity()), "{}", r.stdout);
    let r = sb.mind(&["config", "show"]);
    assert!(
        r.stdout
            .contains("not read while mind-homes or MIND_HOMES is set"),
        "{}",
        r.stdout
    );
    let other = Sandbox::new(Some(&format!(
        "[source]\nmind-home = {:?}\n",
        home.join("other").to_string_lossy()
    )));
    let r = other.mind(&["--yes", "meld", &other.spec()]);
    assert_eq!(r.code, Some(1), "{}\n{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("MIND_HOMES"), "{}", r.stderr);
}

/// `meld --home` takes an unlisted home with none set, and `upgrade` follows a
/// `mind-home` a new commit adds into a home not yet recorded.
#[test]
fn an_unlisted_home_is_taken_by_the_flag_and_by_upgrade() {
    // spec: HOME-17 HOME-4 HOME-15
    let mut sb = Sandbox::new(None);
    sb.env_lists_extra = false;
    let flag_home = sb.base.join("flag-home");
    let r = sb.mind(&[
        "--yes",
        "meld",
        &sb.spec(),
        "--home",
        &flag_home.to_string_lossy(),
    ]);
    assert!(r.ok(), "meld --home: {}\n{}", r.stdout, r.stderr);
    assert_eq!(sb.sources_in(&flag_home), [sb.identity()]);

    let mut moved = Sandbox::new(None);
    moved.env_lists_extra = false;
    let r = moved.mind(&["--yes", "meld", &moved.spec()]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let declared = moved.base.join("declared");
    write(
        &moved.source.join("mind.toml"),
        &format!("[source]\nmind-home = {:?}\n", declared.to_string_lossy()),
    );
    git(&moved.source, &["add", "-A"]);
    git(&moved.source, &["commit", "-qm", "own home"]);
    let r = moved.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert_eq!(moved.sources_in(&declared), [moved.identity()]);
    assert_eq!(moved.items_in(&declared), ["skill:alpha"]);
    assert!(moved.sources_in(&moved.default_home).is_empty());
    let r = moved.mind(&["recall"]);
    assert!(r.stdout.contains("alpha"), "{}", r.stdout);
}

/// An unlisted home is still refused when it is inside a home's store,
/// sources, or .tmp, is under a lobe, or is a directory holding something
/// other than a mind tree.
#[test]
fn a_new_home_must_be_free() {
    // spec: HOME-17
    let mut sb = Sandbox::new(None);
    sb.env_lists_extra = false;
    let store = sb.default_home.join("store/x");
    for (home, why) in [
        (sb.base.clone(), "not a mind home"),
        (store, "store, sources, or .tmp"),
        // The default lobe is empty here, so only the lobe rule refuses it.
        (sb.claude_home.join("sub"), "is a lobe or lies under one"),
    ] {
        let r = sb.mind(&[
            "--yes",
            "meld",
            &sb.spec(),
            "--home",
            &home.to_string_lossy(),
        ]);
        assert_eq!(r.code, Some(1), "{}\n{}", r.stdout, r.stderr);
        assert!(
            r.stderr.contains("is not allowed") && r.stderr.contains(why),
            "{}",
            r.stderr
        );
    }
    assert!(sb.sources_in(&sb.default_home).is_empty());
    assert!(
        config_json(&sb)["source_homes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// Every configured lobe gets a source's links whatever home the source is in:
/// melded into its own `mind-home`, listed or not, and moved there by
/// `upgrade`.
#[test]
fn configured_lobes_are_linked_for_a_source_in_another_home() {
    // spec: HOME-6
    for (listed, via_upgrade) in [(true, false), (false, false), (false, true)] {
        let home = std::env::temp_dir().join(format!(
            "mind-homes-{}-{}-lobes",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _cleanup = Cleanup(home.clone());
        let own = if listed {
            None
        } else {
            Some(home.join("mind"))
        };
        let toml = own
            .as_ref()
            .map(|h| format!("[source]\nmind-home = {:?}\n", h.to_string_lossy()));
        let mut sb = Sandbox::new(if via_upgrade { None } else { toml.as_deref() });
        sb.env_lists_extra = listed;
        let second = sb.base.join("second-lobe");
        write(
            &sb.default_home.join("config.toml"),
            &format!(
                "lobes = [{:?}, {:?}]\n",
                sb.claude_home.to_string_lossy(),
                second.to_string_lossy()
            ),
        );
        let target = own.clone().unwrap_or_else(|| sb.extra.clone());
        let r = if listed {
            sb.mind(&["--yes", "meld", &sb.spec(), "--home", &sb.extra_str()])
        } else if via_upgrade {
            let r = sb.mind(&["--yes", "meld", &sb.spec()]);
            assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
            write(&sb.source.join("mind.toml"), toml.as_deref().unwrap());
            git(&sb.source, &["add", "-A"]);
            git(&sb.source, &["commit", "-qm", "own home"]);
            sb.mind(&["--yes", "upgrade"])
        } else {
            sb.mind(&["--yes", "meld", &sb.spec()])
        };
        let case = format!("listed={listed} via_upgrade={via_upgrade}");
        assert!(r.ok(), "{case}: {}\n{}", r.stdout, r.stderr);
        assert_eq!(sb.items_in(&target), ["skill:alpha"], "{case}");
        for lobe in [&sb.claude_home, &second] {
            let link = lobe.join("skills/alpha");
            let resolved = std::fs::canonicalize(&link)
                .unwrap_or_else(|_| panic!("{case}: {} is not linked", link.display()));
            assert!(
                resolved.starts_with(sb.canon(&target).join("store")),
                "{case}: {} -> {}",
                link.display(),
                resolved.display()
            );
        }

        // A lobe added later is backfilled, a removed link is repaired, and
        // forget removes every link.
        let third = sb.base.join("third-lobe");
        let r = sb.mind(&["config", "lobes", "add", "--yes", &third.to_string_lossy()]);
        assert!(r.ok(), "{case}: lobes add: {}\n{}", r.stdout, r.stderr);
        assert!(
            third.join("skills/alpha").exists(),
            "{case}: backfill: {}\n{}",
            r.stdout,
            r.stderr
        );
        std::fs::remove_file(second.join("skills/alpha")).unwrap();
        let r = sb.mind(&["introspect", "--fix"]);
        assert!(r.ok(), "{case}: fix: {}\n{}", r.stdout, r.stderr);
        assert!(second.join("skills/alpha").exists(), "{case}: fix");
        let r = sb.mind(&["--yes", "forget", "alpha"]);
        assert!(r.ok(), "{case}: forget: {}\n{}", r.stdout, r.stderr);
        for lobe in [&sb.claude_home, &second, &third] {
            assert!(
                std::fs::symlink_metadata(lobe.join("skills/alpha")).is_err(),
                "{case}: forget left {}",
                lobe.display()
            );
        }
        let r = sb.mind(&["--yes", "learn", "alpha"]);
        assert!(r.ok(), "{case}: learn: {}\n{}", r.stdout, r.stderr);
        for lobe in [&sb.claude_home, &second, &third] {
            assert!(
                lobe.join("skills/alpha").exists(),
                "{case}: learn missed {}",
                lobe.display()
            );
        }
    }
}

/// Removes a directory outside the sandbox base when the test ends.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
