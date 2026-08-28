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
fn expanding_a_file_under_a_builtin_ignored_directory_names_the_builtin() {
    // spec: IGN-12 -- when no DECLARED ignore entry caused the exclusion (the
    // built-in VCS set did instead), the error must name the specific
    // built-in (`.git`) rather than the generic "a built-in ignore"
    // placeholder, which promised a pattern but rendered a vague sentence.
    let sb = Sandbox::new();
    sb.write_and_commit(
        "skills/a/SKILL.md",
        "---\ndescription: a\nexpand: .git/config\n---\n# a\n",
    );

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("'.git'"),
        "the error must name the specific built-in that caused the exclusion: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("a built-in ignore"),
        "the generic placeholder must not appear when the built-in is nameable: {}",
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
    //
    // M4: actually construct the pre-feature state rather than merely
    // asserting the post-feature install is stable (which passes unchanged
    // even if the migration behavior were broken). Hand-edit the manifest
    // hash to a junk value (what an older binary's `.git`-including hash
    // would look like next to the newer `.git`-excluding one) and drop a
    // `.git/` into the store copy (what an older binary's copy step would
    // have left there), matching what a pre-IGN-2 install on disk looked
    // like.
    let sb = root_skill_sandbox();
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);

    let manifest_path = sb.mind_home.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).unwrap();
    let mut doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let item = &mut doc["items"]["skill:root-skill"];
    assert!(!item.is_null(), "the installed item must exist: {doc}");
    item["hash"] = serde_json::Value::String("pretend-pre-ign-2-hash".to_string());
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let store = sb.store("skill", "root-skill");
    std::fs::create_dir_all(store.join(".git")).unwrap();
    std::fs::write(store.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

    let recall = sb.mind(&["recall", "skill:root-skill"]);
    assert!(
        recall.stdout.contains("out of date"),
        "the hand-edited hash must read as drift: {}",
        recall.stdout
    );

    let upgrade = sb.mind(&["upgrade", "--yes"]);
    assert!(upgrade.success, "upgrade failed: {}", upgrade.stderr);
    assert!(
        upgrade.stdout.contains("root-skill"),
        "upgrade must report the item: {}",
        upgrade.stdout
    );
    assert!(
        !store.join(".git").exists(),
        "the reinstalled store copy must not carry the VCS directory back"
    );

    let after = sb.mind(&["introspect"]);
    assert!(
        after.stdout.contains("all good"),
        "the install is stable across the upgrade: {}",
        after.stdout
    );
}

#[test]
fn a_tool_with_no_tool_md_may_still_ignore_a_file() {
    // spec: IGN-5 -- a tool's `TOOL.md` is optional (TOOL-2); an ignore
    // pattern must not be refused for excluding an anchor the tool never
    // shipped in the first place.
    let sb = Sandbox::new();
    sb.write_and_commit("tools/mytool/run.sh", "#!/bin/sh\necho hi\n");
    sb.write_and_commit("tools/mytool/README.md", "internal notes\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"*.md\"]\n");

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(
        r.success,
        "a tool with no TOOL.md must not be refused for ignoring *.md: {} {}",
        r.stdout, r.stderr
    );
    let store = sb.store("tool", "mytool");
    assert!(
        store.join("run.sh").is_file(),
        "the tool's own file installs"
    );
    assert!(
        !store.join("README.md").exists(),
        "the ignored README must not be copied"
    );
}

#[test]
fn a_dot_git_file_is_ignored_like_a_dot_git_directory() {
    // spec: IGN-2 IGN-21 -- `.git` is a regular FILE (not a directory) in a
    // submodule, a `git worktree` checkout, and `git init
    // --separate-git-dir`, exactly the trees IGN-21 names as this feature's
    // reach. The built-in set must match a file of the same name too, or the
    // store gets a dangling `.git` pointer file.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    // A `.git` FILE, as a submodule/worktree/separate-git-dir checkout has,
    // rather than a directory. Written directly into the working tree instead
    // of committed: git itself refuses to track a path literally named
    // `.git` (a repository-confusion protection), and a local, unpinned
    // source is a "linked" source (spec/source addressing) that `mind` scans
    // from its working tree directly, so an untracked file here is exactly
    // what a scan reads.
    write(
        &sb.source.join("skills/a/.git"),
        "gitdir: ../../.git/modules/a\n",
    );

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let store = sb.store("skill", "a");
    assert!(store.join("SKILL.md").is_file());
    assert!(
        !store.join(".git").exists(),
        "a `.git` FILE must be excluded the same as a `.git` directory"
    );
}

#[test]
fn changing_the_ignore_list_across_installs_is_an_ordinary_upgrade() {
    // spec: IGN-1 -- the effective ignore list is resolved from the source at
    // every scan, not frozen at install time; adding a pattern later must
    // drift and re-install the item without the newly-excluded content.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("skills/a/docs/guide.md", "guide\n");
    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let store = sb.store("skill", "a");
    assert!(
        store.join("docs/guide.md").is_file(),
        "docs/ installs before any ignore list is declared"
    );

    sb.write_and_commit("mind.toml", "[source]\nignore = [\"docs/\"]\n");
    let recall = sb.mind(&["recall", "skill:a"]);
    assert!(
        recall.stdout.contains("out of date"),
        "a newly-declared ignore list must read as drift: {}",
        recall.stdout
    );

    let upgrade = sb.mind(&["upgrade", "--yes"]);
    assert!(upgrade.success, "upgrade failed: {}", upgrade.stderr);
    assert!(
        !store.join("docs").exists(),
        "the reinstalled copy must honor the newly-declared ignore list"
    );
}

