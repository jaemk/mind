//! Hardening of the catalog scan against source-controlled tree shapes that
//! are not items: a symlinked entry in a convention kind directory, an
//! `--add-root` skills container, or an `[[items]]` declared path (DSC-108), a
//! declared path escaping the clone through a symlinked parent (DSC-73), and a
//! declared `workflow` entry whose path is a directory (DSC-109).
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
    /// A source repo carrying one ordinary workflow, committed. Tests add the
    /// shape under test on top.
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("mind-scan-hardening-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join("agents");
        let sb = Sandbox {
            base: base.clone(),
            source: source.clone(),
            mind_home: base.join("mind"),
            claude_home: base.join("claude"),
        };
        write(
            &source.join("workflows/ship.js"),
            "export const meta = { name: 'ship', description: 'Ship it' };\n",
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
            .env_remove("MIND_MAX_METADATA_SIZE")
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

    fn commit(&self) {
        git(&self.source, &["add", "-A"]);
        git(&self.source, &["commit", "-qm", "fixture"]);
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

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_skill_anchor_is_not_a_discovery_oracle() {
    // The flat kinds are not the only classification the scan makes. A skill is
    // `skills/<name>/` plus a `SKILL.md` anchor, and following the ANCHOR link
    // is strictly worse than following a workflow's: the anchor is the file
    // whose frontmatter becomes the item's description, so a `SKILL.md` aimed
    // at an arbitrary `.md` on the consumer's machine both answers "does this
    // path exist" (the skill appears or does not) and republishes that file's
    // `description:` into the catalog.
    let sb = Sandbox::new();
    let outside = sb.base.join("outside/private.md");
    write(
        &outside,
        "---\ndescription: SECRETDESCRIPTION from outside the source\n---\n",
    );
    std::fs::create_dir_all(sb.source.join("skills/leak")).unwrap();
    std::os::unix::fs::symlink(&outside, sb.source.join("skills/leak/SKILL.md")).unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a source carrying a symlinked anchor must still meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        !listing.contains("leak"),
        "a skill whose SKILL.md is a symlink must not be discovered: {listing}"
    );
    assert!(
        !listing.contains("SECRETDESCRIPTION"),
        "a symlinked anchor must not republish a linked-to file's description: {listing}"
    );
    // It is also not installable, so discovery was the only surface that could
    // have leaked: the copy walk refuses a symlink in an item tree (LIFE-42).
    let learn = sb.mind(&["learn", "skill:leak"]);
    assert!(
        !learn.success,
        "a symlink-anchored skill must not be installable: {} {}",
        learn.stdout, learn.stderr
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_skill_directory_is_not_an_item() {
    // The other half of the skill shape: the DIRECTORY entry itself is a link,
    // pointing at a real skill tree outside the repo. Following it would offer
    // an item whose content is not the source's, and which cannot install
    // (LIFE-42 refuses a symlinked item root), so the offer could never be
    // honored.
    let sb = Sandbox::new();
    write(
        &sb.base.join("outside/realskill/SKILL.md"),
        "---\ndescription: SECRETDESCRIPTION from outside the source\n---\n",
    );
    std::fs::create_dir_all(sb.source.join("skills")).unwrap();
    std::os::unix::fs::symlink(
        sb.base.join("outside/realskill"),
        sb.source.join("skills/leak"),
    )
    .unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a source carrying a symlinked skill dir must still meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        !listing.contains("leak") && !listing.contains("SECRETDESCRIPTION"),
        "a symlinked skill directory must not be discovered as an item: {listing}"
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_kind_container_is_not_listed() {
    // The per-entry no-follow check runs only after the container is listed, so
    // a `workflows/` that is itself a link would walk a directory outside the
    // source and offer its files as items.
    let sb = Sandbox::new();
    write(
        &sb.base.join("outside/wf/leak.js"),
        "export const meta = { name: 'leak', description: 'SECRETDESCRIPTION' };\n",
    );
    std::fs::remove_dir_all(sb.source.join("workflows")).unwrap();
    std::os::unix::fs::symlink(sb.base.join("outside/wf"), sb.source.join("workflows")).unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a source with a symlinked container must still meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        !listing.contains("leak") && !listing.contains("SECRETDESCRIPTION"),
        "a symlinked workflows/ container must not be listed: {listing}"
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_tool_directory_is_not_an_item() {
    // A tool needs no anchor file at all: the directory IS the item, so the
    // directory classification is the whole test of whether it exists. Same
    // no-follow rule, same reason.
    let sb = Sandbox::new();
    write(
        &sb.base.join("outside/realtool/TOOL.md"),
        "---\ndescription: SECRETDESCRIPTION from outside the source\n---\n",
    );
    std::fs::create_dir_all(sb.source.join("tools")).unwrap();
    std::os::unix::fs::symlink(
        sb.base.join("outside/realtool"),
        sb.source.join("tools/leak"),
    )
    .unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a source carrying a symlinked tool dir must still meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        !listing.contains("leak") && !listing.contains("SECRETDESCRIPTION"),
        "a symlinked tool directory must not be discovered as an item: {listing}"
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_discover_glob_does_not_reach_a_symlinked_entry_either() {
    // `[discover]` globs never pass through the flat scan, so the no-follow
    // rule has to hold in the glob expansion too or the authoritative layer is
    // a second route to the same oracle: `workflows/*.js` matches a symlink by
    // NAME, and the match is what the item is built from.
    //
    // A link whose target resolves OUTSIDE the repo is already a hard DSC-81
    // refusal (covered separately below); the two uncovered cases are the ones
    // that stay lexically inside, where nothing but the classification decides:
    // a link to a real file in the repo, and a dangling link (whose target
    // cannot be canonicalized, so DSC-81's check has nothing to reject).
    let sb = Sandbox::new();
    write(
        &sb.source.join("src/real-impl.js"),
        "export const meta = { name: 'leak', description: 'Inside the repo' };\n",
    );
    std::os::unix::fs::symlink(
        sb.source.join("src/real-impl.js"),
        sb.source.join("workflows/leak.js"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        sb.base.join("nowhere/absent.js"),
        sb.source.join("workflows/dangling.js"),
    )
    .unwrap();
    write(
        &sb.source.join("mind.toml"),
        "[discover]\nworkflows = { include = [\"workflows/*.js\"] }\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!(
        "{}{}{}{}",
        meld.stdout, meld.stderr, probe.stdout, probe.stderr
    );
    assert!(
        probe.success,
        "the glob scan must not fail on a symlinked match: {listing}"
    );
    assert!(
        listing.contains("ship"),
        "the real globbed workflow must still be discovered: {listing}"
    );
    assert!(
        !listing.contains("leak") && !listing.contains("dangling"),
        "a globbed symlink must not be discovered as an item: {listing}"
    );
}

// spec: DSC-81
#[cfg(unix)]
#[test]
fn a_discover_glob_matching_an_escaping_symlink_is_still_a_hard_refusal() {
    // The DSC-108 filter must not soften DSC-81: a glob match whose link
    // resolves outside the repo root is the author's own pattern reaching out of
    // the clone, and that stays a named error rather than becoming a silent
    // skip. Pinned because the two rules meet in one loop and the order between
    // them is what decides.
    let sb = Sandbox::new();
    write(
        &sb.base.join("outside/private.js"),
        "export const meta = { name: 'leak', description: 'Outside' };\n",
    );
    std::os::unix::fs::symlink(
        sb.base.join("outside/private.js"),
        sb.source.join("workflows/leak.js"),
    )
    .unwrap();
    write(
        &sb.source.join("mind.toml"),
        "[discover]\nworkflows = { include = [\"workflows/*.js\"] }\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    let combined = format!("{}{}", meld.stdout, meld.stderr);
    assert!(
        !meld.success,
        "a glob match resolving outside the repo root must be refused: {combined}"
    );
    assert!(
        combined.contains("outside the repo root"),
        "the refusal must say the match escaped the root: {combined}"
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_workflow_pointing_outside_the_source_is_not_an_item() {
    // The scan runs over a tree the source controls entirely. Classifying an
    // entry by following its link would answer questions about the CONSUMER's
    // filesystem: whether a path exists (the item appears or does not) and how
    // big the file there is (`review`'s workflow size report). Both answers
    // have to come from the repo's own entries, so a symlink is not an item
    // whatever it points at.
    let sb = Sandbox::new();

    // A real, readable `.js` outside the source repo: the scan must not reach
    // it even though following the link plainly would.
    let outside = sb.base.join("outside/private.js");
    write(
        &outside,
        "export const meta = { name: 'private', description: 'Not the source\\'s file' };\n",
    );
    std::os::unix::fs::symlink(&outside, sb.source.join("workflows/leak.js")).unwrap();
    // The same shape one kind over, to prove the rule is the flat scan's and
    // not something specific to the workflow kind.
    let outside_md = sb.base.join("outside/private.md");
    write(
        &outside_md,
        "---\ndescription: Not the source's file\n---\n",
    );
    std::fs::create_dir_all(sb.source.join("agents")).unwrap();
    std::os::unix::fs::symlink(&outside_md, sb.source.join("agents/leak.md")).unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a source carrying a symlinked entry must still meld: {} {}",
        meld.stdout, meld.stderr
    );

    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "probe must succeed: {}", probe.stderr);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        listing.contains("ship"),
        "the real workflow must still be discovered: {listing}"
    );
    assert!(
        !listing.contains("leak"),
        "a symlinked entry must not be discovered as an item: {listing}"
    );
    assert!(
        !listing.contains("private"),
        "nothing from outside the source root may reach the catalog: {listing}"
    );

    // And the name is not installable either: it is not in the catalog at all.
    let learn = sb.mind(&["learn", "workflow:leak"]);
    assert!(
        !learn.success,
        "a symlinked entry must not be installable: {} {}",
        learn.stdout, learn.stderr
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_symlinked_workflow_with_a_dangling_target_is_not_an_item_either() {
    // The existence oracle is the point: whether the link resolves must not
    // change the item listing, or the listing answers "does this path exist on
    // your machine" one entry at a time.
    let sb = Sandbox::new();
    std::os::unix::fs::symlink(
        sb.base.join("nowhere/absent.js"),
        sb.source.join("workflows/probe-existence.js"),
    )
    .unwrap();
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a dangling symlink must not fail the meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    let listing = format!("{}{}", probe.stdout, probe.stderr);
    assert!(
        !listing.contains("probe-existence"),
        "a dangling symlink must not be discovered as an item: {listing}"
    );
}

// spec: DSC-109
#[test]
fn a_declared_workflow_whose_path_is_a_directory_is_refused() {
    // A workflow IS one `.js` file, and that file is also the metadata the
    // scan reads. A directory has nothing to read, would install a tree at a
    // link target the harness tries to load as a script, and would report a
    // directory's size in `review`. The author wrote the path, so it is a hard
    // refusal rather than a skipped item.
    let sb = Sandbox::new();
    write(
        &sb.source.join("bundle/index.js"),
        "export const meta = { name: 'bundle', description: 'A bundle' };\n",
    );
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"bundle\"\npath = \"bundle\"\n",
    );
    sb.commit();

    // The refusal lands wherever the source is first scanned: at the meld
    // itself, or at the first scanning verb after a register-only meld. Either
    // is the scan-time refusal DSC-109 describes; what must not happen is the
    // entry being catalogued.
    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        !meld.success || !probe.success,
        "a directory-shaped workflow entry must be refused at scan time: meld={} {} probe={} {}",
        meld.stdout,
        meld.stderr,
        probe.stdout,
        probe.stderr
    );
    let combined = format!(
        "{}{}{}{}",
        meld.stdout, meld.stderr, probe.stdout, probe.stderr
    );
    assert!(
        combined.contains("bundle"),
        "the refusal must name the offending item: {combined}"
    );
    assert!(
        combined.contains("directory"),
        "the refusal must say the path is a directory: {combined}"
    );
    assert!(
        combined.contains(".js") || combined.contains("single"),
        "the refusal must say what a workflow path has to name: {combined}"
    );
}

// spec: DSC-109
#[test]
fn a_declared_workflow_whose_path_is_a_file_still_works() {
    // Control: the check is about the shape of the path, not about declaring a
    // workflow in `[[items]]` at all.
    let sb = Sandbox::new();
    write(
        &sb.source.join("bundle/deploy.js"),
        "export const meta = { name: 'deploy', description: 'Deploy it' };\n",
    );
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"deploy\"\npath = \"bundle/deploy.js\"\n",
    );
    sb.commit();

    let r = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        r.success,
        "a file-shaped workflow entry must meld: {} {}",
        r.stdout, r.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(
        probe.stdout.contains("deploy"),
        "the declared workflow must be catalogued: {}",
        probe.stdout
    );
}

// spec: DSC-108 DSC-109
#[cfg(unix)]
#[test]
fn a_declared_workflow_symlinked_to_a_directory_is_dropped_at_scan() {
    // DSC-109 classifies no-follow, so a symlink is NOT a directory to it. The
    // DSC-108 guard on declared paths then drops the symlink at scan with a
    // warning: it is never catalogued, so there is nothing to install.
    let sb = Sandbox::new();
    write(
        &sb.source.join("real/index.js"),
        "export const meta = { name: 'bundle', description: 'A bundle' };\n",
    );
    std::os::unix::fs::symlink(sb.source.join("real"), sb.source.join("bundle")).unwrap();
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"bundle\"\npath = \"bundle\"\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a symlinked declared path is dropped, not a failed meld: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains("symlink"),
        "the drop must be reported, naming the symlink: {}",
        meld.stderr
    );
    let learn = sb.mind(&["learn", "workflow:bundle", "--yes"]);
    assert!(
        !learn.success,
        "a dropped entry is not in the catalog, so it cannot install: {} {}",
        learn.stdout, learn.stderr
    );
    // Nothing linked into the lobe, not even a dangling link.
    assert!(
        std::fs::symlink_metadata(sb.claude_home.join("workflows/bundle.js")).is_err(),
        "a dropped entry must leave no link behind"
    );
}

// spec: DSC-108
#[cfg(unix)]
#[test]
fn a_declared_workflow_symlinked_to_an_outside_file_is_dropped_unread() {
    // `[[items]]` names a path by hand, so without a no-follow check it is a
    // third route (after the convention scan and `[discover]` globs) to read
    // a file on the consumer's machine into the catalog and report its size.
    let sb = Sandbox::new();
    let outside = sb.base.join("outside/probe.js");
    // Padded to a distinctive byte count, so a size report would show it.
    let mut body =
        String::from("export const meta = { name: 'probe', description: 'SECRET-DECL' };\n");
    while body.len() < 4321 {
        body.push('/');
    }
    body.truncate(4321);
    write(&outside, &body);
    std::os::unix::fs::symlink(&outside, sb.source.join("workflows/probe.js")).unwrap();
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"ship\"\npath = \"workflows/ship.js\"\n\n\
         [[items]]\nkind = \"workflow\"\nname = \"probe\"\npath = \"workflows/probe.js\"\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "a symlinked declared path is dropped, not a failed meld: {} {}",
        meld.stdout, meld.stderr
    );
    assert!(
        meld.stderr.contains("warning") && meld.stderr.contains("symlink"),
        "the drop must be warned about: {}",
        meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "probe must succeed: {}", probe.stderr);
    assert!(
        probe.stdout.contains("workflow:ship"),
        "the real declared workflow is still offered: {}",
        probe.stdout
    );
    assert!(
        !probe.stdout.contains("workflow:probe"),
        "a symlinked declared workflow must not be offered: {}",
        probe.stdout
    );
    let review = sb.mind(&["review", &sb.source_spec()]);
    let combined = format!(
        "{}{}{}{}",
        meld.stdout, meld.stderr, review.stdout, review.stderr
    );
    assert!(
        !combined.contains("SECRET-DECL"),
        "the outside file's content must never be read: {combined}"
    );
    assert!(
        !combined.contains("4321"),
        "the outside file's size must never be reported: {combined}"
    );
}

// spec: DSC-73
#[cfg(unix)]
#[test]
fn a_declared_path_through_a_symlinked_parent_dir_is_a_hard_error() {
    // The final component is a regular file, so the no-follow check passes it;
    // only the canonical containment check sees that a symlinked PARENT carries
    // it out of the clone. The author wrote the path, so it is refused outright,
    // like an escaping `[discover]` match (DSC-81).
    let sb = Sandbox::new();
    write(
        &sb.base.join("outside/a.md"),
        "---\ndescription: SECRET-PARENT\n---\n",
    );
    std::os::unix::fs::symlink(sb.base.join("outside"), sb.source.join("linked")).unwrap();
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"agent\"\nname = \"a\"\npath = \"linked/a.md\"\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    let probe = sb.mind(&["probe", "--no-tui"]);
    let combined = format!(
        "{}{}{}{}",
        meld.stdout, meld.stderr, probe.stdout, probe.stderr
    );
    assert!(
        !meld.success || !probe.success,
        "an escaping declared path must be refused at scan: {combined}"
    );
    assert!(
        combined.contains("resolves outside the repo root"),
        "the refusal must say the path escaped the root: {combined}"
    );
    assert!(
        !combined.contains("SECRET-PARENT"),
        "the outside file must not be read: {combined}"
    );
}

// spec: DSC-108 DSC-84
#[cfg(unix)]
#[test]
fn an_add_root_skills_container_classifies_symlinks_no_follow() {
    // The `--add-root` containered `skills/` pass is its own loop, separate
    // from the convention scan, so it needs the same no-follow classification:
    // a linked skill dir or a linked SKILL.md anchor would read a file outside
    // the clone into the catalog.
    let sb = Sandbox::new();
    write(
        &sb.source.join("extra/skills/real/SKILL.md"),
        "---\ndescription: the source's own skill\n---\n",
    );
    write(
        &sb.base.join("outside/linked/SKILL.md"),
        "---\ndescription: SECRET-ADDROOT\n---\n",
    );
    write(
        &sb.base.join("outside/anchor.md"),
        "---\ndescription: SECRET-ADDROOT anchor\n---\n",
    );
    std::os::unix::fs::symlink(
        sb.base.join("outside/linked"),
        sb.source.join("extra/skills/linked"),
    )
    .unwrap();
    std::fs::create_dir_all(sb.source.join("extra/skills/anchor")).unwrap();
    std::os::unix::fs::symlink(
        sb.base.join("outside/anchor.md"),
        sb.source.join("extra/skills/anchor/SKILL.md"),
    )
    .unwrap();
    sb.commit();

    let meld = sb.mind(&[
        "meld",
        &sb.source_spec(),
        "--add-root",
        "extra",
        "--register-only",
    ]);
    assert!(
        meld.success,
        "a source with symlinked add-root entries must still meld: {} {}",
        meld.stdout, meld.stderr
    );
    let probe = sb.mind(&["probe", "--no-tui"]);
    assert!(probe.success, "probe must succeed: {}", probe.stderr);
    assert!(
        probe.stdout.contains("skill:real"),
        "the add-root's own skill is offered: {}",
        probe.stdout
    );
    let combined = format!(
        "{}{}{}{}",
        meld.stdout, meld.stderr, probe.stdout, probe.stderr
    );
    assert!(
        !combined.contains("linked") && !combined.contains("skill:anchor"),
        "a symlinked skill dir or anchor must not be discovered: {combined}"
    );
    assert!(
        !combined.contains("SECRET-ADDROOT"),
        "no linked-to file's description may reach the catalog: {combined}"
    );
}

// spec: DSC-109
#[test]
fn a_declared_workflow_whose_path_does_not_exist_fails_at_install_not_at_scan() {
    // The shape check is about a path that exists and is the wrong KIND of
    // thing. A path that names nothing at all is the ordinary
    // declared-but-absent case every kind shares: it is not a scan failure (the
    // scan makes no existence claim about a declared path), and the install is
    // where it is reported, naming the path.
    let sb = Sandbox::new();
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"ghost\"\npath = \"workflows/ghost.js\"\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(
        meld.success,
        "an absent declared path must not fail the scan: {} {}",
        meld.stdout, meld.stderr
    );
    let learn = sb.mind(&["learn", "workflow:ghost", "--yes"]);
    assert!(
        !learn.success,
        "installing an absent declared workflow must fail: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        format!("{}{}", learn.stdout, learn.stderr).contains("ghost.js"),
        "the failure must name the path that is missing: {} {}",
        learn.stdout,
        learn.stderr
    );
}

// spec: WF-10
#[test]
fn a_declared_workflow_may_carry_a_non_js_source_extension_and_still_links_as_js() {
    // The extension rule (WF-3) governs the CONVENTION scan's discovery, not
    // what an author may declare: a declared entry names its own file, and the
    // link is built from the kind and the effective name, so it is `.js` in the
    // lobe whatever the file is called in the repo. Pinned because the DSC-109
    // shape check sits right next to it and must not grow into an extension
    // check by accident.
    let sb = Sandbox::new();
    write(
        &sb.source.join("src/deploy.ts"),
        "export const meta = { name: 'deploy', description: 'Deploy it' };\n",
    );
    write(
        &sb.source.join("mind.toml"),
        "[[items]]\nkind = \"workflow\"\nname = \"deploy\"\npath = \"src/deploy.ts\"\n",
    );
    sb.commit();

    let meld = sb.mind(&["meld", &sb.source_spec(), "--register-only"]);
    assert!(meld.success, "must meld: {} {}", meld.stdout, meld.stderr);
    let learn = sb.mind(&["learn", "workflow:deploy", "--yes"]);
    assert!(
        learn.success,
        "a declared workflow with a non-.js source path must install: {} {}",
        learn.stdout, learn.stderr
    );
    assert!(
        sb.claude_home.join("workflows/deploy.js").exists(),
        "the lobe link must be `.js` regardless of the source file's extension"
    );
}
