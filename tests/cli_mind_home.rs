//! Integration tests for the global `--mind-home` flag (spec/cli.md CLI-263,
//! spec/storage.md STO-1).
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture: a local git repo melded by path, with `MIND_HOME`/`CLAUDE_HOME`
//! pointed at temp dirs.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Sandbox {
    base: PathBuf,
    source: PathBuf,
    /// The root `MIND_HOME` names; the flag must outrank it.
    env_home: PathBuf,
    /// The root `--mind-home` names.
    flag_home: PathBuf,
    claude_home: PathBuf,
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-mh-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let sb = Sandbox {
            source: base.join("src"),
            env_home: base.join("env-home"),
            flag_home: base.join("flag-home"),
            claude_home: base.join("claude"),
            base,
        };
        // The skill's install hook records the `MIND_HOME` it was given.
        write(
            &sb.source.join("skills/alpha/SKILL.md"),
            &format!(
                "---\ndescription: alpha\ninstall: printf %s \"$MIND_HOME\" > {}\n---\nbody\n",
                sb.base.join("seen").display()
            ),
        );
        git(&sb.source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&sb.source, &["config", "user.email", "t@t"]);
        git(&sb.source, &["config", "user.name", "t"]);
        git(&sb.source, &["add", "-A"]);
        git(&sb.source, &["commit", "-qm", "initial"]);
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.env_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .output()
            .expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        }
    }

    fn flag(&self) -> String {
        format!("--mind-home={}", self.flag_home.display())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
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

/// `--mind-home` outranks `MIND_HOME`: state, store, and links all use the
/// flag's root, and a hook sees that root as `MIND_HOME`.
#[test]
fn the_flag_outranks_the_env_var_and_reaches_hooks() {
    // spec: CLI-263 STO-1
    let sb = Sandbox::new();
    let source = sb.source.to_string_lossy().into_owned();
    let flag = sb.flag();
    let r = sb.mind(&[
        &flag,
        "--yes",
        "meld",
        &source,
        "--accept-hooks=install,update,uninstall",
    ]);
    assert_eq!(r.code, Some(0), "meld: {}\n{}", r.stdout, r.stderr);

    assert!(sb.flag_home.join("sources.json").is_file());
    assert!(sb.flag_home.join("manifest.json").is_file());
    assert!(
        !sb.env_home.exists(),
        "MIND_HOME was outranked, so nothing is written there"
    );
    let link = sb.claude_home.join("skills/alpha");
    assert!(
        std::fs::canonicalize(&link)
            .unwrap()
            .starts_with(std::fs::canonicalize(&sb.flag_home).unwrap()),
        "the link resolves into the flag's store"
    );
    let seen = std::fs::read_to_string(sb.base.join("seen")).expect("hook ran");
    assert_eq!(
        Path::new(&seen),
        sb.flag_home,
        "the hook sees the flag's root"
    );

    // A later run with the flag after the verb reads the same state.
    let r = sb.mind(&["recall", "--json", &flag]);
    assert_eq!(r.code, Some(0), "recall: {}", r.stderr);
    assert!(r.stdout.contains("alpha"), "{}", r.stdout);
    // Without the flag, MIND_HOME's empty root is used.
    let r = sb.mind(&["recall", "--json"]);
    assert_eq!(r.code, Some(0), "recall: {}", r.stderr);
    assert!(!r.stdout.contains("\"alpha\""), "{}", r.stdout);
}

/// A leading `~` in an `=`-joined value, which the shell leaves alone, is
/// expanded against `HOME`.
#[test]
fn the_flag_expands_a_leading_tilde() {
    // spec: CLI-263
    let sb = Sandbox::new();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["--mind-home=~/tilde-home", "config", "show"])
        .env("HOME", &sb.base)
        .env("MIND_HOME", &sb.env_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .stdin(Stdio::null())
        .output()
        .expect("run mind");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&sb.base.join("tilde-home").display().to_string()),
        "{stdout}"
    );
    assert!(!stdout.contains("~/tilde-home"), "{stdout}");
}

/// A relative value is resolved against the working directory, so state and
/// hooks get an absolute root.
#[test]
fn the_flag_resolves_a_relative_path() {
    // spec: CLI-263
    let sb = Sandbox::new();
    let source = sb.source.to_string_lossy().into_owned();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args([
            "--mind-home",
            "rel-home",
            "--yes",
            "meld",
            &source,
            "--accept-hooks=install,update,uninstall",
        ])
        .current_dir(&sb.base)
        .env("MIND_HOME", &sb.env_home)
        .env("CLAUDE_HOME", &sb.claude_home)
        .stdin(Stdio::null())
        .output()
        .expect("run mind");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(sb.base.join("rel-home/sources.json").is_file());
    let seen = std::fs::read_to_string(sb.base.join("seen")).expect("hook ran");
    assert_eq!(Path::new(&seen), sb.base.join("rel-home"));
}

/// An empty value is a usage error rather than a silent fallback.
#[test]
fn an_empty_value_is_a_usage_error() {
    // spec: CLI-263
    let sb = Sandbox::new();
    let r = sb.mind(&["--mind-home=", "recall"]);
    assert_eq!(r.code, Some(2), "{}\n{}", r.stdout, r.stderr);
    assert!(!sb.env_home.exists());
}
