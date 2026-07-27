//! Integration tests for item links (spec/item-link.md, LNK-*): a deep
//! tree/blob URL to one skill inside a repo, consumed as its own single-item
//! source instance.
//!
//! Each test drives the real `mind` binary against a hermetic, network-free
//! fixture: a local git repo addressed through the `file://` link form
//! (LNK-1), with MIND_HOME/CLAUDE_HOME pointed at a temp dir.

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
    /// A source repo with two convention skills (`review`, `extra`).
    fn new() -> Sandbox {
        let sb = Sandbox::bare("agents");
        sb.write_and_commit(
            "skills/review/SKILL.md",
            "---\ndescription: Review the diff for bugs\n---\n# review skill\n",
        );
        sb.write_and_commit(
            "skills/extra/SKILL.md",
            "---\ndescription: A second skill\n---\n# extra skill\n",
        );
        sb
    }

    fn bare(name: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("mind-lnk-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let source = base.join(name);
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

    fn mind(&self, args: &[&str]) -> Run {
        self.mind_env(args, &[])
    }

    fn mind_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mind"));
        cmd.args(args)
            .env("MIND_HOME", &self.mind_home)
            .env("CLAUDE_HOME", &self.claude_home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run mind");
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

    /// A `file://` item link into this sandbox's source repo (LNK-1).
    fn link(&self, tail: &str) -> String {
        format!("file://{}/{tail}", self.source.to_string_lossy())
    }

    /// The registered identity of a link instance for `path` (LNK-4):
    /// `local/<base>/<repo>#<path>`.
    fn link_name(&self, path: &str) -> String {
        format!(
            "local/{}/{}#{path}",
            self.base.file_name().unwrap().to_string_lossy(),
            self.source.file_name().unwrap().to_string_lossy(),
        )
    }

    /// HEAD commit sha of the source repo.
    fn head_sha(&self) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.source)
            .output()
            .expect("git rev-parse");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
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

/// Count the melded sources by reading sources.json (0 when absent).
fn source_count(sb: &Sandbox) -> usize {
    let path = sb.mind_home.join("sources.json");
    let Ok(json) = std::fs::read_to_string(&path) else {
        return 0;
    };
    json.matches("\"url\"").count()
}

#[test]
fn learn_url_installs_the_single_linked_skill() {
    // spec: LNK-6 LNK-7
    // `learn <url>` one-shots: registers the link instance and installs its
    // skill. Only the linked skill is offered/installed; the repo's other
    // skill is untouched.
    let sb = Sandbox::new();
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn <url> failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the linked skill must be installed"
    );
    assert!(
        !sb.claude_home.join("skills/extra").exists(),
        "the repo's other skill must NOT be installed"
    );
    assert_eq!(
        source_count(&sb),
        1,
        "exactly one source instance registered"
    );

    // The instance's catalog is exactly the linked skill (LNK-7).
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:review") && !probe.stdout.contains("skill:extra"),
        "only the linked skill is offered: {}",
        probe.stdout
    );
}

#[test]
fn learn_url_again_is_an_up_to_date_noop() {
    // spec: LNK-6 LNK-4
    // Re-supplying an already-registered link re-enters the standard flow
    // (CLI-12/CLI-157): nothing re-clones, nothing errors.
    let sb = Sandbox::new();
    let url = sb.link("tree/main/skills/review");
    assert!(sb.mind(&["learn", &url]).success);
    let r = sb.mind(&["learn", &url]);
    assert!(r.success, "second learn <url> failed: {}", r.stderr);
    assert!(
        r.stdout.contains("already installed"),
        "second learn must be the up-to-date no-op: {}",
        r.stdout
    );
    assert_eq!(source_count(&sb), 1, "no duplicate instance registered");
}

#[test]
fn meld_url_registers_without_installing() {
    // spec: LNK-6
    // `meld <url> --register-only` follows the standard meld flow: the
    // instance registers and its skill is offered, not installed.
    let sb = Sandbox::new();
    let r = sb.mind(&[
        "meld",
        &sb.link("tree/main/skills/review"),
        "--register-only",
    ]);
    assert!(r.success, "meld <url> failed: {} {}", r.stdout, r.stderr);
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "--register-only must not install"
    );
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:review"),
        "the linked skill must be offered: {}",
        probe.stdout
    );
}