#[test]
fn an_item_link_item_inherits_the_source_level_ignore_list() {
    // spec: IGN-1 -- `resolve_ignores` runs at both `scan_source_at` exits: the
    // item-link early return (LNK-7) and the ordinary scan. This covers the
    // item-link path specifically.
    let sb = Sandbox::new();
    sb.write_and_commit("skills/review/SKILL.md", "---\ndescription: r\n---\n# r\n");
    sb.write_and_commit("skills/review/scratch/notes.md", "draft\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");

    let link = format!("file://{}/tree/main/skills/review", sb.source.display());
    let r = sb.mind(&["learn", &link]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    let store = sb.store("skill", "review");
    assert!(store.join("SKILL.md").is_file());
    assert!(
        !store.join("scratch").exists(),
        "an item-link item must inherit the source-level ignore list"
    );
}

#[test]
fn an_add_root_item_inherits_the_source_level_ignore_list() {
    // spec: IGN-1 -- `resolve_ignores` runs once over EVERY item `scan_source_at`
    // produces for the ordinary scan, whatever discovery layer found it: the
    // authoritative/convention layer and the `--add-root` composition
    // (DSC-84) both feed the same call. This covers the add-root path
    // specifically; moving the resolve call so it only covers the base layer
    // would leave an add-root item silently keeping `ignore: None` (losing
    // both the source list and the IGN-4/5/12 validation) with the rest of
    // the suite green.
    let sb = Sandbox::new();
    // No skills/agents/rules at the repo root: the base convention scan finds
    // nothing on its own, so the item below surfaces only via --add-root.
    sb.write_and_commit(
        "extra/skills/foo/SKILL.md",
        "---\ndescription: foo\n---\n# foo\n",
    );
    sb.write_and_commit("extra/skills/foo/scratch/notes.md", "draft\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"scratch/\"]\n");

    let r = sb.mind(&["meld", &sb.spec(), "--add-root", "extra", "--yes"]);
    assert!(
        r.success,
        "meld --add-root failed: {} {}",
        r.stdout, r.stderr
    );
    let store = sb.store("skill", "foo");
    assert!(
        store.join("SKILL.md").is_file(),
        "the add-root item must install"
    );
    assert!(
        !store.join("scratch").exists(),
        "an add-root item must inherit the source-level ignore list"
    );
}

#[test]
fn an_item_may_declare_an_explicit_empty_ignore_list_overriding_the_source() {
    // spec: IGN-1 -- `ignore = []` on an item is an explicit "no patterns"
    // override, distinct from declaring none at all (which inherits the
    // source list). Without distinguishing `None` from `Some(vec![])` an
    // item could not opt back OUT of a source-level exclusion.
    let sb = Sandbox::new();
    sb.write_and_commit("a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("a/scratch/notes.md", "draft\n");
    sb.write_and_commit(
        "mind.toml",
        "[source]\nignore = [\"scratch/\"]\n\n\
         [[items]]\nkind = \"skill\"\nname = \"a\"\npath = \"a\"\nignore = []\n",
    );

    assert!(sb.mind(&["meld", &sb.spec(), "--yes"]).success);
    let store = sb.store("skill", "a");
    assert!(
        store.join("scratch").exists(),
        "an explicit empty ignore list must override, not inherit, the source list"
    );
}

#[test]
fn a_bad_source_level_ignore_pattern_is_blamed_on_the_source_declaration() {
    // spec: IGN-4 -- `resolve_ignores` copies the source-level list into every
    // item BEFORE validating, so a bad entry there must be blamed on
    // `mind.toml [source].ignore`, not on whichever item's scan happened to
    // reach the check first (which sends the author looking for an `ignore`
    // key on that item that does not exist).
    let sb = Sandbox::new();
    sb.write_and_commit("skills/a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit("mind.toml", "[source]\nignore = [\"/absolute\"]\n");

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("mind.toml [source].ignore"),
        "the error must blame the source declaration, not the item: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("skill:a:"),
        "the error must not misattribute the source-level pattern to the item: {}",
        r.stderr
    );
}

#[test]
fn a_bad_item_level_ignore_pattern_is_blamed_on_the_item() {
    // spec: IGN-4 -- the counterpart to the previous test: an item that
    // declared its OWN bad pattern is still blamed by its item key, exactly
    // as before.
    let sb = Sandbox::new();
    sb.write_and_commit("a/SKILL.md", "---\ndescription: a\n---\n# a\n");
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"a\"\npath = \"a\"\nignore = [\"/absolute\"]\n",
    );

    let r = sb.mind(&["meld", &sb.spec(), "--yes"]);
    assert!(!r.success, "must be refused: {}", r.stdout);
    assert!(
        r.stderr.contains("skill:a:"),
        "the error must blame the declaring item: {}",
        r.stderr
    );
}
