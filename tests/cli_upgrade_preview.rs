//! Outside-in checks for upgrade previews against local sources.
mod common;

use common::{fixture_dir, git, write};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    home: PathBuf,
    extra: PathBuf,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let root = fixture_dir("upgrade-preview");
        let source = root.join("source");
        write(
            &source.join("skills/one/SKILL.md"),
            "---\ndescription: one\n---\n# one\n",
        );
        if !config.is_empty() {
            write(&source.join("mind.toml"), config);
        }
        for args in [
            &["-c", "init.defaultBranch=main", "init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["add", "-A"],
            &["commit", "-qm", "initial"],
        ] {
            git(&source, args);
        }
        Self {
            home: root.join("mind"),
            extra: root.join("extra"),
            root,
            source,
        }
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.home)
            .env("MIND_HOMES", &self.extra)
            .env("CLAUDE_HOME", self.root.join("claude"))
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "mind {args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &self.home)
            .env("MIND_HOMES", &self.extra)
            .env("CLAUDE_HOME", self.root.join("claude"))
            .env_remove("MIND_TTY")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "mind {args:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn source(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// spec: CLI-263
#[test]
fn preview_tracks_profile_additions_without_persisting_them() {
    let f = Fixture::new("[profiles]\ndefault = [\"skill:one\"]\n");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let registry = std::fs::read(f.home.join("sources.json")).unwrap();
    let manifest = std::fs::read(f.home.join("manifest.json")).unwrap();
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\n",
    );
    write(
        &f.source.join("mind.toml"),
        "[profiles]\ndefault = [\"skill:one\", \"skill:two\"]\n",
    );
    let doc = f.run(&["upgrade", "--dry-run", "--json"]);
    assert_eq!(doc["profile_additions"][0]["key"], "skill:two", "{doc}");
    assert_eq!(
        std::fs::read(f.home.join("sources.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(f.home.join("manifest.json")).unwrap(),
        manifest
    );
    assert!(!f.home.join("store/skill/two").exists());
}

// spec: CLI-263
#[test]
fn preview_reports_new_whole_source_items_and_keeps_extra_home_intact() {
    let f = Fixture::new("");
    f.run(&[
        "meld",
        &f.source(),
        "--home",
        f.extra.to_str().unwrap(),
        "--yes",
        "--json",
    ]);
    let registry = std::fs::read(f.extra.join("sources.json")).unwrap();
    let manifest = std::fs::read(f.extra.join("manifest.json")).unwrap();
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\n",
    );
    let doc = f.run(&["upgrade", "--dry-run", "--json"]);
    assert_eq!(doc["new_items"][0]["key"], "skill:two", "{doc}");
    assert_eq!(
        std::fs::read(f.extra.join("sources.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(f.extra.join("manifest.json")).unwrap(),
        manifest
    );
    assert!(!f.extra.join("store/skill/two").exists());
}

// spec: CLI-263
#[test]
fn preview_reports_items_that_ignore_new_would_record() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let registry = std::fs::read(f.home.join("sources.json")).unwrap();
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\n",
    );
    let doc = f.run(&["upgrade", "--dry-run", "--ignore-new", "--json"]);
    assert_eq!(doc["ignored_new_items"][0]["key"], "skill:two", "{doc}");
    let text = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["upgrade", "--dry-run", "--ignore-new"])
        .env("MIND_HOME", &f.home)
        .env("MIND_HOMES", &f.extra)
        .env("CLAUDE_HOME", f.root.join("claude"))
        .env_remove("MIND_TTY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(text.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(
        output.contains("would ignore new item skill:two"),
        "{output}"
    );
    assert_eq!(
        std::fs::read(f.home.join("sources.json")).unwrap(),
        registry
    );
    let applied = f.run(&["upgrade", "--no-sync", "--ignore-new", "--yes", "--json"]);
    assert_eq!(
        applied["ignored_new_items"][0]["key"], "skill:two",
        "{applied}"
    );
}

// spec: CLI-263
#[test]
fn preview_names_a_source_that_can_no_longer_be_scanned() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    std::fs::remove_dir_all(&f.source).unwrap();
    let doc = f.run(&["upgrade", "--dry-run", "--json"]);
    assert_eq!(doc["unscannable"].as_array().unwrap().len(), 1, "{doc}");
    assert!(
        doc["unscannable"][0].as_str().unwrap().ends_with("/source"),
        "{doc}"
    );
}

