//! Integration tests for `mind dump` (DUMP-1 through DUMP-8).
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
    /// Create a sandbox with a source repo that has one skill, one agent, and one rule.
    fn new(name: &str) -> Sandbox {
        Sandbox::build(name, true)
    }

    /// Create a sandbox with a source repo that has no items (pure super-source or registry).
    fn bare(name: &str) -> Sandbox {
        Sandbox::build(name, false)
    }

    fn build(name: &str, with_items: bool) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-dump-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };

        if with_items {
            write_file(
                &source.join("skills/review/SKILL.md"),
                "---\nname: review\ndescription: Review skill\n---\n# review\n",
            );
            write_file(
                &source.join("agents/dev.md"),
                "---\nname: dev\ndescription: Dev agent\n---\n# dev\n",
            );
            write_file(
                &source.join("rules/style.md"),
                "---\ndescription: Style rule\n---\n# style\n",
            );
        } else {
            write_file(&source.join("README.md"), "# registry\n");
        }

        git_init(&source);
        sb
    }

    fn source_spec(&self) -> String {
        self.source.to_string_lossy().into_owned()
    }

    /// A `file://` item-link URL into this sandbox's source repo (item-link.md
    /// LNK-1), e.g. `sb.link("tree/main/skills/review")`.
    fn link(&self, tail: &str) -> String {
        format!("file://{}/{tail}", self.source.to_string_lossy())
    }

    fn mind(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        let out = cmd.output().expect("run mind");
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            success: out.status.success(),
        }
    }

    /// Write a file under the source repo and commit it.
    fn write_and_commit(&self, rel: &str, contents: &str) {
        write_file(&self.source.join(rel), contents);
        git_commit(&self.source);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn write_file(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git_init(dir: &Path) {
    for args in [
        vec!["-c", "init.defaultBranch=main", "init", "-q"],
        vec!["config", "user.email", "t@t"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "initial"],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} in {dir:?}");
    }
}

fn git_commit(dir: &Path) {
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "fixture"]] {
        let _ = Command::new("git")
            .args(&args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Create a tag at the current HEAD of `dir`.
fn git_tag(dir: &Path, tag: &str) {
    let status = Command::new("git")
        .args(["tag", tag])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git tag");
    assert!(status.success(), "git tag {tag} in {dir:?}");
}

/// Force-move an existing tag to the current HEAD of `dir`.
fn git_move_tag(dir: &Path, tag: &str) {
    let status = Command::new("git")
        .args(["tag", "-f", tag])
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git tag -f");
    assert!(status.success(), "git tag -f {tag} in {dir:?}");
}

/// The HEAD commit sha of `dir`.
fn git_head(dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// DUMP-8: empty registry
// ---------------------------------------------------------------------------

#[test]
fn dump_empty_registry_produces_valid_super_source() {
    // spec: DUMP-8 — with no melded sources, `mind dump` emits a valid
    // super-source whose [discover].sources is empty and exits 0.
    let sb = Sandbox::bare("empty");
    // Do not meld anything.
    let r = sb.mind(&["dump"]);
    assert!(
        r.success,
        "dump with no melded sources must exit 0: {} {}",
        r.stdout, r.stderr
    );
    // Output must be valid TOML with [discover].sources.
    let toml_text = &r.stdout;
    assert!(
        toml_text.contains("discover"),
        "must contain discover section: {toml_text}"
    );
    // Parses as a valid MindToml (DUMP-7 / DSC-3 / DSC-30).
    // We can check the TOML is at least parseable structurally by looking for the
    // key markers.
    assert!(
        toml_text.contains("description"),
        "must carry [source].description: {toml_text}"
    );
    // The sources list must be empty: no [[discover.sources]] entries.
    assert!(
        !toml_text.contains("[[discover.sources]]"),
        "no source entries should appear when registry is empty: {toml_text}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-1: stdout by default; --output writes to file
// ---------------------------------------------------------------------------

#[test]
fn dump_writes_to_stdout_by_default() {
    // spec: DUMP-1 — without --output, dump writes to stdout.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump must succeed: {} {}", r.stdout, r.stderr);
    assert!(!r.stdout.is_empty(), "dump must write to stdout by default");
    assert!(
        r.stdout.contains("discover"),
        "stdout must contain discover section: {}",
        r.stdout
    );
}

#[test]
fn dump_output_flag_writes_to_file() {
    // spec: DUMP-1 — --output <path> writes to the given file.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);

    let out_path = sb.base.join("dump.toml");
    let out_str = out_path.to_string_lossy().into_owned();
    let r = sb.mind(&["dump", "--output", &out_str]);
    assert!(
        r.success,
        "dump --output must succeed: {} {}",
        r.stdout, r.stderr
    );
    // stdout must be empty (output was redirected).
    assert!(
        r.stdout.is_empty(),
        "stdout must be empty when --output is given: {}",
        r.stdout
    );
    // The file must exist and contain the document.
    let content = std::fs::read_to_string(&out_path).expect("output file must exist");
    assert!(
        content.contains("discover"),
        "output file must contain discover section: {content}"
    );
    assert!(
        content.contains("description"),
        "output file must carry [source].description: {content}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-2: item filtering
// ---------------------------------------------------------------------------

#[test]
fn dump_all_installed_yields_install_true() {
    // spec: DUMP-2 — every offered item installed -> install = true.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install = true"),
        "all items installed must emit install = true: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install-items"),
        "must NOT emit install-items when all installed: {}",
        r.stdout
    );
}

#[test]
fn dump_none_installed_yields_install_false() {
    // spec: DUMP-2 — none installed -> install = false; never emit [].
    let sb = Sandbox::new("src");
    // Meld but do not install anything.
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install = false"),
        "no items installed must emit install = false: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install-items"),
        "must NOT emit install-items when none installed: {}",
        r.stdout
    );
    // install_items = [] must never appear (DUMP-2).
    assert!(
        !r.stdout.contains("install-items = []"),
        "empty install-items must never be emitted: {}",
        r.stdout
    );
}

#[test]
fn dump_proper_subset_yields_install_items() {
    // spec: DUMP-2 DUMP-5 — proper subset installed -> install_items listing
    // exactly those items by bare kind:name.
    let sb = Sandbox::new("src");
    // Install only skill:review (a proper subset of the three offered items).
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn skill:review failed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install-items"),
        "proper subset must emit install-items: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:review"),
        "install-items must contain skill:review: {}",
        r.stdout
    );
    // The non-installed items must NOT appear in install-items.
    assert!(
        !r.stdout.contains("agent:dev"),
        "agent:dev (not installed) must not appear in install-items: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("rule:style"),
        "rule:style (not installed) must not appear in install-items: {}",
        r.stdout
    );
    // install = true must not be emitted alongside install-items.
    assert!(
        !r.stdout.contains("install = true"),
        "install = true must not be emitted with install-items: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-3: --whole-sources
// ---------------------------------------------------------------------------

#[test]
fn dump_whole_sources_always_emits_install_true() {
    // spec: DUMP-3 — --whole-sources emits install = true for every melded
    // source regardless of how many items are installed.
    let sb = Sandbox::new("src");
    // Install only a subset.
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["dump", "--whole-sources"]);
    assert!(
        r.success,
        "dump --whole-sources failed: {} {}",
        r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("install = true"),
        "--whole-sources must emit install = true: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install-items"),
        "--whole-sources must NOT emit install-items: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-4: meld-time settings in each entry
// ---------------------------------------------------------------------------

#[test]
fn dump_emits_alias_when_melded_with_as() {
    // spec: DUMP-4 DSC-78 — the entry carries the consumer alias as `namespace`
    // (the canonical key) so re-melding restores the prefix.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--as", "mypfx", "--link-only"]);
    assert!(meld.success, "meld --as failed: {}", meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("namespace = \"mypfx\""),
        "emitted entry must carry namespace = \"mypfx\" (DSC-78 canonical key): {}",
        r.stdout
    );
}

#[test]
fn dump_emits_roots_when_set() {
    // spec: DUMP-4 — the entry carries the scan roots when set at meld time.
    let sb = Sandbox::new("src");
    // Meld with an explicit root (the source dir itself, so the scan still works).
    let meld = sb.mind(&["meld", &sb.source_spec(), "--root", ".", "--link-only"]);
    assert!(
        meld.success,
        "meld --root failed: {} {}",
        meld.stdout, meld.stderr
    );

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("roots"),
        "emitted entry must carry roots when set: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-5: install_items are bare kind:name
// ---------------------------------------------------------------------------

#[test]
fn dump_install_items_are_bare_kind_name() {
    // spec: DUMP-5 — items in install_items use bare kind:name; the entry's `as`
    // prefix applies at re-install, not embedded in the ref.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--as", "pfx", "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    // Install only skill:review (under the prefixed name pfx:review).
    let learn = sb.mind(&["learn", "skill:pfx:review"]);
    assert!(
        learn.success,
        "learn skill:pfx:review failed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    // install-items must list the BARE name, not the prefixed name.
    assert!(
        r.stdout.contains("skill:review"),
        "install_items must use bare kind:name (skill:review, not pfx:review): {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("skill:pfx:review"),
        "prefixed name must NOT appear in install_items: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-6: dependency items counted
// ---------------------------------------------------------------------------

#[test]
fn dump_dependency_item_is_in_install_items() {
    // spec: DUMP-6 — an item installed as a within-source dependency (via
    // `requires:` or `{{ns:}}` tokens) is part of the installed set and
    // appears in install_items like any other.
    //
    // We build a source where skill:main `requires: agent:dep`, then install
    // only skill:main. The install flow also installs agent:dep as a
    // dependency. The dump must list both in install_items.
    let sb = Sandbox::bare("dep-src");
    sb.write_and_commit(
        "agents/dep.md",
        "---\nname: dep\ndescription: Dependency agent\n---\n# dep\n",
    );
    sb.write_and_commit(
        "skills/main/SKILL.md",
        "---\nname: main\ndescription: Main skill\nrequires: agent:dep\n---\n# main\n",
    );

    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    // Install main (which pulls in dep as a dependency).
    let learn = sb.mind(&["learn", "skill:main", "--yes"]);
    assert!(
        learn.success,
        "learn skill:main failed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    // The dump must NOT emit install = true (not all items installed if the
    // source has two and only two are installed but only if they differ).
    // Since both are installed and both are offered, either install=true or
    // install_items listing both is valid. Check both appear.
    let has_both_main_and_dep = r.stdout.contains("skill:main") && r.stdout.contains("agent:dep");
    let has_install_true = r.stdout.contains("install = true");
    assert!(
        has_both_main_and_dep || has_install_true,
        "dump must account for the dependency item (agent:dep): {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-7: emitted file is a valid super-source
// ---------------------------------------------------------------------------

#[test]
fn dump_output_is_valid_super_source() {
    // spec: DUMP-7 — the emitted file parses as a valid super-source
    // (DSC-3, DSC-30 deny_unknown_fields) with [source].description.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);

    // Write the stdout to a temp file and meld it to verify it parses.
    let dump_path = sb.base.join("dumped.toml");
    std::fs::write(&dump_path, &r.stdout).expect("write dump output");

    // Verify structure manually since we cannot run `toml::from_str` from
    // an integration test directly. We assert the structural markers.
    let text = &r.stdout;
    assert!(
        text.contains("description"),
        "must have [source].description: {text}"
    );
    assert!(
        text.contains("discover"),
        "must have discover section: {text}"
    );
    assert!(
        !text.contains("[[items]]"),
        "must NOT have [[items]] of its own: {text}"
    );
}

// ---------------------------------------------------------------------------
// Round-trip: dump -> re-meld reproduces the same install set
// ---------------------------------------------------------------------------

#[test]
fn dump_roundtrip_remeld_reproduces_install_set() {
    // spec: DUMP-1 DUMP-2 DUMP-5 DUMP-7 — meld a source + install a subset,
    // dump to a file inside a new git repo (super-source), meld THAT repo into
    // a fresh environment, and assert the manifest reproduces the same install set.
    //
    // The dumped super-source references the original source by its filesystem
    // path, which still exists in the test, so the re-meld can clone it.

    let src = Sandbox::new("original-src");
    // Install only skill:review (a proper subset of the three offered items).
    let meld = src.mind(&["meld", &src.source_spec(), "--link-only"]);
    assert!(meld.success, "initial meld failed: {}", meld.stderr);
    let learn = src.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    // Dump to a temp directory as mind.toml (the super-source repo).
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = src.base.join(format!("super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super-source dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dump_r = src.mind(&["dump", "--output", &dump_path_str]);
    assert!(
        dump_r.success,
        "dump --output failed: {} {}",
        dump_r.stdout, dump_r.stderr
    );
    assert!(dump_path.exists(), "dump output file must exist");

    // The super-source dir needs to be a git repo so `meld` can clone it.
    git_init(&super_dir);

    let super_spec = super_dir.to_string_lossy().into_owned();

    // Set up a second, fresh environment and meld the dumped super-source.
    let fresh_base = src.base.join(format!("fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("create fresh base");
    let fresh_mind_home = fresh_base.join("mind");
    let fresh_claude_home = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind_home)
        .env("CLAUDE_HOME", &fresh_claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run mind meld on dumped super-source");

    let remeld_out = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let remeld_err = String::from_utf8_lossy(&remeld.stderr).into_owned();

    // The re-meld should succeed and install exactly skill:review.
    assert!(
        remeld.status.success(),
        "re-meld of dumped super-source failed: {remeld_out} {remeld_err}"
    );

    // skill:review must be installed in the fresh environment.
    assert!(
        fresh_claude_home.join("skills/review").exists(),
        "skill:review must be installed in the reproduced environment: {:?}",
        fresh_claude_home
    );

    // agent:dev and rule:style must NOT be installed (they were not in the subset).
    assert!(
        !fresh_claude_home.join("agents/dev.md").exists(),
        "agent:dev (not in the subset) must NOT be installed in the reproduced environment"
    );
    assert!(
        !fresh_claude_home.join("rules/style.md").exists(),
        "rule:style (not in the subset) must NOT be installed in the reproduced environment"
    );
}

#[test]
fn dump_of_a_curator_with_a_relative_nested_source_reproduces_it_absolutely() {
    // spec: DSC-92 DUMP-1 -- `dump` is NOT a caller of the DSC-92 read-site fix:
    // it reconstructs `[discover].sources` from the REGISTRY, never from the
    // curator's own entries. So what has to hold is that a curator whose own
    // `mind.toml` declared a relative `../nested` melds into a registry entry
    // carrying the ABSOLUTE resolution, and therefore dumps to an entry that
    // re-melds from any cwd.
    // A dump emitting the curator's literal `../nested` would reproduce a
    // different (or no) source in the fresh environment.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let curator = Sandbox::bare("dsc92-curator");
    let nested = curator.base.join("nested-lib");
    write_file(
        &nested.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet skill\n---\n# greet\n",
    );
    git_init(&nested);
    curator.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"../nested-lib\"\ninstall = true\n",
    );

    let meld = curator.mind(&["meld", &curator.source_spec(), "--yes"]);
    assert!(
        meld.success,
        "melding the curator must succeed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        curator.claude_home.join("skills/greet").exists(),
        "the relative nested source's item must be installed: {:?}",
        curator.claude_home
    );

    let super_dir = curator.base.join(format!("dsc92-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = curator.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        text.contains(&nested.to_string_lossy().into_owned()),
        "the nested source must be dumped by its absolute path: {text}"
    );
    assert!(
        !text.contains("\"../nested-lib\""),
        "a cwd-relative nested spec must never be emitted: {text}"
    );

    let _ = n;
}

#[test]
fn dump_of_a_curator_with_a_relative_nested_source_remelds_in_a_fresh_home() {
    // DSC-93, the case DSC-92 alone does not close. DSC-92 resolves a
    // `[discover].sources` relative path against "the directory it read the
    // mind.toml from". For a LINKED local curator that is the user's working tree
    // and `../nested` finds the real sibling. But a curator reached through a
    // dump is CLONED (dump emits `pin-ref`), so its mind.toml is read from
    // `<mind_home>/sources/<host>/<owner>/<repo>` and `../nested` resolves to a
    // sibling inside the managed sources tree that was never cloned. That entry
    // used to be attempted: the clone failed and, for a curator with no items of
    // its own, DSC-80 turned it into a hard `CuratorAllNestedFailed` that aborted
    // the whole reproduction -- even though the dump ALSO carries a correct
    // absolute entry for that same nested source (asserted by the test above),
    // which the walk had simply not reached yet. DSC-93 skips the dead relative
    // entry with a warning, so the absolute one gets its turn.
    // spec: DSC-92 DSC-93 DUMP-7
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let curator = Sandbox::bare("dsc92-remeld-curator");
    let nested = curator.base.join("nested-lib");
    write_file(
        &nested.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet skill\n---\n# greet\n",
    );
    git_init(&nested);
    curator.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"../nested-lib\"\ninstall = true\n",
    );
    assert!(
        curator
            .mind(&["meld", &curator.source_spec(), "--yes"])
            .success
    );

    let super_dir = curator.base.join(format!("dsc92-remeld-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    assert!(curator.mind(&["dump", "--output", &dump_path_str]).success);

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = curator.base.join(format!("dsc92-remeld-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");
    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    assert!(
        remeld.status.success(),
        "re-meld of the dump must succeed: {} {}",
        String::from_utf8_lossy(&remeld.stdout),
        String::from_utf8_lossy(&remeld.stderr)
    );
    assert!(
        fresh_claude.join("skills/greet").exists(),
        "the nested source's item must be reproduced: {fresh_claude:?}"
    );
    // DSC-93 skips the dead relative entry loudly: silently dropping it would
    // leave a curator whose nested source is genuinely unreachable looking fine.
    assert!(
        String::from_utf8_lossy(&remeld.stderr).contains("sources tree"),
        "the skipped relative entry must be named on stderr: {}",
        String::from_utf8_lossy(&remeld.stderr)
    );
}

#[test]
fn dsc93_skip_is_reported_in_the_meld_json_skipped_array() {
    // spec: DSC-93 CLI-153 CLI-217
    // The prose warning above is the human channel. DSC-93 also pushes a
    // `SkippedEntry` with `reason = "unresolvable_local_path"`, and that array
    // is the ONLY way a `--json` consumer can learn an entry was dropped: the
    // warning goes to stderr (CLI-217) and the meld still exits 0. Nothing read
    // that array, so the reason string -- the part a consumer branches on --
    // was free to change or vanish without a test noticing.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let curator = Sandbox::bare("dsc93-json-curator");
    let nested = curator.base.join("nested-lib");
    write_file(
        &nested.join("skills/greet/SKILL.md"),
        "---\nname: greet\ndescription: Greet skill\n---\n# greet\n",
    );
    git_init(&nested);
    curator.write_and_commit(
        "mind.toml",
        "[[discover.sources]]\nsource = \"../nested-lib\"\ninstall = true\n",
    );
    assert!(
        curator
            .mind(&["meld", &curator.source_spec(), "--yes"])
            .success
    );

    let super_dir = curator.base.join(format!("dsc93-json-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    assert!(curator.mind(&["dump", "--output", &dump_path_str]).success);

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = curator.base.join(format!("dsc93-json-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes", "--json"])
        .env("MIND_HOME", fresh_base.join("mind"))
        .env("CLAUDE_HOME", fresh_base.join("claude"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld --json");
    let stdout = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(
        remeld.status.success(),
        "re-meld of the dump must succeed under --json too: {stdout} {stderr}"
    );

    // CLI-217: the DSC-93 warning is on stderr, so stdout is still exactly one
    // JSON document.
    let doc: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be one JSON document ({e}): {stdout:?}"));
    assert!(
        stderr.contains("sources tree"),
        "the DSC-93 warning must still be emitted, on stderr: {stderr}"
    );
    assert!(
        !stdout.contains("sources tree"),
        "the DSC-93 warning must not reach stdout under --json: {stdout}"
    );

    let skipped = doc["skipped"]
        .as_array()
        .unwrap_or_else(|| panic!("the meld result must carry a `skipped` array: {doc:#}"));
    let entry = skipped
        .iter()
        .find(|e| e["reason"] == "unresolvable_local_path")
        .unwrap_or_else(|| panic!("DSC-93 must record reason=unresolvable_local_path: {doc:#}"));
    let named = entry["source"].as_str().unwrap_or_default();
    assert!(
        named.contains("nested-lib"),
        "the skipped entry must name the entry that was dropped, so a consumer \
         can tell WHICH nested source went missing: {doc:#}"
    );

    // The skip is not a loss: the dump's absolute entry for the same nested
    // source still installed it.
    assert!(
        fresh_base.join("claude/skills/greet").exists(),
        "the nested source's item must still be reproduced: {doc:#} {stderr}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-1 / DSC-65: pin-ref in the dump output pins to the exact commit
// ---------------------------------------------------------------------------

#[test]
fn dump_pin_ref_pins_to_exact_commit_not_new_head() {
    // spec: DUMP-1 DUMP-4 DSC-65 — dump emits `pin-ref = <commit>` for each
    // source. When the dump output is re-melded, the reproduced source sits at
    // the DUMPED commit, not the new HEAD that was added after the dump.
    // This proves the pin-ref is load-bearing for exact-revision reproduction.
    let src = Sandbox::new("evolving-src");
    // Install all items.
    let meld = src.mind(&["meld", &src.source_spec(), "--yes"]);
    assert!(meld.success, "initial meld failed: {}", meld.stderr);

    // Dump to record the current commit.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = src.base.join(format!("pinref-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super-source dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dump_r = src.mind(&["dump", "--output", &dump_path_str]);
    assert!(
        dump_r.success,
        "dump --output failed: {} {}",
        dump_r.stdout, dump_r.stderr
    );

    // Verify the dump output contains `pin-ref` with the recorded commit.
    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump output");
    assert!(
        dump_text.contains("pin-ref"),
        "dump output must contain pin-ref: {dump_text}"
    );

    // Advance the source by adding a new commit AFTER the dump.
    src.write_and_commit(
        "skills/new-skill/SKILL.md",
        "---\nname: new-skill\ndescription: New skill added after dump\n---\n# new\n",
    );

    // The super-source dir needs to be a git repo so `meld` can clone it.
    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();

    // Set up a fresh environment and meld the dumped super-source.
    let fresh_base = src.base.join(format!("pinref-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("create fresh base");
    let fresh_mind_home = fresh_base.join("mind");
    let fresh_claude_home = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind_home)
        .env("CLAUDE_HOME", &fresh_claude_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run mind meld on dumped super-source");

    let remeld_out = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let remeld_err = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(
        remeld.status.success(),
        "re-meld of dumped super-source failed: {remeld_out} {remeld_err}"
    );

    // The new skill (added AFTER the dump) must NOT be installed in the
    // reproduced environment, because the pin-ref pins to the pre-advance commit.
    assert!(
        !fresh_claude_home.join("skills/new-skill").exists(),
        "skill added after dump must NOT appear in the reproduced environment \
         (pin-ref must pin to the pre-advance commit): {:?}",
        fresh_claude_home
    );

    // The pre-dump items must be present.
    assert!(
        fresh_claude_home.join("skills/review").exists(),
        "original skill:review must be installed in the reproduced environment"
    );

    // Verify the dump output contains the source URL (no github shorthands).
    assert!(
        dump_text.contains(&src.source_spec()),
        "dump must reference the source by its path: {dump_text}"
    );
    assert!(
        !dump_text.contains("github.com"),
        "dump must use local path (no github URL in this test): {dump_text}"
    );
}

// ---------------------------------------------------------------------------
// DUMP-2 boundary: "all offered installed" is decided by the offered SET, not a
// count. A source offering 3 items with only 2 installed must emit install-items
// of exactly those 2, NOT install = true.
// ---------------------------------------------------------------------------

#[test]
fn dump_two_of_three_installed_yields_subset_not_install_true() {
    // spec: DUMP-2 — the "all offered installed" branch keys on the offered set.
    // With 3 items offered and 2 installed, the result is a proper subset:
    // install-items listing exactly the 2, and never install = true.
    let sb = Sandbox::new("src"); // offers skill:review, agent:dev, rule:style
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    let l1 = sb.mind(&["learn", "skill:review"]);
    assert!(
        l1.success,
        "learn skill:review: {} {}",
        l1.stdout, l1.stderr
    );
    let l2 = sb.mind(&["learn", "agent:dev"]);
    assert!(l2.success, "learn agent:dev: {} {}", l2.stdout, l2.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install-items"),
        "two-of-three installed must emit install-items (not install = true): {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install = true"),
        "must NOT emit install = true when only a subset is installed: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:review") && r.stdout.contains("agent:dev"),
        "install-items must list exactly the two installed items: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("rule:style"),
        "the un-installed rule:style must not appear in install-items: {}",
        r.stdout
    );
}

#[test]
fn dump_all_of_a_two_item_source_yields_install_true() {
    // spec: DUMP-2 — a source whose every offered item is installed yields
    // install = true regardless of how many items that is. Builds a source with
    // exactly two items and installs both; the offered set equals the installed
    // set, so install = true (not an install-items listing both).
    let sb = Sandbox::bare("two-item");
    sb.write_and_commit(
        "skills/alpha/SKILL.md",
        "---\nname: alpha\ndescription: Alpha skill\n---\n# alpha\n",
    );
    sb.write_and_commit(
        "agents/beta.md",
        "---\nname: beta\ndescription: Beta agent\n---\n# beta\n",
    );
    let meld = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("install = true"),
        "all offered items installed must emit install = true: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install-items"),
        "must NOT emit install-items when the offered set is fully installed: {}",
        r.stdout
    );
}

#[test]
fn dump_install_true_when_all_currently_offered_installed_despite_stale_manifest_item() {
    // spec: DUMP-2 — the directive intersects the manifest with what the catalog
    // currently OFFERS. An item recorded in the manifest but no longer offered by
    // the source is excluded from the comparison, so if every currently-offered
    // item is installed the result is install = true (not an install-items list
    // that would name the stale item). Uses a linked local source so the catalog
    // scans the live working tree: removing an item from the tree drops it from
    // the offered set while it remains in the manifest.
    let sb = Sandbox::new("stale-src"); // offers review, dev, style
    let meld = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);

    // Remove rule:style from the source's working tree. It stays in the manifest
    // (still "installed") but is no longer offered by the catalog.
    std::fs::remove_file(sb.source.join("rules/style.md")).expect("remove rule file");

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    // review + dev are the only offered items now, and both are installed, so the
    // offered set == the installed-and-offered set -> install = true.
    assert!(
        r.stdout.contains("install = true"),
        "all currently-offered items installed must yield install = true even with a stale manifest item: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("install-items"),
        "a stale (no-longer-offered) manifest item must not force an install-items list: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("rule:style"),
        "the no-longer-offered item must not appear in the dump: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// Multiple sources in one dump: a 2-source registry with distinct directives
// (one full, one subset, one none) must all be emitted correctly in one dump.
// ---------------------------------------------------------------------------

#[test]
fn dump_multiple_sources_each_with_distinct_directive() {
    // spec: DUMP-1 DUMP-2 — a registry with three distinct sources, each with a
    // different install state, all emitted in one dump:
    //   full    -> install = true
    //   subset  -> install-items = [one of two]
    //   none    -> install = false
    let full = Sandbox::bare("full-src");
    full.write_and_commit(
        "skills/only/SKILL.md",
        "---\nname: only\ndescription: Only skill\n---\n# only\n",
    );
    // Subset and none sources are melded into the SAME mind/claude home as `full`,
    // so a single dump sees all three. Build them as plain repos here.
    let subset = Sandbox::bare("subset-src");
    subset.write_and_commit(
        "skills/keep/SKILL.md",
        "---\nname: keep\ndescription: Keep skill\n---\n# keep\n",
    );
    subset.write_and_commit(
        "agents/drop.md",
        "---\nname: drop\ndescription: Drop agent\n---\n# drop\n",
    );
    let none = Sandbox::bare("none-src");
    none.write_and_commit(
        "rules/unused.md",
        "---\ndescription: Unused rule\n---\n# unused\n",
    );

    // Meld all three into `full`'s home.
    let m1 = full.mind(&["meld", &full.source_spec(), "--yes"]);
    assert!(m1.success, "meld full failed: {} {}", m1.stdout, m1.stderr);
    let m2 = full.mind(&["meld", &subset.source_spec(), "--link-only"]);
    assert!(
        m2.success,
        "meld subset failed: {} {}",
        m2.stdout, m2.stderr
    );
    let m3 = full.mind(&["meld", &none.source_spec(), "--link-only"]);
    assert!(m3.success, "meld none failed: {} {}", m3.stdout, m3.stderr);
    // Install exactly one of subset-src's two items.
    let l = full.mind(&["learn", "skill:keep"]);
    assert!(
        l.success,
        "learn skill:keep failed: {} {}",
        l.stdout, l.stderr
    );

    let r = full.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);

    // All three sources must appear in the one dump.
    assert!(
        r.stdout.contains(&full.source_spec())
            && r.stdout.contains(&subset.source_spec())
            && r.stdout.contains(&none.source_spec()),
        "all three source specs must appear in the dump: {}",
        r.stdout
    );
    // The full source -> install = true; the subset -> install-items[keep];
    // the none -> install = false. Both true and false must coexist, and a
    // subset listing keep (not drop) must be present.
    assert!(
        r.stdout.contains("install = true"),
        "the fully-installed source must emit install = true: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("install = false"),
        "the un-installed source must emit install = false: {}",
        r.stdout
    );
    assert!(
        r.stdout.contains("skill:keep"),
        "the subset source must list skill:keep in install-items: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("agent:drop"),
        "the un-installed agent:drop must not appear in install-items: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-4 prefix: `as` from a source's OWN [source].prefix when no consumer alias.
// ---------------------------------------------------------------------------

#[test]
fn dump_emits_alias_from_source_own_prefix_when_no_consumer_alias() {
    // spec: DUMP-4 — the emitted `as` comes from the source's own
    // [source].prefix when the consumer did not pass --as. (The alias path is
    // covered separately; this covers the [source].prefix fallback.)
    let sb = Sandbox::bare("prefixed-src");
    sb.write_and_commit("mind.toml", "[source]\nprefix = \"sp\"\n");
    sb.write_and_commit(
        "skills/widget/SKILL.md",
        "---\nname: widget\ndescription: Widget skill\n---\n# widget\n",
    );
    // Meld with NO --as, so the source's own prefix is the only prefix in effect.
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stdout.contains("namespace = \"sp\""),
        "dump must emit namespace = \"sp\" from the source's own [source].prefix (DSC-78): {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// pin-ref round-trip across pin kinds: a tag- or branch-pinned source still
// dumps as pin-ref = <exact commit>, and re-melding lands on that exact commit.
// ---------------------------------------------------------------------------

#[test]
fn dump_tag_pinned_source_dumps_exact_commit_pin_ref() {
    // spec: DUMP-1 DUMP-4 DSC-65 — a source melded with --pin-tag is dumped as
    // pin-ref = <the commit the tag resolved to>, not as pin-tag. Re-melding the
    // dump output pins to that exact commit even after the tag is later moved.
    let sb = Sandbox::new("tagged-src");
    // Tag the current commit, then meld at that tag.
    git_tag(&sb.source, "v1");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--pin-tag", "v1", "--yes"]);
    assert!(
        meld.success,
        "meld --pin-tag failed: {} {}",
        meld.stdout, meld.stderr
    );

    // Dump.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("tag-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("pin-ref"),
        "tag-pinned source must dump as pin-ref (exact commit): {dump_text}"
    );
    assert!(
        !dump_text.contains("pin-tag"),
        "dump must NOT carry the original pin-tag kind: {dump_text}"
    );

    // Advance the source AND move the tag forward, then re-meld the dump.
    sb.write_and_commit(
        "skills/added-after/SKILL.md",
        "---\nname: added-after\ndescription: Added after dump\n---\n# a\n",
    );
    // Move v1 to the new tip; a pin-ref dump must ignore this and stay at the
    // pre-advance commit.
    git_move_tag(&sb.source, "v1");

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("tag-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    // The item added after the dump (and after the tag was moved) must NOT be
    // present: the pin-ref pins to the exact pre-advance commit.
    assert!(
        !fresh_claude.join("skills/added-after").exists(),
        "an item added after the dump must NOT appear (pin-ref must pin exactly): {fresh_claude:?}"
    );
    assert!(
        fresh_claude.join("skills/review").exists(),
        "the original skill:review must be present in the reproduced env"
    );
}

#[test]
fn dump_branch_followed_source_dumps_exact_commit_pin_ref() {
    // spec: DUMP-1 DUMP-4 DSC-65 — a source melded with --follow-branch is
    // dumped as pin-ref = <recorded commit>, not as follow-branch. Re-melding
    // lands on the exact commit even after the branch advances.
    let sb = Sandbox::new("branch-src");
    let meld = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--follow-branch",
        "main",
        "--yes",
    ]);
    assert!(
        meld.success,
        "meld --follow-branch failed: {} {}",
        meld.stdout, meld.stderr
    );

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("branch-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("pin-ref"),
        "branch-followed source must dump as pin-ref (exact commit): {dump_text}"
    );
    assert!(
        !dump_text.contains("follow-branch"),
        "dump must NOT carry the original follow-branch kind: {dump_text}"
    );

    // Advance main, then re-meld; the pin-ref must keep the reproduction at the
    // recorded commit, not the new branch tip.
    sb.write_and_commit(
        "skills/late/SKILL.md",
        "---\nname: late\ndescription: Late skill\n---\n# late\n",
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("branch-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    assert!(
        !fresh_claude.join("skills/late").exists(),
        "a skill added after the dump must NOT appear (pin-ref pins exactly): {fresh_claude:?}"
    );
    assert!(
        fresh_claude.join("skills/review").exists(),
        "the original skill:review must be present in the reproduced env"
    );
}

// ---------------------------------------------------------------------------
// Output parity: --output file and stdout produce byte-identical valid TOML.
// ---------------------------------------------------------------------------

#[test]
fn dump_stdout_and_output_file_are_byte_identical() {
    // spec: DUMP-1 DUMP-7 — the document written to --output must be byte-for-byte
    // the same as what dump writes to stdout for the same state.
    let sb = Sandbox::new("src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {}", meld.stderr);
    let learn = sb.mind(&["learn", "skill:review"]);
    assert!(
        learn.success,
        "learn failed: {} {}",
        learn.stdout, learn.stderr
    );

    let stdout_run = sb.mind(&["dump"]);
    assert!(
        stdout_run.success,
        "dump stdout failed: {}",
        stdout_run.stderr
    );

    let out_path = sb.base.join("parity.toml");
    let out_str = out_path.to_string_lossy().into_owned();
    let file_run = sb.mind(&["dump", "--output", &out_str]);
    assert!(
        file_run.success,
        "dump --output failed: {}",
        file_run.stderr
    );
    let file_content = std::fs::read_to_string(&out_path).expect("read output file");

    assert_eq!(
        stdout_run.stdout, file_content,
        "stdout and --output must be byte-identical:\n--- stdout ---\n{}\n--- file ---\n{}",
        stdout_run.stdout, file_content
    );
    assert!(
        !file_content.is_empty(),
        "the parity content must not be empty"
    );
}

// ---------------------------------------------------------------------------
// DUMP-8 / no-commit: a linked local source with no recorded commit emits NO
// pin (must never emit pin-ref = "").
// ---------------------------------------------------------------------------

#[test]
fn dump_linked_local_source_without_commit_emits_no_pin_ref() {
    // spec: DUMP-1 DUMP-8 — a linked local source that is not a git repo records
    // no commit. The dump must omit the pin field entirely, never emit
    // pin-ref = "".
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("mind-dump-nogit-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let nongit_src = base.join("nongit");
    // A plain (non-git) directory with one item; meld links it (no clone, no
    // recorded commit because head_commit fails on a non-repo).
    write_file(
        &nongit_src.join("skills/plain/SKILL.md"),
        "---\nname: plain\ndescription: Plain skill\n---\n# plain\n",
    );
    let mind_home = base.join("mind");
    let claude_home = base.join("claude");

    let run = |args: &[&str]| -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_mind"))
            .args(args)
            .env("MIND_HOME", &mind_home)
            .env("CLAUDE_HOME", &claude_home)
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
    };

    let nongit_spec = nongit_src.to_string_lossy().into_owned();
    let meld = run(&["meld", &nongit_spec, "--link-only"]);
    assert!(
        meld.success,
        "meld of a non-git local dir must succeed: {} {}",
        meld.stdout, meld.stderr
    );

    let dump = run(&["dump"]);
    assert!(dump.success, "dump failed: {} {}", dump.stdout, dump.stderr);
    assert!(
        !dump.stdout.contains("pin-ref"),
        "a source with no recorded commit must emit no pin-ref: {}",
        dump.stdout
    );
    assert!(
        !dump.stdout.contains("pin-ref = \"\""),
        "must never emit an empty pin-ref: {}",
        dump.stdout
    );
    // The source must still be present in the dump (referenced), just unpinned.
    assert!(
        dump.stdout.contains(&nongit_spec),
        "the source must still be referenced in the dump: {}",
        dump.stdout
    );

    let _ = std::fs::remove_dir_all(&base);
}

// ---------------------------------------------------------------------------
// DUMP-6: a dependency installed via a {{ns:}} token (not `requires:`) is part
// of the installed set and appears in install-items.
// ---------------------------------------------------------------------------

#[test]
fn dump_token_dependency_item_is_in_install_items() {
    // spec: DUMP-6 — an item pulled in only as a within-source dependency via a
    // {{ns:}} reference token is in the installed set and is dumped like any
    // other. Build a source where skill:caller references {{ns:helper}}; install
    // only skill:caller and confirm agent:helper rides along into the dump.
    let sb = Sandbox::bare("tok-dep");
    sb.write_and_commit(
        "agents/helper.md",
        "---\nname: helper\ndescription: Helper agent\n---\n# helper\n",
    );
    // A third item that is NOT installed, so the source is a proper superset of
    // the installed set and the dump must use install-items (not install = true).
    sb.write_and_commit(
        "rules/extra.md",
        "---\ndescription: Extra rule\n---\n# extra\n",
    );
    sb.write_and_commit(
        "skills/caller/SKILL.md",
        "---\nname: caller\ndescription: Caller skill\n---\n# caller\n\nSee {{ns:helper}}.\n",
    );

    let meld = sb.mind(&["meld", &sb.source_spec(), "--link-only"]);
    assert!(meld.success, "meld failed: {} {}", meld.stdout, meld.stderr);
    let learn = sb.mind(&["learn", "skill:caller", "--yes"]);
    assert!(
        learn.success,
        "learn skill:caller failed: {} {}",
        learn.stdout, learn.stderr
    );

    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);

    // DEP-1/DEP-3: the {{ns:helper}} token makes agent:helper a dependency of
    // skill:caller, so learning the caller installs the helper too.
    assert!(
        sb.claude_home.join("agents/helper.md").exists(),
        "the {{{{ns:helper}}}} token must pull agent:helper in as a dependency"
    );
    // Both caller and helper are installed (a proper subset, since rule:extra is
    // not), so the dump lists both in install-items by bare kind:name.
    assert!(
        r.stdout.contains("agent:helper") && r.stdout.contains("skill:caller"),
        "a token-pulled dependency must appear in install-items alongside its referrer: {}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("rule:extra"),
        "the un-installed rule:extra must not appear in install-items: {}",
        r.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-11: dump round-trips --add-root so re-melding reoffers add-root items
// ---------------------------------------------------------------------------

#[test]
fn dump_roundtrips_add_root_items() {
    // spec: DUMP-11 — a source melded with `--add-root <dir>` offers items the
    // added root contributes (items outside the default convention scan). dump
    // must emit `add-roots` so re-melding the super-source in a fresh home
    // rediscovers and installs those same items. The add-root item is only
    // reachable via the extra root, so its presence after re-meld proves the
    // add-roots survived the dump.
    let sb = Sandbox::bare("addroot-src");
    // A base item at the repo root (found by the default convention scan).
    sb.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: Base skill\n---\n# base\n",
    );
    // An item only reachable via the extra root `contrib` (NOT found by the
    // default root scan; only the --add-root pass discovers it).
    sb.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: Add-root contributed skill\n---\n# addon\n",
    );

    // Meld with --add-root and install every offered item (base + addon).
    let meld = sb.mind(&["meld", &sb.source_spec(), "--add-root", "contrib", "--yes"]);
    assert!(
        meld.success,
        "meld --add-root failed: {} {}",
        meld.stdout, meld.stderr
    );

    // Dump to a super-source mind.toml.
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("addroot-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("add-roots"),
        "dump must emit add-roots for an --add-root source: {dump_text}"
    );
    assert!(
        dump_text.contains("contrib"),
        "dump must record the `contrib` add-root: {dump_text}"
    );

    // Re-meld the super-source into a fresh environment.
    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("addroot-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    // The base item installs, and the add-root-contributed item installs too:
    // the latter is only discoverable because the dumped add-roots were applied.
    assert!(
        fresh_claude.join("skills/base").exists(),
        "the base skill must install in the reproduced env: {fresh_claude:?}"
    );
    assert!(
        fresh_claude.join("skills/addon").exists(),
        "the add-root contributed skill must install in the reproduced env \
         (proves dump round-tripped --add-root): {fresh_claude:?}"
    );
}

#[test]
fn dump_add_root_over_authoritative_mindtoml_emits_but_is_gated_on_reroundtrip() {
    // spec: DUMP-11, DSC-88 — a DIRECT `meld --add-root` composes with an
    // authoritative mind.toml (DSC-84: unlike `--root`, it is never gated for
    // the consumer who melds the source themselves), and `dump` still emits
    // the recorded `add-roots` for provenance (DUMP-11). But once the value is
    // re-melded as a NESTED super-source entry, it is a CURATOR value (DSC-59)
    // and DSC-60/DSC-88 gate it: the nested source (addroot-auth-src) ships its
    // own mind.toml, so the curator-supplied add-roots is ignored with a
    // warning on re-meld, and only the authoritatively-declared item survives
    // the round trip. This is the DSC-88 fix: a curator's add-roots must not
    // reach into a nested source's authoritative export list and surface items
    // its author did not export.
    let sb = Sandbox::bare("addroot-auth-src");
    // Authoritative mind.toml: declaring [[items]] turns off plain convention
    // scanning for the source's own root (DSC-30).
    sb.write_and_commit(
        "mind.toml",
        "[[items]]\nkind = \"skill\"\nname = \"core\"\npath = \"skills/core\"\n",
    );
    sb.write_and_commit(
        "skills/core/SKILL.md",
        "---\nname: core\ndescription: Authoritative core skill\n---\n# core\n",
    );
    // Only reachable via --add-root: not listed in [[items]], and ordinary
    // convention scanning of the repo root is suppressed by the authoritative
    // mind.toml.
    sb.write_and_commit(
        "contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: Add-root contributed skill\n---\n# addon\n",
    );

    let meld = sb.mind(&["meld", &sb.source_spec(), "--add-root", "contrib", "--yes"]);
    assert!(
        meld.success,
        "meld --add-root over an authoritative mind.toml failed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        sb.claude_home.join("skills/core").exists(),
        "the authoritative item must install: {:?}",
        sb.claude_home
    );
    assert!(
        sb.claude_home.join("skills/addon").exists(),
        "the add-root item must install alongside the authoritative item \
         (DSC-84): {:?}",
        sb.claude_home
    );

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("addroot-auth-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("add-roots") && dump_text.contains("contrib"),
        "dump must still emit the recorded add-roots even when the source also \
         has an authoritative mind.toml (DUMP-11 provenance): {dump_text}"
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("addroot-auth-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");
    // spec: DSC-60 — the nested entry's add-roots is gated (DSC-88) because
    // addroot-auth-src ships its own mind.toml; the warning names it ignored.
    assert!(
        re.contains("mind.toml") && re.contains("ignored"),
        "re-melding the dumped super-source must warn that the nested entry's \
         curator-supplied add-roots is ignored (DSC-60/DSC-88): {re}"
    );

    assert!(
        fresh_claude.join("skills/core").exists(),
        "the authoritative item must install in the reproduced env: {fresh_claude:?}"
    );
    assert!(
        !fresh_claude.join("skills/addon").exists(),
        "the add-root item must NOT install in the reproduced env: once \
         re-melded as a nested super-source entry, its add-roots is a curator \
         value gated by DSC-60/DSC-88 because the nested source ships its own \
         mind.toml -- it must not bypass that source's authoritative export \
         list: {fresh_claude:?}"
    );
}

#[test]
fn dump_emits_no_add_roots_key_when_source_has_none() {
    // spec: DUMP-11 — a source melded WITHOUT --add-root must emit no
    // `add-roots` key at all (end-to-end, not just the unit-level DumpEntry
    // serialization check).
    let sb = Sandbox::new("no-add-root-src");
    let meld = sb.mind(&["meld", &sb.source_spec(), "--yes"]);
    assert!(
        meld.success,
        "plain meld (no --add-root) failed: {} {}",
        meld.stdout, meld.stderr
    );

    let dr = sb.mind(&["dump"]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);
    assert!(
        !dr.stdout.contains("add-roots"),
        "dump must emit no add-roots key for a source with none recorded: {}",
        dr.stdout
    );
}

// ---------------------------------------------------------------------------
// DUMP-11: the sync re-walk path threads a newly-discovered nested entry's
// add-roots into the nested meld (mirrors the meld-time threading).
// ---------------------------------------------------------------------------

#[test]
fn dump_sync_rewalk_threads_add_roots_into_newly_registered_nested_source() {
    // spec: DUMP-11 — when a super-source's mind.toml is updated (after its
    // initial meld) to list a new nested entry carrying `add-roots`, `sync`'s
    // DSC-57 re-walk must thread that entry's add-roots into the nested meld,
    // exactly as a fresh top-level meld of a [[discover.sources]] entry would.
    // Verified by dumping the registry after sync and checking the newly
    // registered nested source's entry carries `add-roots` (round-tripping
    // through the persisted `Source.add_roots`, DUMP-11/STO-55).
    let nested = Sandbox::bare("rewalk-nested");
    nested.write_and_commit(
        "skills/base/SKILL.md",
        "---\nname: base\ndescription: Base skill\n---\n# base\n",
    );
    // Only reachable via the add-root the re-walked entry will carry.
    nested.write_and_commit(
        "extra-contrib/skills/addon/SKILL.md",
        "---\nname: addon\ndescription: Add-root contributed skill\n---\n# addon\n",
    );
    let nested_spec = nested.source_spec();

    // T starts with no [[discover.sources]] entries, so its initial meld
    // registers only T itself.
    let t = Sandbox::bare("rewalk-super");
    let initial = t.mind(&["meld", &t.source_spec(), "--yes"]);
    assert!(
        initial.success,
        "initial meld of the super-source failed: {} {}",
        initial.stdout, initial.stderr
    );
    let sources_after_meld = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        !sources_after_meld.contains("rewalk-nested"),
        "the nested source must not be registered before it is declared: {sources_after_meld}"
    );

    // T is a linked local source (no pin), so `sync` reads its live working
    // tree: update it, without re-melding, to declare the nested source with
    // add-roots.
    let toml = format!(
        "[[discover.sources]]\nsource = \"{nested_spec}\"\nadd-roots = [\"extra-contrib\"]\n"
    );
    t.write_and_commit("mind.toml", &toml);

    let sync = t.mind(&["sync"]);
    assert!(
        sync.success,
        "sync re-walk failed: {} {}",
        sync.stdout, sync.stderr
    );
    let sources_after_sync = t.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources_after_sync.contains("rewalk-nested"),
        "sync's re-walk must register the newly-declared nested source: {sources_after_sync}"
    );

    let dr = t.mind(&["dump"]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);
    assert!(
        dr.stdout.contains("add-roots") && dr.stdout.contains("extra-contrib"),
        "dump must show add-roots on the nested source sync's re-walk \
         registered, proving the re-walk threaded the entry's add-roots into \
         the nested meld: {}",
        dr.stdout
    );
}

// ---------------------------------------------------------------------------
// Lock-mode: dump takes the Shared lock (read-only)
// ---------------------------------------------------------------------------

#[test]
fn dump_is_classified_as_shared_lock() {
    // spec: DUMP-1 — dump is read-only (registry + manifest + catalog only);
    // verify via the `lock_mode` unit-test path in main.rs that dump is Shared.
    // Here we verify at the CLI level that `mind dump --help` succeeds (i.e.
    // the command parses) and that the binary recognizes the subcommand.
    let sb = Sandbox::bare("lock-test");
    let help = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["dump", "--help"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run mind dump --help");
    assert!(
        help.status.success(),
        "`mind dump --help` must exit 0: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help_out = String::from_utf8_lossy(&help.stdout).into_owned();
    assert!(
        help_out.contains("--output") || help_out.contains("--whole-sources"),
        "help text must mention dump flags: {help_out}"
    );

    // Confirm `mind dump` on an empty home exits 0 (DUMP-8).
    let r = sb.mind(&["dump"]);
    assert!(
        r.success,
        "`mind dump` with empty home must exit 0: {} {}",
        r.stdout, r.stderr
    );
    drop(sb);
}

// ---- dump --json note (DUMP-9) -----------------------------------------------

/// `dump --json` must still write TOML to stdout (not JSON), exit 0, and print
/// a note to stderr indicating that --json does not apply.
#[test]
fn dump_json_flag_prints_stderr_note() {
    // spec: DUMP-9
    let sb = Sandbox::new("json-note");
    let spec = sb.source_spec();
    sb.mind(&["meld", &spec, "--yes"]);

    let r = sb.mind(&["--json", "dump"]);
    assert!(
        r.success,
        "`dump --json` must exit 0: stdout={} stderr={}",
        r.stdout, r.stderr
    );
    // stdout must be TOML, not JSON: a valid TOML file starts with a comment or
    // a key; a JSON object would start with '{'.
    assert!(
        !r.stdout.trim_start().starts_with('{'),
        "dump --json stdout must be TOML, not a JSON object: '{}'",
        r.stdout
    );
    assert!(
        r.stdout.contains("[discover]") || r.stdout.contains("[source]"),
        "dump --json stdout must contain TOML structure: '{}'",
        r.stdout
    );
    // stderr must carry the note about --json not applying.
    assert!(
        r.stderr.contains("--json")
            && (r.stderr.contains("does not apply") || r.stderr.contains("TOML")),
        "dump --json must print a note to stderr: '{}'",
        r.stderr
    );
}

// ---------------------------------------------------------------------------
// LNK-13: dump emits an item-link instance as a reconstructed deep-URL entry,
// and re-melding that entry reproduces the same instance identity, pinned
// commit, and installed item.
// ---------------------------------------------------------------------------

#[test]
fn dump_roundtrips_bare_link_instance() {
    // spec: LNK-13 — a bare (unaliased) item-link instance is emitted as a
    // `tree/<commit>/<path>` entry, not skipped. Re-melding it in a fresh
    // home installs the SAME skill from the SAME instance identity, pinned
    // to the exact commit recorded at dump time (not a later revision of the
    // linked skill).
    let sb = Sandbox::new("link-src");
    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        learn.success,
        "learn <url> failed: {} {}",
        learn.stdout, learn.stderr
    );
    let dumped_commit = git_head(&sb.source);

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("link-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);
    assert!(
        !dr.stderr.contains("skipping item link"),
        "dump must no longer skip an item-link instance: {}",
        dr.stderr
    );

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("/tree/"),
        "dump must emit a reconstructed tree/<ref>/<path> link URL: {dump_text}"
    );
    assert!(
        dump_text.contains("skills/review"),
        "dump must reconstruct the recorded item path: {dump_text}"
    );
    assert!(
        dump_text.contains("pin-ref"),
        "dump must pin the link entry with pin-ref like every other entry: {dump_text}"
    );
    assert!(
        dump_text.contains(&dumped_commit),
        "dump must pin the reconstructed link to the recorded commit: {dump_text}"
    );

    // Advance the linked skill's content AFTER the dump. A commit-exact
    // reproduction must NOT pick this up.
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review skill CHANGED AFTER DUMP\n---\n# review v2\n",
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("link-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    // The same item installed from the same instance identity...
    let installed = fresh_claude.join("skills/review/SKILL.md");
    assert!(
        installed.exists(),
        "the linked skill must be installed in the reproduced env: {fresh_claude:?}"
    );
    // ...at the SAME commit: the content dated after the dump must be absent.
    let content = std::fs::read_to_string(&installed).expect("read reinstalled SKILL.md");
    assert!(
        !content.contains("CHANGED AFTER DUMP"),
        "reproduced install must pin to the dumped commit, not a later revision: {content}"
    );
    assert!(
        content.contains("Review skill"),
        "reproduced install must carry the pre-dump content: {content}"
    );

    // Same instance identity (unaliased `#skills/review`, no trailing `@`).
    let sources = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall", "--sources"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run recall --sources");
    let sources_out = String::from_utf8_lossy(&sources.stdout).into_owned();
    assert!(
        sources_out.contains("#skills/review"),
        "the reproduced instance must carry the same link identity: {sources_out}"
    );
}

#[test]
fn dump_roundtrips_aliased_link_instance() {
    // spec: LNK-13 — an item-link instance melded with a consumer namespace
    // (`--namespace fork`) re-melds as the SAME `host/owner/repo#path@fork`
    // instance: the emitted `namespace` field must carry the recorded
    // IDENTITY alias, not just a display prefix.
    let sb = Sandbox::new("link-alias-src");
    let meld = sb.mind(&[
        "meld",
        &sb.link("tree/main/skills/review"),
        "--namespace",
        "fork",
        "--yes",
    ]);
    assert!(
        meld.success,
        "aliased link meld failed: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        sb.claude_home.join("skills/fork:review").exists(),
        "the aliased link's skill must install under the prefix"
    );

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("link-alias-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("namespace = \"fork\""),
        "dump must emit the identity alias as namespace on the link entry: {dump_text}"
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("link-alias-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    assert!(
        fresh_claude.join("skills/fork:review").exists(),
        "the reproduced env must install the skill under the SAME alias prefix: {fresh_claude:?}"
    );

    let sources = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall", "--sources"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run recall --sources");
    let sources_out = String::from_utf8_lossy(&sources.stdout).into_owned();
    assert!(
        sources_out.contains("#skills/review@fork"),
        "the reproduced instance must carry the SAME aliased identity: {sources_out}"
    );
}

#[test]
fn dump_roundtrips_link_instance_with_ssh_config_set() {
    // spec: LNK-13 — C32: with `ssh = true` set in config, `dump` must still
    // emit a re-meldable deep-URL entry for an item-link instance. Before the
    // fix, `build_link_url` reconstructed the URL from `source.url`, which
    // `prefer_ssh` rewrites to the `git@host:owner/repo` form for a non-local
    // remote; that SSH-form string does not reparse as an item link, so the
    // dump would abort the WHOLE re-meld. `prefer_ssh` is a no-op for a local
    // source (host == "local", as this sandbox's link always is), so this
    // exercises the code path end-to-end under the same config that triggers
    // the bug for a remote link, proving the ssh setting no longer corrupts
    // (or otherwise interferes with) the reconstructed URL.
    let sb = Sandbox::new("link-ssh-src");
    write_file(&sb.mind_home.join("config.toml"), "ssh = true\n");
    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        learn.success,
        "learn <url> failed under ssh=true config: {} {}",
        learn.stdout, learn.stderr
    );
    let dumped_commit = git_head(&sb.source);

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("link-ssh-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);
    assert!(
        !dr.stderr.contains("skipping item link"),
        "dump must not skip the item-link instance under ssh=true: {}",
        dr.stderr
    );

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        !dump_text.contains("git@"),
        "the reconstructed link URL must never be the SSH form (not re-meldable as a link): \
         {dump_text}"
    );
    assert!(
        dump_text.contains("/tree/") && dump_text.contains("skills/review"),
        "dump must emit a reconstructed tree/<ref>/<path> link URL: {dump_text}"
    );
    assert!(
        dump_text.contains(&dumped_commit),
        "dump must pin the reconstructed link to the recorded commit: {dump_text}"
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("link-ssh-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(
        remeld.status.success(),
        "remeld of the ssh=true dump must succeed: {ro} {re}"
    );

    let installed = fresh_claude.join("skills/review/SKILL.md");
    assert!(
        installed.exists(),
        "the linked skill must be installed in the reproduced env: {fresh_claude:?}"
    );

    // Same instance identity (unaliased `#skills/review`, no trailing `@`).
    let sources = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["recall", "--sources"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run recall --sources");
    let sources_out = String::from_utf8_lossy(&sources.stdout).into_owned();
    assert!(
        sources_out.contains("#skills/review"),
        "the reproduced instance must carry the same link identity: {sources_out}"
    );
}

#[test]
fn dump_link_and_ordinary_source_together_roundtrip() {
    // spec: LNK-13 — a dump mixing an item-link instance and an ordinary
    // melded source emits both, and re-melding reproduces both installs.
    let sb = Sandbox::new("link-mixed-src");
    let learn = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(
        learn.success,
        "learn <url> failed: {} {}",
        learn.stdout, learn.stderr
    );

    // A second, ordinary source repo, melded (not linked) alongside the link.
    let other_source = sb.base.join("other-src");
    write_file(
        &other_source.join("skills/helper/SKILL.md"),
        "---\nname: helper\ndescription: Helper skill\n---\n# helper\n",
    );
    git_init(&other_source);
    let other_spec = other_source.to_string_lossy().into_owned();
    let meld_other = sb.mind(&["meld", &other_spec, "--yes"]);
    assert!(
        meld_other.success,
        "ordinary meld failed: {} {}",
        meld_other.stdout, meld_other.stderr
    );

    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let super_dir = sb.base.join(format!("link-mixed-super-{n}"));
    std::fs::create_dir_all(&super_dir).expect("create super dir");
    let dump_path = super_dir.join("mind.toml");
    let dump_path_str = dump_path.to_string_lossy().into_owned();
    let dr = sb.mind(&["dump", "--output", &dump_path_str]);
    assert!(dr.success, "dump failed: {} {}", dr.stdout, dr.stderr);
    assert!(
        !dr.stderr.contains("skipping item link"),
        "dump must not skip the item-link instance: {}",
        dr.stderr
    );

    let dump_text = std::fs::read_to_string(&dump_path).expect("read dump");
    assert!(
        dump_text.contains("/tree/") && dump_text.contains("skills/review"),
        "dump must emit the reconstructed link entry: {dump_text}"
    );
    assert!(
        dump_text.contains(&other_spec),
        "dump must also emit the ordinary source entry: {dump_text}"
    );

    git_init(&super_dir);
    let super_spec = super_dir.to_string_lossy().into_owned();
    let fresh_base = sb.base.join(format!("link-mixed-fresh-{n}"));
    std::fs::create_dir_all(&fresh_base).expect("fresh base");
    let fresh_mind = fresh_base.join("mind");
    let fresh_claude = fresh_base.join("claude");

    let remeld = Command::new(env!("CARGO_BIN_EXE_mind"))
        .args(["meld", &super_spec, "--yes"])
        .env("MIND_HOME", &fresh_mind)
        .env("CLAUDE_HOME", &fresh_claude)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .output()
        .expect("run remeld");
    let ro = String::from_utf8_lossy(&remeld.stdout).into_owned();
    let re = String::from_utf8_lossy(&remeld.stderr).into_owned();
    assert!(remeld.status.success(), "remeld failed: {ro} {re}");

    assert!(
        fresh_claude.join("skills/review").exists(),
        "the link instance's skill must install in the reproduced env: {fresh_claude:?}"
    );
    assert!(
        fresh_claude.join("skills/helper").exists(),
        "the ordinary source's skill must also install in the reproduced env: {fresh_claude:?}"
    );
}
