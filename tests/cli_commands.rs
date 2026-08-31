//! The `command` item kind (spec/commands.md, CMD-1..9): end-to-end tests that
//! drive the real `mind` binary against a hermetic, network-free fixture (a
//! local git repo, isolated MIND_HOME/CLAUDE_HOME).
//!
//! Spec coverage:
//!   CMD-1: `commands/<name>.md` is discovered as a `command` item
//!   CMD-4: `--kind command` and the `command:<name>` ref select it
//!   CMD-5: it stores at `store/command/<name>` and links at `commands/<name>.md`
//!   CMD-6: a namespace prefix gives `commands/<prefix>:<name>.md`
//!   CMD-7: a skills-only lobe (the harness presets) admits no commands
//!   CMD-8: the kind-generic machinery (upgrade, forget, unmanaged) covers it

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

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
    /// A source repo with one command (`ship`) and one skill (`review`).
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-cmd-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("agents");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(
            &source.join("commands/ship.md"),
            "---\ndescription: Cut a release\nargument-hint: [version]\n---\n# ship\n",
        );
        write(
            &source.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Review the diff\n---\n# review\n",
        );
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        std::fs::create_dir_all(&sb.mind_home).unwrap();
        sb
    }

    fn mind(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .env_remove("MIND_AGENT_HOMES")
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

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    fn write_config(&self, body: &str) {
        write(&self.mind_home.join("config.toml"), body);
    }

    /// The lobe path a command links to.
    fn link(&self, name: &str) -> PathBuf {
        self.claude_home.join("commands").join(name)
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

fn git(repo: &Path, args: &[&str]) {
    std::fs::create_dir_all(repo).unwrap();
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A command is discovered, offered by `probe`, installed to the store, and
/// linked into the lobe at `commands/<name>.md`.
#[test]
fn command_installs_to_the_store_and_links_into_the_lobe() {
    // spec: CMD-1 CMD-4 CMD-5
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", &sb.source_spec()]);
    assert!(r.success, "meld: {}", r.stderr);

    let r = sb.mind(&["probe", "--no-tui", "--kind", "command"]);
    assert!(r.success, "probe: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("command:ship"),
        "--kind command must select the command: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("Cut a release"),
        "the command's frontmatter description must be shown: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skill:review"),
        "--kind command must exclude other kinds: {}",
        r.stdout
    );

    let r = sb.mind(&["learn", "command:ship"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    let link = sb.link("ship.md");
    assert!(
        link.symlink_metadata().is_ok(),
        "the command must be linked at commands/ship.md"
    );
    let target = std::fs::read_link(&link).expect("the lobe entry is a symlink");
    assert!(
        target.ends_with("store/command/ship"),
        "the link must point at store/command/ship, got {target:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&link).unwrap(),
        std::fs::read_to_string(sb.source.join("commands/ship.md")).unwrap(),
        "the installed command is the source file"
    );

    let r = sb.mind(&["recall", "--kind", "command"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("command:ship"),
        "recall must report the installed command: {}",
        r.stdout
    );
}

/// A namespace prefix renames a command to `<prefix>:<name>`, which is the
/// spelling the harness itself uses for a namespaced slash command.
#[test]
fn a_prefixed_command_links_under_its_namespaced_name() {
    // spec: CMD-6
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", &sb.source_spec(), "--namespace", "jk"]);
    assert!(r.success, "meld: {}", r.stderr);
    let r = sb.mind(&["learn", "command:jk:ship"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    assert!(
        sb.link("jk:ship.md").symlink_metadata().is_ok(),
        "a prefixed command links at commands/jk:ship.md"
    );
    assert!(
        sb.link("ship.md").symlink_metadata().is_err(),
        "the bare name must not also be linked"
    );
    assert!(
        sb.mind_home.join("store/command/jk:ship").exists(),
        "the store copy uses the effective name"
    );
}

/// `upgrade` picks up an edited command, and `forget` removes both the link and
/// the store copy: the kind rides the generic lifecycle machinery.
#[test]
fn command_upgrades_and_forgets_like_any_item() {
    // spec: CMD-8
    let sb = Sandbox::new();
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    assert!(sb.mind(&["learn", "command:ship"]).success);

    sb.write_and_commit(
        "commands/ship.md",
        "---\ndescription: Cut a release\n---\n# ship v2\n",
    );
    assert!(sb.mind(&["sync"]).success, "sync must succeed");
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("upgraded command:ship"),
        "the command must upgrade: {}",
        r.stdout
    );
    assert!(
        std::fs::read_to_string(sb.link("ship.md"))
            .unwrap()
            .contains("# ship v2"),
        "the linked file must show the new content"
    );

    let r = sb.mind(&["forget", "command:ship", "--yes"]);
    assert!(r.success, "forget: {}\n{}", r.stdout, r.stderr);
    assert!(
        sb.link("ship.md").symlink_metadata().is_err(),
        "forget removes the lobe link"
    );
    assert!(
        !sb.mind_home.join("store/command/ship").exists(),
        "forget removes the store copy"
    );
}

/// A lobe whose `kinds` filter excludes commands gets no command link (HARN-1).
/// The harness presets' own skills-only `kinds` list is covered separately by
/// `preset_lookup_and_resolution` in src/paths.rs, which is the actual CMD-7
/// regression test: a hand-written `kinds = ["skill"]` lobe here exercises the
/// generic filter, not the preset table.
#[test]
fn a_skills_only_lobe_admits_no_commands() {
    // spec: HARN-1
    let sb = Sandbox::new();
    let other = sb.base.join("gemini");
    std::fs::create_dir_all(&other).unwrap();
    sb.write_config(&format!(
        "[[lobes]]\npath = \"{}\"\n\n[[lobes]]\npath = \"{}\"\nkinds = [\"skill\"]\n",
        sb.claude_home.display(),
        other.display(),
    ));

    assert!(sb.mind(&["meld", &sb.source_spec()]).success);
    let r = sb.mind(&["learn", "command:ship"]);
    assert!(r.success, "learn: {}\n{}", r.stdout, r.stderr);

    assert!(
        sb.link("ship.md").symlink_metadata().is_ok(),
        "the all-kinds lobe gets the command"
    );
    assert!(
        !other.join("commands/ship.md").exists(),
        "a skills-only lobe must not receive a command link"
    );
}

/// A hand-written command in a lobe is reported as an unmanaged item, so
/// `recall` accounts for it and `absorb`/`forget` can act on it.
#[test]
fn a_hand_written_command_is_reported_as_unmanaged() {
    // spec: CMD-8
    let sb = Sandbox::new();
    write(
        &sb.claude_home.join("commands/mine.md"),
        "---\ndescription: hand written\n---\n# mine\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("unmanaged") && r.stdout.contains("command:mine"),
        "a hand-written command must be listed as unmanaged: {}",
        r.stdout
    );
}

/// Unmanaged-item detection and `absorb` see only the immediate `.md` children
/// of a lobe's `commands/` directory, matching the flat, non-recursive
/// convention scan (CMD-2): a nested layout (the grouped shape a source can
/// also ship on the source side) is invisible to both.
#[test]
fn a_nested_hand_written_command_is_not_detected_as_unmanaged() {
    // spec: CMD-8
    let sb = Sandbox::new();
    write(
        &sb.claude_home.join("commands/frontend/component.md"),
        "---\ndescription: hand written, nested\n---\n# component\n",
    );
    assert!(sb.mind(&["meld", &sb.source_spec()]).success);

    let r = sb.mind(&["recall"]);
    assert!(r.success, "recall: {}\n{}", r.stdout, r.stderr);
    assert!(
        !r.stdout.contains("component"),
        "a nested command must not be surfaced as unmanaged at all: {}",
        r.stdout
    );

    // The harness's own grouped name for this file is `frontend:component`
    // (CMD-2); `absorb` cannot reach it under that name either, since it never
    // entered the unmanaged set to begin with.
    let r = sb.mind(&["absorb", "frontend:component"]);
    assert!(
        !r.success,
        "absorb must not find a nested command: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    assert!(
        r.stderr.contains("not installed") || r.stderr.contains("NotInstalled"),
        "error must indicate not installed: {}",
        r.stderr
    );
}
