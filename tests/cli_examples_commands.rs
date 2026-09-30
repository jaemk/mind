//! Integration tests proving the two examples added for the `command` kind and
//! the update-hook feature (docs/src/examples.md, finding M20) actually work.
//!
//! Each test drives the real `mind` binary against a hermetic fixture built
//! from a shipped `examples/<name>` directory. No network.

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
    /// A source repo populated from `examples/<name>` in the crate, committed.
    fn from_example(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-exc-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        let example = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name);
        copy_dir(&example, &source);
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
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

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
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

// ---------------------------------------------------------------------------
// The `starter` example: a plain-convention `commands/<n>.md` (M20b)
// ---------------------------------------------------------------------------

/// The `starter` example ships `commands/ship.md`, a plain-convention command
/// item with no `mind.toml`. It is discovered, installs into the store, and
/// links into the agent home's `commands/` directory like any other kind.
#[test]
fn starter_command_item_is_discovered_installed_and_linked() {
    // spec: CMD-1 CMD-5
    let sb = Sandbox::from_example("starter");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("command:ship"),
        "the ship command must be discovered by convention: {}",
        probe.stdout
    );

    let learn = sb.mind(&["learn", "command:ship"]);
    assert!(learn.success, "{}\n{}", learn.stdout, learn.stderr);

    let store = sb.mind_home.join("store/command/ship");
    assert!(
        store.exists(),
        "ship must be copied into the command store: {:?}",
        store
    );

    let link = sb.claude_home.join("commands/ship.md");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "ship must be linked at commands/ship.md: {:?}",
        link
    );
}

/// The `starter` example ships `workflows/hello.js`, a plain-convention
/// workflow item with no `mind.toml`. It is discovered, installs into the
/// store, and links into the agent home's `workflows/` directory like any
/// other kind.
#[test]
fn starter_workflow_item_is_discovered_installed_and_linked() {
    // spec: WF-1 WF-10
    let sb = Sandbox::from_example("starter");
    let meld = sb.mind(&["meld", &sb.source_spec()]);
    assert!(meld.success, "{}", meld.stderr);

    let probe = sb.mind(&["probe"]);
    assert!(probe.success, "{}", probe.stderr);
    assert!(
        probe.stdout.contains("workflow:hello"),
        "the hello workflow must be discovered by convention: {}",
        probe.stdout
    );

    let learn = sb.mind(&["learn", "workflow:hello"]);
    assert!(learn.success, "{}\n{}", learn.stdout, learn.stderr);

    let store = sb.mind_home.join("store/workflow/hello");
    assert!(
        store.exists(),
        "hello must be copied into the workflow store: {:?}",
        store
    );

    let link = sb.claude_home.join("workflows/hello.js");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false),
        "hello must be linked at workflows/hello.js: {:?}",
        link
    );
}

/// The fixture's `export const meta` block must actually be READABLE by
/// `workflow_meta.rs`, not merely present: the sibling test above passes just as
/// well if the object's syntax drifts outside the reader's accepted grammar,
/// because the item is discovered from the file's PATH and would still install
/// with no description at all. An example that silently stopped demonstrating
/// WF-4/WF-51 would be worse than no example, so both string fields the reader
/// takes from the object are pinned here, through `probe`'s own rendering.
///
/// `whenToUse` is the field that makes this worth a separate assertion: no other
/// item kind has one, it is read from the same object by the same reader, and it
/// is the one field a markdown-frontmatter habit would drop.
#[test]
fn starter_workflow_meta_object_is_really_read_not_just_present() {
    // spec: WF-4 WF-5 WF-51
    let sb = Sandbox::from_example("starter");
    assert!(
        sb.mind(&["meld", &sb.source_spec(), "--register-only"])
            .success
    );

    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "{}", probe.stderr);
    let line = probe
        .stdout
        .lines()
        .find(|l| l.contains("workflow:hello"))
        .unwrap_or_else(|| panic!("no workflow:hello line: {}", probe.stdout));
    assert!(
        line.contains("Greet the user and summarize the repo state"),
        "meta.description must be read out of the `meta` object (WF-4): {line}"
    );
    // `whenToUse` is asserted through the SEARCH path rather than the listing:
    // the listing composes it as `<description> - <whenToUse>` (WF-51) and then
    // truncates to the terminal width, so a substring of it is not reliably on
    // screen. A query matches the composed text in full, so a word that appears
    // ONLY in `whenToUse` selecting the item proves the field was read -- and
    // proves it came from the `meta` object rather than the file path (CLI-85).
    let by_when = sb.mind(&["probe", "--no-tui", "orientation"]);
    assert!(by_when.success, "{}", by_when.stderr);
    assert!(
        by_when.stdout.contains("workflow:hello"),
        "a word appearing only in meta.whenToUse must match (WF-51): {}",
        by_when.stdout
    );
}

