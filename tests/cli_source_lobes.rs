//! Integration tests for a source's own lobe (spec/source-lobes.md): the
//! `[source] lobe` a source's `mind.toml` declares under a mind home, which
//! replaces the default lobe for its items, and the skip of a lobe that cannot
//! be written.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture: a local git repo melded by path into an extra mind home, with
//! `MIND_HOME`/`MIND_HOMES`/`CLAUDE_HOME` pointed at temp dirs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    default_home: PathBuf,
    extra: PathBuf,
    /// The default lobe (`CLAUDE_HOME`).
    claude_home: PathBuf,
    /// Whether `MIND_HOMES` lists `extra` (no extra homes otherwise).
    extra_homes: bool,
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
        let base = std::env::temp_dir().join(format!("mind-lobes-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sb = Sandbox {
            source: base.join("src"),
            default_home: base.join("default"),
            extra: base.join("extra"),
            claude_home: base.join("claude"),
            extra_homes: true,
            base,
        };
        std::fs::create_dir_all(&sb.claude_home).unwrap();
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
        sb.commit("initial");
        sb
    }

    fn commit(&self, msg: &str) {
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", msg]);
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
            .current_dir(cwd)
            .env("MIND_HOME", &self.default_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_HOMES")
            .env_remove("MIND_AGENT_HOMES")
            .env_remove("MIND_DEFAULT_LOBE")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if self.extra_homes {
            cmd.env("MIND_HOMES", &self.extra);
        }
        cmd.envs(env.iter().copied());
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        }
    }

    fn meld(&self) -> Run {
        let extra = self.extra.to_string_lossy().into_owned();
        let spec = self.source.to_string_lossy().into_owned();
        self.mind(&["--yes", "meld", &spec, "--home", &extra])
    }

    /// The source's own lobe for `lobe = "claude"`.
    fn own_lobe(&self) -> PathBuf {
        self.extra.join("claude")
    }

    fn links_of(&self, key: &str) -> Vec<String> {
        let text = std::fs::read_to_string(self.extra.join("manifest.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        v["items"][key]["links"]
            .as_array()
            .map(|a| a.iter().map(|l| l.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    }

    fn sources_in(&self, home: &Path) -> usize {
        std::fs::read_to_string(home.join("sources.json"))
            .ok()
            .map(|t| {
                let v: Value = serde_json::from_str(&t).unwrap();
                v["sources"].as_array().unwrap().len()
            })
            .unwrap_or(0)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
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

/// Whether permissions are enforced here (not when running as root).
fn enforced(dir: &Path) -> bool {
    std::fs::write(dir.join(".probe"), "").is_err()
}

fn is_link_into(link: &Path, root: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|t| t.starts_with(root))
}

/// A source's `lobe` takes the default lobe's place for its items while every
/// other configured lobe still gets a link; `introspect` sees no drift, and
/// `forget` removes the link from the source's lobe.
#[test]
fn source_lobe_replaces_the_default_lobe() {
    // spec: LOBE-1 LOBE-2 LOBE-3
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    let project = sb.base.join("project/.devin");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let r = sb.mind(&["config", "lobes", "add", &project.to_string_lossy()]);
    assert!(r.ok(), "lobes add: {}", r.stderr);

    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let store = sb.extra.join("store");
    assert!(is_link_into(&sb.own_lobe().join("skills/alpha"), &store));
    assert!(is_link_into(&project.join("skills/alpha"), &store));
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("skills/alpha")).is_err(),
        "the default lobe is not linked"
    );

    let r = sb.mind(&["introspect", "--json"]);
    assert!(r.ok(), "introspect: {}\n{}", r.stdout, r.stderr);
    let v: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(
        v["issues"].as_array().map(Vec::len),
        Some(0),
        "{}",
        r.stdout
    );

    let r = sb.mind(&["--yes", "forget", "alpha"]);
    assert!(r.ok(), "forget: {}\n{}", r.stdout, r.stderr);
    assert!(std::fs::symlink_metadata(sb.own_lobe().join("skills/alpha")).is_err());
    assert!(std::fs::symlink_metadata(project.join("skills/alpha")).is_err());
}

/// A `lobe` outside every allowed home, or inside a home's store, refuses the
/// meld before the source is registered.
#[test]
fn lobe_outside_the_allowed_homes_is_refused() {
    // spec: LOBE-2
    let outside = std::env::temp_dir().join("mind-lobes-outside");
    for lobe in [
        outside.to_string_lossy().into_owned(),
        "store/x".to_string(),
    ] {
        let sb = Sandbox::new(Some(&format!("[source]\nlobe = {lobe:?}\n")));
        let extra = sb.extra.to_string_lossy().into_owned();
        let spec = sb.source.to_string_lossy().into_owned();
        let r = sb.mind(&["--json", "--yes", "meld", &spec, "--home", &extra]);
        assert!(!r.ok(), "meld with lobe {lobe}: {}", r.stdout);
        assert!(
            r.stdout.contains("lobe-not-allowed") || r.stderr.contains("lobe-not-allowed"),
            "{}\n{}",
            r.stdout,
            r.stderr
        );
        assert_eq!(sb.sources_in(&sb.extra), 0);
        assert_eq!(sb.sources_in(&sb.default_home), 0);
    }
}

/// Adding a `lobe` in a new commit moves the installed item's links there on
/// `upgrade` with no content change, and dropping it moves them back.
#[test]
fn upgrade_relinks_when_the_lobe_changes() {
    // spec: LOBE-1 LOBE-5
    let sb = Sandbox::new(None);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let default_link = sb.claude_home.join("skills/alpha");
    let own_link = sb.own_lobe().join("skills/alpha");
    assert!(is_link_into(&default_link, &sb.extra.join("store")));

    write(
        &sb.source.join("mind.toml"),
        "[source]\nlobe = \"claude\"\n",
    );
    sb.commit("add lobe");
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("1 item to relink"), "{}", r.stdout);
    assert!(r.stdout.contains("lobe"), "{}", r.stdout);
    assert!(is_link_into(&own_link, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(&default_link).is_err());
    assert_eq!(
        sb.links_of("skill:alpha"),
        [own_link.to_string_lossy().into_owned()]
    );

    // A second pass has nothing to do.
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("relink"), "{}", r.stdout);

    std::fs::remove_file(sb.source.join("mind.toml")).unwrap();
    sb.commit("drop lobe");
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("the configured lobes"), "{}", r.stdout);
    assert!(is_link_into(&default_link, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(&own_link).is_err());
}

/// `--local` links into exactly the project lobe, not the source's lobe.
#[test]
fn local_install_ignores_the_source_lobe() {
    // spec: LOBE-4
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    let project = sb.base.join("project/.devin");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let r = sb.mind(&["config", "lobes", "add", &project.to_string_lossy()]);
    assert!(r.ok(), "lobes add: {}", r.stderr);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let r = sb.mind(&["--yes", "forget", "alpha"]);
    assert!(r.ok(), "forget: {}\n{}", r.stdout, r.stderr);

    let r = sb.mind_in(
        &sb.base.join("project"),
        &["--yes", "learn", "alpha", "--local"],
    );
    assert!(r.ok(), "learn --local: {}\n{}", r.stdout, r.stderr);
    assert!(project.join("skills/alpha").exists());
    assert!(std::fs::symlink_metadata(sb.own_lobe().join("skills/alpha")).is_err());
}

/// With the default lobe read-only (a sandbox), an upgrade still succeeds: a
/// link already pointing at the store copy is left alone, a new item's link
/// there is skipped with a note and made by a later `introspect --fix` once it
/// can be written.
#[test]
fn unwritable_lobe_is_skipped_with_a_note() {
    // spec: LOBE-6
    let sb = Sandbox::new(None);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let skills = sb.claude_home.join("skills");

    write(
        &sb.source.join("skills/alpha/SKILL.md"),
        "---\ndescription: alpha\n---\nchanged\n",
    );
    write(
        &sb.source.join("skills/beta/SKILL.md"),
        "---\ndescription: beta\n---\nbody\n",
    );
    sb.commit("change alpha, add beta");
    chmod("a-w", &skills);
    if !enforced(&skills) {
        return;
    }
    let r = sb.mind(&["--yes", "upgrade"]);
    chmod("u+w", &skills);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("cannot be written"), "{}", r.stdout);
    assert_eq!(
        r.stdout.matches("cannot be written").count(),
        1,
        "{}",
        r.stdout
    );
    assert!(is_link_into(&skills.join("alpha"), &sb.extra.join("store")));
    assert_eq!(sb.links_of("skill:alpha").len(), 1);
    assert!(sb.links_of("skill:beta").is_empty());
    assert!(std::fs::symlink_metadata(skills.join("beta")).is_err());

    let r = sb.mind(&["introspect", "--fix"]);
    assert!(r.ok(), "introspect --fix: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(&skills.join("beta"), &sb.extra.join("store")));
}

/// Moving to a `lobe` while the default lobe is read-only links the new lobe
/// and leaves the old link recorded; the next run that can write there
/// removes it.
#[test]
fn stale_link_in_an_unwritable_lobe_stays_recorded() {
    // spec: LOBE-5 LOBE-6
    let sb = Sandbox::new(None);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let skills = sb.claude_home.join("skills");
    let default_link = skills.join("alpha");
    let own_link = sb.own_lobe().join("skills/alpha");

    write(
        &sb.source.join("mind.toml"),
        "[source]\nlobe = \"claude\"\n",
    );
    sb.commit("add lobe");
    chmod("a-w", &skills);
    if !enforced(&skills) {
        return;
    }
    let r = sb.mind(&["--yes", "upgrade"]);
    chmod("u+w", &skills);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("cannot be written"), "{}", r.stdout);
    assert!(is_link_into(&own_link, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(&default_link).is_ok());
    assert_eq!(sb.links_of("skill:alpha").len(), 2);

    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(std::fs::symlink_metadata(&default_link).is_err());
    assert_eq!(
        sb.links_of("skill:alpha"),
        [own_link.to_string_lossy().into_owned()]
    );
}

/// The consumer's `meld --lobe` outranks the source's own `lobe`, is recorded
/// on the source, moves the links of a melded source when given again, and
/// `--lobe ''` clears it back to the source's own lobe. Each run is
/// idempotent.
#[test]
fn consumer_lobe_outranks_and_moves_the_links() {
    // spec: LOBE-7 LOBE-5
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    let extra = sb.extra.to_string_lossy().into_owned();
    let spec = sb.source.to_string_lossy().into_owned();
    let mine = sb.extra.join("mine/skills/alpha");
    let r = sb.mind(&["--yes", "meld", &spec, "--home", &extra, "--lobe", "mine"]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(&mine, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(sb.own_lobe().join("skills/alpha")).is_err());
    let text = std::fs::read_to_string(sb.extra.join("sources.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["sources"][0]["lobe"], "mine");

    // Again, with another lobe: the links move.
    let other = sb.extra.join("other/skills/alpha");
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", "other"]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    assert!(r.stdout.contains("1 item to relink"), "{}", r.stdout);
    assert!(is_link_into(&other, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(&mine).is_err());

    // The same lobe again changes nothing.
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", "other"]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("relink"), "{}", r.stdout);

    // Cleared: back to the source's own lobe.
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", ""]);
    assert!(r.ok(), "re-meld: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(
        &sb.own_lobe().join("skills/alpha"),
        &sb.extra.join("store")
    ));
    assert!(std::fs::symlink_metadata(&other).is_err());
    let text = std::fs::read_to_string(sb.extra.join("sources.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["sources"][0].get("lobe").is_none(), "{text}");
}

/// `learn <url> --lobe` places an item link's item like `meld --lobe`, and
/// moves it when given again; for a plain item ref it is ignored with a note.
#[test]
fn learn_link_takes_lobe() {
    // spec: LOBE-7
    let sb = Sandbox::new(None);
    let extra = sb.extra.to_string_lossy().into_owned();
    let url = format!(
        "file://{}/tree/main/skills/alpha",
        sb.source.to_string_lossy()
    );
    let r = sb.mind(&["--yes", "learn", &url, "--home", &extra, "--lobe", "mine"]);
    assert!(r.ok(), "learn: {}\n{}", r.stdout, r.stderr);
    let mine = sb.extra.join("mine/skills/alpha");
    assert!(is_link_into(&mine, &sb.extra.join("store")));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/alpha")).is_err());

    let r = sb.mind(&["--yes", "learn", &url, "--lobe", "other"]);
    assert!(r.ok(), "learn again: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(
        &sb.extra.join("other/skills/alpha"),
        &sb.extra.join("store")
    ));
    assert!(std::fs::symlink_metadata(&mine).is_err());

    let r = sb.mind(&["--yes", "learn", "alpha", "--lobe", "x"]);
    assert!(r.stderr.contains("--lobe is ignored"), "{}", r.stderr);
}

/// With extra mind homes set, `--lobe` outside every home is refused before
/// the clone. With none set, any directory is allowed, and the install calls
/// it out as outside every mind home.
#[test]
fn lobe_outside_the_homes_is_allowed_only_without_extra_homes() {
    // spec: LOBE-2 LOBE-8
    let mut sb = Sandbox::new(None);
    let spec = sb.source.to_string_lossy().into_owned();
    let outside = sb.base.join("anywhere");
    let outside_str = outside.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", &outside_str]);
    assert!(!r.ok(), "meld: {}", r.stdout);
    assert!(
        r.stderr.contains("must lie under an allowed mind home"),
        "{}",
        r.stderr
    );
    assert_eq!(sb.sources_in(&sb.default_home), 0);

    sb.extra_homes = false;
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", &outside_str]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("outside every mind home"),
        "the install calls the lobe out: {}",
        r.stdout
    );
    assert!(is_link_into(
        &outside.join("skills/alpha"),
        &sb.default_home.join("store")
    ));
    assert!(std::fs::symlink_metadata(sb.claude_home.join("skills/alpha")).is_err());
}

/// A relative lobe is taken against the source's home, so moving the source
/// to another home with `meld --home` moves its links into that home's lobe.
#[test]
fn home_move_carries_a_relative_lobe() {
    // spec: LOBE-2 HOME-8
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let spec = sb.source.to_string_lossy().into_owned();
    let default = sb.default_home.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &spec, "--home", &default]);
    assert!(r.ok(), "move: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(
        &sb.default_home.join("claude/skills/alpha"),
        &sb.default_home.join("store")
    ));
    assert!(std::fs::symlink_metadata(sb.own_lobe().join("skills/alpha")).is_err());
}

/// A source lobe under a mind home is called out too, once per source, with
/// the install preview ahead of the confirmation.
#[test]
fn source_lobe_is_called_out_once() {
    // spec: LOBE-8
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    write(
        &sb.source.join("skills/beta/SKILL.md"),
        "---\ndescription: beta\n---\nbody\n",
    );
    sb.commit("add beta");
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert_eq!(
        r.stdout.matches("links its items into").count(),
        1,
        "{}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("outside every mind home"),
        "{}",
        r.stdout
    );
}

/// The sandbox case end to end: with the default home and the default lobe
/// both read-only, `upgrade <source>` updates a source in an extra home that
/// links into its own lobe there, writing nothing in either.
#[test]
fn sandboxed_upgrade_of_a_source_with_its_own_lobe() {
    // spec: LOBE-3 LOBE-6 HOME-1 HOME-3
    let sb = Sandbox::new(Some("[source]\nlobe = \"claude\"\n"));
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    // A second source in the default home, linked into the default lobe.
    let other_src = sb.base.join("other-src");
    write(
        &other_src.join("skills/gamma/SKILL.md"),
        "---\ndescription: gamma\n---\nbody\n",
    );
    git(&other_src, &["-c", "init.defaultBranch=main", "init", "-q"]);
    git(&other_src, &["config", "user.email", "t@t"]);
    git(&other_src, &["config", "user.name", "t"]);
    git(&other_src, &["add", "-A"]);
    git(&other_src, &["commit", "-qm", "initial"]);
    let r = sb.mind(&["--yes", "meld", &other_src.to_string_lossy()]);
    assert!(r.ok(), "meld other: {}\n{}", r.stdout, r.stderr);

    write(
        &sb.source.join("skills/alpha/SKILL.md"),
        "---\ndescription: alpha\n---\nsandboxed\n",
    );
    sb.commit("change alpha");
    std::fs::write(sb.default_home.join(".lock"), "").ok();
    let lock_ro = Command::new("chmod")
        .args(["-R", "a-w"])
        .arg(&sb.default_home)
        .arg(&sb.claude_home)
        .status()
        .expect("chmod");
    assert!(lock_ro.success());
    if !enforced(&sb.claude_home) {
        return;
    }
    let owner = sb.base.file_name().unwrap().to_string_lossy().into_owned();
    let ident = format!("local/{owner}/src");
    let r = sb.mind(&["--yes", "upgrade", &ident]);
    let _ = Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(&sb.default_home)
        .arg(&sb.claude_home)
        .status();
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    let alpha = sb.own_lobe().join("skills/alpha/SKILL.md");
    assert!(
        std::fs::read_to_string(&alpha)
            .unwrap()
            .contains("sandboxed"),
        "the upgraded copy is linked from the source's own lobe"
    );
    assert!(sb.claude_home.join("skills/gamma").exists());
}

/// A lobe inside the default lobe (allowed with no extra homes) is not
/// mistaken for a link left in the default lobe: once linked, `upgrade` has
/// nothing to relink.
#[test]
fn a_lobe_inside_the_default_lobe_settles() {
    // spec: LOBE-5
    let mut sb = Sandbox::new(None);
    sb.extra_homes = false;
    let spec = sb.source.to_string_lossy().into_owned();
    let inner = sb.claude_home.join("sandbox");
    let inner_str = inner.to_string_lossy().into_owned();
    let r = sb.mind(&["--yes", "meld", &spec, "--lobe", &inner_str]);
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    assert!(is_link_into(
        &inner.join("skills/alpha"),
        &sb.default_home.join("store")
    ));
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("relink"), "{}", r.stdout);
}

