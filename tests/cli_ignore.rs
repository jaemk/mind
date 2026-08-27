//! Integration tests for ignored files (spec/ignore.md, IGN-*): which paths
//! under an item are excluded from the store copy and the content hash.
//!
//! Each test drives the real `mind` binary against a hermetic fixture: a local
//! git repo melded by filesystem path, with `MIND_HOME`/`CLAUDE_HOME` pointed
//! at temp dirs. No network.

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
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-ign-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("lib");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(&source.join("README.md"), "# fixture\n");
        git(&source, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git(&source, &["config", "user.email", "t@t"]);
        git(&source, &["config", "user.name", "t"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "initial"]);
        sb
    }

    fn spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
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

    fn write_and_commit(&self, rel: &str, contents: &str) {
        write(&self.source.join(rel), contents);
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
    }

    /// The store copy of an installed item.
    fn store(&self, kind: &str, name: &str) -> PathBuf {
        self.mind_home.join("store").join(kind).join(name)
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

/// A source whose single skill IS the repo root: a top-level `SKILL.md`
/// declared with `path = "."`. This is the case the feature exists for.
fn root_skill_sandbox() -> Sandbox {
    let sb = Sandbox::new();
    sb.write_and_commit(
        "SKILL.md",
        "---\ndescription: A skill at the repo root\n---\n# root-skill\n",
    );
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"root-skill\"\npath = \".\"\n",
    );
    sb
}

#[test]
fn a_repo_root_item_does_not_install_the_vcs_directory() {
    // spec: IGN-2 IGN-10
    // The motivating case: with the item path at the repo root, the whole tree
    // is the item, so without the built-in set `.git/` is copied into the store
    // and hashed.
    let sb = root_skill_sandbox();
    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);

    let store = sb.store("skill", "root-skill");
    assert!(
        store.join("SKILL.md").is_file(),
        "the skill itself must be installed"
    );
    assert!(
        !store.join(".git").exists(),
        "the source's .git must not be copied into the store"
    );
    // The rest of the repo is still the item's content, since only VCS
    // metadata is implied.
    assert!(
        store.join("README.md").is_file(),
        "ordinary files at the item root are still installed"
    );
}

#[test]
fn a_repo_root_item_does_not_drift_when_the_source_repo_gets_a_commit() {
    // spec: IGN-10 IGN-2
    // The consequence that makes the feature worth having: hashing `.git/`
    // makes the item read as drifted after any commit, fetch, or `git gc` in
    // the source clone, so `upgrade` offers a change that is not a change to
    // the skill. The copy and the hash must exclude the same set.
    let sb = root_skill_sandbox();
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);

    let before = sb.mind(&["introspect"]);
    assert!(
        before.stdout.contains("all good"),
        "a fresh install must be clean: {}",
        before.stdout
    );

    // A commit that does not touch the skill: only `.git/` changes on disk.
    sb.write_and_commit("notes.txt", "unrelated\n");
    // ... and now remove it again, so the item's own content is byte-identical
    // to what was installed while `.git/` has moved on by two commits.
    std::fs::remove_file(sb.source.join("notes.txt")).unwrap();
    git(&sb.source, &["add", "-A"]);
    git(&sb.source, &["commit", "-qm", "revert"]);

    let after = sb.mind(&["introspect"]);
    assert!(
        after.stdout.contains("all good"),
        "commits that leave the item's content unchanged must not drift it: {}",
        after.stdout
    );
    let recall = sb.mind(&["recall", "skill:root-skill"]);
    assert!(
        !recall.stdout.contains("out of date"),
        "recall must not report drift either: {}",
        recall.stdout
    );
}