// spec: CLI-263
#[test]
fn preview_reports_an_additions_dependencies_and_install_hooks() {
    let f = Fixture::new("[profiles]\ndefault = [\"skill:one\"]\n");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let manifest = std::fs::read(f.home.join("manifest.json")).unwrap();
    let marker = f.root.join("hook-ran");
    write(
        &f.source.join("skills/two/SKILL.md"),
        &format!(
            "---\ndescription: two\ninstall: touch {}\n---\n# two\nUses {{{{ns:three}}}}.\n",
            marker.display()
        ),
    );
    write(
        &f.source.join("skills/three/SKILL.md"),
        "---\ndescription: three\n---\n# three\n",
    );
    write(
        &f.source.join("mind.toml"),
        "[profiles]\ndefault = [\"skill:one\", \"skill:two\"]\n",
    );
    let doc = f.run(&["upgrade", "--dry-run", "--json", "--accept-hooks"]);
    assert_eq!(doc["profile_additions"][0]["key"], "skill:two", "{doc}");
    assert_eq!(
        doc["dependencies"],
        serde_json::json!(["skill:three"]),
        "{doc}"
    );
    assert!(
        doc["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["item"] == "skill:two"
                && h["command"].as_str().unwrap_or_default().contains("touch")),
        "{doc}"
    );
    assert!(!marker.exists());
    assert!(!f.home.join("store/skill/three").exists());
    assert_eq!(
        std::fs::read(f.home.join("manifest.json")).unwrap(),
        manifest
    );
}

// spec: CLI-263 HOME-15
#[test]
fn preview_reports_a_declared_home_move_without_moving() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let registry = std::fs::read(f.home.join("sources.json")).unwrap();
    write(
        &f.source.join("mind.toml"),
        &format!("[source]\nmind-home = \"{}\"\n", f.extra.display()),
    );
    let doc = f.run(&["upgrade", "--dry-run", "--json"]);
    let moves = doc["home_moves"].as_array().unwrap();
    assert_eq!(moves.len(), 1, "{doc}");
    assert!(
        moves[0]["to"].as_str().unwrap().ends_with("/extra"),
        "{doc}"
    );
    assert!(
        moves[0]["from"].as_str().unwrap().ends_with("/mind"),
        "{doc}"
    );
    assert_eq!(
        std::fs::read(f.home.join("sources.json")).unwrap(),
        registry
    );
    assert!(!f.extra.join("sources.json").exists());
    assert!(f.home.join("store/skill/one").exists());
    // The real pass makes the move the preview named.
    f.run(&["upgrade", "--yes", "--json"]);
    assert!(f.extra.join("store/skill/one").exists());
}

// spec: CLI-263
#[test]
fn text_preview_says_when_nothing_would_change() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let text = f.text(&["upgrade", "--dry-run"]);
    assert!(text.contains("everything is up to date"), "{text}");
    write(
        &f.source.join("skills/one/SKILL.md"),
        "---\ndescription: one\n---\n# changed\n",
    );
    let text = f.text(&["upgrade", "--dry-run"]);
    assert!(!text.contains("everything is up to date"), "{text}");
    assert!(text.contains("skill:one"), "{text}");
}