#[test]
fn blob_link_to_skill_md_installs() {
    // spec: LNK-1
    // The blob form names the SKILL.md itself; the skill directory is its
    // parent.
    let sb = Sandbox::new();
    let r = sb.mind(&["learn", &sb.link("blob/main/skills/review/SKILL.md")]);
    assert!(r.success, "blob learn failed: {} {}", r.stdout, r.stderr);
    assert!(sb.claude_home.join("skills/review").exists());
}

#[test]
fn link_reaches_a_skill_the_marketplace_manifest_does_not_list() {
    // spec: LNK-7
    // The repo ships a marketplace.json that lists one plugin; the linked
    // skill is outside it. The link bypasses the manifest's authority.
    let sb = Sandbox::bare("mkt");
    sb.write_and_commit(
        ".claude-plugin/marketplace.json",
        r#"{"name":"Cat","plugins":[{"name":"kit","source":"./plugins/kit"}]}"#,
    );
    sb.write_and_commit(
        "plugins/kit/skills/foo/SKILL.md",
        "---\ndescription: listed\n---\n# foo\n",
    );
    sb.write_and_commit(
        "community/hidden/SKILL.md",
        "---\ndescription: unlisted\n---\n# hidden\n",
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/community/hidden")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/hidden").exists(),
        "the unlisted skill must install via its link"
    );
    // The manifest's plugin was not melded as a side effect (LNK-8).
    assert_eq!(source_count(&sb), 1, "only the link instance registers");
}

#[test]
fn link_instances_and_a_plain_meld_coexist() {
    // spec: LNK-4 LNK-12
    // Two links into the same repo, plus the repo itself (namespaced to avoid
    // an item collision), are three distinct registered sources.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/extra")])
            .success
    );
    let spec = sb.source.to_string_lossy().into_owned();
    let r = sb.mind(&["meld", &spec, "--namespace", "full", "--register-only"]);
    assert!(r.success, "plain meld failed: {} {}", r.stdout, r.stderr);
    assert_eq!(source_count(&sb), 3, "instances and the repo are distinct");

    // recall --sources shows each instance under its #-suffixed identity.
    let sources = sb.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("#skills/review") && sources.stdout.contains("#skills/extra"),
        "link identities must be visible: {}",
        sources.stdout
    );
}

#[test]
fn item_link_composes_with_a_consumer_alias() {
    // spec: STO-58 LNK-4 -- `--namespace` composes with an item link: the
    // instance identity is `host/owner/repo#<path>@<alias>` and its skill installs
    // under the prefix, coexisting with an unaliased link into the same path.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success,
        "bare link install"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "unprefixed link skill installs"
    );

    let r = sb.mind(&[
        "meld",
        &sb.link("tree/main/skills/review"),
        "--namespace",
        "jk",
        "--yes",
    ]);
    assert!(r.success, "aliased link meld: {} {}", r.stdout, r.stderr);
    assert!(
        sb.claude_home.join("skills/jk:review").exists(),
        "aliased link installs its skill under the prefix"
    );
    assert!(
        sb.claude_home.join("skills/review").exists(),
        "the bare link's skill survives"
    );
    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("#skills/review@jk"),
        "the aliased link identity must be visible: {sources}"
    );
    assert_eq!(
        source_count(&sb),
        2,
        "the bare and aliased link instances coexist"
    );
}