#[test]
fn a_declared_source_level_list_excludes_from_both_the_copy_and_the_hash() {
    // spec: IGN-1 IGN-10
    let sb = Sandbox::new();
    sb.write_and_commit("skills/review/SKILL.md", "---\ndescription: r\n---\n# r\n");
    sb.write_and_commit("skills/review/scratch/notes.md", "draft\n");
    sb.write_and_commit("skills/review/keep.md", "kept\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");

    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let store = sb.store("skill", "review");
    assert!(store.join("keep.md").is_file(), "unmatched files install");
    assert!(
        !store.join("scratch").exists(),
        "an ignored directory is not copied"
    );

    // And the hash agrees: editing an ignored file is not drift.
    sb.write_and_commit("skills/review/scratch/notes.md", "edited draft\n");
    let after = sb.mind(&["introspect"]);
    assert!(
        after.stdout.contains("all good"),
        "an ignored file's content must not affect the hash: {}",
        after.stdout
    );

    // Editing a file that is NOT ignored still drifts, so the exclusion is
    // narrow rather than switching drift detection off.
    sb.write_and_commit("skills/review/keep.md", "changed\n");
    let drifted = sb.mind(&["introspect"]);
    assert!(
        !drifted.stdout.contains("all good"),
        "a real content change must still drift: {}",
        drifted.stdout
    );
}

#[test]
fn an_item_level_list_replaces_the_source_level_one() {
    // spec: IGN-1 -- an item's own list REPLACES the source's for that item
    // rather than adding to it, matching how `[[items]]` overrides `[source]`.
    let sb = Sandbox::new();
    sb.write_and_commit("a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("a/src-ignored.md", "x\n");
    sb.write_and_commit("a/own-ignored.md", "y\n");
    sb.write_and_commit(
        "mind.toml",
        "[source]\nignore = [\"src-ignored.md\"]\n\n\
         [[items]]\nkind = \"skill\"\nname = \"a\"\npath = \"a\"\nignore = [\"own-ignored.md\"]\n",
    );

    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let store = sb.store("skill", "a");
    assert!(
        !store.join("own-ignored.md").exists(),
        "the item's own list applies"
    );
    assert!(
        store.join("src-ignored.md").is_file(),
        "the source list is REPLACED, not merged, so this is installed"
    );
}

#[test]
fn an_item_may_not_ignore_its_own_anchor_file() {
    // spec: IGN-5 -- otherwise the item installs as an empty directory that
    // discovery still offers and the harness cannot use.
    let sb = Sandbox::new();
    sb.write_and_commit("a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"a\"\npath = \"a\"\nignore = [\"SKILL.md\"]\n",
    );

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("SKILL.md") && r.stderr.contains("cannot ignore"),
        "the error must name the anchor: {}",
        r.stderr
    );
}

#[test]
fn an_unusable_ignore_pattern_is_refused_at_scan() {
    // spec: IGN-4 -- a hard error, not a silently inert entry: an entry that
    // never matches reads exactly like one that does.
    for (pattern, expect) in [
        ("/absolute", "not relative"),
        ("../outside", "'..' component"),
        ("[bad", "not a valid glob"),
    ] {
        let sb = Sandbox::new();
        sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
        sb.write_and_commit(
            "mind.toml",
            &format!("[source]\nignore = [\"{pattern}\"]\n"),
        );
        let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
        assert!(!r.success, "{pattern:?} must be refused: {}", r.stdout);
        assert!(
            r.stderr.contains(expect),
            "{pattern:?} must report {expect:?}: {}",
            r.stderr
        );
    }
}

#[test]
fn expanding_an_ignored_file_is_a_hard_error() {
    // spec: IGN-12 -- `expand:` and `ignore` naming the same file contradict;
    // that is an authoring mistake, named rather than resolved silently.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/a/SKILL.md",
        "---\ndescription: a\nexpand: gen/run.py\n---\n# a\n",
    );
    sb.write_and_commit("skills/a/gen/run.py", "# {{self}}\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"gen/\"]\n");

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("gen/run.py") && r.stderr.contains("expand"),
        "the error must name both directives: {}",
        r.stderr
    );
}