/// Without a source lobe, where the configured lobes point does not move
/// links: an `upgrade` run with `$MIND_AGENT_HOMES` naming another lobe
/// leaves the installed links alone.
#[test]
fn upgrade_does_not_move_links_of_a_source_without_a_lobe() {
    // spec: LOBE-5
    let sb = Sandbox::new(None);
    let project = sb.base.join("project/.devin");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let r = sb.mind(&["config", "lobes", "add", &project.to_string_lossy()]);
    assert!(r.ok(), "lobes add: {}", r.stderr);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let project_link = project.join("skills/alpha");
    assert!(is_link_into(&project_link, &sb.extra.join("store")));
    let other = sb.base.join("other");
    let cwd = std::env::current_dir().unwrap();
    let r = sb.mind_env(&cwd, &[("MIND_AGENT_HOMES", &other)], &["--yes", "upgrade"]);
    assert!(r.ok(), "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(!r.stdout.contains("relink"), "{}", r.stdout);
    assert!(is_link_into(&project_link, &sb.extra.join("store")));
}

/// An upgrade that fails linking into a later lobe rolls back without
/// removing a link in an earlier lobe that already pointed at the store copy:
/// the item is left as it was, still linked.
#[test]
fn a_failed_reinstall_keeps_the_links_it_did_not_make() {
    // spec: LOBE-6
    let sb = Sandbox::new(None);
    let project = sb.base.join("project/.devin");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let r = sb.mind(&["config", "lobes", "add", &project.to_string_lossy()]);
    assert!(r.ok(), "lobes add: {}", r.stderr);
    let r = sb.meld();
    assert!(r.ok(), "meld: {}\n{}", r.stdout, r.stderr);
    let claude_link = sb.claude_home.join("skills/alpha");
    assert!(is_link_into(&claude_link, &sb.extra.join("store")));

    // The project lobe's `skills` becomes a dangling symlink, so linking
    // there fails (not as an unwritable lobe) and the upgrade rolls back.
    std::fs::remove_dir_all(project.join("skills")).unwrap();
    std::os::unix::fs::symlink(sb.base.join("gone"), project.join("skills")).unwrap();
    write(
        &sb.source.join("skills/alpha/SKILL.md"),
        "---\ndescription: alpha\n---\nchanged\n",
    );
    sb.commit("change alpha");
    let r = sb.mind(&["--yes", "upgrade"]);
    assert!(!r.ok(), "upgrade should fail: {}", r.stdout);
    assert!(
        is_link_into(&claude_link, &sb.extra.join("store")),
        "the earlier lobe's link survives the rollback"
    );
    assert!(claude_link.join("SKILL.md").exists());
}