#[test]
fn link_without_skill_md_is_an_error_and_registers_nothing() {
    // spec: LNK-7
    let sb = Sandbox::new();
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/nope")]);
    assert!(!r.success, "a linkless path must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("not a skill directory"),
        "the error must say the path is not a skill: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn branch_link_upgrades_with_the_branch() {
    // spec: LNK-5
    // A tree/<branch> link follows that branch: sync + upgrade pick up an
    // upstream edit to the linked skill.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);
    let r = sb.mind(&["upgrade", "--yes"]);
    assert!(r.success, "upgrade failed: {} {}", r.stdout, r.stderr);
    let installed = std::fs::read_to_string(sb.claude_home.join("skills/review/SKILL.md")).unwrap();
    assert!(
        installed.contains("edited"),
        "the installed skill must carry the upstream edit: {installed}"
    );
}

#[test]
fn sha_link_pins_and_does_not_follow() {
    // spec: LNK-3
    // A tree/<40-hex> link is a commit pin: an upstream edit does not reach
    // the installed skill through sync + upgrade.
    let sb = Sandbox::new();
    let sha = sb.head_sha();
    let r = sb.mind(&["learn", &sb.link(&format!("tree/{sha}/skills/review"))]);
    assert!(r.success, "sha learn failed: {} {}", r.stdout, r.stderr);
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);
    assert!(sb.mind(&["upgrade", "--yes"]).success);
    let installed = std::fs::read_to_string(sb.claude_home.join("skills/review/SKILL.md")).unwrap();
    assert!(
        !installed.contains("edited"),
        "a sha-pinned link must not follow the branch: {installed}"
    );
}

#[test]
fn learn_pin_freezes_a_branch_link() {
    // spec: CLI-200, LNK-3
    // `learn <branch-link> --pin` freezes the link's branch ref to its current
    // commit, so an upstream edit does not reach the installed skill through
    // sync + upgrade (the branch-following default would, cf.
    // branch_link_upgrades_with_the_branch).
    let sb = Sandbox::new();
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review"), "--pin"]);
    assert!(
        r.success,
        "pinned link learn failed: {} {}",
        r.stdout, r.stderr
    );
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff for bugs\n---\n# review skill\nedited\n",
    );
    assert!(sb.mind(&["sync"]).success);
    assert!(sb.mind(&["upgrade", "--yes"]).success);
    let installed = std::fs::read_to_string(sb.claude_home.join("skills/review/SKILL.md")).unwrap();
    assert!(
        !installed.contains("edited"),
        "a --pin frozen branch link must not follow the branch: {installed}"
    );
}

#[test]
fn unmeld_link_instance_uninstalls_its_skill() {
    // spec: LNK-5
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    let name = sb.link_name("skills/review");
    let r = sb.mind(&["unmeld", &name, "--yes"]);
    assert!(r.success, "unmeld failed: {} {}", r.stdout, r.stderr);
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "unmeld must uninstall the instance's skill"
    );
    assert_eq!(source_count(&sb), 0);
}

#[test]
fn forget_of_an_emptied_link_hints_at_unmeld() {
    // spec: LNK-5
    // forget leaves the instance registered and points at unmeld.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    let r = sb.mind(&["forget", "review"]);
    assert!(r.success, "forget failed: {} {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("mind unmeld"),
        "forget must hint at unmeld for an emptied link instance: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 1, "the instance stays registered");
}

#[test]
fn curated_sources_entry_can_be_an_item_link() {
    // spec: LNK-2
    // A [discover].sources entry may be a deep link; the curator's meld
    // registers the link instance (register-only, DSC-54).
    let lib = Sandbox::new();
    let curator = Sandbox::bare("curator");
    curator.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            lib.link("tree/main/skills/review")
        ),
    );
    let spec = curator.source.to_string_lossy().into_owned();
    let r = curator.mind(&["meld", &spec, "--register-only"]);
    assert!(r.success, "curator meld failed: {} {}", r.stdout, r.stderr);
    let sources = curator.mind(&["recall", "--sources"]);
    assert!(
        sources.stdout.contains("#skills/review"),
        "the curated link instance must register: {}",
        sources.stdout
    );
}