/// The starter workflow writes `meta.name` as the `{{ns:hello}}` token, so the
/// harness name tracks the effective name: `hello` unprefixed and `jk:hello`
/// under `--namespace jk`, with no WF-24 divergence warning either way.
#[test]
fn starter_workflows_token_meta_name_tracks_the_prefix() {
    // spec: WF-23 WF-24
    let bare = Sandbox::from_example("starter");
    assert!(
        bare.mind(&["meld", &bare.source_spec(), "--register-only"])
            .success
    );
    let learn = bare.mind(&["learn", "workflow:hello"]);
    assert!(learn.success, "{}\n{}", learn.stdout, learn.stderr);
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        !combined.contains("the harness resolves it as"),
        "unprefixed, the token expands to the effective name, so there is \
         nothing to warn about: {combined}"
    );
    let stored = std::fs::read_to_string(bare.mind_home.join("store/workflow/hello"))
        .expect("read the bare store copy");
    assert!(
        stored.contains("name: 'hello'"),
        "the token must expand to the bare name: {stored}"
    );

    let pre = Sandbox::from_example("starter");
    assert!(
        pre.mind(&[
            "meld",
            &pre.source_spec(),
            "--register-only",
            "--namespace",
            "jk",
        ])
        .success
    );
    let learn = pre.mind(&["learn", "workflow:jk:hello"]);
    assert!(learn.success, "{}\n{}", learn.stdout, learn.stderr);
    let combined = format!("{}{}", learn.stdout, learn.stderr);
    assert!(
        !combined.contains("the harness resolves it as"),
        "under a prefix the token expands to the prefixed name, so there is \
         nothing to warn about: {combined}"
    );
    let stored = std::fs::read_to_string(pre.mind_home.join("store/workflow/jk:hello"))
        .expect("read the prefixed store copy");
    assert!(
        stored.contains("name: 'jk:hello'"),
        "the token must expand to the prefixed name: {stored}"
    );
    assert!(
        std::fs::symlink_metadata(pre.claude_home.join("workflows/jk:hello.js")).is_ok(),
        "the prefixed workflow must be linked: {combined}"
    );
}

// ---------------------------------------------------------------------------
// The `hooks` example: an update hook (M20c)
// ---------------------------------------------------------------------------

/// The `hooks` example declares an update hook alongside its install and
/// uninstall hooks. `hooks list` reports it with the `[update]` event tag, but
/// melding the source never runs it: update hooks run only at `upgrade`, once
/// the source has moved (HOOK-121).
#[test]
fn hooks_example_update_hook_is_listed_and_not_offered_at_meld() {
    // spec: HOOK-120 HOOK-121
    let sb = Sandbox::from_example("hooks");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(meld.success, "{}\n{}", meld.stdout, meld.stderr);

    let migrated = sb.source.join("bin/.migrated");
    assert!(
        !migrated.exists(),
        "the update hook must not run at meld: {:?}",
        migrated
    );

    let list = sb.mind(&["hooks", "list", "*"]);
    assert!(list.success, "{}\n{}", list.stdout, list.stderr);
    assert!(
        list.stdout.contains("[update]"),
        "hooks list must report the declared update hook: {}",
        list.stdout
    );
}
