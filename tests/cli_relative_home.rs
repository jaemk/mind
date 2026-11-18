//! Relative MIND_HOME paths must use the same journal and destination paths.
mod common;

use common::{fixture_dir, git, write};
use std::process::{Command, Stdio};

// spec: STO-87
#[test]
fn relative_home_installs_and_recovers_manifest() {
    let root = fixture_dir("relative-home");
    let source = root.join("source");
    write(
        &source.join("skills/one/SKILL.md"),
        "---\ndescription: one\n---\n# one\n",
    );
    for args in [
        &["-c", "init.defaultBranch=main", "init", "-q"][..],
        &["config", "user.email", "t@t"],
        &["config", "user.name", "t"],
        &["add", "-A"],
        &["commit", "-qm", "initial"],
    ] {
        git(&source, args);
    }
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mind"))
            .current_dir(&root)
            .env("MIND_HOME", "./state")
            .env_remove("MIND_HOMES")
            .env("CLAUDE_HOME", root.join("claude"))
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    };
    let source_arg = source.to_str().unwrap();
    let result = run(&["meld", source_arg, "--yes", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );

    let manifest = root.join("state/manifest.json");
    let journal = root.join("state/.manifest-journal.json");
    assert!(manifest.exists());
    assert!(!journal.exists());
    let contents = std::fs::read(&manifest).unwrap();
    let doc = serde_json::json!({"version": 1, "decision": "Commit", "intents": [{
        "Manifest": {"staged": "./state/manifest.json", "destination": "./state/manifest.json", "contents": contents}
    }]});
    std::fs::write(&journal, serde_json::to_vec(&doc).unwrap()).unwrap();
    let result = run(&["recall", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(!journal.exists());
    let result = run(&["forget", "one", "--yes", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(!journal.exists());
    std::fs::remove_dir_all(root).unwrap();
}