#[test]
fn link_registers_no_nested_sources() {
    // spec: LNK-8
    // The linked repo curates another source; a link into it must not walk
    // that curator layer.
    let other = Sandbox::new();
    let sb = Sandbox::new();
    sb.write_and_commit(
        "mind.toml",
        &format!(
            "[discover]\nsources = [{{ source = \"{}\" }}]\n",
            other.source.to_string_lossy()
        ),
    );
    let r = sb.mind(&["learn", &sb.link("tree/main/skills/review")]);
    assert!(r.success, "learn failed: {} {}", r.stdout, r.stderr);
    assert_eq!(
        source_count(&sb),
        1,
        "the linked repo's [discover].sources must not be walked"
    );
}

#[test]
fn meld_url_with_namespace_prefixes_the_skill() {
    // spec: LNK-9
    let sb = Sandbox::new();
    let r = sb.mind(&[
        "meld",
        &sb.link("tree/main/skills/review"),
        "--namespace",
        "pfx",
        "--register-only",
    ]);
    assert!(r.success, "meld failed: {} {}", r.stdout, r.stderr);
    let probe = sb.mind(&["probe"]);
    assert!(
        probe.stdout.contains("skill:pfx:review"),
        "the link's skill must carry the namespace: {}",
        probe.stdout
    );
}

#[test]
fn policy_allowlist_matches_the_base_repo_identity() {
    // spec: LNK-11
    // The allow pattern names the repo (no #path); a link into it melds under
    // lock. Matching against the extended identity would refuse it.
    let sb = Sandbox::new();
    let policy_path = sb.base.join("policy.toml");
    write(
        &policy_path,
        "[sources]\nlock = true\nallow = [\"local/*/agents\"]\n",
    );
    let policy = policy_path.to_string_lossy().into_owned();
    let r = sb.mind_env(
        &["learn", &sb.link("tree/main/skills/review")],
        &[("MIND_POLICY_FILE", policy.as_str())],
    );
    assert!(
        r.success,
        "an allowed repo must allow links into it: {} {}",
        r.stdout, r.stderr
    );
    // And a repo outside the allowlist stays refused for links too.
    let other = Sandbox::new();
    let deny_path = other.base.join("policy.toml");
    write(
        &deny_path,
        "[sources]\nlock = true\nallow = [\"local/*/other\"]\n",
    );
    let deny = deny_path.to_string_lossy().into_owned();
    let r = other.mind_env(
        &["learn", &other.link("tree/main/skills/review")],
        &[("MIND_POLICY_FILE", deny.as_str())],
    );
    assert!(
        !r.success,
        "a non-allowed repo must refuse links: {}",
        r.stdout
    );
}