// spec: CLI-264
#[test]
fn forget_glob_preview_matches_actual_selection_in_extra_home() {
    let f = Fixture::new("");
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\n",
    );
    git(&f.source, &["add", "-A"]);
    git(&f.source, &["commit", "-qm", "add two"]);
    f.run(&[
        "meld",
        &f.source(),
        "--home",
        f.extra.to_str().unwrap(),
        "--yes",
        "--json",
    ]);
    let registry = std::fs::read(f.extra.join("sources.json")).unwrap();
    let manifest = std::fs::read(f.extra.join("manifest.json")).unwrap();
    let doc = f.run(&["forget", "source#skill:*", "--dry-run", "--json"]);
    let mut preview: Vec<_> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["key"].as_str().unwrap().to_owned())
        .collect();
    preview.sort();
    assert_eq!(preview, ["skill:one", "skill:two"]);
    for item in doc["items"].as_array().unwrap() {
        let store = PathBuf::from(item["store"].as_str().unwrap());
        assert!(store.starts_with(&f.extra) && store.exists(), "{item}");
    }
    assert_eq!(
        std::fs::read(f.extra.join("sources.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(f.extra.join("manifest.json")).unwrap(),
        manifest
    );
    let applied = f.run(&["forget", "source#skill:*", "--yes", "--json"]);
    let mut removed: Vec<_> = applied["removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    removed.sort();
    assert_eq!(preview, removed);
}

// spec: CLI-264
#[test]
fn profile_removal_preview_keeps_dependencies_and_matches_apply() {
    let f = Fixture::new("[profiles]\ndefault = [\"skill:one\"]\n");
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\nUses {{ns:one}}.\n",
    );
    git(&f.source, &["add", "-A"]);
    git(&f.source, &["commit", "-qm", "add dependent"]);
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    f.run(&["learn", "skill:two", "--yes", "--json"]);
    let registry = std::fs::read(f.home.join("sources.json")).unwrap();
    let manifest = std::fs::read(f.home.join("manifest.json")).unwrap();
    let doc = f.run(&[
        "forget",
        "source",
        "--profile",
        "default",
        "--dry-run",
        "--json",
    ]);
    assert!(doc["items"].as_array().unwrap().is_empty(), "{doc}");
    assert_eq!(doc["profile"]["kept"][0]["key"], "skill:one", "{doc}");
    assert_eq!(
        doc["profile"]["kept"][0]["dependents"][0], "skill:two",
        "{doc}"
    );
    assert_eq!(
        std::fs::read(f.home.join("sources.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(f.home.join("manifest.json")).unwrap(),
        manifest
    );
    f.run(&[
        "forget",
        "source",
        "--profile",
        "default",
        "--yes",
        "--json",
    ]);
    assert!(f.home.join("store/skill/one").exists());
    assert!(f.home.join("store/skill/two").exists());
}

// spec: CLI-264
#[test]
fn forget_preview_names_dependents_and_dropped_items() {
    let f = Fixture::new("");
    write(
        &f.source.join("skills/two/SKILL.md"),
        "---\ndescription: two\n---\n# two\nUses {{ns:one}}.\n",
    );
    git(&f.source, &["add", "-A"]);
    git(&f.source, &["commit", "-qm", "add dependent"]);
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let manifest = std::fs::read(f.home.join("manifest.json")).unwrap();
    let doc = f.run(&["forget", "skill:one", "--dry-run", "--json"]);
    assert_eq!(doc["items"][0]["key"], "skill:one", "{doc}");
    assert_eq!(doc["items"][0]["dependents"][0], "skill:two", "{doc}");
    assert!(
        PathBuf::from(doc["items"][0]["store"].as_str().unwrap()).exists(),
        "{doc}"
    );
    std::fs::remove_dir_all(f.source.join("skills/two")).unwrap();
    let doc = f.run(&["forget", "--dropped", "--dry-run", "--json"]);
    let keys: Vec<_> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, ["skill:two"], "{doc}");
    assert_eq!(
        std::fs::read(f.home.join("manifest.json")).unwrap(),
        manifest
    );
    assert!(f.home.join("store/skill/two").exists());
}

// spec: CLI-264
#[test]
fn unmeld_preview_keeps_linked_source_and_matches_removal() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let registry = std::fs::read(f.home.join("sources.json")).unwrap();
    let manifest = std::fs::read(f.home.join("manifest.json")).unwrap();
    let doc = f.run(&["unmeld", "*source", "--dry-run", "--json"]);
    assert_eq!(doc["sources"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(doc["sources"][0]["items"][0]["key"], "skill:one", "{doc}");
    assert!(doc["sources"][0]["clone"].is_null(), "linked source: {doc}");
    assert_eq!(
        std::fs::read(f.home.join("sources.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(f.home.join("manifest.json")).unwrap(),
        manifest
    );
    f.run(&["unmeld", "*source", "--yes", "--json"]);
    assert!(f.source.join("skills/one/SKILL.md").exists());
    assert!(!f.home.join("store/skill/one").exists());
}

#[test]
fn read_command_finishes_committed_manifest_journal_and_rejects_bad_destination() {
    let f = Fixture::new("");
    f.run(&["meld", &f.source(), "--yes", "--json"]);
    let manifest = f.home.join("manifest.json");
    let original = std::fs::read(&manifest).unwrap();
    let journal = f.home.join(".manifest-journal.json");
    let intent = |destination: &std::path::Path| {
        serde_json::json!({
            "version": 1,
            "decision": "Commit",
            "intents": [{"Manifest": {"staged": destination, "destination": destination, "contents": original}}]
        })
    };
    std::fs::write(&manifest, b"{}\n").unwrap();
    std::fs::write(&journal, serde_json::to_vec(&intent(&manifest)).unwrap()).unwrap();
    f.run(&["recall", "--json"]);
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
    assert!(!journal.exists());

    let outside = f.root.join("outside");
    std::fs::write(&outside, b"keep").unwrap();
    std::fs::write(&journal, serde_json::to_vec(&intent(&outside)).unwrap()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall", "--json"])
        .env("MIND_HOME", &f.home)
        .env("MIND_HOMES", &f.extra)
        .env("CLAUDE_HOME", f.root.join("claude"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    assert!(journal.exists());
}