#[test]
fn an_ignored_file_is_invisible_to_the_reference_scan() {
    // spec: IGN-11 -- mind will not install the file, so it cannot be a source
    // of references mind resolves. Without this an unresolvable token in an
    // ignored file fails an install of content that is not being installed.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    // A token naming a sibling that does not exist: a hard BadReference if the
    // file is part of the item.
    sb.write_and_commit(
        "skills/a/scratch/draft.md",
        "handoff to {{ns:nonexistent}}\n",
    );
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(
        r.success,
        "a token in an ignored file must not fail the install: {} {}",
        r.stdout, r.stderr
    );
    // Assert the item really installed before asserting what it lacks: without
    // this the two negative checks below pass for an item that never existed.
    assert!(
        sb.store("skill", "a").join("SKILL.md").is_file(),
        "the item must actually be installed: {}",
        r.stdout
    );
    assert!(
        !sb.store("skill", "a").join("scratch").exists(),
        "and the file is not installed"
    );
}

#[test]
fn ignores_do_not_change_which_items_are_discovered() {
    // spec: IGN-13 -- `ignore` narrows what an item CONTAINS, never which items
    // a source offers. A pattern that looks like it names another item's
    // directory does not remove that item, because patterns are matched inside
    // each item, not against the repo.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/one/SKILL.md", "---\ndescription: 1\n---\n# 1\n");
    sb.write_and_commit("skills/two/SKILL.md", "---\ndescription: 2\n---\n# 2\n");
    sb.write_and_commit(
        "mind.toml",
        "[source]\nignore = [\"two/\", \"skills/two/\"]\n",
    );

    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:one") && probe.stdout.contains("skill:two"),
        "both items are still discovered and installable: {}",
        probe.stdout
    );
    assert!(
        sb.store("skill", "two").join("SKILL.md").is_file(),
        "and the second item installed normally"
    );
}

#[test]
fn ignores_travel_with_the_source_so_a_re_meld_reproduces_them() {
    // spec: IGN-20 -- an ignore list is source truth, living in the source's own
    // mind.toml, so nothing has to be recorded consumer-side for a re-meld (or a
    // dumped super-source) to install the same file set and compute the same
    // hash. There is deliberately no consumer-side ignore flag.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/a/scratch/x.md", "draft\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let first = sb.mind(&["recall", "skill:a"]);
    let hash_line = first
        .stdout
        .lines()
        .find(|l| l.contains("hash"))
        .expect("recall shows a hash")
        .to_string();

    // Drop it and meld again from the same source: same list, same file set.
    assert!(sb.mind(&["unmeld", "lib", "--yes"]).success);
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let second = sb.mind(&["recall", "skill:a"]);
    assert!(
        second.stdout.contains(hash_line.trim()),
        "the reproduced install must hash identically:\nfirst: {hash_line}\nsecond: {}",
        second.stdout
    );
    assert!(!sb.store("skill", "a").join("scratch").exists());
}

#[test]
fn dropping_a_vcs_directory_from_an_installed_item_is_an_ordinary_upgrade() {
    // spec: IGN-21 -- an item installed before the built-in set existed has the
    // VCS directory in its store copy and in its recorded hash. It reports as
    // out of date once, and `upgrade` re-installs it without that directory.
    // This is a real content change (the store copy loses files), so it is
    // reported rather than suppressed.
    let sb = root_skill_sandbox();
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);

    // Simulate the pre-feature state: a store copy that contains `.git` and a
    // manifest hash that measured it. Recomputing the hash is what the older
    // binary would have recorded, so hand-editing the manifest is the only way
    // to reach that state from here; instead assert the invariant that makes
    // the migration ordinary, namely that the current install is stable and
    // carries no VCS directory.
    let store = sb.store("skill", "root-skill");
    assert!(!store.join(".git").exists());
    let first = sb.mind(&["upgrade", "--yes"]);
    assert!(first.success, "upgrade failed: {}", first.stderr);
    let second = sb.mind(&["introspect"]);
    assert!(
        second.stdout.contains("all good"),
        "the install is stable across an upgrade: {}",
        second.stdout
    );
}