#[test]
fn learn_blob_url_not_ending_in_skill_md_reports_bad_item_link() {
    // spec: LNK-14
    // A blob URL that does not end in /SKILL.md carries a tree/blob marker,
    // so it is an attempted item link that failed to parse: the error must
    // name the expected shapes, not the generic invalid-repo-spec message.
    let sb = Sandbox::new();
    let url = sb.link("blob/main/skills/review");
    let r = sb.mind(&["learn", &url]);
    assert!(!r.success, "a malformed blob link must fail: {}", r.stdout);
    assert!(
        r.stderr.contains(&url),
        "the error must name the offending URL: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("tree/<ref>") && r.stderr.contains("blob/<ref>"),
        "the error must name the expected link shapes: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("not a valid repo spec"),
        "the error must not fall back to the generic repo-spec message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn learn_tree_url_with_no_path_reports_bad_item_link() {
    // spec: LNK-14
    // A tree URL missing its skill-directory path is likewise an attempted
    // item link that failed to parse.
    let sb = Sandbox::new();
    let url = sb.link("tree/main");
    let r = sb.mind(&["learn", &url]);
    assert!(!r.success, "a pathless tree link must fail: {}", r.stdout);
    assert!(
        r.stderr.contains(&url),
        "the error must name the offending URL: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("tree/<ref>") && r.stderr.contains("blob/<ref>"),
        "the error must name the expected link shapes: {}",
        r.stderr
    );
    assert!(
        !r.stderr.contains("not a valid repo spec"),
        "the error must not fall back to the generic repo-spec message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn meld_plain_bad_spec_keeps_the_generic_invalid_repo_spec_message() {
    // spec: LNK-14
    // A bad spec with no tree/blob marker at all is not an attempted item
    // link, so it must keep reporting the generic invalid-repo-spec message,
    // unaffected by the LNK-14 routing. `meld` (unlike `learn`, which only
    // parses a spec as a repo/link when it contains "://") always parses its
    // argument through the same repo-spec parser (source.rs::parse_spec).
    let sb = Sandbox::new();
    let r = sb.mind(&["meld", "notarealspec"]);
    assert!(!r.success, "a malformed spec must fail: {}", r.stdout);
    assert!(
        r.stderr.contains("not a valid repo spec"),
        "a marker-less bad spec must keep the generic message: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn json_error_envelope_kind_is_bad_item_link() {
    // spec: LNK-14, CLI-181, CLI-182
    // Under --json, a malformed item-link URL (tree/blob marker present, tail
    // fails to parse) must emit the standard error envelope
    // {"schema":1,"error":{"kind":"bad-item-link","message":"..."}} on stdout,
    // not the generic "invalid-repo-spec" kind.
    let sb = Sandbox::new();
    let url = sb.link("tree/main");
    let r = sb.mind(&["learn", &url, "--json"]);
    assert!(!r.success, "a pathless tree link must fail: {}", r.stdout);
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON ({e}): {:?}", r.stdout));
    assert_eq!(v["schema"], 1, "schema must be 1: {}", r.stdout);
    let err = &v["error"];
    assert_eq!(
        err["kind"], "bad-item-link",
        "kind must be bad-item-link, not the generic invalid-repo-spec: {}",
        r.stdout
    );
    let msg = err["message"].as_str().unwrap_or("");
    assert!(
        msg.contains(&url),
        "message must name the offending URL: {msg}"
    );
    assert!(
        !r.stderr.contains("error:"),
        "main error handler must not write to stderr under --json: {}",
        r.stderr
    );
    assert_eq!(source_count(&sb), 0, "nothing registered on failure");
}

#[test]
fn link_instances_at_different_refs_keep_independent_commits_after_sync() {
    // spec: STO-70
    // Two links into the SAME repo at different item paths, pinned to
    // DIFFERENT branches, must not clobber each other's clone. This is the
    // C31 bug: before STO-70, `clone_dir` ignored `item_path`, so both
    // instances (and a plain meld of the repo) resolved to the identical
    // clone directory; syncing one instance's branch checkout onto that
    // shared directory would silently discard the other's. The existing
    // `link_instances_and_a_plain_meld_coexist` test above uses `tree/main`
    // for both links, so even under the old bug both instances would read
    // identical content post-clobber and the bug stayed invisible; pinning
    // the two instances to DIFFERENT branches with DIFFERENT content makes a
    // clobber observable.
    let sb = Sandbox::new(); // skills/review, skills/extra on `main`.
    git(&sb.source, &["checkout", "-b", "other"]);
    sb.write_and_commit(
        "skills/extra/SKILL.md",
        "---\ndescription: A second skill\n---\n# extra skill\non the other branch\n",
    );
    git(&sb.source, &["checkout", "main"]);

    // review is pinned to `main`; extra is pinned to `other`.
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/other/skills/extra")])
            .success
    );

    let extra_initial =
        std::fs::read_to_string(sb.claude_home.join("skills/extra/SKILL.md")).unwrap();
    assert!(
        extra_initial.contains("on the other branch"),
        "extra must install from `other`'s content: {extra_initial}"
    );

    // Diverge both branches further, independently.
    git(&sb.source, &["checkout", "main"]);
    sb.write_and_commit(
        "skills/review/SKILL.md",
        "---\ndescription: Review the diff for bugs\n---\n# review skill\nmain edit\n",
    );
    git(&sb.source, &["checkout", "other"]);
    sb.write_and_commit(
        "skills/extra/SKILL.md",
        "---\ndescription: A second skill\n---\n# extra skill\nother edit\n",
    );
    git(&sb.source, &["checkout", "main"]);

    assert!(sb.mind(&["sync"]).success, "sync must succeed");
    let up = sb.mind(&["upgrade", "--yes"]);
    assert!(up.success, "upgrade failed: {} {}", up.stdout, up.stderr);

    let review_final =
        std::fs::read_to_string(sb.claude_home.join("skills/review/SKILL.md")).unwrap();
    let extra_final =
        std::fs::read_to_string(sb.claude_home.join("skills/extra/SKILL.md")).unwrap();
    assert!(
        review_final.contains("main edit"),
        "review must reflect main's own edit: {review_final}"
    );
    assert!(
        !review_final.contains("other edit"),
        "review must not be clobbered by other's content: {review_final}"
    );
    assert!(
        extra_final.contains("other edit"),
        "extra must reflect other's own edit: {extra_final}"
    );
    assert!(
        !extra_final.contains("main edit"),
        "extra must not be clobbered by main's content: {extra_final}"
    );
}

#[test]
fn unmeld_of_one_link_instance_leaves_the_others_clone_and_skill_intact() {
    // spec: STO-70
    // Before STO-70, two non-aliased link instances into the same repo shared
    // one clone directory; `unmeld`'s cleanup of one instance's clone would
    // remove that shared directory out from under the other, breaking its
    // installed skill and any future sync of it.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/extra")])
            .success
    );

    let name_review = sb.link_name("skills/review");
    let r = sb.mind(&["unmeld", &name_review, "--yes"]);
    assert!(r.success, "unmeld failed: {} {}", r.stdout, r.stderr);

    // The unmelded instance's skill is gone...
    assert!(
        !sb.claude_home.join("skills/review").exists(),
        "the unmelded instance's skill must be uninstalled"
    );
    // ...but the surviving instance's skill and registration are untouched.
    assert!(
        sb.claude_home.join("skills/extra").exists(),
        "the surviving instance's installed skill must remain"
    );
    let extra_content =
        std::fs::read_to_string(sb.claude_home.join("skills/extra/SKILL.md")).unwrap();
    assert!(extra_content.contains("A second skill"));

    let sources = sb.mind(&["recall", "--sources"]).stdout;
    assert!(
        sources.contains("#skills/extra"),
        "the surviving instance must stay registered: {sources}"
    );
    assert!(
        !sources.contains("#skills/review"),
        "the unmelded instance must be gone: {sources}"
    );

    // The surviving instance's clone directory itself -- keyed on its own
    // encoded item_path, per the STO-70 leaf formula -- must still be present
    // on disk.
    let survivor_clone = sb
        .mind_home
        .join("sources")
        .join("local")
        .join(sb.base.file_name().unwrap())
        .join(format!(
            "{}#skills%2Fextra",
            sb.source.file_name().unwrap().to_string_lossy()
        ));
    assert!(
        survivor_clone.join(".git").is_dir(),
        "the surviving instance's own clone dir must remain: {survivor_clone:?}"
    );

    // And it must still be fully usable: a further sync succeeds.
    let r2 = sb.mind(&["sync"]);
    assert!(
        r2.success,
        "sync after unmeld must still succeed: {} {}",
        r2.stdout, r2.stderr
    );
}

#[test]
fn dump_emits_a_reconstructed_link_entry_not_a_skip_note() {
    // spec: LNK-13
    // `dump` now emits a reconstructed deep-URL entry for a link instance
    // (round-tripped in tests/cli_dump.rs), rather than skipping it with a
    // note. This pins that basic behavior from the item-link side: the entry
    // is present and no "skipping item link" note is printed.
    let sb = Sandbox::new();
    assert!(
        sb.mind(&["learn", &sb.link("tree/main/skills/review")])
            .success
    );
    let r = sb.mind(&["dump"]);
    assert!(r.success, "dump failed: {} {}", r.stdout, r.stderr);
    assert!(
        !r.stderr.contains("skipping item link"),
        "dump must no longer skip an item-link instance: {}",
        r.stderr
    );
    assert!(
        r.stdout.contains("skills/review"),
        "an entry reconstructing the link instance must be emitted: {}",
        r.stdout
    );
}
